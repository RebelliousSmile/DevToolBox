"""Tests du journal des runs destructeurs.

Ce qui est vérifié ici tient en quatre propriétés, et chacune correspond à une
manière connue de rendre un journal d'audit trompeur :

1. `null` et `0` ne se confondent pas à l'aller-retour ;
2. `trim` coupe par **lignes**, pas par enregistrements analysés, donc une ligne
   corrompue survit à l'élagage au lieu d'être silencieusement supprimée ;
3. seule une **tentative** de suppression écrit une ligne — pas un total d'octets
   non nul, qui manquerait le run `moderate` et `docker-light` ;
4. une simulation n'écrit rien.
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import clean, history  # noqa: E402
from scripts.winclean.common import CleanResult, Level, RunReport  # noqa: E402
from scripts.winclean.tests.test_clean import (  # noqa: E402
    candidate,
    docker_stub,
    fake_module,
    recycle_stub,
    registry_of,
    run_cli,
)
from scripts.winclean.tests.test_mod_dev import tempdir, write  # noqa: E402


def result(
    module: str,
    *,
    estimated: int | None = None,
    freed: int | None = None,
    recycled: int | None = None,
    failed: int | None = None,
) -> CleanResult:
    return CleanResult(
        module=module,
        estimated=estimated,
        freed=freed,
        recycled=recycled,
        failed=failed,
    )


def write_text(path: Path, text: str) -> Path:
    """Écrit un journal **texte**. `write()` de `test_mod_dev` prend une taille."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    return path


