"""Machine-local, non-roaming settings for the neutral model library."""

from __future__ import annotations

import json
import ntpath
import os
import shutil
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Mapping

from .paths import PathSafetyError, normalize_absolute_path

SETTINGS_SCHEMA_VERSION = 1
DEFAULT_PROVIDER_ORDER = ("ollama", "huggingface", "lm-studio", "direct")
KNOWN_PROVIDERS = frozenset(DEFAULT_PROVIDER_ORDER)


@dataclass(frozen=True)
class ModelSettings:
    library_root: str
    provider_order: tuple[str, ...] = DEFAULT_PROVIDER_ORDER
    enabled_providers: tuple[str, ...] = DEFAULT_PROVIDER_ORDER
    xet_enabled: bool = True
    keep_patterns: tuple[str, ...] = ()
    schema_version: int = SETTINGS_SCHEMA_VERSION

    def __post_init__(self) -> None:
        if set(self.provider_order) != KNOWN_PROVIDERS or len(self.provider_order) != len(
            KNOWN_PROVIDERS
        ):
            raise ValueError("provider_order doit contenir chaque fournisseur exactement une fois")
        if not set(self.enabled_providers).issubset(KNOWN_PROVIDERS):
            raise ValueError("enabled_providers contient un fournisseur inconnu")
        if not isinstance(self.xet_enabled, bool):
            raise ValueError("xet_enabled doit être booléen")


def state_root(*, platform_name: str, env: Mapping[str, str]) -> Path:
    if platform_name == "windows":
        raw = env.get("LOCALAPPDATA", "").strip()
        if not raw:
            raise PathSafetyError("LOCALAPPDATA est requis pour les réglages locaux")
        return Path(ntpath.join(raw, "DevToolBox"))
    if platform_name == "linux":
        raw = env.get("XDG_DATA_HOME", "").strip()
        if raw:
            return Path(raw) / "devtoolbox"
        home = env.get("HOME", "").strip()
        if not home:
            raise PathSafetyError("HOME est requis pour les réglages locaux")
        return Path(home) / ".local/share/devtoolbox"
    raise PathSafetyError(f"plateforme non prise en charge : {platform_name}")


def settings_path(*, platform_name: str, env: Mapping[str, str]) -> Path:
    root = state_root(platform_name=platform_name, env=env)
    if platform_name == "windows":
        return Path(ntpath.join(str(root), "model-settings.json"))
    return root / "model-settings.json"


def default_library_root(*, platform_name: str, env: Mapping[str, str]) -> str:
    root = state_root(platform_name=platform_name, env=env)
    if platform_name == "windows":
        return ntpath.join(str(root), "models")
    return str(root / "models")


def load_settings(*, platform_name: str, env: Mapping[str, str] | None = None) -> ModelSettings:
    environment = os.environ if env is None else env
    path = settings_path(platform_name=platform_name, env=environment)
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return ModelSettings(default_library_root(platform_name=platform_name, env=environment))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"Réglages de modèles illisibles : {exc}") from exc
    if not isinstance(payload, dict) or payload.get("schema_version") != SETTINGS_SCHEMA_VERSION:
        raise ValueError("Version de réglages de modèles non prise en charge")
    root = payload.get("library_root")
    if not isinstance(root, str):
        raise ValueError("library_root est requis")
    normalized = normalize_absolute_path(root, platform_name=platform_name, env=environment)
    provider_order = tuple(payload.get("provider_order", DEFAULT_PROVIDER_ORDER))
    enabled = tuple(payload.get("enabled_providers", DEFAULT_PROVIDER_ORDER))
    keep_patterns = tuple(
        value
        for value in payload.get("keep_patterns", ())
        if isinstance(value, str) and value.strip()
    )
    return ModelSettings(
        normalized,
        provider_order=provider_order,
        enabled_providers=enabled,
        xet_enabled=payload.get("xet_enabled", True),
        keep_patterns=keep_patterns,
    )


def validate_library_root(
    root: str,
    *,
    platform_name: str,
    env: Mapping[str, str] | None = None,
    required_free_bytes: int = 0,
) -> str:
    environment = os.environ if env is None else env
    normalized = normalize_absolute_path(root, platform_name=platform_name, env=environment)
    candidate = Path(normalized)
    existing = candidate
    while not existing.exists() and existing != existing.parent:
        existing = existing.parent
    if not existing.exists() or not existing.is_dir():
        raise ValueError("Aucun parent existant pour la bibliothèque")
    if not os.access(existing, os.W_OK):
        raise ValueError("La bibliothèque n'est pas accessible en écriture")
    if required_free_bytes < 0:
        raise ValueError("L'espace requis ne peut pas être négatif")
    free = shutil.disk_usage(existing).free
    if free < required_free_bytes:
        raise ValueError(f"Espace insuffisant : {free} octets disponibles")
    return normalized


def save_settings(
    settings: ModelSettings,
    *,
    platform_name: str,
    env: Mapping[str, str] | None = None,
) -> Path:
    """Persist only the new path; existing artifacts are never relocated here."""

    environment = os.environ if env is None else env
    normalized = validate_library_root(
        settings.library_root, platform_name=platform_name, env=environment
    )
    path = settings_path(platform_name=platform_name, env=environment)
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = asdict(
        ModelSettings(
            normalized,
            provider_order=settings.provider_order,
            enabled_providers=settings.enabled_providers,
            xet_enabled=settings.xet_enabled,
            keep_patterns=settings.keep_patterns,
        )
    )
    descriptor, temporary = tempfile.mkstemp(prefix="model-settings-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(payload, stream, ensure_ascii=False, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise
    return path
