"""
Exemplo de rust_py_audit com a integração ImmutableLog (modo hybrid).

Rode com:
    export IMMUTABLELOG_URL=https://api.immutablelog.com
    export IMMUTABLELOG_API_KEY=iml_live_xxxxx
    python examples/immutablelog_basic.py
"""
from rust_py_audit import AuditLogger


def main() -> None:
    # mode/immutablelog_url/immutablelog_api_key não são passados aqui
    # de propósito — caem no fallback de RUST_PY_AUDIT_MODE /
    # IMMUTABLELOG_URL / IMMUTABLELOG_API_KEY (ver topo deste arquivo).
    # Também dá pra passar tudo explicitamente, como no README.
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

    if "immutablelog" in event:
        print("\nStatus de entrega ao ImmutableLog:", event["immutablelog"]["status"])

        if event["immutablelog"]["status"] == "pending":
            print("\nTentando reentregar eventos pendentes...")
            print(audit.flush_pending())
    else:
        print("\nmode='local' (default) — nenhum envio ao ImmutableLog foi tentado.")


if __name__ == "__main__":
    main()
