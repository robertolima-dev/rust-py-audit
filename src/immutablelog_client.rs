//! Cliente HTTP para o endpoint de ingestão do ImmutableLog.
//!
//! Formato confirmado em https://immutablelog.com/en/documentation/
//! (NÃO é o que aparecia no rascunho original do pedido — lá não existe
//! `/events` genérico, e a resposta não tem `block_id`/`block_hash`/
//! `event_hash`):
//!
//! - `POST {base_url}/v1/events`
//! - body: `{"payload": "<AuditEvent serializado como STRING JSON>",
//!   "meta": {"type", "event_name", "service", "trace_id"}}`
//! - resposta 202: `{"ok", "tx_id", "payload_hash", "status",
//!   "duplicate", "request_id"}`
//! - erro: `{"ok": false, "reason": "..."}`
//!
//! Este módulo é Rust puro (sem `pyo3`): não tem acesso ao GIL nem
//! precisa dele. Quem chama `send()` a partir de `audit_logger.rs` é
//! responsável por liberar o GIL (`Python::allow_threads`) durante a
//! chamada, já que é uma operação de rede bloqueante.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::AuditEvent;
use crate::immutablelog_config::ImmutableLogConfig;
use crate::immutablelog_receipt::ImmutableLogReceipt;

const USER_AGENT: &str = concat!("rust-py-audit/", env!("CARGO_PKG_VERSION"));

/// Erro ao tentar enviar um evento ao ImmutableLog.
///
/// A distinção `Permanent`/`Retryable` é o que `retry.rs` (próxima
/// etapa) usa para decidir se vale a pena tentar de novo:
/// - `Permanent`: 400 (payload inválido), 401/403 (credenciais
///   inválidas), 429 (cota mensal esgotada — retentar em segundos não
///   resolve) ou qualquer outro 4xx. Retentar não vai mudar o
///   resultado.
/// - `Retryable`: 5xx (problema do lado do ImmutableLog), timeout, ou
///   falha de rede/conexão. Pode ser transitório.
#[derive(Debug)]
pub enum ImmutableLogClientError {
    Permanent { status: Option<u16>, reason: String },
    Retryable { status: Option<u16>, reason: String },
}

impl ImmutableLogClientError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, ImmutableLogClientError::Retryable { .. })
    }
}

impl std::fmt::Display for ImmutableLogClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImmutableLogClientError::Permanent { status, reason } => {
                write!(f, "ImmutableLog rejeitou o evento (status={status:?}): {reason}")
            }
            ImmutableLogClientError::Retryable { status, reason } => {
                write!(f, "falha ao contatar o ImmutableLog (status={status:?}): {reason}")
            }
        }
    }
}

impl std::error::Error for ImmutableLogClientError {}

/// Valores aceitos por `meta.type`, conforme a doc do ImmutableLog.
pub const ALLOWED_SEVERITIES: [&str; 4] = ["error", "warning", "info", "success"];

/// Valida `severity` (se vier do chamador) contra `ALLOWED_SEVERITIES`.
/// `None` cai no default `"info"` — mesmo default de antes desta opção
/// existir, então nenhum código que não passa `severity` é afetado.
///
/// Devolve `String` (e não `ImmutableLogClientError`) porque isso é
/// validação de ARGUMENTO do chamador, não falha de entrega — quem
/// chama (`audit_logger.rs`) converte para `AuditError::Config`, que
/// vira `ValueError` no Python, não `RuntimeError`.
pub fn resolve_severity(severity: Option<&str>) -> Result<&'static str, String> {
    match severity {
        None => Ok("info"),
        Some(value) => ALLOWED_SEVERITIES.iter().find(|allowed| **allowed == value).copied().ok_or_else(|| {
            format!("severity inválida: '{value}' (use uma de {ALLOWED_SEVERITIES:?})")
        }),
    }
}

