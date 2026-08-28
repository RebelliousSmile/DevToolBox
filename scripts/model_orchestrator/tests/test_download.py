from __future__ import annotations

import hashlib
import io
import json
import struct
import subprocess
import tempfile
import unittest
import urllib.error
import urllib.request
from dataclasses import replace
from pathlib import Path

from scripts.model_orchestrator.download import (
    comparable_groups,
    create_plan,
    execute_plan,
    public_offer,
)
from scripts.model_orchestrator.events import EventStream
from scripts.model_orchestrator.library import NeutralLibrary
from scripts.model_orchestrator.models import AcquisitionOffer, AcquisitionRequest
from scripts.model_orchestrator.providers.direct import (
    DirectProvider,
    ProviderError,
    SafeRedirectHandler,
    validate_direct_url,
)
from scripts.model_orchestrator.providers.huggingface import HuggingFaceProvider


def gguf() -> bytes:
    return struct.pack("<4sIQQ", b"GGUF", 3, 0, 0)


class Response:
    def __init__(self, body: bytes, *, status: int = 200, headers=None):
        self.status = status
        self.headers = headers or {}
        self._stream = io.BytesIO(body)
        self.closed = False

    def read(self, size=-1):
        return self._stream.read(size)

    def getcode(self):
        return self.status

    def close(self):
        self.closed = True

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()


class QueueOpener:
    def __init__(self, *responses):
        self.responses = list(responses)
        self.requests = []

    def __call__(self, request, *, timeout):
        self.requests.append((request, timeout))
        outcome = self.responses.pop(0)
        if isinstance(outcome, BaseException):
            raise outcome
        return outcome


def lines_writer():
    lines = []
    return lines, lines.append


class EventContractTests(unittest.TestCase):
    def test_progress_is_monotonic_redacted_and_has_one_terminal(self) -> None:
        lines, write = lines_writer()
        events = EventStream("op", write)
        events.progress(1, 2)
        events.failed("Échec https://example.test/model?token=secret")
        payloads = [json.loads(line) for line in lines]
        self.assertEqual([row["kind"] for row in payloads], ["schema", "progress", "failed"])
        self.assertNotIn("token", payloads[-1]["message"])
        with self.assertRaises(RuntimeError):
            events.completed("late")

    def test_exact_grouping_never_groups_weak_or_different_offers(self) -> None:
        base = AcquisitionOffer(
            "one", "x", "llm", "revision", "model.gguf", "gguf",
            trusted_digest="a" * 64,
        )
        same = replace(base, provider="two", locator="y")
        weak = replace(base, provider="three", trusted_digest=None)
        different = replace(base, provider="four", trusted_digest="b" * 64)
        sizes = sorted(len(group) for group in comparable_groups((base, same, weak, different)))
        self.assertEqual(sizes, [1, 1, 2])


