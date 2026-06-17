import json

from django.http import HttpResponse
from django.test import RequestFactory, override_settings

from rust_py_audit.django import AuditMiddleware

factory = RequestFactory()


def view_ok(request):
    return HttpResponse("ok", status=200)


def view_conflict(request):
    return HttpResponse("conflict", status=409)


def test_hybrid_mode_sends_to_immutablelog_and_keeps_local_file(tmp_path, immutablelog_server):
    with override_settings(
        RUST_PY_AUDIT_APP_NAME="billing-django",
        RUST_PY_AUDIT_FILE_PATH=str(tmp_path / "audit.jsonl"),
        RUST_PY_AUDIT_MODE="hybrid",
        RUST_PY_AUDIT_IMMUTABLELOG_URL=immutablelog_server.url,
        RUST_PY_AUDIT_IMMUTABLELOG_API_KEY="iml_live_ok",
    ):
        request = factory.delete("/invoices/inv_1")
        middleware = AuditMiddleware(view_ok)
        response = middleware(request)

    assert response.status_code == 200
    event = json.loads((tmp_path / "audit.jsonl").read_text().strip())
    assert event["immutablelog"]["status"] == "delivered"
    assert immutablelog_server.request_count == 1


def test_remote_failure_does_not_break_the_actual_response(tmp_path, immutablelog_server):
    immutablelog_server.set_default_response(401, {"ok": False, "reason": "invalid_api_key"})

    with override_settings(
        RUST_PY_AUDIT_APP_NAME="billing-django",
        RUST_PY_AUDIT_FILE_PATH=str(tmp_path / "audit.jsonl"),
        RUST_PY_AUDIT_MODE="remote",
        RUST_PY_AUDIT_IMMUTABLELOG_URL=immutablelog_server.url,
        RUST_PY_AUDIT_IMMUTABLELOG_API_KEY="iml_live_bad",
    ):
        request = factory.delete("/invoices/inv_1")
        middleware = AuditMiddleware(view_ok)
        response = middleware(request)

    assert response.status_code == 200
    assert response.content == b"ok"


def test_severity_is_derived_from_response_status_code(tmp_path, immutablelog_server):
    with override_settings(
        RUST_PY_AUDIT_APP_NAME="billing-django",
        RUST_PY_AUDIT_FILE_PATH=str(tmp_path / "audit.jsonl"),
        RUST_PY_AUDIT_MODE="remote",
        RUST_PY_AUDIT_IMMUTABLELOG_URL=immutablelog_server.url,
        RUST_PY_AUDIT_IMMUTABLELOG_API_KEY="iml_live_ok",
    ):
        AuditMiddleware(view_ok)(factory.delete("/invoices/inv_1"))
        AuditMiddleware(view_conflict)(factory.post("/invoices/inv_1"))

    assert immutablelog_server.requests[0]["body"]["meta"]["type"] == "success"
    assert immutablelog_server.requests[1]["body"]["meta"]["type"] == "error"


def test_immutable_trail_header_is_propagated(tmp_path, immutablelog_server):
    with override_settings(
        RUST_PY_AUDIT_APP_NAME="billing-django",
        RUST_PY_AUDIT_FILE_PATH=str(tmp_path / "audit.jsonl"),
        RUST_PY_AUDIT_MODE="remote",
        RUST_PY_AUDIT_IMMUTABLELOG_URL=immutablelog_server.url,
        RUST_PY_AUDIT_IMMUTABLELOG_API_KEY="iml_live_ok",
    ):
        request = factory.delete("/invoices/inv_1", HTTP_X_AUDIT_TRAIL="order-2026-00441")
        AuditMiddleware(view_ok)(request)

    assert immutablelog_server.requests[0]["body"]["meta"]["immutable_trail"] == "order-2026-00441"
