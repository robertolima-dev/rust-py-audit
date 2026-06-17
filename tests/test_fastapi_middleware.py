"""
Testes de integração: FastAPI + AuditMiddleware.

`httpx.AsyncClient` com `ASGITransport` dispara requisições reais dentro
do próprio processo, sem precisar de um servidor HTTP rodando — o
middleware é ativado exatamente como em produção.
"""
import json

import pytest
from fastapi import FastAPI
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient

from rust_py_audit.fastapi import AuditMiddleware


def build_app(file_path: str) -> FastAPI:
    app = FastAPI()
    app.add_middleware(AuditMiddleware, app_name="billing-api", file_path=file_path)

    @app.get("/invoices/{invoice_id}")
    async def get_invoice(invoice_id: str):
        return {"id": invoice_id}

    @app.delete("/invoices/{invoice_id}")
    async def delete_invoice(invoice_id: str):
        return JSONResponse({"deleted": invoice_id}, status_code=200)

    return app


@pytest.mark.asyncio
async def test_delete_request_is_logged(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    app = build_app(str(file_path))

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.delete("/invoices/inv_987", headers={"X-User-Id": "user_42"})

    assert response.status_code == 200
    lines = file_path.read_text().strip().splitlines()
    assert len(lines) == 1
    event = json.loads(lines[0])
    assert event["app_name"] == "billing-api"
    assert event["actor_id"] == "user_42"
    assert event["action"] == "DELETE"
    assert event["resource"] == "/invoices/inv_987"
    assert event["metadata"]["status_code"] == 200


@pytest.mark.asyncio
async def test_get_request_is_not_logged(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    app = build_app(str(file_path))

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get("/invoices/inv_987")

    assert response.status_code == 200
    assert not file_path.exists()


@pytest.mark.asyncio
async def test_actor_id_falls_back_to_anonymous_without_header(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    app = build_app(str(file_path))

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        await client.delete("/invoices/inv_987")

    event = json.loads(file_path.read_text().strip())
    assert event["actor_id"] == "anonymous"


@pytest.mark.asyncio
async def test_chains_hash_across_multiple_audited_requests(tmp_path):
    file_path = tmp_path / "audit.jsonl"
    app = build_app(str(file_path))

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        await client.delete("/invoices/inv_1")
        await client.delete("/invoices/inv_2")

    lines = file_path.read_text().strip().splitlines()
    first_event = json.loads(lines[0])
    second_event = json.loads(lines[1])
    assert second_event["previous_hash"] == first_event["hash"]