class DirectProviderTests(unittest.TestCase):
    def test_fresh_https_stream_commits_verified_bytes_and_monotonic_events(self) -> None:
        body = gguf()
        opener = QueueOpener(
            Response(body, headers={"Content-Length": str(len(body)), "ETag": '"v1"'})
        )
        provider = DirectProvider(opener=opener)
        request = AcquisitionRequest(
            "https://example.test/model.gguf?token=secret", "llm"
        )
        offer = provider.resolve(request, request.primary_locator)
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("direct-1", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(provider,),
            )
            self.assertIsNotNone(result.record)
            self.assertEqual(Path(result.record.path).read_bytes(), body)
            self.assertEqual(result.record.origin, "https://example.test/model.gguf")
            self.assertEqual(result.record.identity.state, "verified")
            events = [json.loads(line) for line in lines]
            progress = [row["transferred_bytes"] for row in events if row["kind"] == "progress"]
            self.assertEqual(progress, sorted(progress))
            self.assertEqual(sum(row["kind"] == "completed" for row in events), 1)

    def test_resume_appends_only_with_stable_validator_and_length(self) -> None:
        body = gguf()
        offset = 8
        response = Response(
            body[offset:],
            status=206,
            headers={
                "Content-Range": f"bytes {offset}-{len(body)-1}/{len(body)}",
                "Content-Length": str(len(body) - offset),
                "ETag": '"stable"',
            },
        )
        opener = QueueOpener(response)
        provider = DirectProvider(opener=opener)
        offer = provider.resolve(
            AcquisitionRequest("https://example.test/model.gguf", "llm"),
            "https://example.test/model.gguf",
        )
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            journal = library.begin("resume-1", "model.gguf")
            Path(journal.staging_path).write_bytes(body[:offset])
            (Path(journal.staging_path).parent / "resume.json").write_text(
                json.dumps({"validator": 'etag:"stable"', "total": len(body)})
            )
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("resume-1", offer), library=library, write_event=write,
                providers=(provider,),
            )
            self.assertEqual(Path(result.record.path).read_bytes(), body)
            self.assertEqual(opener.requests[0][0].headers["Range"], f"bytes={offset}-")

    def test_changed_validator_restarts_instead_of_appending(self) -> None:
        old = b"old-data"
        body = gguf()
        opener = QueueOpener(
            Response(
                b"ignored", status=206,
                headers={"Content-Range": f"bytes {len(old)}-{len(body)-1}/{len(body)}", "ETag": '"new"'},
            ),
            Response(body, headers={"Content-Length": str(len(body)), "ETag": '"new"'}),
        )
        provider = DirectProvider(opener=opener)
        offer = provider.resolve(
            AcquisitionRequest("https://example.test/model.gguf", "llm"),
            "https://example.test/model.gguf",
        )
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            journal = library.begin("restart-1", "model.gguf")
            Path(journal.staging_path).write_bytes(old)
            (Path(journal.staging_path).parent / "resume.json").write_text(
                json.dumps({"validator": 'etag:"old"', "total": len(body)})
            )
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("restart-1", offer), library=library, write_event=write,
                providers=(provider,),
            )
            self.assertEqual(Path(result.record.path).read_bytes(), body)
            self.assertEqual(len(opener.requests), 2)
            self.assertNotIn("Range", opener.requests[1][0].headers)

    def test_unsafe_urls_redirects_checksum_and_timeout_never_commit(self) -> None:
        for url in (
            "http://example.test/model.gguf",
            "https://user:secret@example.test/model.gguf",
        ):
            with self.subTest(url=url), self.assertRaises(ProviderError):
                validate_direct_url(url)
        self.assertEqual(
            validate_direct_url("http://127.0.0.1:8000/model.gguf"),
            "http://127.0.0.1:8000/model.gguf",
        )
        handler = SafeRedirectHandler()
        request = urllib.request.Request("https://example.test/model.gguf")
        with self.assertRaises(urllib.error.HTTPError):
            handler.redirect_request(
                request, None, 302, "Found", {}, "http://remote.test/model.gguf"
            )

        body = gguf()
        provider = DirectProvider(
            opener=QueueOpener(Response(body, headers={"Content-Length": str(len(body))}))
        )
        request = AcquisitionRequest(
            "https://example.test/model.gguf", "llm", user_sha256="0" * 64
        )
        offer = provider.resolve(request, request.primary_locator)
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            library = NeutralLibrary(Path(directory) / "library")
            result = execute_plan(
                create_plan("mismatch", offer), library=library, write_event=write,
                providers=(provider,),
            )
            self.assertEqual(result.error_code, "download-checksum-mismatch")
            self.assertEqual(library.list_records(), [])
            self.assertEqual(json.loads(lines[-1])["kind"], "failed")

        timeout_provider = DirectProvider(opener=QueueOpener(TimeoutError("slow")))
        timeout_offer = timeout_provider.resolve(
            AcquisitionRequest("https://example.test/model.gguf", "llm"),
            "https://example.test/model.gguf",
        )
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("timeout", timeout_offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(timeout_provider,),
            )
            self.assertEqual(result.error_code, "download-transport-error")

    def test_cancellation_is_typed_and_terminal(self) -> None:
        body = gguf()
        provider = DirectProvider(
            opener=QueueOpener(
                Response(body, headers={"Content-Length": str(len(body)), "ETag": '"v1"'})
            ),
            cancelled=lambda: True,
        )
        offer = provider.resolve(
            AcquisitionRequest("https://example.test/model.gguf", "llm"),
            "https://example.test/model.gguf",
        )
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("cancelled", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(provider,),
            )
            self.assertEqual(result.error_code, "download-cancelled")
            self.assertEqual(json.loads(lines[-1])["kind"], "cancelled")

    def test_malformed_structured_output_has_a_stable_error(self) -> None:
        provider = DirectProvider(
            opener=QueueOpener(
                Response(b"GGUF", headers={"Content-Length": "4", "ETag": '"bad"'})
            )
        )
        offer = provider.resolve(
            AcquisitionRequest("https://example.test/bad.gguf", "llm"),
            "https://example.test/bad.gguf",
        )
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("malformed", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(provider,),
            )
            self.assertEqual(result.error_code, "download-output-invalid")


