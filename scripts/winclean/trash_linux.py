"""Corbeille utilisateur Linux, spécification freedesktop.org Trash.

Contrepartie de la corbeille Windows de `remove.py` (`can_recycle()`/
`recycle()`) : c'est ce module que `remove.py` devra apprendre à appeler sur
Linux (Part 5 Phase 3), pas l'inverse - `remove.py` reste aujourd'hui
entièrement Windows (`ctypes.windll`) et n'importe rien d'ici tant que cette
phase n'a pas eu lieu. C'est aussi la raison pour laquelle ce fichier ne
s'appelle pas `mod_linux_*.py` : `tests/test_registry_mod_linux_contract.py`
interdit à tout module de ce motif d'importer la couche de suppression, et
c'est précisément le rôle de celui-ci.

Ne couvre que la « home trashcan » (`$XDG_DATA_HOME/Trash`), pas les
corbeilles de premier niveau d'un point de montage (`$topdir/.Trash-uid`) que
la spécification prévoit pour les volumes hors `$HOME` : aucun module de ce
paquet ne découvre de cible en dehors de `$HOME`, donc ce cas ne se présente
jamais ici.

Deux fichiers par entrée jetée, comme l'exige la spécification : le contenu
sous `files/<nom>`, les métadonnées `Path=`/`DeletionDate=` sous
`info/<nom>.trashinfo`. Un nom déjà pris gagne un suffixe numérique - jamais
une écrasement de l'entrée existante.
"""

from __future__ import annotations

import datetime
import os
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import quote

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import platform_paths  # noqa: E402

__all__ = [
    "TRASH_FAILED",
    "TrashError",
    "TrashOutcome",
    "can_trash",
    "info_file_content",
    "move_to_trash",
]

#: Raison d'échec d'une mise en corbeille, au même titre que `RECYCLE_FAILED`
#: côté Windows (décision 14 : pas de repli silencieux vers une suppression
#: directe).
TRASH_FAILED = "trash-failed"

#: Permissions requises par la spécification pour `files/` et `info/`.
_TRASH_DIR_MODE = 0o700


@dataclass
class TrashError:
    """Une entrée que la mise en corbeille n'a pas pu retirer."""

    path: str
    message: str


@dataclass
class TrashOutcome:
    """Résultat d'une tentative de mise en corbeille."""

    ok: bool
    trashed_path: str | None = None
    reason: str | None = None
    detail: str = ""
    errors: list[TrashError] = field(default_factory=list)


def can_trash(path: str | os.PathLike[str], env: dict[str, str] | None = None) -> bool:
    """Vrai si la corbeille utilisateur peut accueillir ce chemin.

    Faux si `$HOME`/`$XDG_DATA_HOME` ne se résout pas, si le chemin est
    relatif - la spécification veut un `Path=` absolu pour la home trashcan,
    un chemin relatif serait donc écrit faux dès l'origine - ou s'il
    n'existe déjà plus.
    """
    source = Path(path)
    if not source.is_absolute():
        return False
    if platform_paths.trash_files_dir(env) is None:
        return False
    return source.exists() or source.is_symlink()


def info_file_content(original_path: Path, deletion_time: datetime.datetime) -> str:
    """Contenu d'un `.trashinfo`, au format `[Trash Info]` de la spécification.

    `Path=` est pourcent-encodé (tout sauf `/`, comme une URL de fichier) :
    la spécification l'exige pour que les retours à la ligne ou les `%`
    littéraux du nom original ne cassent pas le format INI à la relecture.
    `DeletionDate=` est un horodatage local sans fuseau, tel que documenté.
    """
    encoded = quote(str(original_path), safe="/")
    stamp = deletion_time.strftime("%Y-%m-%dT%H:%M:%S")
    return f"[Trash Info]\nPath={encoded}\nDeletionDate={stamp}\n"


def _unique_trash_name(files_dir: Path, info_dir: Path, name: str) -> str:
    """`name`, ou `name (n)` si l'un ou l'autre côté le porte déjà.

    Les deux répertoires sont vérifiés : un `.trashinfo` orphelin (son
    `files/` supprimé à la main entre deux runs) doit encore empêcher la
    réutilisation de son nom, sous peine d'écrire des métadonnées qui ne
    décriraient plus l'entrée réellement présente.
    """
    candidate = name
    suffix = 1
    while (files_dir / candidate).exists() or (info_dir / f"{candidate}.trashinfo").exists():
        candidate = f"{name} ({suffix})"
        suffix += 1
    return candidate


def move_to_trash(
    path: str | os.PathLike[str],
    env: dict[str, str] | None = None,
    now: datetime.datetime | None = None,
) -> TrashOutcome:
    """Déplace `path` vers la corbeille utilisateur. Aucun repli sur `delete_tree`.

    Écrit le `.trashinfo` **avant** de déplacer l'entrée : un crash entre les
    deux laisse un fichier orphelin sous `info/` (inoffensif, ignoré par tout
    lecteur qui vérifie l'existence du côté `files/`), jamais l'inverse -
    une entrée dans `files/` sans métadonnées serait un objet que rien ne
    sait restaurer ni dater.
    """
    source = Path(path)
    files_dir = platform_paths.trash_files_dir(env)
    info_dir = platform_paths.trash_info_dir(env)
    if files_dir is None or info_dir is None:
        return TrashOutcome(
            ok=False,
            reason=TRASH_FAILED,
            detail="corbeille non résolue ($HOME/$XDG_DATA_HOME absents)",
        )
    if not source.is_absolute():
        return TrashOutcome(
            ok=False,
            reason=TRASH_FAILED,
            detail=f"chemin relatif refusé : {source}",
        )
    if not (source.exists() or source.is_symlink()):
        return TrashOutcome(
            ok=False,
            reason=TRASH_FAILED,
            detail=f"chemin absent : {source}",
        )

    try:
        files_dir.mkdir(parents=True, exist_ok=True, mode=_TRASH_DIR_MODE)
        info_dir.mkdir(parents=True, exist_ok=True, mode=_TRASH_DIR_MODE)
    except OSError as exc:
        return TrashOutcome(
            ok=False,
            reason=TRASH_FAILED,
            detail=str(exc),
            errors=[TrashError(path=str(source), message=str(exc))],
        )

    name = _unique_trash_name(files_dir, info_dir, source.name)
    target = files_dir / name
    info_path = info_dir / f"{name}.trashinfo"
    stamp = now if now is not None else datetime.datetime.now()

    try:
        info_path.write_text(info_file_content(source, stamp), encoding="utf-8")
    except OSError as exc:
        return TrashOutcome(
            ok=False,
            reason=TRASH_FAILED,
            detail=str(exc),
            errors=[TrashError(path=str(source), message=str(exc))],
        )

    try:
        shutil.move(str(source), str(target))
    except OSError as exc:
        try:
            info_path.unlink()
        except OSError:
            pass
        return TrashOutcome(
            ok=False,
            reason=TRASH_FAILED,
            detail=str(exc),
            errors=[TrashError(path=str(source), message=str(exc))],
        )

    return TrashOutcome(ok=True, trashed_path=str(target))
