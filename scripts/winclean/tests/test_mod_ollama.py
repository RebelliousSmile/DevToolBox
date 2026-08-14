"""Tests unitaires de l'adaptateur Ollama, sans aucune socket réelle."""

from __future__ import annotations

import json
import urllib.error
import unittest

from scripts.winclean import mod_ollama
from scripts.winclean.common import (
    SKIP_GONE,
    SKIP_RUNNING,
    SKIP_UNATTEMPTED,
    Level,
    ModuleDiscoveryError,
)


class Response:
    def __init__(self, payload: object = None, status: int = 200) -> None:
        self.status = status
        self._body = b"" if payload is None else json.dumps(payload).encode("utf-8")

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def read(self) -> bytes:
        return self._body

    def getcode(self) -> int:
        return self.status


class RawResponse(Response):
    def __init__(self, body: bytes, status: int = 200) -> None:
        self.status = status
        self._body = body


class QueueOpener:
    def __init__(self, *responses: object) -> None:
        self.responses = list(responses)
        self.requests = []

    def __call__(self, request, *, timeout):
        self.requests.append((request, timeout))
        outcome = self.responses.pop(0)
        if isinstance(outcome, BaseException):
            raise outcome
        return outcome


def tags(*models: tuple[str, int]) -> dict:
    return {"models": [{"name": name, "size": size} for name, size in models]}


def running(*names: str) -> dict:
    return {"models": [{"name": name} for name in names]}


class EndpointTest(unittest.TestCase):
    def test_default_and_supported_loopback_forms_are_canonical(self) -> None:
        self.assertEqual(
            mod_ollama.normalise_endpoint({}), "http://127.0.0.1:11434"
        )
        self.assertEqual(
            mod_ollama.normalise_endpoint({"OLLAMA_HOST": "localhost"}),
            "http://localhost:11434",
        )
        self.assertEqual(
            mod_ollama.normalise_endpoint({"OLLAMA_HOST": "http://[::1]:9999/"}),
            "http://[::1]:9999",
        )

    def test_unsafe_endpoint_is_rejected_before_the_opener_is_called(self) -> None:
        unsafe = (
            "https://localhost:11434",
            "http://192.168.1.2:11434",
            "http://user@localhost:11434",
            "http://localhost:11434/api",
            "http://localhost:11434?x=1",
            "http://localhost:11434#x",
            "http://localhost:",
            "ftp://localhost:11434",
        )
        for endpoint in unsafe:
            with self.subTest(endpoint=endpoint):
                opener = QueueOpener()
                with self.assertRaises(ModuleDiscoveryError):
                    mod_ollama.discover_ollama_models(
                        requested_models=("m",),
                        env={"OLLAMA_HOST": endpoint},
                        opener=opener,
                    )
                self.assertEqual(opener.requests, [])


class DiscoveryTest(unittest.TestCase):
    def test_exact_unique_stopped_models_become_pathless_candidates(self) -> None:
        opener = QueueOpener(Response(tags(("a:latest", 12), ("b", 34))), Response(running()))
        found = mod_ollama.discover_ollama_models(
            requested_models=("b", "a:latest", "b"), env={}, opener=opener
        )
        self.assertEqual([c.resource_id for c in found], ["b", "a:latest"])
        self.assertEqual([c.estimated_bytes for c in found], [34, 12])
        self.assertTrue(all(c.path is None for c in found))
        self.assertTrue(all(c.level is Level.AGGRESSIVE and c.no_undo for c in found))
        self.assertTrue(all(c.needs_network for c in found))
        self.assertTrue(
            all(timeout == mod_ollama.REQUEST_TIMEOUT_SECONDS for _request, timeout in opener.requests)
        )

    def test_missing_or_running_model_rejects_the_complete_request(self) -> None:
        for requested, responses, code in (
            (("missing",), (Response(tags(("a", 1))), Response(running())), "ollama-model-missing"),
            (("a",), (Response(tags(("a", 1))), Response(running("a"))), "ollama-model-running"),
        ):
            with self.subTest(code=code), self.assertRaises(ModuleDiscoveryError) as caught:
                mod_ollama.discover_ollama_models(
                    requested_models=requested, env={}, opener=QueueOpener(*responses)
                )
            self.assertEqual(caught.exception.code, code)

    def test_transport_and_invalid_payload_are_typed(self) -> None:
        cases = (
            (urllib.error.URLError("down"), "ollama-transport-error"),
            (Response({}), "ollama-payload-invalid"),
            (RawResponse(b"not-json"), "ollama-payload-invalid"),
            (Response({"models": [{"name": "a", "size": -1}]}), "ollama-payload-invalid"),
            (Response(tags(("a", 1), ("a", 2))), "ollama-payload-duplicate"),
        )
        for response, code in cases:
            with self.subTest(code=code), self.assertRaises(ModuleDiscoveryError) as caught:
                mod_ollama.discover_ollama_models(
                    requested_models=("a",), env={}, opener=QueueOpener(response)
                )
            self.assertEqual(caught.exception.code, code)


class CleanTest(unittest.TestCase):
    def _candidates(self):
        opener = QueueOpener(Response(tags(("a", 10), ("b", 20))), Response(running()))
        return mod_ollama.discover_ollama_models(
            requested_models=("a", "b"), env={}, opener=opener
        )

    def test_success_revalidates_each_model_and_never_claims_freed_bytes(self) -> None:
        candidates = self._candidates()
        opener = QueueOpener(
            Response(tags(("a", 10), ("b", 20))), Response(running()), Response(),
            Response(tags(("b", 20),)), Response(running()), Response(),
        )
        result = mod_ollama.clean_ollama_models(candidates=candidates, env={}, opener=opener)
        self.assertEqual([r.resource_id for r in result.completed_resources], ["a", "b"])
        deletes = [request for request, _timeout in opener.requests if request.method == "DELETE"]
        self.assertEqual([json.loads(request.data)["model"] for request in deletes], ["a", "b"])
        self.assertIsNone(result.freed)
        self.assertIsNone(result.recycled)
        self.assertIsNone(result.failed)
        self.assertIsNone(result.measured)

    def test_disappeared_and_newly_running_models_are_skipped(self) -> None:
        candidates = self._candidates()
        opener = QueueOpener(
            Response(tags(("b", 20),)), Response(running()),
            Response(tags(("b", 20),)), Response(running("b")),
        )
        result = mod_ollama.clean_ollama_models(candidates=candidates, env={}, opener=opener)
        self.assertEqual([s.status for s in result.skipped], [SKIP_GONE, SKIP_RUNNING])
        self.assertFalse(any(request.method == "DELETE" for request, _ in opener.requests))

    def test_first_delete_failure_stops_and_marks_remainder_unattempted(self) -> None:
        candidates = self._candidates()
        opener = QueueOpener(
            Response(tags(("a", 10), ("b", 20))), Response(running()),
            Response(status=500),
        )
        result = mod_ollama.clean_ollama_models(candidates=candidates, env={}, opener=opener)
        self.assertEqual([f.resource_id for f in result.operation_failures], ["a"])
        self.assertEqual([s.status for s in result.skipped], [SKIP_UNATTEMPTED])
        self.assertEqual(len([r for r, _ in opener.requests if r.method == "DELETE"]), 1)


if __name__ == "__main__":
    unittest.main()
