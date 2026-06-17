"""
URLs do exemplo Django.

Rode:
    pip install "rust-py-audit[django]"
    DJANGO_SETTINGS_MODULE=examples.django_example.settings python -m django runserver

Depois:
    curl -X DELETE http://localhost:8000/invoices/inv_987/
    cat audit.jsonl
"""
from django.urls import path

from . import views

urlpatterns = [
    path("", views.index),
    path("invoices/<str:invoice_id>/", views.invoice_detail),
]
