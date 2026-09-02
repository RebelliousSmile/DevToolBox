#!/usr/bin/env python3
"""Fail unless latest.json describes the exact local release bytes."""

from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path

import hashlib

ASSETS = (
    ("windows", "x86_64", "nsis", "DevToolBox_{v}_windows_x86_64-setup.exe"),
    ("macos", "aarch64", "app", "DevToolBox_{v}_macos_aarch64.dmg"),
    ("macos", "x86_64", "app", "DevToolBox_{v}_macos_x86_64.dmg"),
    ("linux", "x86_64", "deb", "DevToolBox_{v}_linux_x86_64.deb"),
    ("linux", "x86_64", "appimage", "DevToolBox_{v}_linux_x86_64.AppImage"),
)


def digest(path: Path) -> tuple[int, str]:
    data = path.read_bytes()
    return len(data), hashlib.sha256(data).hexdigest()


def verify(manifest_path: Path, assets_dir: Path, version: str) -> None:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1 or manifest.get("version") != version:
        raise ValueError("manifest schema/version mismatch")
    expected = {pattern.format(v=version) for _, _, _, pattern in ASSETS}
    described = {Path(asset["url"]).name for asset in manifest.get("assets", [])}
    if described != expected:
        raise ValueError("manifest does not describe exactly the five expected assets")
    for asset in manifest["assets"]:
        name = Path(asset["url"]).name
        size, sha256 = digest(assets_dir / name)
        if asset["size"] != size or asset["sha256"] != sha256:
            raise ValueError(f"manifest bytes mismatch for {name}")
        if not asset.get("signatures"):
            raise ValueError(f"missing updater signature for {name}")
        if asset["format"] in {"nsis", "app"} and not asset.get("recovery"):
            raise ValueError(f"missing recovery payload for {name}")
        if asset["format"] in {"deb", "appimage"} and not (assets_dir / f"{name}.minisig").is_file():
            raise ValueError(f"missing Minisign signature for {name}")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        assets = []
        for os_name, arch, fmt, pattern in ASSETS:
            name = pattern.format(v="0.11.0")
            path = root / name
            path.write_bytes(name.encode())
            size, sha256 = digest(path)
            if fmt in {"deb", "appimage"}:
                (root / f"{name}.minisig").write_text("fixture", encoding="utf-8")
            assets.append({
                "os": os_name, "arch": arch, "format": fmt,
                "url": f"https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.11.0/{name}",
                "size": size, "sha256": sha256,
                "signatures": [{"key_id": "fixture"}],
                "recovery": {"fixture": True} if fmt in {"nsis", "app"} else None,
            })
        manifest = root / "latest.json"
        manifest.write_text(json.dumps({"schema_version": 1, "version": "0.11.0", "notes": "", "assets": assets}), encoding="utf-8")
        verify(manifest, root, "0.11.0")
        (root / ASSETS[0][3].format(v="0.11.0")).write_bytes(b"changed")
        try:
            verify(manifest, root, "0.11.0")
        except ValueError as error:
            assert "bytes mismatch" in str(error)
        else:
            raise AssertionError("changed release bytes did not fail")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path, nargs="?")
    parser.add_argument("assets", type=Path, nargs="?")
    parser.add_argument("version", nargs="?")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not all((args.manifest, args.assets, args.version)):
        parser.error("manifest, assets and version are required")
    verify(args.manifest, args.assets, args.version)


if __name__ == "__main__":
    main()
