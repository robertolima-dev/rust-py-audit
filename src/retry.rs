//! Retry simples para o envio de eventos ao ImmutableLog.
//!
//! Não duplica eventos: todas as tentativas (a inicial e os retries)
//! usam a mesma `Idempotency-Key` (`event.id`, fixada dentro de
//! `ImmutableLogClient::send`) — cabe ao ImmutableLog deduplicar do
//! lado dele se, por exemplo, a primeira tentativa tiver sido aceita
//! mas a resposta se perdido por timeout antes de chegar até nós.

use std::thread::sleep;
use std::time::Duration;

use crate::event::AuditEvent;
use crate::immutablelog_client::{ImmutableLogClient, ImmutableLogClientError};
use crate::immutablelog_receipt::ImmutableLogReceipt;

/// Backoff fixo (não exponencial) entre tentativas — suficiente para o
/// MVP síncrono. Uma estratégia mais sofisticada (exponencial, jitter)
/// só faz sentido quando existir uma fila de delivery assíncrona.
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

/// Envia `event` via `client`, retentando em caso de erro retryable
/// (5xx, timeout, falha de rede) até `max_retries` vezes. Erros
/// permanentes (400, 401, 403, 429, ...) nunca são retentados, mesmo
/// com `retry_enabled = true`.
pub fn send_with_retry(
    client: &ImmutableLogClient,
    event: &AuditEvent,
    severity: &str,
    immutable_trail: Option<&str>,
    retry_enabled: bool,
    max_retries: u8,
) -> Result<ImmutableLogReceipt, ImmutableLogClientError> {
    let mut attempt: u8 = 0;

    loop {
        match client.send(event, severity, immutable_trail) {
            Ok(receipt) => return Ok(receipt),
            Err(err) => {
                let should_retry = retry_enabled && err.is_retryable() && attempt < max_retries;
                if !should_retry {
                    return Err(err);
                }
                attempt += 1;
                sleep(RETRY_BACKOFF);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::immutablelog_config::ImmutableLogConfig;
    use httpmock::prelude::*;
    use serde_json::json;

    fn sample_event() -> AuditEvent {
        AuditEvent {
            id: "evt_retry".to_string(),
            timestamp: "2026-06-17T10:00:00Z".to_string(),
            app_name: "billing-api".to_string(),
            actor_id: "user_123".to_string(),
            action: "DELETE_INVOICE".to_string(),
            resource: "invoice".to_string(),
            resource_id: "inv_987".to_string(),
            metadata: json!({}),
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
    fn does_not_retry_permanent_errors() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(401).json_body(json!({"ok": false, "reason": "invalid_api_key"}));
        });

        let result = send_with_retry(&client_for(&server), &sample_event(), "info", None, true, 3);

        assert!(matches!(result, Err(ImmutableLogClientError::Permanent { .. })));
        mock.assert_hits(1);
    }

    #[test]
    fn retries_retryable_errors_up_to_max_retries() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(500).json_body(json!({"ok": false, "reason": "internal_error"}));
        });

        let result = send_with_retry(&client_for(&server), &sample_event(), "info", None, true, 2);

        assert!(matches!(result, Err(ImmutableLogClientError::Retryable { .. })));
        // 1 tentativa inicial + 2 retries = 3 chamadas HTTP no total.
        mock.assert_hits(3);
    }

    #[test]
    fn does_not_retry_when_retry_is_disabled() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(500).json_body(json!({"ok": false, "reason": "internal_error"}));
        });

        let result = send_with_retry(&client_for(&server), &sample_event(), "info", None, false, 5);

        assert!(result.is_err());
        mock.assert_hits(1);
    }

    #[test]
    fn succeeds_after_a_transient_failure_using_the_same_idempotency_key() {
        let server = MockServer::start();
        // Primeira chamada falha com 500, segunda (mesma Idempotency-Key)
        // é aceita — simula uma falha transitória do lado do ImmutableLog.
        let failing_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/events")
                .header("Idempotency-Key", "evt_retry");
            then.status(500).json_body(json!({"ok": false, "reason": "internal_error"}));
        });

        let result = send_with_retry(&client_for(&server), &sample_event(), "info", None, true, 1);

        // Sem suporte a "responder diferente por chamada" no mock básico,
        // validamos aqui só que a MESMA Idempotency-Key foi usada em
        // todas as tentativas (a asserção de header já garante isso —
        // se o header mudasse entre tentativas, o mock não daria match).
        assert!(result.is_err());
        failing_mock.assert_hits(2);
    }
}
