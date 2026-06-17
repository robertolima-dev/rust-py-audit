use pyo3::prelude::*;

// `mod event;` declara que existe um arquivo `src/event.rs` e o conecta
// como submódulo deste crate. `pub` faz `event::AuditEvent` ser visível
// fora deste arquivo (vamos precisar disso em `audit_logger.rs`,
// `storage.rs`, `hash.rs` e `verifier.rs`, criados nas próximas etapas).
pub mod audit_logger;
pub mod errors;
pub mod event;
pub mod hash;
pub mod immutablelog_client;
pub mod immutablelog_config;
pub mod immutablelog_receipt;
pub mod pyconv;
pub mod retry;
pub mod storage;
pub mod verifier;

use audit_logger::AuditLogger;

/// Função de exemplo, só para validar a ponte Rust <-> Python.
///
/// `#[pyfunction]` gera o código de "cola" que expõe esta função Rust
/// como uma função Python chamável. Note o tipo de retorno: `String`
/// (owned, dona dos seus bytes) e não `&str` (uma referência/borrow).
/// Isso é obrigatório aqui: `&str` precisaria viver pelo menos tanto
/// quanto quem chama, mas a string é criada *dentro* da função e
/// morreria ao sair do escopo — então ela precisa ser movida (owned)
/// para fora. PyO3 converte esse `String` automaticamente num `str`
/// do Python.
#[pyfunction]
fn hello() -> String {
    "rust_py_audit is alive 🦀🐍".to_string()
}

/// Módulo nativo exposto ao Python.
///
/// O nome aqui ("_rust_py_audit") tem que bater com o `module-name` que
/// configuramos em `pyproject.toml`. O Python importa este módulo nativo
/// e o `python/rust_py_audit/__init__.py` reexporta o que for público.
#[pymodule]
fn _rust_py_audit(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // `wrap_pyfunction!` transforma a função Rust marcada com
    // `#[pyfunction]` em um objeto Python e `m.add_function` a registra
    // dentro do módulo, sob o mesmo nome ("hello").
    m.add_function(wrap_pyfunction!(hello, m)?)?;
    // `add_class` registra o `#[pyclass]` no módulo, tornando
    // `AuditLogger` instanciável do lado Python como
    // `_rust_py_audit.AuditLogger(...)` (e, via __init__.py, como
    // `rust_py_audit.AuditLogger(...)`).
    m.add_class::<AuditLogger>()?;
    Ok(())
}

// `#[cfg(test)]` faz este módulo só ser compilado quando rodamos
// `cargo test` — ele nem existe no binário final que vai pro PyPI.
// Rodamos com `cargo test --no-default-features` (veja o comentário
// sobre features no Cargo.toml) para evitar o erro de linker do
// "extension-module".
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_returns_non_empty_string() {
        let message = hello();
        assert!(!message.is_empty());
    }
}
