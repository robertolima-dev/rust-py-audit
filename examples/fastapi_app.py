"""
Exemplo de app FastAPI com rust-py-audit.

Rode:
    pip install "rust-py-audit[fastapi]" uvicorn
    uvicorn examples.fastapi_app:app --reload

Depois:
    curl http://localhost:8000/invoices/inv_987 -X DELETE
    cat audit.jsonl
"""
from fastapi import FastAPI
from fastapi.responses import JSONResponse

from rust_py_audit.fastapi import AuditMiddleware

app = FastAPI(title="rust-py-audit FastAPI example")

# A ordem importa: adicionando primeiro, o AuditMiddleware fica "por
# fora" da pilha, vendo o status_code final de qualquer middleware
# adicionado depois.
app.add_middleware(AuditMiddleware, app_name="billing-api", file_path="./audit.jsonl")
# Com ImmutableLog (ver examples/immutablelog_basic.py e o README):
# app.add_middleware(
#     AuditMiddleware,
#     app_name="billing-api",
#     file_path="./audit.jsonl",
#     mode="hybrid",
#     immutablelog_url="https://api.immutablelog.com",
#     immutablelog_api_key="iml_live_xxxxx",
# )


@app.get("/")
async def root():
    return {"status": "ok"}


@app.get("/invoices/{invoice_id}")
async def get_invoice(invoice_id: str):
    return {"id": invoice_id, "amount": 199.90}


@app.delete("/invoices/{invoice_id}")
async def delete_invoice(invoice_id: str):
    # Em uma aplicação real, aqui entraria a lógica de negócio (apagar
    # do banco, etc). O AuditMiddleware já registra o DELETE sozinho —
    # sem nenhuma chamada explícita a `audit.log()` dentro da rota.
    return JSONResponse({"deleted": invoice_id}, status_code=200)


@app.post("/invoices/{invoice_id}/restore")
async def restore_invoice(invoice_id: str):
    return {"restored": invoice_id}
