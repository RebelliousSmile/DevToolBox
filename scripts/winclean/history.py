"""Journal JSONL des runs qui ont détruit quelque chose.

Quatre propriétés de ce fichier sont des décisions, pas des détails :

1. **Seuls les runs destructeurs y entrent.** Une simulation n'écrit rien. C'est
   pourquoi l'enregistrement ne porte **aucun champ `mode`** : il ne pourrait
   contenir que `"apply"`, colonne constante qui invite un lecteur à filtrer
   dessus et à conclure que les simulations sont *absentes du filtre* plutôt que
   jamais écrites.
2. **Le déclencheur est une tentative de suppression, pas un total d'octets.**
   `clean.py` positionne un drapeau au moment où il tente, et c'est lui qui est
   testé dans le `finally`. Les totaux ne peuvent pas répondre à la question :
   `freed` vaut `None` pour une récupération non mesurable (`docker-light` après
   un `prune` réussi), et un run `moderate` met en corbeille, donc libère zéro.
   Une règle dérivée des octets laisserait sans trace le run destructeur le plus
   courant.
3. **`trim` ne lit pas ce qu'il coupe.** Il garde les N dernières **lignes dans
   l'ordre du fichier**, sans analyser, trier ni filtrer. `read_runs` a pour
   consigne de tolérer une ligne illisible ; un `trim` qui analyserait pour
   trier par `timestamp` supprimerait précisément ces lignes-là. Un journal
   d'audit qui perd des enregistrements en silence est pire que pas de journal.
4. **`None` n'est pas zéro**, ici comme partout dans le paquet : en JSON c'est
   `null`, et l'aller-retour le préserve.

L'écriture est un unique `write()` d'une ligne déjà sérialisée, en mode `"a"` :
suffisamment atomique pour un fichier orienté ligne sous Windows, et documenté
comme « au mieux », pas comme transactionnel.
"""

from __future__ import annotations

import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean.common import (  # noqa: E402
    CleanResult,
    Level,
    RunReport,
    human_size,
)

__all__ = [
    "HistoryError",
    "HISTORY_MAX_LINES",
    "STATUS_COMPLETED",
    "STATUS_INTERRUPTED",
    "RECORD_KEYS",
    "default_history_path",
    "resolve_path",
    "utc_stamp",
    "build_record",
    "append_run",
    "read_runs",
    "trim",
    "format_history",
]

#: Plafond du journal, en lignes. Au-delà, les plus anciennes partent.
HISTORY_MAX_LINES = 500

STATUS_COMPLETED = "completed"
STATUS_INTERRUPTED = "interrupted"

#: Clés d'un enregistrement, dans l'ordre d'écriture. Jetons machine en anglais.
RECORD_KEYS: tuple[str, ...] = (
    "timestamp",
    "level",
    "status",
    "estimated_bytes",
    "freed_bytes",
    "recycled_bytes",
    "failed_bytes",
    "modules",
)


class HistoryError(OSError):
    """Le journal n'a pas pu être lu ou écrit. Jamais fatal pour un run."""


# --------------------------------------------------------------------------- #
# Emplacement
# --------------------------------------------------------------------------- #


def default_history_path(env: Mapping[str, str] | None = None) -> Path | None:
    """`%LOCALAPPDATA%\\winclean\\history.jsonl`, `None` si la variable est absente.

    `%LOCALAPPDATA%` et non `%APPDATA%` : un journal de machine ne se synchronise
    pas d'un poste à l'autre, sinon deux machines s'écrasent mutuellement des
    lignes que rien ne permet de rattacher à l'une ou à l'autre.
    """
    environment = os.environ if env is None else env
    base = environment.get("LOCALAPPDATA")
    if not base:
        return None
    return Path(base) / "winclean" / "history.jsonl"


def resolve_path(
    path: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
) -> Path:
    """Chemin explicite, sinon l'emplacement par défaut. Lève s'il n'y en a aucun."""
    if path is not None:
        return Path(path)
    target = default_history_path(env)
    if target is None:
        raise HistoryError(
            "%LOCALAPPDATA% est absent : aucun emplacement d'historique n'est connu"
        )
    return target


# --------------------------------------------------------------------------- #
# Enregistrement
# --------------------------------------------------------------------------- #


