"""
Configuração compartilhada de testes.

O Django é configurado aqui uma única vez, antes da coleta dos testes
(hook `pytest_configure`), para que `tests/test_django_middleware.py`
possa importar `rust_py_audit.django` sem precisar de um projeto Django
completo no disco.
"""
import django
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
