use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::errors::AuditError;
use crate::event::AuditEvent;

/// Representa apenas os campos de um `AuditEvent` que entram no cálculo
/// do hash — ou seja, todo mundo MENOS o próprio campo `hash` (faria
/// sentido um hash que depende de si mesmo?).
///
/// Por que um struct novo em vez de reusar `AuditEvent` direto? Porque
/// `AuditEvent` tem o campo `hash`, e se serializássemos o evento
/// inteiro (incluindo um `hash` ainda vazio ou antigo) ele entraria na
/// conta — o que tornaria o hash inconsistente dependendo de quando ele
/// é calculado. Separando os campos aqui, deixamos explícito o que
/// exatamente compõe a "impressão digital" do evento.
///
/// Os campos são todos referências (`&'a str`, etc.), não cópias. Isso
/// é "borrowing": em vez de assumir a posse (`String`) dos dados que já
/// existem dentro do `AuditEvent` original, apenas pedimos um empréstimo
/// de leitura. O parâmetro de lifetime `'a` é o jeito do compilador
/// rastrear "essas referências são válidas no máximo até quando os
/// dados originais (`event: &'a AuditEvent`) também forem" — ele garante,
/// em tempo de compilação, que `HashPayload` nunca vai apontar para
/// memória já liberada. Tudo isso sem custo em tempo de execução: é
/// só uma verificação estática, não existe coleta de lixo nem contagem
/// de referências rodando por baixo.
#[derive(Serialize)]
struct HashPayload<'a> {
    id: &'a str,
    timestamp: &'a str,
    app_name: &'a str,
    actor_id: &'a str,
    action: &'a str,
    resource: &'a str,
    resource_id: &'a str,
    metadata: &'a serde_json::Value,
    previous_hash: &'a Option<String>,
}

/// Calcula o hash SHA-256 determinístico de um evento.
///
/// Recebe `&AuditEvent` (uma referência) em vez de `AuditEvent` (o valor
/// inteiro): essa função só precisa *ler* os campos para montar o
/// payload, nunca precisa ser dona do evento. Pedir só uma referência
/// evita uma cópia inteira da struct (incluindo o `metadata`, que pode
/// ser um JSON arbitrariamente grande) só para calcular um hash.
///
/// Retorna `Result<String, AuditError>` em vez de `String` porque a
/// serialização para JSON, embora rarissimamente falhe para os tipos
/// que usamos aqui, ainda é uma operação que *pode* falhar (por
/// exemplo, em teoria, com tipos não suportados). Capturar isso como
/// `Result` evita qualquer `.unwrap()`/`.expect()` que faria o processo
/// inteiro (o processo Python que importou a lib!) abortar com panic.
pub fn compute_hash(event: &AuditEvent) -> Result<String, AuditError> {
    let payload = HashPayload {
        id: &event.id,
        timestamp: &event.timestamp,
        app_name: &event.app_name,
        actor_id: &event.actor_id,
        action: &event.action,
        resource: &event.resource,
        resource_id: &event.resource_id,
        metadata: &event.metadata,
        previous_hash: &event.previous_hash,
    };

    // `to_vec` em vez de `to_string` para gerar bytes diretamente — o
    // SHA-256 trabalha sobre bytes, então evitamos um passo extra de
    // converter String -> &[u8].
    let canonical_bytes = serde_json::to_vec(&payload)
        .map_err(|err| AuditError::Serialization(err.to_string()))?;

    let digest = Sha256::digest(&canonical_bytes);
    // "{:x}" formata os bytes do digest como hexadecimal minúsculo,
    // que é o formato de hash usado no exemplo de evento (64 caracteres).
    Ok(format!("{digest:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_event() -> AuditEvent {
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
            hash: String::new(),
        }
    }

    #[test]
    fn produces_a_64_char_hex_digest() {
        let hash = compute_hash(&base_event()).expect("hash não deveria falhar");

        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn is_deterministic_for_identical_content() {
        let hash_a = compute_hash(&base_event()).expect("hash não deveria falhar");
        let hash_b = compute_hash(&base_event()).expect("hash não deveria falhar");

        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn changes_when_any_field_changes() {
        let original = compute_hash(&base_event()).expect("hash não deveria falhar");

        let mut tampered = base_event();
        tampered.action = "RESTORE_INVOICE".to_string();
        let tampered_hash = compute_hash(&tampered).expect("hash não deveria falhar");

        assert_ne!(original, tampered_hash);
    }

    #[test]
    fn changes_when_previous_hash_changes() {
        let mut first = base_event();
        first.previous_hash = None;
        let hash_without_chain = compute_hash(&first).expect("hash não deveria falhar");

        first.previous_hash = Some("some-previous-hash".to_string());
        let hash_with_chain = compute_hash(&first).expect("hash não deveria falhar");

        assert_ne!(hash_without_chain, hash_with_chain);
    }

    #[test]
    fn is_independent_of_metadata_key_insertion_order() {
        // O hash não deve depender da ordem em que as chaves do JSON de
        // metadata foram inseridas, só do conteúdo. Isso funciona porque
        // `serde_json::Value::Object`, sem a feature `preserve_order`,
        // usa um BTreeMap por baixo — as chaves saem sempre ordenadas
        // alfabeticamente na hora de serializar, independente da ordem
        // de inserção.
        let mut event_a = base_event();
        event_a.metadata = json!({"ip": "192.168.0.10", "reason": "duplicate invoice"});

        let mut event_b = base_event();
        event_b.metadata = json!({"reason": "duplicate invoice", "ip": "192.168.0.10"});

        let hash_a = compute_hash(&event_a).expect("hash não deveria falhar");
        let hash_b = compute_hash(&event_b).expect("hash não deveria falhar");

        assert_eq!(hash_a, hash_b);
    }
}
