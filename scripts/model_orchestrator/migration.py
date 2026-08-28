"""Immutable migration planning, revalidation, execution, and scoped rollback."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Protocol

from .models import (
    LibraryRecord,
    MigrationPlan,
    MigrationResult,
    MigrationStep,
    MigrationValidation,
    ToolInstallation,
)
from .paths import PathSafetyError, ensure_owned_target, same_filesystem

_EXACT_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")


@dataclass(frozen=True)
class MigrationError(Exception):
    code: str
    message: str

    def __str__(self) -> str:
        return self.message


class MigrationDriver(Protocol):
    def planned_steps(self, plan: MigrationPlan) -> list[MigrationStep]: ...

    def execute(
        self,
        plan: MigrationPlan,
        steps: list[MigrationStep],
        persist: Callable[[], None],
    ) -> MigrationValidation: ...

    def rollback(
        self,
        plan: MigrationPlan,
        steps: list[MigrationStep],
        persist: Callable[[], None],
    ) -> None: ...


def create_migration_plan(
    *,
    plan_id: str,
    source: LibraryRecord,
    destination: ToolInstallation,
    destination_root: str,
    destination_native_id: str,
    target_path: str | None,
    shared_path_supported: bool = False,
    reflink_probe: Callable[[Path, Path], bool] | None = None,
) -> MigrationPlan:
    if _EXACT_ID.fullmatch(plan_id) is None:
        raise MigrationError("migration-plan-id-invalid", "L'identifiant de plan est invalide.")
    if not destination_native_id.strip():
        raise MigrationError("destination-id-invalid", "L'identifiant natif est requis.")
    if source.identity.state != "verified" or source.identity.value is None:
        raise MigrationError("source-identity-unverified", "La source doit avoir un SHA-256 vérifié.")
    if source.format != "gguf":
        raise MigrationError("source-format-unsupported", "Seul un GGUF exact est migrable ici.")
    if source.validation.level != "strong":
        raise MigrationError("source-validation-weak", "La validation structurelle forte est requise.")
    if not destination.detected or destination.confidence in {"unknown", "low"}:
        raise MigrationError("destination-ownership-unknown", "La destination n'est pas possédée avec assez de confiance.")
    if not destination.version:
        raise MigrationError("destination-version-unknown", "La version de destination est requise.")
    if destination_root not in destination.roots:
        raise MigrationError("destination-root-unknown", "La racine ne vient pas de l'inventaire figé.")
    root = Path(destination_root)
    if not root.is_absolute() or not root.is_dir() or root.is_symlink():
        raise MigrationError("destination-root-invalid", "La racine de destination est invalide.")
    source_path = Path(source.path)
    if not source_path.is_file() or source_path.is_symlink():
        raise MigrationError("source-path-invalid", "La source canonique est absente ou symbolique.")
    if target_path is not None:
        try:
            ensure_owned_target(
                target_path,
                owned_root=destination_root,
                platform_name="windows" if os.name == "nt" else "linux",
            )
        except PathSafetyError as exc:
            raise MigrationError("destination-path-unsafe", str(exc)) from exc
        target = Path(target_path)
        if _symlinked_parent(target, root):
            raise MigrationError(
                "destination-path-unsafe", "Un parent symbolique rend la cible ambiguë."
            )
        if target.exists():
            raise MigrationError("destination-collision", "La cible existe déjà.")
        if target.absolute() == source_path.absolute():
            raise MigrationError("migration-overlap", "La source et la cible se chevauchent.")
    capabilities = tuple(
        name
        for name, enabled in asdict(destination.capabilities).items()
        if enabled
    )
    method = _select_method(
        source_path,
        Path(target_path) if target_path else None,
        destination,
        shared_path_supported,
        reflink_probe,
    )
    try:
        source_path.resolve().relative_to(root.resolve())
        source_inside_destination = True
    except ValueError:
        source_inside_destination = False
    if source_inside_destination and method != "shared_path":
        raise MigrationError("migration-overlap", "La source chevauche la destination.")
    size = source_path.stat().st_size
    allocated = 0 if method in {"shared_path", "hard_link", "symbolic_link", "reflink"} else size
    temporary = size if method in {"native_import", "copy"} else 0
    free = shutil.disk_usage(root).free
    if free < allocated + temporary:
        raise MigrationError("destination-space-insufficient", "Espace destination insuffisant.")
    stat = source_path.stat()
    return MigrationPlan(
        plan_id=plan_id,
        source_artifact_id=source.artifact_id,
        source_path=str(source_path),
        source_sha256=source.identity.value,
        source_size=stat.st_size,
        source_mtime_ns=stat.st_mtime_ns,
        destination_tool=destination.tool,
        destination_version=destination.version,
        destination_root=destination_root,
        destination_native_id=destination_native_id,
        target_path=target_path,
        method=method,
        free_bytes=free,
        temporary_bytes=temporary,
        allocated_bytes=allocated,
        validation_level=source.validation.level,
        capabilities=capabilities,
        created_at=datetime.now(timezone.utc).isoformat(),
    )


def _select_method(source, target, destination, shared, reflink_probe):
    caps = destination.capabilities
    if shared and caps.reference:
        return "shared_path"
    if target is not None and caps.hard_link and same_filesystem(source, _existing_parent(target)):
        return "hard_link"
    if target is not None and caps.symbolic_link:
        return "symbolic_link"
    if caps.native_import:
        return "native_import"
    if target is not None and reflink_probe is not None and reflink_probe(source, target.parent):
        return "reflink"
    if caps.copy:
        return "copy"
    raise MigrationError("migration-method-unavailable", "Aucune méthode de migration sûre.")


def _existing_parent(path: Path) -> Path:
    candidate = path.parent
    while not candidate.exists() and candidate != candidate.parent:
        candidate = candidate.parent
    return candidate


def _symlinked_parent(path: Path, root: Path) -> bool:
    candidate = path.parent
    root_resolved = root.resolve()
    while candidate != candidate.parent:
        if candidate.exists() and candidate.is_symlink():
            return True
        if candidate.exists() and candidate.resolve() == root_resolved:
            return False
        candidate = candidate.parent
    return True


def revalidate_plan(plan: MigrationPlan, destination: ToolInstallation) -> None:
    source = Path(plan.source_path)
    if not source.is_file() or source.is_symlink():
        raise MigrationError("migration-plan-stale", "La source a disparu ou changé de type.")
    stat = source.stat()
    if stat.st_size != plan.source_size or stat.st_mtime_ns != plan.source_mtime_ns:
        raise MigrationError("migration-plan-stale", "Les métadonnées source ont changé.")
    if _sha256(source) != plan.source_sha256:
        raise MigrationError("migration-plan-stale", "L'identité source a changé.")
    after_hash = source.stat()
    if after_hash.st_size != stat.st_size or after_hash.st_mtime_ns != stat.st_mtime_ns:
        raise MigrationError("migration-plan-stale", "La source a changé pendant la validation.")
    if (
        not destination.detected
        or destination.confidence in {"unknown", "low"}
        or destination.tool != plan.destination_tool
        or destination.version != plan.destination_version
        or plan.destination_root not in destination.roots
    ):
        raise MigrationError("migration-plan-stale", "La destination inventoriée a changé.")
    current_capabilities = tuple(
        name
        for name, enabled in asdict(destination.capabilities).items()
        if enabled
    )
    if current_capabilities != plan.capabilities:
        raise MigrationError("migration-plan-stale", "Les capacités destination ont changé.")
    root = Path(plan.destination_root)
    if not root.is_dir() or root.is_symlink():
        raise MigrationError("migration-plan-stale", "La racine destination a changé.")
    if plan.target_path and Path(plan.target_path).exists():
        raise MigrationError("destination-collision", "La cible existe maintenant.")
    if plan.target_path and _symlinked_parent(Path(plan.target_path), root):
        raise MigrationError("migration-plan-stale", "Le chemin destination a changé.")
    current_free = shutil.disk_usage(plan.destination_root).free
    if current_free < plan.allocated_bytes + plan.temporary_bytes:
        raise MigrationError("destination-space-insufficient", "L'espace libre a changé.")


class MigrationExecutor:
    def __init__(self, journal_root: str | Path):
        self.journal_root = Path(journal_root)

    def apply(
        self,
        plan: MigrationPlan,
        *,
        destination: ToolInstallation,
        driver: MigrationDriver,
    ) -> MigrationResult:
        self.journal_root.mkdir(parents=True, exist_ok=True)
        journal = self.journal_root / f"{plan.plan_id}.json"
        steps = driver.planned_steps(plan)
        result: MigrationResult | None = None

        def persist() -> None:
            payload = {
                "plan": asdict(plan),
                "steps": [asdict(step) for step in steps],
                "result": None if result is None else asdict(result),
            }
            temporary = journal.with_suffix(".tmp")
            temporary.write_text(
                json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            os.replace(temporary, journal)

        def rollback_safely() -> str | None:
            try:
                driver.rollback(plan, steps, persist)
                return None
            except Exception:
                return "Le rollback natif est incomplet; intervention manuelle requise."

        persist()
        try:
            revalidate_plan(plan, destination)
            validation = driver.execute(plan, steps, persist)
            success = (
                validation.identity == "passed"
                and validation.catalog == "passed"
                and (validation.load == "passed" or validation.inference == "passed")
                and validation.destination_digest == plan.source_sha256
            )
            rollback_error = None
            if not success:
                rollback_error = rollback_safely()
            if not Path(plan.source_path).is_file():
                raise MigrationError("source-mutated", "La migration a modifié la source.")
            result = MigrationResult(
                plan.plan_id,
                success,
                tuple(steps),
                validation,
                retirement_eligible=success,
                confirmation_token=None,
                error_code=(
                    None
                    if success
                    else "migration-rollback-incomplete"
                    if rollback_error
                    else "migration-validation-failed"
                ),
                message=(
                    None
                    if success
                    else rollback_error
                    or "La validation a échoué; rollback limité exécuté."
                ),
            )
        except MigrationError as exc:
            rollback_error = rollback_safely()
            result = MigrationResult(
                plan.plan_id,
                False,
                tuple(steps),
                MigrationValidation(message=exc.message),
                error_code="migration-rollback-incomplete" if rollback_error else exc.code,
                message=rollback_error or exc.message,
            )
        except Exception:
            rollback_error = rollback_safely()
            result = MigrationResult(
                plan.plan_id,
                False,
                tuple(steps),
                MigrationValidation(message="Échec de migration."),
                error_code=(
                    "migration-rollback-incomplete" if rollback_error else "migration-failed"
                ),
                message=rollback_error or "Échec de migration sans suppression de source.",
            )
        persist()
        return result


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()
