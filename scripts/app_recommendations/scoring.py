"""Deterministic v1 scoring for application removal candidates."""

from __future__ import annotations

from dataclasses import replace

from .models import Candidate

MIB = 1024**2
GIB = 1024**3


def size_points(size_bytes: int | None) -> int:
    if size_bytes is None or size_bytes < 250 * MIB:
        return 0
    if size_bytes < GIB:
        return 10
    if size_bytes < 5 * GIB:
        return 25
    if size_bytes < 10 * GIB:
        return 35
    return 50


def inactivity_points(covered_days: int | None) -> int:
    if covered_days is None or covered_days < 30:
        return 0
    if covered_days < 90:
        return 10
    if covered_days < 180:
        return 25
    if covered_days < 365:
        return 40
    return 50


def _overall_confidence(candidate: Candidate) -> str:
    values = {candidate.size.confidence, candidate.usage.confidence}
    if "unknown" in values:
        return "low"
    if "low" in values:
        return "low"
    if "medium" in values:
        return "medium"
    return "high"


def score_candidate(candidate: Candidate) -> Candidate:
    """Return a scored copy; protections always override score and command."""
    if candidate.protection.protected:
        return replace(
            candidate,
            command=None,
            score=0,
            confidence=_overall_confidence(candidate),
            reasons=[f"Protégé : {reason}" for reason in candidate.protection.reasons],
        )

    size_score = size_points(candidate.size.installed_bytes)
    usage_score = inactivity_points(
        candidate.usage.covered_days if candidate.usage.kind != "unknown" else None
    )
    reasons: list[str] = []
    if size_score:
        reasons.append(f"Empreinte disque : +{size_score}")
    elif candidate.size.installed_bytes is None:
        reasons.append("Empreinte disque inconnue : +0")
    else:
        reasons.append("Empreinte disque inférieure à 250 Mio : +0")

    if candidate.usage.kind == "known_last_seen":
        reasons.append(
            f"Dernier usage connu, {candidate.usage.covered_days} jours couverts depuis : +{usage_score}"
        )
    elif candidate.usage.kind == "not_observed":
        reasons.append(
            f"Non observé pendant le suivi ({candidate.usage.covered_days} jours couverts) : +{usage_score}"
        )
    else:
        reasons.append("Usage inconnu faute de couverture : +0")

    return replace(
        candidate,
        score=size_score + usage_score,
        confidence=_overall_confidence(candidate),
        reasons=reasons,
    )
