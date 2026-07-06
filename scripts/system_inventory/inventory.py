#!/usr/bin/env python3
"""Orchestrator + CLI entry point for the system inventory tool.

Aggregates every enabled scanner source (only ``registry`` in Part 1 of the
master plan), sorts the combined result by descending on-disk/estimated
size, and prints either a text report or a ``--json`` payload, always with
a grand total. Read-only, offline, stdlib only — see ``README.md``.

Import bootstrap (deliberate, see the module docstring of
``tests/test_common.py`` for the sibling half of this story): this file is
invoked two different ways by the project's own acceptance criteria:

  1. ``python scripts/system_inventory/inventory.py [--json]`` — a *direct
     script* execution from the repo root. In this mode Python sets
     ``sys.path[0]`` to the script's own directory
     (``scripts/system_inventory/``), NOT the repo root and NOT ``cwd`` —
     so a bare absolute import of ``scripts.system_inventory.common`` would
     fail: the ``scripts`` package is not on ``sys.path`` at all.
  2. ``python -m unittest discover -s scripts/system_inventory/tests -v``
     — which imports ``test_inventory.py``, which in turn imports this
     module as ``scripts.system_inventory.inventory``. In this mode the
     repo root *is* already on ``sys.path`` (as ``cwd``), so the same
     absolute import spelling resolves without any help.

To make a single absolute-import line work in both contexts, the repo root
(this file's grandparent's parent) is inserted onto ``sys.path`` before the
``scripts.system_inventory.*`` imports below, if not already present. This
was verified empirically against both literal invocations before commit,
per the plan's instructions — see the two exact commands in the module
docstring above.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Callable

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.system_inventory.common import (  # noqa: E402
    InventoryItem,
    human_size,
    sort_items,
    to_json_payload,
    total_bytes,
)
from scripts.system_inventory.registry import (  # noqa: E402
    WINREG_AVAILABLE,
    WinregUnavailableError,
    scan_registry,
)

# Every enabled scanner source, keyed by its stable ``source`` tag. Parts 2
# and 3 of the master plan add ``appdata``, ``dotfolder``, ``programdata``,
# ``scoop-choco``, ``path``, ``docker-wsl`` here — nothing else in this
# module needs to change for that: ``--source`` choices, the default
# all-sources set, and the aggregation loop all derive from this dict.
SCANNERS: dict[str, Callable[[], list[InventoryItem]]] = {
    "registry": scan_registry,
}


def _resolve_active_sources(requested: list[str] | None) -> list[str]:
    """Resolve the ``--source`` CLI argument into the list of sources to run.

    ``requested is None`` (flag omitted entirely) means "all currently
    registered sources", i.e. ``list(SCANNERS)`` — just ``["registry"]`` in
    Part 1. A source is dropped from the active list (rather than raising)
    when its dependency is unavailable on this platform: today the only
    such case is ``"registry"`` requiring ``winreg`` (Windows-only stdlib).
    An empty return value signals "nothing runnable" to the caller, which
    is the trigger for the non-zero-exit guard in ``main()``.
    """
    names = requested if requested is not None else list(SCANNERS)
    active: list[str] = []
    for name in names:
        if name == "registry" and not WINREG_AVAILABLE:
            continue
        active.append(name)
    return active


def _render_text_report(items: list[InventoryItem], top: int | None) -> str:
    """Render the text report, honoring ``--top`` without perturbing the total.

    Mirrors ``common.format_report()``'s line format exactly, but does not
    call it directly: ``format_report()`` computes both the displayed lines
    and the grand total from the same list it is given, and the CLI
    contract requires ``--top`` to cap only the displayed lines while the
    grand total always reflects every scanned item.
    """
    if not items:
        return "Aucun élément trouvé."

    sorted_items = sort_items(items)
    displayed = sorted_items[:top] if top is not None else sorted_items

    lines = [
        f"[{item.source}] {item.name} — {human_size(item.size_bytes)} ({item.path or '-'})"
        for item in displayed
    ]
    total = total_bytes(items)
    lines.append("")
    lines.append(f"Total: {human_size(total)} ({total} bytes)")
    return "\n".join(lines)


def _render_json_payload(items: list[InventoryItem], top: int | None) -> dict[str, object]:
    """Build the ``--json`` payload, honoring ``--top`` without perturbing the total.

    ``to_json_payload()`` is computed once over the *full* item list so
    ``total_bytes``/``total_human`` always reflect every scanned item, then
    the ``items`` array alone is truncated to ``--top`` entries (it is
    already descending-sorted by ``to_json_payload()``, so slicing keeps
    the largest ``top`` items).
    """
    payload = to_json_payload(items)
    if top is not None:
        payload["items"] = payload["items"][:top]
    return payload


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Inventaire en lecture seule des traces disque des outils de dev "
            "Windows (registre Uninstall pour l'instant ; AppData/Scoop/Choco/"
            "PATH/Docker-WSL suivront dans les parties 2 et 3)."
        )
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="émet le rapport au format JSON",
    )
    parser.add_argument(
        "--source",
        action="append",
        choices=sorted(SCANNERS),
        default=None,
        help=(
            "restreint aux source(s) indiquée(s) ; répétable "
            "(par défaut : toutes les sources activées)"
        ),
    )
    parser.add_argument(
        "--top",
        type=int,
        default=None,
        metavar="N",
        help=(
            "limite le nombre d'éléments affichés/émis après tri (le total "
            "général reste toujours calculé sur l'ensemble des éléments scannés)"
        ),
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    # Force UTF-8 on stdout regardless of the console's default codepage.
    # Windows terminals commonly default stdout to a legacy codepage (e.g.
    # cp1252), under which non-ASCII characters that appear in real-world
    # data (an em dash in the report format, accented installer names from
    # the registry) get mis-encoded: cp1252 happily encodes them to a
    # single byte without raising, but that byte is not valid UTF-8 — so a
    # consumer expecting UTF-8 (``python -m json.tool``, this project's own
    # acceptance criteria) fails to parse it. ``reconfigure`` may be
    # unavailable on a stand-in stdout (e.g. ``io.StringIO`` used by tests
    # via ``redirect_stdout``), so the call is best-effort.
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except (AttributeError, ValueError):
        pass

    parser = build_parser()
    args = parser.parse_args(argv)

    if args.top is not None and args.top < 0:
        parser.error("--top doit être un entier positif ou nul")

    active_sources = _resolve_active_sources(args.source)
    if not active_sources:
        print(
            "Aucune source disponible : winreg (registre Windows) est requis "
            "pour la source 'registry' et n'est pas disponible sur cette "
            "plateforme, ou aucune source valide n'a été demandée.",
            file=sys.stderr,
        )
        return 2

    items: list[InventoryItem] = []
    for name in active_sources:
        scanner = SCANNERS[name]
        try:
            items.extend(scanner())
        except WinregUnavailableError as exc:  # pragma: no cover - guarded above already
            print(f"Source '{name}' indisponible : {exc}", file=sys.stderr)

    if args.json:
        payload = _render_json_payload(items, args.top)
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print(_render_text_report(items, args.top))

    return 0


if __name__ == "__main__":
    sys.exit(main())
