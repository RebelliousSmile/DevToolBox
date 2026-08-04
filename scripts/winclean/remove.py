"""Le seul endroit du paquet autorisé à détruire quelque chose.

Trois règles tiennent ce fichier :

1. La forme préfixée `\\\\?\\` est produite **à la frontière des appels système**
   et n'en sort jamais. Elle n'est ni stockée sur un candidat, ni renvoyée, ni
   comparée, ni imprimée. `SHFileOperationW` la refuse, la comparaison
   `exclude_paths` de `dir_size_on_disk` est lexicale, et toute la couche de
   gardes compare des chemins nus : un chemin à deux orthographes, c'est un
   garde qui évalue une chaîne que rien ne supprimera.
2. Un point de re-analyse (jonction, lien symbolique) est supprimé **comme
   entrée**, jamais descendu. La cible survit.
3. Aucune erreur par entrée ne remonte : elle est capturée avec son `winerror`
   et comptée. Un arbre partiellement verrouillé est un résultat, pas une
   exception.
"""

from __future__ import annotations

import ctypes
import os
import stat as _stat
import sys
from ctypes import wintypes
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean.common import estimate_path, volume_of  # noqa: E402

__all__ = [
    "MAX_PATH",
    "DRIVE_FIXED",
    "ERROR_SHARING_VIOLATION",
    "ERROR_LOCK_VIOLATION",
    "ERROR_DIR_NOT_EMPTY",
    "RECYCLE_FAILED",
    "RemovalError",
    "RecycleOutcome",
    "long_path",
    "delete_tree",
    "can_recycle",
    "recycle",
    "measure_freed",
    "is_lock_error",
    "is_partial_error",
]

MAX_PATH = 260
DRIVE_FIXED = 3

#: `FILE_ATTRIBUTE_REPARSE_POINT`, même valeur que dans `system_inventory`.
FILE_ATTRIBUTE_REPARSE_POINT = 0x400

ERROR_SHARING_VIOLATION = 32
ERROR_LOCK_VIOLATION = 33
ERROR_DIR_NOT_EMPTY = 145

#: Raison d'échec d'une mise en corbeille (décision 14 : pas de repli).
RECYCLE_FAILED = "recycle-failed"

_ILLEGAL_COMPONENTS = frozenset({".", ".."})


@dataclass
class RemovalError:
    """Une entrée que la suppression n'a pas pu retirer."""

    path: str
    winerror: int | None
    message: str
    size: int = 0

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "winerror": self.winerror,
            "message": self.message,
            "size": self.size,
        }


@dataclass
class RecycleOutcome:
    """Résultat d'un appel à `SHFileOperationW`.

    `code` est le retour brut de la fonction shell : ce sont ses propres codes
    `DE_*`, qui **ne vivent pas** dans l'espace des erreurs Win32 — `WinError()`
    en tirerait un message plausible et sans rapport.
    """

    ok: bool
    code: int
    aborted: bool = False
    reason: str | None = None
    detail: str = ""
    errors: list[RemovalError] = field(default_factory=list)


def is_lock_error(error: RemovalError) -> bool:
    """Vrai pour un verrou de partage : la seule défaillance qui n'est pas une erreur."""
    return error.winerror in (ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION)


def is_partial_error(error: RemovalError) -> bool:
    """Vrai pour une défaillance qui rend la suppression *partielle*, pas fatale.

    Un verrou de partage, et « le répertoire n'est pas vide » : ce dernier n'est
    **jamais** une cause première. Il ne survient que parce qu'une entrée fille
    a résisté, et cette entrée a déjà produit sa propre erreur ; le compter à
    part transformerait un fichier verrouillé en échec du run alors que le
    contrat est d'être partiel, pas fatal.
    """
    return is_lock_error(error) or error.winerror == ERROR_DIR_NOT_EMPTY


# --------------------------------------------------------------------------- #
# Forme longue
# --------------------------------------------------------------------------- #


