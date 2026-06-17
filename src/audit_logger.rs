use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::event::AuditEvent;
use crate::hash::compute_hash;
use crate::pyconv::{event_to_pydict, python_to_json};
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
/// Limitação consciente do MVP: esse cache assume que só **este**
/// `AuditLogger` (este processo) escreve no arquivo. Se dois processos
/// diferentes apontarem para o mesmo `file_path` e chamarem `log()`
/// concorrentemente, o cache de um processo pode ficar desatualizado em
/// relação ao que o outro gravou — a cadeia no arquivo continuaria
/// íntegra (cada processo encadeia com o que *ele* sabia ser o último
/// hash), mas teríamos duas pontas de cadeia em paralelo. Resolver isso
/// exigiria lock de arquivo entre processos, fora do escopo do MVP.
#[pyclass]
pub struct AuditLogger {
    app_name: String,
    file_path: PathBuf,
    last_hash: Option<String>,
}

#[pymethods]
impl AuditLogger {
    /// `#[new]` marca este método como o `__init__` da classe Python.
    /// Retornar `PyResult<Self>` (em vez de só `Self`) é o que permite
    /// usar `?` para propagar um eventual erro de leitura do arquivo
    /// (ex.: arquivo existente mas corrompido) como uma exceção Python
    /// em vez de um panic.
    #[new]
    #[pyo3(signature = (app_name, file_path = "./audit.jsonl".to_string()))]
    fn new(app_name: String, file_path: String) -> PyResult<Self> {
        let file_path = PathBuf::from(file_path);

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
            last_hash,
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
    fn last_hash(&self) -> Option<String> {
        self.last_hash.clone()
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
    #[pyo3(signature = (actor_id, action, resource, resource_id, metadata = None))]
    fn log(
        &mut self,
        py: Python<'_>,
        actor_id: String,
        action: String,
        resource: String,
        resource_id: String,
        metadata: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let metadata_value = match metadata {
            Some(value) => python_to_json(&value)?,
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        // RFC3339 em UTC produz exatamente o formato do exemplo do MVP:
        // "2026-06-17T10:00:00Z". `map_err` converte o erro de
        // formatação (praticamente impossível de ocorrer aqui, mas
        // ainda é um `Result`) em `PyErr`, evitando `.unwrap()`.
        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|err| pyo3::exceptions::PyValueError::new_err(err.to_string()))?;

        // Montamos o evento com `hash` vazio primeiro: o hash só pode
        // ser calculado depois que todos os OUTROS campos já existem
        // (Etapa 5 — `compute_hash` ignora o campo `hash` de propósito,
        // mas ainda precisamos de algum valor para inicializar o
        // struct).
        let mut event = AuditEvent {
            id: Uuid::new_v4().to_string(),
            timestamp,
            app_name: self.app_name.clone(),
            actor_id,
            action,
            resource,
            resource_id,
            metadata: metadata_value,
            previous_hash: self.last_hash.clone(),
            hash: String::new(),
        };

        event.hash = compute_hash(&event)?;

        // Só atualizamos `self.last_hash` (e devolvemos o evento) DEPOIS
        // que `append_event` confirma que a gravação em disco deu certo
        // — se a escrita falhar, o cache continua apontando para o
        // último evento realmente persistido, e o erro sobe como
        // exceção Python (via `From<AuditError> for PyErr`).
        storage::append_event(&self.file_path, &event)?;
        self.last_hash = Some(event.hash.clone());

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
}
