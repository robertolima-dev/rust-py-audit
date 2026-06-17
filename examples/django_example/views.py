"""
Views de exemplo para o app Django.
"""
from django.http import JsonResponse


def index(request):
    return JsonResponse({"status": "ok"})


def invoice_detail(request, invoice_id):
    if request.method == "DELETE":
        # O AuditMiddleware já registra este DELETE sozinho — nenhuma
        # chamada explícita a `audit.log()` é necessária aqui.
        return JsonResponse({"deleted": invoice_id})

    return JsonResponse({"id": invoice_id, "amount": 199.90})
