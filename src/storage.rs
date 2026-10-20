use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

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

/// Caminho do arquivo de lock que serializa a cadeia entre processos:
/// `"./audit.jsonl"` -> `"./audit.jsonl.lock"`.
///
/// O lock fica num arquivo À PARTE, e não no próprio JSONL, porque
/// `rewrite_events` troca o JSONL por `rename` — um lock preso ao inode
/// antigo deixaria de proteger o arquivo novo sem ninguém perceber.
pub fn lock_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "audit.jsonl".into());
    name.push(".lock");
    path.with_file_name(name)
}

/// Lock exclusivo entre processos sobre a cadeia de um `file_path`.
/// Liberado no `Drop` (e pelo SO, se o processo morrer segurando-o — não
/// há lock órfão a limpar à mão).
///
/// É isto que mantém a cadeia linear com vários workers: sem o lock, cada
/// processo encadeava no `last_hash` que *ele* tinha em memória, dois
/// eventos acabavam com o mesmo `previous_hash` e `verify()` acusava
/// adulteração num arquivo que ninguém adulterou.
///
/// O lock vale por *descrição de arquivo aberta*: threads do mesmo processo
/// compartilhando o handle não se excluem por ele — quem as serializa é o
/// `Mutex` do logger, sempre travado antes.
pub struct ChainLock<'a> {
    file: &'a File,
}

impl<'a> ChainLock<'a> {
    /// Bloqueia até obter o lock exclusivo sobre `lock_file` (aberto com
    /// [`open_lock_file`]).
    pub fn acquire(lock_file: &'a File) -> Result<Self, AuditError> {
        lock_file
            .lock_exclusive()
            .map_err(|err| AuditError::Io(format!("lock da cadeia: {err}")))?;
        Ok(ChainLock { file: lock_file })
    }
}

impl Drop for ChainLock<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.file);
    }
}

/// Abre (criando se preciso) o `.lock` de `path`. O logger abre uma vez e
/// guarda o handle: diferente do JSONL, o `.lock` nunca é trocado por
/// `rename`, então o handle continua válido — e o `log()` economiza um
/// `open` por evento.
pub fn open_lock_file(path: &Path) -> Result<File, AuditError> {
    let lock_path = lock_path_for(path);
    if let Some(parent) = lock_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| AuditError::Io(err.to_string()))?;
        }
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|err| AuditError::Io(format!("{lock_path:?}: {err}")))
}

/// Tamanho do arquivo em bytes; `0` se ele ainda não existe.
pub fn file_len(path: &Path) -> Result<u64, AuditError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(AuditError::Io(err.to_string())),
    }
}

/// Hash do último evento do arquivo, lendo **só o fim** dele.
///
/// Usado quando outro processo gravou desde o nosso último `log()`: em
/// vez de reler o arquivo inteiro (O(n)), lemos blocos de trás para a
/// frente até achar a última linha não vazia. Custa O(tamanho da última
/// linha).
pub fn read_last_hash(path: &Path) -> Result<Option<String>, AuditError> {
    const CHUNK: u64 = 8 * 1024;

    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(AuditError::Io(err.to_string())),
    };
    let io = |err: std::io::Error| AuditError::Io(err.to_string());

    let mut pos = file.seek(SeekFrom::End(0)).map_err(io)?;
    let mut tail: Vec<u8> = Vec::new();
    loop {
        // Fim útil = depois de descartar quebras de linha/espaços finais.
        let end = tail
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map(|i| i + 1);
        if let Some(end) = end {
            if let Some(start) = tail[..end].iter().rposition(|&b| b == b'\n') {
                return parse_hash(&tail[start + 1..end]).map(Some);
            }
            if pos == 0 {
                return parse_hash(&tail[..end]).map(Some);
            }
        } else if pos == 0 {
            return Ok(None); // arquivo vazio ou só com espaços
        }

        let step = CHUNK.min(pos);
        pos -= step;
        file.seek(SeekFrom::Start(pos)).map_err(io)?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk).map_err(io)?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
    }
}

