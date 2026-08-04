"""Tests des modules `aggressive` : `recycle-bin` et `package-cache` (phase 3).

La corbeille est fabriquée de toutes pièces sous un dossier temporaire, avec la
structure réelle : `<volume>\\$Recycle.Bin\\<SID>\\$I…` + `$R…`. Le SID est celui
que le helper rend, donc il est corrigé ici — jamais celui du compte qui lance
les tests, sinon le test viserait la vraie corbeille de la machine.

Le type de volume est corrigé lui aussi : le dossier temporaire vit sur `C:`,
qui est bien `DRIVE_FIXED`, mais l'affirmer par accident laisserait le critère du
volume distant sans contrepartie.
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import mod_system, remove  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DEFAULT_TRASH_DAYS,
    CleanWarning,
    Level,
    render_warning,
)
from scripts.winclean.tests.test_mod_dev import tempdir, write  # noqa: E402

#: SID d'épreuve, et un second qui doit rester invisible.
OUR_SID = "S-1-5-21-1111111111-2222222222-3333333333-1001"
OTHER_SID = "S-1-5-21-1111111111-2222222222-3333333333-1002"

#: Version de format `$I` de Windows 10 et suivants.
INFO_VERSION = 2

NOW = datetime(2026, 8, 4, 12, 0, tzinfo=timezone.utc)

DRIVE_REMOTE = 4


def filetime(moment: datetime) -> int:
    """`FILETIME` d'un instant : ticks de 100 ns depuis 1601-01-01 UTC."""
    delta = moment - mod_system.FILETIME_EPOCH
    return int(delta.total_seconds() * 10_000_000)


def info_bytes(deleted_at: datetime, *, original_size: int = 0, version: int = INFO_VERSION) -> bytes:
    """En-tête `$I` : version, taille d'origine, horodatage, puis le nom.

    Les trois champs sont posés à leur décalage réel — c'est le point du test :
    un `$I` fabriqué avec l'horodatage ailleurs qu'à l'octet 16 validerait une
    lecture fausse.
    """
    header = (
        version.to_bytes(8, "little")
        + original_size.to_bytes(8, "little")
        + filetime(deleted_at).to_bytes(8, "little")
    )
    name = "C:\\Users\\test\\document.txt\x00".encode("utf-16-le")
    return header + len(name).to_bytes(4, "little") + name


def bin_dir(volume: Path, sid: str = OUR_SID) -> Path:
    path = volume / mod_system.RECYCLE_BIN_DIRNAME / sid
    path.mkdir(parents=True, exist_ok=True)
    return path


def recycled_file(
    volume: Path,
    ident: str,
    deleted_at: datetime,
    size: int = 400,
    *,
    sid: str = OUR_SID,
) -> tuple[Path, Path]:
    """Une paire `$I`/`$R` dont le `$R` est un fichier. Rend `(info, payload)`."""
    folder = bin_dir(volume, sid)
    info = folder / f"$I{ident}.txt"
    payload = folder / f"$R{ident}.txt"
    info.write_bytes(info_bytes(deleted_at, original_size=size))
    write(payload, size)
    return info, payload


def recycled_folder(
    volume: Path,
    ident: str,
    deleted_at: datetime,
    sizes: tuple[int, ...] = (500, 700),
    *,
    sid: str = OUR_SID,
) -> tuple[Path, Path]:
    """Une paire dont le `$R` est un **dossier** : sa taille ne peut venir que
    de `dir_size_on_disk`, jamais du champ de taille du `$I`."""
    folder = bin_dir(volume, sid)
    info = folder / f"$I{ident}"
    payload = folder / f"$R{ident}"
    # Taille d'origine du header délibérément fausse : si l'implémentation la
    # lisait, le test le verrait.
    info.write_bytes(info_bytes(deleted_at, original_size=7))
    for index, size in enumerate(sizes):
        write(payload / f"fichier-{index}.bin", size)
    return info, payload


class _BinCase(unittest.TestCase):
    """Socle : SID corrigé, volume déclaré local, `now` figé."""

    def setUp(self) -> None:
        self.volume = tempdir(self)
        patch_sid = mock.patch.object(mod_system, "current_user_sid", return_value=OUR_SID)
        patch_sid.start()
        self.addCleanup(patch_sid.stop)
        patch_type = mock.patch.object(
            remove, "_get_drive_type", return_value=remove.DRIVE_FIXED
        )
        patch_type.start()
        self.addCleanup(patch_type.stop)

    def discover(self, **kwargs: object) -> list:
        params: dict[str, object] = {
            "volumes": [self.volume],
            "now": NOW,
        }
        params.update(kwargs)
        return mod_system.discover_recycle_bin(**params)  # type: ignore[arg-type]

    def plan_text(self, notes: list[CleanWarning]) -> str:
        return "\n".join(render_warning(note) for note in notes)


