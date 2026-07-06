"""Shared model and helpers for the system inventory scanners.

This module has no OS-scanning entry point of its own: it defines the
``InventoryItem`` model, a safe (reparse-point-skipping, error-tolerant)
recursive directory-size helper, and the sort/report/JSON formatting shared
by every source (``registry``, ``appdata``, ``packages``, ``path_env``,
``docker_wsl`` — the latter four are added in Part 2/3).

Stdlib only. Never writes to disk, the registry, or the environment.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path

# Windows file attribute flag for reparse points (symlinks, junctions, mount
# points). Directory/file entries carrying this flag are never followed:
# following them can loop (self-referential junctions) or double-count bytes
# that live on another volume/mount.
FILE_ATTRIBUTE_REPARSE_POINT = 0x400

_UNITS = ("B", "KB", "MB", "GB")


@dataclass
class InventoryItem:
    """A single inventoried item, regardless of which source produced it."""

    source: str
    name: str
    path: str | None
    size_bytes: int | None
    detail: dict[str, str] = field(default_factory=dict)


def human_size(n: int | None) -> str:
    """Render a byte count as a human-readable ``B``/``KB``/``MB``/``GB`` string.

    ``None`` (unknown size) renders as ``"unknown"``.
    """
    if n is None:
        return "unknown"
    size = float(n)
    unit_index = 0
    while size >= 1024 and unit_index < len(_UNITS) - 1:
        size /= 1024
        unit_index += 1
    unit = _UNITS[unit_index]
    if unit == "B":
        return f"{int(size)} {unit}"
    return f"{size:.1f} {unit}"


def dir_size_on_disk(
    path: str | os.PathLike[str],
    exclude_paths: set[Path] | None = None,
) -> int:
    """Sum the on-disk size of every regular file under ``path``.

    Walks iteratively (no recursion, so no stack-depth risk on deep trees).
    Any entry (file or directory) whose absolute path matches one in
    ``exclude_paths`` is skipped entirely: a directory match is not recursed
    into, a file match is not summed. This lets a source that claims a
    specific file or subtree exactly (e.g. a Docker/WSL ``.vhdx`` owned by
    the ``docker_wsl`` source) opt out of being re-summed by a broader
    first-level folder scan.

    The exclusion match is a lexical, case-insensitive string comparison
    (``os.path.normcase(os.path.normpath(...))`` on both sides) rather than
    a real ``Path.resolve()`` per entry. This is deliberate: ``resolve()``
    performs a filesystem round-trip (it opens the entry to follow any
    symlink chain), and calling it for *every single entry* of a full
    ``%LOCALAPPDATA%``/``%APPDATA%`` walk — hundreds of thousands of files
    on a real dev machine — turned a sub-minute scan into one that could run
    for many minutes as soon as ``exclude_paths`` was non-empty (the
    ``docker-wsl`` source populating it is exactly what first exercised this
    at scale). A plain lexical comparison is sound here because any entry
    reached by this walk is, by construction, never itself inside a
    symlinked/junctioned directory: a reparse-point directory is skipped
    below and never pushed onto the walk stack, so entries never need
    symlink-following to compare correctly against the (already-resolved)
    paths callers pass in ``exclude_paths``.

    Entries carrying ``FILE_ATTRIBUTE_REPARSE_POINT`` (symlinks, junctions,
    mount points) are skipped and never followed. Any ``OSError`` /
    ``PermissionError`` raised while listing or inspecting an entry is
    swallowed and the walk continues with the remaining entries.
    """
    excluded = exclude_paths or set()
    excluded_keys = {os.path.normcase(os.path.normpath(str(p))) for p in excluded}
    total = 0
    stack: list[Path] = [Path(path)]
    while stack:
        current = stack.pop()
        try:
            entries = list(os.scandir(current))
        except (OSError, PermissionError):
            continue
        for entry in entries:
            try:
                entry_path = Path(entry.path)
                if excluded_keys:
                    entry_key = os.path.normcase(os.path.normpath(str(entry_path)))
                    if entry_key in excluded_keys:
                        continue
                stat_result = entry.stat(follow_symlinks=False)
                attrs = getattr(stat_result, "st_file_attributes", 0)
                if attrs & FILE_ATTRIBUTE_REPARSE_POINT:
                    continue
                if entry.is_dir(follow_symlinks=False):
                    stack.append(entry_path)
                else:
                    total += stat_result.st_size
            except (OSError, PermissionError):
                continue
    return total


def sort_items(items: list[InventoryItem]) -> list[InventoryItem]:
    """Sort by descending ``size_bytes``; ``None`` sizes sort last; ties break by name."""

    def key(item: InventoryItem) -> tuple[bool, int, str]:
        is_unknown = item.size_bytes is None
        return (is_unknown, -(item.size_bytes or 0), item.name)

    return sorted(items, key=key)


def total_bytes(items: list[InventoryItem]) -> int:
    """Sum of all known ``size_bytes`` (``None`` sizes contribute 0)."""
    return sum(item.size_bytes or 0 for item in items)


def format_report(items: list[InventoryItem]) -> str:
    """Render a human-readable text report, sorted, with a grand total."""
    if not items:
        return "Aucun élément trouvé."
    lines: list[str] = []
    for item in sort_items(items):
        size = human_size(item.size_bytes)
        location = item.path or "-"
        lines.append(f"[{item.source}] {item.name} — {size} ({location})")
    total = total_bytes(items)
    lines.append("")
    lines.append(f"Total: {human_size(total)} ({total} bytes)")
    return "\n".join(lines)


def to_json_payload(items: list[InventoryItem]) -> dict[str, object]:
    """Render the sorted items plus grand total as a JSON-serializable dict."""
    sorted_items = sort_items(items)
    total = total_bytes(items)
    return {
        "items": [
            {
                "source": item.source,
                "name": item.name,
                "path": item.path,
                "size_bytes": item.size_bytes,
                "detail": dict(item.detail),
            }
            for item in sorted_items
        ],
        "total_bytes": total,
        "total_human": human_size(total),
    }
