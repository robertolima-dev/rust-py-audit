use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::event::AuditEvent;
use crate::hash::compute_hash;
use crate::immutablelog_client::{resolve_severity, sanitize_trail, ImmutableLogClient};
use crate::immutablelog_config::{mode_from_env_or, AuditMode, ImmutableLogConfig};
use crate::immutablelog_receipt::ImmutableLogReceipt;
use crate::pyconv::{event_to_pydict, python_to_json};
use crate::retry::send_with_retry;
use crate::storage;
use crate::verifier::verify_chain;

/// A classe Python `AuditLogger`.
///
/// `#[pyclass]` é a macro do PyO3 que transforma este struct Rust numa
/// classe de verdade do Python — instanciável com `AuditLogger(...)`,
/// com atributos e métodos próprios.
///
/// Guardamos `file_path` como `PathBuf` (e não como `String`) porque é
/// o tipo idiomático do Rust para representar caminhos de arquivo —
/// já se integra direto com `storage::append_event`/`read_events`
/// (que recebem `&Path`).
///
/// `last_hash` é um cache em memória do hash do último evento gravado.
/// Sem ele, cada chamada de `log()` precisaria reler o arquivo inteiro
/// (O(n)) só para descobrir qual foi o último hash — com esse campo,
/// isso é O(1): lemos o arquivo uma única vez, na criação do
/// `AuditLogger` (`new()`), e depois mantemos o cache atualizado a cada
/// `log()` bem-sucedido.
///
/// O `Mutex` em volta de `last_hash` serve a dois propósitos ligados a
/// concorrência:
/// 1. Permite que `log()` e `flush_pending()` recebam `&self` (em vez de
///    `&mut self`). Com `&mut self`, o PyO3 mantém um *borrow* mutável
///    exclusivo durante TODO o método — e como `log()` libera o GIL
///    durante a chamada de rede (`allow_threads`), uma segunda thread
///    (ex.: WSGI multi-thread compartilhando o mesmo logger) que
///    chamasse `log()` ao mesmo tempo receberia `RuntimeError: Already
///    borrowed`, perdendo o evento. Com `&self` + `Mutex`, isso não
///    acontece.
/// 2. Garante que a leitura do `last_hash`, o cálculo do `previous_hash`
///    e a atualização do cache aconteçam atomicamente — duas threads
///    nunca encadeiam dois eventos no MESMO `previous_hash` (o que
///    bifurcaria a cadeia).
///
/// REGRA CRÍTICA: este `Mutex` só pode ser travado com o GIL JÁ LIBERADO
/// (dentro de `py.allow_threads`). Travar segurando o GIL poderia
/// causar deadlock com a thread que liberou o GIL durante a rede e ainda
/// segura o lock. Por isso `log()`/`flush_pending()`/`last_hash()`
/// envolvem o uso do lock em `allow_threads`.
///
/// Limitação consciente do MVP: esse cache assume que só **este**
/// `AuditLogger` (este processo) escreve no arquivo. Se dois processos
/// diferentes apontarem para o mesmo `file_path` e chamarem `log()`
/// concorrentemente, o cache de um processo pode ficar desatualizado em
/// relação ao que o outro gravou — a cadeia no arquivo continuaria
/// íntegra (cada processo encadeia com o que *ele* sabia ser o último
/// hash), mas teríamos duas pontas de cadeia em paralelo. Resolver isso
/// exigiria lock de arquivo entre processos, fora do escopo do MVP.
///
/// Limitação específica de `mode="remote"`: como esse modo NÃO grava
/// JSONL local, `last_hash` vive apenas em memória. Ao recriar o
/// `AuditLogger` (reinício do processo), `new()` relê `file_path` —
/// que está vazio em remote puro — e `last_hash` volta a `None`, ou
/// seja, o `previous_hash` REINICIA a cada restart. A continuidade da
/// cadeia entre reinícios, nesse modo, depende inteiramente do
/// ImmutableLog do lado servidor, não do encadeamento local. Quem
/// precisa de cadeia local contínua entre reinícios deve usar
/// `mode="hybrid"`.
#[pyclass]
pub struct AuditLogger {
    app_name: String,
    file_path: PathBuf,
    last_hash: Mutex<Option<String>>,
    /// Modo de operação (`local`/`remote`/`hybrid`). Por ora só fica
    /// guardado e exposto via getter — `log()` ainda se comporta como
    /// antes (sempre local) independente do valor aqui. O envio remoto
    /// chega nas próximas etapas.
    mode: AuditMode,
    immutablelog: ImmutableLogConfig,
    /// `Some` somente quando `mode` exige contato com o ImmutableLog
    /// (`remote`/`hybrid`). Construído uma única vez em `new()` —
    /// `reqwest::blocking::Client` já mantém um pool de conexões
    /// internamente, então reusar a mesma instância entre chamadas de
    /// `log()` é o comportamento certo (evita reconectar/renegociar TLS
    /// a cada evento).
    client: Option<ImmutableLogClient>,
}

