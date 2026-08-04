"""Modules `aggressive` : corbeille de l'utilisateur et cache d'installation MSI.

Deux modules, et les deux suppriment **en direct** : mettre la corbeille à la
corbeille n'a pas de sens, et `%ProgramData%\\Package Cache` est déjà annoncé
comme sans retour arrière. `--recycle` est donc accepté et inerte à ce niveau
(décision 4), exactement comme à `safe`.

Ce fichier est le seul `mod_*` à importer `remove` : `recycle-bin` porte son
propre `clean()` parce que son unité de suppression est une **paire**
`$I`/`$R`, et `CleanCandidate` n'a qu'un champ `path`. Le candidat porte le
`$R` — la charge utile, celle que `dir_size_on_disk` tarife et celle dont
l'utilisateur remarquerait la survie — et le `clean()` retire ce `$R` puis le
`$I` dérivé du même chemin. L'ordre n'est pas indifférent : interrompu, il
laisse au pire un talon de métadonnées que le shell affiche et ne sait pas
restaurer (visible, minuscule, effaçable à la main) là où l'ordre inverse
laisserait une charge utile orpheline, invisible de l'interface et qui garde
ses octets pour toujours.

Conséquence assumée : le `measured` de ce module dépasse **légitimement** son
`estimated`, des octets du sidecar. L'estimation tarife la charge utile, le run
retire la charge utile et ses métadonnées.

Un `discover_*()` ne rend que des candidats. Le canal de ce module vers le texte
du plan est donc `notes`, une liste de `CleanWarning` que le constructeur de plan
lui passe — trois de ses quatre codes parlent justement des cas où il n'y a
aucun candidat à porter le message (SID irrésolu, entrées trop récentes, entrées
indatables).

Aucun `discover_*()` ne décide `needs_network` (déclaré dans `registry_mod.py`).
"""

from __future__ import annotations

import ctypes
import os
import sys
from ctypes import wintypes
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Iterable, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import remove  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DEFAULT_TRASH_DAYS,
    CleanCandidate,
    CleanResult,
    CleanWarning,
    Level,
    estimate_path,
    human_size,
    volume_of,
)

__all__ = [
    "RECYCLE_BIN_DIRNAME",
    "PACKAGE_CACHE_RELATIVE",
    "PACKAGE_CACHE_WARNING",
    "PACKAGE_CACHE_CONFIRM",
    "FILETIME_EPOCH",
    "KNOWN_INFO_VERSIONS",
    "current_user_sid",
    "info_sibling",
    "discover_recycle_bin",
    "clean_recycle_bin",
    "discover_package_cache",
]

#: Nom du dossier de corbeille à la racine de chaque volume. Caché et système :
#: `os.scandir` le lit quand même, aucun attribut n'est à lever.
RECYCLE_BIN_DIRNAME = "$Recycle.Bin"

#: Cache d'installation de Windows Installer, sous `%ProgramData%`.
PACKAGE_CACHE_RELATIVE = "Package Cache"

#: Avertissement joint à la raison du candidat `package-cache`. Trois faits, et
#: les trois sont nécessaires : ce que la suppression retire, que rien ne le
#: reconstitue localement, et que la conséquence survit au run — ce dernier point
#: étant la raison d'être de la seconde confirmation.
PACKAGE_CACHE_WARNING = (
    "Windows Installer garde ici les charges MSI/MSP des produits installés : les "
    "supprimer fait qu'une réparation, une modification ou une désinstallation "
    "ultérieure réclame le média d'origine ou échoue. Ces octets ne se "
    "reconstituent pas localement — seul un nouveau téléchargement ou le "
    "programme d'installation de chaque éditeur les rétablit. La conséquence "
    "survit au run, ce qu'aucun autre module de ce niveau ne fait."
)

#: Question de la confirmation propre au module (partie 3, phase 3). `--yes` n'y
#: répond pas : décision 4 ne lui donne que deux effets et celui-ci n'en est pas.
PACKAGE_CACHE_CONFIRM = (
    "package-cache : supprimer %ProgramData%\\Package Cache casse les réparations "
    "et désinstallations futures, et rien ne le reconstitue localement. Confirmer ?"
)

#: Origine des `FILETIME` : 1601-01-01 UTC, en ticks de 100 ns.
FILETIME_EPOCH = datetime(1601, 1, 1, tzinfo=timezone.utc)
_TICKS_PER_MICROSECOND = 10

#: Versions du format `$I` connues : `1` avant Windows 10, `2` depuis. Une valeur
#: hors de cette liste fait échouer la lecture **fermé**, jamais « très vieux ».
KNOWN_INFO_VERSIONS: tuple[int, ...] = (1, 2)