def long_path(p: str | os.PathLike[str]) -> str:
    """Forme `\\\\?\\` d'un chemin absolu déjà résolu.

    Lève sur un chemin relatif, **et** sur un chemin absolu portant encore un
    composant `.` ou `..` ou terminé par un point ou une espace. L'absoluité
    seule est la mauvaise assertion : le préfixe **désactive** la normalisation
    Win32, donc `C:\\a\\..\\b` — absolu — atteindrait le système de fichiers avec
    un composant littéral `..` et désignerait un chemin qu'aucun garde n'a
    évalué. On lève plutôt que de réparer : réparer placerait une seconde
    résolution, plus discrète, **sous** la couche de gardes au lieu de devant.
    """
    raw = str(p)
    if raw.startswith("\\\\?\\"):
        return raw.replace("/", "\\")
    text = raw.replace("/", "\\")
    if not os.path.isabs(text):
        raise ValueError(f"chemin relatif refusé par long_path() : {raw!r}")
    drive, rest = os.path.splitdrive(text)
    for component in rest.split("\\"):
        if not component:
            continue
        if component in _ILLEGAL_COMPONENTS:
            raise ValueError(
                f"composant {component!r} non résolu dans {raw!r} : le préfixe \\\\?\\ "
                "désactive la normalisation, ce chemin ne désigne pas ce qu'il paraît."
            )
        if component[-1] in ". ":
            raise ValueError(
                f"composant terminé par un point ou une espace dans {raw!r} : "
                "illisible sous le préfixe \\\\?\\."
            )
    if text.startswith("\\\\"):
        return "\\\\?\\UNC\\" + text.lstrip("\\")
    return "\\\\?\\" + text


# --------------------------------------------------------------------------- #
# Suppression directe
# --------------------------------------------------------------------------- #


def _is_reparse_point(entry_or_stat: Any) -> bool:
    attributes = getattr(entry_or_stat, "st_file_attributes", 0)
    return bool(attributes & FILE_ATTRIBUTE_REPARSE_POINT)


def _stat_nofollow(path: Path) -> os.stat_result | None:
    try:
        return os.stat(long_path(path), follow_symlinks=False)
    except (OSError, ValueError):
        return None


def _error_from(path: Path, exc: OSError, size: int = 0) -> RemovalError:
    """Message reconstruit, jamais `str(exc)`.

    `str(OSError)` réinjecte le `filename` de l'appel système, c'est-à-dire la
    forme préfixée `\\\\?\\` — la fuite exacte que la tâche 1 interdit : elle
    ressortirait dans le rapport, dans `--json` et dans `locked_paths` sans que
    rien ne la signale. Le chemin nu vit dans `path` ; le message ne porte que
    le code et son libellé.
    """
    winerror = getattr(exc, "winerror", None)
    strerror = getattr(exc, "strerror", None) or exc.__class__.__name__
    prefix = f"[WinError {winerror}] " if winerror is not None else ""
    return RemovalError(
        path=str(path),
        winerror=winerror,
        message=f"{prefix}{strerror}",
        size=size,
    )


def _remove_as_entry(path: Path) -> None:
    """Retire un point de re-analyse sans le descendre."""
    try:
        os.rmdir(long_path(path))
    except OSError:
        os.unlink(long_path(path))


