import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import sftp_fetch
from sftp_fetch import (
    ConfigurationError,
    _env_secret,
    default_relative_path,
    load_config,
    parse_downloads,
    safe_local_path,
    select_downloads,
)


class FakeYaml:
    class YAMLError(Exception):
        pass

    @staticmethod
    def safe_load(text):
        return json.loads(text)


class ConfigurationTests(unittest.TestCase):
    def test_secret_is_read_from_named_environment_variable(self):
        with patch.dict(os.environ, {"SFTP_TEST_SECRET": "secret-value"}):
            self.assertEqual(
                _env_secret({"password_env": "SFTP_TEST_SECRET"}, "password_env"),
                "secret-value",
            )

    def test_missing_secret_environment_variable_is_rejected(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ConfigurationError, "SFTP_TEST_SECRET"):
                _env_secret({"password_env": "SFTP_TEST_SECRET"}, "password_env")

    def test_load_config_accepts_valid_document(self):
        document = {
            "server": {"host": "sftp.example.test", "username": "alice"},
            "downloads": ["/documents/report.pdf"],
        }
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.yaml"
            config.write_text(json.dumps(document), encoding="utf-8")
            with patch.object(sftp_fetch, "yaml", FakeYaml):
                self.assertEqual(load_config(config), document)

    def test_load_config_rejects_empty_download_list(self):
        document = {
            "server": {"host": "sftp.example.test", "username": "alice"},
            "downloads": [],
        }
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.yaml"
            config.write_text(json.dumps(document), encoding="utf-8")
            with patch.object(sftp_fetch, "yaml", FakeYaml):
                with self.assertRaisesRegex(ConfigurationError, "downloads"):
                    load_config(config)

    def test_short_and_detailed_downloads(self):
        parsed = parse_downloads(
            [
                "/exports/report.pdf",
                {
                    "name": "facture",
                    "remote": "/exports/factures/juin.csv",
                    "local": "compta/juin.csv",
                },
            ]
        )
        self.assertEqual(parsed[0].name, "report.pdf")
        self.assertEqual(parsed[0].remote, "/exports/report.pdf")
        self.assertEqual(parsed[1].name, "facture")
        self.assertEqual(parsed[1].local, Path("compta/juin.csv"))

    def test_duplicate_names_are_rejected_case_insensitively(self):
        with self.assertRaisesRegex(ConfigurationError, "nom unique"):
            parse_downloads(
                [
                    {"name": "Pro", "remote": "/documents/Pro"},
                    {"name": "pro", "remote": "/backup/Pro"},
                ]
            )

    def test_remote_path_must_be_absolute(self):
        with self.assertRaises(ConfigurationError):
            parse_downloads(["relative/file.txt"])

    def test_default_path_preserves_remote_tree(self):
        self.assertEqual(default_relative_path("/a/b.txt"), Path("a/b.txt"))

    def test_local_path_cannot_escape_destination(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ConfigurationError):
                safe_local_path(Path(directory), Path("../outside.txt"))

    def test_download_selection_is_case_insensitive(self):
        parsed = parse_downloads(
            [
                {"name": "Pro", "remote": "/documents/Pro"},
                {"name": "Perso", "remote": "/documents/Perso"},
            ]
        )
        selected = select_downloads(parsed, ["pro"])
        self.assertEqual([item.name for item in selected], ["Pro"])

    def test_unknown_download_selection_lists_available_names(self):
        parsed = parse_downloads([{"name": "Pro", "remote": "/documents/Pro"}])
        with self.assertRaisesRegex(ConfigurationError, "choix: Pro"):
            select_downloads(parsed, ["unknown"])


if __name__ == "__main__":
    unittest.main()
