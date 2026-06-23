import unittest
from pathlib import PurePosixPath
from unittest.mock import patch

import sftp_fetch
from sftp_fetch import _directory_is_ignored, _read_gitignore_patterns, walk_remote

from tests.helpers import FakeGitWildMatchPattern, FakeSFTP


class TraversalTests(unittest.TestCase):
    def setUp(self):
        self.tree = {
            "/root": [
                (".git", "dir"),
                ("_code", "dir"),
                ("ignored", "dir"),
                ("kept", "dir"),
                (".gitignore", "file"),
                ("root.txt", "file"),
            ],
            "/root/.git": [("config", "file")],
            "/root/_code": [("main.rs", "file")],
            "/root/ignored": [("large.bin", "file")],
            "/root/kept": [("document.txt", "file")],
        }
        self.files = {"/root/.gitignore": b"ignored/\n"}

    def test_walk_excludes_git_code_and_gitignored_directories_by_default(self):
        sftp = FakeSFTP(self.tree, self.files)
        with patch.object(sftp_fetch, "GitWildMatchPattern", FakeGitWildMatchPattern):
            files = [
                item.remote for item in walk_remote(sftp, "/root", include_code=False)
            ]
        self.assertEqual(
            files,
            [
                "/root/kept/document.txt",
                "/root/.gitignore",
                "/root/root.txt",
            ],
        )

    def test_include_code_never_includes_git_or_gitignored_directories(self):
        sftp = FakeSFTP(self.tree, self.files)
        with patch.object(sftp_fetch, "GitWildMatchPattern", FakeGitWildMatchPattern):
            files = [
                item.remote for item in walk_remote(sftp, "/root", include_code=True)
            ]
        self.assertIn("/root/_code/main.rs", files)
        self.assertNotIn("/root/.git/config", files)
        self.assertNotIn("/root/ignored/large.bin", files)

    def test_nested_gitignore_applies_only_below_its_directory(self):
        tree = {
            "/root": [("project", "dir"), ("cache", "dir")],
            "/root/cache": [("outside.bin", "file")],
            "/root/project": [(".gitignore", "file"), ("cache", "dir")],
            "/root/project/cache": [("inside.bin", "file")],
        }
        files = {"/root/project/.gitignore": b"cache/\n"}
        with patch.object(sftp_fetch, "GitWildMatchPattern", FakeGitWildMatchPattern):
            result = [
                item.remote
                for item in walk_remote(FakeSFTP(tree, files), "/root", False)
            ]
        self.assertIn("/root/cache/outside.bin", result)
        self.assertNotIn("/root/project/cache/inside.bin", result)

    def test_negative_rule_overrides_an_earlier_rule(self):
        rules = [
            (PurePosixPath(), FakeGitWildMatchPattern("generated/")),
            (PurePosixPath(), FakeGitWildMatchPattern("!generated/")),
        ]
        self.assertFalse(_directory_is_ignored(PurePosixPath("generated"), rules))

    def test_oversized_gitignore_is_ignored(self):
        sftp = FakeSFTP(
            {"/root": [(".gitignore", "file")]},
            {"/root/.gitignore": b"a" * (1024 * 1024 + 1)},
        )
        with patch.object(sftp_fetch, "GitWildMatchPattern", FakeGitWildMatchPattern):
            with self.assertLogs("sftp_fetch", level="WARNING"):
                self.assertEqual(
                    _read_gitignore_patterns(sftp, "/root", PurePosixPath()), []
                )

    @unittest.skipIf(
        sftp_fetch.GitWildMatchPattern is None,
        "PathSpec n'est pas installé dans cet environnement de test",
    )
    def test_real_pathspec_patterns_support_negation(self):
        pattern_type = sftp_fetch.GitWildMatchPattern
        rules = [
            (PurePosixPath(), pattern_type("generated/")),
            (PurePosixPath(), pattern_type("!generated/")),
        ]
        self.assertFalse(_directory_is_ignored(PurePosixPath("generated"), rules))


if __name__ == "__main__":
    unittest.main()
