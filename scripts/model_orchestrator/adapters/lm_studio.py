"""LM Studio inventory preferring the supported ``lms`` CLI."""

from __future__ import annotations

from pathlib import Path
import json
import os
import subprocess
from typing import Any, Mapping, Protocol, Sequence

from ..models import (
    AdapterCapabilities,
    Artifact,
    ArtifactIdentity,
    RootEvidence,
    SourceError,
    ToolInstallation,
    ToolReference,
    MigrationPlan,
    MigrationStep,
    MigrationValidation,
)
from ..migration import MigrationError
from ..paths import ensure_owned_target
from ..paths import file_evidence
from . import AdapterContext, AdapterObservation, bounded_files, command_json, executable_version


class LMStudioAdapter:
    name = "lm-studio"

    def inventory(self, context: AdapterContext) -> AdapterObservation:
        executable = context.which("lms")
        root, source, confidence = self._root(context)
        installation = ToolInstallation(
            tool=self.name,
            detected=executable is not None or root.exists(),
            version=executable_version(context, executable),
            executable=executable,
            roots=(str(root),),
            root_evidence=(RootEvidence(str(root), source, confidence),),
            discovery_source="lms-cli+models-root",
            confidence=confidence,
            capabilities=AdapterCapabilities(
                inventory=True,
                reference=True,
                hard_link=True,
                symbolic_link=True,
                copy=True,
                native_import=True,
                load_validation=True,
                inference_validation=True,
            ),
        )
        observation = AdapterObservation(installation)
        loaded: set[str] = set()
        rows: list[dict[str, Any]] = []
        if executable:
            listed, error = command_json(context, (executable, "ls", "--json"), source=self.name)
            if error:
                observation.errors.append(error)
            else:
                rows = self._rows(listed, observation)
            processes, ps_error = command_json(context, (executable, "ps", "--json"), source=self.name)
            if ps_error:
                observation.errors.append(ps_error)
            else:
                loaded = self._loaded_ids(processes)
        paths: dict[str, tuple[Path, str]] = {}
        for row in rows:
            raw_path = row.get("path")
            identifier = str(row.get("modelKey") or row.get("id") or raw_path or "")
            if isinstance(raw_path, str) and raw_path:
                paths[str(Path(raw_path).absolute())] = (Path(raw_path), identifier)
        if not paths and root.is_dir():
            files, error = bounded_files(root, {".gguf", ".safetensors"})
            if error:
                observation.errors.append(SourceError(self.name, error.code, error.message))
            for path in files:
                paths[str(path.absolute())] = (path, path.stem)
        for rendered, (path, identifier) in paths.items():
            try:
                evidence = file_evidence(path)
                stat = path.stat()
            except OSError as exc:
                observation.errors.append(SourceError(self.name, "file-inaccessible", str(exc)))
                continue
            suffix = path.suffix.lower()
            observation.artifacts.append(
                Artifact(
                    artifact_id=f"lm-studio:{rendered}",
                    path=rendered,
                    family="llm",
                    format="gguf" if suffix == ".gguf" else "safetensors",
                    identity=ArtifactIdentity("unknown", source="lms-cli"),
                    logical_size=stat.st_size,
                    allocated_size=evidence.allocated_size,
                    relationship=evidence.relationship,
                    allocation_id=evidence.allocation_id,
                    references=[ToolReference(self.name, identifier, owner=True, loaded=identifier in loaded)],
                )
            )
        return observation

    @staticmethod
    def _root(context: AdapterContext) -> tuple[Path, str, str]:
        override = context.env.get("LM_STUDIO_MODELS_DIR", "").strip()
        if override:
            return Path(override), "environment:LM_STUDIO_MODELS_DIR", "high"
        return context.home / ".lmstudio/models", "documented-default", "medium"

    @staticmethod
    def _rows(payload: Any, observation: AdapterObservation) -> list[dict[str, Any]]:
        if isinstance(payload, list) and all(isinstance(row, dict) for row in payload):
            return payload
        if isinstance(payload, dict):
            for key in ("models", "data"):
                rows = payload.get(key)
                if isinstance(rows, list) and all(isinstance(row, dict) for row in rows):
                    return rows
        observation.errors.append(SourceError("lm-studio", "cli-payload-invalid", "Liste de modèles absente"))
        return []

    @staticmethod
    def _loaded_ids(payload: Any) -> set[str]:
        if isinstance(payload, dict):
            payload = payload.get("models", payload.get("data", []))
        if not isinstance(payload, list):
            return set()
        return {
            str(row.get("modelKey") or row.get("id"))
            for row in payload if isinstance(row, dict) and (row.get("modelKey") or row.get("id"))
        }


class LMStudioMigrationBackend(Protocol):
    def dry_run_import(self, source: str, method: str) -> tuple[str, str]: ...
    def import_model(self, source: str, method: str) -> tuple[str, str]: ...
    def listed_digest(self, native_id: str) -> str | None: ...
    def load(self, native_id: str) -> bool: ...
    def infer(self, native_id: str) -> bool: ...
    def unload(self, native_id: str) -> None: ...


