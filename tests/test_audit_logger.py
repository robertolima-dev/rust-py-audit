import json

import pytest

from rust_py_audit import AuditLogger


def test_creates_logger_with_explicit_file_path():
    audit = AuditLogger(app_name="billing-api", file_path="./audit.jsonl")

    assert audit.app_name == "billing-api"
    assert audit.file_path == "./audit.jsonl"


def test_mode_defaults_to_local(tmp_path):
    audit = AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))

    assert audit.mode == "local"
    assert audit.immutablelog_url is None
    assert audit.timeout_ms == 500
    assert audit.retry_enabled is True
    assert audit.max_retries == 3


def test_mode_local_explicit(tmp_path):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="local",
    )

    assert audit.mode == "local"


def test_invalid_mode_raises_value_error(tmp_path):
    with pytest.raises(ValueError):
        AuditLogger(
            app_name="billing-api",
            file_path=str(tmp_path / "audit.jsonl"),
            mode="turbo",
        )


def test_remote_mode_without_credentials_raises_value_error(tmp_path):
    with pytest.raises(ValueError):
        AuditLogger(
            app_name="billing-api",
            file_path=str(tmp_path / "audit.jsonl"),
            mode="remote",
        )


def test_hybrid_mode_with_explicit_credentials(tmp_path):
    audit = AuditLogger(
        app_name="billing-api",
        file_path=str(tmp_path / "audit.jsonl"),
        mode="hybrid",
        immutablelog_url="https://api.immutablelog.com",
        immutablelog_api_key="iml_live_xxx",
        timeout_ms=2000,
        retry_enabled=False,
        max_retries=5,
    )

    assert audit.mode == "hybrid"
    assert audit.immutablelog_url == "https://api.immutablelog.com"
    assert audit.timeout_ms == 2000
    assert audit.retry_enabled is False
    assert audit.max_retries == 5


def test_mode_and_credentials_fall_back_to_env_vars(tmp_path, monkeypatch):
    monkeypatch.setenv("RUST_PY_AUDIT_MODE", "remote")
    monkeypatch.setenv("IMMUTABLELOG_URL", "https://api.immutablelog.com")
    monkeypatch.setenv("IMMUTABLELOG_API_KEY", "iml_live_from_env")

    audit = AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))

    assert audit.mode == "remote"
    assert audit.immutablelog_url == "https://api.immutablelog.com"


def test_file_path_has_a_default_value():
    audit = AuditLogger(app_name="billing-api")

    assert audit.app_name == "billing-api"
    assert audit.file_path == "./audit.jsonl"


def test_log_returns_a_well_formed_event(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    event = audit.log(
        actor_id="user_123",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_987",
        metadata={"ip": "192.168.0.10", "reason": "duplicate invoice"},
    )

    assert event["app_name"] == "billing-api"
    assert event["actor_id"] == "user_123"
    assert event["action"] == "DELETE_INVOICE"
    assert event["resource"] == "invoice"
    assert event["resource_id"] == "inv_987"
    assert event["metadata"] == {"ip": "192.168.0.10", "reason": "duplicate invoice"}
    assert event["previous_hash"] is None
    assert isinstance(event["hash"], str)
    assert len(event["hash"]) == 64


def test_log_without_metadata_defaults_to_empty_dict(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    event = audit.log(
        actor_id="user_1",
        action="LOGIN",
        resource="session",
        resource_id="session_123",
    )

    assert event["metadata"] == {}


def test_log_chains_previous_hash_across_calls(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    first = audit.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")
    second = audit.log(actor_id="user_1", action="LOGOUT", resource="session", resource_id="s1")

    assert second["previous_hash"] == first["hash"]


def test_log_persists_events_as_jsonl(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    audit.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")
    audit.log(actor_id="user_1", action="LOGOUT", resource="session", resource_id="s1")

    lines = file_path.read_text().strip().splitlines()
    assert len(lines) == 2
    assert json.loads(lines[0])["action"] == "LOGIN"
    assert json.loads(lines[1])["action"] == "LOGOUT"


def test_new_logger_resumes_chain_from_existing_file(tmp_path):
    file_path = tmp_path / "audit.jsonl"

    first_logger = AuditLogger(app_name="billing-api", file_path=str(file_path))
    first_event = first_logger.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")

    # Um novo AuditLogger, apontando para o mesmo arquivo, precisa
    # continuar a cadeia a partir do hash que já estava lá — e não
    # recomeçar do zero (previous_hash=None) como se fosse um arquivo novo.
    second_logger = AuditLogger(app_name="billing-api", file_path=str(file_path))
    second_event = second_logger.log(actor_id="user_1", action="LOGOUT", resource="session", resource_id="s1")

    assert second_event["previous_hash"] == first_event["hash"]


def test_last_hash_is_none_for_fresh_logger(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    assert audit.last_hash() is None


def test_last_hash_updates_after_each_log_call(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    first = audit.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")
    assert audit.last_hash() == first["hash"]

    second = audit.log(actor_id="user_1", action="LOGOUT", resource="session", resource_id="s1")
    assert audit.last_hash() == second["hash"]


def test_last_hash_resumes_from_existing_file(tmp_path):
    file_path = tmp_path / "audit.jsonl"

    first_logger = AuditLogger(app_name="billing-api", file_path=str(file_path))
    first_event = first_logger.log(actor_id="user_1", action="LOGIN", resource="session", resource_id="s1")

    second_logger = AuditLogger(app_name="billing-api", file_path=str(file_path))

    assert second_logger.last_hash() == first_event["hash"]
