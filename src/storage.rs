use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::errors::AuditError;
use crate::event::AuditEvent;
use crate::immutablelog_receipt::ImmutableLogReceipt;

/// Deriva o caminho do arquivo de eventos pendentes de entrega a partir
/// do `file_path` configurado. `"./audit.jsonl"` -> `"./audit.pending.jsonl"`,
/// no mesmo diretório.
pub fn pending_path_for(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audit".to_string());
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    let pending_name = format!("{stem}.pending{extension}");

    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(pending_name),
        _ => PathBuf::from(pending_name),
    }
}

/// Acrescenta um evento ao final do arquivo JSONL, criando o arquivo
/// (e os diretórios pais, se preciso) caso ainda não existam.
///
/// JSONL ("JSON Lines") = um objeto JSON completo por linha, em vez de
/// um array JSON gigante. A vantagem central: para registrar um novo
/// evento, só precisamos *acrescentar* uma linha no fim do arquivo —
/// não precisamos reler e reescrever o arquivo inteiro (o que seria
/// O(n) por evento e arriscaria corromper tudo se o processo morresse
/// no meio da escrita). Cada `append` aqui é uma operação independente.
///
/// Recebe `path: &Path` (uma referência emprestada) — esta função não
/// precisa ser dona do caminho, só precisa lê-lo para abrir o arquivo.
/// Quem chama (o `AuditLogger`, na Etapa 7) continua dono do `PathBuf`.
pub fn append_event(path: &Path, event: &AuditEvent) -> Result<(), AuditError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| AuditError::Io(err.to_string()))?;
        }
    }

    // `OpenOptions` com `.append(true)`: cada `write_all` é adicionado
    // ao final do arquivo, nunca sobrescreve o conteúdo já gravado.
    // `.create(true)`: se o arquivo não existir ainda, ele é criado.
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| AuditError::Io(err.to_string()))?;

    let mut line = serde_json::to_string(event).map_err(|err| AuditError::Serialization(err.to_string()))?;
    line.push('\n');

    file.write_all(line.as_bytes())
        .map_err(|err| AuditError::Io(err.to_string()))?;
    // `flush()` empurra os bytes do buffer interno do processo para o
    // sistema operacional antes de devolvermos `Ok`. Isso reduz (mas
    // não elimina 100%, sem fsync) a chance de perder o evento se o
    // processo Python morrer logo após o `log()` retornar.
    file.flush().map_err(|err| AuditError::Io(err.to_string()))?;

    Ok(())
}

/// Lê todos os eventos do arquivo JSONL, em ordem (do mais antigo para
/// o mais novo).
///
/// Se o arquivo ainda não existir, devolve uma lista vazia em vez de
/// erro: um `AuditLogger` que nunca chamou `log()` é um estado válido
/// (zero eventos), não uma falha. Isso é importante para `last_hash()`
/// e `verify()` (Etapa 9) funcionarem mesmo antes do primeiro evento.
pub fn read_events(path: &Path) -> Result<Vec<AuditEvent>, AuditError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(AuditError::Io(err.to_string())),
    };

    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (line_number, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| AuditError::Io(err.to_string()))?;
        if line.trim().is_empty() {
            // Ignora linhas vazias (ex.: uma quebra de linha sobrando
            // no fim do arquivo) sem tratar como evento corrompido.
            continue;
        }

        let event: AuditEvent = serde_json::from_str(&line).map_err(|err| {
            // `line_number` é 0-based (vem do `.enumerate()`), por isso
            // o "+ 1": queremos reportar números de linha humanos
            // (1-based), úteis quando o `verifier.rs` precisar apontar
            // exatamente onde a cadeia foi corrompida.
            AuditError::Serialization(format!("linha {}: {err}", line_number + 1))
        })?;
        events.push(event);
    }

    Ok(events)
}

/// Reescreve o arquivo inteiro com a lista de eventos dada, de forma
/// atômica (grava num arquivo temporário no mesmo diretório e troca com
/// `rename`, que é atômico no mesmo filesystem — nunca existe uma janela
/// onde o arquivo final está parcialmente escrito).
///
/// Usada só para atualizar campos de ENVELOPE (`immutablelog`), nunca
/// para alterar `hash`/`previous_hash`/etc — quem chama é responsável
/// por isso. `verify()` recalcula o hash a partir do conteúdo, então
/// uma reescrita que preserve os campos hasheados não invalida a cadeia.
fn rewrite_events(path: &Path, events: &[AuditEvent]) -> Result<(), AuditError> {
    let tmp_path = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        uuid::Uuid::new_v4()
    ));

    let mut tmp_file = File::create(&tmp_path).map_err(|err| AuditError::Io(err.to_string()))?;
    for event in events {
        let mut line = serde_json::to_string(event).map_err(|err| AuditError::Serialization(err.to_string()))?;
        line.push('\n');
        tmp_file.write_all(line.as_bytes()).map_err(|err| AuditError::Io(err.to_string()))?;
    }
    tmp_file.flush().map_err(|err| AuditError::Io(err.to_string()))?;
    drop(tmp_file);

    std::fs::rename(&tmp_path, path).map_err(|err| AuditError::Io(err.to_string()))?;
    Ok(())
}

