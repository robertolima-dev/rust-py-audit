# rust-py-audit

[![PyPI](https://img.shields.io/pypi/v/rust-py-audit?color=e8673a&label=PyPI)](https://pypi.org/project/rust-py-audit/)
[![Python](https://img.shields.io/pypi/pyversions/rust-py-audit?color=4b8bbe)](https://pypi.org/project/rust-py-audit/)
[![License](https://img.shields.io/pypi/l/rust-py-audit?color=3fb950)](https://github.com/robertolima-dev/rust-py-audit/blob/main/LICENSE)
[![GitHub](https://img.shields.io/github/stars/robertolima-dev/rust-py-audit?style=flat&color=e8673a)](https://github.com/robertolima-dev/rust-py-audit)

🌐 **[rust-py-audit.vercel.app](https://rust-py-audit.vercel.app/)**

Event audit logging library for Python applications, with a Rust core.

Records audit events (`who did what, when, on which resource`) in a fast, structured way, and chains each event to the previous one with SHA-256 — any later edit, deletion, or reordering of the log file is detectable with `verify()`.

---

## Features

- **`AuditLogger`** — simple API: `log(...)`, `verify()`, `last_hash()`
- **Hash chain (SHA-256)** — each event embeds the hash of the previous event; altering any recorded event breaks the chain in a detectable way
- **JSONL storage** — one event per line, append-only, no database required
- **Thread-safe** — a single `AuditLogger` can be shared across threads (e.g. a multi-threaded WSGI server, or one middleware instance serving concurrent requests); the hash chain stays linear under concurrency
- **Free-form `metadata`** — any JSON-serializable `dict` (IP, reason, request_id, etc.)
- **FastAPI middleware** — automatically logs state-changing requests (POST/PUT/PATCH/DELETE)
- **Django middleware** — same idea, supports WSGI and ASGI
- **[ImmutableLog](https://immutablelog.com/en/) integration** — `local`/`remote`/`hybrid` modes, automatic retry, pending queue, and `flush_pending()` (see [dedicated section](#immutablelog-integration))
- **Rust core** — hash generation, serialization, I/O, and the ImmutableLog HTTP client all run in Rust via PyO3; the Python API stays simple

---

## Requirements

- Python 3.10+
- No required runtime dependencies

Optional, installed separately:
- `fastapi` + `starlette` — for `rust_py_audit.fastapi.AuditMiddleware`
- `django` — for `rust_py_audit.django.AuditMiddleware`

---

## Installation

```bash
pip install rust-py-audit
```

With optional extras:

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
print(event["hash"])   # sha256, 64 hex characters

print(audit.last_hash())  # hash of the last recorded event

result = audit.verify()
print(result)
# {"valid": True, "total_events": 1, "last_hash": "..."}
```

---

## Chain integrity

Each event records the hash of the previous event (`previous_hash`) and its own hash (`hash`), computed from the event's content + `previous_hash`. The first event in the chain has `previous_hash = null`.

```json
{"id":"evt_123","timestamp":"2026-06-17T10:00:00Z","app_name":"billing-api","actor_id":"user_123","action":"DELETE_INVOICE","resource":"invoice","resource_id":"inv_987","metadata":{"ip":"192.168.0.10"},"previous_hash":null,"hash":"abc123..."}
```

`verify()` re-reads the file from scratch and recomputes everything — it never trusts any in-memory cache:

```python
result = audit.verify()
```

If the chain is intact:

```python
{"valid": True, "total_events": 10, "last_hash": "..."}
```

If any event was edited, removed, or reordered:

```python
{"valid": False, "total_events": 10, "error_index": 4, "reason": "hash_mismatch"}
# or "reason": "broken_chain" (removed/reordered/forged event)
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

By default, only `POST`/`PUT`/`PATCH`/`DELETE` requests are logged. `actor_id` comes from the `X-User-Id` header (adjustable via `actor_header=`); falls back to `"anonymous"` if absent.

See the full example in [`examples/fastapi_app.py`](examples/fastapi_app.py).

---

## Django

```python
# settings.py
MIDDLEWARE = [
    "rust_py_audit.django.AuditMiddleware",
    # ... other middlewares ...
]

# Optional:
RUST_PY_AUDIT_APP_NAME = "my-django-app"
RUST_PY_AUDIT_FILE_PATH = "./audit.jsonl"
RUST_PY_AUDIT_METHODS = {"POST", "PUT", "PATCH", "DELETE"}
```

`actor_id` comes from `request.user.pk` when there's an authenticated user (via `django.contrib.auth`); falls back to `"anonymous"` otherwise. The middleware supports both WSGI and ASGI applications automatically.

See the full example in [`examples/django_example/`](examples/django_example/).

---

## ImmutableLog Integration

`rust_py_audit` can send each audit event to [ImmutableLog](https://immutablelog.com/en/) ([documentation](https://immutablelog.com/en/documentation/)), in addition to — or instead of — writing locally.

### Operating modes

| `mode` | Writes local JSONL | Sends to ImmutableLog | Typical use |
|---|---|---|---|
| `"local"` (default) | ✅ | ❌ | The library's original behavior, no external dependency |
| `"remote"` | ❌ (except for pending entries, see below) | ✅ | ImmutableLog is the single source of truth; a delivery failure raises an exception |
| `"hybrid"` | ✅ | ✅ | Local chain + remote receipt; a delivery failure NEVER raises — a transient (retryable) failure becomes `status="pending"` (queued for `flush_pending()`), a permanent one becomes `status="failed"` (not queued) |

`mode="local"` is the default — existing code calling `AuditLogger(app_name, file_path)` keeps working unchanged.

### Basic example

```python
from rust_py_audit import AuditLogger

audit = AuditLogger(
    app_name="billing-api",
    file_path="./audit.jsonl",
    mode="hybrid",
    immutablelog_url="https://api.immutablelog.com",
    immutablelog_api_key="iml_live_xxxxx",
    timeout_ms=500,
    retry_enabled=True,
    max_retries=3,
)

event = audit.log(
    actor_id="user_123",
    action="DELETE_INVOICE",
    resource="invoice",
    resource_id="inv_987",
    metadata={"ip": "192.168.0.10", "reason": "duplicate invoice"},
)

print(event["immutablelog"])
# {"status": "delivered", "tx_id": "tx_...", "payload_hash": "...", ...}
# or {"status": "pending", "tx_id": None, ...} on a transient failure (hybrid mode)
# or {"status": "failed", "tx_id": None, ...} on a permanent failure (hybrid mode)

# Retries delivery of every event still marked "pending":
print(audit.flush_pending())
# {"flushed": 1, "failed": 0, "still_pending": 0, "total": 1}
```

### Environment variables

`mode`, `immutablelog_url`, and `immutablelog_api_key` accept `None` (the default) to fall back to an environment variable — handy for not hardcoding credentials:

```bash
export RUST_PY_AUDIT_MODE=hybrid
export IMMUTABLELOG_URL=https://api.immutablelog.com
export IMMUTABLELOG_API_KEY=iml_live_xxxxx
```

```python
# Without passing mode/immutablelog_url/immutablelog_api_key explicitly,
# they come from the environment variables above:
audit = AuditLogger(app_name="billing-api", file_path="./audit.jsonl")
```

An explicit parameter always takes priority over the environment variable. `mode="remote"`/`"hybrid"` without `immutablelog_url`/`immutablelog_api_key` (neither as a parameter nor as an env var) raises `ValueError` when the `AuditLogger` is created — failing fast instead of only on the first `log()` call.

### Severity, `immutable_trail`, and `env`

`audit.log(...)` accepts two optional parameters that only affect what gets sent to ImmutableLog (they never enter the hash):

```python
event = audit.log(
    actor_id="user_123",
    action="DELETE_INVOICE",
    resource="invoice",
    resource_id="inv_987",
    severity="error",                    # meta.type — defaults to "info" if omitted
    immutable_trail="order-2026-00441",  # meta.immutable_trail — groups related events
)
```

- `severity` must be one of `"error"`, `"warning"`, `"info"`, `"success"` — any other value raises `ValueError`, in any `mode` (even `"local"`, where `severity` is just stored without being used).
- `immutable_trail` is sanitized automatically (trimmed, `:` replaced with `-`, truncated at 256 chars); if it ends up empty after that, the field is omitted instead of being sent broken.
- Both are preserved locally (without affecting `hash`) precisely so that `flush_pending()` can resend later with the same original classification.
- `immutablelog_env` (on the `AuditLogger` constructor, falling back to the `IMMUTABLELOG_ENV` env var) sets `meta.env` — useful for telling `staging`/`production` apart in ImmutableLog.

### FastAPI

```python
from fastapi import FastAPI
from rust_py_audit.fastapi import AuditMiddleware

app = FastAPI()
app.add_middleware(
    AuditMiddleware,
    app_name="billing-api",
    file_path="./audit.jsonl",
    mode="hybrid",
    immutablelog_url="https://api.immutablelog.com",
    immutablelog_api_key="iml_live_xxxxx",
    immutablelog_env="production",
    trail_header="X-Audit-Trail",  # default — read from the request, becomes meta.immutable_trail
)
```

In `mode="remote"`/`"hybrid"`, the middleware computes `severity` automatically from the response's `status_code` (`>=400` → `"error"`, `300-399` → `"info"`, `200-299` → `"success"`). In `mode="remote"`, if delivery fails the middleware logs a `logging.warning(...)` and moves on — an audit failure never takes down the actual response already computed by the application.

### Django

```python
# settings.py
MIDDLEWARE = [
    "rust_py_audit.django.AuditMiddleware",
    # ... other middlewares ...
]

RUST_PY_AUDIT_MODE = "hybrid"
RUST_PY_AUDIT_FILE_PATH = "./audit.jsonl"
RUST_PY_AUDIT_IMMUTABLELOG_URL = "https://api.immutablelog.com"
RUST_PY_AUDIT_IMMUTABLELOG_API_KEY = "iml_live_xxxxx"
RUST_PY_AUDIT_IMMUTABLELOG_ENV = "production"
RUST_PY_AUDIT_TRAIL_HEADER = "X-Audit-Trail"  # default — read from the request, becomes meta.immutable_trail
```

Same behavior as FastAPI: `severity` computed from `status_code`, and delivery failures logged via `logging.warning(...)` without affecting the response.

### Retry and idempotency

- `retry_enabled`/`max_retries` control how many times a **retryable** failure is retried (the same `Idempotency-Key` is used on every attempt — never creates duplicate events on ImmutableLog).
- **Retryable**: `5xx` and timeouts.
- **Permanent** (never retried): `400`, `401`, `403`, `429`, and any other client error.
- In `mode="remote"`, exhausting retries (or a permanent error) raises `RuntimeError`.
- In `mode="hybrid"`, delivery never raises:
  - a **retryable** failure (5xx/timeout) marks the event `status="pending"` and queues it in `audit.pending.jsonl` — call `audit.flush_pending()` (manually, or from a cron/worker) to retry later;
  - a **permanent** failure (4xx) marks the event `status="failed"` and does **not** queue it (retrying would never succeed, so it stays out of the queue instead of getting stuck there forever). The event is still recorded locally and the chain stays valid — `"failed"` is just operational metadata.

In `mode="hybrid"`, the event is appended to the local JSONL **once**, already carrying its final receipt (an O(1) append per event, not a full-file rewrite). A network failure still records the event locally (as `pending`/`failed`), so it is never lost to a failed delivery; the only loss window is the process being killed mid-request, and even then a delivery that did reach ImmutableLog is preserved in the remote store.

### Integrity guarantee

The local hash (`event["hash"]`) is computed **before** any delivery attempt and never includes the `immutablelog` field — the remote receipt is operational metadata, attached afterward, and never invalidates `verify()`:

```python
audit.log(...)        # hash computed, event already recorded/chained
audit.flush_pending()  # only updates event["immutablelog"]; event["hash"] doesn't change
audit.verify()         # still valid, even after flush_pending()
```

---

## API Reference

### `AuditLogger(app_name, file_path="./audit.jsonl", mode=None, immutablelog_url=None, immutablelog_api_key=None, timeout_ms=500, retry_enabled=True, max_retries=3, immutablelog_env=None)`

| Parameter | Type | Description |
|---|---|---|
| `app_name` | `str` | Application name, recorded on every event |
| `file_path` | `str` | Path to the JSONL file. If it already exists, the chain resumes from the last recorded hash |
| `mode` | `str \| None` | `"local"` (default) / `"remote"` / `"hybrid"`. `None` falls back to `RUST_PY_AUDIT_MODE`, and finally to `"local"` |
| `immutablelog_url` | `str \| None` | ImmutableLog base URL. `None` falls back to `IMMUTABLELOG_URL`. Required (one way or another) in `mode="remote"`/`"hybrid"` |
| `immutablelog_api_key` | `str \| None` | API key (`Bearer`). `None` falls back to `IMMUTABLELOG_API_KEY`. Same requirement as `immutablelog_url` |
| `timeout_ms` | `int` | HTTP request timeout to ImmutableLog, in milliseconds |
| `retry_enabled` | `bool` | If `True`, retries retryable errors (5xx, timeout) up to `max_retries` times |
| `max_retries` | `int` | Maximum number of retries (in addition to the initial attempt) |
| `immutablelog_env` | `str \| None` | Logical environment (`meta.env`, e.g. `"production"`). `None` falls back to `IMMUTABLELOG_ENV`; if neither is set, the field is omitted |

See [ImmutableLog Integration](#immutablelog-integration) for details on each mode.

---

### `audit.log(actor_id, action, resource, resource_id, metadata=None, severity=None, immutable_trail=None) → dict`

Records an event and returns the full event (already with `id`, `timestamp`, `hash`, etc.) as a `dict`. `severity`/`immutable_trail` are optional and only affect delivery to ImmutableLog — see [Severity, immutable_trail, and env](#severity-immutable_trail-and-env).

| Event field | Type | Description |
|---|---|---|
| `id` | `str` | UUID v4 |
| `timestamp` | `str` | RFC3339 / UTC, e.g.: `2026-06-17T10:00:00Z` |
| `app_name` | `str` | Comes from the `AuditLogger` |
| `actor_id` | `str` | Who performed the action |
| `action` | `str` | E.g.: `DELETE_INVOICE` |
| `resource` | `str` | E.g.: `invoice` |
| `resource_id` | `str` | E.g.: `inv_987` |
| `metadata` | `dict` | Free-form — any JSON-serializable value |
| `previous_hash` | `str \| None` | Hash of the previous event in the chain |
| `hash` | `str` | SHA-256 (64 hex chars) of the event + `previous_hash` |
| `severity` | `str \| absent` | Only present if passed to `log()`. Becomes `meta.type` on ImmutableLog |
| `immutable_trail` | `str \| absent` | Only present if passed to `log()` (and not empty after sanitization). Becomes `meta.immutable_trail` |
| `immutablelog` | `dict \| absent` | Only present in `mode="remote"`/`"hybrid"`. `status` is `"delivered"`, `"pending"` (transient failure, queued), or `"failed"` (permanent failure, not queued); other fields (`tx_id`, `payload_hash`, `duplicate`, `request_id`, ...) come from the ImmutableLog response |

In `mode="remote"`, a permanent failure or exhausted retries raise `RuntimeError` instead of returning the dict.

---

### `audit.verify() → dict`

Re-reads the file and revalidates the entire chain from scratch. See [Chain integrity](#chain-integrity). Unaffected by the `immutablelog` field — only the hashed fields matter (see [Integrity guarantee](#integrity-guarantee)).

---

### `audit.last_hash() → str | None`

Hash of the last recorded event (in-memory cache, O(1)) — `None` if no event has been recorded yet.

---

### `audit.flush_pending() → dict`

Attempts to redeliver to ImmutableLog every event marked as `pending` (recorded in `audit.pending.jsonl`, derived from `file_path`). Only relevant in `mode="hybrid"` — other modes never populate this queue.

```python
{"flushed": 1, "failed": 0, "still_pending": 0, "total": 1}
```

For each queued event:
- **delivered** → updates `event["immutablelog"]` in `audit.jsonl` to `"delivered"` (without changing `hash`) and removes it from the queue (counts toward `flushed`);
- **permanent failure** → marks it `"failed"` in `audit.jsonl` and removes it from the queue, so it doesn't stay stuck forever (counts toward `failed`);
- **retryable failure** → left in the queue for the next call (counts toward `still_pending`).

---

## Building from Source

Requires Rust and [maturin](https://github.com/PyO3/maturin).

```bash
git clone https://github.com/robertolima-dev/rust-py-audit
cd rust-py-audit

python3 -m venv .venv
source .venv/bin/activate
pip install maturin

# Development build (installs into the current Python environment)
maturin develop

# Release wheel
maturin build --release
```

### Running tests

```bash
# Rust unit tests
cargo test --no-default-features

# Python integration tests
pip install -e ".[dev]"
pytest tests/
```

---

## Architecture

```
Python API (rust_py_audit)
    ├── AuditLogger(...)         ──► src/audit_logger.rs (PyO3 #[pyclass])
    │       ├── log()            ──► src/event.rs               (AuditEvent)
    │       │                    ──► src/hash.rs                (deterministic SHA-256)
    │       │                    ──► src/storage.rs             (append/update in JSONL)
    │       │                    ──► src/immutablelog_client.rs (POST /v1/events, via reqwest)
    │       │                    ──► src/retry.rs               (retry with Idempotency-Key)
    │       ├── verify()         ──► src/verifier.rs (revalidates the local chain)
    │       ├── flush_pending()  ──► redelivers audit.pending.jsonl
    │       └── last_hash()      ──► in-memory cache
    │
    ├── fastapi.AuditMiddleware ──► audit.log() on every mutating request
    └── django.AuditMiddleware  ──► same idea, WSGI/ASGI
```

`src/immutablelog_config.rs` holds `AuditMode`/`ImmutableLogConfig`; `src/immutablelog_receipt.rs` defines the `ImmutableLogReceipt` attached to each event.

The core is compiled into a native extension (`.so`/`.pyd`) by [maturin](https://github.com/PyO3/maturin) and [PyO3](https://pyo3.rs). The Python layer is thin — it just routes calls and provides the framework adapters.

---

## License

MIT — see [LICENSE](LICENSE).
