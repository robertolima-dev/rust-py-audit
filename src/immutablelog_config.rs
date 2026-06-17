//! Configuração do modo de operação do `AuditLogger` e dos parâmetros
//! de entrega ao ImmutableLog.
//!
//! Este módulo só guarda/valida configuração — nenhuma requisição HTTP
//! acontece aqui. O cliente HTTP (`immutablelog_client.rs`) e o envio
//! de eventos (Etapas 4+) ainda vão ser implementados separadamente,
//! depois de confirmar o formato real aceito pela API do ImmutableLog.

use crate::errors::AuditError;

/// Modo de operação do `AuditLogger`.
///
/// `Copy` porque é só uma tag pequena (sem heap), sem motivo para exigir
/// `.clone()` explícito em todo lugar que precisa ler o modo atual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditMode {
    /// Comportamento atual da lib: só grava localmente em JSONL,
    /// mantém a cadeia de hashes local. Nunca contata o ImmutableLog.
    Local,
    /// Envia eventos para o ImmutableLog. Não grava JSONL local (exceto
    /// fila de falha, quando essa parte for implementada).
    Remote,
    /// Grava localmente em JSONL E envia para o ImmutableLog, guardando
    /// o receipt remoto junto ao evento local.
    Hybrid,
}

impl AuditMode {
    /// Converte a string vinda do Python (ou de `RUST_PY_AUDIT_MODE`)
    /// no enum. Erro claro para qualquer valor fora de
    /// `local`/`remote`/`hybrid`, em vez de aceitar silenciosamente
    /// algo inesperado.
    pub fn parse(value: &str) -> Result<Self, AuditError> {
        match value {
            "local" => Ok(AuditMode::Local),
            "remote" => Ok(AuditMode::Remote),
            "hybrid" => Ok(AuditMode::Hybrid),
            other => Err(AuditError::Config(format!(
                "mode inválido: '{other}' (use 'local', 'remote' ou 'hybrid')"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AuditMode::Local => "local",
            AuditMode::Remote => "remote",
            AuditMode::Hybrid => "hybrid",
        }
    }

    pub fn sends_to_immutablelog(&self) -> bool {
        matches!(self, AuditMode::Remote | AuditMode::Hybrid)
    }

    pub fn saves_local_jsonl(&self) -> bool {
        matches!(self, AuditMode::Local | AuditMode::Hybrid)
    }
}

/// Parâmetros de conexão com o ImmutableLog. Existe mesmo em `mode =
/// Local` (com `url`/`api_key` como `None`) para manter um único lugar
/// de leitura de configuração — quem decide se esses campos são
/// obrigatórios é `ImmutableLogConfig::validate_for`, não quem chama.
#[derive(Debug, Clone)]
pub struct ImmutableLogConfig {
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
    pub retry_enabled: bool,
    pub max_retries: u8,
    /// Ambiente lógico (`"production"`, `"staging"`, ...), enviado em
    /// `meta.env`. Puramente informativo do lado do ImmutableLog — `None`
    /// omite o campo (a doc pública trata `env` como opcional).
    pub env: Option<String>,
}

impl ImmutableLogConfig {
    /// Resolve `url`/`api_key`/`env` a partir do que foi passado
    /// explicitamente (prioridade) ou das variáveis de ambiente
    /// `IMMUTABLELOG_URL` / `IMMUTABLELOG_API_KEY` / `IMMUTABLELOG_ENV`
    /// (fallback).
    pub fn resolve(
        url: Option<String>,
        api_key: Option<String>,
        timeout_ms: u64,
        retry_enabled: bool,
        max_retries: u8,
        env: Option<String>,
    ) -> Self {
        ImmutableLogConfig {
            url: url.or_else(|| std::env::var("IMMUTABLELOG_URL").ok()),
            api_key: api_key.or_else(|| std::env::var("IMMUTABLELOG_API_KEY").ok()),
            timeout_ms,
            retry_enabled,
            max_retries,
            env: env.or_else(|| std::env::var("IMMUTABLELOG_ENV").ok()),
        }
    }

    /// Garante que `url`/`api_key` existem quando o modo exige contato
    /// com o ImmutableLog. Falhar aqui, na construção do `AuditLogger`,
    /// é melhor do que falhar mais tarde dentro de `log()`.
    pub fn validate_for(&self, mode: AuditMode) -> Result<(), AuditError> {
        if !mode.sends_to_immutablelog() {
            return Ok(());
        }

        if self.url.is_none() || self.api_key.is_none() {
            return Err(AuditError::Config(format!(
                "mode='{}' exige immutablelog_url e immutablelog_api_key \
                 (via parâmetro ou variáveis de ambiente IMMUTABLELOG_URL / IMMUTABLELOG_API_KEY)",
                mode.as_str()
            )));
        }

        Ok(())
    }
}

/// Lê `RUST_PY_AUDIT_MODE` do ambiente, usado como fallback quando o
/// parâmetro `mode` não é passado explicitamente ao `AuditLogger`.
pub fn mode_from_env_or(explicit: Option<String>, default: &str) -> String {
    explicit
        .or_else(|| std::env::var("RUST_PY_AUDIT_MODE").ok())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_modes() {
        assert_eq!(AuditMode::parse("local").unwrap(), AuditMode::Local);
        assert_eq!(AuditMode::parse("remote").unwrap(), AuditMode::Remote);
        assert_eq!(AuditMode::parse("hybrid").unwrap(), AuditMode::Hybrid);
    }

    #[test]
    fn rejects_unknown_mode() {
        let result = AuditMode::parse("turbo");
        assert!(matches!(result, Err(AuditError::Config(_))));
    }

    #[test]
    fn local_mode_does_not_require_immutablelog_config() {
        let config = ImmutableLogConfig::resolve(None, None, 500, true, 3, None);
        assert!(config.validate_for(AuditMode::Local).is_ok());
    }

    #[test]
    fn remote_mode_requires_url_and_api_key() {
        let config = ImmutableLogConfig {
            url: None,
            api_key: None,
            timeout_ms: 500,
            retry_enabled: true,
            max_retries: 3,
            env: None,
        };

        let result = config.validate_for(AuditMode::Remote);
        assert!(matches!(result, Err(AuditError::Config(_))));
    }

    #[test]
    fn hybrid_mode_passes_when_url_and_api_key_are_set() {
        let config = ImmutableLogConfig {
            url: Some("https://api.immutablelog.com".to_string()),
            api_key: Some("iml_live_xxx".to_string()),
            timeout_ms: 500,
            retry_enabled: true,
            max_retries: 3,
            env: None,
        };

        assert!(config.validate_for(AuditMode::Hybrid).is_ok());
    }
}
