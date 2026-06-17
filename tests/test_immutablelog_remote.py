import pytest

from rust_py_audit import AuditLogger


def test_remote_success_attaches_receipt_and_skips_local_file(tmp_path, immutablelog_server):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(file_path),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert event["immutablelog"]["status"] == "delivered"
    assert event["immutablelog"]["tx_id"] == "tx_default"
    assert not file_path.exists()


def test_remote_sends_expected_headers_and_payload(tmp_path, immutablelog_server):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    event = audit.log(
        actor_id="user_1",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_1",
        metadata={"ip": "10.0.0.1"},
    )

    assert immutablelog_server.request_count == 1
    request = immutablelog_server.requests[0]
    assert request["path"] == "/v1/events"
    assert request["headers"]["authorization"] == "Bearer iml_live_ok"
    assert request["headers"]["idempotency-key"] == event["id"]
    assert request["headers"]["user-agent"].startswith("rust-py-audit/")

    body = request["body"]
    assert isinstance(body["payload"], str)
    assert body["meta"]["event_name"] == "DELETE_INVOICE"
    assert body["meta"]["service"] == "billing-api"
    assert body["meta"]["trace_id"] == event["id"]


def test_remote_401_raises_runtime_error_without_retrying(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(401, {"ok": False, "reason": "invalid_api_key"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_bad",
    )

    with pytest.raises(RuntimeError, match="invalid_api_key"):
        audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert immutablelog_server.request_count == 1


def test_remote_500_is_retried_up_to_max_retries(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(500, {"ok": False, "reason": "internal_error"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        max_retries=2,
    )

    with pytest.raises(RuntimeError, match="internal_error"):
        audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    # 1 tentativa inicial + 2 retries.
    assert immutablelog_server.request_count == 3


def test_remote_500_then_success_recovers_within_retries(tmp_path, immutablelog_server):
    immutablelog_server.queue_response(500, {"ok": False, "reason": "internal_error"})
    immutablelog_server.set_default_response(202, {
        "ok": True,
        "tx_id": "tx_recovered",
        "payload_hash": "hash_recovered",
        "status": "accepted",
        "duplicate": False,
        "request_id": "req_recovered",
    })
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        max_retries=2,
    )

    event = audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert event["immutablelog"]["tx_id"] == "tx_recovered"
    assert immutablelog_server.request_count == 2
    # Mesma Idempotency-Key nas duas tentativas.
    keys = {req["headers"]["idempotency-key"] for req in immutablelog_server.requests}
    assert keys == {event["id"]}


def test_remote_timeout_is_retryable(tmp_path, immutablelog_server):
    immutablelog_server.set_delay(0.5)
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        timeout_ms=100,
        max_retries=1,
    )

    with pytest.raises(RuntimeError, match="timeout"):
        audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    # 1 tentativa inicial + 1 retry, ambas expiraram por timeout.
    assert immutablelog_server.request_count == 2


def test_log_without_severity_omits_field_locally(tmp_path):
    audit = AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))

    event = audit.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")

    assert "severity" not in event
    assert "immutable_trail" not in event


def test_invalid_severity_raises_value_error(tmp_path):
    audit = AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))

    with pytest.raises(ValueError, match="severity"):
        audit.log(
            actor_id="user_1",
            action="DELETE_INVOICE",
            resource="invoice",
            resource_id="inv_1",
            severity="critical",
        )


def test_severity_flows_into_meta_type(tmp_path, immutablelog_server):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    event = audit.log(
        actor_id="user_1",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_1",
        severity="error",
    )

    assert event["severity"] == "error"
    assert immutablelog_server.requests[0]["body"]["meta"]["type"] == "error"


def test_immutable_trail_is_sanitized_and_sent(tmp_path, immutablelog_server):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    event = audit.log(
        actor_id="user_1",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_1",
        immutable_trail="  order:2026:00441  ",
    )

    assert event["immutable_trail"] == "order-2026-00441"
    assert immutablelog_server.requests[0]["body"]["meta"]["immutable_trail"] == "order-2026-00441"


def test_blank_immutable_trail_is_omitted(tmp_path, immutablelog_server):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    event = audit.log(
        actor_id="user_1",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_1",
        immutable_trail="   ",
    )

    assert "immutable_trail" not in event
    assert "immutable_trail" not in immutablelog_server.requests[0]["body"]["meta"]


def test_immutablelog_env_is_sent_in_meta(tmp_path, immutablelog_server):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        immutablelog_env="staging",
    )

    audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert audit.immutablelog_env == "staging"
    assert immutablelog_server.requests[0]["body"]["meta"]["env"] == "staging"


def test_immutablelog_env_falls_back_to_env_var(tmp_path, immutablelog_server, monkeypatch):
    monkeypatch.setenv("IMMUTABLELOG_ENV", "production")
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert immutablelog_server.requests[0]["body"]["meta"]["env"] == "production"


def test_remote_400_is_permanent_and_not_retried(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(400, {"ok": False, "reason": "invalid_payload"})
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
        max_retries=3,
    )

    with pytest.raises(RuntimeError, match="invalid_payload"):
        audit.log(actor_id="user_1", action="DELETE_INVOICE", resource="invoice", resource_id="inv_1")

    assert immutablelog_server.request_count == 1
