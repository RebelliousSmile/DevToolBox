from __future__ import annotations

import hashlib
import io
import json
import os
import struct
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from scripts.model_orchestrator.download import create_plan, execute_plan
from scripts.model_orchestrator.events import ChildResult, NativeChildRunner
from scripts.model_orchestrator.library import NeutralLibrary
from scripts.model_orchestrator.models import AcquisitionRequest
from scripts.model_orchestrator.providers.lm_studio import LMStudioProvider
from scripts.model_orchestrator.providers.ollama import OllamaProvider


def gguf() -> bytes:
    return struct.pack("<4sIQQ", b"GGUF", 3, 0, 0)


class PullResponse:
    status = 200

    def __init__(self, rows):
        self._stream = io.BytesIO(
            b"".join(json.dumps(row).encode() + b"\n" for row in rows)
        )

    def getcode(self):
        return self.status

    def readline(self):
        return self._stream.readline()

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False


class PullOpener:
    def __init__(self, response):
        self.response = response
        self.requests = []

    def __call__(self, request, *, timeout):
        self.requests.append((request, timeout))
        return self.response


def install_ollama_blob(root: Path, model: str = "tiny", tag: str = "q4"):
    body = gguf()
    digest = hashlib.sha256(body).hexdigest()
    blob = root / "blobs" / f"sha256-{digest}"
    blob.parent.mkdir(parents=True)
    blob.write_bytes(body)
    manifest = root / "manifests/registry.ollama.ai/library" / model / tag
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        json.dumps(
            {
                "layers": [
                    {
                        "mediaType": "application/vnd.ollama.image.model",
                        "digest": f"sha256:{digest}",
                    }
                ]
            }
        )
    )
    return blob, digest