/// Atualiza o campo `immutablelog` do evento com `id == event_id`,
/// preservando todos os outros campos (incluindo `hash`) exatamente
/// como estavam.
pub fn update_event_receipt(
    path: &Path,
    event_id: &str,
    receipt: &ImmutableLogReceipt,
) -> Result<(), AuditError> {
    let mut events = read_events(path)?;

    let event = events
        .iter_mut()
        .find(|event| event.id == event_id)
        .ok_or_else(|| AuditError::Io(format!("evento '{event_id}' não encontrado em {path:?}")))?;
    event.immutablelog = Some(receipt.clone());

    rewrite_events(path, &events)
}

/// Remove o evento com `id == event_id` do arquivo (usado por
/// `flush_pending()` para tirar um evento já confirmado da fila de
/// pendências).
pub fn remove_event(path: &Path, event_id: &str) -> Result<(), AuditError> {
    let events: Vec<AuditEvent> = read_events(path)?
        .into_iter()
        .filter(|event| event.id != event_id)
        .collect();

    rewrite_events(path, &events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_path() -> PathBuf {
        // Usa uuid (já é dependência do projeto) para gerar um nome de
        // arquivo único por teste, evitando que testes rodando em
        // paralelo (padrão do `cargo test`) pisem no mesmo arquivo.
        std::env::temp_dir().join(format!("rust_py_audit_test_{}.jsonl", uuid::Uuid::new_v4()))
    }

    fn sample_event(action: &str, previous_hash: Option<&str>) -> AuditEvent {
        AuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: "2026-06-17T10:00:00Z".to_string(),
            app_name: "billing-api".to_string(),
            actor_id: "user_123".to_string(),
            action: action.to_string(),
            resource: "invoice".to_string(),
            resource_id: "inv_987".to_string(),
            metadata: json!({"ip": "192.168.0.10"}),
            previous_hash: previous_hash.map(|hash| hash.to_string()),
            hash: "placeholder".to_string(),
            severity: None,
            immutable_trail: None,
            immutablelog: None,
        }
    }

    #[test]
    fn read_events_on_missing_file_returns_empty_vec() {
        let path = temp_path();

        let events = read_events(&path).expect("não deveria falhar para arquivo inexistente");

        assert!(events.is_empty());
    }

    #[test]
    fn append_then_read_round_trips_a_single_event() {
        let path = temp_path();
        let event = sample_event("LOGIN", None);

        append_event(&path, &event).expect("append não deveria falhar");
        let events = read_events(&path).expect("read não deveria falhar");

        assert_eq!(events, vec![event]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn append_preserves_insertion_order_across_multiple_events() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("DELETE_INVOICE", Some("hash-of-first"));

        append_event(&path, &first).expect("append não deveria falhar");
        append_event(&path, &second).expect("append não deveria falhar");
        let events = read_events(&path).expect("read não deveria falhar");

        assert_eq!(events, vec![first, second]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_events_reports_line_number_on_corrupted_line() {
        let path = temp_path();
        std::fs::write(&path, "{\"not\": \"a valid audit event\"}\n").expect("write deveria funcionar");

        let result = read_events(&path);

        assert!(matches!(result, Err(AuditError::Serialization(ref msg)) if msg.contains("linha 1")));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pending_path_for_inserts_pending_before_the_extension() {
        assert_eq!(
            pending_path_for(&PathBuf::from("./audit.jsonl")),
            PathBuf::from("./audit.pending.jsonl")
        );
        assert_eq!(
            pending_path_for(&PathBuf::from("/var/log/audit.jsonl")),
            PathBuf::from("/var/log/audit.pending.jsonl")
        );
    }

    #[test]
    fn update_event_receipt_attaches_receipt_without_changing_hash_or_order() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("LOGOUT", Some(&first.hash));
        append_event(&path, &first).expect("append não deveria falhar");
        append_event(&path, &second).expect("append não deveria falhar");

        let receipt = crate::immutablelog_receipt::ImmutableLogReceipt {
            status: "delivered".to_string(),
            tx_id: Some("tx_123".to_string()),
            ..Default::default()
        };
        update_event_receipt(&path, &second.id, &receipt).expect("update não deveria falhar");

        let events = read_events(&path).expect("read não deveria falhar");
        assert_eq!(events.len(), 2);
        // O primeiro evento (não tocado) continua idêntico.
        assert_eq!(events[0], first);
        // O segundo manteve hash/previous_hash/conteúdo — só ganhou o receipt.
        assert_eq!(events[1].hash, second.hash);
        assert_eq!(events[1].previous_hash, second.previous_hash);
        assert_eq!(events[1].action, second.action);
        assert_eq!(events[1].immutablelog, Some(receipt));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn update_event_receipt_fails_for_unknown_event_id() {
        let path = temp_path();
        append_event(&path, &sample_event("LOGIN", None)).expect("append não deveria falhar");

        let receipt = crate::immutablelog_receipt::ImmutableLogReceipt::pending();
        let result = update_event_receipt(&path, "evento-inexistente", &receipt);

        assert!(matches!(result, Err(AuditError::Io(_))));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_event_drops_only_the_matching_event() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("LOGOUT", Some(&first.hash));
        append_event(&path, &first).expect("append não deveria falhar");
        append_event(&path, &second).expect("append não deveria falhar");

        remove_event(&path, &first.id).expect("remove não deveria falhar");

        let events = read_events(&path).expect("read não deveria falhar");
        assert_eq!(events, vec![second]);

        let _ = std::fs::remove_file(&path);
    }
}
