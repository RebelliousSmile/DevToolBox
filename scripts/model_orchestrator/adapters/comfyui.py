"""ComfyUI model-root, live category, and saved-workflow inventory."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any
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
            additions, error = self._extra_roots(config_path)
            roots.extend((path, f"extra-model-paths:{config_path}", "high") for path in additions)
            if error:
                errors.append(error)
        unique: dict[str, tuple[Path, str, str]] = {}
        for row in roots:
            unique.setdefault(str(row[0].absolute()), row)
        return list(unique.values()), errors

    def _extra_roots(self, config: Path) -> tuple[list[Path], SourceError | None]:
        try:
            lines = config.read_text(encoding="utf-8").splitlines()
        except FileNotFoundError:
            return [], None
        except OSError as exc:
            return [], SourceError(self.name, "config-inaccessible", str(exc))
        roots: list[Path] = []
        base: Path | None = None
        active_category: str | None = None
        try:
            for raw in lines:
                content = raw.split("#", 1)[0].strip()
                if not content:
                    continue
                key, separator, value = content.partition(":")
                if not separator:
                    if active_category and raw[:1].isspace():
                        candidate = Path(content.lstrip("- "))
                        roots.append((base / candidate) if base and not candidate.is_absolute() else candidate)
                        continue
                    raise ValueError(f"ligne invalide: {raw}")
                value = value.strip().strip("'\"")
                if key.strip() == "base_path":
                    base = Path(value)
                elif key.strip() in CATEGORIES:
                    active_category = key.strip()
                    if value and value not in {"|", ">"}:
                        candidate = Path(value)
                        roots.append((base / candidate) if base and not candidate.is_absolute() else candidate)
        except ValueError as exc:
            return roots, SourceError(self.name, "config-payload-invalid", str(exc))
        return roots, None

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
