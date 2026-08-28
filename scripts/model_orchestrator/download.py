"""Provider-neutral exact acquisition resolution, planning, and execution."""

from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timezone
from typing import Iterable

from .events import EventStream
from .library import NeutralLibrary, redact_origin
from .models import (
    AcquisitionOffer,
    AcquisitionPlan,
    AcquisitionRequest,
    AcquisitionResult,
)
from .providers import AcquisitionProvider, builtin_providers
from .providers.direct import ProviderError


def resolve_request(
    request: AcquisitionRequest, providers: Iterable[AcquisitionProvider] | None = None
) -> list[AcquisitionOffer]:
    selected = tuple(builtin_providers() if providers is None else providers)
    offers: list[AcquisitionOffer] = []
    for locator in (request.primary_locator, *request.alternatives):
        provider = next((item for item in selected if item.accepts(locator)), None)
        if provider is None:
            raise ProviderError("provider-unknown", "Aucun fournisseur n'accepte ce locator exact.")
        offers.append(provider.resolve(request, locator))
    return offers


def comparable_groups(offers: Iterable[AcquisitionOffer]) -> list[list[AcquisitionOffer]]:
    groups: list[list[AcquisitionOffer]] = []
    exact: dict[tuple[str, str, str, str], list[AcquisitionOffer]] = {}
    for offer in offers:
        key = offer.exact_group_key
        if key is None:
            groups.append([offer])
        else:
            exact.setdefault(key, []).append(offer)
    groups.extend(exact[key] for key in sorted(exact))
    return groups


def create_plan(operation_id: str, offer: AcquisitionOffer) -> AcquisitionPlan:
    if not operation_id:
        raise ValueError("operation_id est requis")
    return AcquisitionPlan(
        operation_id=operation_id,
        offer=offer,
        created_at=datetime.now(timezone.utc).isoformat(),
    )


def execute_plan(
    plan: AcquisitionPlan,
    *,
    library: NeutralLibrary,
    write_event,
    providers: Iterable[AcquisitionProvider] | None = None,
) -> AcquisitionResult:
    events = EventStream(plan.operation_id, write_event)
    selected = tuple(builtin_providers() if providers is None else providers)
    provider = next((item for item in selected if item.name == plan.offer.provider), None)
    if provider is None:
        error = ProviderError("provider-unavailable", "Le fournisseur planifié est indisponible.")
        events.failed(error.message)
        return AcquisitionResult(plan.operation_id, plan.offer.provider, None, error.code, error.message)
    if plan.offer.conversion_required or not plan.offer.executable:
        error = ProviderError(
            "offer-not-executable",
            "Cette offre reste visible mais n'est pas exécutable sans conversion ou configuration.",
        )
        events.failed(error.message)
        return AcquisitionResult(plan.operation_id, provider.name, None, error.code, error.message)
    try:
        record = provider.download(
            plan.offer,
            operation_id=plan.operation_id,
            library=library,
            events=events,
        )
        events.completed(record.artifact_id)
        return AcquisitionResult(plan.operation_id, provider.name, record)
    except ProviderError as exc:
        if exc.code == "download-cancelled":
            events.cancelled(exc.message)
        else:
            events.failed(exc.message)
        return AcquisitionResult(plan.operation_id, provider.name, None, exc.code, exc.message)
    except Exception:
        message = "Le téléchargement a échoué sans exposer de détail sensible."
        events.failed(message)
        return AcquisitionResult(plan.operation_id, provider.name, None, "download-failed", message)


def public_offer(offer: AcquisitionOffer) -> AcquisitionOffer:
    """Return a presentation-safe copy without signed URL queries or fragments."""

    locator = redact_origin(offer.locator) if offer.locator.startswith(("http://", "https://")) else offer.locator
    return replace(offer, locator=locator)
