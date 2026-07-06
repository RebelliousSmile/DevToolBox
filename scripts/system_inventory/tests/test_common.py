"""Unit tests for scripts/system_inventory/common.py.

Import note (deliberate, do not "simplify" back to a bare/relative import):
this test module is invoked two ways by the project:

  1. ``python -m unittest discover -s scripts/system_inventory/tests -v``
     from the repo root (the master/part success condition). Discovery's
     ``-t`` (top-level dir) defaults to the ``-s`` start dir here, so each
     test module is imported as a *top-level* module named e.g.
     ``test_common`` — the ``tests/`` package's own ``__init__.py`` is not
     on the import path for it, which rules out a relative ``from ..common``
     import (no known parent package) and a bare ``from system_inventory
     .common`` import (that package name is not importable: only the tests
     dir and the repo root/cwd end up on ``sys.path``, not ``scripts/``).
  2. ``python -m unittest scripts.system_inventory.tests.test_common``
     from the repo root, which imports it as a dotted submodule instead.

  Both forms run with the repo root on ``sys.path`` (as ``cwd``/argv[0]
  dir), and ``scripts/`` has no ``__init__.py`` of its own so it works as
  an implicit namespace package. The one import spelling that resolves
  under both invocations is the fully qualified
  ``scripts.system_inventory.common`` — verified empirically against this
  exact Python 3.13 interpreter before writing these tests.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from scripts.system_inventory.common import (
    FILE_ATTRIBUTE_REPARSE_POINT,
    InventoryItem,
    dir_size_on_disk,
    format_report,
    human_size,
    sort_items,
    to_json_payload,
    total_bytes,
)


class HumanSizeTests(unittest.TestCase):
    def test_none_is_unknown(self):
        self.assertEqual(human_size(None), "unknown")

    def test_zero_bytes(self):
        self.assertEqual(human_size(0), "0 B")

    def test_bytes_below_kb_boundary(self):
        self.assertEqual(human_size(1023), "1023 B")

    def test_exact_kb_boundary(self):
        self.assertEqual(human_size(1024), "1.0 KB")

    def test_fractional_mb_value(self):
        self.assertEqual(human_size(int(1.5 * 1024 * 1024)), "1.5 MB")

    def test_exact_mb_boundary(self):
        self.assertEqual(human_size(1024 * 1024), "1.0 MB")

    def test_exact_gb_boundary(self):
        self.assertEqual(human_size(1024**3), "1.0 GB")

    def test_multi_gb_stays_in_gb_unit(self):
        # Part 1's human_size only defines B/KB/MB/GB (no TB tier).
        self.assertEqual(human_size(5 * 1024**4), "5120.0 GB")


class SortItemsTests(unittest.TestCase):
    def test_descending_by_size(self):
        small = InventoryItem("s", "small", None, 10, {})
        big = InventoryItem("s", "big", None, 1000, {})
        medium = InventoryItem("s", "medium", None, 100, {})
        result = sort_items([small, big, medium])
        self.assertEqual([item.name for item in result], ["big", "medium", "small"])

    def test_none_sizes_sort_last(self):
        known = InventoryItem("s", "known", None, 5, {})
        unknown = InventoryItem("s", "unknown", None, None, {})
        result = sort_items([unknown, known])
        self.assertEqual([item.name for item in result], ["known", "unknown"])

    def test_ties_break_by_name(self):
        b = InventoryItem("s", "bravo", None, 50, {})
        a = InventoryItem("s", "alpha", None, 50, {})
        result = sort_items([b, a])
        self.assertEqual([item.name for item in result], ["alpha", "bravo"])

    def test_multiple_none_sizes_tie_break_by_name_among_themselves(self):
        z = InventoryItem("s", "zulu", None, None, {})
        a = InventoryItem("s", "alpha", None, None, {})
        known = InventoryItem("s", "known", None, 1, {})
        result = sort_items([z, a, known])
        self.assertEqual([item.name for item in result], ["known", "alpha", "zulu"])


class TotalBytesTests(unittest.TestCase):
    def test_sums_known_sizes_and_ignores_none(self):
        items = [
            InventoryItem("s", "a", None, 10, {}),
            InventoryItem("s", "b", None, None, {}),
            InventoryItem("s", "c", None, 5, {}),
        ]
        self.assertEqual(total_bytes(items), 15)

    def test_empty_list_is_zero(self):
        self.assertEqual(total_bytes([]), 0)


class DirSizeOnDiskRealFilesystemTests(unittest.TestCase):
    def test_sums_nested_regular_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a").mkdir()
            (root / "a" / "b").mkdir()
            (root / "top.txt").write_bytes(b"x" * 10)
            (root / "a" / "mid.txt").write_bytes(b"x" * 20)
            (root / "a" / "b" / "deep.txt").write_bytes(b"x" * 30)

            self.assertEqual(dir_size_on_disk(root), 60)

    def test_exclude_paths_skips_nested_file_several_levels_deep(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a" / "b" / "c").mkdir(parents=True)
            (root / "keep_root.txt").write_bytes(b"x" * 13)
            (root / "a" / "keep_b.txt").write_bytes(b"x" * 11)
            (root / "a" / "b" / "keep_c.txt").write_bytes(b"x" * 7)
            deep_file = root / "a" / "b" / "c" / "excluded_deep.txt"
            deep_file.write_bytes(b"x" * 5000)

            exclude_paths = {deep_file.resolve()}
            total = dir_size_on_disk(root, exclude_paths=exclude_paths)

            # 13 + 11 + 7 = 31; the excluded 5000-byte file must not contribute.
            self.assertEqual(total, 31)

    def test_exclude_paths_skips_excluded_directory_subtree_entirely(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            excluded_dir = root / "excluded_subtree"
            excluded_dir.mkdir()
            (excluded_dir / "inside.txt").write_bytes(b"x" * 999)
            (root / "keep.txt").write_bytes(b"x" * 3)

            total = dir_size_on_disk(root, exclude_paths={excluded_dir.resolve()})

            self.assertEqual(total, 3)


class _FakeDirEntry:
    """Minimal os.DirEntry stand-in for reparse-point / permission-error tests."""

    def __init__(self, path, is_dir, size=0, attrs=0, stat_error=None):
        self.path = path
        self._is_dir = is_dir
        self._size = size
        self._attrs = attrs
        self._stat_error = stat_error

    def stat(self, *, follow_symlinks=False):
        if self._stat_error is not None:
            raise self._stat_error
        return SimpleNamespace(st_size=self._size, st_file_attributes=self._attrs)

    def is_dir(self, *, follow_symlinks=False):
        return self._is_dir


class DirSizeOnDiskFaultInjectionTests(unittest.TestCase):
    """Exercises reparse-point skipping and error tolerance without needing
    real Windows reparse points (which require elevated/dev-mode privileges
    to create) or a live locked/permission-denied file."""

    def test_reparse_point_and_errors_are_skipped_without_raising(self):
        root = "C:\\fake-root"
        normal_file = root + "\\normal.txt"
        reparse_dir = root + "\\reparse_link"
        denied_dir = root + "\\denied"
        locked_file = root + "\\locked.txt"

        root_entries = [
            _FakeDirEntry(normal_file, is_dir=False, size=100),
            _FakeDirEntry(reparse_dir, is_dir=True, attrs=FILE_ATTRIBUTE_REPARSE_POINT),
            _FakeDirEntry(denied_dir, is_dir=True),
            _FakeDirEntry(locked_file, is_dir=False, stat_error=PermissionError("denied")),
        ]

        def fake_scandir(path):
            key = str(path)
            if key == root:
                return iter(root_entries)
            if key == denied_dir:
                raise PermissionError("cannot list denied dir")
            raise AssertionError(f"unexpected scandir({key!r})")

        with patch(
            "scripts.system_inventory.common.os.scandir",
            side_effect=fake_scandir,
        ):
            total = dir_size_on_disk(root)

        # Only normal_file's 100 bytes should count: reparse_dir is skipped
        # and never listed, denied_dir raises on listing and is skipped,
        # locked_file raises on stat and is skipped. None of this raises.
        self.assertEqual(total, 100)


class FormatReportAndJsonPayloadTests(unittest.TestCase):
    def test_format_report_empty(self):
        self.assertEqual(format_report([]), "Aucun élément trouvé.")

    def test_format_report_includes_total(self):
        items = [InventoryItem("registry", "App", None, 2048, {})]
        report = format_report(items)
        self.assertIn("App", report)
        self.assertIn("2.0 KB", report)
        self.assertIn("Total: 2.0 KB (2048 bytes)", report)

    def test_to_json_payload_shape_and_order(self):
        small = InventoryItem("registry", "small", "C:\\small", 10, {"k": "v"})
        big = InventoryItem("registry", "big", None, 2000, {})
        unknown = InventoryItem("registry", "unknown", None, None, {})

        payload = to_json_payload([small, big, unknown])

        self.assertEqual(payload["total_bytes"], 2010)
        self.assertEqual(payload["total_human"], human_size(2010))
        names = [entry["name"] for entry in payload["items"]]
        self.assertEqual(names, ["big", "small", "unknown"])
        self.assertEqual(
            payload["items"][1],
            {
                "source": "registry",
                "name": "small",
                "path": "C:\\small",
                "size_bytes": 10,
                "detail": {"k": "v"},
            },
        )


if __name__ == "__main__":
    unittest.main()
