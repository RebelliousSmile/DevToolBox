"""Modules `moderate` : caches d'applications, `%TEMP%`, CrashDumps, Docker.

Comme `mod_dev.py`, ce fichier n'importe jamais `remove` — sauf que l'un de ses
modules n'a pas de chemin du tout et récupère l'espace par une commande externe :
`docker-light` porte son propre `clean()`, qui appelle `docker system prune -f` et
rien d'autre.

Deux règles gouvernent tous les modules de caches d'ici, et elles sont
solidaires :

1. **Liste blanche fermée.** Un nom n'est candidat que si l'application le
   reconstitue seule, sans perte visible pour l'utilisateur. Tout ce qui n'est
   pas listé n'est jamais proposé, y compris ce qui *ressemble* à un cache :
   `Local Storage`, `IndexedDB`, `Service Worker\\CacheStorage`, `Cookies`,
   `Login Data` et `Bookmarks` sont des données de site et d'utilisateur — les
   documents hors ligne d'une PWA vivent dans les deux premiers — et `moderate`
   est annoncé comme coûtant des sessions web, pas des fichiers.
2. **Aucune racine de profil n'est ajoutée à l'ensemble protégé.** `is_protected()`
   correspond à un chemin exact *ou à un sous-arbre résolu* : protéger
   `<profil>` protégerait le `Cache` que ces modules existent pour retirer. Les
   deux règles sont justes séparément et s'annulent ensemble — la liste blanche
   est le seul mécanisme, et il suffit.

Un profil ne portant aucun nom de la liste ne rend **rien** pour ce profil : il
ne se replie jamais sur sa racine.

Aucun `discover_*()` ne décide `needs_network` (déclaré dans `registry_mod.py`).
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import procs  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    CleanCandidate,
    CleanResult,
    Level,
    estimate_path,
)

__all__ = [
    "BROWSER_OWNERS",
    "EDITOR_OWNERS",
    "BROWSER_SPECS",
    "VSCODE_SPECS",
    "VSCODE_CACHE_NAMES",
    "DOCKER_DF_COMMAND",
    "DOCKER_PRUNE_COMMAND",
    "BrowserSpec",
    "AppCacheSpec",
    "discover_browser_cache",
    "discover_vscode_cache",
    "discover_user_temp",
    "discover_crashdumps",
    "discover_docker_light",
    "docker_available",
    "clean_docker_light",
]

#: Propriétaires surveillés. La détection reste informative : jamais de
#: terminaison, jamais de suspension, jamais d'élévation (contrat de `procs.py`).
BROWSER_OWNERS: tuple[str, ...] = (
    "chrome.exe",
    "msedge.exe",
    "brave.exe",
    "vivaldi.exe",
    "firefox.exe",
)
EDITOR_OWNERS: tuple[str, ...] = ("Code.exe", "Cursor.exe")

_TOOL_TIMEOUT_SECONDS = 20
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0) if sys.platform == "win32" else 0


# --------------------------------------------------------------------------- #
# Outils communs
# --------------------------------------------------------------------------- #


def _env(env: Mapping[str, str] | None) -> Mapping[str, str]:
    return os.environ if env is None else env


def _base(env: Mapping[str, str], key: str, relative: str) -> Path | None:
    """Chemin `%KEY%\\relative`, ou `None` si la variable est absente."""
    value = env.get(key)
    if not value:
        return None
    return Path(value) / relative if relative else Path(value)


def _candidate(
    module: str,
    path: Path,
    label: str,
    reason: str,
    warning: str | None = None,
    no_undo: bool = False,
) -> CleanCandidate:
    """Candidat `moderate` porteur de chemin, horodaté pour le garde secondaire."""
    try:
        mtime: float | None = path.stat().st_mtime
    except OSError:
        mtime = None
    return CleanCandidate(
        module=module,
        path=str(path),
        label=label,
        estimated_bytes=estimate_path(path),
        level=Level.MODERATE,
        reason=reason if warning is None else f"{reason} - {warning}",
        no_undo=no_undo,
        stat_mtime=mtime,
    )


def _owner_reason(owners: Sequence[str]) -> str | None:
    """Avertissement à joindre à la raison, ou `None` si rien ne tourne.

    Un `None` de `procs.is_running` — liste de processus illisible — produit une
    phrase disant que l'état est **inconnu**. Un module `warn-only` n'omet
    jamais, donc cette phrase est le seul signal que l'utilisateur reçoit :
    n'imprimer rien affirmerait que le navigateur est fermé.
    """
    return procs.owner_reason(procs.is_running(owners), owners)


def _first_level_dirs(base: Path) -> list[Path]:
    """Sous-répertoires immédiats de `base`, triés, ou rien si illisible."""
    try:
        with os.scandir(base) as iterator:
            found = [
                Path(entry.path)
                for entry in iterator
                if _is_dir(entry)
            ]
    except OSError:
        return []
    return sorted(found, key=lambda p: os.path.normcase(str(p)))


def _is_dir(entry: os.DirEntry[str]) -> bool:
    try:
        return entry.is_dir(follow_symlinks=False)
    except OSError:
        return False


def _allowlisted(
    module: str,
    profile: Path,
    names: Sequence[str],
    label_prefix: str,
    reason: str,
    warning: str | None,
) -> list[CleanCandidate]:
    """Les seuls noms de `names` réellement présents sous `profile`.

    Fermé par construction : la boucle porte sur la liste blanche, pas sur le
    contenu du répertoire, donc aucun nom non listé ne peut devenir candidat -
    ce qu'un motif `*Cache*` ne garantit pas.
    """
    out: list[CleanCandidate] = []
    for name in names:
        path = profile / name
        if not path.is_dir():
            continue
        out.append(
            _candidate(
                module=module,
                path=path,
                label=f"{label_prefix} {name}",
                reason=reason,
                warning=warning,
            )
        )
    return out


# --------------------------------------------------------------------------- #
# `browser-cache`
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class BrowserSpec:
    """Une famille de navigateurs : où sont ses profils, et quoi y retirer.

    `relative` désigne le répertoire dont les **sous-répertoires immédiats** sont
    les profils : `User Data` chez les Chromium, `Profiles` chez Firefox, qui n'a
    aucun `User Data`. Les deux familles ne partagent pas leur disposition, donc
    la découverte est par famille - l'énumérer « sous `User Data` » ne trouverait
    zéro profil Firefox, et le module couvrirait quatre des cinq processus qu'il
    surveille.
    """

    label: str
    base_env: str
    relative: str
    cache_names: tuple[str, ...]


#: Chez les Chromium, la forme est `Cache` / `Code Cache` / `GPUCache`.
CHROMIUM_CACHE_NAMES: tuple[str, ...] = ("Cache", "Code Cache", "GPUCache")

#: Chez Firefox, le côté `%LOCALAPPDATA%` n'est **pas** uniformément du cache :
#: `settings`, `remote-settings`, `safebrowsing` et `personality-provider` y
#: cohabitent avec ce qui suit et n'en font pas partie.
FIREFOX_CACHE_NAMES: tuple[str, ...] = ("cache2", "startupCache", "thumbnails", "jumpListCache")

BROWSER_SPECS: tuple[BrowserSpec, ...] = (
    BrowserSpec("Chrome", "LOCALAPPDATA", "Google\\Chrome\\User Data", CHROMIUM_CACHE_NAMES),
    BrowserSpec("Edge", "LOCALAPPDATA", "Microsoft\\Edge\\User Data", CHROMIUM_CACHE_NAMES),
    BrowserSpec(
        "Brave",
        "LOCALAPPDATA",
        "BraveSoftware\\Brave-Browser\\User Data",
        CHROMIUM_CACHE_NAMES,
    ),
    BrowserSpec("Vivaldi", "LOCALAPPDATA", "Vivaldi\\User Data", CHROMIUM_CACHE_NAMES),
    BrowserSpec(
        "Firefox",
        "LOCALAPPDATA",
        "Mozilla\\Firefox\\Profiles",
        FIREFOX_CACHE_NAMES,
    ),
)


def discover_browser_cache(
    env: Mapping[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Caches de navigateurs, par famille et par profil, liste blanche seulement."""
    environment = _env(env)
    warning = _owner_reason(BROWSER_OWNERS)
    out: list[CleanCandidate] = []
    for spec in BROWSER_SPECS:
        base = _base(environment, spec.base_env, spec.relative)
        if base is None or not base.is_dir():
            continue
        for profile in _first_level_dirs(base):
            out.extend(
                _allowlisted(
                    module="browser-cache",
                    profile=profile,
                    names=spec.cache_names,
                    label_prefix=f"{spec.label} {profile.name} :",
                    reason=(
                        f"cache {spec.label} reconstitué à la navigation "
                        "(sessions web perdues, pas de fichier)"
                    ),
                    warning=warning,
                )
            )
    return out