class TestRecycleBinSelection(_BinCase):
    def test_only_the_old_entry_is_a_candidate(self) -> None:
        recycled_file(self.volume, "RECENT", NOW - timedelta(days=1))
        _info, old = recycled_file(self.volume, "ANCIEN", NOW - timedelta(days=30))
        found = self.discover()
        self.assertEqual([c.path for c in found], [str(old)])
        self.assertEqual([c.level for c in found], [Level.AGGRESSIVE])

    def test_another_users_sid_folder_is_never_a_candidate(self) -> None:
        _info, ours = recycled_file(self.volume, "NOTRE", NOW - timedelta(days=30))
        recycled_file(self.volume, "AUTRE", NOW - timedelta(days=30), sid=OTHER_SID)
        found = self.discover()
        self.assertEqual([c.path for c in found], [str(ours)])

    def test_a_bare_payload_without_its_info_sibling_yields_no_candidate(self) -> None:
        """Un `$R` orphelin n'est pas datable, donc jamais supposé ancien."""
        folder = bin_dir(self.volume)
        write(folder / "$RORPHELIN.txt", 900)
        self.assertEqual(self.discover(), [])

    def test_a_corrupt_info_header_falls_back_to_the_payload_mtime(self) -> None:
        """Fermé, pas ouvert : un `$I` illisible retombe sur le `st_mtime` du `$R`.

        Le fixture est récent des deux côtés, donc l'entrée **survit**. Un repli
        sur « très ancien » — ce que donnerait une lecture du `FILETIME` comme un
        temps Unix — la détruirait.
        """
        info, payload = recycled_file(self.volume, "CASSE", NOW - timedelta(days=30))
        info.write_bytes(b"\x00" * 4)  # tronqué
        recent = (NOW - timedelta(hours=2)).timestamp()
        os.utime(payload, (recent, recent))
        self.assertEqual(self.discover(), [])

    def test_an_unknown_info_version_also_falls_back(self) -> None:
        info, payload = recycled_file(self.volume, "VERSION", NOW - timedelta(days=30))
        info.write_bytes(info_bytes(NOW - timedelta(days=30), version=99))
        recent = (NOW - timedelta(hours=2)).timestamp()
        os.utime(payload, (recent, recent))
        self.assertEqual(self.discover(), [])

    def test_trash_days_zero_makes_a_seconds_old_entry_eligible(self) -> None:
        """Le drapeau qui ferme la boucle de la décision 18, prouvé et non supposé."""
        _info, fresh = recycled_file(self.volume, "FRAIS", NOW - timedelta(seconds=5))
        self.assertEqual(self.discover(), [])
        found = self.discover(trash_days=0)
        self.assertEqual([c.path for c in found], [str(fresh)])

    def test_a_remote_volume_contributes_nothing(self) -> None:
        recycled_file(self.volume, "ANCIEN", NOW - timedelta(days=30))
        with mock.patch.object(remove, "_get_drive_type", return_value=DRIVE_REMOTE):
            self.assertEqual(self.discover(), [])

    def test_an_unresolvable_sid_yields_zero_candidates_and_says_why(self) -> None:
        recycled_file(self.volume, "ANCIEN", NOW - timedelta(days=30))
        notes: list[CleanWarning] = []
        with mock.patch.object(mod_system, "current_user_sid", return_value=None):
            found = self.discover(notes=notes)
        self.assertEqual(found, [])
        codes = [note.code for note in notes]
        self.assertIn("recycle-bin-sid-unknown", codes)
        text = self.plan_text(notes)
        self.assertIn("SID", text)


class TestRecycleBinSizeAndRemoval(_BinCase):
    def test_the_candidate_reports_the_payload_side_size(self) -> None:
        """La taille vient du `$R`, dossier compris — jamais du header `$I`."""
        _info, payload = recycled_folder(
            self.volume, "DOSSIER", NOW - timedelta(days=30), sizes=(500, 700)
        )
        found = self.discover()
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].path, str(payload))
        self.assertEqual(found[0].estimated_bytes, 1200)

    def test_clean_removes_both_members_of_the_pair(self) -> None:
        info, payload = recycled_folder(self.volume, "PAIRE", NOW - timedelta(days=30))
        found = self.discover()
        result = mod_system.clean_recycle_bin(candidates=found)
        self.assertFalse(payload.exists())
        self.assertFalse(info.exists())
        self.assertEqual(result.module, "recycle-bin")
        self.assertGreater(result.freed or 0, 0)

    def test_the_pair_is_exactly_one_candidate_carrying_the_payload_path(self) -> None:
        """Un élément mis en corbeille = **un** candidat, et c'est le `$R`.

        Deux candidats seraient la forme qui met les octets de métadonnées dans le
        plan et laisse un run partiel orpheliner une charge utile.
        """
        info, payload = recycled_folder(self.volume, "UNIQUE", NOW - timedelta(days=30))
        found = self.discover()
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].path, str(payload))
        self.assertNotIn(str(info), [c.path for c in found])

        # La sonde prouve que la voie générique n'a reçu que le `$R` : c'est le
        # `clean()` du module qui traite le `$I`, en le dérivant.
        seen: list[str] = []
        real = remove.delete_tree

        def spy(path):  # type: ignore[no-untyped-def]
            seen.append(str(path))
            return real(path)

        with mock.patch.object(remove, "delete_tree", side_effect=spy):
            mod_system.clean_recycle_bin(candidates=found)
        self.assertEqual(seen[0], str(payload))
        self.assertEqual(seen, [str(payload), str(info)])

    def test_the_info_sidecar_survives_a_locked_payload(self) -> None:
        """Ordre imposé : pas de `$I` retiré tant que le `$R` est là.

        L'inverse laisserait une charge utile orpheline, invisible du shell et
        impossible à restaurer, qui garde ses octets pour toujours.
        """
        info, payload = recycled_file(self.volume, "VERROU", NOW - timedelta(days=30))
        found = self.discover()

        def refuse(path):  # type: ignore[no-untyped-def]
            return (0, 400, [])

        with mock.patch.object(remove, "delete_tree", side_effect=refuse):
            mod_system.clean_recycle_bin(candidates=found)
        self.assertTrue(payload.exists())
        self.assertTrue(info.exists())