def utc_stamp(moment: datetime | None = None) -> str:
    """ISO-8601 **UTC** avec un `Z` final, l'orthographe machine de ce dépôt.

    Une heure locale naïve est ambiguë deux fois par an et impossible à trier sur
    une machine qui change de zone — dans un fichier dont le seul ordre est celui
    de son écriture.
    """
    stamped = datetime.now(timezone.utc) if moment is None else moment.astimezone(timezone.utc)
    return stamped.strftime("%Y-%m-%dT%H:%M:%SZ")


def _measured(result: CleanResult) -> int | None:
    """Octets réellement récupérés pour ce module, `None` si non mesurable.

    Accès indirect assumé : le champ `measured` de `CleanResult` arrive à la
    phase 4 de cette part, et l'historique ne doit pas dépendre de l'ordre des
    phases pour être écrit correctement. Le jour où le champ existe, cette
    fonction devient un simple accesseur.
    """
    return getattr(result, "measured", None)


def build_record(
    level: Level | str,
    report: RunReport,
    *,
    timestamp: str | None = None,
) -> dict[str, Any]:
    """Enregistrement d'un run destructeur, prêt à sérialiser.

    Les quatre totaux d'octets sont `int|null` aux mêmes conditions que la paire
    par module : un total vaut `null` seulement quand aucun module du run n'a su
    mesurer quoi que ce soit, ce qu'un `--only docker-light` atteint à lui seul.
    """
    return {
        "timestamp": timestamp or utc_stamp(),
        "level": Level(level).value,
        "status": report.status,
        "estimated_bytes": report.estimated_total,
        "freed_bytes": report.total_freed(),
        "recycled_bytes": report.total_recycled(),
        "failed_bytes": report.total_failed(),
        "modules": {
            result.module: {
                "estimated": result.estimated,
                "measured": _measured(result),
            }
            for result in report.results
        },
    }


# --------------------------------------------------------------------------- #
# Écriture
# --------------------------------------------------------------------------- #


def append_run(
    record: Mapping[str, Any],
    path: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
    max_lines: int = HISTORY_MAX_LINES,
) -> Path:
    """Ajoute une ligne, puis élague. Rend le fichier écrit.

    La sérialisation précède l'ouverture : un enregistrement non sérialisable
    échoue **avant** d'avoir ouvert le journal, donc sans y laisser de ligne
    partielle.
    """
    target = resolve_path(path, env)
    line = json.dumps(record, ensure_ascii=False, sort_keys=False)
    target.parent.mkdir(parents=True, exist_ok=True)
    with open(target, "a", encoding="utf-8", newline="\n") as handle:
        handle.write(line + "\n")
    trim(max_lines, target)
    return target


