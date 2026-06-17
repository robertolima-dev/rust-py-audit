"""
Testes de integração: Django + AuditMiddleware.

`RequestFactory` cria requests reais sem precisar de um servidor HTTP.
`override_settings` isola cada teste num arquivo `audit.jsonl` próprio
(via `tmp_path`), já que `RUST_PY_AUDIT_FILE_PATH` é lido das settings
globais do Django na hora em que o middleware é instanciado.
"""
import json

from django.http import HttpResponse, JsonResponse
from django.test import RequestFactory, override_settings

from rust_py_audit.django import AuditMiddleware

factory = RequestFactory()


def view_ok(request):
    return HttpResponse("ok", status=200)


def view_created(request):
    return JsonResponse({"id": 1}, status=201)


def _audit_settings(tmp_path, **extra):
    return override_settings(
        RUST_PY_AUDIT_APP_NAME="billing-django",
        RUST_PY_AUDIT_FILE_PATH=str(tmp_path / "audit.jsonl"),
        **extra,
    )


def test_post_request_is_logged(tmp_path):
    with _audit_settings(tmp_path):
        request = factory.post("/users", data={}, content_type="application/json")
        middleware = AuditMiddleware(view_created)
        response = middleware(request)

    assert response.status_code == 201
    lines = (tmp_path / "audit.jsonl").read_text().strip().splitlines()
    assert len(lines) == 1
    event = json.loads(lines[0])
    assert event["app_name"] == "billing-django"
    assert event["action"] == "POST"
    assert event["resource"] == "/users"
    assert event["metadata"]["status_code"] == 201


def test_get_request_is_not_logged(tmp_path):
    with _audit_settings(tmp_path):
        request = factory.get("/ping")
        middleware = AuditMiddleware(view_ok)
        middleware(request)

    assert not (tmp_path / "audit.jsonl").exists()


def test_custom_audited_methods_can_include_get(tmp_path):
    with _audit_settings(tmp_path, RUST_PY_AUDIT_METHODS={"GET"}):
        request = factory.get("/ping")
        middleware = AuditMiddleware(view_ok)
        middleware(request)

    lines = (tmp_path / "audit.jsonl").read_text().strip().splitlines()
    assert len(lines) == 1
    assert json.loads(lines[0])["action"] == "GET"


def test_actor_id_falls_back_to_anonymous_without_authenticated_user(tmp_path):
    with _audit_settings(tmp_path):
        request = factory.delete("/invoices/inv_1")
        middleware = AuditMiddleware(view_ok)
        middleware(request)

    event = json.loads((tmp_path / "audit.jsonl").read_text().strip())
    assert event["actor_id"] == "anonymous"


def test_middleware_returns_response_unchanged(tmp_path):
    with _audit_settings(tmp_path):
        request = factory.post("/users", data={}, content_type="application/json")
        middleware = AuditMiddleware(view_created)
        response = middleware(request)

    assert response.status_code == 201
    assert json.loads(response.content) == {"id": 1}