fn parse_hash(line: &[u8]) -> Result<String, AuditError> {
    let event: AuditEvent = serde_json::from_slice(line)
        .map_err(|err| AuditError::Serialization(format!("última linha: {err}")))?;
    Ok(event.hash)
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
///
/// Com `sync = true`, chama `sync_data()` (fsync) antes de devolver: o
/// evento está no disco, não só no cache do SO, e sobrevive a uma queda de
/// energia. Devolve o tamanho do arquivo depois da escrita, que o logger
/// usa para notar se outro processo gravou desde então.
pub fn append_event(path: &Path, event: &AuditEvent, sync: bool) -> Result<u64, AuditError> {
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

    let mut line =
        serde_json::to_string(event).map_err(|err| AuditError::Serialization(err.to_string()))?;
    line.push('\n');

    file.write_all(line.as_bytes())
        .map_err(|err| AuditError::Io(err.to_string()))?;
    // `flush()` empurra os bytes do buffer interno do processo para o
    // sistema operacional antes de devolvermos `Ok`. Isso reduz (mas
    // não elimina 100%, sem fsync) a chance de perder o evento se o
    // processo Python morrer logo após o `log()` retornar.
    file.flush()
        .map_err(|err| AuditError::Io(err.to_string()))?;
    if sync {
        file.sync_data()
            .map_err(|err| AuditError::Io(err.to_string()))?;
    }

    file.metadata()
        .map(|meta| meta.len())
        .map_err(|err| AuditError::Io(err.to_string()))
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
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        uuid::Uuid::new_v4()
    ));

    let mut tmp_file = File::create(&tmp_path).map_err(|err| AuditError::Io(err.to_string()))?;
    for event in events {
        let mut line = serde_json::to_string(event)
            .map_err(|err| AuditError::Serialization(err.to_string()))?;
        line.push('\n');
        tmp_file
            .write_all(line.as_bytes())
            .map_err(|err| AuditError::Io(err.to_string()))?;
    }
    tmp_file
        .flush()
        .map_err(|err| AuditError::Io(err.to_string()))?;
    // No disco antes do `rename`: senão uma queda de energia logo após a
    // troca pode deixar o arquivo final vazio ou truncado.
    tmp_file
        .sync_data()
        .map_err(|err| AuditError::Io(err.to_string()))?;
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

/// Versão em lote de `update_event_receipt`: anexa o receipt
/// correspondente a cada evento cujo `id` esteja em `receipts`, numa
/// ÚNICA leitura + reescrita do arquivo. `flush_pending()` usa isso para
/// não pagar um O(n) (ler+reescrever o arquivo inteiro) por evento
/// confirmado — o que tornaria o flush O(n*m) com `m` pendentes.
///
/// Relê o arquivo na hora da escrita (em vez de receber a lista pronta)
/// de propósito: assim eventos gravados concorrentemente por um `log()`
/// entre o início do flush e este ponto são preservados, não
/// sobrescritos.
pub fn update_event_receipts(
    path: &Path,
    receipts: &std::collections::HashMap<String, ImmutableLogReceipt>,
) -> Result<(), AuditError> {
    if receipts.is_empty() {
        return Ok(());
    }

    let mut events = read_events(path)?;
    for event in events.iter_mut() {
        if let Some(receipt) = receipts.get(&event.id) {
            event.immutablelog = Some(receipt.clone());
        }
    }

    rewrite_events(path, &events)
}

