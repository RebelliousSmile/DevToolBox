from __future__ import annotations

import fnmatch
import io
import posixpath
import stat
from types import SimpleNamespace


class FakeGitWildMatchPattern:
    """Small deterministic substitute used to isolate traversal tests from PathSpec."""

    def __init__(self, line: str):
        line = line.strip()
        if not line or line.startswith("#"):
            self.include = None
            self.pattern = ""
            return
        self.include = not line.startswith("!")
        self.pattern = line.lstrip("!").lstrip("/").rstrip("/")

    def match_file(self, path: str):
        candidate = path.rstrip("/")
        if fnmatch.fnmatch(candidate, self.pattern):
            return object()
        return None


class FakeSFTP:
    """In-memory SFTP tree implementing only the operations used by the script."""

    def __init__(self, tree: dict[str, list[tuple[str, str]]], files=None):
        self.tree = tree
        self.files = files or {}
        self.closed = False

    def listdir_attr(self, remote: str):
        return [
            SimpleNamespace(
                filename=name,
                st_mode=(stat.S_IFDIR | 0o755) if kind == "dir" else (stat.S_IFREG | 0o644),
                st_size=len(self.files.get(posixpath.join(remote, name), b"")),
                st_mtime=1_700_000_000,
            )
            for name, kind in self.tree[remote]
        ]

    def open(self, remote: str, mode: str):
        if remote not in self.files:
            raise OSError(f"missing remote file: {remote}")
        return io.BytesIO(self.files[remote])

    def stat(self, remote: str):
        if remote in self.tree:
            return SimpleNamespace(st_mode=stat.S_IFDIR | 0o755)
        if remote in self.files:
            return SimpleNamespace(
                st_mode=stat.S_IFREG | 0o644,
                st_size=len(self.files[remote]),
                st_mtime=1_700_000_000,
            )
        raise OSError(f"missing remote path: {remote}")

    def get(self, remote: str, local: str):
        with open(local, "wb") as stream:
            stream.write(self.files[remote])

    def close(self):
        self.closed = True


class FakeSSH:
    def __init__(self):
        self.closed = False

    def close(self):
        self.closed = True


def remote_file(tree: dict[str, list[tuple[str, str]]], parent: str, name: str):
    tree.setdefault(parent, []).append((name, "file"))
    return posixpath.join(parent, name)
