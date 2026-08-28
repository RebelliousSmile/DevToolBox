"""Acquisition provider protocol and built-in registry."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Protocol

from ..events import EventStream
from ..library import NeutralLibrary
from ..models import AcquisitionOffer, AcquisitionRequest, LibraryRecord, ProviderStatus


class AcquisitionProvider(Protocol):
    name: str

    def accepts(self, locator: str) -> bool: ...

    def status(self) -> ProviderStatus: ...

    def resolve(self, request: AcquisitionRequest, locator: str) -> AcquisitionOffer: ...

    def download(
        self,
        offer: AcquisitionOffer,
        *,
        operation_id: str,
        library: NeutralLibrary,
        events: EventStream,
    ) -> LibraryRecord: ...


def builtin_providers(*, enabled=None, order=None, xet_enabled: bool = True):
    from .direct import DirectProvider
    from .huggingface import HuggingFaceProvider
    from .lm_studio import LMStudioProvider
    from .ollama import OllamaProvider

    cancel_path = os.environ.get("DEVTOOLBOX_MODEL_CANCEL_FILE")
    cancelled = (lambda: Path(cancel_path).is_file()) if cancel_path else (lambda: False)
    providers = {
        "huggingface": HuggingFaceProvider(cancelled=cancelled, high_performance=xet_enabled),
        "direct": DirectProvider(cancelled=cancelled),
        "ollama": OllamaProvider(cancelled=cancelled),
        "lm-studio": LMStudioProvider(cancelled=cancelled),
    }
    selected_order = tuple(order or ("ollama", "huggingface", "lm-studio", "direct"))
    selected_enabled = set(providers) if enabled is None else set(enabled)
    return tuple(
        providers[name]
        for name in selected_order
        if name in providers and name in selected_enabled
    )
