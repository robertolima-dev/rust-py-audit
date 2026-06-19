"""
Concorrência: várias threads compartilhando o MESMO `AuditLogger`.

Cenário real: servidor WSGI multi-thread (ex.: `gunicorn --threads N`)
compartilha uma única instância do middleware — e, portanto, um único
`AuditLogger` — entre as threads de requisição.

Antes do `AuditLogger` virar thread-safe (`&self` + `Mutex`, em vez de
`&mut self`), uma segunda thread chamando `log()` enquanto a primeira
tinha liberado o GIL durante a chamada de rede (`mode="remote"`/
`"hybrid"`) recebia `RuntimeError: Already borrowed` — perdendo o evento
silenciosamente. Estes testes travam justamente esse caso.
"""
import threading

from rust_py_audit import AuditLogger


def _run_concurrently(target, n):
    errors: list[Exception] = []

    def worker(i):
        try:
            target(i)
        except Exception as exc:  # noqa: BLE001 — queremos capturar QUALQUER erro
            errors.append(exc)

    threads = [threading.Thread(target=worker, args=(i,)) for i in range(n)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return errors


def test_concurrent_hybrid_logging_does_not_raise_and_keeps_chain_valid(tmp_path, immutablelog_server):
    # O delay força a sobreposição das threads: enquanto uma está no
    # envio HTTP (GIL liberado), as outras tentam chamar `log()`.
    immutablelog_server.set_delay(0.05)
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    n = 12
    errors = _run_concurrently(
        lambda i: audit.log(actor_id=f"u{i}", action="POST", resource="/r", resource_id=str(i)),
        n,
    )

    assert errors == [], f"log() concorrente levantou exceção(ões): {errors}"

    # A cadeia precisa ter exatamente N eventos e continuar íntegra —
    # nenhuma thread pode ter encadeado no mesmo previous_hash de outra
    # (o que bifurcaria/quebraria a cadeia).
    result = audit.verify()
    assert result["valid"] is True
    assert result["total_events"] == n


def test_concurrent_local_logging_keeps_chain_valid(tmp_path):
    audit = AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))

    n = 50
    errors = _run_concurrently(
        lambda i: audit.log(actor_id=f"u{i}", action="POST", resource="/r", resource_id=str(i)),
        n,
    )

    assert errors == []
    result = audit.verify()
    assert result["valid"] is True
    assert result["total_events"] == n
