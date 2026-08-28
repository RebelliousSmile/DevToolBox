"""Read-only schema and fixture entry point for the model orchestrator."""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict

from .catalog import build_snapshot, inventory_snapshot
from .library import NeutralLibrary
from .models import Artifact, ArtifactIdentity, SCHEMA_VERSION
from .settings import ModelSettings, load_settings, save_settings


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Orchestrateur de modèles locaux")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("schema", help="afficher la version du schéma JSON")
    subparsers.add_parser("fixture", help="émettre un catalogue déterministe de contrat")
    subparsers.add_parser("inventory", help="inventorier les modèles locaux sans mutation")
    settings = subparsers.add_parser("settings", help="afficher ou modifier la bibliothèque locale")
    settings.add_argument("--set-library-root")
    library = subparsers.add_parser("library", help="inspecter les artefacts canoniques")
    library.add_argument("--root")
    recovery = subparsers.add_parser("recovery", help="inspecter les opérations interrompues")
    recovery.add_argument("--root")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "schema":
        print(json.dumps({"schema_version": SCHEMA_VERSION}, sort_keys=True))
        return 0
    if args.command == "inventory":
        print(json.dumps(inventory_snapshot().to_dict(), ensure_ascii=False, sort_keys=True))
        return 0
    platform_name = "windows" if sys.platform == "win32" else "linux"
    if args.command == "settings":
        settings = load_settings(platform_name=platform_name, env=os.environ)
        if args.set_library_root:
            settings = ModelSettings(args.set_library_root)
            save_settings(settings, platform_name=platform_name, env=os.environ)
            settings = load_settings(platform_name=platform_name, env=os.environ)
        print(json.dumps(asdict(settings), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command in {"library", "recovery"}:
        selected_root = args.root or load_settings(
            platform_name=platform_name, env=os.environ
        ).library_root
        library = NeutralLibrary(selected_root)
        rows = library.list_records() if args.command == "library" else library.reconcile()
        print(json.dumps([asdict(row) for row in rows], ensure_ascii=False, sort_keys=True))
        return 0
    if args.command != "fixture":
        raise AssertionError(f"commande non traitée : {args.command}")
    artifact = Artifact(
        artifact_id="fixture-gguf",
        path="/fixtures/model.gguf",
        family="llm",
        format="gguf",
        identity=ArtifactIdentity(
            state="verified", algorithm="sha256", value="a" * 64, source="fixture"
        ),
        logical_size=1024,
        allocated_size=4096,
        relationship="canonical",
    )
    snapshot = build_snapshot(
        platform="fixture", artifacts=[artifact], generated_at="2026-08-28T00:00:00+00:00"
    )
    print(json.dumps(snapshot.to_dict(), ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
