import json

from rust_py_audit import AuditLogger


def test_flush_pending_on_empty_queue_is_a_no_op(tmp_path, immutablelog_server):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    result = audit.flush_pending()

    assert result == {"flushed": 0, "failed": 0, "still_pending": 0, "total": 0}


def test_flush_pending_retries_and_updates_receipt_on_recovery(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        retry_enabled=False,
    )

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")
    assert event["immutablelog"]["status"] == "pending"

    still_failing = audit.flush_pending()
    assert still_failing == {"flushed": 0, "failed": 0, "still_pending": 1, "total": 1}

    immutablelog_server.set_default_response(202, {
        "ok": True,
        "tx_id": "tx_flushed",
        "payload_hash": "hash_flushed",
        "status": "accepted",
        "duplicate": False,
        "request_id": "req_flushed",
    })
    recovered = audit.flush_pending()
    assert recovered == {"flushed": 1, "failed": 0, "still_pending": 0, "total": 1}

    pending_path = tmp_path / "audit.pending.jsonl"
    assert pending_path.read_text().strip() == ""

    audit_lines = (tmp_path / "audit.jsonl").read_text().strip().splitlines()
    persisted = json.loads(audit_lines[0])
    assert persisted["hash"] == event["hash"]
    assert persisted["immutablelog"]["status"] == "delivered"
    assert persisted["immutablelog"]["tx_id"] == "tx_flushed"


def test_flush_pending_keeps_local_chain_valid(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        retry_enabled=False,
    )
    audit.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")
    audit.log(actor_id="user_1", action="LOGOUT", resource="session", resource_id="s1")

    immutablelog_server.set_default_response(202, {
        "ok": True,
        "tx_id": "tx_flushed",
        "payload_hash": "hash_flushed",
        "status": "accepted",
        "duplicate": False,
        "request_id": "req_flushed",
    })
    audit.flush_pending()

    result = audit.verify()
    assert result["valid"] is True
    assert result["total_events"] == 2


def test_flush_pending_resends_with_the_original_severity_and_trail(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        retry_enabled=False,
    )

    audit.log(
        actor_id="user_1",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_1",
        severity="error",
        immutable_trail="order-42",
    )

    immutablelog_server.set_default_response(202, {
        "ok": True,
        "tx_id": "tx_flushed",
        "payload_hash": "hash_flushed",
        "status": "accepted",
        "duplicate": False,
        "request_id": "req_flushed",
    })
    audit.flush_pending()

    flush_request = immutablelog_server.requests[-1]
    assert flush_request["body"]["meta"]["type"] == "error"
    assert flush_request["body"]["meta"]["immutable_trail"] == "order-42"


def test_flush_pending_without_pending_file_is_a_no_op(tmp_path):
    audit = AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))

    result = audit.flush_pending()

    assert result == {"flushed": 0, "failed": 0, "still_pending": 0, "total": 0}


def test_flush_pending_gives_up_on_permanent_error_and_dequeues(tmp_path, immutablelog_server):
    # Um evento que ficou "pending" por uma falha transitória (500), mas
    # cuja causa virou PERMANENTE (401) na hora do flush, precisa sair da
    # fila marcado como "failed" — em vez de ficar preso para sempre (#8).
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        retry_enabled=False,
    )

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")
    assert event["immutablelog"]["status"] == "pending"

    immutablelog_server.set_default_response(401, {"ok": False, "reason": "invalid_api_key"})
    result = audit.flush_pending()
    assert result == {"flushed": 0, "failed": 1, "still_pending": 0, "total": 1}

    # Saiu da fila e ficou marcado como "failed" no arquivo principal.
    pending_path = tmp_path / "audit.pending.jsonl"
    assert pending_path.read_text().strip() == ""

    persisted = json.loads((tmp_path / "audit.jsonl").read_text().strip())
    assert persisted["immutablelog"]["status"] == "failed"
    assert persisted["hash"] == event["hash"]

    # Uma nova chamada não tem mais nada para tentar.
    assert audit.flush_pending() == {"flushed": 0, "failed": 0, "still_pending": 0, "total": 0}
