import json

from rust_py_audit import AuditLogger


def _audit_logger(tmp_path, immutablelog_server, **overrides):
    kwargs = dict(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )
    kwargs.update(overrides)
    return AuditLogger(**kwargs)


def test_hybrid_success_saves_local_and_attaches_receipt(tmp_path, immutablelog_server):
    audit = _audit_logger(tmp_path, immutablelog_server)

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert event["immutablelog"]["status"] == "delivered"
    assert event["immutablelog"]["tx_id"] == "tx_default"

    lines = (tmp_path / "audit.jsonl").read_text().strip().splitlines()
    assert len(lines) == 1
    persisted = json.loads(lines[0])
    assert persisted["hash"] == event["hash"]
    assert persisted["immutablelog"]["status"] == "delivered"


def test_hybrid_does_not_raise_on_remote_failure(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = _audit_logger(tmp_path, immutablelog_server, retry_enabled=False)

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert event["immutablelog"]["status"] == "pending"


def test_hybrid_failure_keeps_local_event_and_queues_pending_file(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(401, {"ok": False, "reason": "invalid_api_key"})
    audit = _audit_logger(tmp_path, immutablelog_server)

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    audit_lines = (tmp_path / "audit.jsonl").read_text().strip().splitlines()
    assert len(audit_lines) == 1
    persisted = json.loads(audit_lines[0])
    assert persisted["hash"] == event["hash"]
    assert persisted["immutablelog"]["status"] == "pending"

    pending_path = tmp_path / "audit.pending.jsonl"
    assert pending_path.exists()
    pending_lines = pending_path.read_text().strip().splitlines()
    assert len(pending_lines) == 1
    assert json.loads(pending_lines[0])["id"] == event["id"]


def test_hybrid_local_chain_stays_valid_after_failure(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = _audit_logger(tmp_path, immutablelog_server, retry_enabled=False)

    audit.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")
    audit.log(actor_id="user_1", action="LOGOUT", resource="session", resource_id="s1")

    result = audit.verify()
    assert result["valid"] is True
    assert result["total_events"] == 2


def test_hybrid_local_save_happens_even_when_url_is_unreachable(tmp_path):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url="http://127.0.0.1:1",  # porta inválida, conexão recusada
        immutablelog_api_key="iml_live_ok",
        retry_enabled=False,
        timeout_ms=200,
    )

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert event["immutablelog"]["status"] == "pending"
    assert (tmp_path / "audit.jsonl").exists()
