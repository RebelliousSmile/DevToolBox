from __future__ import annotations

import stat
from types import SimpleNamespace


class FakeSFTP:
    """In-memory SFTP tree implementing only the operations used by the script."""

    def __init__(self, tree: dict[str, list[tuple[str, str]]], files=None):
        self.tree = tree
        self.files = files or {}
        self.closed = False

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
