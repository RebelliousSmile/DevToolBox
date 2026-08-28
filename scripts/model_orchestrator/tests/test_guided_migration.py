from __future__ import annotations

import json
import io
import contextlib
import os
import struct
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.model_orchestrator.adapters import AdapterContext, AdapterObservation
from scripts.model_orchestrator.adapters.comfyui import (
    ComfyUIGuidedIntegration,
    DevToolBoxComfyLaunchBackend,
)
from scripts.model_orchestrator.adapters.jan import JanAdapter, JanGuidedIntegration
from scripts.model_orchestrator.catalog import canonical_artifacts
from scripts.model_orchestrator.library import NeutralLibrary
from scripts.model_orchestrator.migration import GuidedMigrationStore
from scripts.model_orchestrator.models import (
    Artifact,
    ArtifactIdentity,
    ToolInstallation,
    ToolReference,
)
from scripts.model_orchestrator.__main__ import main
from scripts.model_orchestrator.settings import ModelSettings, save_settings


def gguf() -> bytes:
    return struct.pack("<4sIQQ", b"GGUF", 3, 0, 0)


def safetensors() -> bytes:
    header = json.dumps(
        {"weight": {"dtype": "F32", "shape": [1], "data_offsets": [0, 4]}}
    ).encode()
    return len(header).to_bytes(8, "little") + header + b"data"


def installation(tool: str) -> ToolInstallation:
    return ToolInstallation(tool, True, version="1", confidence="high")


class FakeHook:
    def __init__(self, hook=None):
        self.hook = hook
        self.registered = []
        self.unregistered = []

    def supported_hook(self):
        return self.hook

    def register(self, config_path, hook):
        self.registered.append((config_path, hook))

    def unregister(self, config_path, hook):
        self.unregistered.append((config_path, hook))


class JanGuidedTests(unittest.TestCase):
    def test_high_level_cli_starts_from_an_exact_library_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            library = root / "library"
            source = NeutralLibrary(library).commit_stream(
                "cli-source", "model.gguf", (gguf(),), family="llm"
            )
            env = {"HOME": str(root), "XDG_DATA_HOME": str(root / "state")}
            save_settings(ModelSettings(str(library)), platform_name="linux", env=env)
            output = io.StringIO()
            with mock.patch.dict(os.environ, env, clear=False), contextlib.redirect_stdout(output):
                code = main(
                    [
                        "guided-start",
                        "--artifact-id",
                        f"library:{source.artifact_id}",
                        "--destination",
                        "jan",
                        "--migration-id",
                        "cli-guided",
                    ]
                )
            self.assertEqual(code, 0)
            payload = json.loads(output.getvalue())
            self.assertEqual(payload["state"], "pending-manual")
            self.assertEqual(payload["source_artifact_id"], source.artifact_id)

    def test_manual_link_checkpoint_resumes_only_after_exact_observation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = NeutralLibrary(root / "library").commit_stream(
                "jan-source", "model.gguf", (gguf(),), family="llm"
            )
            store = GuidedMigrationStore(root / "guided")
            migration = store.create(
                migration_id="jan-guided", source=source, destination_tool="jan"
            )
            integration = JanGuidedIntegration(store)
            integration.prepare(migration)
            self.assertEqual(migration.state, "pending-manual")
            self.assertIn("Link Files", migration.manual_step.documented_action)
            self.assertTrue(Path(source.path).is_file())

            empty = AdapterObservation(installation("jan"))
            integration.resume(migration, empty)
            self.assertEqual(migration.state, "pending-manual")

            jan_root = root / "jan/models"
            jan_root.mkdir(parents=True)
            copied = jan_root / "copied.gguf"
            copied.write_bytes(Path(source.path).read_bytes())
            copied_observation = JanAdapter().inventory(
                AdapterContext(
                    env={"JAN_DATA_FOLDER": str(jan_root)}, home=root, which=lambda _: None
                )
            )
            integration.resume(migration, copied_observation)
            self.assertEqual(migration.state, "pending-manual")

            copied.unlink()
            linked = jan_root / "linked.gguf"
            linked.symlink_to(source.path)
            linked_observation = JanAdapter().inventory(
                AdapterContext(
                    env={"JAN_DATA_FOLDER": str(jan_root)}, home=root, which=lambda _: None
                )
            )
            integration.resume(migration, linked_observation)
            self.assertEqual(migration.state, "completed-weak")
            self.assertEqual(migration.validation.load, "unavailable")
            self.assertFalse(migration.retirement_eligible)
            self.assertTrue(Path(source.path).is_file())


