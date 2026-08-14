"""Résolution de chemins Linux, partagée par les modules `mod_linux_*.py`.

Unique site de résolution XDG pour ce paquet : les quatre modules Linux
(`mod_linux_pkg.py`, `mod_linux_cache.py`, `mod_linux_system.py`,
`trash_linux.py`) lisent `cache_home()`/`data_home()`/`trash_*_dir()` d'ici
plutôt que de relire `$XDG_*`/`$HOME` chacun de son côté - la même raison qui a
fait de `registry_mod.py` l'unique site de vérité des trois tables déclarées de
la partie Windows.

Délègue la résolution `$XDG_*` proprement dite à
`scripts.system_inventory.xdg_dirs` (Part 4) plutôt que de la dupliquer : ce
module n'ajoute que ce que la partie 4 n'avait pas besoin de connaître -
`$HOME` nu, et l'emplacement de la corbeille freedesktop
(https://specifications.freedesktop.org/trash-spec/trashspec-latest.html).

Chaque fonction accepte un `env` optionnel (comme `xdg_dirs`) : un test passe
un dict synthétique, la production lit `os.environ`.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.system_inventory.xdg_dirs import (  # noqa: E402
    cache_home,
    config_home,
    data_home,
    state_home,
)

__all__ = [
    "APT_ARCHIVES_DIR",
    "home",
    "config_home",
    "data_home",
    "cache_home",
    "state_home",
    "trash_home",
    "trash_files_dir",
    "trash_info_dir",
]

#: Cache des paquets `.deb` téléchargés sous Debian/Ubuntu. Chemin système fixe,
#: jamais dérivé de `$HOME` ni d'une variable XDG - distinct en cela de tout le
#: reste de ce module.
APT_ARCHIVES_DIR = Path("/var/cache/apt/archives")


def home(env: dict[str, str] | None = None) -> Path | None:
    """`$HOME`, ou `None` s'il est absent/vide.

    `xdg_dirs` lit déjà `HOME` en repli de chaque variable `$XDG_*` ; cette
    fonction expose la même lecture pour les cas qui n'ont pas besoin d'un
    sous-répertoire XDG particulier (par exemple `mod_linux_pkg.py` résolvant
    le repli documenté d'un outil dont la commande ne répond pas).
    """
    source = env if env is not None else os.environ
    value = source.get("HOME")
    return Path(value) if value else None


def trash_home(env: dict[str, str] | None = None) -> Path | None:
    """Racine de la corbeille freedesktop : `$XDG_DATA_HOME/Trash`.

    `None` seulement quand `data_home()` l'est déjà - mêmes conditions
    (`$XDG_DATA_HOME` et `$HOME` tous deux absents/vides) : le spec Trash ne
    documente aucun repli au-delà de celui de `XDG_DATA_HOME` lui-même.
    """
    base = data_home(env)
    return None if base is None else base / "Trash"


def trash_files_dir(env: dict[str, str] | None = None) -> Path | None:
    """`Trash/files/` - le contenu réellement déplacé, un élément par entrée."""
    base = trash_home(env)
    return None if base is None else base / "files"


def trash_info_dir(env: dict[str, str] | None = None) -> Path | None:
    """`Trash/info/` - un `.trashinfo` par élément : chemin d'origine et date."""
    base = trash_home(env)
    return None if base is None else base / "info"