def delete_tree(path: str | os.PathLike[str]) -> tuple[int, int, list[RemovalError]]:
    """Supprime un arbre entrée par entrée. Renvoie `(libéré, en échec, erreurs)`.

    `deleted_bytes` est la somme des `st_size` réellement désindexés : c'est le
    seul chiffre qui reste juste quand une partie de l'arbre est verrouillée,
    parce qu'il compte ce qui a été retiré au lieu de différencier un répertoire
    qui contient encore les survivants.
    """
    root = Path(path)
    deleted_bytes = 0
    failed_bytes = 0
    errors: list[RemovalError] = []

    root_stat = _stat_nofollow(root)
    if root_stat is None:
        return 0, 0, errors

    if _is_reparse_point(root_stat) or not _stat.S_ISDIR(root_stat.st_mode):
        size = 0 if _is_reparse_point(root_stat) else root_stat.st_size
        try:
            if _stat.S_ISDIR(root_stat.st_mode):
                _remove_as_entry(root)
            else:
                os.unlink(long_path(root))
            deleted_bytes += size
        except OSError as exc:
            failed_bytes += size
            errors.append(_error_from(root, exc, size))
        return deleted_bytes, failed_bytes, errors

    directories: list[Path] = []
    stack: list[Path] = [root]
    while stack:
        current = stack.pop()
        directories.append(current)
        try:
            with os.scandir(long_path(current)) as it:
                entries = list(it)
        except OSError as exc:
            errors.append(_error_from(current, exc))
            continue
        for entry in entries:
            child = current / entry.name
            try:
                entry_stat = entry.stat(follow_symlinks=False)
            except OSError as exc:
                errors.append(_error_from(child, exc))
                continue
            if _is_reparse_point(entry_stat):
                # Retiré comme entrée, jamais descendu : la cible survit.
                try:
                    _remove_as_entry(child)
                except OSError as exc:
                    errors.append(_error_from(child, exc))
                continue
            if _stat.S_ISDIR(entry_stat.st_mode):
                stack.append(child)
                continue
            size = entry_stat.st_size
            try:
                os.unlink(long_path(child))
                deleted_bytes += size
            except OSError as exc:
                failed_bytes += size
                errors.append(_error_from(child, exc, size))

    for directory in reversed(directories):
        try:
            os.rmdir(long_path(directory))
        except OSError as exc:
            errors.append(_error_from(directory, exc))

    return deleted_bytes, failed_bytes, errors


# --------------------------------------------------------------------------- #
# Corbeille
# --------------------------------------------------------------------------- #

FO_DELETE = 0x0003
FOF_SILENT = 0x0004
FOF_NOCONFIRMATION = 0x0010
FOF_ALLOWUNDO = 0x0040
FOF_NOERRORUI = 0x0400
#: Les quatre sont requis. `FOF_NOERRORUI` n'est pas décoratif : sans lui le
#: shell peut lever une boîte de dialogue modale, et dans un processus lancé par
#: le lanceur (`CREATE_NO_WINDOW`, décision 16) c'est une fenêtre invisible que
#: personne ne peut fermer — donc un blocage indéfini.
RECYCLE_FLAGS = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT


class SHFILEOPSTRUCTW(ctypes.Structure):
    """Champ pour champ la signature Win32 de `SHFILEOPSTRUCTW`."""

    _fields_ = [
        ("hwnd", wintypes.HWND),
        ("wFunc", wintypes.UINT),
        ("pFrom", wintypes.LPCWSTR),
        ("pTo", wintypes.LPCWSTR),
        ("fFlags", ctypes.c_uint16),
        ("fAnyOperationsAborted", wintypes.BOOL),
        ("hNameMappings", ctypes.c_void_p),
        ("lpszProgressTitle", wintypes.LPCWSTR),
    ]


def _sh_file_operation(op: SHFILEOPSTRUCTW) -> int:
    """Appel réel à `SHFileOperationW`. Indirection : les tests la remplacent."""
    shell32 = ctypes.windll.shell32  # type: ignore[attr-defined]
    shell32.SHFileOperationW.argtypes = [ctypes.POINTER(SHFILEOPSTRUCTW)]
    shell32.SHFileOperationW.restype = ctypes.c_int
    return int(shell32.SHFileOperationW(ctypes.byref(op)))


def _get_drive_type(root: str) -> int:
    """Type de volume via `GetDriveTypeW`. Indirection : les tests la remplacent."""
    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    kernel32.GetDriveTypeW.argtypes = [wintypes.LPCWSTR]
    kernel32.GetDriveTypeW.restype = wintypes.UINT
    return int(kernel32.GetDriveTypeW(root))