# --------------------------------------------------------------------------- #
# `vscode-cache`
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class AppCacheSpec:
    """Une application dont les caches sont directement sous un dossier connu."""

    label: str
    base_env: str
    relative: str


#: Exactement six noms. Les trois autres répertoires en `Cached*` du vrai
#: agencement - `CachedConfigurations`, `CachedProfilesData`,
#: `CachedExtensionVSIXs` - sont **exclus** : les deux premiers nomment de l'état
#: de configuration et de profil, dont rien n'établit qu'il se régénère sans
#: perte visible ; le troisième contient des paquets d'extensions que seul un
#: **re-téléchargement** restaure, ce qui contredirait le `needs_network = False`
#: du module. `logs` et `User` ne sont pas des caches du tout - `User` porte les
#: réglages, les raccourcis, les extraits et `workspaceStorage`. C'est la raison
#: pour laquelle un motif `*Cache*` est interdit et pas seulement déconseillé.
VSCODE_CACHE_NAMES: tuple[str, ...] = (
    "Cache",
    "Code Cache",
    "GPUCache",
    "CachedData",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
)

VSCODE_SPECS: tuple[AppCacheSpec, ...] = (
    AppCacheSpec("VS Code", "APPDATA", "Code"),
    AppCacheSpec("Cursor", "APPDATA", "Cursor"),
)


