"""Module `aggressive` : purge du journal systemd (`journalctl --vacuum-time`).

Contrepartie de `mod_system.py` côté Linux pour la partie « commande externe »
de ce niveau - pas pour sa partie corbeille : la home trashcan freedesktop est
couverte par `trash_linux.py`, pas ici, et ce fichier n'a donc qu'un seul
module, sans équivalent de `recycle-bin`.

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
    Level,
)

__all__ = [
    "JOURNALCTL_DISK_USAGE_COMMAND",
    "JOURNALCTL_VACUUM_COMMAND",
    "JOURNAL_VACUUM_LABEL",
    "journalctl_available",
    "discover_journal_vacuum",
    "clean_journal_vacuum",
]

_TOOL_TIMEOUT_SECONDS = 20

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
