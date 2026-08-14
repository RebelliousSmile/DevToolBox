"""Tests du modèle, des agrégats et du rendu (Phase 1, tâches 2 à 4)."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean.common import (  # noqa: E402
    CleanCandidate,
    CompletedResource,
    CleanModule,
    CleanResult,
    CleanWarning,
    Level,
    Plan,
    RunReport,
    ModuleDiscoveryError,
    OperationFailure,
    SkippedEntry,
    SKIP_RUNNING,
    estimate_path,
    field_names,
    format_plan,
    format_result_report,
    human_size,
    render_warning,
    to_json_payload,
)


def candidate(
    label: str,
    estimated: int | None,
    *,
    module: str = "mod",
    path: str | None = "C:\\some\\where\\deep",
) -> CleanCandidate:
    return CleanCandidate(
        module=module,
        path=path,
        label=label,
        estimated_bytes=estimated,
        level=Level.SAFE,
        reason="régénérable",
    )


class TestModel(unittest.TestCase):
    def test_unknown_estimate_renders_unknown_and_sorts_last(self) -> None:
        plan = Plan(
            candidates=[
                candidate("inconnu", None),
                candidate("petit", 10),
                candidate("gros", 4096),
            ]
        )
        labels = [c.label for c in plan.sorted_candidates()]
        self.assertEqual(labels, ["gros", "petit", "inconnu"])
        self.assertEqual(human_size(None), "unknown")
        self.assertIn("unknown", format_plan(plan))

    def test_recycled_bytes_never_contribute_to_freed_total(self) -> None:
        report = RunReport(
            results=[
                CleanResult(module="a", freed=100, recycled=900),
                CleanResult(module="b", freed=None, recycled=50),
            ]
        )
        self.assertEqual(report.total_freed(), 100)
        self.assertEqual(report.total_recycled(), 950)
        payload = to_json_payload(report)
        self.assertEqual(payload["freed_total_bytes"], 100)
        self.assertEqual(payload["recycled_total_bytes"], 950)

    def test_candidate_defaults_but_module_has_none(self) -> None:
        # Le défaut est une propriété du *candidat* : une commodité de
        # construction, jamais un classement.
        c = candidate("x", 1)
        self.assertFalse(c.needs_network)
        self.assertIsNone(c.stat_mtime)
        self.assertIsNone(c.resource_id)
        # Sur le module, l'omission est un TypeError à l'import (décision 11).
        with self.assertRaises(TypeError):
            CleanModule(  # type: ignore[call-arg]
                name="m",
                level=Level.SAFE,
                requires=(),
                discover=lambda **_kw: [],
                clean=None,
            )

    def test_result_renders_unmeasurable_and_has_no_boolean_flag(self) -> None:
        text = format_result_report([CleanResult(module="a", freed=None)])
        self.assertIn("unknown", text)
        names = field_names(CleanResult)
        self.assertEqual(
            names,
            (
                "module",
                "estimated",
                "freed",
                "recycled",
                "failed",
                # Part 3 phase 4 : le second avis sur `estimated`, nullable aux
                # mêmes conditions que les colonnes d'octets.
                "measured",
                "skipped",
                # Part 2 : les deux classes de défaillance derrière la colonne
                # `failed`, séparées, et le compteur d'événements de corbeille.
                "locked_paths",
                "recycle_failed_paths",
                "recycle_events",
                "completed_resources",
                "operation_failures",
            ),
        )
        # `measured` est une **valeur** d'octets nullable, jamais un drapeau : ce
        # test interdisait tout nom contenant `measur` tant que le champ n'existait
        # pas, et l'interdiction visait en réalité les formes booléennes. Elle est
        # donc resserrée sur elles plutôt que levée — `measurable` ou `is_measured`
        # rouvriraient la porte que la doctrine « on interroge la valeur » ferme.
        self.assertEqual([n for n in names if "measur" in n], ["measured"])
        for name in names:
            self.assertNotIn("measurable", name)
            self.assertNotIn("unknown", name)

    def test_discovery_error_carries_a_stable_code(self) -> None:
        error = ModuleDiscoveryError("adapter-unavailable", "service indisponible")
        self.assertEqual(error.code, "adapter-unavailable")
        self.assertEqual(str(error), "service indisponible")

    def test_external_resource_outcomes_are_serialized_without_fake_bytes(self) -> None:
        result = CleanResult(
            module="external",
            completed_resources=[CompletedResource("one")],
            operation_failures=[OperationFailure("two", "api-error", "refusé")],
        )
        payload = result.to_json_payload()
        self.assertEqual(payload["completed_resources"], [{"resource_id": "one"}])
        self.assertEqual(
            payload["operation_failures"],
            [{"resource_id": "two", "code": "api-error", "reason": "refusé"}],
        )
        self.assertIsNone(payload["freed"])

    def test_empty_label_is_rejected_at_construction(self) -> None:
        for bad in ("", "   "):
            with self.assertRaises(ValueError):
                CleanCandidate(
                    module="m",
                    path=None,
                    label=bad,
                    estimated_bytes=None,
                    level=Level.SAFE,
                    reason="r",
                )


class TestSkipReporting(unittest.TestCase):
    def setUp(self) -> None:
        self.entry = SkippedEntry(
            label="target de devtoolbox",
            path="C:\\dev\\devtoolbox\\target",
            status=SKIP_RUNNING,
            reason="cargo.exe actif",
        )
        self.result = CleanResult(module="cargo-target", skipped=[self.entry])

    def test_text_report_names_label_status_and_reason(self) -> None:
        text = format_result_report([self.result])
        self.assertIn("Omis :", text)
        self.assertIn(self.entry.label, text)
        self.assertIn(SKIP_RUNNING, text)
        self.assertIn(self.entry.reason, text)

    def test_payload_carries_the_same_entry_under_its_own_key(self) -> None:
        payload = to_json_payload(self.result)
        self.assertEqual(
            payload["skipped"],
            [
                {
                    "label": self.entry.label,
                    "path": self.entry.path,
                    "status": SKIP_RUNNING,
                    "reason": self.entry.reason,
                }
            ],
        )

    def test_skip_contributes_to_no_byte_total(self) -> None:
        report = RunReport(results=[self.result])
        self.assertIsNone(report.total_freed())
        self.assertIsNone(report.total_recycled())
        self.assertIsNone(report.total_failed())
        self.assertNotIn("bytes", field_names(SkippedEntry))


class TestPayloadMirrorsData(unittest.TestCase):
    def test_free_space_for_two_volumes_is_in_text_and_payload(self) -> None:
        plan = Plan(
            candidates=[candidate("x", 1)],
            free_space={"C:\\": 1024, "D:\\": 2048},
        )
        text = format_plan(plan)
        self.assertIn("C:\\", text)
        self.assertIn("D:\\", text)
        payload = to_json_payload(plan)
        self.assertEqual(payload["free_space_before"], {"C:\\": 1024, "D:\\": 2048})

    def test_payload_holds_no_rendered_sentence(self) -> None:
        plan = Plan(candidates=[candidate("x", 1)])
        payload = to_json_payload(plan)
        self.assertEqual(
            set(payload),
            {
                "level",
                "apply",
                "roots",
                "candidates",
                "total_estimated_bytes",
                "unpriced_modules",
                "dropped",
                "absorbed",
                "excluded",
                "warnings",
                "free_space_before",
            },
        )
        prose = ("Simulation", "Niveau :", "Avertissement", "--apply", "Total estimé")
        flat = repr(payload)
        for fragment in prose:
            self.assertNotIn(fragment, flat)


class TestWarnings(unittest.TestCase):
    def test_warning_is_rendered_and_carried_with_its_fields(self) -> None:
        warning = CleanWarning(code="sync-root", fields={"root": "C:\\Users\\x\\Documents"})
        plan = Plan(candidates=[candidate("x", 1)], warnings=[warning])
        text = format_plan(plan)
        self.assertIn("Racine synchronisée", text)
        self.assertIn("C:\\Users\\x\\Documents", text)
        payload = to_json_payload(plan)
        self.assertEqual(
            payload["warnings"],
            [{"code": "sync-root", "root": "C:\\Users\\x\\Documents"}],
        )

    def test_no_warning_emits_an_empty_list_not_a_missing_key(self) -> None:
        payload = to_json_payload(Plan(candidates=[candidate("x", 1)]))
        self.assertIn("warnings", payload)
        self.assertEqual(payload["warnings"], [])
        report_payload = to_json_payload(RunReport())
        self.assertIn("warnings", report_payload)
        self.assertEqual(report_payload["warnings"], [])

    def test_unknown_code_still_renders_code_and_fields(self) -> None:
        rendered = render_warning(CleanWarning(code="brand-new", fields={"k": "v"}))
        self.assertIn("brand-new", rendered)
        self.assertIn("k=v", rendered)


class TestEstimation(unittest.TestCase):
    def test_unreadable_root_is_none_and_renders_unknown(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "denied"
            root.mkdir()
            (root / "f.bin").write_bytes(b"x" * 10)
            with mock.patch("os.scandir", side_effect=PermissionError(5, "denied")):
                estimated = estimate_path(root)
        self.assertIsNone(estimated)
        text = format_plan([candidate("refusé", estimated)])
        self.assertIn("unknown", text)
        self.assertNotIn("0 B", text)

    def test_empty_readable_directory_estimates_zero_and_renders_zero(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "vide"
            root.mkdir()
            estimated = estimate_path(root)
        self.assertEqual(estimated, 0)
        text = format_plan([candidate("vide", estimated)])
        self.assertIn("0 B", text)
        self.assertNotIn("unknown", text)

    def test_missing_path_is_unmeasurable(self) -> None:
        self.assertIsNone(estimate_path(Path(tempfile.gettempdir()) / "winclean-absent-xyz"))

    def test_file_is_priced_by_its_own_size(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "f.bin"
            target.write_bytes(b"y" * 7)
            self.assertEqual(estimate_path(target), 7)


class TestPlanSections(unittest.TestCase):
    def test_top_hides_rows_without_changing_the_total(self) -> None:
        plan = Plan(
            candidates=[candidate(f"c{i}", (i + 1) * 100) for i in range(5)],
            top=2,
        )
        text = format_plan(plan)
        self.assertEqual(plan.total_estimated(), 100 + 200 + 300 + 400 + 500)
        self.assertIn("masquée", text)
        self.assertIn("5 élément(s)", text)

    def test_dry_run_footer_is_prose_and_apply_is_announced(self) -> None:
        self.assertIn("Simulation", format_plan(Plan(candidates=[candidate("x", 1)])))
        self.assertIn(
            "--apply est actif",
            format_plan(Plan(candidates=[candidate("x", 1)], apply=True)),
        )


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