def discover_vscode_cache(
    env: Mapping[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Caches de VS Code et de Cursor : les six noms de la liste blanche."""
    environment = _env(env)
    warning = _owner_reason(EDITOR_OWNERS)
    out: list[CleanCandidate] = []
    for spec in VSCODE_SPECS:
        base = _base(environment, spec.base_env, spec.relative)
        if base is None or not base.is_dir():
            continue
        out.extend(
            _allowlisted(
                module="vscode-cache",
                profile=base,
                names=VSCODE_CACHE_NAMES,
                label_prefix=f"{spec.label} :",
                reason=f"cache {spec.label} régénéré au démarrage de l'éditeur",
                warning=warning,
            )
        )
    return out


# --------------------------------------------------------------------------- #
# `user-temp` et `crashdumps`
# --------------------------------------------------------------------------- #


def _first_level_entries(base: Path) -> list[Path]:
    """Entrées immédiates de `base`, fichiers **et** répertoires, triées."""
    try:
        with os.scandir(base) as iterator:
            found = [Path(entry.path) for entry in iterator]
    except OSError:
        return []
    return sorted(found, key=lambda p: os.path.normcase(str(p)))


def discover_user_temp(
    env: Mapping[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Entrées de premier niveau de `%TEMP%`. Aucune auto-exclusion.

    winclean n'écrit rien sous `%TEMP%` pendant un run (`--out` écrit là où
    l'utilisateur a demandé), il n'y a donc rien de sien à exclure. Une entrée
    qu'un autre processus tient ouverte ressort en `winerror 32` dans
    `locked_paths` plutôt que d'être filtrée d'avance.

    Ce module déclare `proc_guard = None`, et c'est un constat sur `%TEMP%` :
    les deux forces de garde interrogent des processus **propriétaires nommés**,
    et `%TEMP%` n'a pas de propriétaire — c'est l'espace de travail partagé de
    tous les processus de la machine. Le garde secondaire (re-`stat` du mtime)
    est donc son seul garde, et un mtime de répertoire ne bouge que pour ses
    entrées **directes** : un installeur qui dépose dans `%TEMP%\\<guid>\\payload\\`
    est invisible aux deux. Risque résiduel documenté dans le README.
    """
    environment = _env(env)
    base = _base(environment, "TEMP", "")
    if base is None or not base.is_dir():
        return []
    return [
        _candidate(
            module="user-temp",
            path=entry,
            label=f"%TEMP% {entry.name}",
            reason="fichier temporaire, recréé par son producteur au besoin",
        )
        for entry in _first_level_entries(base)
    ]


def discover_crashdumps(
    env: Mapping[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Contenu de `%LOCALAPPDATA%\\CrashDumps`, entrée par entrée."""
    environment = _env(env)
    base = _base(environment, "LOCALAPPDATA", "CrashDumps")
    if base is None or not base.is_dir():
        return []
    return [
        _candidate(
            module="crashdumps",
            path=entry,
            label=f"vidage {entry.name}",
            reason="vidage mémoire d'un plantage passé, sans usage après diagnostic",
        )
        for entry in _first_level_entries(base)
    ]


# --------------------------------------------------------------------------- #
# `docker-light` : le seul module sans chemin de la v1
# --------------------------------------------------------------------------- #

#: Sonde du démon **et** rien d'autre : c'est le statut de sortie de cette
#: commande qui dit si le démon répond. Il n'existe aucun `docker info` dans le
#: paquet — deux mécanismes rendraient le test impassable sur la machine, très
#: commune, où le CLI est installé et le démon arrêté.
DOCKER_DF_COMMAND: tuple[str, ...] = ("docker", "system", "df", "--format", "json")

#: Ni `--volumes` ni `-a` : les volumes portent des données irremplaçables
#: (décision figée 7) et `-a` retirerait des images étiquetées mais inutilisées.
DOCKER_PRUNE_COMMAND: tuple[str, ...] = ("docker", "system", "prune", "-f")

#: Libellé obligatoire d'un candidat sans chemin : il nomme les trois sortes de
#: ressources retirées, puisqu'aucun chemin ne les nomme à sa place.
DOCKER_LABEL = (
    "Docker : images sans étiquette, conteneurs arrêtés, cache de build"
)


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    try:
        return subprocess.run(
            list(command),
            capture_output=True,
            text=True,
            timeout=_TOOL_TIMEOUT_SECONDS,
            creationflags=_NO_WINDOW,
        )
    except (OSError, subprocess.SubprocessError):
        return None


def docker_available() -> bool:
    """Sonde en deux étapes, et il n'y en a que deux.

    Le binaire par `shutil.which("docker")`, puis le démon par le **statut de
    sortie** de `docker system df --format json`. Échec fermé : pas de binaire,
    ou sortie non nulle, rend zéro candidat et n'écrit rien sur stderr.
    """
    if shutil.which("docker") is None:
        return False
    completed = _run(DOCKER_DF_COMMAND)
    return completed is not None and completed.returncode == 0


def discover_docker_light(**_kw: object) -> list[CleanCandidate]:
    """Un candidat sans chemin, non tarifé, ou rien si Docker ne répond pas.

    `estimated_bytes` est `None` **par décision**. `docker system df
    --format json` émet un objet JSON **par ligne**, un par `Type`, et ses champs
    `Size` / `Reclaimable` sont des chaînes destinées à l'œil (`"6.271GB (35%)"`),
    pas des entiers d'octets : tout chiffre viendrait du ré-analyse d'une chaîne
    formatée, soit une précision inventée. Et la colonne `Reclaimable` n'est de
    toute façon pas le plafond de ce module — sa ligne `Local Volumes` compte des
    octets qu'un `prune` sans `--volumes` ne touche jamais, et sa ligne `Images`
    des images étiquetées que seul `-a` retire. La sommer annoncerait comme
    récupérable une base de données.
    """
    if not docker_available():
        return []
    return [
        CleanCandidate(
            module="docker-light",
            path=None,
            label=DOCKER_LABEL,
            estimated_bytes=None,
            level=Level.MODERATE,
            reason="reconstruit par `docker build` / `docker pull` - volumes jamais touchés",
            no_undo=True,
        )
    ]


def clean_docker_light(
    candidates: Sequence[CleanCandidate] | None = None,
    recycle: bool = False,
    yes: bool = False,
    **_kw: object,
) -> CleanResult:
    """`docker system prune -f`. Les trois colonnes d'octets restent `None`.

    `measure_freed()` mesure un chemin avant et après ; il n'y a pas de chemin
    ici, donc `freed` est **`None`** : le module rend compte qu'il a tourné et
    que sa récupération n'est pas chiffrable. `0` serait faux de la façon la plus
    bruyante possible — un `prune` ayant libéré des gigaoctets afficherait
    « libéré 0 ». La ligne `Total reclaimed space:` que `prune` imprime n'est pas
    analysée : ce serait la seule colonne d'octets du paquet non issue d'une
    mesure.

    Une sortie non nulle est un échec de suppression autre qu'un verrou : la
    fonction lève, le run sort non nul, et `failed` reste `None` puisque les
    octets sont inconnus dans les deux sens.
    """
    del candidates, recycle, yes  # la commande est la même dans tous les cas
    completed = _run(DOCKER_PRUNE_COMMAND)
    if completed is None:
        raise OSError("docker system prune n'a pas pu être lancé")
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip().splitlines()
        raise OSError(
            "docker system prune a échoué (code "
            + str(completed.returncode)
            + ")"
            + (f" : {detail[-1]}" if detail else "")
        )
    return CleanResult(module="docker-light")


def _unused_json_guard() -> None:  # pragma: no cover - voir le commentaire
    """`json` est importé pour la seule raison documentée ci-dessous.

    La sortie de `docker system df --format json` n'est **pas** analysée (un
    objet par ligne, des tailles en chaînes de caractères) : seul son statut de
    sortie compte. Cette fonction existe pour que l'import ne soit pas retiré par
    un lecteur pressé, et pour dire pourquoi il ne sert à rien.
    """
    _ = json