#: Décalage du `FILETIME` dans un `$I` : 8 octets de version, 8 de taille
#: d'origine, puis l'horodatage. Lire ces 8 octets comme un temps Unix daterait
#: l'entrée de 1601 — donc *très ancienne* — et détruirait un fichier mis en
#: corbeille il y a cinq minutes.
_INFO_TIMESTAMP_OFFSET = 16
_INFO_HEADER_SIZE = 24

#: Bornes de plausibilité de l'horodatage lu. En dessous, le `$I` est corrompu ou
#: mal interprété ; au-dessus, l'horloge a bougé. Les deux retombent sur le
#: `st_mtime` du `$R`, jamais sur une supposition d'ancienneté.
_EARLIEST_PLAUSIBLE = datetime(1990, 1, 1, tzinfo=timezone.utc)
_FUTURE_GRACE = timedelta(minutes=5)

_TOKEN_QUERY = 0x0008
_TOKEN_USER_CLASS = 1


# --------------------------------------------------------------------------- #
# SID du compte courant
# --------------------------------------------------------------------------- #


def current_user_sid() -> str | None:
    """SID du compte courant sous forme textuelle, ou `None`.

    Résolu, jamais deviné : `OpenProcessToken(TOKEN_QUERY)` →
    `GetTokenInformation(TokenUser)` → `ConvertSidToStringSidW`. Toute défaillance
    de la chaîne rend `None`, et l'appelant en fait **zéro candidat** — il ne se
    replie pas sur l'énumération de tous les `$Recycle.Bin\\S-1-5-*` lisibles, ce
    qui sur une machine partagée viderait la corbeille d'un autre compte.
    """
    try:
        advapi32 = ctypes.windll.advapi32  # type: ignore[attr-defined]
        kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    except AttributeError:  # pragma: no cover - hors Windows
        return None
    token = wintypes.HANDLE()
    try:
        if not advapi32.OpenProcessToken(
            kernel32.GetCurrentProcess(), _TOKEN_QUERY, ctypes.byref(token)
        ):
            return None
    except (OSError, AttributeError, ValueError):  # pragma: no cover - filet
        return None
    try:
        size = wintypes.DWORD(0)
        advapi32.GetTokenInformation(
            token, _TOKEN_USER_CLASS, None, 0, ctypes.byref(size)
        )
        if not size.value:  # pragma: no cover - jeton dégénéré
            return None
        buffer = ctypes.create_string_buffer(size.value)
        if not advapi32.GetTokenInformation(
            token, _TOKEN_USER_CLASS, buffer, size, ctypes.byref(size)
        ):  # pragma: no cover - second appel en échec
            return None
        # TOKEN_USER commence par un SID_AND_ATTRIBUTES, dont le premier champ est
        # le pointeur `PSID` que la fonction suivante attend.
        sid = ctypes.cast(buffer, ctypes.POINTER(ctypes.c_void_p)).contents
        text = ctypes.c_wchar_p()
        if not advapi32.ConvertSidToStringSidW(sid, ctypes.byref(text)):
            return None  # pragma: no cover - conversion en échec
        try:
            value = text.value
        finally:
            kernel32.LocalFree(text)
        return str(value) if value else None
    except (OSError, AttributeError, ValueError):  # pragma: no cover - filet
        return None
    finally:
        try:
            kernel32.CloseHandle(token)
        except (OSError, AttributeError):  # pragma: no cover - filet
            pass


# --------------------------------------------------------------------------- #
# Paires `$I` / `$R`
# --------------------------------------------------------------------------- #


def info_sibling(payload: str | os.PathLike[str]) -> Path | None:
    """Le `$I` d'un `$R`, dérivé par substitution de la lettre de tête.

    `None` si le nom ne commence pas par `$R` : rien à dériver, et fabriquer un
    chemin depuis un nom inattendu vaut mieux ne pas être tenté.
    """
    path = Path(payload)
    name = path.name
    if len(name) < 2 or not name.startswith("$R"):
        return None
    return path.with_name("$I" + name[2:])


def _payload_sibling(info: Path) -> Path:
    """Le `$R` d'un `$I`. L'inverse strict de `info_sibling`."""
    return info.with_name("$R" + info.name[2:])


