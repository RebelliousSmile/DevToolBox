from __future__ import annotations

import json
import os
import struct
import tempfile
import unittest
from dataclasses import asdict
from pathlib import Path

from scripts.model_orchestrator.adapters.lm_studio import LMStudioMigrationDriver
from scripts.model_orchestrator.adapters.ollama import OllamaMigrationDriver
from scripts.model_orchestrator.library import NeutralLibrary
from scripts.model_orchestrator.__main__ import main
from scripts.model_orchestrator.migration import (
    MigrationError,
    MigrationExecutor,
    create_migration_plan,
)
from scripts.model_orchestrator.models import (
    AdapterCapabilities,
    ArtifactIdentity,
    LibraryRecord,
    ToolInstallation,
)


def gguf() -> bytes:
    return struct.pack("<4sIQQ", b"GGUF", 3, 0, 0)


def canonical(root: Path) -> LibraryRecord:
    return NeutralLibrary(root / "library").commit_stream(
        "source",
        "source.gguf",
        (gguf(),),
        family="llm",
        origin="fixture",
    )


def installation(tool: str, root: Path, capabilities: AdapterCapabilities):
    return ToolInstallation(
        tool=tool,
        detected=True,
        version="1.0",
        roots=(str(root),),
        discovery_source="fixture",
        confidence="high",
        capabilities=capabilities,
    )


class FakeOllamaBackend:
    def __init__(self, *, blob_exists=False, model_exists=False, infer=True):
        self.blob = blob_exists
        self.model = model_exists
        self.inference = infer
        self.digest = None
        self.uploads = []
        self.created = []
        self.deleted = []

    def model_exists(self, native_id):
        return self.model

    def blob_exists(self, digest):
        return self.blob

    def upload_blob(self, source, digest):
        self.uploads.append((source, digest))
        self.blob = True

    def create_model(self, native_id, digest):
        self.created.append(native_id)
        self.model = True
        self.digest = digest

    def model_digest(self, native_id):
        return self.digest if self.model else None

    def infer(self, native_id):
        return self.inference

    def delete_model(self, native_id):
        self.deleted.append(native_id)
        self.model = False


class FakeLMSBackend:
    def __init__(self, target: Path, native_id: str, digest: str, *, load=True, infer=True):
        self.target = target
        self.native_id = native_id
        self.digest = digest
        self.inference = infer
        self.loaded = load
        self.unloaded = []
        self.import_calls = 0

    def dry_run_import(self, source, method):
        return str(self.target), self.native_id

    def import_model(self, source, method):
        self.import_calls += 1
        self.target.parent.mkdir(parents=True, exist_ok=True)
        if method == "hard_link":
            os.link(source, self.target)
        elif method == "symbolic_link":
            self.target.symlink_to(source)
        else:
            self.target.write_bytes(Path(source).read_bytes())
        return str(self.target), self.native_id

    def listed_digest(self, native_id):
        return self.digest

    def load(self, native_id):
        return self.loaded

    def infer(self, native_id):
        return self.inference

    def unload(self, native_id):
        self.unloaded.append(native_id)