class OllamaProviderTests(unittest.TestCase):
    def test_native_pull_exports_verified_blob_by_hardlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "ollama"
            blob, digest = install_ollama_blob(root)
            opener = PullOpener(
                PullResponse(
                    [
                        {"status": "pulling", "completed": 4, "total": 24},
                        {"status": "success", "completed": 24, "total": 24},
                    ]
                )
            )
            provider = OllamaProvider(
                env={"OLLAMA_MODELS": str(root)}, opener=opener
            )
            locator = "ollama://tiny:q4"
            offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
            self.assertTrue(offer.executable)
            lines = []
            library = NeutralLibrary(Path(directory) / "library")
            result = execute_plan(
                create_plan("ollama-1", offer),
                library=library,
                write_event=lines.append,
                providers=(provider,),
            )
            self.assertEqual(result.record.identity.value, digest)
            self.assertEqual(result.record.relationship, "hard_link")
            self.assertEqual(blob.stat().st_ino, Path(result.record.path).stat().st_ino)
            self.assertTrue(blob.is_file())
            self.assertFalse(offer.retirement_supported)
            self.assertEqual(json.loads(lines[-1])["kind"], "completed")

    def test_cross_volume_export_copies_without_claiming_source_freed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "ollama"
            blob, _digest = install_ollama_blob(root)
            provider = OllamaProvider(
                env={"OLLAMA_MODELS": str(root)},
                opener=PullOpener(PullResponse([{"status": "success"}])),
            )
            locator = "ollama://tiny:q4"
            offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
            library = NeutralLibrary(Path(directory) / "library")
            with mock.patch(
                "scripts.model_orchestrator.library.same_filesystem",
                side_effect=(True, False),
            ):
                result = execute_plan(
                    create_plan("ollama-copy", offer),
                    library=library,
                    write_event=lambda _line: None,
                    providers=(provider,),
                )
            self.assertEqual(result.record.relationship, "copy")
            self.assertNotEqual(blob.stat().st_ino, Path(result.record.path).stat().st_ino)
            self.assertTrue(blob.is_file())

    def test_unknown_layout_or_remote_endpoint_disables_pull_before_store_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "unknown"
            root.mkdir()
            opener = PullOpener(PullResponse([]))
            provider = OllamaProvider(
                env={"OLLAMA_MODELS": str(root)}, opener=opener
            )
            locator = "ollama://tiny:q4"
            offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
            self.assertFalse(offer.executable)
            result = execute_plan(
                create_plan("disabled", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=lambda _line: None,
                providers=(provider,),
            )
            self.assertEqual(result.error_code, "offer-not-executable")
            self.assertEqual(opener.requests, [])
            self.assertEqual(list(root.iterdir()), [])

            (root / "manifests").mkdir()
            (root / "blobs").mkdir()
            remote = OllamaProvider(
                env={
                    "OLLAMA_MODELS": str(root),
                    "OLLAMA_HOST": "http://remote.invalid:11434",
                },
                opener=opener,
            )
            self.assertFalse(
                remote.resolve(AcquisitionRequest(locator, "llm"), locator).executable
            )

    def test_cancelled_pull_keeps_truthful_resumable_journal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "ollama"
            install_ollama_blob(root)
            provider = OllamaProvider(
                env={"OLLAMA_MODELS": str(root)},
                opener=PullOpener(PullResponse([{"status": "pulling"}])),
                cancelled=lambda: True,
            )
            locator = "ollama://tiny:q4"
            offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
            library = NeutralLibrary(Path(directory) / "library")
            lines = []
            result = execute_plan(
                create_plan("ollama-cancel", offer),
                library=library,
                write_event=lines.append,
                providers=(provider,),
            )
            self.assertEqual(result.error_code, "download-cancelled")
            self.assertEqual(library.load_journal("ollama-cancel").state, "resumable")
            self.assertEqual(json.loads(lines[-1])["kind"], "cancelled")


class FakeChildRunner:
    def __init__(self, result: ChildResult, lines=()):
        self.result = result
        self.lines = lines
        self.calls = []

    def run(self, command, **kwargs):
        self.calls.append((command, kwargs))
        for line in self.lines:
            kwargs["on_stdout"](line)
        return self.result


class LMSListRunner:
    def __init__(self, rows):
        self.rows = rows

    def __call__(self, command, env):
        if "--version" in command:
            return subprocess.CompletedProcess(command, 0, "lms 0.4", "")
        return subprocess.CompletedProcess(command, 0, json.dumps(self.rows), "")


class LMStudioProviderTests(unittest.TestCase):
    def test_native_get_exports_only_exact_owned_verified_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "lm-models"
            model = root / "publisher/model/tiny.gguf"
            model.parent.mkdir(parents=True)
            model.write_bytes(gguf())
            digest = hashlib.sha256(model.read_bytes()).hexdigest()
            child = FakeChildRunner(
                ChildResult(0, (), ()),
                lines=(json.dumps({"downloadedBytes": 24, "totalBytes": 24}),),
            )
            provider = LMStudioProvider(
                which=lambda _name: "/bin/lms",
                child_runner=child,
                list_runner=LMSListRunner(
                    [{"modelKey": "publisher/model/tiny.gguf", "path": str(model), "sha256": digest}]
                ),
                env={"LM_STUDIO_MODELS_DIR": str(root)},
                home=Path(directory),
            )
            locator = "lmstudio://publisher/model/tiny.gguf"
            offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
            result = execute_plan(
                create_plan("lms-1", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=lambda _line: None,
                providers=(provider,),
            )
            self.assertEqual(result.record.identity.value, digest)
            self.assertEqual(result.record.relationship, "hard_link")
            self.assertFalse(offer.retirement_supported)
            self.assertIn("get", child.calls[0][0])

    def test_hidden_path_or_missing_identity_stays_tool_owned(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "owned"
            root.mkdir()
            outside = Path(directory) / "outside.gguf"
            outside.write_bytes(gguf())
            child = FakeChildRunner(ChildResult(0, (), ()))
            locator = "lmstudio://publisher/model/tiny.gguf"
            for row, code in (
                (
                    {"modelKey": "publisher/model/tiny.gguf", "path": str(outside), "sha256": "a" * 64},
                    "lms-export-path-unsafe",
                ),
                (
                    {"modelKey": "publisher/model/tiny.gguf", "path": str(root / "tiny.gguf")},
                    "lms-export-identity-unknown",
                ),
            ):
                if str(row["path"]).startswith(str(root)):
                    Path(row["path"]).write_bytes(gguf())
                provider = LMStudioProvider(
                    which=lambda _name: "/bin/lms",
                    child_runner=child,
                    list_runner=LMSListRunner([row]),
                    env={"LM_STUDIO_MODELS_DIR": str(root)},
                )
                offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
                library = NeutralLibrary(Path(directory) / f"library-{code}")
                result = execute_plan(
                    create_plan(code, offer), library=library,
                    write_event=lambda _line: None, providers=(provider,),
                )
                self.assertEqual(result.error_code, code)
                self.assertEqual(library.list_records(), [])
                self.assertEqual(library.load_journal(code).state, "manual-attention")
                self.assertTrue(Path(row["path"]).is_file())

    def test_cancel_and_timeout_are_distinct_resumable_states(self) -> None:
        locator = "lmstudio://publisher/model/tiny.gguf"
        for child_result, code in (
            (ChildResult(-15, (), (), cancelled=True), "download-cancelled"),
            (ChildResult(-9, (), (), timed_out=True), "download-timeout"),
        ):
            with tempfile.TemporaryDirectory() as directory:
                provider = LMStudioProvider(
                    which=lambda _name: "/bin/lms",
                    child_runner=FakeChildRunner(child_result),
                    list_runner=LMSListRunner([]),
                    env={"LM_STUDIO_MODELS_DIR": str(Path(directory) / "models")},
                )
                offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
                library = NeutralLibrary(Path(directory) / "library")
                result = execute_plan(
                    create_plan("native-stop", offer), library=library,
                    write_event=lambda _line: None, providers=(provider,),
                )
                self.assertEqual(result.error_code, code)
                self.assertEqual(library.load_journal("native-stop").state, "resumable")


@unittest.skipIf(sys.platform == "win32", "assertion de groupe de processus POSIX")
class NativeChildLifecycleTests(unittest.TestCase):
    def test_cancellation_terminates_the_complete_process_group(self) -> None:
        pids = []
        script = (
            "import os,subprocess,sys,time; "
            "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']); "
            "print(str(os.getpid())+','+str(child.pid),flush=True); time.sleep(30)"
        )
        runner = NativeChildRunner()
        result = runner.run(
            (sys.executable, "-c", script),
            env=dict(os.environ),
            on_stdout=lambda line: pids.extend(int(value) for value in line.split(",")),
            cancelled=lambda: bool(pids),
            timeout_seconds=5,
        )
        self.assertTrue(result.cancelled)
        self.assertEqual(len(pids), 2)
        with self.assertRaises(ProcessLookupError):
            os.killpg(pids[0], 0)


if __name__ == "__main__":
    unittest.main()
