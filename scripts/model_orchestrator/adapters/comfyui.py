"""ComfyUI model-root, live category, and saved-workflow inventory."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Protocol
from urllib.parse import urlsplit

from scripts.local_ai.ollama_http import LOOPBACK_HOSTS, RejectRedirects

from ..models import (
    AdapterCapabilities,
    Artifact,
    ArtifactIdentity,
    RootEvidence,
    SourceError,
    ToolInstallation,
    ToolReference,
    GuidedMigration,
    ManualStep,
)
from ..migration import (
    GuidedMigrationStore,
    MigrationError,
    complete_guided_visibility,
    observes_exact_guided_source,
)
from ..paths import file_evidence
from . import AdapterContext, AdapterObservation, bounded_files, executable_version

CATEGORIES = {
    "checkpoints": "checkpoint",
    "diffusion_models": "diffusion-model",
    "unet": "diffusion-model",
    "vae": "vae",
    "loras": "lora",
    "controlnet": "controlnet",
    "text_encoders": "text-encoder",
    "clip": "text-encoder",
    "clip_vision": "clip-vision",
    "upscale_models": "upscale",
}
MODEL_SUFFIXES = {".safetensors", ".ckpt", ".pt", ".pth", ".bin", ".gguf"}


class ComfyUIAdapter:
    name = "comfyui"

    def inventory(self, context: AdapterContext) -> AdapterObservation:
        executable = context.which("comfy") or context.which("comfyui")
        roots, config_errors = self._roots(context)
        installation = ToolInstallation(
            tool=self.name,
            detected=executable is not None or any(path.exists() for path, _, _ in roots),
            version=executable_version(context, executable),
            executable=executable,
            roots=tuple(str(path) for path, _, _ in roots),
            root_evidence=tuple(RootEvidence(str(path), source, confidence) for path, source, confidence in roots),
            discovery_source="model-roots+extra-paths+api+workflows",
            confidence="high" if any(source.startswith("environment") or source.startswith("extra-model-paths") for _, source, _ in roots) else "medium",
            capabilities=AdapterCapabilities(inventory=True, reference=True, symbolic_link=True),
        )
        observation = AdapterObservation(installation, errors=config_errors)
        api_names = self._api_names(context, observation)
        workflow_names = self._workflow_names(context, roots, observation)
        seen: set[str] = set()
        for root, _, _ in roots:
            if not root.is_dir():
                continue
            files, error = bounded_files(root, MODEL_SUFFIXES)
            if error:
                observation.errors.append(SourceError(self.name, error.code, error.message))
            for path in files:
                rendered = str(path.absolute())
                if rendered in seen:
                    continue
                seen.add(rendered)
                try:
                    evidence = file_evidence(path)
                    stat = path.stat()
                except OSError as exc:
                    observation.errors.append(SourceError(self.name, "file-inaccessible", str(exc)))
                    continue
                category = self._category(path, root)
                references = [ToolReference(self.name, rendered, owner=True)]
                if path.name in api_names or rendered in api_names:
                    references.append(ToolReference(self.name, f"api:{path.name}", kind="live-catalog"))
                if path.name in workflow_names:
                    references.extend(
                        ToolReference(self.name, workflow, kind="workflow", workflow=True)
                        for workflow in sorted(workflow_names[path.name])
                    )
                observation.artifacts.append(
                    Artifact(
                        artifact_id=f"comfyui:{rendered}",
                        path=rendered,
                        family="image",
                        format=path.suffix.lower().lstrip("."),
                        identity=ArtifactIdentity("unknown", source="comfyui-file"),
                        logical_size=stat.st_size,
                        allocated_size=evidence.allocated_size,
                        category=category,
                        relationship=evidence.relationship,
                        allocation_id=evidence.allocation_id,
                        references=references,
                    )
                )
        return observation

    def _roots(
        self, context: AdapterContext
    ) -> tuple[list[tuple[Path, str, str]], list[SourceError]]:
        override = context.env.get("COMFYUI_MODELS_DIR", "").strip()
        if override:
            roots = [
                (Path(value), "environment:COMFYUI_MODELS_DIR", "high")
                for value in override.split(os.pathsep) if value
            ]
        elif context.platform_name == "windows":
            appdata = Path(context.env.get("APPDATA", str(context.home / "AppData/Roaming")))
            roots = [(appdata / "ComfyUI/models", "documented-default", "medium")]
        else:
            roots = [(context.home / "ComfyUI/models", "documented-default", "medium")]
        config = context.env.get("COMFYUI_EXTRA_MODEL_PATHS", "").strip()
        configs = [Path(config)] if config else [roots[0][0].parent / "extra_model_paths.yaml"]
        errors: list[SourceError] = []
        for config_path in configs:
            if config_path.exists():
                errors.append(
                    SourceError(
                        self.name,
                        "user-yaml-unparsed",
                        f"{config_path}: YAML utilisateur laissé intact; racines attendues via un hook documenté.",
                        confidence="medium",
                    )
                )
        registered = context.env.get("COMFYUI_REGISTERED_MODEL_ROOTS", "").strip()
        if registered:
            roots.extend(
                (Path(value), "documented-setting-or-launch-hook", "high")
                for value in registered.split(os.pathsep)
                if value
            )
        unique: dict[str, tuple[Path, str, str]] = {}
        for row in roots:
            unique.setdefault(str(row[0].absolute()), row)
        return list(unique.values()), errors

    def _api_names(self, context: AdapterContext, observation: AdapterObservation) -> set[str]:
        request = context.comfy_request
        if request is None:
            request = lambda path: self._default_request(context, path)
        names: set[str] = set()
        try:
            categories = request("/models")
            if not isinstance(categories, list):
                raise ValueError("La liste des catégories est absente")
            for category in categories:
                if category not in CATEGORIES:
                    continue
                payload = request(f"/models/{category}")
                if not isinstance(payload, list):
                    raise ValueError(f"Catégorie {category} invalide")
                names.update(str(item) for item in payload if isinstance(item, str))
        except (OSError, urllib.error.URLError, urllib.error.HTTPError, TimeoutError, ValueError, json.JSONDecodeError) as exc:
            observation.errors.append(SourceError(self.name, "comfyui-api-unavailable", str(exc)))
        return names

    @staticmethod
    def _default_request(context: AdapterContext, path: str) -> Any:
        raw = context.env.get("COMFYUI_HOST", "http://127.0.0.1:8188")
        candidate = raw if "://" in raw else f"http://{raw}"
        parsed = urlsplit(candidate)
        if parsed.scheme != "http" or (parsed.hostname or "").lower() not in LOOPBACK_HOSTS:
            raise ValueError("ComfyUI doit utiliser une origine HTTP loopback")
        if parsed.path not in ("", "/") or parsed.query or parsed.fragment or parsed.username:
            raise ValueError("L'origine ComfyUI est invalide")
        port = parsed.port or 8188
        host = f"[{parsed.hostname}]" if parsed.hostname == "::1" else parsed.hostname
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), RejectRedirects())
        with opener.open(f"http://{host}:{port}{path}", timeout=5.0) as response:
            if response.getcode() != 200:
                raise OSError(f"HTTP {response.getcode()}")
            return json.loads(response.read().decode("utf-8"))

    def _workflow_names(
        self,
        context: AdapterContext,
        roots: list[tuple[Path, str, str]],
        observation: AdapterObservation,
    ) -> dict[str, set[str]]:
        workflow_root = context.env.get("COMFYUI_WORKFLOWS_DIR", "").strip()
        candidates = [Path(workflow_root)] if workflow_root else [
            roots[0][0].parent / "user/default/workflows"
        ]
        references: dict[str, set[str]] = {}
        for root in candidates:
            if not root.is_dir():
                continue
            files, error = bounded_files(root, {".json"})
            if error:
                observation.errors.append(SourceError(self.name, error.code, error.message))
            for workflow in files:
                try:
                    payload = json.loads(workflow.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError) as exc:
                    observation.errors.append(SourceError(self.name, "workflow-invalid", f"{workflow}: {exc}"))
                    continue
                for value in self._strings(payload):
                    if Path(value).suffix.lower() in MODEL_SUFFIXES:
                        references.setdefault(Path(value).name, set()).add(str(workflow))
        return references

    @classmethod
    def _strings(cls, value: Any):
        if isinstance(value, str):
            yield value
        elif isinstance(value, dict):
            for nested in value.values():
                yield from cls._strings(nested)
        elif isinstance(value, list):
            for nested in value:
                yield from cls._strings(nested)

    @staticmethod
    def _category(path: Path, root: Path) -> str | None:
        try:
            parts = path.relative_to(root).parts
        except ValueError:
            return None
        if len(parts) > 1:
            return CATEGORIES.get(parts[0])
        return CATEGORIES.get(root.name)


class ComfyHookBackend(Protocol):
    def supported_hook(self) -> str | None: ...
    def register(self, config_path: str, hook: str) -> None: ...
    def unregister(self, config_path: str, hook: str) -> None: ...


class DevToolBoxComfyLaunchBackend:
    """Register the documented CLI flag only in a DevToolBox-owned launcher file."""

    def __init__(self, registry_path: str | Path, *, flag_supported: bool):
        self.registry_path = Path(registry_path)
        self.flag_supported = flag_supported

    def supported_hook(self) -> str | None:
        return "launch-arg" if self.flag_supported else None

    def register(self, config_path: str, hook: str) -> None:
        if hook != "launch-arg":
            raise ValueError("Hook ComfyUI non pris en charge")
        payload = self._load()
        configs = payload.setdefault("extra_model_paths_configs", [])
        if config_path not in configs:
            configs.append(config_path)
        self._save(payload)

    def unregister(self, config_path: str, hook: str) -> None:
        if hook != "launch-arg":
            raise ValueError("Hook ComfyUI non pris en charge")
        payload = self._load()
        configs = payload.setdefault("extra_model_paths_configs", [])
        payload["extra_model_paths_configs"] = [
            value for value in configs if value != config_path
        ]
        self._save(payload)

    def _load(self) -> dict[str, Any]:
        try:
            payload = json.loads(self.registry_path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return {"schema_version": 1, "extra_model_paths_configs": []}
        if (
            not isinstance(payload, dict)
            or payload.get("schema_version") != 1
            or not isinstance(payload.get("extra_model_paths_configs"), list)
        ):
            raise ValueError("Registre de lancement DevToolBox invalide")
        return payload

    def _save(self, payload: dict[str, Any]) -> None:
        self.registry_path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.registry_path.with_suffix(".tmp")
        temporary.write_text(
            json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        os.replace(temporary, self.registry_path)


_CATEGORY_CONFIG_KEYS = {
    "checkpoint": "checkpoints",
    "diffusion-model": "diffusion_models",
    "vae": "vae",
    "lora": "loras",
    "controlnet": "controlnet",
    "text-encoder": "text_encoders",
    "clip-vision": "clip_vision",
    "upscale": "upscale_models",
}


class ComfyUIGuidedIntegration:
    """Own one separate YAML file and never rewrite arbitrary ComfyUI YAML."""

    def __init__(
        self,
        store: GuidedMigrationStore,
        owned_config_root: str | Path,
        backend: ComfyHookBackend,
    ):
        self.store = store
        self.owned_config_root = Path(owned_config_root)
        self.backend = backend

    def prepare(
        self, migration: GuidedMigration, observation: AdapterObservation
    ) -> GuidedMigration:
        self.store.revalidate_source(migration)
        if migration.category not in _CATEGORY_CONFIG_KEYS:
            raise MigrationError("comfy-category-unsupported", "Catégorie ComfyUI invalide.")
        already_observed = observes_exact_guided_source(migration, observation)
        if already_observed:
            if self.live_visible(migration, observation):
                complete_guided_visibility(
                    migration,
                    visible=True,
                    workflow=self.workflow_state(migration, observation),
                )
            else:
                migration.state = "pending-validation"
            self.store.save(migration)
            return migration

        self.owned_config_root.mkdir(parents=True, exist_ok=True)
        config = self.owned_config_root / f"{migration.migration_id}.yaml"
        if config.exists():
            raise MigrationError(
                "comfy-owned-config-collision", "Le fichier géré existe déjà."
            )
        migration.owned_config_path = str(config)
        migration.state = "configuring"
        self.store.save(migration)
        category_key = _CATEGORY_CONFIG_KEYS[migration.category]
        content = (
            "devtoolbox:\n"
            f"  base_path: {json.dumps(str(Path(migration.source_path).parent))}\n"
            f"  {category_key}: "
            + json.dumps(".")
            + "\n"
        )
        temporary = config.with_suffix(".tmp")
        temporary.write_text(content, encoding="utf-8")
        os.replace(temporary, config)
        stat = config.lstat()
        migration.config_allocation_id = f"{stat.st_dev}:{stat.st_ino}:{stat.st_size}:{stat.st_mtime_ns}"
        migration.state = "config-created"
        self.store.save(migration)
        hook = self.backend.supported_hook()
        if hook is not None:
            self.backend.register(str(config), hook)
            migration.registration_created = True
            migration.state = "pending-validation"
        else:
            migration.manual_step = ManualStep(
                step_id="comfy-extra-model-paths",
                source_path=migration.source_path,
                destination_tool="comfyui",
                documented_action=(
                    "Chargez le fichier DevToolBox séparé avec le réglage Desktop documenté "
                    f"ou --extra-model-paths-config {config}."
                ),
                expected_reference=f"live-category:{migration.category}:{Path(migration.source_path).name}",
                resume_condition="L'API /models doit exposer ce fichier dans la catégorie exacte.",
            )
            migration.state = "pending-manual"
        self.store.save(migration)
        return migration

    def resume(
        self, migration: GuidedMigration, observation: AdapterObservation
    ) -> GuidedMigration:
        self.store.revalidate_source(migration)
        visible = self.live_visible(migration, observation)
        complete_guided_visibility(
            migration,
            visible=visible,
            workflow=self.workflow_state(migration, observation) if visible else "unavailable",
        )
        self.store.save(migration)
        return migration

    def rollback(self, migration: GuidedMigration) -> GuidedMigration:
        registration_unresolved = False
        if migration.registration_created and migration.owned_config_path:
            hook = self.backend.supported_hook()
            if hook is not None:
                try:
                    self.backend.unregister(migration.owned_config_path, hook)
                    migration.registration_created = False
                except Exception:
                    registration_unresolved = True
            else:
                registration_unresolved = True
        if migration.owned_config_path:
            config = Path(migration.owned_config_path)
            if registration_unresolved:
                migration.state = "rollback-incomplete"
            elif config.is_file() and not config.is_symlink():
                stat = config.lstat()
                evidence = f"{stat.st_dev}:{stat.st_ino}:{stat.st_size}:{stat.st_mtime_ns}"
                if evidence == migration.config_allocation_id:
                    config.unlink()
                    migration.state = "rolled-back"
                else:
                    migration.state = "rollback-incomplete"
        migration.retirement_eligible = False
        self.store.save(migration)
        return migration

    @staticmethod
    def live_visible(migration: GuidedMigration, observation: AdapterObservation) -> bool:
        if not observes_exact_guided_source(migration, observation):
            return False
        source = Path(migration.source_path)
        for artifact in observation.artifacts:
            if artifact.category != migration.category:
                continue
            try:
                same = source.samefile(artifact.path)
            except OSError:
                same = artifact.identity.exact_key == f"sha256:{migration.source_sha256}"
            if same and any(
                reference.tool == "comfyui" and reference.kind == "live-catalog"
                for reference in artifact.references
            ):
                return True
        return False

    @staticmethod
    def workflow_state(migration: GuidedMigration, observation: AdapterObservation) -> str:
        source = Path(migration.source_path)
        for artifact in observation.artifacts:
            try:
                same = source.samefile(artifact.path)
            except OSError:
                same = artifact.identity.exact_key == f"sha256:{migration.source_sha256}"
            if same and any(reference.workflow for reference in artifact.references):
                return "passed"
        return "none"
