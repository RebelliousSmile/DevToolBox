"""Read-only JSON command-line entry point."""

from __future__ import annotations

import argparse
import sys

from .history import load_history
from .report import build_report, to_json


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Rapport consultatif des applications à désinstaller")
    parser.add_argument("--json", action="store_true", help="émettre le rapport JSON")
    parser.add_argument("--history", help="lire un historique local d'usage")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if not args.json:
        print("Le format --json est requis.", file=sys.stderr)
        return 2
    report = build_report({}, load_history(args.history))
    print(to_json(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
