"""Pure catalog aggregation, identity grouping, and protection rules."""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Iterable

from .models import Artifact, CatalogSnapshot, Protection, SourceError, ToolInstallation


def protection_reasons(artifact: Artifact) -> list[str]:
    reasons = set(artifact.protection.reasons)
    if artifact.identity.state != "verified":
        reasons.add("identity-unverified")
    for reference in artifact.references:
        reasons.add(f"referenced:{reference.tool}")
        if reference.loaded:
            reasons.add(f"loaded:{reference.tool}")
        if reference.workflow:
            reasons.add(f"workflow:{reference.tool}")
    return sorted(reasons)


def _apply_duplicate_groups(artifacts: list[Artifact]) -> None:
    groups: dict[str, list[Artifact]] = {}
    for artifact in artifacts:
        exact_key = artifact.identity.exact_key
        if exact_key is not None:
            groups.setdefault(exact_key, []).append(artifact)
    for exact_key, matches in groups.items():
        if len(matches) < 2:
            continue
        for artifact in matches:
            artifact.duplicate_group = exact_key


def build_snapshot(
    *,
    platform: str,
    artifacts: Iterable[Artifact],
    installations: Iterable[ToolInstallation] = (),
    source_errors: Iterable[SourceError] = (),
    warnings: Iterable[str] = (),
    generated_at: str | None = None,
) -> CatalogSnapshot:
    rows = sorted(list(artifacts), key=lambda item: (item.family, item.path, item.artifact_id))
    for artifact in rows:
        reasons = protection_reasons(artifact)
        artifact.protection = Protection(bool(reasons), reasons)
    _apply_duplicate_groups(rows)
    return CatalogSnapshot(
        generated_at=generated_at or datetime.now(timezone.utc).isoformat(),
        platform=platform,
        installations=sorted(list(installations), key=lambda item: item.tool),
        artifacts=rows,
        source_errors=sorted(list(source_errors), key=lambda item: (item.source, item.code)),
        warnings=sorted(set(warnings)),
    )
