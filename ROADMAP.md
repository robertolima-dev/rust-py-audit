# Roadmap — rust-py-audit

Direction for `rust-py-audit`: a tamper-evident audit log for Python with a Rust
core (PyO3 + maturin). The library is already mature (v0.3.0) — the SHA-256 hash
chain, JSONL storage, FastAPI/Django middlewares, and the ImmutableLog
`local`/`remote`/`hybrid` integration are shipped and stable. This document tracks
what is done and the **directional** ideas under consideration.

> Status legend: ✅ shipped · 🔜 planned (next) · 💡 idea (no version yet) · ⚠️ note

> ⚠️ Beyond "shipped", items below are **inferred directions, not commitments**.
> Confirm priorities with the maintainer before planning a release around them.

---

## Shipped — up to v0.3.0

- ✅ `AuditLogger` with `log(...)`, `verify()`, `last_hash()`.
- ✅ **SHA-256 hash chain** — each event embeds the previous event's hash; any
  edit, deletion, or reordering is detected by `verify()` (re-read from disk,
  never trusts an in-memory cache).
- ✅ Append-only **JSONL** storage — one event per line, no database.
- ✅ Thread-safe — the chain stays linear under concurrency.
- ✅ Free-form JSON-serializable `metadata`.
- ✅ FastAPI and Django middlewares (log state-changing requests:
  POST/PUT/PATCH/DELETE).
- ✅ **ImmutableLog integration** — `local`/`remote`/`hybrid` modes, automatic
  retry with idempotency key, pending queue (`audit.pending.jsonl`),
  `flush_pending()`, and transient (`pending`) vs permanent (`failed`) failure
  handling that never takes down the application response.
- ✅ `severity`, `immutable_trail`, and `env` delivery metadata (never enter the
  hash, so `verify()` stays valid after `flush_pending()`).
- ✅ ImmutableLog HTTP client runs in the Rust core (PyO3 + reqwest).

---

## Directional ideas (no version assigned — confirm before planning)

- 💡 **Log rotation / segmentation** — roll the JSONL by size or date while
  preserving chain continuity across segments (carry the last hash forward).
- 💡 **Incremental verification** — verify only events appended since the last
  known-good hash, instead of re-reading the whole file every time.
- 💡 **More delivery backends** — additional sinks alongside ImmutableLog (e.g.
  S3/object storage, syslog, a generic webhook), reusing the retry/pending logic.
- 💡 **Async client** — non-blocking delivery path for high-throughput async apps.
- 💡 **Flask middleware** — parity with the FastAPI/Django adapters.
- 💡 **Query/read helpers** — iterate or filter recorded events (by actor, action,
  resource, time range) without hand-parsing the JSONL.
- 💡 **Benchmarks suite** — hashing + append throughput under concurrency.

---

## Known limitations (by design, for now)

- Local JSONL is single-file and append-only (no built-in rotation yet).
- `verify()` re-reads the entire file (O(n)); fine for typical logs, see the
  incremental-verification idea above for very large files.
- ImmutableLog delivery is synchronous on the `log()` path (bounded by
  `timeout_ms` + retries); `hybrid` mode never raises, queuing transient failures.

---

## Contributing to the roadmap

Versions and ordering are indicative and may shift. Bump the version in **both**
`Cargo.toml` and `pyproject.toml` (kept in sync) plus `__version__`, ship tests
(`cargo test --no-default-features` + `pytest`), then tag `vX.Y.Z` to trigger the
release workflow (Trusted Publishing / OIDC to PyPI).
