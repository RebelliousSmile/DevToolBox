from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from launch_rust_app import resolve_build_plan, resolve_plan


class ResolvePlanTests(unittest.TestCase):
    def test_auto_prefers_release(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            release = root / "target" / "release" / "app.exe"
            debug = root / "target" / "debug" / "app.exe"
            release.parent.mkdir(parents=True)
            debug.parent.mkdir(parents=True)
            release.write_bytes(b"release")
            debug.write_bytes(b"debug")

            plan = resolve_plan(
                label="App",
                project_dir=root,
                release_relpath="target/release/app.exe",
                debug_relpath="target/debug/app.exe",
                mode="auto",
                run_args=[],
            )

            self.assertEqual(plan.mode, "release")
            self.assertEqual(plan.executable, release)

    def test_auto_falls_back_to_debug(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            debug = root / "target" / "debug" / "app.exe"
            debug.parent.mkdir(parents=True)
            debug.write_bytes(b"debug")

            plan = resolve_plan(
                label="App",
                project_dir=root,
                release_relpath="target/release/app.exe",
                debug_relpath="target/debug/app.exe",
                mode="auto",
                run_args=[],
            )

            self.assertEqual(plan.mode, "debug")
            self.assertEqual(plan.executable, debug)

    def test_release_mode_requires_release_binary(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            with self.assertRaises(FileNotFoundError):
                resolve_plan(
                    label="App",
                    project_dir=root,
                    release_relpath="target/release/app.exe",
                    debug_relpath="target/debug/app.exe",
                    mode="release",
                    run_args=[],
                )


    def test_resolve_plan_preserves_run_args(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            release = root / "target" / "release" / "app.exe"
            release.parent.mkdir(parents=True)
            release.write_bytes(b"release")

            plan = resolve_plan(
                label="App",
                project_dir=root,
                release_relpath="target/release/app.exe",
                debug_relpath="target/debug/app.exe",
                mode="release",
                run_args=["tray", "--verbose"],
            )

            self.assertEqual(plan.run_args, ("tray", "--verbose"))

    def test_build_plan_none_without_program(self) -> None:
        self.assertIsNone(
            resolve_build_plan(build_cwd=None, build_program=None, build_args=[])
        )

    def test_build_plan_requires_cwd(self) -> None:
        with self.assertRaises(ValueError):
            resolve_build_plan(
                build_cwd=None,
                build_program="cargo",
                build_args=["build"],
            )


if __name__ == "__main__":
    unittest.main()
