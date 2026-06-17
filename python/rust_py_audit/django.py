"""
Middleware Django para rust_py_audit.

Uso em settings.py:

    MIDDLEWARE = [
        "rust_py_audit.django.AuditMiddleware",
        # ... outros middlewares ...
    ]

Configuração opcional, também em settings.py (o middleware Django não
recebe argumentos de construtor — só `get_response` —, então a
configuração precisa vir das settings em vez de argumentos diretos,
diferente do middleware FastAPI):

    RUST_PY_AUDIT_APP_NAME = "my-django-app"
    RUST_PY_AUDIT_FILE_PATH = "./audit.jsonl"
    RUST_PY_AUDIT_METHODS = {"POST", "PUT", "PATCH", "DELETE"}

Por padrão, `actor_id` vem de `request.user.pk` se o usuário estiver
autenticado (via `django.contrib.auth`), e cai para `"anonymous"` caso
contrário.

Este módulo só é importável se `django` estiver instalado
(`pip install "rust-py-audit[django]"`) — não é dependência obrigatória
do pacote base.
"""
import asyncio
from typing import Callable

from django.conf import settings

from rust_py_audit import AuditLogger

DEFAULT_AUDITED_METHODS = frozenset({"POST", "PUT", "PATCH", "DELETE"})


class AuditMiddleware:
    """
    Registra um evento de auditoria para cada requisição Django cujo
    método esteja entre os configurados em `RUST_PY_AUDIT_METHODS`.

    Suporta tanto aplicações síncronas (WSGI) quanto assíncronas (ASGI):
    - WSGI: o Django chama `__call__` diretamente.
    - ASGI: o Django detecta `_is_coroutine` e chama `__acall__`.
    Esse é o mesmo padrão usado por qualquer middleware Django "híbrido"
    — não é específico desta lib.
    """

    async_capable = True
    sync_capable = True

    def __init__(self, get_response: Callable) -> None:
        self.get_response = get_response
        self._audit = AuditLogger(
            app_name=getattr(settings, "RUST_PY_AUDIT_APP_NAME", "django-app"),
            file_path=getattr(settings, "RUST_PY_AUDIT_FILE_PATH", "./audit.jsonl"),
        )
        self._audited_methods = getattr(settings, "RUST_PY_AUDIT_METHODS", DEFAULT_AUDITED_METHODS)

        # Se o handler seguinte for uma coroutine (modo ASGI), o Django
        # precisa saber que este middleware também é assíncrono. Definir
        # `_is_coroutine` é o sinal oficial do Django para isso.
        if asyncio.iscoroutinefunction(self.get_response):
            self._is_coroutine = asyncio.coroutines._is_coroutine  # type: ignore[attr-defined]

    @staticmethod
    def _actor_id(request) -> str:
        user = getattr(request, "user", None)
        if user is not None and getattr(user, "is_authenticated", False):
            return str(user.pk)
        return "anonymous"

    def _record(self, request, response) -> None:
        if request.method not in self._audited_methods:
            return

        self._audit.log(
            actor_id=self._actor_id(request),
            action=request.method,
            resource=request.path,
            resource_id="-",
            metadata={"status_code": response.status_code},
        )

    def __call__(self, request):  # type: ignore[no-untyped-def]
        """Handler síncrono — usado em aplicações WSGI."""
        response = self.get_response(request)
        self._record(request, response)
        return response

    async def __acall__(self, request):  # type: ignore[no-untyped-def]
        """Handler assíncrono — usado em aplicações Django ASGI."""
        response = await self.get_response(request)
        self._record(request, response)
        return response
