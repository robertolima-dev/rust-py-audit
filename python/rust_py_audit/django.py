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

    # Integração com o ImmutableLog (opcional — repassados direto para
    # `AuditLogger(...)`; se omitidos, caem no fallback de variável de
    # ambiente RUST_PY_AUDIT_MODE / IMMUTABLELOG_URL / IMMUTABLELOG_API_KEY):
    RUST_PY_AUDIT_MODE = "hybrid"
    RUST_PY_AUDIT_IMMUTABLELOG_URL = "https://api.immutablelog.com"
    RUST_PY_AUDIT_IMMUTABLELOG_API_KEY = "iml_live_xxxxx"
    RUST_PY_AUDIT_TIMEOUT_MS = 500
    RUST_PY_AUDIT_RETRY_ENABLED = True
    RUST_PY_AUDIT_MAX_RETRIES = 3
    RUST_PY_AUDIT_IMMUTABLELOG_ENV = "production"
    RUST_PY_AUDIT_TRAIL_HEADER = "X-Audit-Trail"

Por padrão, `actor_id` vem de `request.user.pk` se o usuário estiver
autenticado (via `django.contrib.auth`), e cai para `"anonymous"` caso
contrário.

Em `mode="remote"`/`"hybrid"`, cada evento carrega automaticamente:
- `severity` (`meta.type` no ImmutableLog), calculada a partir do
  `status_code` da resposta: >=400 -> `"error"`, 300-399 -> `"info"`,
  200-299 -> `"success"`.
- `immutable_trail` (`meta.immutable_trail`), lido do header definido em
  `RUST_PY_AUDIT_TRAIL_HEADER` (padrão `X-Audit-Trail`) — ausente se o
  cliente não enviar esse header.

Este módulo só é importável se `django` estiver instalado
(`pip install "rust-py-audit[django]"`) — não é dependência obrigatória
do pacote base.
"""
import asyncio
import logging
from typing import Callable

from django.conf import settings

from rust_py_audit import AuditLogger

logger = logging.getLogger(__name__)

DEFAULT_AUDITED_METHODS = frozenset({"POST", "PUT", "PATCH", "DELETE"})
DEFAULT_TRAIL_HEADER = "X-Audit-Trail"


def _severity_from_status(status_code: int) -> str:
    """Mesmo mapeamento do middleware FastAPI — ver `rust_py_audit.fastapi`."""
    if status_code >= 400:
        return "error"
    if status_code >= 300:
        return "info"
    if status_code >= 200:
        return "success"
    return "info"


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
            mode=getattr(settings, "RUST_PY_AUDIT_MODE", None),
            immutablelog_url=getattr(settings, "RUST_PY_AUDIT_IMMUTABLELOG_URL", None),
            immutablelog_api_key=getattr(settings, "RUST_PY_AUDIT_IMMUTABLELOG_API_KEY", None),
            timeout_ms=getattr(settings, "RUST_PY_AUDIT_TIMEOUT_MS", 500),
            retry_enabled=getattr(settings, "RUST_PY_AUDIT_RETRY_ENABLED", True),
            max_retries=getattr(settings, "RUST_PY_AUDIT_MAX_RETRIES", 3),
            immutablelog_env=getattr(settings, "RUST_PY_AUDIT_IMMUTABLELOG_ENV", None),
        )
        self._audited_methods = getattr(settings, "RUST_PY_AUDIT_METHODS", DEFAULT_AUDITED_METHODS)
        self._trail_header = getattr(settings, "RUST_PY_AUDIT_TRAIL_HEADER", DEFAULT_TRAIL_HEADER)

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

        try:
            self._audit.log(
                actor_id=self._actor_id(request),
                action=request.method,
                resource=request.path,
                resource_id="-",
                metadata={"status_code": response.status_code},
                severity=_severity_from_status(response.status_code),
                immutable_trail=request.headers.get(self._trail_header),
            )
        except RuntimeError:
            # Em `mode="remote"`, falha ao entregar ao ImmutableLog
            # levanta `RuntimeError` — não deixamos isso virar um 500
            # na resposta já computada por `get_response`. Em
            # `mode="hybrid"` isso não acontece: a falha já vira
            # `delivery_status="pending"` em vez de exceção.
            logger.warning("falha ao enviar evento de auditoria ao ImmutableLog", exc_info=True)

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
