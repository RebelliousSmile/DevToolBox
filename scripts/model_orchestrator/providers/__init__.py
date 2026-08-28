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


def builtin_providers():
    from .direct import DirectProvider
    from .huggingface import HuggingFaceProvider
    from .lm_studio import LMStudioProvider
    from .ollama import OllamaProvider

    cancel_path = os.environ.get("DEVTOOLBOX_MODEL_CANCEL_FILE")
    cancelled = (lambda: Path(cancel_path).is_file()) if cancel_path else (lambda: False)
    return (
        HuggingFaceProvider(cancelled=cancelled),
        DirectProvider(cancelled=cancelled),
        OllamaProvider(cancelled=cancelled),
        LMStudioProvider(cancelled=cancelled),
    )
