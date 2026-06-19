"""
Inteiros de precisão arbitrária em `metadata`.

Python permite inteiros sem limite; o core Rust precisa aceitá-los sem
estourar `OverflowError` (regressão: antes só tentávamos `i64`). A regra
é: `i64` -> `u64` -> `f64` (fallback para inteiros acima de 2^64, com a
perda de precisão inevitável em JSON).
"""
import json

from rust_py_audit import AuditLogger


def _logger(tmp_path):
    return AuditLogger(app_name="billing-api", file_path=str(tmp_path / "audit.jsonl"))


def test_metadata_accepts_ints_beyond_i64_range(tmp_path):
    audit = _logger(tmp_path)

    above_i64 = 2**63 + 1  # > i64::MAX, ainda cabe em u64
    u64_max = 2**64 - 1  # 18446744073709551615
    negative = -(2**40)

    event = audit.log(
        actor_id="user_1",
        action="LOGIN",
        resource="session",
        resource_id="s1",
        metadata={"above_i64": above_i64, "u64_max": u64_max, "negative": negative},
    )

    # Round-trip exato, sem virar float nem estourar.
    assert event["metadata"]["above_i64"] == above_i64
    assert isinstance(event["metadata"]["above_i64"], int)
    assert event["metadata"]["u64_max"] == u64_max
    assert isinstance(event["metadata"]["u64_max"], int)
    assert event["metadata"]["negative"] == negative


def test_metadata_bigint_persists_and_chain_stays_valid(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    audit = _logger(tmp_path)

    audit.log(
        actor_id="user_1",
        action="LOGIN",
        resource="session",
        resource_id="s1",
        metadata={"snowflake": 2**63 + 12345},
    )

    # O valor grande precisa ter sido gravado como número JSON inteiro
    # (entra no hash), e a cadeia tem que continuar verificável.
    persisted = json.loads(file_path.read_text().strip())
    assert persisted["metadata"]["snowflake"] == 2**63 + 12345

    assert audit.verify()["valid"] is True


def test_metadata_int_above_u64_falls_back_to_float(tmp_path):
    audit = _logger(tmp_path)

    huge = 2**100  # acima de u64::MAX -> só representável como float

    event = audit.log(
        actor_id="user_1",
        action="LOGIN",
        resource="session",
        resource_id="s1",
        metadata={"huge": huge},
    )

    assert isinstance(event["metadata"]["huge"], float)
    assert event["metadata"]["huge"] == float(huge)
    assert audit.verify()["valid"] is True
