from rust_py_audit import AuditLogger


def test_verify_on_fresh_logger_is_valid_with_zero_events(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    result = audit.verify()

    assert result == {"valid": True, "total_events": 0, "last_hash": None}


def test_verify_after_logging_events_is_valid(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    audit.log(actor_id="u1", action="LOGIN", resource="session", resource_id="s1")
    last_event = audit.log(actor_id="u1", action="LOGOUT", resource="session", resource_id="s1")

    result = audit.verify()

    assert result["valid"] is True
    assert result["total_events"] == 2
    assert result["last_hash"] == last_event["hash"]


def test_verify_detects_a_tampered_event(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    audit.log(actor_id="u1", action="LOGIN", resource="session", resource_id="s1")
    audit.log(actor_id="u1", action="LOGOUT", resource="session", resource_id="s1")

    _tamper_line(file_path, line_index=0, action="HACKED")

    result = audit.verify()

    assert result["valid"] is False
    assert result["error_index"] == 0
    assert result["reason"] == "hash_mismatch"


def test_verify_detects_a_removed_event(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = AuditLogger(app_name="billing-api", file_path=str(file_path))

    audit.log(actor_id="u1", action="LOGIN", resource="session", resource_id="s1")
    audit.log(actor_id="u1", action="LOGOUT", resource="session", resource_id="s1")
    audit.log(actor_id="u1", action="LOGIN", resource="session", resource_id="s2")

    lines = file_path.read_text().splitlines()
    del lines[1]
    file_path.write_text("\n".join(lines) + "\n")

    result = audit.verify()

    assert result["valid"] is False
    assert result["error_index"] == 1
    assert result["reason"] == "broken_chain"


def _tamper_line(file_path, line_index, **field_overrides):
    import json

    lines = file_path.read_text().splitlines()
    event = json.loads(lines[line_index])
    event.update(field_overrides)
    lines[line_index] = json.dumps(event)
    file_path.write_text("\n".join(lines) + "\n")
