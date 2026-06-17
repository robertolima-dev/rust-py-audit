use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::errors::AuditError;
use crate::event::AuditEvent;

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
}