def lines_of(path: Path) -> list[str]:
    return [line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def records_of(path: Path) -> list[dict]:
    return [json.loads(line) for line in lines_of(path)]


# --------------------------------------------------------------------------- #
# Emplacement
# --------------------------------------------------------------------------- #


class TestLocation(unittest.TestCase):
    def test_default_path_sits_under_localappdata(self):
        target = history.default_history_path({"LOCALAPPDATA": r"C:\Users\x\AppData\Local"})
        self.assertEqual(
            target, Path(r"C:\Users\x\AppData\Local") / "winclean" / "history.jsonl"
        )

    def test_a_machine_journal_is_not_roaming(self):
        """`%APPDATA%` synchronise entre postes ; deux machines s'y écraseraient."""
        target = history.default_history_path(
            {"APPDATA": r"C:\Roaming", "LOCALAPPDATA": r"C:\Local"}
        )
        self.assertIsNotNone(target)
        self.assertIn("Local", str(target))
        self.assertNotIn("Roaming", str(target))

    def test_absent_variable_yields_no_location(self):
        self.assertIsNone(history.default_history_path({}))

    def test_resolve_raises_without_a_known_location(self):
        with self.assertRaises(history.HistoryError):
            history.resolve_path(None, {})

    def test_read_runs_is_empty_without_a_known_location(self):
        self.assertEqual(history.read_runs(3, None, env={}), [])


# --------------------------------------------------------------------------- #
# Enregistrement
# --------------------------------------------------------------------------- #


class TestBuildRecord(unittest.TestCase):
    def test_keys_are_exactly_the_declared_set(self):
        report = RunReport(results=[result("pycache", estimated=10, freed=10)], estimated_total=10)
        record = history.build_record(Level.SAFE, report)
        self.assertEqual(tuple(record), history.RECORD_KEYS)

    def test_no_mode_field_is_written(self):
        """Une simulation n'écrit rien : la colonne serait constante et trompeuse."""
        report = RunReport(results=[result("pycache", freed=1)])
        record = history.build_record("safe", report)
        self.assertNotIn("mode", record)
        for name in record:
            self.assertNotIn("mode", name)

    def test_level_accepts_the_enum_and_the_string(self):
        report = RunReport()
        self.assertEqual(history.build_record(Level.MODERATE, report)["level"], "moderate")
        self.assertEqual(history.build_record("aggressive", report)["level"], "aggressive")

    def test_status_follows_the_report(self):
        self.assertEqual(history.build_record("safe", RunReport())["status"], "completed")
        self.assertEqual(
            history.build_record("safe", RunReport(interrupted=True))["status"], "interrupted"
        )

    def test_timestamp_is_utc_with_a_z(self):
        record = history.build_record("safe", RunReport())
        self.assertRegex(record["timestamp"], r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")

    def test_an_unmeasurable_total_stays_null(self):
        report = RunReport(results=[result("docker-light")], estimated_total=None)
        record = history.build_record("moderate", report)
        self.assertIsNone(record["freed_bytes"])
        self.assertIsNone(record["recycled_bytes"])
        self.assertIsNone(record["estimated_bytes"])

    def test_zero_is_not_null(self):
        report = RunReport(
            results=[result("pycache", estimated=0, freed=0, recycled=0, failed=0)],
            estimated_total=0,
        )
        record = history.build_record("safe", report)
        for key in ("estimated_bytes", "freed_bytes", "recycled_bytes", "failed_bytes"):
            self.assertEqual(record[key], 0, key)
            self.assertIsNotNone(record[key], key)

    def test_modules_carry_the_pair(self):
        report = RunReport(results=[result("pycache", estimated=42, freed=40)])
        modules = history.build_record("safe", report)["modules"]
        self.assertEqual(set(modules), {"pycache"})
        self.assertEqual(modules["pycache"]["estimated"], 42)
        self.assertIn("measured", modules["pycache"])


# --------------------------------------------------------------------------- #
# Aller-retour
# --------------------------------------------------------------------------- #


class TestRoundTrip(unittest.TestCase):
    def test_null_and_zero_survive_the_round_trip(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        report = RunReport(
            results=[
                result("pycache", estimated=0, freed=0, recycled=0, failed=0),
                result("docker-light"),
            ],
            estimated_total=0,
        )
        history.append_run(history.build_record("safe", report), target)
        (back,) = history.read_runs(None, target)
        self.assertEqual(back["estimated_bytes"], 0)
        self.assertIsNone(back["modules"]["docker-light"]["estimated"])
        self.assertEqual(back["modules"]["pycache"]["estimated"], 0)

    def test_append_creates_the_directory(self):
        root = tempdir(self)
        target = Path(root) / "winclean" / "history.jsonl"
        history.append_run({"timestamp": "2026-01-01T00:00:00Z"}, target)
        self.assertTrue(target.exists())

    def test_each_append_adds_exactly_one_line(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        for index in range(3):
            history.append_run({"n": index}, target)
        self.assertEqual([json.loads(line)["n"] for line in lines_of(target)], [0, 1, 2])

    def test_an_unserialisable_record_leaves_no_partial_line(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        history.append_run({"n": 0}, target)
        with self.assertRaises(TypeError):
            history.append_run({"n": object()}, target)
        self.assertEqual(lines_of(target), ['{"n": 0}'])


# --------------------------------------------------------------------------- #
# Élagage
# --------------------------------------------------------------------------- #


class TestTrim(unittest.TestCase):
    def test_it_caps_at_five_hundred_keeping_the_last_lines_in_file_order(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, "".join(json.dumps({"n": index}) + "\n" for index in range(600)))
        kept = history.trim(history.HISTORY_MAX_LINES, target)
        self.assertEqual(kept, 500)
        numbers = [json.loads(line)["n"] for line in lines_of(target)]
        self.assertEqual(len(numbers), 500)
        self.assertEqual(numbers[0], 100)
        self.assertEqual(numbers[-1], 599)
        self.assertEqual(numbers, sorted(numbers))

    def test_a_corrupted_line_survives_a_trim(self):
        """`trim` coupe des lignes, il n'analyse rien : voir `history.py`.

        Un élagage qui trierait par `timestamp` supprimerait exactement les lignes
        que `read_runs` a pour consigne de tolérer.
        """
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, '{"n": 1}\nligne tronquee {"n":\n{"n": 3}\n')
        history.trim(2, target)
        self.assertEqual(lines_of(target), ['ligne tronquee {"n":', '{"n": 3}'])

    def test_a_short_file_is_left_alone(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, '{"n": 1}\n')
        self.assertEqual(history.trim(500, target), 1)
        self.assertEqual(lines_of(target), ['{"n": 1}'])

    def test_an_absent_file_is_not_an_error(self):
        root = tempdir(self)
        self.assertEqual(history.trim(500, Path(root) / "absent.jsonl"), 0)

    def test_no_temporary_file_is_left_behind(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, "".join(json.dumps({"n": i}) + "\n" for i in range(10)))
        history.trim(3, target)
        self.assertEqual(
            sorted(p.name for p in Path(root).iterdir()),
            ["history.jsonl"],
        )

    def test_append_trims(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, "".join(json.dumps({"n": i}) + "\n" for i in range(10)))
        history.append_run({"n": 10}, target, max_lines=4)
        self.assertEqual([json.loads(line)["n"] for line in lines_of(target)], [7, 8, 9, 10])


# --------------------------------------------------------------------------- #
# Lecture
# --------------------------------------------------------------------------- #


class TestReadRuns(unittest.TestCase):
    def test_a_corrupted_middle_line_is_skipped_and_reported(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, '{"n": 1}\nligne tronquee {"n":\n{"n": 3}\n')
        notices: list[str] = []
        records = history.read_runs(None, target, notice=notices.append)
        self.assertEqual([r["n"] for r in records], [1, 3])
        self.assertEqual(len(notices), 1)
        self.assertIn("ligne 2", notices[0])

    def test_a_json_scalar_line_is_not_a_record(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, '{"n": 1}\n42\n')
        notices: list[str] = []
        records = history.read_runs(None, target, notice=notices.append)
        self.assertEqual([r["n"] for r in records], [1])
        self.assertEqual(len(notices), 1)

    def test_the_limit_takes_the_most_recent(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, "".join(json.dumps({"n": i}) + "\n" for i in range(5)))
        self.assertEqual([r["n"] for r in history.read_runs(2, target)], [3, 4])

    def test_an_empty_file_yields_nothing(self):
        root = tempdir(self)
        target = Path(root) / "history.jsonl"
        write_text(target, "")
        self.assertEqual(history.read_runs(3, target), [])

    def test_an_absent_file_yields_nothing(self):
        root = tempdir(self)
        self.assertEqual(history.read_runs(3, Path(root) / "absent.jsonl"), [])


class TestFormatHistory(unittest.TestCase):
    def test_nothing_recorded_says_so(self):
        text = history.format_history([], Path(r"C:\Local\winclean\history.jsonl"))
        self.assertIn("Aucun run enregistré", text)
        self.assertIn("history.jsonl", text)

    def test_an_unknown_total_is_never_shown_as_zero(self):
        report = RunReport(results=[result("docker-light")])
        text = history.format_history([history.build_record("moderate", report)])
        self.assertIn("unknown", text)
        self.assertIn("moderate", text)

    def test_the_status_is_shown_in_french(self):
        report = RunReport(results=[result("pycache", freed=1)], interrupted=True)
        text = history.format_history([history.build_record("safe", report)])
        self.assertIn("interrompu", text)


# --------------------------------------------------------------------------- #
# Déclencheur : une tentative de suppression, pas un total d'octets
# --------------------------------------------------------------------------- #


class TestWrittenOnlyByADestructiveRun(unittest.TestCase):
    def journal(self) -> Path:
        return Path(tempdir(self)) / "winclean" / "history.jsonl"

    def test_a_dry_run_appends_nothing(self):
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 500)
        journal = self.journal()
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 500),))
        with registry_of(module):
            code, _out, err = run_cli(["--root", str(root)], history_path=journal)
        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertFalse(journal.exists())

    def test_an_apply_that_deleted_appends_exactly_one_line(self):
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 500)
        journal = self.journal()
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 500),))
        with registry_of(module):
            code, _out, err = run_cli(
                ["--root", str(root), "--apply", "--yes"], history_path=journal
            )
        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        records = records_of(journal)
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["status"], "completed")
        self.assertEqual(records[0]["level"], "safe")
        self.assertEqual(records[0]["freed_bytes"], 500)
        self.assertEqual(set(records[0]["modules"]), {"fake-a"})

    def test_a_recycling_run_freed_zero_is_still_recorded(self):
        """`freed = 0, recycled > 0` : le run `moderate` typique.

        C'est le cas qu'un déclencheur fondé sur les octets libérés perdrait.
        """
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 640)
        journal = self.journal()
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 640),))
        with registry_of(module), recycle_stub() as recycled:
            code, _out, err = run_cli(
                ["--root", str(root), "--level", "moderate", "--apply", "--yes"],
                history_path=journal,
            )
        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertEqual(recycled["recycle"], [str(target)])
        (record,) = records_of(journal)
        self.assertEqual(record["status"], "completed")
        self.assertEqual(record["level"], "moderate")
        self.assertEqual(record["freed_bytes"], 0)
        self.assertEqual(record["recycled_bytes"], 640)

    def test_an_unmeasurable_module_is_recorded_with_a_null_total(self):
        """`--only docker-light` : le `prune` réussit et ne rend aucun octet.

        Second cas perdu par un déclencheur fondé sur les octets : ici il n'y a
        aucun octet *à* mesurer, et la suppression a bien eu lieu.
        """
        journal = self.journal()
        with docker_stub():
            code, _out, err = run_cli(
                [
                    "--level",
                    "moderate",
                    "--only",
                    "docker-light",
                    "--apply",
                    "--yes",
                ],
                history_path=journal,
            )
        self.assertEqual(code, 0, err)
        (record,) = records_of(journal)
        self.assertEqual(record["status"], "completed")
        self.assertIsNone(record["freed_bytes"])
        self.assertEqual(set(record["modules"]), {"docker-light"})

    def test_a_run_whose_every_candidate_vanished_appends_nothing(self):
        """Aucune tentative : le chemin avait disparu entre le plan et l'application."""
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 500)
        gone = candidate("fake-a", target, 500)  # capture l'horodatage réel
        for child in (target / "fichier.txt",):
            child.unlink()
        target.rmdir()
        journal = self.journal()
        module = fake_module("fake-a", candidates=(gone,))
        with registry_of(module):
            code, out, err = run_cli(
                ["--root", str(root), "--apply", "--yes"], history_path=journal
            )
        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertIn("skipped-gone", out)
        self.assertFalse(journal.exists())

    def test_an_interrupted_run_that_already_wrote_is_recorded(self):
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 300)
        journal = self.journal()

        def _interrupt(**_kwargs):
            raise KeyboardInterrupt

        first = fake_module("fake-a", candidates=(candidate("fake-a", target, 300),))
        second = fake_module(
            "fake-b",
            candidates=(candidate("fake-b", None, 100, label="fake-b sans chemin"),),
            clean_fn=_interrupt,
        )
        with registry_of(first, second):
            code, _out, err = run_cli(
                ["--root", str(root), "--apply", "--yes"], history_path=journal
            )
        self.assertEqual(code, clean.EXIT_INTERRUPTED)
        self.assertIn("Interruption", err)
        (record,) = records_of(journal)
        self.assertEqual(record["status"], "interrupted")
        self.assertEqual(record["freed_bytes"], 300)

    def test_a_refused_recycle_alone_appends_nothing(self):
        """La corbeille a refusé le chemin : rien n'a été tenté sur lui."""
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 400)
        journal = self.journal()
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 400),))
        with registry_of(module), recycle_stub(can=False) as recycled:
            code, out, err = run_cli(
                ["--root", str(root), "--level", "moderate", "--apply", "--yes"],
                history_path=journal,
            )
        self.assertEqual(code, 0, err)
        self.assertEqual(recycled["recycle"], [])
        self.assertIn("[no-undo]", out)
        self.assertFalse(journal.exists())

    def test_a_failing_journal_is_a_warning_not_a_status(self):
        """Le run a supprimé ; ne pas savoir l'écrire ne change pas son statut.

        L'échec est réel et non simulé : le journal est demandé *sous* un fichier
        ordinaire, donc son `mkdir` lève. Un `raise` échappé d'ici masquerait le
        chemin de sortie du run.
        """
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 200)
        obstacle = write(Path(root) / "obstacle", 1)
        journal = obstacle / "history.jsonl"
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 200),))
        with registry_of(module):
            code, _out, err = run_cli(
                ["--root", str(root), "--apply", "--yes"], history_path=journal
            )
        self.assertEqual(code, 0)
        self.assertIn("Historique non écrit", err)
        self.assertFalse(target.exists())
        self.assertTrue(obstacle.is_file())


