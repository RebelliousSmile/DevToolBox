#!/usr/bin/env python3
"""Orchestrator + CLI entry point for the system inventory tool.

Aggregates every enabled scanner source (all seven, as of Part 3 of the
master plan: ``registry``, ``appdata``, ``dotfolder``, ``programdata``,
``scoop-choco``, ``path``, ``docker-wsl``), sorts the combined result by
descending on-disk/estimated size, and prints either a text report or a
``--json`` payload, always with a grand total. Read-only, offline, stdlib
only — see ``README.md``.

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
from scripts.system_inventory.appdata import (  # noqa: E402
    scan_appdata,
    scan_dotfolders,
    scan_programdata,
)
from scripts.system_inventory.packages import scan_scoop_choco  # noqa: E402
from scripts.system_inventory.path_env import scan_path  # noqa: E402
from scripts.system_inventory.docker_wsl import scan_docker_wsl  # noqa: E402

# Every enabled scanner source, keyed by its stable ``source`` tag. All seven
# sources from the master plan are now registered: ``--source`` choices, the
# default all-sources set, and the aggregation loop all derive from this dict.
#
# Type is ``Callable[..., ...]`` rather than ``Callable[[], ...]`` because
# ``scan_appdata`` (and, less commonly, ``scan_docker_wsl``) accept optional
# keyword arguments (``exclude_paths=`` / ``base=``) that ``main()`` passes
# explicitly in the ``docker-wsl`` + ``appdata`` special case below; every
# other call site still invokes each scanner with zero arguments.
SCANNERS: dict[str, Callable[..., list[InventoryItem]]] = {
    "registry": scan_registry,
    "appdata": scan_appdata,
    "dotfolder": scan_dotfolders,
    "programdata": scan_programdata,
    "scoop-choco": scan_scoop_choco,
    "path": scan_path,
    "docker-wsl": scan_docker_wsl,
}

# Sources that require ``winreg`` (Windows-only stdlib) to run at all. Each
# also raises ``WinregUnavailableError`` itself if called anyway, but
# pre-filtering here lets ``_resolve_active_sources`` return an empty list
# (rather than a list whose scanners are guaranteed to fail) so the
# no-active-source guard in ``main()`` can report a clear message.
_WINREG_DEPENDENT_SOURCES = frozenset({"registry", "path", "docker-wsl"})


def _resolve_active_sources(requested: list[str] | None) -> list[str]:
    """Resolve the ``--source`` CLI argument into the list of sources to run.

    ``requested is None`` (flag omitted entirely) means "all currently
    registered sources", i.e. ``list(SCANNERS)`` — all seven. A source is
    dropped from the active list (rather than raising) when its dependency
    is unavailable on this platform: today that means any of
    ``_WINREG_DEPENDENT_SOURCES`` when ``winreg`` (Windows-only stdlib)
    failed to import. An empty return value signals "nothing runnable" to
    the caller, which is the trigger for the non-zero-exit guard in
    ``main()``.
    """
    names = requested if requested is not None else list(SCANNERS)
    active: list[str] = []
    for name in names:
        if name in _WINREG_DEPENDENT_SOURCES and not WINREG_AVAILABLE:
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
            "Windows : registre Uninstall, AppData/dotfolders/ProgramData, "
            "Scoop/Chocolatey, entrées PATH (utilisateur + système, avec "
            "détection des entrées mortes) et fichiers .vhdx Docker/WSL2 "
            "(disques + distros WSL enregistrées) — sept sources au total."
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

    # Double-count avoidance (Part 3): a docker-wsl .vhdx can live nested
    # inside a LOCALAPPDATA first-level folder (Docker\wsl\ or
    # Packages\<PackageFamilyName>\LocalState\) that the generic appdata
    # scan would otherwise also sum. When both sources are active, run
    # docker-wsl first, collect every returned item's absolute path, and
    # thread that set into scan_appdata()'s exclude_paths= so those exact
    # bytes are skipped there and attributed only once, to docker-wsl. When
    # docker-wsl is not active (e.g. --source appdata alone), scan_appdata()
    # runs with no exclusions — a documented, accepted over-count in that
    # filtered view (see README.md).
    thread_docker_wsl_excludes = "docker-wsl" in active_sources and "appdata" in active_sources

    items: list[InventoryItem] = []
    appdata_exclude_paths: set[Path] | None = None

    if thread_docker_wsl_excludes:
        try:
            docker_wsl_items = SCANNERS["docker-wsl"]()
        except WinregUnavailableError as exc:
            print(f"Source 'docker-wsl' indisponible : {exc}", file=sys.stderr)
            docker_wsl_items = []
        items.extend(docker_wsl_items)
        appdata_exclude_paths = {
            Path(item.path).resolve() for item in docker_wsl_items if item.path
        }

    for name in active_sources:
        if thread_docker_wsl_excludes and name == "docker-wsl":
            continue  # already run above, first, to build appdata_exclude_paths
        scanner = SCANNERS[name]
        try:
            if thread_docker_wsl_excludes and name == "appdata":
                items.extend(scanner(exclude_paths=appdata_exclude_paths))
            else:
                items.extend(scanner())
        except WinregUnavailableError as exc:
            print(f"Source '{name}' indisponible : {exc}", file=sys.stderr)

    if args.json:
        payload = _render_json_payload(items, args.top)
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print(_render_text_report(items, args.top))

    return 0


if __name__ == "__main__":
    sys.exit(main())