class LMStudioMigrationDriver:
    def __init__(self, backend: LMStudioMigrationBackend):
        self.backend = backend

    def planned_steps(self, plan: MigrationPlan) -> list[MigrationStep]:
        if plan.target_path is None:
            raise MigrationError("destination-path-unknown", "La cible LM Studio est requise.")
        return [
            MigrationStep(
                "import", "lms-import", plan.target_path, "unlink-created-import"
            ),
            MigrationStep(
                "load", "lms-load", plan.destination_native_id, "lms-unload"
            ),
        ]

    def execute(self, plan, steps, persist):
        expected = (plan.target_path, plan.destination_native_id)
        if self.backend.dry_run_import(plan.source_path, plan.method) != expected:
            raise MigrationError("migration-plan-stale", "Le dry-run lms a changé.")
        import_step, load_step = steps
        import_step.state = "executing"
        persist()
        try:
            actual = self.backend.import_model(plan.source_path, plan.method)
        except Exception:
            if plan.target_path and Path(plan.target_path).exists():
                self._record_created(import_step)
                persist()
            raise
        if actual != expected:
            if actual[0] == plan.target_path:
                self._record_created(import_step)
                persist()
            raise MigrationError("lms-import-output-invalid", "La cible lms réelle diffère du plan.")
        self._record_created(import_step)
        persist()
        digest = self.backend.listed_digest(plan.destination_native_id)
        identity = "passed" if digest == plan.source_sha256 else "failed"
        catalog = "passed" if digest is not None else "failed"
        load_step.state = "executing"
        persist()
        loaded = self.backend.load(plan.destination_native_id)
        load_step.created_by_operation = loaded
        load_step.state = "created" if loaded else "failed"
        persist()
        inferred = loaded and self.backend.infer(plan.destination_native_id)
        return MigrationValidation(
            identity=identity,
            catalog=catalog,
            load="passed" if loaded else "failed",
            inference="passed" if inferred else "failed",
            destination_digest=digest,
        )

    def rollback(self, plan, steps, persist):
        refused = False
        for step in reversed(steps):
            if not step.created_by_operation or step.state == "rolled-back":
                continue
            if step.kind == "lms-load":
                try:
                    self.backend.unload(plan.destination_native_id)
                    step.state = "rolled-back"
                except Exception:
                    step.state = "rollback-refused"
                    refused = True
                persist()
                continue
            target = Path(step.target)
            try:
                ensure_owned_target(
                    str(target),
                    owned_root=plan.destination_root,
                    platform_name="windows" if os.name == "nt" else "linux",
                )
            except ValueError:
                step.state = "rollback-refused"
                refused = True
                persist()
                continue
            try:
                if target.is_file() or target.is_symlink():
                    current = target.lstat()
                    current_id = f"{current.st_dev}:{current.st_ino}"
                    if (
                        current_id != step.created_allocation_id
                        or current.st_size != step.created_size
                        or current.st_mtime_ns != step.created_mtime_ns
                    ):
                        step.state = "rollback-refused"
                        refused = True
                        persist()
                        continue
                    target.unlink()
            except OSError:
                step.state = "rollback-refused"
                refused = True
                persist()
                continue
            step.state = "rolled-back"
            persist()
        if refused:
            raise MigrationError(
                "migration-rollback-incomplete", "Une cible modifiée n'a pas été supprimée."
            )

    @staticmethod
    def _record_created(step: MigrationStep) -> None:
        created = Path(step.target).lstat()
        step.created_by_operation = True
        step.created_allocation_id = f"{created.st_dev}:{created.st_ino}"
        step.created_size = created.st_size
        step.created_mtime_ns = created.st_mtime_ns
        step.state = "created"


class LMSCliMigrationBackend:
    def __init__(self, executable: str = "lms", runner=None, env=None):
        self.executable = executable
        self.runner = runner or self._run
        self.env = dict(os.environ if env is None else env)

    @staticmethod
    def _run(command: Sequence[str], env: Mapping[str, str]):
        return subprocess.run(
            list(command), capture_output=True, text=True, timeout=300, check=False,
            env=dict(env),
        )

    def dry_run_import(self, source: str, method: str) -> tuple[str, str]:
        return self._import(source, method, dry_run=True)

    def import_model(self, source: str, method: str) -> tuple[str, str]:
        return self._import(source, method, dry_run=False)

    def _import(self, source: str, method: str, *, dry_run: bool) -> tuple[str, str]:
        command = [self.executable, "import", source, "--json", "--mode", method]
        if dry_run:
            command.append("--dry-run")
        payload = self._json(command)
        if not isinstance(payload, dict):
            raise MigrationError("lms-import-output-invalid", "Sortie lms import invalide.")
        target = payload.get("targetPath")
        native_id = payload.get("modelKey") or payload.get("id")
        if not isinstance(target, str) or not isinstance(native_id, str):
            raise MigrationError("lms-import-output-invalid", "Sortie lms import invalide.")
        return target, native_id

    def listed_digest(self, native_id: str) -> str | None:
        payload = self._json([self.executable, "ls", "--json"])
        rows = payload if isinstance(payload, list) else payload.get("models", [])
        matches = [
            row for row in rows
            if isinstance(row, dict)
            and (row.get("modelKey") == native_id or row.get("id") == native_id)
        ]
        if len(matches) != 1 or not isinstance(matches[0].get("sha256"), str):
            return None
        return matches[0]["sha256"].removeprefix("sha256:").lower()

    def load(self, native_id: str) -> bool:
        result = self.runner(
            [self.executable, "load", native_id, "--yes"], self.env
        )
        return result.returncode == 0

    def infer(self, native_id: str) -> bool:
        result = self.runner(
            [self.executable, "chat", native_id, "--prompt", "Reply with OK", "--json"],
            self.env,
        )
        return result.returncode == 0 and bool(result.stdout.strip())

    def unload(self, native_id: str) -> None:
        self.runner([self.executable, "unload", native_id], self.env)

    def _json(self, command: Sequence[str]):
        result = self.runner(list(command), self.env)
        if result.returncode != 0:
            raise MigrationError("lms-command-failed", "Une commande lms a échoué.")
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise MigrationError("lms-command-output-invalid", "JSON lms invalide.") from exc
        if not isinstance(payload, (dict, list)):
            raise MigrationError("lms-command-output-invalid", "JSON lms invalide.")
        return payload
