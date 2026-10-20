"""Vários processos gravando no mesmo JSONL (gunicorn com N workers).

Antes da 0.4, cada processo encadeava no `last_hash` que tinha em memória:
dois eventos saíam com o mesmo `previous_hash` e `verify()` acusava
adulteração num arquivo que ninguém tinha adulterado. O lock de arquivo entre
processos mantém a cadeia linear.
"""

import multiprocessing
import os

from rust_py_audit import AuditLogger

WORKERS = 4
EVENTS_PER_WORKER = 50


def _worker(path, worker_id, ready, go):
    audit = AuditLogger("app", path)
    ready.wait()  # todos abriram o logger antes de qualquer um gravar
    go.wait()
    for i in range(EVENTS_PER_WORKER):
        audit.log(f"worker-{worker_id}", "TICK", "job", str(i))


def _run_workers(path):
    ctx = multiprocessing.get_context("spawn")
    ready = ctx.Barrier(WORKERS)
    go = ctx.Barrier(WORKERS)
    procs = [ctx.Process(target=_worker, args=(path, w, ready, go)) for w in range(WORKERS)]
    for p in procs:
        p.start()
    for p in procs:
        p.join(timeout=60)
        assert p.exitcode == 0


def test_concurrent_processes_keep_a_single_valid_chain(tmp_path):
    path = str(tmp_path / "audit.jsonl")
    _run_workers(path)

    result = AuditLogger("app", path).verify()
    assert result["valid"] is True, result
    assert result["total_events"] == WORKERS * EVENTS_PER_WORKER


def test_logger_picks_up_events_written_by_another_process(tmp_path):
    path = str(tmp_path / "audit.jsonl")
    mine = AuditLogger("app", path)
    mine.log("me", "FIRST", "doc", "1")

    # Outro processo grava depois que `mine` já cacheou o último hash.
    ctx = multiprocessing.get_context("spawn")
    ready, go = ctx.Barrier(1), ctx.Barrier(1)
    other = ctx.Process(target=_worker, args=(path, 9, ready, go))
    other.start()
    other.join(timeout=60)
    assert other.exitcode == 0

    event = mine.log("me", "AFTER", "doc", "2")
    result = mine.verify()
    assert result["valid"] is True, result
    assert result["total_events"] == 2 + EVENTS_PER_WORKER
    # Encadeou no último evento do OUTRO processo, não no próprio cache.
    assert event["previous_hash"] is not None
    assert mine.last_hash() == event["hash"]


def test_lock_file_sits_next_to_the_log(tmp_path):
    path = tmp_path / "audit.jsonl"
    AuditLogger("app", str(path)).log("a", "B", "c", "d")
    assert os.path.exists(str(path) + ".lock")


def test_fsync_is_opt_in(tmp_path):
    assert AuditLogger("app", str(tmp_path / "a.jsonl")).fsync is False
    durable = AuditLogger("app", str(tmp_path / "b.jsonl"), fsync=True)
    assert durable.fsync is True
    durable.log("a", "B", "c", "d")
    assert durable.verify()["valid"] is True
