"""
Middleware FastAPI/Starlette para rust_py_audit.

Uso:
    from fastapi import FastAPI
    from rust_py_audit.fastapi import AuditMiddleware

    app = FastAPI()
    app.add_middleware(AuditMiddleware, app_name="billing-api")

    # Com ImmutableLog (envia eventos POST/PUT/PATCH/DELETE também
    # para o ImmutableLog, além de — ou em vez de — gravar local):
    app.add_middleware(
        AuditMiddleware,
        app_name="billing-api",
        mode="hybrid",
        immutablelog_url="https://api.immutablelog.com",
        immutablelog_api_key="iml_live_xxxxx",
    )

Por padrão, só registra requisições que costumam alterar estado
(POST/PUT/PATCH/DELETE) — GET/HEAD/OPTIONS não passam por `AuditLogger.log()`,
para não inflar o arquivo de auditoria com leituras. Ajuste via
`audited_methods=` se quiser outro comportamento.

`mode`/`immutablelog_url`/`immutablelog_api_key`/`timeout_ms`/
`retry_enabled`/`max_retries`/`immutablelog_env` são só repassados para
`AuditLogger(...)` — veja lá os defaults e o fallback por variável de
ambiente (`RUST_PY_AUDIT_MODE`, `IMMUTABLELOG_URL`, `IMMUTABLELOG_API_KEY`,
`IMMUTABLELOG_ENV`).

Em `mode="remote"`/`"hybrid"`, cada evento carrega automaticamente:
- `severity` (`meta.type` no ImmutableLog), calculada a partir do
  `status_code` da resposta: >=400 -> `"error"`, 300-399 -> `"info"`,
  200-299 -> `"success"`.
- `immutable_trail` (`meta.immutable_trail`), lido do header definido em
  `trail_header` (padrão `X-Audit-Trail`) — ausente se o cliente não
  enviar esse header.

Este módulo só é importável se `fastapi`/`starlette` estiverem instalados
(`pip install "rust-py-audit[fastapi]"`) — eles não são dependências
obrigatórias do pacote base.
"""
import logging

from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response
from starlette.types import ASGIApp

from rust_py_audit import AuditLogger

logger = logging.getLogger(__name__)

DEFAULT_AUDITED_METHODS = frozenset({"POST", "PUT", "PATCH", "DELETE"})
DEFAULT_ACTOR_HEADER = "X-User-Id"
DEFAULT_TRAIL_HEADER = "X-Audit-Trail"


def _severity_from_status(status_code: int) -> str:
    """Classifica a resposta em error/info/success para `meta.type` no
    ImmutableLog. Mesmo mapeamento usado em integrações de referência:
    >=400 é tratado como erro (sem distinguir warning), 300-399 como
    info, 200-299 como success."""
    if status_code >= 400:
        return "error"
    if status_code >= 300:
        return "info"
    if status_code >= 200:
        return "success"
    return "info"


class AuditMiddleware(BaseHTTPMiddleware):
    """
    Registra um evento de auditoria para cada requisição HTTP cujo
    método esteja em `audited_methods`.

    `actor_id` é lido do header HTTP definido em `actor_header` (padrão
    `X-User-Id`); cai para `"anonymous"` se o header não vier na
    requisição. Ajuste esse header — ou substitua `_actor_id()` numa
    subclasse — para integrar com o seu esquema real de autenticação
    (JWT, sessão, etc).

    Nota de performance: `AuditLogger.log()` faz I/O síncrono — de
    arquivo local (sub-milissegundo) e, em `mode="remote"`/`"hybrid"`,
    também uma requisição HTTP síncrona ao ImmutableLog (até
    `timeout_ms`, mais retries). Aqui ele roda dentro do `dispatch`
    assíncrono, então cada chamada bloqueia esse event loop pelo tempo
    da operação — o lado Rust libera o GIL durante a chamada HTTP (não
    trava OUTRAS threads Python), mas não tira o trabalho do event loop
    atual. Para APIs de altíssimo throughput, considere mover a chamada
    para uma threadpool (`starlette.concurrency.run_in_threadpool`) ou,
    numa versão futura, uma fila assíncrona — fora do escopo deste MVP.
    """

    def __init__(
        self,
        app: ASGIApp,
        app_name: str = "fastapi-app",
        file_path: str = "./audit.jsonl",
        audited_methods: frozenset[str] | None = None,
        actor_header: str = DEFAULT_ACTOR_HEADER,
        mode: str | None = None,
        immutablelog_url: str | None = None,
        immutablelog_api_key: str | None = None,
        timeout_ms: int = 500,
        retry_enabled: bool = True,
        max_retries: int = 3,
        immutablelog_env: str | None = None,
        trail_header: str = DEFAULT_TRAIL_HEADER,
        **kwargs: object,
    ) -> None:
        super().__init__(app, **kwargs)
        self._audit = AuditLogger(
            app_name=app_name,
            file_path=file_path,
            mode=mode,
            immutablelog_url=immutablelog_url,
            immutablelog_api_key=immutablelog_api_key,
            timeout_ms=timeout_ms,
            retry_enabled=retry_enabled,
            max_retries=max_retries,
            immutablelog_env=immutablelog_env,
        )
        self._audited_methods = audited_methods or DEFAULT_AUDITED_METHODS
        self._actor_header = actor_header
        self._trail_header = trail_header

    def _actor_id(self, request: Request) -> str:
        return request.headers.get(self._actor_header, "anonymous")

    def _immutable_trail(self, request: Request) -> str | None:
        return request.headers.get(self._trail_header)

    async def dispatch(self, request: Request, call_next) -> Response:  # type: ignore[override]
        response = await call_next(request)

        if request.method in self._audited_methods:
            try:
                self._audit.log(
                    actor_id=self._actor_id(request),
                    action=request.method,
                    resource=request.url.path,
                    resource_id="-",
                    metadata={
                        "status_code": response.status_code,
                        "client_ip": request.client.host if request.client else None,
                    },
                    severity=_severity_from_status(response.status_code),
                    immutable_trail=self._immutable_trail(request),
                )
            except RuntimeError:
                # Em `mode="remote"`, uma falha ao entregar ao
                # ImmutableLog levanta `RuntimeError` (ver
                # `AuditLogger.log()`). Uma falha de AUDITORIA não deve
                # virar um 500 na resposta real já computada por
                # `call_next` — registramos e seguimos. Em `mode="hybrid"`
                # isso não acontece: a falha de entrega já vira
                # `delivery_status="pending"` em vez de exceção.
                logger.warning("falha ao enviar evento de auditoria ao ImmutableLog", exc_info=True)

        return response