#[pymethods]
impl AuditLogger {
    /// `#[new]` marca este método como o `__init__` da classe Python.
    /// Retornar `PyResult<Self>` (em vez de só `Self`) é o que permite
    /// usar `?` para propagar um eventual erro de leitura do arquivo
    /// (ex.: arquivo existente mas corrompido) como uma exceção Python
    /// em vez de um panic.
    /// `mode`, `immutablelog_url` e `immutablelog_api_key` aceitam
    /// `None` (o default) para cair no fallback de variáveis de
    /// ambiente (`RUST_PY_AUDIT_MODE`, `IMMUTABLELOG_URL`,
    /// `IMMUTABLELOG_API_KEY`) e, por fim, em `mode="local"` se nada
    /// for encontrado — preservando o comportamento de quem já chama
    /// `AuditLogger(app_name, file_path)` sem saber que esses
    /// parâmetros existem.
    #[new]
    #[pyo3(signature = (
        app_name,
        file_path = "./audit.jsonl".to_string(),
        mode = None,
        immutablelog_url = None,
        immutablelog_api_key = None,
        timeout_ms = 500,
        retry_enabled = true,
        max_retries = 3,
        immutablelog_env = None,
    ))]
    fn new(
        app_name: String,
        file_path: String,
        mode: Option<String>,
        immutablelog_url: Option<String>,
        immutablelog_api_key: Option<String>,
        timeout_ms: u64,
        retry_enabled: bool,
        max_retries: u8,
        immutablelog_env: Option<String>,
    ) -> PyResult<Self> {
        let file_path = PathBuf::from(file_path);

        let mode_value = mode_from_env_or(mode, "local");
        let mode = AuditMode::parse(&mode_value)?;

        let immutablelog = ImmutableLogConfig::resolve(
            immutablelog_url,
            immutablelog_api_key,
            timeout_ms,
            retry_enabled,
            max_retries,
            immutablelog_env,
        );
        immutablelog.validate_for(mode)?;

        let client = if mode.sends_to_immutablelog() {
            Some(ImmutableLogClient::new(&immutablelog).map_err(crate::errors::AuditError::from)?)
        } else {
            None
        };

        // Lê o arquivo (se existir) uma única vez, para inicializar o
        // cache de `last_hash`. `storage::read_events` já devolve uma
        // lista vazia se o arquivo ainda não existe — então um logger
        // novo, apontando para um arquivo novo, simplesmente começa com
        // `last_hash = None`.
        let last_hash = storage::read_events(&file_path)?
            .last()
            .map(|event| event.hash.clone());

        Ok(AuditLogger {
            app_name,
            file_path,
            last_hash: Mutex::new(last_hash),
            mode,
            immutablelog,
            client,
        })
    }

    #[getter]
    fn app_name(&self) -> &str {
        &self.app_name
    }

    #[getter]
    fn file_path(&self) -> String {
        self.file_path.to_string_lossy().into_owned()
    }

    #[getter]
    fn mode(&self) -> &'static str {
        self.mode.as_str()
    }

    #[getter]
    fn immutablelog_url(&self) -> Option<String> {
        self.immutablelog.url.clone()
    }

    #[getter]
    fn timeout_ms(&self) -> u64 {
        self.immutablelog.timeout_ms
    }

    #[getter]
    fn retry_enabled(&self) -> bool {
        self.immutablelog.retry_enabled
    }

    #[getter]
    fn max_retries(&self) -> u8 {
        self.immutablelog.max_retries
    }

    #[getter]
    fn immutablelog_env(&self) -> Option<String> {
        self.immutablelog.env.clone()
    }

    /// Devolve o hash do último evento gravado, ou `None` se o logger
    /// ainda não registrou nenhum evento (nem leu nenhum de um arquivo
    /// existente).
    ///
    /// É um método (`audit.last_hash()`), não um `#[getter]`
    /// (`audit.last_hash`), de propósito: diferente de `app_name`/
    /// `file_path` — que são configuração fixa, definida na criação do
    /// logger — `last_hash` é um valor que muda a cada `log()`. Marcar
    /// como método deixa essa natureza "dinâmica" mais explícita para
    /// quem lê o código Python chamador.
    ///
    /// Implementação O(1): só devolve o cache em memória (`Option`
    /// clonado), sem tocar no arquivo em disco — diferente de
    /// `verify()`, que releria tudo de propósito.
    ///
    /// O `allow_threads` em volta do lock segue a REGRA CRÍTICA descrita
    /// na doc da struct: nunca travar o `Mutex` segurando o GIL.
    fn last_hash(&self, py: Python<'_>) -> Option<String> {
        py.allow_threads(|| self.lock_last_hash().clone())
    }

    /// Registra um novo evento de auditoria.
    ///
    /// `&mut self`: diferente dos getters (que só leem), `log()`
    /// precisa *alterar* o estado do logger (`self.last_hash`) depois
    /// de gravar o evento — por isso pede uma referência mutável.
    ///
    /// `metadata: Option<Bound<'_, PyAny>>` com `default = None`: um
    /// `dict` Python qualquer (ou nenhum). `Bound<'py, PyAny>` é como o
    /// PyO3 representa "um objeto Python qualquer, com vida limitada ao
    /// escopo do GIL atual" — não sabemos o tipo concreto até
    /// inspecioná-lo dentro de `python_to_json`.
    /// `severity` e `immutable_trail` só afetam o que é enviado ao
    /// ImmutableLog (`meta.type`/`meta.immutable_trail`) — não entram no
    /// hash. `severity` precisa ser uma de `error`/`warning`/`info`/
    /// `success` (default `"info"` se omitido); `immutable_trail` é
    /// sanitizado (trim, sem `:`, máx. 256 chars) e omitido se vazio
    /// depois disso.
    #[pyo3(signature = (actor_id, action, resource, resource_id, metadata = None, severity = None, immutable_trail = None))]
    fn log(
        &self,
        py: Python<'_>,
        actor_id: String,
        action: String,
        resource: String,
        resource_id: String,
        metadata: Option<Bound<'_, PyAny>>,
        severity: Option<String>,
        immutable_trail: Option<String>,
    ) -> PyResult<PyObject> {
        // A conversão de `metadata` (e a validação de `severity`) precisa
        // do GIL e é feita ANTES de liberar threads: `python_to_json`
        // inspeciona objetos Python, e validar cedo faz `log()` falhar
        // rápido mesmo em `mode="local"`.
        let metadata_value = match metadata {
            Some(value) => python_to_json(&value)?,
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        // `resolved_severity` é o valor efetivo usado na PRIMEIRA
        // tentativa de envio (default "info" quando `severity` é `None`);
        // `event.severity` guarda só o que foi passado (ou `None`), para
        // `flush_pending()` reenviar mais tarde com a mesma classificação.
        let resolved_severity =
            resolve_severity(severity.as_deref()).map_err(crate::errors::AuditError::Config)?;
        let sanitized_trail = sanitize_trail(immutable_trail.as_deref());

        // RFC3339 em UTC produz exatamente o formato do exemplo do MVP:
        // "2026-06-17T10:00:00Z".
        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|err| pyo3::exceptions::PyValueError::new_err(err.to_string()))?;

        // Toda a seção crítica (travar `last_hash`, encadear, gravar
        // local e/ou enviar à rede, atualizar o cache) roda com o GIL
        // LIBERADO — ver a REGRA CRÍTICA na doc da struct. `append_chained`
        // é puro Rust, não toca em objetos Python.
        let event = py.allow_threads(|| {
            self.append_chained(
                actor_id,
                action,
                resource,
                resource_id,
                metadata_value,
                timestamp,
                severity,
                sanitized_trail,
                resolved_severity,
            )
        })?;

        event_to_pydict(py, &event)
    }

    /// Revalida a cadeia inteira a partir do arquivo em disco.
    ///
    /// `&self` (não `&mut self`): `verify()` só lê — não usa nem altera
    /// `self.last_hash`. De propósito: o objetivo aqui é checar o que
    /// está *realmente gravado* no arquivo, não confiar no cache em
    /// memória, que poderia mascarar uma alteração feita por fora (por
    /// exemplo, alguém editando o arquivo JSONL manualmente).
    fn verify(&self, py: Python<'_>) -> PyResult<PyObject> {
        let events = storage::read_events(&self.file_path)?;
        let result = verify_chain(&events)?;

        let dict = PyDict::new_bound(py);
        dict.set_item("valid", result.valid)?;
        dict.set_item("total_events", result.total_events)?;

        // O formato de saída muda conforme o resultado (igual ao
        // contrato descrito no MVP): sucesso devolve `last_hash`;
        // falha devolve `error_index` e `reason` no lugar dele.
        if result.valid {
            dict.set_item("last_hash", result.last_hash)?;
        } else {
            dict.set_item("error_index", result.error_index)?;
            dict.set_item("reason", result.reason)?;
        }

        Ok(dict.into())
    }

    /// Tenta reentregar ao ImmutableLog todo evento que ficou marcado
    /// como `pending` (gravado em `audit.pending.jsonl`).
    ///
    /// Para cada evento pendente: tenta o envio (com o mesmo
    /// `retry_enabled`/`max_retries` configurados no logger, e a mesma
    /// `Idempotency-Key` de antes — `event.id` nunca muda). Em sucesso,
    /// atualiza o receipt em `audit.jsonl` e remove o evento da fila de
    /// pendências; em nova falha, deixa o evento na fila para a próxima
    /// chamada.
    ///
    /// Funciona em qualquer `mode`: se não houver `audit.pending.jsonl`
    /// (ex.: `mode="local"`, que nunca cria esse arquivo), simplesmente
    /// não há nada para tentar e o resultado vem zerado.
    fn flush_pending(&self, py: Python<'_>) -> PyResult<PyObject> {
        let pending_path = storage::pending_path_for(&self.file_path);
        let pending_events = storage::read_events(&pending_path)?;
        let total = pending_events.len();

        // Igual a `log()`: tudo que trava o `Mutex` ou faz rede roda com
        // o GIL liberado (REGRA CRÍTICA na doc da struct).
        let (flushed, still_pending) = py.allow_threads(|| -> Result<(usize, usize), crate::errors::AuditError> {
            // Primeiro a parte cara/lenta (rede), SEM segurar o lock:
            // acumulamos os receipts entregues e os ids a remover da fila.
            let mut delivered: HashMap<String, ImmutableLogReceipt> = HashMap::new();
            let mut flushed_ids: HashSet<String> = HashSet::new();

            for event in &pending_events {
                let severity = resolve_severity(event.severity.as_deref())
                    .map_err(crate::errors::AuditError::Config)?;
                if let Ok(receipt) = self.deliver(event, severity, event.immutable_trail.as_deref())
                {
                    delivered.insert(event.id.clone(), receipt);
                    flushed_ids.insert(event.id.clone());
                }
                // Em falha o evento simplesmente continua na fila para a
                // próxima chamada de `flush_pending()` tentar de novo.
            }

            // Só agora travamos, para a parte que mexe nos arquivos —
            // serializando contra um `log()` concorrente. As funções em
            // lote releem os arquivos aqui dentro, então eventos
            // acrescentados nesse meio-tempo são preservados.
            let _guard = self.lock_last_hash();
            storage::update_event_receipts(&self.file_path, &delivered)?;
            storage::remove_events(&pending_path, &flushed_ids)?;

            let flushed = flushed_ids.len();
            Ok((flushed, total - flushed))
        })?;

        let dict = PyDict::new_bound(py);
        dict.set_item("flushed", flushed)?;
        dict.set_item("still_pending", still_pending)?;
        dict.set_item("total", total)?;
        Ok(dict.into())
    }
}

