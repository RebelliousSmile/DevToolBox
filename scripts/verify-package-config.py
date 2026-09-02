#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import pathlib
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
config = tomllib.loads((ROOT / "packager.toml").read_text(encoding="utf-8"))
cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
errors: list[str] = []

def require(condition: bool, message: str) -> None:
    if not condition:
        errors.append(message)

require(config.get("identifier") == "com.rebellioussmile.devtoolbox", "identifiant incorrect")
require(config.get("name") == cargo["package"]["name"], "nom de paquet Cargo absent ou divergent")
require(config.get("version") == cargo["package"]["version"], "versions Cargo/Packager divergentes")
require(config.get("nsis", {}).get("installMode") == "currentUser", "NSIS doit être par utilisateur")
require(config.get("macos", {}).get("minimumSystemVersion") == "13.0", "macOS 13 minimum absent")
require(set(config.get("deb", {}).get("depends", [])) >= {"libc6 (>= 2.35)", "libx11-6", "libwayland-client0"}, "dépendances deb incomplètes")
require("desktopTemplate" in config.get("deb", {}), "template desktop deb absent")
require(config.get("linux", {}).get("generateDesktopEntry") is True, "desktop Linux absent")
for path in [
    "assets/app-icon/devtoolbox.icns", "assets/app-icon/devtoolbox.ico",
    "assets/app-icon/devtoolbox.png", "packaging/macos/entitlements.plist",
    "packaging/linux/devtoolbox.desktop", "THIRD_PARTY_LICENSES.md",
    "LICENSE",
]:
    require((ROOT / path).is_file(), f"ressource absente: {path}")

if errors:
    print("Configuration de paquet invalide:\n- " + "\n- ".join(errors), file=sys.stderr)
    raise SystemExit(1)
for path in config["icons"]:
    payload = (ROOT / path).read_bytes()
    if path.endswith(".png"):
        require(payload[:8] == b"\x89PNG\r\n\x1a\n", "signature PNG invalide")
        require(int.from_bytes(payload[16:20], "big") == 1024, "largeur PNG incorrecte")
        require(int.from_bytes(payload[20:24], "big") == 1024, "hauteur PNG incorrecte")
    elif path.endswith(".ico"):
        require(payload[:4] == b"\x00\x00\x01\x00", "signature ICO invalide")
    elif path.endswith(".icns"):
        require(payload[:4] == b"icns", "signature ICNS invalide")
    print(f"{path} {hashlib.sha256(payload).hexdigest()} {len(payload)}")
if errors:
    print("Ressources invalides:\n- " + "\n- ".join(errors), file=sys.stderr)
    raise SystemExit(1)
print("packaging: nsis x64; dmg arm64+x86_64; deb x64; appimage x64")
