"""rust_py_audit: auditoria de eventos com core em Rust.

A maior parte da lógica vive na extensão nativa `_rust_py_audit`
(compilada a partir de `src/lib.rs`). Este arquivo é a "fachada" Python
pública: reexporta o que faz parte da API, sem o usuário precisar saber
que existe um módulo nativo por trás.
"""

from ._rust_py_audit import AuditLogger, hello

__version__ = "0.2.4"
__all__ = ["AuditLogger", "hello"]