class TestRecycleBinPlanText(_BinCase):
    def test_the_floor_is_stated_at_its_default(self) -> None:
        notes: list[CleanWarning] = []
        self.discover(notes=notes)
        text = self.plan_text(notes)
        self.assertIn(str(DEFAULT_TRASH_DAYS), text)

    def test_not_yet_eligible_bytes_are_named_with_the_flag_that_includes_them(self) -> None:
        recycled_file(self.volume, "RECENT", NOW - timedelta(hours=3), size=2048)
        notes: list[CleanWarning] = []
        self.discover(notes=notes)
        text = self.plan_text(notes)
        self.assertIn("--trash-days 0", text)
        self.assertIn("2", text)  # 2.0 KiB
        codes = [note.code for note in notes]
        self.assertIn("recycle-bin-not-yet-eligible", codes)
        deferred = next(n for n in notes if n.code == "recycle-bin-not-yet-eligible")
        self.assertEqual(deferred.fields["count"], 1)
        self.assertEqual(deferred.fields["bytes"], 2048)

    def test_nothing_recent_omits_both(self) -> None:
        recycled_file(self.volume, "ANCIEN", NOW - timedelta(days=30))
        notes: list[CleanWarning] = []
        self.discover(notes=notes)
        codes = [note.code for note in notes]
        self.assertNotIn("recycle-bin-not-yet-eligible", codes)
        self.assertNotIn("--trash-days 0", self.plan_text(notes))

    def test_an_undatable_entry_is_reported(self) -> None:
        """Omise **et** rapportée : une entrée qu'on ne sait pas dater se dit."""
        folder = bin_dir(self.volume)
        info = folder / "$IMUET.txt"
        payload = folder / "$RMUET.txt"
        info.write_bytes(b"\x00" * 4)
        write(payload, 100)
        with mock.patch.object(mod_system, "_mtime_of", return_value=None):
            notes: list[CleanWarning] = []
            found = self.discover(notes=notes)
        self.assertEqual(found, [])
        self.assertIn("recycle-bin-undatable", [note.code for note in notes])


class TestPackageCache(unittest.TestCase):
    def test_the_candidate_carries_the_warning_text(self) -> None:
        base = Path(tempfile.mkdtemp(prefix="winclean-pc-"))
        self.addCleanup(_cleanup, base)
        write(base / mod_system.PACKAGE_CACHE_RELATIVE / "produit" / "charge.msi", 3000)
        found = mod_system.discover_package_cache(env={"PROGRAMDATA": str(base)})
        self.assertEqual(len(found), 1)
        candidate = found[0]
        self.assertEqual(candidate.path, str(base / mod_system.PACKAGE_CACHE_RELATIVE))
        self.assertEqual(candidate.estimated_bytes, 3000)
        self.assertTrue(candidate.no_undo)
        # Jetons stables, jamais une phrase : le chemin littéral et l'avertissement
        # tel que le module le déclare (décision 20 — le texte est en français).
        self.assertIn("%ProgramData%\\Package Cache", candidate.reason)
        self.assertIn(mod_system.PACKAGE_CACHE_WARNING, candidate.reason)

    def test_a_missing_directory_yields_nothing(self) -> None:
        base = Path(tempfile.mkdtemp(prefix="winclean-pc-"))
        self.addCleanup(_cleanup, base)
        self.assertEqual(mod_system.discover_package_cache(env={"PROGRAMDATA": str(base)}), [])
        self.assertEqual(mod_system.discover_package_cache(env={}), [])


def _cleanup(root: Path) -> None:
    from scripts.winclean.tests.test_mod_dev import _rmtree

    _rmtree(root)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
