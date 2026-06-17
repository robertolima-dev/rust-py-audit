"""
Configuração compartilhada de testes.

O Django é configurado aqui uma única vez, antes da coleta dos testes
(hook `pytest_configure`), para que `tests/test_django_middleware.py`
possa importar `rust_py_audit.django` sem precisar de um projeto Django
completo no disco.
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import django
import pytest
from django.conf import settings


def pytest_configure():
    if not settings.configured:
        settings.configure(
            DATABASES={"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": ":memory:"}},
            INSTALLED_APPS=[],
            USE_TZ=True,
            SECRET_KEY="test-secret-key-rust-py-audit",
        )
        django.setup()


class _QuietThreadingHTTPServer(ThreadingHTTPServer):
    def handle_error(self, request, client_address):
        # No teste de timeout, o cliente (reqwest) desiste e fecha a
        # conexão antes do servidor terminar de "dormir" e escrever a
        # resposta — gera BrokenPipeError/ConnectionResetError aqui, que
        # é o comportamento ESPERADO do teste, não um bug do servidor
        # de mentirinha. Qualquer outra exceção continua sendo impressa
        # normalmente (comportamento padrão do `socketserver`).
        exc_type = sys.exc_info()[0]
        if exc_type not in (BrokenPipeError, ConnectionResetError):
            super().handle_error(request, client_address)


class FakeImmutableLogServer:
    """
    Servidor HTTP local de mentirinha, simulando `POST /v1/events` do
    ImmutableLog.

    Necessário porque o envio HTTP real acontece dentro da extensão
    Rust (via `reqwest`) — bibliotecas de mock no nível do Python
    (`responses`, `httpx_mock`) não interceptam essas chamadas, já que
    elas não passam pela camada de transporte HTTP do Python.
    """

    def __init__(self) -> None:
        self.requests: list[dict] = []
        self._lock = threading.Lock()
        self._queue: list[tuple[int, dict]] = []
        self._default_status = 202
        self._default_body = {
            "ok": True,
            "tx_id": "tx_default",
            "payload_hash": "hash_default",
            "status": "accepted",
            "duplicate": False,
            "request_id": "req_default",
        }
        self._delay_seconds = 0.0

        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # noqa: D102
                pass

            def do_POST(self):  # noqa: N802
                length = int(self.headers.get("Content-Length", 0))
                raw_body = self.rfile.read(length)

                with outer._lock:
                    outer.requests.append(
                        {
                            "path": self.path,
                            "headers": {k.lower(): v for k, v in self.headers.items()},
                            "body": json.loads(raw_body) if raw_body else None,
                        }
                    )
                    delay = outer._delay_seconds
                    if outer._queue:
                        status, body = outer._queue.pop(0)
                    else:
                        status, body = outer._default_status, outer._default_body

                if delay:
                    time.sleep(delay)

                payload = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(payload)

        # `ThreadingHTTPServer` (não `HTTPServer`): precisa atender mais
        # de uma conexão "em voo" ao mesmo tempo — ex.: o teste de
        # timeout/retry, onde a 2ª tentativa (retry) é disparada antes
        # da 1ª (que está "dormindo" simulando lentidão) terminar de
        # responder.
        self._server = _QuietThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self._server.server_port}"

    @property
    def request_count(self) -> int:
        with self._lock:
            return len(self.requests)

    def queue_response(self, status: int, body: dict) -> None:
        with self._lock:
            self._queue.append((status, body))

    def set_default_response(self, status: int, body: dict) -> None:
        with self._lock:
            self._default_status = status
            self._default_body = body

    def set_delay(self, seconds: float) -> None:
        self._delay_seconds = seconds

    def shutdown(self) -> None:
        self._server.shutdown()
        self._server.server_close()


@pytest.fixture
def immutablelog_server():
    server = FakeImmutableLogServer()
    yield server
    server.shutdown()