class MigrationPlanningTests(unittest.TestCase):
    def test_plan_freezes_identity_destination_costs_and_preferred_method(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "lm-models"
            destination_root.mkdir()
            target = destination_root / "owner/model/source.gguf"
            destination = installation(
                "lm-studio",
                destination_root,
                AdapterCapabilities(
                    hard_link=True,
                    symbolic_link=True,
                    copy=True,
                    native_import=True,
                    load_validation=True,
                    inference_validation=True,
                ),
            )
            plan = create_migration_plan(
                plan_id="plan-1",
                source=source,
                destination=destination,
                destination_root=str(destination_root),
                destination_native_id="owner/model/source.gguf",
                target_path=str(target),
            )
            self.assertEqual(plan.method, "hard_link")
            self.assertEqual(plan.source_sha256, source.identity.value)
            self.assertEqual(plan.allocated_bytes, 0)
            self.assertIn("native_import", plan.capabilities)
            self.assertEqual(plan.destination_version, "1.0")

    def test_provisional_collision_escape_and_unknown_destination_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "models"
            destination_root.mkdir()
            destination = installation(
                "lm-studio", destination_root, AdapterCapabilities(copy=True)
            )
            weak = LibraryRecord(
                **{
                    **source.__dict__,
                    "identity": ArtifactIdentity("provisional", source="test"),
                }
            )
            with self.assertRaises(MigrationError):
                create_migration_plan(
                    plan_id="weak", source=weak, destination=destination,
                    destination_root=str(destination_root), destination_native_id="x",
                    target_path=str(destination_root / "x.gguf"),
                )
            collision = destination_root / "x.gguf"
            collision.write_bytes(b"existing")
            for target in (collision, root / "outside.gguf"):
                with self.assertRaises(MigrationError):
                    create_migration_plan(
                        plan_id="rejected", source=source, destination=destination,
                        destination_root=str(destination_root), destination_native_id="x",
                        target_path=str(target),
                    )

    def test_cli_can_freeze_and_revalidate_a_plan_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            source_record = Path(source.path).parent / "record.json"
            destination_root = root / "models"
            destination_root.mkdir()
            destination = installation(
                "lm-studio", destination_root,
                AdapterCapabilities(hard_link=True, native_import=True),
            )
            installation_file = root / "installation.json"
            installation_file.write_text(json.dumps(asdict(destination)))
            plan_file = root / "plan.json"
            code = main(
                [
                    "migration-plan",
                    "--source-record", str(source_record),
                    "--destination-installation", str(installation_file),
                    "--plan-id", "cli-plan",
                    "--destination-root", str(destination_root),
                    "--native-id", "owner/model.gguf",
                    "--target-path", str(destination_root / "model.gguf"),
                    "--out", str(plan_file),
                ]
            )
            self.assertEqual(code, 0)
            self.assertEqual(
                main(
                    [
                        "migration-validate",
                        "--plan", str(plan_file),
                        "--destination-installation", str(installation_file),
                    ]
                ),
                0,
            )

    def test_method_priority_uses_only_declared_or_probed_capabilities(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "dest"
            destination_root.mkdir()
            cases = (
                (AdapterCapabilities(reference=True), True, None, "shared_path"),
                (AdapterCapabilities(symbolic_link=True), False, None, "symbolic_link"),
                (AdapterCapabilities(native_import=True), False, None, "native_import"),
                (AdapterCapabilities(copy=True), False, lambda _a, _b: True, "reflink"),
                (AdapterCapabilities(copy=True), False, None, "copy"),
            )
            for index, (capabilities, shared, probe, expected) in enumerate(cases):
                destination = installation("tool", destination_root, capabilities)
                plan = create_migration_plan(
                    plan_id=f"method-{index}", source=source, destination=destination,
                    destination_root=str(destination_root), destination_native_id=f"model-{index}",
                    target_path=str(destination_root / f"model-{index}.gguf"),
                    shared_path_supported=shared, reflink_probe=probe,
                )
                self.assertEqual(plan.method, expected)


class MigrationExecutionTests(unittest.TestCase):
    def test_ollama_success_keeps_source_and_issues_no_retirement_token(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "ollama"
            destination_root.mkdir()
            destination = installation(
                "ollama",
                destination_root,
                AdapterCapabilities(
                    native_import=True,
                    load_validation=True,
                    inference_validation=True,
                ),
            )
            plan = create_migration_plan(
                plan_id="ollama-success", source=source, destination=destination,
                destination_root=str(destination_root), destination_native_id="tiny:migrated",
                target_path=None,
            )
            backend = FakeOllamaBackend(blob_exists=True)
            executor = MigrationExecutor(root / "migration-journals")
            result = executor.apply(
                plan, destination=destination, driver=OllamaMigrationDriver(backend)
            )
            self.assertTrue(result.success)
            self.assertTrue(result.retirement_eligible)
            self.assertIsNone(result.confirmation_token)
            self.assertTrue(Path(source.path).is_file())
            self.assertEqual(backend.deleted, [])
            payload = json.loads((root / "migration-journals/ollama-success.json").read_text())
            self.assertEqual(payload["result"]["success"], True)

    def test_failed_ollama_inference_rolls_back_only_created_model(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "ollama"
            destination_root.mkdir()
            destination = installation(
                "ollama", destination_root,
                AdapterCapabilities(native_import=True, inference_validation=True),
            )
            plan = create_migration_plan(
                plan_id="ollama-fail", source=source, destination=destination,
                destination_root=str(destination_root), destination_native_id="tiny:failed",
                target_path=None,
            )
            backend = FakeOllamaBackend(blob_exists=True, infer=False)
            result = MigrationExecutor(root / "journals").apply(
                plan, destination=destination, driver=OllamaMigrationDriver(backend)
            )
            self.assertFalse(result.success)
            self.assertEqual(backend.deleted, ["tiny:failed"])
            self.assertTrue(backend.blob)
            self.assertTrue(Path(source.path).is_file())

    def test_lms_validation_failure_unloads_and_unlinks_only_created_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "lm-models"
            destination_root.mkdir()
            target = destination_root / "owner/model/source.gguf"
            native_id = "owner/model/source.gguf"
            destination = installation(
                "lm-studio", destination_root,
                AdapterCapabilities(
                    hard_link=True, copy=True, native_import=True,
                    load_validation=True, inference_validation=True,
                ),
            )
            plan = create_migration_plan(
                plan_id="lms-fail", source=source, destination=destination,
                destination_root=str(destination_root), destination_native_id=native_id,
                target_path=str(target),
            )
            backend = FakeLMSBackend(
                target, native_id, source.identity.value, load=False, infer=False
            )
            result = MigrationExecutor(root / "journals").apply(
                plan, destination=destination, driver=LMStudioMigrationDriver(backend)
            )
            self.assertFalse(result.success)
            self.assertFalse(target.exists())
            self.assertEqual(backend.unloaded, [])
            self.assertTrue(Path(source.path).is_file())
            self.assertIsNone(result.confirmation_token)

    def test_stale_source_or_destination_refuses_before_native_execution(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "lm-models"
            destination_root.mkdir()
            target = destination_root / "source.gguf"
            destination = installation(
                "lm-studio", destination_root,
                AdapterCapabilities(hard_link=True, native_import=True),
            )
            plan = create_migration_plan(
                plan_id="stale", source=source, destination=destination,
                destination_root=str(destination_root), destination_native_id="owner/source.gguf",
                target_path=str(target),
            )
            Path(source.path).write_bytes(gguf() + b"changed")
            backend = FakeLMSBackend(
                target, "owner/source.gguf", source.identity.value
            )
            result = MigrationExecutor(root / "journals").apply(
                plan, destination=destination, driver=LMStudioMigrationDriver(backend)
            )
            self.assertEqual(result.error_code, "migration-plan-stale")
            self.assertEqual(backend.import_calls, 0)
            self.assertFalse(target.exists())

    def test_rollback_refuses_a_target_replaced_after_import(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = canonical(root)
            destination_root = root / "lm-models"
            destination_root.mkdir()
            target = destination_root / "model.gguf"
            native_id = "owner/model.gguf"
            destination = installation(
                "lm-studio", destination_root,
                AdapterCapabilities(hard_link=True, native_import=True),
            )
            plan = create_migration_plan(
                plan_id="rollback-race", source=source, destination=destination,
                destination_root=str(destination_root), destination_native_id=native_id,
                target_path=str(target),
            )

            class ReplacingBackend(FakeLMSBackend):
                def load(self, native_id):
                    self.target.unlink()
                    self.target.write_bytes(b"replacement")
                    return False

            backend = ReplacingBackend(target, native_id, source.identity.value)
            result = MigrationExecutor(root / "journals").apply(
                plan, destination=destination, driver=LMStudioMigrationDriver(backend)
            )
            self.assertEqual(result.error_code, "migration-rollback-incomplete")
            self.assertEqual(target.read_bytes(), b"replacement")


if __name__ == "__main__":
    unittest.main()
