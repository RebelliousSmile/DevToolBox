"""Modules `moderate` : caches de navigateurs et balayage générique `~/.cache`.

Contrepartie de `mod_apps.py` côté Linux, même discipline : jamais d'import de
`remove`, jamais de `needs_network` auto-déclaré, liste blanche fermée pour les
caches de navigateurs - voir `mod_apps.py` pour la justification complète (un
nom n'est candidat que si l'application le reconstitue seule, sans perte
visible ; `Local Storage`, `IndexedDB`, `Cookies` en restent exclus).

`user-cache-linux` n'a pas d'équivalent Windows direct : `$XDG_CACHE_HOME` est
défini par la spécification freedesktop comme des données « non essentielles »
qu'il est censé être sûr de supprimer - contrairement à `%LOCALAPPDATA%`, qui
mélange cache et état applicatif dans un même arbre. Un balayage de premier
niveau y est donc défendable ; il exclut seulement les noms déjà couverts par
un autre module (`pip-cache-linux`, `browser-cache-linux`), pour qu'un même
octet ne soit jamais compté sous deux libellés à la fois.

La détection de processus réutilise `procs.py` tel quel : sa commande
(`tasklist`) échoue toujours sur Linux et rend donc systématiquement « état
inconnu », ce qui reste la réponse honnête tant que ce fichier n'a pas gagné
de détection `/proc` (voir le plan, Part 5, "Files to modify" - `procs.py`).
Aucun garde n'est donc actif en pratique aujourd'hui ; ce module ne le prétend
pas et n'invente aucune détection à sa place.
"""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import mod_linux_dev, platform_paths, procs  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    CleanCandidate,
    Level,
    estimate_path,
)

__all__ = [
    "BROWSER_OWNERS",
    "BROWSER_SPECS",
    "CHROMIUM_CACHE_NAMES",
    "FIREFOX_CACHE_NAMES",
    "ALREADY_COVERED_CACHE_NAMES",
    "BrowserSpec",
    "discover_browser_cache",
    "discover_user_cache",
]

#: Propriétaires surveillés, noms de processus Linux (pas `.exe`). La
#: détection reste informative - voir le contrat de `procs.py`.
BROWSER_OWNERS: tuple[str, ...] = (
    "firefox",
    "chrome",
    "google-chrome",
    "chromium",
    "chromium-browser",
    "brave",
    "brave-browser",
    "vivaldi-bin",
)


def _candidate(
    module: str,
    path: Path,
    label: str,
    reason: str,
    warning: str | None = None,
) -> CleanCandidate:
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
        stat_mtime=mtime,
    )


def _owner_reason(owners: Sequence[str]) -> str | None:
    return procs.owner_reason(procs.is_running(owners), owners)


def _first_level_dirs(base: Path) -> list[Path]:
    """Sous-répertoires immédiats de `base`, triés, ou rien si illisible."""
    try:
        with os.scandir(base) as iterator:
            found = [Path(entry.path) for entry in iterator if _is_dir(entry)]
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

    Fermé par construction, comme `mod_apps._allowlisted` : la boucle porte
    sur la liste blanche, jamais sur le contenu du répertoire.
    """
    out: list[CleanCandidate] = []
    for name in names:
        path = profile / name
        if not path.is_dir():
            continue
        out.append(
            _candidate(module=module, path=path, label=f"{label_prefix} {name}", reason=reason, warning=warning)
        )
    return out


# --------------------------------------------------------------------------- #
# `browser-cache-linux`
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class BrowserSpec:
    """Une famille de navigateurs : où sont ses profils sous `~/.cache`, et quoi y retirer.

    `relative` désigne, sous `platform_paths.cache_home()`, le répertoire dont
    les **sous-répertoires immédiats** sont les profils - Chromium et Firefox
    partagent cette forme sur Linux, contrairement à Windows où seul Firefox
    l'utilisait déjà (`Profiles`).
    """

    label: str
    relative: str
    cache_names: tuple[str, ...]


#: Chez les Chromium, la forme reste `Cache` / `Code Cache` / `GPUCache`,
#: identique à Windows : c'est le nom interne du moteur, pas de l'OS.
CHROMIUM_CACHE_NAMES: tuple[str, ...] = ("Cache", "Code Cache", "GPUCache")

#: Chez Firefox sur Linux, `jumpListCache` n'existe pas (spécifique aux listes
#: de raccourcis Windows) ; les trois autres noms sont partagés avec la
#: définition Windows de `mod_apps.py`.
FIREFOX_CACHE_NAMES: tuple[str, ...] = ("cache2", "startupCache", "thumbnails")

BROWSER_SPECS: tuple[BrowserSpec, ...] = (
    BrowserSpec("Chrome", "google-chrome", CHROMIUM_CACHE_NAMES),
    BrowserSpec("Chromium", "chromium", CHROMIUM_CACHE_NAMES),
    BrowserSpec("Brave", "BraveSoftware/Brave-Browser", CHROMIUM_CACHE_NAMES),
    BrowserSpec("Vivaldi", "vivaldi", CHROMIUM_CACHE_NAMES),
    BrowserSpec("Firefox", "mozilla/firefox", FIREFOX_CACHE_NAMES),
)


def discover_browser_cache(
    env: dict[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Caches de navigateurs sous `~/.cache`, par famille et par profil."""
    warning = _owner_reason(BROWSER_OWNERS)
    out: list[CleanCandidate] = []
    cache_root = platform_paths.cache_home(env)
    if cache_root is None:
        return []
    for spec in BROWSER_SPECS:
        base = cache_root / spec.relative
        if not base.is_dir():
            continue
        for profile in _first_level_dirs(base):
            out.extend(
                _allowlisted(
                    module="browser-cache-linux",
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
# `user-cache-linux` : balayage générique de `~/.cache`
# --------------------------------------------------------------------------- #

#: Noms déjà couverts par un autre module - exclus du balayage générique pour
#: qu'un même octet ne soit jamais compté sous deux libellés à la fois. Les
#: cinq derniers sont les racines de `BROWSER_SPECS` (leur premier segment) :
#: le balayage générique s'arrête au nom de vendeur, `discover_browser_cache`
#: descend jusqu'aux sous-répertoires de cache réels à l'intérieur.
ALREADY_COVERED_CACHE_NAMES: frozenset[str] = frozenset(
    {
        "pip",
        *mod_linux_dev.PLAYWRIGHT_CACHE_NAMES,
        "google-chrome",
        "chromium",
        "BraveSoftware",
        "vivaldi",
        "mozilla",
    }
)


def discover_user_cache(
    env: dict[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Sous-répertoires de premier niveau de `~/.cache`, hors noms déjà couverts.

    Défendable en balayage générique là où l'équivalent Windows ne l'est pas :
    `$XDG_CACHE_HOME` est documenté par la spécification freedesktop comme des
    données non essentielles, sûres à supprimer - `%LOCALAPPDATA%` mélange
    cache et état applicatif dans un même arbre, ce que `mod_apps.py` gère par
    une liste blanche fermée faute d'un tel contrat.
    """
    cache_root = platform_paths.cache_home(env)
    if cache_root is None or not cache_root.is_dir():
        return []
    return [
        _candidate(
            module="user-cache-linux",
            path=entry,
            label=f"~/.cache {entry.name}",
            reason="cache utilisateur XDG, reconstruit par son application propriétaire",
        )
        for entry in _first_level_dirs(cache_root)
        if entry.name not in ALREADY_COVERED_CACHE_NAMES
    ]