/// Métodos auxiliares internos — fora do bloco `#[pymethods]` de
/// propósito: nada aqui deve ser exposto como método Python de
/// `AuditLogger`.
impl AuditLogger {
    /// Trava o `Mutex` de `last_hash`, recuperando-se de um eventual
    /// *poisoning* (que só ocorreria se uma thread tivesse entrado em
    /// panic segurando o lock — improvável aqui, já que a seção crítica
    /// não chama `.unwrap()`/`panic!`). Recuperar em vez de propagar o
    /// panic evita derrubar o processo Python inteiro por causa de um
    /// lock envenenado.
    ///
    /// IMPORTANTE: só chamar com o GIL liberado (ver REGRA CRÍTICA na
    /// doc da struct).
    fn lock_last_hash(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.last_hash
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Envia `event` ao ImmutableLog (com retry). Rust puro: NÃO toca no
    /// GIL nem o libera — quem chama (`append_chained`/`flush_pending`)
    /// já está dentro de um `allow_threads`, então a chamada HTTP
    /// bloqueante já roda com o GIL liberado, sem travar outras threads
    /// Python.
    fn deliver(
        &self,
        event: &AuditEvent,
        severity: &str,
        immutable_trail: Option<&str>,
    ) -> Result<ImmutableLogReceipt, crate::immutablelog_client::ImmutableLogClientError> {
        use crate::immutablelog_client::ImmutableLogClientError;
        let client = self.client.as_ref().ok_or_else(|| ImmutableLogClientError::Permanent {
            status: None,
            reason: format!(
                "mode='{}' exige um cliente ImmutableLog inicializado (estado inconsistente)",
                self.mode.as_str()
            ),
        })?;

        send_with_retry(
            client,
            event,
            severity,
            immutable_trail,
            self.immutablelog.retry_enabled,
            self.immutablelog.max_retries,
        )
    }

    /// Seção crítica de `log()`, em Rust puro (sem GIL): trava o
    /// `last_hash`, monta o evento já encadeado, calcula o hash, grava
    /// local e/ou envia à rede conforme o modo, e atualiza o cache. O
    /// lock é mantido por toda a operação — inclusive a chamada de rede
    /// em `remote`/`hybrid` — de propósito: a cadeia de hashes é
    /// inerentemente sequencial, então serializar `log()` é o
    /// comportamento correto (impede duas threads de encadearem no mesmo
    /// `previous_hash`).
    #[allow(clippy::too_many_arguments)]
    fn append_chained(
        &self,
        actor_id: String,
        action: String,
        resource: String,
        resource_id: String,
        metadata: serde_json::Value,
        timestamp: String,
        severity: Option<String>,
        immutable_trail: Option<String>,
        resolved_severity: &str,
    ) -> Result<AuditEvent, crate::errors::AuditError> {
        let mut guard = self.lock_last_hash();

        // `hash` começa vazio: só dá para calculá-lo depois que todos os
        // outros campos existem (`compute_hash` ignora o campo `hash`).
        let mut event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp,
            app_name: self.app_name.clone(),
            actor_id,
            action,
            resource,
            resource_id,
            metadata,
            previous_hash: guard.clone(),
            hash: String::new(),
            severity,
            immutable_trail,
            immutablelog: None,
        };
        event.hash = compute_hash(&event)?;

        match self.mode {
            AuditMode::Local => {
                storage::append_event(&self.file_path, &event)?;
                *guard = Some(event.hash.clone());
            }
            AuditMode::Remote => {
                // Não grava JSONL local: se o envio falhar, o erro sobe
                // como exceção Python e `last_hash` continua apontando
                // para o último evento que de fato foi confirmado.
                let receipt = self
                    .deliver(&event, resolved_severity, event.immutable_trail.as_deref())
                    .map_err(crate::errors::AuditError::from)?;
                event.immutablelog = Some(receipt);
                *guard = Some(event.hash.clone());
            }
            AuditMode::Hybrid => {
                // Salva local PRIMEIRO: mesmo que o processo morra antes
                // do envio terminar, o evento já está persistido (sem
                // receipt) — nunca perdemos o registro local por uma
                // falha de rede.
                storage::append_event(&self.file_path, &event)?;
                *guard = Some(event.hash.clone());

                match self.deliver(&event, resolved_severity, event.immutable_trail.as_deref()) {
                    Ok(receipt) => {
                        event.immutablelog = Some(receipt.clone());
                        storage::update_event_receipt(&self.file_path, &event.id, &receipt)?;
                    }
                    Err(_) => {
                        // Falha em hybrid NÃO é exceção Python: o evento
                        // já está seguro localmente, então devolvemos
                        // status "pending" e deixamos `flush_pending()`
                        // tentar de novo depois.
                        let pending_receipt = ImmutableLogReceipt::pending();
                        event.immutablelog = Some(pending_receipt.clone());
                        storage::update_event_receipt(&self.file_path, &event.id, &pending_receipt)?;
                        storage::append_event(&storage::pending_path_for(&self.file_path), &event)?;
                    }
                }
            }
        }

        Ok(event)
    }
}
