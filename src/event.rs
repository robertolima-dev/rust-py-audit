use serde::{Deserialize, Serialize};

use crate::immutablelog_receipt::ImmutableLogReceipt;

/// Um evento de auditoria, já com seu hash calculado.
///
/// `#[derive(...)]` pede ao compilador para gerar implementações
/// automáticas de alguns traits:
/// - `Serialize` / `Deserialize` (serde): permitem transformar este
///   struct em JSON (`serde_json::to_string`) e voltar (`from_str`).
///   É a base de tudo que vamos fazer no `storage.rs` (gravar/ler JSONL).
/// - `Debug`: permite usar `{:?}` para imprimir o struct (útil em testes
///   e mensagens de erro).
/// - `Clone`: o struct pode ser duplicado explicitamente com `.clone()`.
///   Não é automático/implícito como em outras linguagens — em Rust,
///   por padrão, atribuir ou passar um valor *move* a posse dele; só
///   conseguimos copiar se o tipo implementar `Clone` e chamarmos isso
///   de forma explícita. Vamos precisar disso para, por exemplo, manter
///   uma cópia do evento depois de o gravar no arquivo.
/// - `PartialEq`: permite comparar dois eventos com `==`, usado nos
///   testes (Etapa 5 em diante) e no `verifier.rs`.
///
/// IMPORTANTE para o hash determinístico: a ordem dos campos abaixo é
/// exatamente a ordem em que eles serão serializados no JSON quando
/// chamamos `serde_json::to_string(&event)` diretamente sobre o struct
/// (diferente de passar por um `serde_json::Value` solto, cuja ordem de
/// chaves dependeria da implementação do mapa). Não reordene os campos
/// sem recalcular/migrar hashes já gravados.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub app_name: String,
    pub actor_id: String,
    pub action: String,
    pub resource: String,
    pub resource_id: String,
    /// Livre, pois cada aplicação registra metadados diferentes (IP,
    /// motivo, request_id, etc). `serde_json::Value` representa
    /// "qualquer JSON válido": objeto, array, string, número...
    pub metadata: serde_json::Value,
    /// `Option<String>`: o primeiro evento da cadeia não tem evento
    /// anterior, então não existe um `previous_hash` válido. Em Rust não
    /// temos `null`/`None` implícito como em Python — para representar
    /// "pode não ter valor" usamos o tipo `Option<T>`, que é um enum com
    /// duas variantes: `Some(valor)` ou `None`. O compilador *obriga* a
    /// tratar os dois casos sempre que lemos esse campo, o que elimina
    /// de vez a classe de bug "esqueci de checar null".
    pub previous_hash: Option<String>,
    pub hash: String,
    /// Classificação (`error`/`warning`/`info`/`success`) usada em
    /// `meta.type` ao enviar para o ImmutableLog. Guardado aqui (e não
    /// só passado direto pra `immutablelog_client.rs`) para que
    /// `flush_pending()` consiga reenviar mais tarde com a MESMA
    /// classificação original. Puramente operacional, igual
    /// `immutablelog` abaixo: não entra no hash, e fica omitido do JSON
    /// quando ninguém passa `severity` em `log()` — mantém o formato
    /// local idêntico ao de antes desta opção existir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Agrupador opcional (`meta.immutable_trail`), já sanitizado (ver
    /// `immutablelog_client::sanitize_trail`). Mesma lógica de
    /// `severity`: guardado para `flush_pending()`, omitido por padrão.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_trail: Option<String>,
    /// Receipt remoto do ImmutableLog (ou `status: "pending"` enquanto
    /// não confirmado). Campo puramente operacional: vem DEPOIS de
    /// `hash` na ordem dos campos e `hash.rs::HashPayload` não o lista,
    /// então ele nunca entra no cálculo do hash — anexar/atualizar este
    /// campo não invalida `verify()`. `skip_serializing_if` mantém o
    /// JSON idêntico ao formato atual quando não há receipt (modo
    /// `local`, ou eventos gravados antes desta versão), e
    /// `#[serde(default)]` permite ler arquivos antigos sem o campo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutablelog: Option<ImmutableLogReceipt>,
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn serializes_fields_in_declaration_order() {
        let event = sample_event();

        let json = serde_json::to_string(&event).expect("serialização não deveria falhar");

        // A ordem das chaves no JSON precisa bater com a ordem dos
        // campos do struct — é essa estabilidade que torna o hash
        // determinístico (Etapa 5).
        let expected = r#"{"id":"evt_123","timestamp":"2026-06-17T10:00:00Z","app_name":"billing-api","actor_id":"user_123","action":"DELETE_INVOICE","resource":"invoice","resource_id":"inv_987","metadata":{"ip":"192.168.0.10"},"previous_hash":null,"hash":"abc123"}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn previous_hash_some_serializes_as_string() {
        let mut event = sample_event();
        event.previous_hash = Some("def456".to_string());

        let json = serde_json::to_string(&event).expect("serialização não deveria falhar");

        assert!(json.contains(r#""previous_hash":"def456""#));
    }

    #[test]
    fn round_trips_through_json() {
        let event = sample_event();

        let json = serde_json::to_string(&event).expect("serialização não deveria falhar");
        let parsed: AuditEvent = serde_json::from_str(&json).expect("desserialização não deveria falhar");

        assert_eq!(event, parsed);
    }
}
