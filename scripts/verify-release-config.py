#!/usr/bin/env python3
"""Static, secret-free release workflow gate."""

from __future__ import annotations

import argparse
import re
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def verify(root: Path) -> None:
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    packager = tomllib.loads((root / "packager.toml").read_text(encoding="utf-8"))
    toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text(encoding="utf-8"))
    version = cargo["package"]["version"]
    if version != packager["version"] or tuple(map(int, version.split("."))) < (0, 10, 0):
        raise ValueError("Cargo/Packager version mismatch or updater-incompatible version")
    if toolchain["toolchain"]["channel"] != "1.93.0":
        raise ValueError("release toolchain must be Rust 1.93.0")
    workflows = "\n".join(
        (root / ".github" / "workflows" / name).read_text(encoding="utf-8")
        for name in ("ci.yml", "release.yml")
    )
    for runner in ("windows-2025", "ubuntu-22.04", "macos-15", "macos-15-intel"):
        if runner not in workflows:
            raise ValueError(f"missing explicit runner {runner}")
    for line in workflows.splitlines():
        if "uses:" in line and not re.search(r"@[0-9a-f]{40}(?:\s|#|$)", line):
            raise ValueError(f"action is not pinned by full SHA: {line.strip()}")
    release = (root / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
    for token in ("environment: production-release", "DEVTOOLBOX_RELEASE_BUILD", "DEVTOOLBOX_UPDATE_PUBLIC_KEYS", "NATIVE_QUALIFICATION_COMPLETE", "draft=true"):
        if token not in release:
            raise ValueError(f"missing release gate: {token}")
    if "pull_request_target" in workflows:
        raise ValueError("pull_request_target could expose secrets to external contributions")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / ".github" / "workflows").mkdir(parents=True)
        (root / "Cargo.toml").write_text('[package]\nversion = "0.10.0"\n', encoding="utf-8")
        (root / "packager.toml").write_text('version = "0.10.0"\n', encoding="utf-8")
        (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.93.0"\n', encoding="utf-8")
        workflow = """permissions:\n  contents: read\njobs:\n  fixture:\n    runs-on: windows-2025\n    steps:\n      - uses: actions/checkout@1111111111111111111111111111111111111111\n# ubuntu-22.04 macos-15 macos-15-intel\n"""
        (root / ".github" / "workflows" / "ci.yml").write_text(workflow, encoding="utf-8")
        release = """permissions:\n  contents: read\n# windows-2025 ubuntu-22.04 macos-15 macos-15-intel\n# environment: production-release\n# DEVTOOLBOX_RELEASE_BUILD DEVTOOLBOX_UPDATE_PUBLIC_KEYS NATIVE_QUALIFICATION_COMPLETE draft=true\n"""
        release_path = root / ".github" / "workflows" / "release.yml"
        release_path.write_text(release, encoding="utf-8")
        verify(root)
        release_path.write_text(release.replace("DEVTOOLBOX_UPDATE_PUBLIC_KEYS", "MISSING_KEYRING"), encoding="utf-8")
        try:
            verify(root)
        except ValueError as error:
            assert "DEVTOOLBOX_UPDATE_PUBLIC_KEYS" in str(error)
        else:
            raise AssertionError("missing production keyring gate did not fail")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    verify(args.root.resolve())


if __name__ == "__main__":
    main()