/// Versão em lote de `remove_event`: remove todos os eventos cujo `id`
/// esteja em `ids`, numa única leitura + reescrita. Remover por `id`
/// (em vez de reescrever a fila com um snapshot antigo) preserva
/// eventos pendentes que tenham sido acrescentados concorrentemente
/// durante o flush.
pub fn remove_events(
    path: &Path,
    ids: &std::collections::HashSet<String>,
) -> Result<(), AuditError> {
    if ids.is_empty() {
        return Ok(());
    }

    let events: Vec<AuditEvent> = read_events(path)?
        .into_iter()
        .filter(|event| !ids.contains(&event.id))
        .collect();

    rewrite_events(path, &events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn read_last_hash_reads_only_the_tail() {
        let path = temp_path();
        assert_eq!(read_last_hash(&path).unwrap(), None);

        // Muitos eventos com metadata grande: o arquivo passa de vários
        // blocos de 8 KiB, e a última linha cruza a fronteira de um bloco.
        let mut last = String::new();
        for i in 0..50 {
            let mut event = sample_event("LOGIN", None);
            event.metadata = json!({ "pad": "x".repeat(3_000 + i) });
            event.hash = format!("hash-{i}");
            append_event(&path, &event, false).unwrap();
            last = event.hash.clone();
        }
        assert_eq!(read_last_hash(&path).unwrap(), Some(last));

        // Quebras de linha extras no fim não confundem a leitura.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n\n")
            .unwrap();
        assert_eq!(read_last_hash(&path).unwrap().as_deref(), Some("hash-49"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn append_returns_the_new_length() {
        let path = temp_path();
        let len = append_event(&path, &sample_event("LOGIN", None), true).unwrap();
        assert_eq!(len, file_len(&path).unwrap());
        assert!(len > 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn chain_lock_is_exclusive_across_handles() {
        // Dois `ChainLock` no mesmo processo usam descritores diferentes,
        // o que exercita o mesmo caminho do flock entre processos.
        let path = temp_path();
        let first_file = open_lock_file(&path).unwrap();
        let first = ChainLock::acquire(&first_file).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let path2 = path.clone();
        let waiter = std::thread::spawn(move || {
            let second_file = open_lock_file(&path2).unwrap();
            let _second = ChainLock::acquire(&second_file).unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "o segundo lock não podia ser obtido com o primeiro ativo"
        );
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("o segundo lock deveria sair assim que o primeiro caísse");
        waiter.join().unwrap();
        let _ = std::fs::remove_file(lock_path_for(&path));
    }

    #[test]
    fn lock_path_sits_next_to_the_log() {
        assert_eq!(
            lock_path_for(Path::new("/var/log/app/audit.jsonl")),
            PathBuf::from("/var/log/app/audit.jsonl.lock")
        );
    }

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

        append_event(&path, &event, false).expect("append não deveria falhar");
        let events = read_events(&path).expect("read não deveria falhar");

        assert_eq!(events, vec![event]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn append_preserves_insertion_order_across_multiple_events() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("DELETE_INVOICE", Some("hash-of-first"));

        append_event(&path, &first, false).expect("append não deveria falhar");
        append_event(&path, &second, false).expect("append não deveria falhar");
        let events = read_events(&path).expect("read não deveria falhar");

        assert_eq!(events, vec![first, second]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_events_reports_line_number_on_corrupted_line() {
        let path = temp_path();
        std::fs::write(&path, "{\"not\": \"a valid audit event\"}\n")
            .expect("write deveria funcionar");

        let result = read_events(&path);

        assert!(
            matches!(result, Err(AuditError::Serialization(ref msg)) if msg.contains("linha 1"))
        );

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
        append_event(&path, &first, false).expect("append não deveria falhar");
        append_event(&path, &second, false).expect("append não deveria falhar");

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
        append_event(&path, &sample_event("LOGIN", None), false)
            .expect("append não deveria falhar");

        let receipt = crate::immutablelog_receipt::ImmutableLogReceipt::pending();
        let result = update_event_receipt(&path, "evento-inexistente", &receipt);

        assert!(matches!(result, Err(AuditError::Io(_))));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn update_event_receipts_applies_many_in_a_single_rewrite() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("LOGOUT", Some(&first.hash));
        let third = sample_event("DELETE", Some(&second.hash));
        append_event(&path, &first, false).expect("append não deveria falhar");
        append_event(&path, &second, false).expect("append não deveria falhar");
        append_event(&path, &third, false).expect("append não deveria falhar");

        let mut receipts = std::collections::HashMap::new();
        receipts.insert(first.id.clone(), ImmutableLogReceipt::pending());
        receipts.insert(
            third.id.clone(),
            crate::immutablelog_receipt::ImmutableLogReceipt {
                status: "delivered".to_string(),
                tx_id: Some("tx_3".to_string()),
                ..Default::default()
            },
        );
        update_event_receipts(&path, &receipts).expect("update em lote não deveria falhar");

        let events = read_events(&path).expect("read não deveria falhar");
        assert_eq!(events.len(), 3);
        // Ordem preservada; só os ids no mapa ganharam receipt.
        assert_eq!(
            events[0].immutablelog.as_ref().map(|r| r.status.as_str()),
            Some("pending")
        );
        assert_eq!(events[1].immutablelog, None);
        assert_eq!(
            events[2]
                .immutablelog
                .as_ref()
                .and_then(|r| r.tx_id.clone()),
            Some("tx_3".to_string())
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_events_drops_only_the_matching_ids_in_one_pass() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("LOGOUT", Some(&first.hash));
        let third = sample_event("DELETE", Some(&second.hash));
        append_event(&path, &first, false).expect("append não deveria falhar");
        append_event(&path, &second, false).expect("append não deveria falhar");
        append_event(&path, &third, false).expect("append não deveria falhar");

        let mut ids = std::collections::HashSet::new();
        ids.insert(first.id.clone());
        ids.insert(third.id.clone());
        remove_events(&path, &ids).expect("remove em lote não deveria falhar");

        let events = read_events(&path).expect("read não deveria falhar");
        assert_eq!(events, vec![second]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn batch_helpers_are_noops_when_empty() {
        let path = temp_path();
        append_event(&path, &sample_event("LOGIN", None), false)
            .expect("append não deveria falhar");

        update_event_receipts(&path, &std::collections::HashMap::new()).expect("noop");
        remove_events(&path, &std::collections::HashSet::new()).expect("noop");

        let events = read_events(&path).expect("read não deveria falhar");
        assert_eq!(events.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_event_drops_only_the_matching_event() {
        let path = temp_path();
        let first = sample_event("LOGIN", None);
        let second = sample_event("LOGOUT", Some(&first.hash));
        append_event(&path, &first, false).expect("append não deveria falhar");
        append_event(&path, &second, false).expect("append não deveria falhar");

        remove_event(&path, &first.id).expect("remove não deveria falhar");

        let events = read_events(&path).expect("read não deveria falhar");
        assert_eq!(events, vec![second]);

        let _ = std::fs::remove_file(&path);
    }
}
