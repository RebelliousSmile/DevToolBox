import unittest
from unittest.mock import patch

from sftp_fetch import ConfigurationError, main


class CliTests(unittest.TestCase):
    @patch("sftp_fetch.execute", return_value=(3, 0))
    @patch("sftp_fetch.load_config", return_value={"downloads": ["/unused"]})
    def test_main_forwards_selection(self, _load, execute):
        result = main(
            [
                "config.yaml",
                "--only",
                "pro",
                "--only",
                "perso",
            ]
        )
        self.assertEqual(result, 0)
        execute.assert_called_once_with(
            {"downloads": ["/unused"]},
            dry_run=False,
            selected_names=["pro", "perso"],
        )

    @patch("sftp_fetch.execute", return_value=(2, 1))
    @patch("sftp_fetch.load_config", return_value={"downloads": ["/unused"]})
    def test_main_returns_one_when_a_download_failed(self, _load, _execute):
        self.assertEqual(main(["config.yaml"]), 1)

    @patch(
        "sftp_fetch.load_config",
        side_effect=ConfigurationError("invalid configuration"),
    )
    def test_main_returns_two_for_configuration_error(self, _load):
        with self.assertLogs("sftp_fetch", level="ERROR"):
            self.assertEqual(main(["config.yaml"]), 2)


if __name__ == "__main__":
    unittest.main()
