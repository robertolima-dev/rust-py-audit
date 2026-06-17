//! Receipt remoto devolvido pelo ImmutableLog após o envio de um evento.
//!
//! Campos confirmados na resposta real (`POST /v1/events`, 202
//! Accepted, conforme https://immutablelog.com/en/documentation/):
//! `tx_id`, `payload_hash`, `status`, `duplicate`, `request_id`.
//! `block_id`/`block_hash`/`event_hash` NÃO existem na doc pública atual
//! — ficam como `Option<String>` só para não quebrar se uma versão
//! futura da API passar a devolvê-los.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ImmutableLogReceipt {
    /// Status da entrega do PONTO DE VISTA do rust-py-audit:
    /// `"delivered"` (confirmado pelo ImmutableLog) ou `"pending"`
    /// (ainda não confirmado — aguardando `flush_pending()`).
    pub status: String,
    pub tx_id: Option<String>,
    pub payload_hash: Option<String>,
    pub duplicate: Option<bool>,
    pub request_id: Option<String>,
    pub remote_timestamp: Option<String>,
    /// Valor literal do campo `status` devolvido pela API do
    /// ImmutableLog (ex.: `"accepted"`) — distinto do nosso próprio
    /// `status` (`"delivered"`/`"pending"`), que descreve a entrega do
    /// ponto de vista do rust-py-audit.
    pub remote_status: Option<String>,
    /// Não documentados publicamente — reservados para compatibilidade
    /// futura, sempre `None` com a API atual.
    pub block_id: Option<String>,
    pub block_hash: Option<String>,
    pub event_hash: Option<String>,
}

impl ImmutableLogReceipt {
    pub fn pending() -> Self {
        ImmutableLogReceipt {
            status: "pending".to_string(),
            ..Default::default()
        }
    }
}
