"""Tests de la couche de sûreté (Phase 1, tâche 5)."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean.common import CleanCandidate, Level, Plan  # noqa: E402
from scripts.winclean.guards import (  # noqa: E402
    CeilingExceeded,
    DEFAULT_PROTECTED,
    absorb_nested,
    default_protected,
    enforce_ceiling,
    is_protected,
    normalise,
    path_sanity,
    screen_candidates,
)

MIB = 1024 * 1024
IS_WINDOWS = sys.platform == "win32"


def candidate(
    label: str,
    path: str | None,
    estimated: int | None = 1024,
    *,
    module: str = "mod",
) -> CleanCandidate:
    return CleanCandidate(
        module=module,
        path=path,
        label=label,
        estimated_bytes=estimated,
        level=Level.SAFE,
        reason="régénérable",
    )


def make_junction(link: Path, target: Path) -> bool:
    """Crée une jonction NTFS. `False` si l'environnement ne le permet pas."""
    try:
        completed = subprocess.run(
            ["cmd", "/c", "mklink", "/J", str(link), str(target)],
            capture_output=True,
            check=False,
        )
    except OSError:
        return False
    return completed.returncode == 0 and link.exists()


@unittest.skipUnless(IS_WINDOWS, "sémantique des chemins Windows")
class TestNormalise(unittest.TestCase):
    def test_is_idempotent_and_case_folded(self) -> None:
        once = normalise("C:\\Users\\X\\.cargo")
        self.assertEqual(once, normalise(once))
        self.assertEqual(once, normalise("c:/users/x/.CARGO"))

    def test_comparisons_reject_unnormalised_input(self) -> None:
        with self.assertRaises(ValueError):
            is_protected("C:\\Users\\X\\Documents", DEFAULT_PROTECTED)
        with self.assertRaises(ValueError):
            path_sanity("C:\\Users\\X\\Documents")


