from __future__ import annotations

import json
import urllib.error
import urllib.request
import unittest
from unittest import mock

from scripts.local_ai import ollama_http


class Response:
    def __init__(self, payload: object = None, status: int = 200, raw: bytes | None = None):
        self.status = status
        self._body = raw if raw is not None else (
            b"" if payload is None else json.dumps(payload).encode("utf-8")
        )

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def read(self) -> bytes:
        return self._body

    def getcode(self) -> int:
        return self.status


class QueueOpener:
    def __init__(self, *responses: object):
        self.responses = list(responses)
        self.requests = []

    def __call__(self, request, *, timeout):
        self.requests.append((request, timeout))
        outcome = self.responses.pop(0)
        if isinstance(outcome, BaseException):
            raise outcome
        return outcome


class OllamaHttpTests(unittest.TestCase):
    def test_loopback_forms_are_canonical(self) -> None:
        self.assertEqual(ollama_http.normalize_endpoint({}), "http://127.0.0.1:11434")
        self.assertEqual(
            ollama_http.normalize_endpoint({"OLLAMA_HOST": "localhost"}),
            "http://localhost:11434",
        )
        self.assertEqual(
            ollama_http.normalize_endpoint({"OLLAMA_HOST": "http://[::1]:9999/"}),
            "http://[::1]:9999",
        )

    def test_unsafe_origins_are_typed_before_network_access(self) -> None:
        cases = {
            "https://localhost:11434": "ollama-endpoint-unsafe",
            "http://192.168.1.2:11434": "ollama-endpoint-remote",
            "http://user@localhost:11434": "ollama-endpoint-invalid",
            "http://localhost:11434/api": "ollama-endpoint-invalid",
            "http://localhost:11434?x=1": "ollama-endpoint-invalid",
            "http://localhost:11434#x": "ollama-endpoint-invalid",
            "http://localhost:": "ollama-endpoint-invalid",
        }
        for origin, code in cases.items():
            with self.subTest(origin=origin), self.assertRaises(ollama_http.OllamaHttpError) as caught:
                ollama_http.normalize_endpoint({"OLLAMA_HOST": origin})
            self.assertEqual(caught.exception.code, code)

    def test_opener_ignores_environment_proxies_and_rejects_redirects(self) -> None:
        with mock.patch.object(
            urllib.request, "getproxies", return_value={"http": "http://remote.invalid:9999"}
        ) as getproxies:
            opener = ollama_http.build_local_opener()
        getproxies.assert_not_called()
        self.assertFalse(
            any(isinstance(handler, urllib.request.ProxyHandler) for handler in opener.handlers)
        )
        handler = ollama_http.RejectRedirects()
        request = urllib.request.Request("http://127.0.0.1:11434/api/tags")
        with self.assertRaises(urllib.error.HTTPError):
            handler.redirect_request(request, None, 302, "Found", {}, "http://remote.invalid")

    def test_request_is_bounded_and_serializes_json(self) -> None:
        opener = QueueOpener(Response({"ok": True}))
        payload = ollama_http.request_json(
            "http://127.0.0.1:11434", "DELETE", "/api/delete",
            payload={"model": "tiny"}, opener=opener,
        )
        self.assertEqual(payload, {"ok": True})
        request, timeout = opener.requests[0]
        self.assertEqual(timeout, ollama_http.REQUEST_TIMEOUT_SECONDS)
        self.assertEqual(json.loads(request.data), {"model": "tiny"})

    def test_http_transport_timeout_and_json_failures_are_stable(self) -> None:
        cases = (
            (Response(status=503), "ollama-http-error"),
            (urllib.error.HTTPError("url", 302, "redirect", {}, None), "ollama-http-error"),
            (urllib.error.URLError("down"), "ollama-transport-error"),
            (TimeoutError("slow"), "ollama-transport-error"),
            (Response(raw=b"not-json"), "ollama-payload-invalid"),
        )
        for outcome, code in cases:
            with self.subTest(code=code), self.assertRaises(ollama_http.OllamaHttpError) as caught:
                ollama_http.request_json(
                    "http://127.0.0.1:11434", "GET", "/api/tags", opener=QueueOpener(outcome)
                )
            self.assertEqual(caught.exception.code, code)


if __name__ == "__main__":
    unittest.main()
