# rust-py-audit

[![PyPI](https://img.shields.io/pypi/v/rust-py-audit?color=e8673a&label=PyPI)](https://pypi.org/project/rust-py-audit/)
[![Python](https://img.shields.io/pypi/pyversions/rust-py-audit?color=4b8bbe)](https://pypi.org/project/rust-py-audit/)
[![License](https://img.shields.io/pypi/l/rust-py-audit?color=3fb950)](https://github.com/robertolima-dev/rust-py-audit/blob/main/LICENSE)
[![GitHub](https://img.shields.io/github/stars/robertolima-dev/rust-py-audit?style=flat&color=e8673a)](https://github.com/robertolima-dev/rust-py-audit)

Biblioteca de auditoria de eventos para aplicações Python, com core em Rust.

Registra eventos de auditoria (`quem fez o quê, quando, em qual recurso`) de forma rápida e estruturada, e encadeia cada evento ao anterior com SHA-256 — qualquer edição, remoção ou reordenação posterior do arquivo de log é detectável com `verify()`.

---

## Features

- **`AuditLogger`** — API simples: `log(...)`, `verify()`, `last_hash()`
- **Cadeia de hashes (SHA-256)** — cada evento inclui o hash do evento anterior; alterar qualquer evento gravado quebra a cadeia de forma detectável
- **Armazenamento em JSONL** — um evento por linha, append-only, sem necessidade de banco de dados
- **`metadata` livre** — qualquer `dict` JSON-serializável (IP, motivo, request_id, etc.)
- **Middleware FastAPI** — registra automaticamente requisições que alteram estado (POST/PUT/PATCH/DELETE)
- **Middleware Django** — mesma ideia, suporta WSGI e ASGI
- **Core em Rust** — geração de hash, serialização e I/O acontecem em Rust via PyO3; a API Python permanece simples

---

## Requirements

- Python 3.10+
- Nenhuma dependência obrigatória em runtime

Opcionais, instaladas separadamente:
- `fastapi` + `starlette` — para `rust_py_audit.fastapi.AuditMiddleware`
- `django` — para `rust_py_audit.django.AuditMiddleware`

---

## Installation

```bash
pip install rust-py-audit
```

Com extras opcionais:

```bash
pip install "rust-py-audit[fastapi]"
pip install "rust-py-audit[django]"
```

---

## Quick Start

```python
from rust_py_audit import AuditLogger

audit = AuditLogger(app_name="billing-api", file_path="./audit.jsonl")

event = audit.log(
    actor_id="user_123",
    action="DELETE_INVOICE",
    resource="invoice",
    resource_id="inv_987",
    metadata={"ip": "192.168.0.10", "reason": "duplicate invoice"},
)

print(event["id"])     # uuid v4
print(event["hash"])   # sha256, 64 caracteres hex

print(audit.last_hash())  # hash do último evento gravado

result = audit.verify()
print(result)
# {"valid": True, "total_events": 1, "last_hash": "..."}
```

---

## Integridade da cadeia

Cada evento grava o hash do evento anterior (`previous_hash`) e o próprio hash (`hash`), calculado a partir do conteúdo do evento + `previous_hash`. O primeiro evento da cadeia tem `previous_hash = null`.

```json
{"id":"evt_123","timestamp":"2026-06-17T10:00:00Z","app_name":"billing-api","actor_id":"user_123","action":"DELETE_INVOICE","resource":"invoice","resource_id":"inv_987","metadata":{"ip":"192.168.0.10"},"previous_hash":null,"hash":"abc123..."}
```

`verify()` relê o arquivo do zero e recalcula tudo — não confia em nenhum cache em memória:

```python
result = audit.verify()
```

Se a cadeia estiver intacta:

```python
{"valid": True, "total_events": 10, "last_hash": "..."}
```

Se algum evento foi editado, removido ou reordenado:

```python
{"valid": False, "total_events": 10, "error_index": 4, "reason": "hash_mismatch"}
# ou "reason": "broken_chain" (evento removido/reordenado/forjado)
```

---

## FastAPI

```python
from fastapi import FastAPI
from rust_py_audit.fastapi import AuditMiddleware

app = FastAPI()
app.add_middleware(AuditMiddleware, app_name="billing-api", file_path="./audit.jsonl")


@app.delete("/invoices/{invoice_id}")
async def delete_invoice(invoice_id: str):
    return {"deleted": invoice_id}
```

Por padrão, só requisições `POST`/`PUT`/`PATCH`/`DELETE` são registradas. `actor_id` vem do header `X-User-Id` (ajustável via `actor_header=`); cai para `"anonymous"` se ausente.

Ver exemplo completo em [`examples/fastapi_app.py`](examples/fastapi_app.py).

---

## Django

```python
# settings.py
MIDDLEWARE = [
    "rust_py_audit.django.AuditMiddleware",
    # ... outros middlewares ...
]

# Opcional:
RUST_PY_AUDIT_APP_NAME = "my-django-app"
RUST_PY_AUDIT_FILE_PATH = "./audit.jsonl"
RUST_PY_AUDIT_METHODS = {"POST", "PUT", "PATCH", "DELETE"}
```

`actor_id` vem de `request.user.pk` quando há um usuário autenticado (via `django.contrib.auth`); cai para `"anonymous"` caso contrário. O middleware suporta tanto aplicações WSGI quanto ASGI automaticamente.

Ver exemplo completo em [`examples/django_example/`](examples/django_example/).

---

## API Reference

### `AuditLogger(app_name, file_path="./audit.jsonl")`

| Parâmetro | Tipo | Descrição |
|---|---|---|
| `app_name` | `str` | Nome da aplicação, gravado em todo evento |
| `file_path` | `str` | Caminho do arquivo JSONL. Se já existir, a cadeia é retomada a partir do último hash gravado |

---

### `audit.log(actor_id, action, resource, resource_id, metadata=None) → dict`

Registra um evento e devolve o evento completo (já com `id`, `timestamp`, `hash`, etc.) como `dict`.

| Campo do evento | Tipo | Descrição |
|---|---|---|
| `id` | `str` | UUID v4 |
| `timestamp` | `str` | RFC3339 / UTC, ex: `2026-06-17T10:00:00Z` |
| `app_name` | `str` | Vem do `AuditLogger` |
| `actor_id` | `str` | Quem realizou a ação |
| `action` | `str` | Ex: `DELETE_INVOICE` |
| `resource` | `str` | Ex: `invoice` |
| `resource_id` | `str` | Ex: `inv_987` |
| `metadata` | `dict` | Livre — qualquer JSON serializável |
| `previous_hash` | `str \| None` | Hash do evento anterior na cadeia |
| `hash` | `str` | SHA-256 (64 hex chars) do evento + `previous_hash` |

---

### `audit.verify() → dict`

Relê o arquivo e revalida a cadeia inteira do zero. Ver [Integridade da cadeia](#integridade-da-cadeia).

---

### `audit.last_hash() → str | None`

Hash do último evento gravado (cache em memória, O(1)) — `None` se nenhum evento foi registrado ainda.

---

## Building from Source

Requer Rust e [maturin](https://github.com/PyO3/maturin).

```bash
git clone https://github.com/robertolima-dev/rust-py-audit
cd rust-py-audit

python3 -m venv .venv
source .venv/bin/activate
pip install maturin

# Build de desenvolvimento (instala no ambiente Python atual)
maturin develop

# Wheel de release
maturin build --release
```

### Running tests

```bash
# Testes unitários em Rust
cargo test --no-default-features

# Testes de integração em Python
pip install -e ".[dev]"
pytest tests/
```

---

## Architecture

```
Python API (rust_py_audit)
    ├── AuditLogger(...)        ──► src/audit_logger.rs (PyO3 #[pyclass])
    │       ├── log()           ──► src/event.rs    (AuditEvent)
    │       │                   ──► src/hash.rs     (SHA-256 determinístico)
    │       │                   ──► src/storage.rs  (append em JSONL)
    │       ├── verify()        ──► src/verifier.rs (revalida a cadeia)
    │       └── last_hash()     ──► cache em memória
    │
    ├── fastapi.AuditMiddleware ──► audit.log() a cada request mutante
    └── django.AuditMiddleware  ──► idem, WSGI/ASGI
```

O core é compilado para uma extensão nativa (`.so`/`.pyd`) por [maturin](https://github.com/PyO3/maturin) e [PyO3](https://pyo3.rs). A camada Python é fina — só roteia chamadas e oferece os adaptadores de framework.

---

## License

MIT — ver [LICENSE](LICENSE).
