"""
Middleware FastAPI/Starlette para rust_py_audit.

Uso:
    from fastapi import FastAPI
    from rust_py_audit.fastapi import AuditMiddleware

    app = FastAPI()
    app.add_middleware(AuditMiddleware, app_name="billing-api")

Por padrão, só registra requisições que costumam alterar estado
(POST/PUT/PATCH/DELETE) — GET/HEAD/OPTIONS não passam por `AuditLogger.log()`,
para não inflar o arquivo de auditoria com leituras. Ajuste via
`audited_methods=` se quiser outro comportamento.

Este módulo só é importável se `fastapi`/`starlette` estiverem instalados
(`pip install "rust-py-audit[fastapi]"`) — eles não são dependências
obrigatórias do pacote base.
"""
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response
from starlette.types import ASGIApp

from rust_py_audit import AuditLogger

DEFAULT_AUDITED_METHODS = frozenset({"POST", "PUT", "PATCH", "DELETE"})
DEFAULT_ACTOR_HEADER = "X-User-Id"


class AuditMiddleware(BaseHTTPMiddleware):
    """
    Registra um evento de auditoria para cada requisição HTTP cujo
    método esteja em `audited_methods`.

    `actor_id` é lido do header HTTP definido em `actor_header` (padrão
    `X-User-Id`); cai para `"anonymous"` se o header não vier na
    requisição. Ajuste esse header — ou substitua `_actor_id()` numa
    subclasse — para integrar com o seu esquema real de autenticação
    (JWT, sessão, etc).

    Nota de performance: `AuditLogger.log()` faz I/O síncrono de arquivo.
    Aqui ele roda dentro do `dispatch` assíncrono, então cada chamada
    bloqueia o event loop por um instante (uma escrita pequena em
    arquivo local, tipicamente sub-milissegundo). Para APIs de altíssimo
    throughput, considere mover a chamada para uma threadpool
    (`starlette.concurrency.run_in_threadpool`) — fora do escopo deste
    MVP, mas é a próxima otimização natural.
    """

    def __init__(
        self,
        app: ASGIApp,
        app_name: str = "fastapi-app",
        file_path: str = "./audit.jsonl",
        audited_methods: frozenset[str] | None = None,
        actor_header: str = DEFAULT_ACTOR_HEADER,
        **kwargs: object,
    ) -> None:
        super().__init__(app, **kwargs)
        self._audit = AuditLogger(app_name=app_name, file_path=file_path)
        self._audited_methods = audited_methods or DEFAULT_AUDITED_METHODS
        self._actor_header = actor_header

    def _actor_id(self, request: Request) -> str:
        return request.headers.get(self._actor_header, "anonymous")

    async def dispatch(self, request: Request, call_next) -> Response:  # type: ignore[override]
        response = await call_next(request)

        if request.method in self._audited_methods:
            self._audit.log(
                actor_id=self._actor_id(request),
                action=request.method,
                resource=request.url.path,
                resource_id="-",
                metadata={
                    "status_code": response.status_code,
                    "client_ip": request.client.host if request.client else None,
                },
            )

        return response
