from __future__ import annotations

import hashlib
import json
import os
import struct
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.model_orchestrator.catalog import canonical_artifacts
from scripts.model_orchestrator.formats import validate_model_file
from scripts.model_orchestrator.library import LibraryError, NeutralLibrary
from scripts.model_orchestrator.settings import (
    ModelSettings,
    default_library_root,
    load_settings,
    save_settings,
    validate_library_root,
)


def gguf(*, tensors: int = 0, metadata: int = 0, version: int = 3) -> bytes:
    return struct.pack("<4sIQQ", b"GGUF", version, tensors, metadata)


def safetensors(offsets=(0, 4), data=b"data") -> bytes:
    header = json.dumps(
        {"weight": {"dtype": "F32", "shape": [1], "data_offsets": list(offsets)}}
    ).encode()
    return len(header).to_bytes(8, "little") + header + data


class SettingsTests(unittest.TestCase):
    def test_defaults_are_machine_local_on_linux_and_windows(self) -> None:
        self.assertEqual(
            default_library_root(
                platform_name="linux", env={"HOME": "/home/dev"}
            ),
            "/home/dev/.local/share/devtoolbox/models",
        )
        self.assertEqual(
            default_library_root(
                platform_name="linux",
                env={"HOME": "/home/dev", "XDG_DATA_HOME": "/machine/data"},
            ),
            "/machine/data/devtoolbox/models",
        )
        self.assertEqual(
            default_library_root(
                platform_name="windows", env={"LOCALAPPDATA": r"C:\Users\dev\AppData\Local"}
            ),
            r"C:\Users\dev\AppData\Local\DevToolBox\models",
        )

    def test_setting_a_new_root_does_not_relocate_existing_data(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            old_root = home / "old-models"
            old_root.mkdir()
            marker = old_root / "keep.gguf"
            marker.write_bytes(b"keep")
            new_root = home / "new-models"
            new_root.mkdir()
            env = {"HOME": str(home), "XDG_DATA_HOME": str(home / "state")}
            save_settings(
                ModelSettings(str(new_root)), platform_name="linux", env=env
            )
            loaded = load_settings(platform_name="linux", env=env)
            self.assertEqual(loaded.library_root, str(new_root))
            self.assertTrue(marker.is_file())
            self.assertFalse(any(new_root.iterdir()))

    def test_override_requires_absolute_writable_root_and_enough_space(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                validate_library_root(
                    directory,
                    platform_name="linux",
                    env={"HOME": directory},
                    required_free_bytes=2**100,
                )

    def test_provider_xet_and_keep_preferences_round_trip_locally(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            library = home / "models"
            library.mkdir()
            env = {"HOME": str(home), "XDG_DATA_HOME": str(home / "state")}
            settings = ModelSettings(
                str(library),
                provider_order=("direct", "lm-studio", "huggingface", "ollama"),
                enabled_providers=("direct", "huggingface"),
                xet_enabled=False,
                keep_patterns=("*important*",),
            )
            save_settings(settings, platform_name="linux", env=env)
            self.assertEqual(load_settings(platform_name="linux", env=env), settings)


class FormatTests(unittest.TestCase):
    def test_structured_formats_validate_bounds_without_loading_tensors(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gguf_path = root / "ok.gguf"
            gguf_path.write_bytes(gguf())
            safe_path = root / "ok.safetensors"
            safe_path.write_bytes(safetensors())
            self.assertEqual(validate_model_file(gguf_path).level, "structural")
            self.assertEqual(validate_model_file(safe_path).level, "structural")
            safe_path.write_bytes(safetensors(offsets=(0, 99)))
            self.assertEqual(validate_model_file(safe_path).level, "failed")

    def test_opaque_nonzero_is_honest_and_malformed_gguf_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            opaque = root / "weights.ckpt"
            opaque.write_bytes(b"opaque")
            malformed = root / "bad.gguf"
            malformed.write_bytes(gguf(tensors=999))
            self.assertEqual(validate_model_file(opaque).level, "opaque")
            self.assertFalse(validate_model_file(malformed).valid)


class NeutralLibraryTests(unittest.TestCase):
    def test_stream_hashes_inline_commits_and_redacts_origin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            body = gguf()
            digest = hashlib.sha256(body).hexdigest()
            record = library.commit_stream(
                "download-1",
                "tiny.gguf",
                (body[:8], body[8:]),
                family="llm",
                expected_digest=digest,
                origin="https://user:secret@example.test/model.gguf?token=secret#part",
                revision="commit-1",
            )
            self.assertEqual(record.artifact_id, digest)
            self.assertEqual(record.identity.exact_key, f"sha256:{digest}")
            self.assertEqual(record.validation.level, "strong")
            self.assertEqual(record.destination_usability, {})
            canonical = canonical_artifacts([record])[0]
            self.assertEqual(canonical.family, "llm")
            self.assertEqual(canonical.relationship, "canonical")
            self.assertEqual(record.origin, "https://example.test/model.gguf")
            self.assertTrue(Path(record.path).is_file())
            self.assertIsNotNone(record.allocated_size)
            self.assertFalse(Path(library.reconcile()[0].staging_path).exists())
            self.assertEqual(library.reconcile()[0].state, "completed")

    def test_same_digest_reuses_one_canonical_allocation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            body = gguf()
            first = library.commit_stream("one", "one.gguf", (body,), family="llm")
            second = library.commit_stream("two", "two.gguf", (body,), family="llm")
            self.assertEqual(first.path, second.path)
            self.assertEqual(len(library.list_records()), 1)

    def test_local_import_is_provisional_hardlinked_and_queues_hashing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "native.gguf"
            source.write_bytes(gguf())
            library = NeutralLibrary(root / "library")
            record = library.import_file("import-1", source, family="llm")
            self.assertEqual(record.identity.state, "provisional")
            self.assertEqual(record.relationship, "hard_link")
            self.assertEqual(source.stat().st_ino, Path(record.path).stat().st_ino)
            self.assertTrue(record.hash_pending)
            self.assertTrue((library.hash_queue_root / "import-1.json").is_file())

    def test_cross_volume_import_copies_and_symbolic_link_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.bin"
            source.write_bytes(b"opaque")
            library = NeutralLibrary(root / "library")
            with mock.patch(
                "scripts.model_orchestrator.library.same_filesystem",
                side_effect=(True, False),
            ):
                record = library.import_file("copy-1", source, family="llm")
            self.assertNotEqual(source.stat().st_ino, Path(record.path).stat().st_ino)
            self.assertEqual(record.relationship, "copy")
            link = root / "linked.bin"
            link.symlink_to(source)
            with self.assertRaises(LibraryError):
                library.import_file("link-1", link, family="llm")
            self.assertEqual(
                next(row for row in library.reconcile() if row.operation_id == "link-1").state,
                "manual-attention",
            )

    def test_hash_mismatch_and_corruption_never_become_canonical(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            with self.assertRaises(LibraryError):
                library.commit_stream(
                    "mismatch", "bad.gguf", (gguf(),), family="llm", expected_digest="0" * 64
                )
            with self.assertRaises(LibraryError):
                library.commit_stream("corrupt", "bad.gguf", (b"GGUF",), family="llm")
            self.assertEqual(library.list_records(), [])
            self.assertTrue(
                all(row.state == "manual-attention" for row in library.reconcile())
            )

    def test_interrupted_partial_is_resumable_and_discard_requires_exact_id(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            journal = library.begin("interrupted", "model.gguf")
            Path(journal.staging_path).write_bytes(b"partial")
            reconciled = library.reconcile()[0]
            self.assertEqual(reconciled.state, "resumable")
            self.assertEqual(reconciled.bytes_written, 7)
            with self.assertRaises(LibraryError):
                library.discard("../interrupted")
            library.discard("interrupted")
            self.assertEqual(library.reconcile(), [])

    def test_empty_started_operation_is_explicitly_discardable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            library.begin("empty-operation", "model.gguf")
            self.assertEqual(library.reconcile()[0].state, "discardable")


if __name__ == "__main__":
    unittest.main()
