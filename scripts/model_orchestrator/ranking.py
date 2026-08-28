"""Deterministic provider ranking from bounded local evidence."""

from __future__ import annotations

import statistics
from typing import Iterable, Mapping

from .models import AcquisitionOffer, PerformanceObservation, RankedOffer

DEFAULT_COLD_ORDER = ("ollama", "huggingface", "lm-studio", "direct")


def rank_offers(
    offers: Iterable[AcquisitionOffer],
    observations: Iterable[PerformanceObservation],
    *,
    cold_order: tuple[str, ...] = DEFAULT_COLD_ORDER,
    manual_provider: str | None = None,
) -> list[RankedOffer]:
    history = list(observations)
    ranked = [_rank(offer, history) for offer in offers]
    order = {provider: index for index, provider in enumerate(cold_order)}

    def key(row: RankedOffer):
        manual = 0 if manual_provider and row.offer.provider == manual_provider else 1
        executable = 0 if row.offer.executable and not row.offer.conversion_required else 1
        cached = 0 if row.offer.cache_verified and _remaining_network(row.offer) == 0 else 1
        known = 0 if row.adjusted_seconds is not None else 1
        predicted = row.adjusted_seconds if row.adjusted_seconds is not None else float("inf")
        cold = order.get(row.offer.provider, len(order))
        return (manual, executable, cached, known, predicted, cold, row.offer.provider, row.offer.locator)

    return sorted(ranked, key=key)


def _rank(
    offer: AcquisitionOffer, history: list[PerformanceObservation]
) -> RankedOffer:
    rows = [row for row in history if row.provider == offer.provider and row.kind == offer.format]
    successes = [row for row in rows if row.success]
    sample_count = len(successes)
    observed_range = (
        (min(row.elapsed_seconds for row in successes), max(row.elapsed_seconds for row in successes))
        if successes
        else None
    )
    reasons: list[str] = []
    if offer.conversion_required:
        reasons.append("conversion-required-v1")
    remaining_network = _remaining_network(offer)
    if offer.cache_verified and remaining_network == 0:
        reasons.append("verified-complete-cache")
        return RankedOffer(offer, 0.0, 0.0, sample_count, observed_range, "high", tuple(reasons))
    if offer.network_bytes is None:
        reasons.append("network-bytes-unknown")
        return RankedOffer(offer, None, None, sample_count, observed_range, "unknown", tuple(reasons))
    if sample_count < 3:
        reasons.append("fewer-than-three-successes")
        return RankedOffer(offer, None, None, sample_count, observed_range, "unknown", tuple(reasons))
    startup = statistics.median(row.startup_seconds for row in successes)
    predicted = startup
    reasons.append(f"startup-median:{startup:.6f}")
    if remaining_network > 0:
        network_rates = [
            row.network_bytes / row.network_seconds
            for row in successes
            if row.network_bytes > 0 and row.network_seconds is not None
        ]
        if len(network_rates) < 3:
            reasons.append("network-estimate-unknown")
            return RankedOffer(offer, None, None, sample_count, observed_range, "unknown", tuple(reasons))
        rate = statistics.median(network_rates)
        predicted += remaining_network / rate
        reasons.append(f"network-median-bps:{rate:.3f}")
    local_copy = offer.local_copy_bytes or 0
    if local_copy > 0:
        copy_rates = [
            row.local_copy_bytes / row.local_copy_seconds
            for row in successes
            if row.local_copy_bytes > 0 and row.local_copy_seconds is not None
        ]
        if len(copy_rates) < 3:
            reasons.append("copy-estimate-unknown")
            return RankedOffer(offer, None, None, sample_count, observed_range, "unknown", tuple(reasons))
        rate = statistics.median(copy_rates)
        predicted += local_copy / rate
        reasons.append(f"copy-median-bps:{rate:.3f}")
    success_rate = sum(row.success for row in rows) / len(rows) if rows else 0.0
    adjusted = predicted / max(success_rate, 0.25)
    reasons.append(f"success-rate:{success_rate:.3f}")
    confidence = "high" if sample_count >= 5 else "medium"
    return RankedOffer(
        offer,
        predicted,
        adjusted,
        sample_count,
        observed_range,
        confidence,
        tuple(reasons),
    )


def _remaining_network(offer: AcquisitionOffer) -> int:
    if offer.network_bytes is None:
        return 0
    return max(offer.network_bytes - offer.cached_bytes, 0)


def fallback_compatible(failed: AcquisitionOffer, candidate: AcquisitionOffer) -> bool:
    return (
        failed.exact_group_key is not None
        and failed.exact_group_key == candidate.exact_group_key
        and failed.quantization == candidate.quantization
        and failed.category == candidate.category
        and failed.family == candidate.family
    )


def choose_fallback(
    failed: AcquisitionOffer,
    candidates: Iterable[AcquisitionOffer],
    *,
    failure_code: str,
) -> AcquisitionOffer | None:
    if failure_code not in {
        "download-transport-error",
        "download-timeout",
        "provider-nonzero-exit",
    }:
        return None
    compatible = [
        candidate
        for candidate in candidates
        if candidate.provider != failed.provider and fallback_compatible(failed, candidate)
    ]
    return compatible[0] if len(compatible) == 1 else None
