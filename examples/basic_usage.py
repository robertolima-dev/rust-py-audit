"""
Exemplo básico de rust_py_audit, sem nenhum framework web.

Rode com:
    python examples/basic_usage.py
"""
from rust_py_audit import AuditLogger


def main() -> None:
    audit = AuditLogger(app_name="billing-api", file_path="./audit.jsonl")

    event = audit.log(
        actor_id="user_123",
        action="DELETE_INVOICE",
        resource="invoice",
        resource_id="inv_987",
        metadata={"ip": "192.168.0.10", "reason": "duplicate invoice"},
    )
    print("Evento registrado:")
    print(event)

    audit.log(
        actor_id="user_1",
        action="LOGIN",
        resource="session",
        resource_id="session_123",
    )

    print("\nHash do último evento:", audit.last_hash())

    result = audit.verify()
    print("\nResultado de verify():")
    print(result)


if __name__ == "__main__":
    main()
