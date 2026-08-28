from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.model_orchestrator.adapters import AdapterContext
from scripts.model_orchestrator.adapters.comfyui import ComfyUIAdapter
from scripts.model_orchestrator.adapters.jan import JanAdapter
from scripts.model_orchestrator.adapters.lm_studio import LMStudioAdapter
from scripts.model_orchestrator.adapters.ollama import OllamaAdapter
from scripts.model_orchestrator.catalog import inventory_snapshot


class Response:
    def __init__(self, payload: object):
        self.status = 200
        self.body = json.dumps(payload).encode()

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def read(self):
        return self.body


class QueueOpener:
    def __init__(self, *payloads: object):
        self.payloads = list(payloads)

    def __call__(self, _request, *, timeout):
        assert timeout == 5.0
        return Response(self.payloads.pop(0))


class FakeRunner:
    def __init__(self, models: object, loaded: object):
        self.models = models
        self.loaded = loaded

    def __call__(self, command):
        if "--version" in command:
            return subprocess.CompletedProcess(command, 0, "lms 0.3\n", "")
        payload = self.loaded if "ps" in command else self.models
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")


class AdapterInventoryTests(unittest.TestCase):
    def test_ollama_recognizes_one_shared_blob_and_loaded_reference(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            digest = "a" * 64
            blob = root / "blobs" / f"sha256-{digest}"
            blob.parent.mkdir()
            blob.write_bytes(b"gguf")
            manifest_payload = {
                "layers": [{
                    "mediaType": "application/vnd.ollama.image.model",
                    "digest": f"sha256:{digest}",
                }]
            }
            for name in ("tiny", "tiny-alias"):
                manifest = root / "manifests/registry.ollama.ai/library" / name / "latest"
                manifest.parent.mkdir(parents=True)
                manifest.write_text(json.dumps(manifest_payload))
            context = AdapterContext(
                env={"OLLAMA_MODELS": str(root)},
                which=lambda _name: "/bin/ollama",
                run=lambda command: subprocess.CompletedProcess(command, 0, "ollama 1\n", ""),
                ollama_opener=QueueOpener(
                    {"models": [{"name": "tiny:latest", "size": 4}]},
                    {"models": [{"name": "tiny:latest", "size": 4}]},
                ),
            )
            observation = OllamaAdapter().inventory(context)
            self.assertEqual(len(observation.artifacts), 1)
            self.assertEqual(observation.artifacts[0].identity.exact_key, f"sha256:{digest}")
            self.assertEqual(len(observation.artifacts[0].references), 2)
            self.assertTrue(any(reference.loaded for reference in observation.artifacts[0].references))
            self.assertEqual(observation.installation.root_evidence[0].confidence, "high")

    def test_jan_distinguishes_symbolic_link_from_duplicated_gguf(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.gguf"
            source.write_bytes(b"source")
            data = root / "jan data/models"
            data.mkdir(parents=True)
            (data / "copy.gguf").write_bytes(b"copy")
            (data / "linked.gguf").symlink_to(source)
            context = AdapterContext(env={"JAN_DATA_FOLDER": str(data)}, home=root, which=lambda _: None)
            observation = JanAdapter().inventory(context)
            self.assertEqual(
                {artifact.relationship for artifact in observation.artifacts},
                {"copy", "symbolic_link"},
            )
            self.assertFalse(observation.installation.capabilities.native_delete)

    def test_lm_studio_prefers_cli_and_marks_loaded_model(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            model = Path(directory) / "model.gguf"
            model.write_bytes(b"model")
            context = AdapterContext(
                env={"LM_STUDIO_MODELS_DIR": directory},
                which=lambda name: "/bin/lms" if name == "lms" else None,
                run=FakeRunner(
                    [{"modelKey": "publisher/model", "path": str(model)}],
                    [{"modelKey": "publisher/model"}],
                ),
            )
            observation = LMStudioAdapter().inventory(context)
            self.assertEqual(len(observation.artifacts), 1)
            self.assertTrue(observation.artifacts[0].references[0].loaded)
            self.assertEqual(observation.installation.version, "lms 0.3")

    def test_comfyui_combines_extra_roots_api_categories_and_workflows(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            models = base / "ComfyUI/models"
            checkpoint = models / "checkpoints/hero.safetensors"
            checkpoint.parent.mkdir(parents=True)
            checkpoint.write_bytes(b"image")
            extra = base / "shared/loras"
            extra.mkdir(parents=True)
            (extra / "style.safetensors").write_bytes(b"lora")
            config = base / "extra_model_paths.yaml"
            config.write_text(f"shared:\n  base_path: {base / 'shared'}\n  loras: loras\n")
            workflows = base / "workflows"
            workflows.mkdir()
            (workflows / "portrait.json").write_text(json.dumps({"inputs": {"ckpt_name": "hero.safetensors"}}))

            def api(path: str):
                if path == "/models":
                    return ["checkpoints", "loras", "ignored"]
                return ["hero.safetensors"] if path.endswith("checkpoints") else ["style.safetensors"]

            context = AdapterContext(
                env={
                    "COMFYUI_MODELS_DIR": str(models),
                    "COMFYUI_EXTRA_MODEL_PATHS": str(config),
                    "COMFYUI_REGISTERED_MODEL_ROOTS": str(extra),
                    "COMFYUI_WORKFLOWS_DIR": str(workflows),
                },
                home=base,
                which=lambda _: None,
                comfy_request=api,
            )
            observation = ComfyUIAdapter().inventory(context)
            self.assertEqual({artifact.category for artifact in observation.artifacts}, {"checkpoint", "lora"})
            hero = next(item for item in observation.artifacts if item.path.endswith("hero.safetensors"))
            self.assertTrue(any(reference.workflow for reference in hero.references))
            self.assertTrue(any(reference.kind == "live-catalog" for reference in hero.references))

    def test_platform_defaults_are_recorded_with_source_and_confidence(self) -> None:
        linux = AdapterContext(platform_name="linux", env={}, home=Path("/home/dev"), which=lambda _: None)
        windows = AdapterContext(platform_name="windows", env={}, home=Path("C:/Users/dev"), which=lambda _: None)
        self.assertEqual(len(JanAdapter()._roots(linux)), 2)
        self.assertIn(".ollama", str(OllamaAdapter()._root(windows)[0]))
        self.assertEqual(LMStudioAdapter()._root(linux)[1:], ("documented-default", "medium"))

    def test_malformed_jan_settings_and_remote_comfy_api_are_typed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings = root / "settings.json"
            settings.write_text("not-json")
            jan = JanAdapter().inventory(
                AdapterContext(
                    env={"JAN_SETTINGS": str(settings)}, home=root, which=lambda _: None
                )
            )
            self.assertIn("settings-invalid", {error.code for error in jan.errors})
            comfy = ComfyUIAdapter().inventory(
                AdapterContext(
                    env={
                        "COMFYUI_MODELS_DIR": str(root / "models"),
                        "COMFYUI_HOST": "http://remote.invalid:8188",
                    },
                    home=root,
                    which=lambda _: None,
                )
            )
            self.assertIn("comfyui-api-unavailable", {error.code for error in comfy.errors})
            self.assertFalse(comfy.installation.capabilities.native_delete)

    def test_one_adapter_exception_preserves_other_results(self) -> None:
        class Broken:
            name = "broken"

            def inventory(self, _context):
                raise RuntimeError("boom")

        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            (data / "ok.gguf").write_bytes(b"ok")
            context = AdapterContext(env={"JAN_DATA_FOLDER": directory}, home=data, which=lambda _: None)
            snapshot = inventory_snapshot(
                context=context, adapters=(Broken(), JanAdapter()), generated_at="fixed"
            )
            self.assertEqual(len(snapshot.artifacts), 1)
            self.assertEqual(snapshot.source_errors[0].code, "adapter-failed")
            self.assertFalse(snapshot.installations[0].capabilities.native_delete)


if __name__ == "__main__":
    unittest.main()
