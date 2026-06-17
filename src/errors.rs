use std::fmt;

/// Erros do "core" da biblioteca (tudo que não é PyO3).
///
/// Por enquanto só temos uma variante, mas o enum já existe para que
/// `hash.rs`, `storage.rs` e `verifier.rs` tenham um tipo de erro comum
/// para devolver em seus `Result`. Quando o `AuditLogger` (Etapa 7) for
/// criado, cada variante será convertida na exceção Python mais
/// apropriada (`ValueError`, `OSError`, etc.) em vez de estourar um
/// panic — é assim que cumprimos a regra "converter erros Rust em
/// exceções Python" sem usar `.unwrap()` em nenhum ponto crítico.
#[derive(Debug)]
pub enum AuditError {
    /// Falha ao serializar/desserializar um evento em JSON (para
    /// calcular o hash ou para ler/gravar o arquivo JSONL).
    Serialization(String),
    /// Falha de I/O ao acessar o arquivo de eventos (permissão,
    /// disco, diretório inexistente, etc).
    Io(String),
    /// Configuração inválida passada pelo usuário (ex.: `mode`
    /// desconhecido, ou `mode="remote"`/`"hybrid"` sem
    /// `immutablelog_url`/`immutablelog_api_key`).
    Config(String),
    /// Falha ao entregar um evento ao ImmutableLog em `mode="remote"`
    /// (erro permanente, ou retries esgotados em erro retryable). Em
    /// `mode="hybrid"` isso NÃO é levantado como exceção — vira
    /// `delivery_status="pending"` em vez disso.
    ImmutableLog(String),
}

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuditError::Serialization(message) => {
                write!(f, "falha ao serializar evento: {message}")
            }
            AuditError::Io(message) => write!(f, "falha de I/O: {message}"),
            AuditError::Config(message) => write!(f, "configuração inválida: {message}"),
            AuditError::ImmutableLog(message) => write!(f, "{message}"),
        }
    }
}

impl From<crate::immutablelog_client::ImmutableLogClientError> for AuditError {
    fn from(err: crate::immutablelog_client::ImmutableLogClientError) -> Self {
        AuditError::ImmutableLog(err.to_string())
    }
}

// Implementar `std::error::Error` é o que torna `AuditError` um "erro"
// de verdade aos olhos do ecossistema Rust (permite usar `?` em funções
// que retornam `Box<dyn Error>`, compor com outras libs, etc).
impl std::error::Error for AuditError {}

// Esta é a peça que cumpre literalmente a regra "converter erros Rust
// para exceções Python". Implementando `From<AuditError> for PyErr`,
// qualquer função que devolva `Result<T, AuditError>` pode ser chamada
// com `?` dentro de um método `#[pymethods]` que devolve `PyResult<T>`
// (que é só um alias para `Result<T, PyErr>`) — a conversão acontece
// automaticamente, sem `match` manual em cada chamada.
impl From<AuditError> for pyo3::PyErr {
    fn from(err: AuditError) -> pyo3::PyErr {
        match err {
            // Erro de serialização -> ValueError: o problema é o
            // *conteúdo* do evento (não serializável ou JSON inválido),
            // categoria de erro que o Python já reconhece bem.
            AuditError::Serialization(message) => {
                pyo3::exceptions::PyValueError::new_err(message)
            }
            // Erro de I/O -> OSError: o mesmo tipo de exceção que o
            // próprio Python levanta para problemas de arquivo/disco
            // (`open()`, por exemplo), então o comportamento já é
            // familiar para quem usa a lib.
            AuditError::Io(message) => pyo3::exceptions::PyOSError::new_err(message),
            // Erro de configuração -> ValueError: o problema é um
            // argumento inválido passado pelo chamador (mesma categoria
            // que o Python já usa para argumentos inválidos).
            AuditError::Config(message) => pyo3::exceptions::PyValueError::new_err(message),
            // Erro de delivery -> RuntimeError: a operação local (criar
            // e hashear o evento) funcionou; o que falhou foi contatar
            // um serviço externo, categoria que o Python não tem uma
            // exceção mais específica e amplamente reconhecida para.
            AuditError::ImmutableLog(message) => pyo3::exceptions::PyRuntimeError::new_err(message),
        }
    }
}
