"""
Settings Django mínimas para o exemplo do rust-py-audit.
"""
SECRET_KEY = "django-example-key-not-for-production"
DEBUG = True
ALLOWED_HOSTS = ["*"]

INSTALLED_APPS = [
    "django.contrib.contenttypes",
    "django.contrib.auth",
]

# rust-py-audit: configuração lida pelo AuditMiddleware (veja
# rust_py_audit/django.py) — não há como passar essas opções por
# argumento de construtor, já que o Django só injeta `get_response`.
RUST_PY_AUDIT_APP_NAME = "billing-django"
RUST_PY_AUDIT_FILE_PATH = "./audit.jsonl"

MIDDLEWARE = [
    "rust_py_audit.django.AuditMiddleware",
    "django.middleware.common.CommonMiddleware",
]

ROOT_URLCONF = "examples.django_example.urls"

DATABASES = {
    "default": {
        "ENGINE": "django.db.backends.sqlite3",
        "NAME": ":memory:",
    }
}

USE_TZ = True
DEFAULT_AUTO_FIELD = "django.db.models.BigAutoField"