/// Normaliza `immutable_trail` conforme as regras do ImmutableLog: sem
/// espaços nas pontas, não-vazio, no máximo 256 caracteres, sem `:`
/// (substituído por `-`). Retorna `None` se o valor for inválido/vazio
/// depois da normalização — o campo é omitido em vez de enviado quebrado.
pub fn sanitize_trail(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }

    let sanitized = trimmed.replace(':', "-");
    Some(if sanitized.len() > 256 {
        sanitized.chars().take(256).collect()
    } else {
        sanitized
    })
}

#[derive(Serialize)]
struct EventMeta<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    event_name: &'a str,
    service: &'a str,
    trace_id: &'a str,
    request_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    immutable_trail: Option<&'a str>,
}

#[derive(Serialize)]
struct IngestRequest<'a> {
    payload: String,
    meta: EventMeta<'a>,
}

#[derive(Deserialize)]
struct IngestResponse {
    tx_id: Option<String>,
    payload_hash: Option<String>,
    status: Option<String>,
    duplicate: Option<bool>,
    request_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct ErrorResponse {
    reason: Option<String>,
}

pub struct ImmutableLogClient {
    http: reqwest::blocking::Client,
    base_url: String,
    api_key: String,
    env: Option<String>,
}

impl ImmutableLogClient {
    pub fn new(config: &ImmutableLogConfig) -> Result<Self, ImmutableLogClientError> {
        let base_url = config.url.clone().ok_or_else(|| ImmutableLogClientError::Permanent {
            status: None,
            reason: "immutablelog_url não configurado".to_string(),
        })?;
        let api_key = config.api_key.clone().ok_or_else(|| ImmutableLogClientError::Permanent {
            status: None,
            reason: "immutablelog_api_key não configurado".to_string(),
        })?;

        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|err| ImmutableLogClientError::Retryable {
                status: None,
                reason: format!("falha ao montar cliente HTTP: {err}"),
            })?;

        Ok(ImmutableLogClient { http, base_url, api_key, env: config.env.clone() })
    }

