"""Read-only schema and fixture entry point for the model orchestrator."""

from __future__ import annotations

import argparse
import json

from .catalog import build_snapshot, inventory_snapshot
from .models import Artifact, ArtifactIdentity, SCHEMA_VERSION


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Orchestrateur de modèles locaux")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("schema", help="afficher la version du schéma JSON")
    subparsers.add_parser("fixture", help="émettre un catalogue déterministe de contrat")
    subparsers.add_parser("inventory", help="inventorier les modèles locaux sans mutation")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "schema":
        print(json.dumps({"schema_version": SCHEMA_VERSION}, sort_keys=True))
        return 0
    if args.command == "inventory":
        print(json.dumps(inventory_snapshot().to_dict(), ensure_ascii=False, sort_keys=True))
        return 0
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