def _read_info_timestamp(info: Path) -> datetime | None:
    """Horodatage de mise en corbeille lu dans le `$I`, ou `None`.

    `None` couvre tous les échecs sans les distinguer — fichier absent, tronqué,
    version inconnue, date implausible — parce que l'appelant les traite
    identiquement : repli sur le `$R`, puis omission. Les distinguer n'ajouterait
    qu'un vocabulaire dont personne n'a l'usage.
    """
    try:
        with open(info, "rb") as handle:
            header = handle.read(_INFO_HEADER_SIZE)
    except OSError:
        return None
    if len(header) < _INFO_HEADER_SIZE:
        return None
    version = int.from_bytes(header[0:8], "little", signed=False)
    if version not in KNOWN_INFO_VERSIONS:
        return None
    ticks = int.from_bytes(
        header[_INFO_TIMESTAMP_OFFSET : _INFO_TIMESTAMP_OFFSET + 8],
        "little",
        signed=False,
    )
    try:
        return FILETIME_EPOCH + timedelta(microseconds=ticks // _TICKS_PER_MICROSECOND)
    except (OverflowError, ValueError, OSError):
        return None


def _mtime_of(path: Path) -> datetime | None:
    try:
        stamp = path.stat().st_mtime
    except OSError:
        return None
    try:
        return datetime.fromtimestamp(stamp, tz=timezone.utc)
    except (OverflowError, ValueError, OSError):  # pragma: no cover - filet
        return None


def _deleted_at(info: Path, payload: Path, now: datetime) -> datetime | None:
    """Date de mise en corbeille de la paire, ou `None` si indatable.

    Chaîne fermée : en-tête `$I` d'abord, `st_mtime` du `$R` ensuite, omission
    sinon. Une entrée dont on ne sait rien n'est **jamais** supposée ancienne :
    c'est exactement le cas que le plancher d'âge existe pour protéger.
    """
    stamp = _read_info_timestamp(info)
    if stamp is not None and _EARLIEST_PLAUSIBLE <= stamp <= now + _FUTURE_GRACE:
        return stamp
    return _mtime_of(payload)


def _payload_size(payload: Path) -> int | None:
    """Taille du côté `$R`, dossier compris (`estimate_path`, donc
    `dir_size_on_disk` pour un dossier).

    Jamais la taille d'origine inscrite dans le header `$I` : celle-là est
    *logique*, ne concerne qu'une entrée unique et, pour un dossier mis en
    corbeille, ne décrit pas ce que le volume récupère.
    """
    return estimate_path(payload)


# --------------------------------------------------------------------------- #
# `recycle-bin`
# --------------------------------------------------------------------------- #


def _fixed_volume(volume: Path) -> bool:
    """Vrai si ce volume est `DRIVE_FIXED`, via `GetDriveTypeW` (partie 1).

    Jamais une heuristique sur le préfixe du chemin : un dossier réseau monté sur
    une lettre ressemble à un disque local et la corbeille d'un partage n'est pas
    la nôtre.
    """
    try:
        return remove._get_drive_type(volume_of(volume)) == remove.DRIVE_FIXED
    except (OSError, AttributeError, ValueError):  # pragma: no cover - filet
        return False


def _fixed_volumes() -> list[Path]:
    """Volumes locaux, par lettre. Le type est demandé **avant** tout accès.

    `GetDriveTypeW` répond sans I/O : interroger `exists()` d'abord bloquerait
    plusieurs secondes sur un lecteur réseau déconnecté, pour finir par l'écarter.
    """
    found: list[Path] = []
    for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ":
        root = Path(f"{letter}:\\")
        if _fixed_volume(root):
            found.append(root)
    return found


def _bin_entries(bin_dir: Path) -> list[Path]:
    """Les `$I*` d'un dossier de corbeille, triés. Rien si illisible.

    L'énumération part des `$I` et **dérive** chaque `$R` : partir des entrées du
    dossier ferait de chaque `$I` un candidat de son propre chef, un run
    retirerait quelques centaines d'octets de métadonnées par entrée et
    laisserait toutes les charges utiles derrière — corbeille vide à l'écran, sur
    des gigaoctets ni récupérés ni restaurables, et un rapport qui se lit comme
    un succès.
    """
    try:
        with os.scandir(bin_dir) as iterator:
            names = [entry.name for entry in iterator]
    except OSError:
        return []
    return [bin_dir / name for name in sorted(names) if name.startswith("$I")]


def discover_recycle_bin(
    trash_days: int | None = None,
    volumes: Iterable[str | os.PathLike[str]] | None = None,
    notes: list[CleanWarning] | None = None,
    now: datetime | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Paires `$I`/`$R` de **notre** corbeille, plus vieilles que le plancher.

    Une paire = un candidat, porteur du chemin `$R`. Le module fournit son propre
    `clean()` pour retirer les deux membres ; voir la docstring du fichier.
    """
    days = DEFAULT_TRASH_DAYS if trash_days is None else int(trash_days)
    moment = datetime.now(timezone.utc) if now is None else now
    floor = moment - timedelta(days=days)

    if notes is not None:
        notes.append(
            CleanWarning(
                code="recycle-bin-floor" if days > 0 else "recycle-bin-floor-none",
                fields={"days": days},
            )
        )

    sid = current_user_sid()
    if not sid:
        if notes is not None:
            notes.append(CleanWarning(code="recycle-bin-sid-unknown", fields={}))
        return []

    roots = _fixed_volumes() if volumes is None else [Path(v) for v in volumes]
    candidates: list[CleanCandidate] = []
    deferred_bytes = 0
    deferred_count = 0
    undatable = 0

    for volume in roots:
        if volumes is not None and not _fixed_volume(volume):
            continue
        bin_dir = volume / RECYCLE_BIN_DIRNAME / sid
        for info in _bin_entries(bin_dir):
            payload = _payload_sibling(info)
            if not os.path.exists(payload):
                # Un `$I` sans `$R` : métadonnées orphelines, aucune charge utile
                # à récupérer. Rien à proposer, et surtout pas le `$I` seul.
                continue
            stamp = _deleted_at(info, payload, moment)
            if stamp is None:
                undatable += 1
                continue
            size = _payload_size(payload)
            if stamp > floor:
                deferred_count += 1
                if size is not None:
                    deferred_bytes += size
                continue
            candidates.append(
                CleanCandidate(
                    module="recycle-bin",
                    path=str(payload),
                    label=f"corbeille {volume_of(volume)}",
                    estimated_bytes=size,
                    level=Level.AGGRESSIVE,
                    reason=(
                        "mis en corbeille le "
                        + stamp.astimezone().strftime("%Y-%m-%d %H:%M")
                        + f" (plancher : {days} jour(s)) - suppression définitive, "
                        "la corbeille ne défait pas ce run"
                    ),
                    no_undo=True,
                    stat_mtime=_mtime_stat(payload),
                )
            )

    if notes is not None and deferred_count:
        notes.append(
            CleanWarning(
                code="recycle-bin-not-yet-eligible",
                fields={
                    "count": deferred_count,
                    "days": days,
                    "bytes": deferred_bytes,
                    "size": human_size(deferred_bytes),
                },
            )
        )
    if notes is not None and undatable:
        notes.append(
            CleanWarning(code="recycle-bin-undatable", fields={"count": undatable})
        )
    return candidates


def _mtime_stat(path: Path) -> float | None:
    """`st_mtime` brut du candidat, pour le garde secondaire de l'application."""
    try:
        return path.stat().st_mtime
    except OSError:  # pragma: no cover - disparu entre deux stats
        return None


def clean_recycle_bin(
    candidates: Sequence[CleanCandidate] | None = None,
    recycle: bool = False,
    yes: bool = False,
    **_kw: object,
) -> CleanResult:
    """Retire chaque paire : le `$R` d'abord, puis son `$I`.

    `recycle` est ignoré — mettre la corbeille à la corbeille n'a pas de sens, et
    `recycling_enabled()` rend déjà `False` à ce niveau. `yes` ne sert pas non
    plus : la confirmation de ce niveau est traitée par le CLI.

    Le `$I` n'est retiré que si le `$R` a réellement disparu. Un `$R` verrouillé
    dont on effacerait quand même les métadonnées deviendrait une charge utile
    orpheline, invisible du shell et impossible à restaurer.
    """
    del recycle, yes
    result = CleanResult(module="recycle-bin", freed=0, recycled=0, failed=0)
    for candidate in candidates or ():
        if not candidate.path:  # pragma: no cover - garanti par la découverte
            continue
        payload = Path(candidate.path)
        _remove_into(payload, result)
        if os.path.exists(payload):
            continue
        info = info_sibling(payload)
        if info is None or not os.path.exists(info):
            continue
        _remove_into(info, result)
    return result


def _remove_into(path: Path, result: CleanResult) -> None:
    """Supprime un chemin et verse ses octets dans le résultat du module."""
    freed, failed, errors = remove.delete_tree(path)
    result.freed = (result.freed or 0) + freed
    if failed:
        result.failed = (result.failed or 0) + failed
    for error in errors:
        if remove.is_lock_error(error):
            result.locked_paths.append(error.path)


# --------------------------------------------------------------------------- #
# `package-cache`
# --------------------------------------------------------------------------- #


def discover_package_cache(
    env: Mapping[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """`%ProgramData%\\Package Cache` en un candidat, avec son avertissement."""
    environment = os.environ if env is None else env
    value = environment.get("PROGRAMDATA")
    if not value:
        return []
    base = Path(value) / PACKAGE_CACHE_RELATIVE
    if not base.is_dir():
        return []
    try:
        mtime: float | None = base.stat().st_mtime
    except OSError:  # pragma: no cover - lu juste au-dessus
        mtime = None
    return [
        CleanCandidate(
            module="package-cache",
            path=str(base),
            label="package cache",
            estimated_bytes=estimate_path(base),
            level=Level.AGGRESSIVE,
            reason=f"%ProgramData%\\{PACKAGE_CACHE_RELATIVE} - {PACKAGE_CACHE_WARNING}",
            no_undo=True,
            stat_mtime=mtime,
        )
    ]