    /// Envia um evento já com hash local calculado. `event.immutablelog`
    /// precisa ser `None` neste ponto — o que vai no corpo da
    /// requisição (`payload`) é o evento local inteiro serializado
    /// (id, timestamp, hash, previous_hash, metadata, etc.), exatamente
    /// como ficou gravado em `audit.jsonl`. O envio nunca recalcula nem
    /// altera esse hash.
    ///
    /// `severity` já deve vir validada (ver `resolve_severity`) — este
    /// método não revalida. `immutable_trail` já deve vir sanitizado
    /// (ver `sanitize_trail`) ou `None`.
    pub fn send(
        &self,
        event: &AuditEvent,
        severity: &str,
        immutable_trail: Option<&str>,
    ) -> Result<ImmutableLogReceipt, ImmutableLogClientError> {
        let payload = serde_json::to_string(event).map_err(|err| ImmutableLogClientError::Permanent {
            status: None,
            reason: format!("falha ao serializar evento para envio: {err}"),
        })?;

        let endpoint = format!("{}/v1/events", self.base_url.trim_end_matches('/'));
        let request_id = Uuid::new_v4().to_string();

        let body = IngestRequest {
            payload,
            meta: EventMeta {
                kind: severity,
                event_name: &event.action,
                service: &event.app_name,
                trace_id: &event.id,
                request_id: &request_id,
                env: self.env.as_deref(),
                immutable_trail,
            },
        };

        let response = self
            .http
            .post(&endpoint)
            .bearer_auth(&self.api_key)
            .header("Idempotency-Key", &event.id)
            .header("Request-Id", &request_id)
            .header("User-Agent", USER_AGENT)
            .json(&body)
            .send()
            .map_err(|err| ImmutableLogClientError::Retryable {
                status: None,
                reason: if err.is_timeout() {
                    "timeout ao contatar o ImmutableLog".to_string()
                } else {
                    err.to_string()
                },
            })?;

        let status = response.status();

        if status.is_success() {
            let parsed: IngestResponse = response.json().map_err(|err| ImmutableLogClientError::Retryable {
                status: Some(status.as_u16()),
                reason: format!("resposta inesperada do ImmutableLog: {err}"),
            })?;

            return Ok(ImmutableLogReceipt {
                status: "delivered".to_string(),
                tx_id: parsed.tx_id,
                payload_hash: parsed.payload_hash,
                duplicate: parsed.duplicate,
                request_id: parsed.request_id,
                remote_timestamp: None,
                remote_status: parsed.status,
                block_id: None,
                block_hash: None,
                event_hash: None,
            });
        }

        let status_code = status.as_u16();
        let reason = response
            .json::<ErrorResponse>()
            .ok()
            .and_then(|body| body.reason)
            .unwrap_or_else(|| format!("HTTP {status_code}"));

        if status.is_server_error() {
            Err(ImmutableLogClientError::Retryable { status: Some(status_code), reason })
        } else {
            Err(ImmutableLogClientError::Permanent { status: Some(status_code), reason })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn sample_event() -> AuditEvent {
        AuditEvent {
            id: "evt_123".to_string(),
            timestamp: "2026-06-17T10:00:00Z".to_string(),
            app_name: "billing-api".to_string(),
            actor_id: "user_123".to_string(),
            action: "DELETE_INVOICE".to_string(),
            resource: "invoice".to_string(),
            resource_id: "inv_987".to_string(),
            metadata: json!({"ip": "192.168.0.10"}),
            previous_hash: None,
            hash: "abc123".to_string(),
            severity: None,
            immutable_trail: None,
            immutablelog: None,
        }
    }

    fn client_for(server: &MockServer) -> ImmutableLogClient {
        let config = ImmutableLogConfig {
            url: Some(server.base_url()),
            api_key: Some("iml_live_test".to_string()),
            timeout_ms: 2000,
            retry_enabled: true,
            max_retries: 3,
            env: None,
        };
        ImmutableLogClient::new(&config).expect("client deveria ser criado")
    }

    #[test]
    fn send_maps_a_202_response_to_a_delivered_receipt() {
        let server = MockServer::start();
        let event = sample_event();

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/events")
                .header("Authorization", "Bearer iml_live_test")
                .header("Idempotency-Key", "evt_123")
                .json_body_partial(r#"{"meta": {"event_name": "DELETE_INVOICE", "service": "billing-api", "trace_id": "evt_123", "type": "info"}}"#);
            then.status(202).json_body(json!({
                "ok": true,
                "tx_id": "tx_abc",
                "payload_hash": "sha256deadbeef",
                "status": "accepted",
                "duplicate": false,
                "request_id": "req_xyz"
            }));
        });

        let receipt = client_for(&server).send(&event, "info", None).expect("envio deveria ter sucesso");

        mock.assert();
        assert_eq!(receipt.status, "delivered");
        assert_eq!(receipt.tx_id, Some("tx_abc".to_string()));
        assert_eq!(receipt.payload_hash, Some("sha256deadbeef".to_string()));
        assert_eq!(receipt.duplicate, Some(false));
        assert_eq!(receipt.request_id, Some("req_xyz".to_string()));
        assert_eq!(receipt.remote_status, Some("accepted".to_string()));
    }

    #[test]
    fn send_treats_401_as_permanent_error() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(401).json_body(json!({"ok": false, "reason": "invalid_api_key"}));
        });

        let result = client_for(&server).send(&sample_event(), "info", None);

        mock.assert();
        match result {
            Err(ImmutableLogClientError::Permanent { status, reason }) => {
                assert_eq!(status, Some(401));
                assert_eq!(reason, "invalid_api_key");
            }
            other => panic!("esperava Permanent, recebeu {other:?}"),
        }
    }

    #[test]
    fn send_treats_400_as_permanent_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(400).json_body(json!({"ok": false, "reason": "invalid_payload"}));
        });

        let result = client_for(&server).send(&sample_event(), "info", None);

        assert!(matches!(result, Err(ImmutableLogClientError::Permanent { .. })));
    }

    #[test]
    fn send_treats_500_as_retryable_error() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(500).json_body(json!({"ok": false, "reason": "internal_error"}));
        });

        let result = client_for(&server).send(&sample_event(), "info", None);

        mock.assert();
        match result {
            Err(ImmutableLogClientError::Retryable { status, .. }) => assert_eq!(status, Some(500)),
            other => panic!("esperava Retryable, recebeu {other:?}"),
        }
    }

    #[test]
    fn send_treats_429_as_permanent_error() {
        // Cota mensal esgotada não se resolve com retry imediato.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(429)
                .json_body(json!({"ok": false, "reason": "monthly_limit_exceeded"}));
        });

        let result = client_for(&server).send(&sample_event(), "info", None);

        assert!(matches!(result, Err(ImmutableLogClientError::Permanent { .. })));
    }

    #[test]
    fn resolve_severity_defaults_to_info_when_absent() {
        assert_eq!(resolve_severity(None).unwrap(), "info");
    }

    #[test]
    fn resolve_severity_accepts_all_documented_values() {
        for value in ALLOWED_SEVERITIES {
            assert_eq!(resolve_severity(Some(value)).unwrap(), value);
        }
    }

    #[test]
    fn resolve_severity_rejects_unknown_value() {
        let result = resolve_severity(Some("critical"));
        assert!(result.is_err());
    }

    #[test]
    fn sanitize_trail_passes_through_a_clean_value() {
        assert_eq!(sanitize_trail(Some("order-2026-00441")), Some("order-2026-00441".to_string()));
    }

    #[test]
    fn sanitize_trail_trims_and_replaces_colons() {
        assert_eq!(sanitize_trail(Some("  order:2026:00441  ")), Some("order-2026-00441".to_string()));
    }

    #[test]
    fn sanitize_trail_omits_blank_values() {
        assert_eq!(sanitize_trail(Some("   ")), None);
        assert_eq!(sanitize_trail(None), None);
    }

    #[test]
    fn sanitize_trail_truncates_at_256_chars() {
        let long_value = "a".repeat(300);
        let sanitized = sanitize_trail(Some(&long_value)).unwrap();
        assert_eq!(sanitized.len(), 256);
    }

    #[test]
    fn send_includes_severity_trail_env_and_request_id_in_meta() {
        let server = MockServer::start();
        let config = ImmutableLogConfig {
            url: Some(server.base_url()),
            api_key: Some("iml_live_test".to_string()),
            timeout_ms: 2000,
            retry_enabled: true,
            max_retries: 3,
            env: Some("staging".to_string()),
        };
        let client = ImmutableLogClient::new(&config).expect("client deveria ser criado");

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/events")
                .json_body_partial(
                    r#"{"meta": {"type": "error", "env": "staging", "immutable_trail": "order-42"}}"#,
                )
                .matches(|req| {
                    let body: serde_json::Value =
                        serde_json::from_slice(req.body.as_deref().unwrap_or(&[])).unwrap();
                    body["meta"]["request_id"].as_str().is_some_and(|value| !value.is_empty())
                });
            then.status(202).json_body(json!({
                "ok": true,
                "tx_id": "tx_meta",
                "payload_hash": "hash_meta",
                "status": "accepted",
                "duplicate": false,
                "request_id": "req_meta"
            }));
        });

        client.send(&sample_event(), "error", Some("order-42")).expect("envio deveria ter sucesso");

        mock.assert();
    }

    #[test]
    fn send_omits_env_and_immutable_trail_when_not_configured() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events").matches(|req| {
                let body: serde_json::Value = serde_json::from_slice(req.body.as_deref().unwrap_or(&[])).unwrap();
                let meta = &body["meta"];
                meta.get("env").is_none() && meta.get("immutable_trail").is_none()
            });
            then.status(202).json_body(json!({
                "ok": true,
                "tx_id": "tx_default",
                "payload_hash": "hash_default",
                "status": "accepted",
                "duplicate": false,
                "request_id": "req_default"
            }));
        });

        client_for(&server).send(&sample_event(), "info", None).expect("envio deveria ter sucesso");

        mock.assert();
    }
}
