"""Capability-gated startup reconciliation and exact recovery actions."""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Callable, Mapping, Set

from .library import LibraryError, NeutralLibrary
from .models import RecoveryAction
from .paths import PathSafetyError, ensure_owned_target

_EXACT_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")


def reconcile_operations(
    library: NeutralLibrary,
    *,
    capabilities: Mapping[str, Set[str]],
    migration_journal_root: str | Path | None = None,
) -> list[RecoveryAction]:
    actions: list[RecoveryAction] = []
    for journal in library.reconcile():
        proofs = capabilities.get(journal.operation_id, set())
        if journal.state == "resumable":
            actions.append(
                RecoveryAction(
                    journal.operation_id,
                    "resume",
                    "resume" in proofs,
                    "Métadonnées de reprise prouvées." if "resume" in proofs else "Fournisseur sans preuve de reprise.",
                )
            )
        elif journal.state == "discardable":
            actions.append(
                RecoveryAction(
                    journal.operation_id,
                    "discard-partial",
                    "discard-partial" in proofs,
                    "Staging possédé et sans artefact canonique.",
                )
            )
        elif journal.state == "manual-attention":
            actions.append(
                RecoveryAction(
                    journal.operation_id,
                    "manual-attention",
                    False,
                    journal.error or "État ambigu.",
                )
            )
    if migration_journal_root is not None:
        root = Path(migration_journal_root)
        if root.is_dir():
            for path in sorted(root.glob("*.json")):
                operation_id = path.stem
                try:
                    payload = json.loads(path.read_text(encoding="utf-8"))
                    steps = payload.get("steps", [])
                    created = any(step.get("created_by_operation") for step in steps)
                except (OSError, AttributeError, json.JSONDecodeError):
                    created = False
                if created:
                    proofs = capabilities.get(operation_id, set())
                    actions.append(
                        RecoveryAction(
                            operation_id,
                            "rollback",
                            "rollback" in proofs,
                            "Rollback driver prouvé." if "rollback" in proofs else "Driver de rollback indisponible.",
                        )
                    )
    return actions


def recover_operation(
    library: NeutralLibrary,
    *,
    operation_id: str,
    action: str,
    capabilities: Mapping[str, Set[str]],
    rollback: Callable[[str], None] | None = None,
    migration_journal_root: str | Path | None = None,
) -> RecoveryAction:
    if _EXACT_ID.fullmatch(operation_id) is None:
        raise LibraryError("Identifiant d'opération invalide")
    available = reconcile_operations(
        library,
        capabilities=capabilities,
        migration_journal_root=migration_journal_root,
    )
    selected = next(
        (row for row in available if row.operation_id == operation_id and row.action == action),
        None,
    )
    if selected is None or not selected.available:
        raise LibraryError("Action de recovery non prouvée")
    if action == "discard-partial":
        operation = library.staging_root / operation_id
        if (
            operation.is_symlink()
            or operation.resolve().parent != library.staging_root.resolve()
        ):
            raise LibraryError("Staging recovery non possédé")
        library.discard(operation_id)
    elif action == "rollback":
        if rollback is None:
            raise LibraryError("Driver de rollback absent")
        if migration_journal_root is None:
            raise LibraryError("Journal de migration absent")
        _validate_migration_recovery(
            Path(migration_journal_root), operation_id
        )
        rollback(operation_id)
    elif action != "resume":
        raise LibraryError("Action de recovery inconnue")
    return selected


def _validate_migration_recovery(root: Path, operation_id: str) -> None:
    journal = root / f"{operation_id}.json"
    if journal.is_symlink() or journal.resolve().parent != root.resolve():
        raise LibraryError("Journal de rollback non possédé")
    try:
        payload = json.loads(journal.read_text(encoding="utf-8"))
        destination_root = payload["plan"]["destination_root"]
        steps = payload["steps"]
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
        raise LibraryError("Journal de rollback invalide") from exc
    for step in steps:
        target = step.get("target") if isinstance(step, dict) else None
        if not step.get("created_by_operation") or not isinstance(target, str):
            continue
        if os.path.isabs(target):
            try:
                ensure_owned_target(
                    target,
                    owned_root=destination_root,
                    platform_name="windows" if os.name == "nt" else "linux",
                )
            except PathSafetyError as exc:
                raise LibraryError("Cible de rollback hors propriété") from exc
