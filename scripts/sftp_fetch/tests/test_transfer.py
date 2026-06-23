import tempfile
import os
import unittest
from pathlib import Path
from unittest.mock import patch

from sftp_fetch import download_file, execute

from tests.helpers import FakeSFTP, FakeSSH


class TransferTests(unittest.TestCase):
    def test_download_file_writes_atomically(self):
        sftp = FakeSFTP({}, {"/remote/report.txt": b"new content"})
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "nested" / "report.txt"
            download_file(sftp, "/remote/report.txt", destination, overwrite=False)
            self.assertEqual(destination.read_bytes(), b"new content")
            self.assertFalse(destination.with_name("report.txt.part").exists())

    def test_download_file_refuses_existing_file_without_overwrite(self):
        sftp = FakeSFTP({}, {"/remote/report.txt": b"new content"})
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "report.txt"
            destination.write_bytes(b"old content")
            with self.assertRaises(FileExistsError):
                download_file(sftp, "/remote/report.txt", destination, overwrite=False)
            self.assertEqual(destination.read_bytes(), b"old content")

    def test_download_file_replaces_existing_file_when_enabled(self):
        sftp = FakeSFTP({}, {"/remote/report.txt": b"new content"})
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "report.txt"
            destination.write_bytes(b"old content")
            download_file(sftp, "/remote/report.txt", destination, overwrite=True)
            self.assertEqual(destination.read_bytes(), b"new content")

    def test_download_file_skips_same_size_and_mtime(self):
        sftp = FakeSFTP({}, {"/remote/report.txt": b"same content"})
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "report.txt"
            destination.write_bytes(b"same content")
            os.utime(destination, (1_700_000_000, 1_700_000_000))
            downloaded = download_file(
                sftp,
                "/remote/report.txt",
                destination,
                overwrite=True,
                remote_size=len(b"same content"),
                remote_mtime=1_700_000_000,
            )
            self.assertFalse(downloaded)

    def test_download_file_preserves_remote_mtime(self):
        sftp = FakeSFTP({}, {"/remote/report.txt": b"new content"})
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "report.txt"
            self.assertTrue(
                download_file(
                    sftp,
                    "/remote/report.txt",
                    destination,
                    overwrite=False,
                    remote_size=len(b"new content"),
                    remote_mtime=1_700_000_000,
                )
            )
            self.assertLess(abs(destination.stat().st_mtime - 1_700_000_000), 1)

    def test_execute_downloads_only_selected_entry_and_closes_connections(self):
        ssh = FakeSSH()
        sftp = FakeSFTP(
            {},
            {
                "/documents/Pro/report.txt": b"professional",
                "/documents/Perso/note.txt": b"personal",
            },
        )
        with tempfile.TemporaryDirectory() as directory:
            config = {
                "server": {"host": "example.test", "username": "alice"},
                "destination": directory,
                "overwrite": True,
                "parallel_downloads": 1,
                "downloads": [
                    {
                        "name": "pro",
                        "remote": "/documents/Pro/report.txt",
                        "local": "Pro/report.txt",
                    },
                    {
                        "name": "perso",
                        "remote": "/documents/Perso/note.txt",
                        "local": "Perso/note.txt",
                    },
                ],
            }
            with patch("sftp_fetch.connect", return_value=(ssh, sftp)):
                completed, failed = execute(config, selected_names=["pro"])
            self.assertEqual((completed, failed), (1, 0))
            self.assertEqual(
                (Path(directory) / "Pro" / "report.txt").read_bytes(),
                b"professional",
            )
            self.assertFalse((Path(directory) / "Perso" / "note.txt").exists())
        self.assertTrue(sftp.closed)
        self.assertTrue(ssh.closed)

    def test_execute_continues_after_missing_remote_when_configured(self):
        ssh = FakeSSH()
        sftp = FakeSFTP({}, {"/documents/present.txt": b"present"})
        with tempfile.TemporaryDirectory() as directory:
            config = {
                "server": {"host": "example.test", "username": "alice"},
                "destination": directory,
                "continue_on_error": True,
                "parallel_downloads": 1,
                "downloads": [
                    {"name": "missing", "remote": "/documents/missing.txt"},
                    {"name": "present", "remote": "/documents/present.txt"},
                ],
            }
            with patch("sftp_fetch.connect", return_value=(ssh, sftp)):
                with self.assertLogs("sftp_fetch", level="ERROR"):
                    self.assertEqual(execute(config), (1, 1))


if __name__ == "__main__":
    unittest.main()