def can_recycle(path: str | os.PathLike[str]) -> bool:
    """Vrai si la corbeille peut accueillir ce chemin.

    Faux au-delà de MAX_PATH — la corbeille ne connaît pas les chemins longs —
    et faux sur tout volume qui n'est pas `DRIVE_FIXED`, ce que décide
    `GetDriveTypeW` et jamais une heuristique sur le préfixe du chemin. Un
    retour inconnu ou en échec vaut faux.
    """
    try:
        prefixed = long_path(path)
    except ValueError:
        return False
    if len(prefixed) > MAX_PATH:
        return False
    try:
        drive_type = _get_drive_type(volume_of(path))
    except (OSError, AttributeError):
        return False
    return drive_type == DRIVE_FIXED


def recycle(path: str | os.PathLike[str]) -> RecycleOutcome:
    """Déplace un chemin vers la corbeille. Aucun repli sur `delete_tree`.

    Un retour non nul **ou** `fAnyOperationsAborted` armé signifie : candidat
    compté en échec avec la raison `recycle-failed`, laissé en place
    (décision 14). Le chemin est passé **non préfixé** — `SHFileOperationW`
    refuse `\\\\?\\`.
    """
    target = str(Path(path))
    op = SHFILEOPSTRUCTW()
    op.hwnd = None
    op.wFunc = FO_DELETE
    op.pFrom = target + "\0\0"
    op.pTo = None
    op.fFlags = RECYCLE_FLAGS
    op.fAnyOperationsAborted = 0
    op.hNameMappings = None
    op.lpszProgressTitle = None

    try:
        code = _sh_file_operation(op)
    except (OSError, AttributeError) as exc:
        return RecycleOutcome(
            ok=False,
            code=-1,
            aborted=False,
            reason=RECYCLE_FAILED,
            detail=f"SHFileOperationW indisponible : {exc}",
            errors=[RemovalError(path=target, winerror=None, message=str(exc))],
        )

    aborted = bool(op.fAnyOperationsAborted)
    if code != 0 or aborted:
        detail = (
            f"SHFileOperationW a renvoyé {code}"
            + (" et a armé fAnyOperationsAborted" if aborted else "")
        )
        return RecycleOutcome(
            ok=False,
            code=code,
            aborted=aborted,
            reason=RECYCLE_FAILED,
            detail=detail,
            errors=[RemovalError(path=target, winerror=None, message=detail)],
        )
    return RecycleOutcome(ok=True, code=code, aborted=False)


# --------------------------------------------------------------------------- #
# Mesure après opération
# --------------------------------------------------------------------------- #


def _exists(path: Path) -> bool:
    try:
        os.stat(long_path(path), follow_symlinks=False)
        return True
    except ValueError:
        return path.exists()
    except OSError:
        return False


def measure_freed(before_bytes: int | None, path: str | os.PathLike[str]) -> int | None:
    """Re-mesure indépendante, après opération. Ne devine jamais un nombre.

    Chemin disparu : la mesure d'avant est la réponse — `None` si elle était
    elle-même inconnue, jamais `0`. Chemin encore présent : la différence, et
    une différence **négative** (l'arbre a grossi sous un écrivain concurrent)
    est rapportée `0` plutôt que de retrancher des octets à un vrai total.

    Cette fonction ne source **pas** `CleanResult.freed` : `freed` est la somme
    des `deleted_bytes` de `delete_tree`. Ici on répond « que dit le disque
    maintenant », ce qui est le second avis que la colonne `measured` de la
    Part 3 publie.
    """
    target = Path(path)
    if not _exists(target):
        return before_bytes
    if before_bytes is None:
        return None
    after = estimate_path(target)
    if after is None:
        return None
    return max(0, before_bytes - after)