class ComfyGuidedTests(unittest.TestCase):
    def _source(self, root: Path):
        return NeutralLibrary(root / "library").commit_stream(
            "image-source", "hero.safetensors", (safetensors(),), family="image"
        )

    def _observation(self, source, *, live: bool, workflow: bool = False) -> AdapterObservation:
        artifact = canonical_artifacts([source])[0]
        artifact.category = "checkpoint"
        artifact.references = [
            ToolReference(
                "comfyui",
                "hero.safetensors",
                kind="live-catalog" if live else "catalog",
                owner=True,
            )
        ]
        if workflow:
            artifact.references.append(
                ToolReference("comfyui", "portrait.json", kind="workflow", workflow=True)
            )
        return AdapterObservation(installation("comfyui"), artifacts=[artifact])

    def test_existing_live_shared_root_needs_no_configuration_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self._source(root)
            store = GuidedMigrationStore(root / "guided")
            migration = store.create(
                migration_id="comfy-existing",
                source=source,
                destination_tool="comfyui",
                category="checkpoint",
            )
            hook = FakeHook("desktop-setting")
            integration = ComfyUIGuidedIntegration(store, root / "owned", hook)
            integration.prepare(migration, self._observation(source, live=True, workflow=True))
            self.assertEqual(migration.state, "completed-weak")
            self.assertIsNone(migration.owned_config_path)
            self.assertEqual(hook.registered, [])
            self.assertEqual(migration.validation.workflow, "passed")
            self.assertFalse(migration.retirement_eligible)

    def test_owned_yaml_uses_supported_hook_and_rollback_removes_only_owned_state(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self._source(root)
            user_yaml = root / "extra_model_paths.yaml"
            user_yaml.write_text("user: untouched\n")
            store = GuidedMigrationStore(root / "guided")
            migration = store.create(
                migration_id="comfy-hook",
                source=source,
                destination_tool="comfyui",
                category="checkpoint",
            )
            hook = FakeHook("launch-arg")
            integration = ComfyUIGuidedIntegration(store, root / "owned", hook)
            integration.prepare(migration, AdapterObservation(installation("comfyui")))
            config = Path(migration.owned_config_path)
            self.assertTrue(config.is_file())
            self.assertIn(str(Path(source.path).parent), config.read_text())
            self.assertEqual(user_yaml.read_text(), "user: untouched\n")
            self.assertEqual(hook.registered, [(str(config), "launch-arg")])
            self.assertEqual(migration.state, "pending-validation")

            integration.resume(migration, self._observation(source, live=False))
            self.assertEqual(migration.state, "pending-validation")
            integration.resume(migration, self._observation(source, live=True))
            self.assertEqual(migration.state, "completed-weak")
            self.assertEqual(migration.validation.inference, "unavailable")
            self.assertEqual(migration.validation.workflow, "none")
            integration.rollback(migration)
            self.assertFalse(config.exists())
            self.assertEqual(hook.unregistered, [(str(config), "launch-arg")])
            self.assertEqual(user_yaml.read_text(), "user: untouched\n")

    def test_missing_hook_becomes_manual_and_replaced_owned_file_survives_rollback(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self._source(root)
            store = GuidedMigrationStore(root / "guided")
            migration = store.create(
                migration_id="comfy-manual",
                source=source,
                destination_tool="comfyui",
                category="checkpoint",
            )
            integration = ComfyUIGuidedIntegration(store, root / "owned", FakeHook())
            integration.prepare(migration, AdapterObservation(installation("comfyui")))
            self.assertEqual(migration.state, "pending-manual")
            self.assertIn("--extra-model-paths-config", migration.manual_step.documented_action)
            config = Path(migration.owned_config_path)
            config.unlink()
            config.write_text("replacement: true\n")
            integration.rollback(migration)
            self.assertEqual(migration.state, "rollback-incomplete")
            self.assertEqual(config.read_text(), "replacement: true\n")

    def test_documented_launch_hook_is_registered_only_in_owned_registry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "devtoolbox-launch.json"
            backend = DevToolBoxComfyLaunchBackend(registry, flag_supported=True)
            backend.register("/owned/one.yaml", "launch-arg")
            backend.register("/owned/one.yaml", "launch-arg")
            self.assertEqual(
                json.loads(registry.read_text())["extra_model_paths_configs"],
                ["/owned/one.yaml"],
            )
            backend.unregister("/owned/one.yaml", "launch-arg")
            self.assertEqual(
                json.loads(registry.read_text())["extra_model_paths_configs"], []
            )


if __name__ == "__main__":
    unittest.main()