def trim(
    max_lines: int = HISTORY_MAX_LINES,
    path: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> int:
    """Garde les `max_lines` **dernières lignes**, dans l'ordre du fichier.

    N'analyse rien : voir la propriété 3 du module. La réécriture passe par un
    fichier temporaire **dans le même dossier** puis `os.replace()`, seul moyen
    pour qu'un `trim` interrompu ne laisse pas un journal tronqué — le fichier
    dont le métier est de survivre à un run interrompu ne doit pas être détruit
    par un.
    """
    target = resolve_path(path, env)
    if not target.exists():
        return 0
    with open(target, "r", encoding="utf-8", newline="") as handle:
        lines = handle.readlines()
    if len(lines) <= max_lines:
        return len(lines)
    kept = lines[-max_lines:] if max_lines > 0 else []
    temporary = target.with_name(target.name + ".tmp")
    with open(temporary, "w", encoding="utf-8", newline="") as handle:
        handle.writelines(kept)
    os.replace(temporary, target)
    return len(kept)


# --------------------------------------------------------------------------- #
# Lecture
# --------------------------------------------------------------------------- #


def _default_notice(message: str) -> None:
    print(message, file=sys.stderr)


def read_runs(
    limit: int | None = None,
    path: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
    notice: Callable[[str], None] | None = None,
) -> list[dict[str, Any]]:
    """Les `limit` derniers enregistrements lisibles, plus anciens d'abord.

    Une ligne illisible est **signalée et sautée**, jamais fatale : le journal
    est un fichier ajouté ligne par ligne par un outil qui peut être interrompu,
    et une ligne tronquée ne doit pas rendre l'audit impossible.
    """
    tell = _default_notice if notice is None else notice
    try:
        target = resolve_path(path, env)
    except HistoryError:
        return []
    if not target.exists():
        return []
    records: list[dict[str, Any]] = []
    with open(target, "r", encoding="utf-8", newline="") as handle:
        for number, line in enumerate(handle, start=1):
            text = line.strip()
            if not text:
                continue
            try:
                parsed = json.loads(text)
            except json.JSONDecodeError as exc:
                tell(f"{target} ligne {number} illisible, ignorée : {exc.msg}")
                continue
            if not isinstance(parsed, dict):
                tell(f"{target} ligne {number} n'est pas un objet JSON, ignorée")
                continue
            records.append(parsed)
    if limit is not None and limit >= 0:
        return records[-limit:] if limit else []
    return records


# --------------------------------------------------------------------------- #
# Affichage
# --------------------------------------------------------------------------- #

#: Statut machine → mot français. Le fichier garde le jeton anglais ; seul
#: l'affichage est traduit, et `--history --json` rend l'enregistrement brut.
_STATUS_LABELS = {
    STATUS_COMPLETED: "terminé",
    STATUS_INTERRUPTED: "interrompu",
}

_STAMP_WIDTH = 21
_LEVEL_WIDTH = 10
_STATUS_WIDTH = 10
_SIZE_WIDTH = 11


def _cell(value: Any) -> str:
    """Octets rendus comme partout ailleurs : `None` n'est jamais montré `0`."""
    if value is None or isinstance(value, int):
        return human_size(value)
    return str(value)


def format_history(
    records: Sequence[Mapping[str, Any]],
    path: str | os.PathLike[str] | None = None,
) -> str:
    """Tableau compact des runs, du plus ancien au plus récent."""
    out: list[str] = []
    if not records:
        out.append("Aucun run enregistré.")
        if path is not None:
            out.append(f"Journal attendu : {path}")
        return "\n".join(out)

    out.append(f"Historique : {len(records)} run(s) destructeur(s), du plus ancien au plus récent.")
    out.append(
        f"  {'horodatage (UTC)':<{_STAMP_WIDTH}} {'niveau':<{_LEVEL_WIDTH}} "
        f"{'statut':<{_STATUS_WIDTH}} {'estimé':>{_SIZE_WIDTH}} "
        f"{'libéré':>{_SIZE_WIDTH}} {'corbeille':>{_SIZE_WIDTH}} {'échec':>{_SIZE_WIDTH}}"
    )
    for record in records:
        status = str(record.get("status", ""))
        out.append(
            f"  {str(record.get('timestamp', '-')):<{_STAMP_WIDTH}} "
            f"{str(record.get('level', '-')):<{_LEVEL_WIDTH}} "
            f"{_STATUS_LABELS.get(status, status):<{_STATUS_WIDTH}} "
            f"{_cell(record.get('estimated_bytes')):>{_SIZE_WIDTH}} "
            f"{_cell(record.get('freed_bytes')):>{_SIZE_WIDTH}} "
            f"{_cell(record.get('recycled_bytes')):>{_SIZE_WIDTH}} "
            f"{_cell(record.get('failed_bytes')):>{_SIZE_WIDTH}}"
        )
    modules = _modules_line(records[-1])
    if modules:
        out.append("")
        out.append("Dernier run, par module (estimé / mesuré) :")
        out.extend(modules)
    if path is not None:
        out.append("")
        out.append(f"Journal : {path}")
    return "\n".join(out)



def _modules_line(record: Mapping[str, Any]) -> list[str]:
    modules = record.get("modules")
    if not isinstance(modules, Mapping) or not modules:
        return []
    lines: list[str] = []
    for name, figures in modules.items():
        if not isinstance(figures, Mapping):
            continue
        lines.append(
            f"  {str(name):<24} {_cell(figures.get('estimated')):>{_SIZE_WIDTH}} "
            f"{_cell(figures.get('measured')):>{_SIZE_WIDTH}}"
        )
    return lines