# --------------------------------------------------------------------------- #
# Mode requête `--history`
# --------------------------------------------------------------------------- #


class TestHistoryQueryMode(unittest.TestCase):
    def test_it_reads_back_what_a_run_wrote(self):
        root = tempdir(self)
        target = Path(root) / "target"
        write(target / "fichier.txt", 500)
        journal = Path(root) / "winclean" / "history.jsonl"
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 500),))
        with registry_of(module):
            run_cli(["--root", str(root), "--apply", "--yes"], history_path=journal)
        code, out, err = run_cli(["--history", "1"], history_path=journal)
        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertIn("fake-a", out)
        self.assertIn("safe", out)
        self.assertIn("terminé", out)

    def test_json_emits_the_raw_records(self):
        root = tempdir(self)
        journal = Path(root) / "history.jsonl"
        history.append_run({"timestamp": "2026-01-01T00:00:00Z", "level": "safe"}, journal)
        code, out, _err = run_cli(["--history", "5", "--json"], history_path=journal)
        self.assertEqual(code, 0)
        payload = json.loads(out)
        self.assertEqual(payload["history"][0]["level"], "safe")
        self.assertIn("history.jsonl", payload["path"])

    def test_a_corrupted_line_does_not_break_the_query(self):
        root = tempdir(self)
        journal = Path(root) / "history.jsonl"
        write_text(journal, 'ligne tronquee {"n":\n{"timestamp": "2026-01-01T00:00:00Z"}\n')
        code, out, err = run_cli(["--history", "5"], history_path=journal)
        self.assertEqual(code, 0)
        self.assertIn("2026-01-01T00:00:00Z", out)
        self.assertIn("ligne 1", err)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
