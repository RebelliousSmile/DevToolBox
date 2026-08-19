"""Modules `aggressive` Linux pilotés par une commande système.

Contrepartie de `mod_system.py` côté Linux pour la partie « commande externe »
de ce niveau - pas pour sa partie corbeille : la home trashcan freedesktop est
couverte par `trash_linux.py`, pas ici. Ce fichier couvre aussi les anciennes
révisions Snap que ``snap list --all`` marque explicitement ``disabled``.

Même forme que `docker-light` (`mod_apps.py`) : un candidat **sans chemin**,
son propre `clean()` qui lance une commande externe, `estimated_bytes` à
`None` par décision plutôt qu'une conversion approximative de la sortie
lisible par un humain de `journalctl --disk-usage` (`"8.0M"` n'est pas un
entier d'octets, c'est une chaîne mise en forme pour l'œil - voir la
justification complète dans `mod_apps.discover_docker_light`).

Décision propre à ce module : la fenêtre de rétention (`_VACUUM_RETENTION_DAYS`)
est une **constante fixe**, pas `--trash-days`. `apply_plan` (`clean.py`)
n'appelle `module.clean()` qu'avec `candidates` / `recycle` / `yes` - jamais
`trash_days`, contrairement à `discover_module()` qui le reçoit pour construire
le plan. Un module sans chemin ne peut pas coder sa fenêtre dans un candidat
individuel comme le fait `recycle-bin` (une entrée = une paire `$I`/`$R` déjà
filtrée par l'âge à la découverte) : la commande de suppression est unique et
tardive, donc soit elle relit `--trash-days` d'une façon que le contrat
d'appel ne lui donne pas, soit elle est fixe. Une fenêtre fixe garantit que ce
que la découverte annonce est exactement ce que l'application exécute ; une
fenêtre dérivée d'un paramètre qu'elle ne reçoit pas romprait ce lien en
silence.

Aucun `discover_*()` ne décide `needs_network` (déclaré dans `registry_mod.py`).
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean.common import (  # noqa: E402
    DEFAULT_TRASH_DAYS,
    CleanCandidate,
    CleanResult,
    CompletedResource,
    Level,
    OperationFailure,
    estimate_path,
)

__all__ = [
    "JOURNALCTL_DISK_USAGE_COMMAND",
    "JOURNALCTL_VACUUM_COMMAND",
    "JOURNAL_VACUUM_LABEL",
    "SNAP_LIST_COMMAND",
    "SNAP_PACKAGE_DIR",
    "parse_disabled_snap_revisions",
    "journalctl_available",
    "discover_journal_vacuum",
    "clean_journal_vacuum",
    "discover_snap_old_revisions",
    "clean_snap_old_revisions",
]

_TOOL_TIMEOUT_SECONDS = 20
_SNAP_TIMEOUT_SECONDS = 120

#: Fenêtre de rétention fixe - voir la docstring du fichier pour pourquoi elle
#: n'est pas `--trash-days`. La valeur réutilise `DEFAULT_TRASH_DAYS` (7 jours)
#: pour rester cohérente avec le plancher par défaut de la corbeille, sans lui
#: être fonctionnellement liée : rien ne relie plus les deux après cette ligne.
_VACUUM_RETENTION_DAYS = DEFAULT_TRASH_DAYS

#: Sonde disponibilité **et** rien d'autre : le statut de sortie dit si le
#: journal est lisible sur cette machine. Sa sortie n'est pas analysée - voir
#: la docstring du fichier.
JOURNALCTL_DISK_USAGE_COMMAND: tuple[str, ...] = ("journalctl", "--disk-usage")

#: Commande de purge, fenêtre fixe. `--vacuum-time` retire les entrées **plus
#: vieilles** que la fenêtre ; contrairement à `--vacuum-size`, elle ne dépend
#: pas de la taille actuelle du journal et son effet est donc prévisible d'un
#: run à l'autre.
JOURNALCTL_VACUUM_COMMAND: tuple[str, ...] = (
    "journalctl",
    f"--vacuum-time={_VACUUM_RETENTION_DAYS}d",
)

JOURNAL_VACUUM_LABEL = "Journal systemd (journalctl --vacuum-time)"


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    try:
        return subprocess.run(
            list(command),
            capture_output=True,
            text=True,
            timeout=_TOOL_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None


def journalctl_available() -> bool:
    """Vrai si `journalctl` existe et répond. Échec fermé, comme `docker_available`."""
    if shutil.which("journalctl") is None:
        return False
    completed = _run(JOURNALCTL_DISK_USAGE_COMMAND)
    return completed is not None and completed.returncode == 0


def discover_journal_vacuum(**_kw: object) -> list[CleanCandidate]:
    """Un candidat sans chemin, non tarifé, ou rien si `journalctl` ne répond pas."""
    if not journalctl_available():
        return []
    return [
        CleanCandidate(
            module="journal-vacuum",
            path=None,
            label=JOURNAL_VACUUM_LABEL,
            estimated_bytes=None,
            level=Level.AGGRESSIVE,
            reason=(
                f"purge les entrées de plus de {_VACUUM_RETENTION_DAYS} jour(s) - "
                "reconstitué au fil du fonctionnement du système, historique de "
                "logs perdu au-delà de la fenêtre"
            ),
            no_undo=True,
        )
    ]


def clean_journal_vacuum(
    candidates: Sequence[CleanCandidate] | None = None,
    recycle: bool = False,
    yes: bool = False,
    **_kw: object,
) -> CleanResult:
    """`journalctl --vacuum-time=<fenêtre fixe>d`. Les trois colonnes restent `None`.

    Même raisonnement que `clean_docker_light` : pas de chemin, donc pas de
    mesure avant/après possible ici - `measure_freed()` s'applique à un chemin,
    et il n'y en a pas. Une sortie non nulle lève, une sortie nulle rend un
    `CleanResult` dont les octets sont inconnus des deux côtés.
    """
    del candidates, recycle, yes  # la commande est fixe dans tous les cas
    completed = _run(JOURNALCTL_VACUUM_COMMAND)
    if completed is None:
        raise OSError("journalctl --vacuum-time n'a pas pu être lancé")
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip().splitlines()
        raise OSError(
            "journalctl --vacuum-time a échoué (code "
            + str(completed.returncode)
            + ")"
            + (f" : {detail[-1]}" if detail else "")
        )
    return CleanResult(module="journal-vacuum")


# --------------------------------------------------------------------------- #
# Anciennes révisions Snap : sélection explicite uniquement
# --------------------------------------------------------------------------- #

SNAP_LIST_COMMAND: tuple[str, ...] = ("snap", "list", "--all")
SNAP_PACKAGE_DIR = Path("/var/lib/snapd/snaps")


def _run_snap(command: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    """Lance Snap en anglais afin que le marqueur ``disabled`` soit stable."""
    env = os.environ.copy()
    env.update({"LANG": "C", "LC_ALL": "C"})
    try:
        return subprocess.run(
            list(command),
            capture_output=True,
            text=True,
            timeout=_SNAP_TIMEOUT_SECONDS,
            env=env,
        )
    except (OSError, subprocess.SubprocessError):
        return None


def parse_disabled_snap_revisions(output: str) -> list[tuple[str, str]]:
    """Extrait ``(nom, révision)`` des seules lignes marquées ``disabled``.

    ``snap list`` n'offre pas de sortie JSON. Sous ``LANG=C``, les colonnes
    utiles sont stables : nom en première position, révision en troisième et
    notes en dernière position. Les autres colonnes restent volontairement
    ignorées.
    """
    found: list[tuple[str, str]] = []
    for line in output.splitlines()[1:]:
        fields = line.split()
        if len(fields) < 6 or "disabled" not in fields[-1].split(","):
            continue
        name, revision = fields[0], fields[2]
        if name and revision:
            found.append((name, revision))
    return found


def discover_snap_old_revisions(**_kw: object) -> list[CleanCandidate]:
    """Révisions Snap inactives, identifiées exclusivement par snapd."""
    if shutil.which("snap") is None:
        return []
    completed = _run_snap(SNAP_LIST_COMMAND)
    if completed is None or completed.returncode != 0:
        return []

    candidates: list[CleanCandidate] = []
    for name, revision in parse_disabled_snap_revisions(completed.stdout or ""):
        package = SNAP_PACKAGE_DIR / f"{name}_{revision}.snap"
        try:
            mtime: float | None = package.stat().st_mtime
        except OSError:
            mtime = None
        candidates.append(
            CleanCandidate(
                module="snap-old-revisions",
                path=str(package) if package.is_file() else None,
                label=f"Snap {name}, ancienne révision {revision}",
                estimated_bytes=estimate_path(package) if package.is_file() else None,
                level=Level.AGGRESSIVE,
                reason="révision désactivée ; retour arrière vers cette révision perdu",
                no_undo=True,
                stat_mtime=mtime,
                resource_id=f"{name}@{revision}",
            )
        )
    return candidates


def clean_snap_old_revisions(
    candidates: Sequence[CleanCandidate] | None = None,
    recycle: bool = False,
    yes: bool = False,
    **_kw: object,
) -> CleanResult:
    """Retire chaque révision via snapd, sans créer de snapshot de secours."""
    del recycle, yes
    result = CleanResult(module="snap-old-revisions")
    for candidate in candidates or ():
        resource_id = candidate.resource_id or "revision-inconnue"
        name, separator, revision = resource_id.partition("@")
        if not separator or not name or not revision:
            result.operation_failures.append(
                OperationFailure(resource_id, "snap-invalid-resource", "identifiant invalide")
            )
            continue

        command = ("snap", "remove", name, f"--revision={revision}", "--purge")
        completed = _run_snap(command)
        if completed is None or completed.returncode != 0:
            detail = "snap remove n'a pas pu être lancé"
            if completed is not None:
                lines = (completed.stderr or completed.stdout or "").strip().splitlines()
                detail = lines[-1] if lines else f"snap remove a échoué (code {completed.returncode})"
            result.operation_failures.append(
                OperationFailure(resource_id, "snap-remove-error", detail)
            )
            continue

        result.completed_resources.append(CompletedResource(resource_id))
        if candidate.path:
            before = candidate.estimated_bytes
            after = estimate_path(candidate.path)
            if before is not None:
                result.freed = (result.freed or 0) + max(0, before - (after or 0))
    return result
