import json

import pytest
from fastapi import FastAPI
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient

from rust_py_audit.fastapi import AuditMiddleware


def build_app(**middleware_kwargs) -> FastAPI:
    app = FastAPI()
    app.add_middleware(AuditMiddleware, app_name="billing-api", **middleware_kwargs)

    @app.delete("/invoices/{invoice_id}")
    async def delete_invoice(invoice_id: str):
        return JSONResponse({"deleted": invoice_id}, status_code=200)

    @app.post("/invoices/{invoice_id}/fail")
    async def fail_invoice(invoice_id: str):
        return JSONResponse({"error": "nope"}, status_code=409)

    return app


@pytest.mark.asyncio
async def test_hybrid_mode_sends_to_immutablelog_and_keeps_local_file(tmp_path, immutablelog_server):
    file_path = tmp_path / "audit.jsonl"
    app = build_app(
        file_path=str(file_path),
        mode="hybrid",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.delete("/invoices/inv_1", headers={"X-User-Id": "user_42"})

    assert response.status_code == 200
    event = json.loads(file_path.read_text().strip())
    assert event["immutablelog"]["status"] == "delivered"
    assert immutablelog_server.request_count == 1


@pytest.mark.asyncio
async def test_remote_failure_does_not_break_the_actual_response(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(401, {"ok": False, "reason": "invalid_api_key"})
    app = build_app(
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_bad",
    )

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.delete("/invoices/inv_1")

    # A falha de auditoria (RuntimeError) é registrada e engolida — a
    # resposta real da aplicação não pode virar um 500 por causa disso.
    assert response.status_code == 200
    assert response.json() == {"deleted": "inv_1"}


@pytest.mark.asyncio
async def test_severity_is_derived_from_response_status_code(tmp_path, immutablelog_server):
    app = build_app(
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        await client.delete("/invoices/inv_1")
        await client.post("/invoices/inv_1/fail")

    assert immutablelog_server.requests[0]["body"]["meta"]["type"] == "success"
    assert immutablelog_server.requests[1]["body"]["meta"]["type"] == "error"


@pytest.mark.asyncio
async def test_immutable_trail_header_is_propagated(tmp_path, immutablelog_server):
    app = build_app(
        file_path=str(tmp_path / "audit.jsonl"),
        mode="remote",
        immutablelog_url=immutablelog_server.url,
        immutablelog_api_key="iml_live_ok",
    )

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        await client.delete("/invoices/inv_1", headers={"X-Audit-Trail": "order-2026-00441"})

    assert immutablelog_server.requests[0]["body"]["meta"]["immutable_trail"] == "order-2026-00441"
