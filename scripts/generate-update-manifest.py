#!/usr/bin/env python3
"""Build latest.json from immutable release assets and signer sidecars."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import tempfile
from pathlib import Path

ASSETS = (
    ("windows", "x86_64", "nsis", "DevToolBox_{v}_windows_x86_64-setup.exe"),
    ("macos", "aarch64", "app", "DevToolBox_{v}_macos_aarch64.dmg"),
    ("macos", "x86_64", "app", "DevToolBox_{v}_macos_x86_64.dmg"),
    ("linux", "x86_64", "deb", "DevToolBox_{v}_linux_x86_64.deb"),
    ("linux", "x86_64", "appimage", "DevToolBox_{v}_linux_x86_64.AppImage"),
)
REPOSITORY = "RebelliousSmile/DevToolBox"


def digest(path: Path) -> tuple[int, str]:
    data = path.read_bytes()
    return len(data), hashlib.sha256(data).hexdigest()


def checked_sidecar(path: Path, size: int, sha256: str) -> list[dict]:
    sidecar = json.loads(path.read_text(encoding="utf-8"))
    if sidecar.get("size") != size or sidecar.get("sha256") != sha256:
        raise ValueError(f"signature metadata does not match {path.name}")
    signatures = sidecar.get("signatures")
    if not isinstance(signatures, list) or not signatures:
        raise ValueError(f"no updater signature in {path.name}")
    required = {"key_id", "signature", "activated_minor", "valid_until_epoch_days"}
    if any(set(item) != required for item in signatures):
        raise ValueError(f"invalid updater signature schema in {path.name}")
    return signatures


def build_manifest(directory: Path, version: str, notes: str, recovery_dir: Path | None) -> dict:
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("version must be strict semver without a v prefix")
    assets = []
    for os_name, arch, fmt, pattern in ASSETS:
        filename = pattern.format(v=version)
        path = directory / filename
        if not path.is_file():
            raise ValueError(f"missing release asset: {filename}")
        size, sha256 = digest(path)
        signatures = checked_sidecar(path.with_name(filename + ".signatures.json"), size, sha256)
        asset = {
            "os": os_name,
            "arch": arch,
            "format": fmt,
            "url": f"https://github.com/{REPOSITORY}/releases/download/v{version}/{filename}",
            "size": size,
            "sha256": sha256,
            "signatures": signatures,
            "recovery": None,
        }
        if fmt in {"nsis", "app"}:
            if recovery_dir is None:
                raise ValueError(f"recovery directory required for {filename}")
            recovery_meta = json.loads(
                path.with_name(filename + ".recovery.json").read_text(encoding="utf-8")
            )
            recovery_version = recovery_meta["version"]
            recovery_name = recovery_meta["filename"]
            recovery_path = recovery_dir / recovery_name
            recovery_size, recovery_sha = digest(recovery_path)
            recovery_signatures = checked_sidecar(
                recovery_path.with_name(recovery_name + ".signatures.json"),
                recovery_size,
                recovery_sha,
            )
            asset["recovery"] = {
                "version": recovery_version,
                "os": os_name,
                "arch": arch,
                "format": fmt,
                "url": f"https://github.com/{REPOSITORY}/releases/download/v{recovery_version}/{recovery_name}",
                "size": recovery_size,
                "sha256": recovery_sha,
                "signatures": recovery_signatures,
            }
        if fmt in {"deb", "appimage"} and not path.with_name(filename + ".minisig").is_file():
            raise ValueError(f"missing first-download Minisign signature: {filename}.minisig")
        assets.append(asset)
    return {"schema_version": 1, "version": version, "notes": notes, "assets": assets}


def self_test() -> None:
    with tempfile.TemporaryDirectory() as root:
        root_path = Path(root)
        current = root_path / "current"
        recovery = root_path / "recovery"
        current.mkdir()
        recovery.mkdir()
        for os_name, arch, fmt, pattern in ASSETS:
            name = pattern.format(v="0.11.0")
            path = current / name
            path.write_bytes(name.encode())
            size, sha = digest(path)
            signature = {"key_id": "fixture", "signature": "AA==", "activated_minor": 11, "valid_until_epoch_days": 99999}
            path.with_name(name + ".signatures.json").write_text(
                json.dumps({"size": size, "sha256": sha, "signatures": [signature]}), encoding="utf-8"
            )
            if fmt in {"deb", "appimage"}:
                path.with_name(name + ".minisig").write_text("fixture", encoding="utf-8")
            if fmt in {"nsis", "app"}:
                old_name = pattern.format(v="0.10.0")
                old = recovery / old_name
                old.write_bytes(old_name.encode())
                old_size, old_sha = digest(old)
                old.with_name(old_name + ".signatures.json").write_text(
                    json.dumps({"size": old_size, "sha256": old_sha, "signatures": [signature]}), encoding="utf-8"
                )
                path.with_name(name + ".recovery.json").write_text(
                    json.dumps({"version": "0.10.0", "filename": old_name}), encoding="utf-8"
                )
        manifest = build_manifest(current, "0.11.0", "fixture", recovery)
        assert len(manifest["assets"]) == 5
        (current / "DevToolBox_0.11.0_linux_x86_64.AppImage.minisig").unlink()
        try:
            build_manifest(current, "0.11.0", "fixture", recovery)
        except ValueError as error:
            assert "Minisign" in str(error)
        else:
            raise AssertionError("missing Linux signature did not fail")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--version")
    parser.add_argument("--notes", default="")
    parser.add_argument("--recovery-assets", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not all((args.assets, args.version, args.output)):
        parser.error("--assets, --version and --output are required")
    manifest = build_manifest(args.assets, args.version, args.notes, args.recovery_assets)
    args.output.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
