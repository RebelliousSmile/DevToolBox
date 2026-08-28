"""Bounded model-container validation that never deserializes tensor content."""

from __future__ import annotations

import json
import struct
from pathlib import Path

from .models import ValidationEvidence

MAX_SAFETENSORS_HEADER = 64 * 1024 * 1024


def detect_format(path: str | Path) -> str:
    suffix = Path(path).suffix.lower().lstrip(".")
    return suffix or "opaque"


def validate_model_file(
    path: str | Path, *, format_name: str | None = None, identity_verified: bool = False
) -> ValidationEvidence:
    candidate = Path(path)
    selected = (format_name or detect_format(candidate)).lower()
    try:
        size = candidate.stat().st_size
        if size <= 0:
            return ValidationEvidence(False, "failed", selected, "Le fichier est vide.")
        if selected == "gguf":
            message = _validate_gguf(candidate, size)
            level = "strong" if identity_verified else "structural"
        elif selected == "safetensors":
            message = _validate_safetensors(candidate, size)
            level = "strong" if identity_verified else "structural"
        else:
            message = "Fichier opaque non vide; aucun contenu n'a été désérialisé."
            level = "opaque"
        return ValidationEvidence(True, level, selected, message)
    except (OSError, UnicodeDecodeError, ValueError, json.JSONDecodeError, struct.error) as exc:
        return ValidationEvidence(False, "failed", selected, f"Structure invalide : {exc}")


def _validate_gguf(path: Path, size: int) -> str:
    if size < 24:
        raise ValueError("en-tête GGUF tronqué")
    with path.open("rb") as stream:
        header = stream.read(24)
    magic, version, tensor_count, metadata_count = struct.unpack("<4sIQQ", header)
    if magic != b"GGUF":
        raise ValueError("signature GGUF absente")
    if version not in {1, 2, 3}:
        raise ValueError(f"version GGUF non prise en charge : {version}")
    # Every table entry needs at least one encoded length/value. This conservative
    # lower bound catches impossible counts without walking or loading tensors.
    if tensor_count > size // 8 or metadata_count > size // 8:
        raise ValueError("compteurs GGUF hors limites")
    return f"En-tête GGUF v{version} et compteurs bornés."


def _validate_safetensors(path: Path, size: int) -> str:
    if size < 10:
        raise ValueError("en-tête SafeTensors tronqué")
    with path.open("rb") as stream:
        raw_length = stream.read(8)
        header_length = int.from_bytes(raw_length, "little", signed=False)
        if header_length <= 1 or header_length > MAX_SAFETENSORS_HEADER:
            raise ValueError("taille d'index SafeTensors invalide")
        if 8 + header_length > size:
            raise ValueError("index SafeTensors hors fichier")
        header = stream.read(header_length)
    index = json.loads(header.decode("utf-8"))
    if not isinstance(index, dict):
        raise ValueError("index SafeTensors non objet")
    data_size = size - 8 - header_length
    for name, descriptor in index.items():
        if name == "__metadata__":
            if not isinstance(descriptor, dict):
                raise ValueError("métadonnées SafeTensors invalides")
            continue
        if not isinstance(descriptor, dict):
            raise ValueError("descripteur de tenseur invalide")
        offsets = descriptor.get("data_offsets")
        if (
            not isinstance(offsets, list)
            or len(offsets) != 2
            or any(isinstance(value, bool) or not isinstance(value, int) for value in offsets)
            or offsets[0] < 0
            or offsets[1] < offsets[0]
            or offsets[1] > data_size
        ):
            raise ValueError("bornes de tenseur SafeTensors invalides")
    return "Index SafeTensors et bornes de données valides."