@unittest.skipUnless(IS_WINDOWS, "sémantique des chemins Windows")
class TestProtection(unittest.TestCase):
    def test_mixed_case_protection_holds(self) -> None:
        protected = (normalise("C:\\Users\\X\\.cargo"),)
        self.assertTrue(is_protected(normalise("c:\\users\\x\\.CARGO"), protected))
        self.assertTrue(is_protected(normalise("C:\\Users\\X\\.cargo\\registry"), protected))
        self.assertFalse(is_protected(normalise("C:\\Users\\X\\.cargo-other"), protected))
        self.assertFalse(is_protected(normalise("C:\\Users\\X"), protected))

    def test_junction_into_a_protected_tree_is_caught_after_resolution(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            real = Path(tmp) / "coffre"
            real.mkdir()
            link = Path(tmp) / "raccourci"
            if not make_junction(link, real):
                self.skipTest("jonction NTFS indisponible dans cet environnement")
            protected = (normalise(real),)
            self.assertTrue(is_protected(normalise(link), protected))


@unittest.skipUnless(IS_WINDOWS, "sémantique des chemins Windows")
class TestPathSanity(unittest.TestCase):
    def test_short_path_is_refused(self) -> None:
        self.assertEqual(path_sanity(normalise("C:\\a\\b")), "too-short")

    def test_profile_root_is_refused(self) -> None:
        profile = os.environ.get("USERPROFILE")
        if not profile:
            self.skipTest("%USERPROFILE% absent")
        self.assertEqual(path_sanity(normalise(profile)), "profile-root")

    def test_drive_root_is_refused(self) -> None:
        self.assertEqual(path_sanity(normalise("C:\\")), "drive-root")

    def test_unc_share_root_is_refused(self) -> None:
        self.assertEqual(
            path_sanity(normalise("\\\\winclean-nosuchhost\\partage")), "unc-share-root"
        )

    def test_a_plausible_target_passes(self) -> None:
        self.assertIsNone(path_sanity(normalise("C:\\dev\\projet\\target")))


@unittest.skipUnless(IS_WINDOWS, "sémantique des chemins Windows")
class TestScreenCandidates(unittest.TestCase):
    def test_returns_what_it_removed_with_a_reason_class(self) -> None:
        protected = (normalise("C:\\Users\\X\\Documents"),)
        kept, dropped = screen_candidates(
            [
                candidate("bon", "C:\\dev\\projet\\target"),
                candidate("protégé", "C:\\Users\\X\\Documents\\these"),
                candidate("racine", "C:\\"),
            ],
            protected=protected,
        )
        self.assertEqual([c.label for c in kept], ["bon"])
        self.assertEqual(
            [(d.label, d.reason_class, d.detail) for d in dropped],
            [("protégé", "protected", ""), ("racine", "sanity", "drive-root")],
        )

    def test_kept_paths_stay_in_display_form(self) -> None:
        kept, _dropped = screen_candidates(
            [candidate("bon", "C:\\dev\\Projet\\Target")], protected=()
        )
        self.assertEqual(kept[0].path, "C:\\dev\\Projet\\Target")


class TestPathlessCandidates(unittest.TestCase):
    def test_survives_the_whole_chain_untouched(self) -> None:
        pathless = candidate("docker (prune)", None, 5 * MIB, module="docker-light")
        kept, dropped = screen_candidates([pathless], protected=DEFAULT_PROTECTED)
        self.assertEqual(kept, [pathless])
        self.assertEqual(dropped, [])
        survivors, absorbed = absorb_nested(kept, rank={"docker-light": 0})
        self.assertEqual(survivors, [pathless])
        self.assertEqual(absorbed, [])

    def test_still_counts_toward_the_ceiling(self) -> None:
        pathless = candidate("docker (prune)", None, 5 * MIB, module="docker-light")
        total, warnings = enforce_ceiling([pathless], 10 * MIB)
        self.assertEqual(total, 5 * MIB)
        self.assertEqual(warnings, [])

    def test_empty_label_is_rejected_at_construction(self) -> None:
        with self.assertRaises(ValueError):
            CleanCandidate(
                module="docker-light",
                path=None,
                label="",
                estimated_bytes=None,
                level=Level.SAFE,
                reason="r",
            )


class TestCeiling(unittest.TestCase):
    def test_aborts_one_byte_over_the_limit(self) -> None:
        with self.assertRaises(CeilingExceeded) as raised:
            enforce_ceiling([candidate("gros", "C:\\dev\\a\\target", 1025)], 1024)
        message = str(raised.exception)
        self.assertIn("--max-delete-bytes", message)
        self.assertIn("1025", message)
        self.assertIn("1024", message)

    def test_exactly_at_the_limit_passes(self) -> None:
        total, _warnings = enforce_ceiling(
            [candidate("pile", "C:\\dev\\a\\target", 1024)], 1024
        )
        self.assertEqual(total, 1024)

    def test_unknown_estimate_neither_raises_nor_aborts_and_is_reported(self) -> None:
        candidates = [
            candidate("connu-1", "C:\\dev\\a\\target", 1000, module="cargo-target"),
            candidate("connu-2", "C:\\dev\\b\\target", 2000, module="cargo-target"),
            candidate("inconnu", None, None, module="docker-light"),
        ]
        total, warnings = enforce_ceiling(candidates, 10 * MIB)
        self.assertEqual(total, 3000)
        self.assertEqual([w.code for w in warnings], ["ceiling-total-partial"])
        self.assertIn("docker-light", warnings[0].fields["modules"])
        self.assertNotIn("cargo-target", warnings[0].fields["modules"])

    def test_plan_total_matches_the_ceiling_total(self) -> None:
        candidates = [
            candidate("a", "C:\\dev\\a\\target", 1000),
            candidate("b", None, None, module="docker-light"),
        ]
        plan = Plan(candidates=candidates)
        total, _warnings = enforce_ceiling(candidates, 10 * MIB)
        self.assertEqual(plan.total_estimated(), total)
        self.assertEqual(plan.unpriced_modules(), ["docker-light"])


@unittest.skipUnless(IS_WINDOWS, "profil utilisateur Windows")
class TestDefaultProtected(unittest.TestCase):
    def test_a_path_inside_a_user_data_root_is_protected(self) -> None:
        protected = default_protected(env={"USERPROFILE": "C:\\Users\\X"})
        for folder in ("Documents", "Desktop", "Pictures", "Videos", "Music", "Downloads"):
            self.assertTrue(
                is_protected(normalise(f"C:\\Users\\X\\{folder}\\fichier"), protected),
                folder,
            )

    def test_the_profile_root_itself_is_deliberately_absent(self) -> None:
        protected = default_protected(env={"USERPROFILE": "C:\\Users\\X"})
        self.assertNotIn(normalise("C:\\Users\\X"), protected)
        # Sinon `%LOCALAPPDATA%` et `%APPDATA%` seraient protégés par sous-arbre
        # et les modules de cache se videraient en silence.
        self.assertFalse(
            is_protected(normalise("C:\\Users\\X\\AppData\\Local\\npm-cache"), protected)
        )

    def test_never_written_as_a_literal_user_path(self) -> None:
        source = (Path(__file__).parent.parent / "guards.py").read_text(encoding="utf-8")
        self.assertNotIn("C:\\Users\\fxgui", source)

    def test_every_registry_candidate_survives_the_default_set(self) -> None:
        try:
            from scripts.winclean import registry_mod
        except ImportError:
            self.skipTest("registry_mod arrive en Phase 3 : moitié survie différée")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "fixture"
            root.mkdir()
            emitted: list[CleanCandidate] = []
            for name in registry_mod.MODULE_ORDER:
                module = registry_mod.MODULES[name]
                try:
                    emitted.extend(module.discover(roots=[root], max_depth=3))
                except TypeError:
                    emitted.extend(module.discover())
            for c in emitted:
                if c.path is None:
                    continue
                self.assertFalse(
                    is_protected(normalise(c.path), DEFAULT_PROTECTED),
                    f"{c.module} annulé en silence : {c.path}",
                )


@unittest.skipUnless(IS_WINDOWS, "sémantique des chemins Windows")
class TestAbsorbNested(unittest.TestCase):
    RANK = {"cargo-target": 0, "pycache": 1}

    def test_ancestor_keeps_the_candidate_and_bytes_count_once(self) -> None:
        target = candidate("target", "C:\\dev\\projet\\target", 1000, module="cargo-target")
        nested = candidate(
            "__pycache__", "C:\\dev\\projet\\target\\debug\\__pycache__", 300, module="pycache"
        )
        kept, absorbed = absorb_nested([nested, target], rank=self.RANK)
        self.assertEqual([c.label for c in kept], ["target"])
        self.assertEqual(len(absorbed), 1)
        self.assertEqual(absorbed[0].label, "__pycache__")
        self.assertEqual(absorbed[0].ancestor_label, "target")
        self.assertEqual(absorbed[0].ancestor_module, "cargo-target")
        self.assertEqual(Plan(candidates=kept).total_estimated(), 1000)

    def test_disjoint_candidates_are_both_kept(self) -> None:
        a = candidate("a", "C:\\dev\\a\\target", 10, module="cargo-target")
        b = candidate("b", "C:\\dev\\b\\target", 20, module="cargo-target")
        kept, absorbed = absorb_nested([a, b], rank=self.RANK)
        self.assertEqual(sorted(c.label for c in kept), ["a", "b"])
        self.assertEqual(absorbed, [])

    def test_exact_path_tie_goes_to_the_lower_rank_whatever_the_input_order(self) -> None:
        shared = "C:\\dev\\projet\\target"
        first = candidate("par cargo", shared, 100, module="cargo-target")
        second = candidate("par pycache", shared, 100, module="pycache")
        for order in ([first, second], [second, first]):
            kept, absorbed = absorb_nested(order, rank=self.RANK)
            self.assertEqual([c.module for c in kept], ["cargo-target"], order)
            self.assertEqual([a.module for a in absorbed], ["pycache"], order)

    def test_depth_alone_decides_nesting_not_the_rank(self) -> None:
        # L'ancêtre gagne même quand son module a le rang le plus élevé.
        ancestor = candidate("ancêtre", "C:\\dev\\projet", 500, module="pycache")
        descendant = candidate(
            "descendant", "C:\\dev\\projet\\target", 400, module="cargo-target"
        )
        kept, absorbed = absorb_nested([descendant, ancestor], rank=self.RANK)
        self.assertEqual([c.label for c in kept], ["ancêtre"])
        self.assertEqual([a.label for a in absorbed], ["descendant"])


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
