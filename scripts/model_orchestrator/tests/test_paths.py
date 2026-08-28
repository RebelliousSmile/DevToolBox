from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from scripts.model_orchestrator.paths import (
    PathSafetyError,
    ensure_owned_target,
    file_evidence,
    normalize_absolute_path,
    same_filesystem,
)


class PathContractTests(unittest.TestCase):
    def test_linux_expands_known_values_and_keeps_spaces(self) -> None:
        self.assertEqual(
            normalize_absolute_path(
                "$HOME/Model Library", platform_name="linux", env={"HOME": "/home/alice"}
            ),
            "/home/alice/Model Library",
        )

    def test_windows_expands_case_insensitive_values(self) -> None:
        self.assertEqual(
            normalize_absolute_path(
                r"%LOCALAPPDATA%\DevToolBox\models",
                platform_name="windows",
                env={"localappdata": r"C:\Users\Alice\AppData\Local"},
            ),
            r"C:\Users\Alice\AppData\Local\DevToolBox\models",
        )

    def test_unresolved_and_relative_paths_are_rejected(self) -> None:
        for raw in ("$MISSING/models", "relative/models"):
            with self.subTest(raw=raw), self.assertRaises(PathSafetyError):
                normalize_absolute_path(raw, platform_name="linux", env={})

    def test_target_must_be_below_but_not_equal_to_owned_root(self) -> None:
        ensure_owned_target(
            "/models/.staging/op/file", owned_root="/models/.staging", platform_name="linux"
        )
        for target in ("/models/.staging", "/models/outside"):
            with self.subTest(target=target), self.assertRaises(PathSafetyError):
                ensure_owned_target(target, owned_root="/models/.staging", platform_name="linux")

    def test_windows_different_volume_is_rejected(self) -> None:
        with self.assertRaises(PathSafetyError):
            ensure_owned_target(
                r"D:\models\file", owned_root=r"C:\models", platform_name="windows"
            )

    def test_file_evidence_distinguishes_copy_hardlink_and_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = root / "model.gguf"
            original.write_bytes(b"GGUF")
            linked = root / "linked.gguf"
            os.link(original, linked)
            symbolic = root / "symbolic.gguf"
            symbolic.symlink_to(original)
            original_evidence = file_evidence(original)
            linked_evidence = file_evidence(linked)
            symbolic_evidence = file_evidence(symbolic)
            self.assertEqual(original_evidence.relationship, "hard_link")
            self.assertEqual(original_evidence.allocation_id, linked_evidence.allocation_id)
            self.assertEqual(symbolic_evidence.relationship, "symbolic_link")
            self.assertTrue(same_filesystem(original, linked))


if __name__ == "__main__":
    unittest.main()