class FakeHFRunner:
    def __init__(self, body: bytes):
        self.body = body
        self.calls = []

    def __call__(self, command, env):
        self.calls.append((tuple(command), dict(env)))
        if "--version" in command:
            return subprocess.CompletedProcess(command, 0, "hf 1.2.3", "")
        if "whoami" in command:
            return subprocess.CompletedProcess(command, 0, "developer", "")
        local_dir = Path(command[command.index("--local-dir") + 1])
        remote_filename = command[3]
        output = local_dir / remote_filename
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(self.body)
        revision = command[command.index("--revision") + 1]
        metadata = local_dir / ".cache/huggingface/download" / f"{remote_filename}.metadata"
        metadata.parent.mkdir(parents=True, exist_ok=True)
        metadata.write_text(
            f"{revision}\n{hashlib.sha256(self.body).hexdigest()}\n0\n"
        )
        return subprocess.CompletedProcess(command, 0, str(output), "")


class HuggingFaceProviderTests(unittest.TestCase):
    def test_exact_immutable_xet_download_uses_existing_cli_and_auth(self) -> None:
        body = gguf()
        digest = hashlib.sha256(body).hexdigest()
        runner = FakeHFRunner(body)
        provider = HuggingFaceProvider(
            which=lambda name: "/bin/hf" if name == "hf" else None,
            runner=runner,
            metadata_resolver=lambda repo, revision, filename: {
                "sha256": digest,
                "size": len(body),
            },
            env={"HF_HOME": "/provider-owned"},
        )
        revision = "a" * 40
        locator = f"hf://owner/repo@{revision}/models/tiny.gguf"
        request = AcquisitionRequest(locator, "llm")
        offer = provider.resolve(request, locator)
        self.assertTrue(offer.executable)
        self.assertEqual(provider.status().authenticated, True)
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("hf-1", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(provider,),
            )
            self.assertEqual(result.record.identity.source, "hf-lfs-xet-sha256")
            download_call = next(call for call in runner.calls if "download" in call[0])
            self.assertEqual(download_call[1]["HF_XET_HIGH_PERFORMANCE"], "1")
            self.assertNotIn("token", json.dumps(download_call))

    def test_missing_hf_is_visible_but_never_installed_or_executed(self) -> None:
        calls = []

        def runner(command, env):
            calls.append(command)
            raise AssertionError("runner must not execute")

        provider = HuggingFaceProvider(which=lambda _name: None, runner=runner, env={})
        locator = f"hf://owner/repo@{'a' * 40}/tiny.gguf"
        offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
        self.assertFalse(offer.executable)
        self.assertEqual(provider.status().state, "unavailable")
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("hf-missing", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(provider,),
            )
            self.assertEqual(result.error_code, "offer-not-executable")
            self.assertEqual(calls, [])
            self.assertEqual(json.loads(lines[-1])["kind"], "failed")

    def test_hf_local_metadata_promotes_identity_without_reading_model_again(self) -> None:
        body = gguf()
        runner = FakeHFRunner(body)
        provider = HuggingFaceProvider(
            which=lambda _name: "/bin/hf", runner=runner, env={}
        )
        locator = f"hf://owner/repo@{'b' * 40}/tiny.gguf"
        offer = provider.resolve(AcquisitionRequest(locator, "llm"), locator)
        self.assertIsNone(offer.trusted_digest)
        with tempfile.TemporaryDirectory() as directory:
            lines, write = lines_writer()
            result = execute_plan(
                create_plan("hf-metadata", offer),
                library=NeutralLibrary(Path(directory) / "library"),
                write_event=write,
                providers=(provider,),
            )
            self.assertEqual(
                result.record.identity.value, hashlib.sha256(body).hexdigest()
            )
            self.assertFalse(result.record.hash_pending)

    def test_public_offer_redacts_direct_query(self) -> None:
        offer = DirectProvider().resolve(
            AcquisitionRequest("https://example.test/model.gguf?token=secret", "llm"),
            "https://example.test/model.gguf?token=secret",
        )
        self.assertEqual(public_offer(offer).locator, "https://example.test/model.gguf")


if __name__ == "__main__":
    unittest.main()
