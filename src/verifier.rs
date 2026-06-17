use crate::errors::AuditError;
use crate::event::AuditEvent;
use crate::hash::compute_hash;

/// Resultado de uma verificação de integridade da cadeia de eventos.
///
/// Todos os campos depois de `total_events` são opcionais porque o
/// formato de saída é diferente conforme o resultado: quando `valid` é
/// `true`, devolvemos `last_hash`; quando é `false`, devolvemos
/// `error_index` e `reason` em vez disso. Quem decide o que aparece no
/// `dict` Python final é o método `verify()` em `audit_logger.rs`
/// (Etapa de PyO3) — este struct é puro Rust, sem nenhuma dependência
/// de `pyo3`.
#[derive(Debug, PartialEq)]
pub struct VerificationResult {
    pub valid: bool,
    pub total_events: usize,
    pub last_hash: Option<String>,
    pub error_index: Option<usize>,
    pub reason: Option<String>,
}

/// Motivo de falha: o hash gravado não bate com o hash recalculado a
/// partir do conteúdo do evento — ou seja, algum campo do evento foi
/// editado depois de gravado.
const REASON_HASH_MISMATCH: &str = "hash_mismatch";

/// Motivo de falha: o `previous_hash` do evento não bate com o `hash`
/// do evento anterior na lista. Cobre tanto remoção (um evento do meio
/// foi apagado) quanto reordenação (dois eventos trocaram de posição).
const REASON_BROKEN_CHAIN: &str = "broken_chain";

/// Revalida a cadeia de eventos do zero: para cada evento, recalcula o
/// hash a partir do conteúdo (ignorando o `hash` já gravado) e confirma
/// que (1) esse hash recalculado bate com o que está no arquivo e (2)
/// o `previous_hash` do evento aponta corretamente para o `hash` do
/// evento anterior.
///
/// Recebe `&[AuditEvent]` (um *slice*, fatia emprestada de uma lista) em
/// vez de `Vec<AuditEvent>`: esta função só precisa percorrer e ler os
/// eventos, nunca precisa ser dona da lista nem alterá-la. Usar slice
/// também deixa a função reutilizável tanto para um `Vec` quanto para
/// um array Rust comum, sem custo de cópia.
pub fn verify_chain(events: &[AuditEvent]) -> Result<VerificationResult, AuditError> {
    let mut expected_previous_hash: Option<String> = None;

    for (index, event) in events.iter().enumerate() {
        if event.previous_hash != expected_previous_hash {
            return Ok(VerificationResult {
                valid: false,
                total_events: events.len(),
                last_hash: None,
                error_index: Some(index),
                reason: Some(REASON_BROKEN_CHAIN.to_string()),
            });
        }

        let recomputed_hash = compute_hash(event)?;
        if recomputed_hash != event.hash {
            return Ok(VerificationResult {
                valid: false,
                total_events: events.len(),
                last_hash: None,
                error_index: Some(index),
                reason: Some(REASON_HASH_MISMATCH.to_string()),
            });
        }

        expected_previous_hash = Some(event.hash.clone());
    }

    Ok(VerificationResult {
        valid: true,
        total_events: events.len(),
        last_hash: expected_previous_hash,
        error_index: None,
        reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::compute_hash;
    use serde_json::json;

    /// Monta uma cadeia de `n` eventos válidos, cada um já com o hash
    /// certo e encadeado ao anterior — útil como base para os testes
    /// que em seguida "corrompem" um detalhe específico.
    fn valid_chain(n: usize) -> Vec<AuditEvent> {
        let mut events = Vec::new();
        let mut previous_hash: Option<String> = None;

        for i in 0..n {
            let mut event = AuditEvent {
                id: format!("evt_{i}"),
                timestamp: "2026-06-17T10:00:00Z".to_string(),
                app_name: "billing-api".to_string(),
                actor_id: "user_123".to_string(),
                action: format!("ACTION_{i}"),
                resource: "invoice".to_string(),
                resource_id: format!("inv_{i}"),
                metadata: json!({"ip": "192.168.0.10"}),
                previous_hash: previous_hash.clone(),
                hash: String::new(),
            };
            event.hash = compute_hash(&event).expect("hash não deveria falhar");
            previous_hash = Some(event.hash.clone());
            events.push(event);
        }

        events
    }

    #[test]
    fn empty_chain_is_valid_with_zero_events() {
        let result = verify_chain(&[]).expect("verify não deveria falhar");

        assert_eq!(
            result,
            VerificationResult {
                valid: true,
                total_events: 0,
                last_hash: None,
                error_index: None,
                reason: None,
            }
        );
    }

    #[test]
    fn untouched_chain_is_valid() {
        let events = valid_chain(3);
        let expected_last_hash = events.last().map(|event| event.hash.clone());

        let result = verify_chain(&events).expect("verify não deveria falhar");

        assert!(result.valid);
        assert_eq!(result.total_events, 3);
        assert_eq!(result.last_hash, expected_last_hash);
    }

    #[test]
    fn detects_a_tampered_field_as_hash_mismatch() {
        let mut events = valid_chain(3);
        // Alteramos o conteúdo do evento do meio sem recalcular o hash
        // — exatamente o que aconteceria se alguém editasse o arquivo
        // JSONL manualmente.
        events[1].action = "TAMPERED_ACTION".to_string();

        let result = verify_chain(&events).expect("verify não deveria falhar");

        assert!(!result.valid);
        assert_eq!(result.error_index, Some(1));
        assert_eq!(result.reason, Some(REASON_HASH_MISMATCH.to_string()));
    }

    #[test]
    fn detects_a_removed_event_as_broken_chain() {
        let mut events = valid_chain(3);
        // Remove o evento do meio: o evento que sobra no índice 1 ainda
        // tem o `hash` correto para o SEU próprio conteúdo, mas o
        // `previous_hash` dele não bate mais com o `hash` do evento que
        // agora está antes dele na lista.
        events.remove(1);

        let result = verify_chain(&events).expect("verify não deveria falhar");

        assert!(!result.valid);
        assert_eq!(result.error_index, Some(1));
        assert_eq!(result.reason, Some(REASON_BROKEN_CHAIN.to_string()));
    }

    #[test]
    fn detects_reordered_events_as_broken_chain() {
        let mut events = valid_chain(3);
        events.swap(0, 1);

        let result = verify_chain(&events).expect("verify não deveria falhar");

        assert!(!result.valid);
        assert_eq!(result.error_index, Some(0));
        assert_eq!(result.reason, Some(REASON_BROKEN_CHAIN.to_string()));
    }

    #[test]
    fn detects_a_forged_first_previous_hash_as_broken_chain() {
        let mut events = valid_chain(2);
        // O primeiro evento da cadeia precisa ter `previous_hash =
        // None`. Se alguém forjar um valor aqui (mesmo que o hash do
        // próprio evento continue "certo" para esse valor forjado, já
        // que recalculamos com o `previous_hash` que está no arquivo),
        // ainda assim é uma tentativa de reescrever a cadeia, e
        // precisa ser rejeitada.
        events[0].previous_hash = Some("forged-hash".to_string());
        events[0].hash = compute_hash(&events[0]).expect("hash não deveria falhar");

        let result = verify_chain(&events).expect("verify não deveria falhar");

        assert!(!result.valid);
        assert_eq!(result.error_index, Some(0));
        assert_eq!(result.reason, Some(REASON_BROKEN_CHAIN.to_string()));
    }
}
