"""Registre des modules. Unique site de déclaration de leurs propriétés.

Le CLI lit `discovery`, `proc_guard`, `needs_network` et `opt_in` sur
l'enregistrement et se comporte en conséquence. Ajouter un module ici suffit à
le faire prendre en charge partout ; la seule donnée ciblée est routée ici vers
l'adaptateur qui la comprend.

`needs_network` est déclaré ici et estampillé sur les candidats par
`discover_module()`. Aucun `discover_*()` ne le fixe lui-même : deux sites de
vérité laisseraient le registre annoncer `True` pendant que la fonction émet
`False`, et le test de correspondance resterait au vert.
"""

from __future__ import annotations

import functools
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import (  # noqa: E402
    mod_apps,
    mod_dev,
    mod_linux_cache,
    mod_linux_pkg,
    mod_linux_system,
    mod_ollama,
    mod_system,
    procs,
)
from scripts.winclean.common import (  # noqa: E402
    DISCOVERY_FIXED,
    DISCOVERY_PATHLESS,
    DISCOVERY_WALKING,
    LEVEL_ORDER,
    PROC_GUARD_WARN_AND_SKIP,
    PROC_GUARD_WARN_ONLY,
    SKIP_RUNNING,
    CleanCandidate,
    CleanModule,
    Level,
    SkippedEntry,
)

__all__ = [
    "ValidationError",
    "ExtraConfirm",
    "MODULES",
    "MODULE_ORDER",
    "PROC_OWNERS",
    "EXTRA_CONFIRM",
    "extra_confirm",
    "ownership_rank",
    "modules_for_level",
    "validate_names",
    "validate_level",
    "select_modules",
    "requirements_met",
    "discover_module",
    "proc_owners",
    "proc_guard_state",
    "proc_guard_skip",
]


class ValidationError(Exception):
    """Sélection invalide : le run s'arrête avant toute découverte."""


def _cache_module(name: str, requires: tuple[str, ...] = ()) -> CleanModule:
    """Module `fixed` d'un cache de gestionnaire de paquets.

    Tous partagent la même forme : un chemin documenté, un candidat, et
    `needs_network = True` puisque le contenu se re-télécharge.
    """
    return CleanModule(
        name=name,
        level=Level.SAFE,
        requires=requires,
        discover=functools.partial(mod_dev.discover_cache, name),
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=True,
        opt_in=False,
    )


_REGISTERED: tuple[CleanModule, ...] = (
    CleanModule(
        name="cargo-target",
        level=Level.SAFE,
        requires=(),
        discover=mod_dev.discover_cargo_target,
        clean=None,
        discovery=DISCOVERY_WALKING,
        proc_guard=PROC_GUARD_WARN_AND_SKIP,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="pycache",
        level=Level.SAFE,
        requires=(),
        discover=mod_dev.discover_pycache,
        clean=None,
        discovery=DISCOVERY_WALKING,
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="dotnet-binobj",
        level=Level.SAFE,
        requires=(),
        discover=mod_dev.discover_dotnet_binobj,
        clean=None,
        discovery=DISCOVERY_WALKING,
        proc_guard=PROC_GUARD_WARN_AND_SKIP,
        needs_network=False,
        opt_in=False,
    ),
    _cache_module("cargo-registry"),
    _cache_module("npm-cache"),
    # Seul module à exiger son binaire : le store pnpm vit où pnpm le dit (par
    # volume), et le repli `%LOCALAPPDATA%` désigne souvent un dossier vide.
    _cache_module("pnpm-store", requires=("pnpm",)),
    _cache_module("yarn-cache"),
    _cache_module("bun-cache"),
    _cache_module("pip-cache"),
    _cache_module("uv-cache"),
    _cache_module("nuget-packages"),
    # --- caches de gestionnaires de paquets, Linux ------------------------- #
    # Même forme que `_cache_module` (fixed, needs_network=True), sans passer
    # par elle : `mod_linux_pkg.discover_cache`/`discover_apt_archives`
    # prennent `env`, pas la signature `(module, roots=..., trash_days=...)`
    # que `_cache_module` fige via `functools.partial`.
    CleanModule(
        name="pip-cache-linux",
        level=Level.SAFE,
        requires=(),
        discover=functools.partial(mod_linux_pkg.discover_cache, "pip-cache-linux"),
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=True,
        opt_in=False,
    ),
    CleanModule(
        name="pnpm-store-linux",
        level=Level.SAFE,
        # Même décision que `pnpm-store` côté Windows : le store pnpm vit où
        # pnpm le dit, et le repli XDG désigne souvent un dossier vide.
        requires=("pnpm",),
        discover=functools.partial(mod_linux_pkg.discover_cache, "pnpm-store-linux"),
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=True,
        opt_in=False,
    ),
    CleanModule(
        name="apt-cache",
        level=Level.SAFE,
        requires=(),
        discover=mod_linux_pkg.discover_apt_archives,
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=True,
        opt_in=False,
    ),
    # --- niveau `moderate` ------------------------------------------------- #
    # Aucun ne parcourt les racines : quatre chemins documentés par utilisateur
    # (`fixed`) et un seul candidat sans chemin (`pathless`). Tous portent
    # `needs_network=False` : aucun ne se re-télécharge depuis un registre de
    # paquets, donc `--offline` n'a aucune raison de les écarter.
    CleanModule(
        name="browser-cache",
        level=Level.MODERATE,
        requires=(),
        discover=mod_apps.discover_browser_cache,
        clean=None,
        discovery=DISCOVERY_FIXED,
        # `warn-only` et non `warn-and-skip` : un navigateur ouvert tient ses
        # fichiers de cache **ouverts**, il ne réécrit pas l'arbre supprimé. Le
        # résultat attendu est un `winerror 32` rapporté dans `locked_paths`, pas
        # une omission silencieuse de tous les candidats du module.
        proc_guard=PROC_GUARD_WARN_ONLY,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="vscode-cache",
        level=Level.MODERATE,
        requires=(),
        discover=mod_apps.discover_vscode_cache,
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=PROC_GUARD_WARN_ONLY,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="user-temp",
        level=Level.MODERATE,
        requires=(),
        discover=mod_apps.discover_user_temp,
        clean=None,
        discovery=DISCOVERY_FIXED,
        # `None` est un constat sur `%TEMP%` : les deux forces de garde
        # interrogent des propriétaires **nommés**, et `%TEMP%` n'en a pas.
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="crashdumps",
        level=Level.MODERATE,
        requires=(),
        discover=mod_apps.discover_crashdumps,
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="docker-light",
        level=Level.MODERATE,
        requires=("docker",),
        discover=mod_apps.discover_docker_light,
        clean=mod_apps.clean_docker_light,
        discovery=DISCOVERY_PATHLESS,
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    # --- caches applicatifs, Linux ------------------------------------------ #
    # Même `proc_guard` que leurs équivalents Windows : `warn-only` pour le
    # cache de navigateurs (fichiers tenus ouverts, pas réécrits), `None` pour
    # le balayage générique (aucun propriétaire nommé pour `~/.cache`).
    CleanModule(
        name="browser-cache-linux",
        level=Level.MODERATE,
        requires=(),
        discover=mod_linux_cache.discover_browser_cache,
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=PROC_GUARD_WARN_ONLY,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="user-cache-linux",
        level=Level.MODERATE,
        requires=(),
        discover=mod_linux_cache.discover_user_cache,
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    # --- niveau `aggressive` ----------------------------------------------- #
    # Les deux suppriment en direct : `--recycle` est accepté et inerte ici
    # (décision 4). Les deux sont `fixed` — la corbeille s'énumère par volume
    # local, sans parcourir les racines — et `needs_network=False` : rien de ce
    # qu'ils retirent ne se re-télécharge depuis un registre de paquets.
    CleanModule(
        name="recycle-bin",
        level=Level.AGGRESSIVE,
        requires=(),
        discover=mod_system.discover_recycle_bin,
        # Seul module `aggressive` à porter son propre `clean()` : son unité de
        # suppression est la paire `$I`/`$R`, et un `CleanCandidate` n'a qu'un
        # champ `path`.
        clean=mod_system.clean_recycle_bin,
        discovery=DISCOVERY_FIXED,
        # Aucun propriétaire nommé : la corbeille n'appartient à aucun processus
        # qu'on saurait interroger, et `explorer.exe` tourne toujours.
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    CleanModule(
        name="package-cache",
        level=Level.AGGRESSIVE,
        requires=(),
        discover=mod_system.discover_package_cache,
        clean=None,
        discovery=DISCOVERY_FIXED,
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    # Contrepartie Linux : pas de corbeille ici (`trash_linux.py` couvre la
    # home trashcan freedesktop, câblée dans `remove.py`, pas dans un module
    # de ce registre). `journal-vacuum` porte son propre `clean()` pour la
    # même raison que `docker-light` : son unité de suppression n'est pas un
    # chemin, `apply_plan()` ne route donc les candidats sans chemin que vers
    # un module qui définit `clean`.
    CleanModule(
        name="journal-vacuum",
        level=Level.AGGRESSIVE,
        requires=(),
        discover=mod_linux_system.discover_journal_vacuum,
        clean=mod_linux_system.clean_journal_vacuum,
        discovery=DISCOVERY_PATHLESS,
        proc_guard=None,
        needs_network=False,
        opt_in=False,
    ),
    # Modèles utilisateur : jamais inclus par un niveau large. Le CLI exige un
    # nom canonique explicite et l'adaptateur délègue exclusivement à Ollama.
    CleanModule(
        name=mod_ollama.MODULE_NAME,
        level=Level.AGGRESSIVE,
        requires=(),
        discover=mod_ollama.discover_ollama_models,
        clean=mod_ollama.clean_ollama_models,
        discovery=DISCOVERY_PATHLESS,
        proc_guard=None,
        needs_network=True,
        opt_in=True,
    ),
)

MODULES: dict[str, CleanModule] = {m.name: m for m in _REGISTERED}

#: Rang de possession. Ne tranche que l'égalité de chemin **exacte** dans
#: `absorb_nested` : le module le plus spécifique d'un chemin est déclaré avant
#: celui qui ne fait que le contenir.
MODULE_ORDER: tuple[str, ...] = tuple(m.name for m in _REGISTERED)

#: Processus dont l'activité rend un candidat instable, par module. Table lue à
#: la fois par la découverte (pour l'avertissement) et par l'application (pour
#: l'omission) : une seule déclaration, deux lecteurs.
PROC_OWNERS: dict[str, tuple[str, ...]] = {
    "cargo-target": mod_dev.CARGO_OWNERS,
    "dotnet-binobj": mod_dev.DOTNET_OWNERS,
    "browser-cache": mod_apps.BROWSER_OWNERS,
    "vscode-cache": mod_apps.EDITOR_OWNERS,
    "browser-cache-linux": mod_linux_cache.BROWSER_OWNERS,
}


@dataclass(frozen=True)
class ExtraConfirm:
    """Seconde confirmation propre à un module, au-delà de celle du niveau.

    `flag` est le drapeau qui y répond d'avance. Il est **distinct de `--yes`** :
    la décision 4 ne donne à `--yes` que deux effets (répondre à la confirmation
    de niveau, lever les omissions de garde de processus), et un `--yes` qui
    emporterait aussi celle-ci ferait exactement ce que cette table existe pour
    empêcher — un run non surveillé qui casse les réparations MSI de la machine.
    """

    question: str
    flag: str


#: Modules exigeant une confirmation à eux, par nom. Même forme que
#: `PROC_OWNERS` : une table nom → données, lue par le CLI qui ne connaît aucun
#: nom de module. Un module absent de la table n'a rien de plus à demander.
EXTRA_CONFIRM: dict[str, ExtraConfirm] = {
    "package-cache": ExtraConfirm(
        question=mod_system.PACKAGE_CACHE_CONFIRM,
        flag="--yes-package-cache",
    ),
}


def extra_confirm(module: str) -> ExtraConfirm | None:
    """Confirmation supplémentaire de ce module, ou `None`."""
    return EXTRA_CONFIRM.get(module)


def _registry(registry: Mapping[str, CleanModule] | None) -> Mapping[str, CleanModule]:
    return MODULES if registry is None else registry


def ownership_rank(module: str, registry: Mapping[str, CleanModule] | None = None) -> int:
    """Index du module dans l'ordre de possession ; inconnu = dernier."""
    order = MODULE_ORDER if registry is None else tuple(registry)
    try:
        return order.index(module)
    except ValueError:
        return len(order)


def modules_for_level(
    level: Level, registry: Mapping[str, CleanModule] | None = None
) -> list[CleanModule]:
    """Modules dont le niveau est atteint par `level`, dans l'ordre déclaré."""
    ceiling = LEVEL_ORDER.index(level)
    return [
        m
        for m in _registry(registry).values()
        if not m.opt_in and LEVEL_ORDER.index(m.level) <= ceiling
    ]


def validate_names(
    names: Iterable[str],
    registry: Mapping[str, CleanModule] | None = None,
    *,
    disabled: Iterable[str] = (),
    config_source: str | None = None,
) -> None:
    """Refuse un nom inconnu **avant** la découverte, en listant les valides.

    Une faute de frappe dans `--only` doit coûter une erreur, pas un run qui
    parcourt des disques pour finir sur « rien à nettoyer » : l'utilisateur en
    conclurait que sa machine est propre.

    `disabled` porte les modules que la configuration a désactivés, et n'est
    passé que depuis le chemin `--only` : dans `--skip` le même nom est une
    redondance légitime, l'union des deux étant déjà la règle. Nommer un module
    désactivé dans `--only` est en revanche une erreur non nulle qui **nomme le
    fichier et la clé** — le laisser filer rendrait un plan vide, indiscernable
    d'un « rien à nettoyer » pour un module que l'utilisateur a demandé
    explicitement.
    """
    known = _registry(registry)
    unknown = [n for n in names if n not in known]
    if unknown:
        raise ValidationError(
            "module inconnu : "
            + ", ".join(sorted(unknown))
            + " - noms valides : "
            + ", ".join(known)
        )
    blocked = set(disabled)
    refused = [n for n in names if n in blocked]
    if not refused:
        return
    source = config_source or "le fichier de configuration"
    raise ValidationError(
        "module désactivé par la configuration : "
        + ", ".join(sorted(refused))
        + f" - retiré par DISABLED_MODULES dans {source}"
        + " - retirer ce nom de la clé, ou ne pas le demander avec --only"
    )


def validate_level(
    names: Iterable[str],
    level: Level,
    registry: Mapping[str, CleanModule] | None = None,
    *,
    disabled: Iterable[str] = (),
    config_source: str | None = None,
) -> None:
    """Refuse un module demandé au-dessus du niveau actif, en nommant le niveau.

    Le silence serait pire que l'erreur : `--only <module moderate>` sans
    `--level moderate` rendrait un plan vide, indiscernable d'un module qui n'a
    rien trouvé. `validate_names()` passe d'abord, sinon un nom inconnu
    ressortirait comme un problème de niveau.
    """
    validate_names(names, registry, disabled=disabled, config_source=config_source)
    known = _registry(registry)
    ceiling = LEVEL_ORDER.index(level)
    too_high = [n for n in names if LEVEL_ORDER.index(known[n].level) > ceiling]
    if not too_high:
        return
    required = max((known[n].level for n in too_high), key=LEVEL_ORDER.index)
    raise ValidationError(
        "module au-dessus du niveau actif ("
        + str(level)
        + ") : "
        + ", ".join(sorted(too_high))
        + f" - relancer avec --level {required}"
    )


def select_modules(
    level: Level,
    only: Sequence[str] = (),
    skip: Sequence[str] = (),
    registry: Mapping[str, CleanModule] | None = None,
    *,
    disabled: Sequence[str] = (),
    config_source: str | None = None,
) -> list[CleanModule]:
    """Sélection finale : `--only` d'abord, `--skip` ensuite.

    L'ordre compte et il est celui-là : `--skip` retire de ce que `--only` a
    retenu, si bien que `--only pycache --skip pycache` ne sélectionne rien -
    sans erreur, l'utilisateur ayant dit les deux.

    `disabled` (la clé `DISABLED_MODULES` du fichier) **s'unit** à `--skip` au
    lieu d'être remplacé par lui : c'est une protection, et une protection que la
    ligne de commande défait ne protège pas un run non surveillé. Un run ordinaire
    omet donc le module en silence — c'est ce que le fichier demande — et seul
    `--only` sur ce nom lève une erreur, parce que là l'utilisateur l'a réclamé.
    """
    known = _registry(registry)
    if only:
        validate_level(
            only, level, registry, disabled=disabled, config_source=config_source
        )
        chosen = [known[n] for n in only]
    else:
        chosen = modules_for_level(level, registry)
    validate_names(skip, registry)
    excluded = set(skip) | set(disabled)
    return [m for m in chosen if m.name not in excluded]


def requirements_met(module: CleanModule) -> bool:
    """Vrai si tous les binaires exigés par le module sont sur le `PATH`."""
    return all(shutil.which(binary) is not None for binary in module.requires)


def discover_module(
    module: CleanModule,
    *,
    ollama_models: Sequence[str] = (),
    **kwargs: object,
) -> list[CleanCandidate]:
    """Découverte d'un module, avec estampillage de `needs_network`.

    Unique appelant de `discover()` dans le programme : c'est ici que la valeur
    déclarée devient la valeur portée. Un module dont un `requires` manque rend
    zéro candidat, sans message : l'outil n'est pas installé, il n'y a pas
    d'anomalie à signaler.
    """
    if not requirements_met(module):
        return []
    if module.name == mod_ollama.MODULE_NAME:
        found = module.discover(requested_models=tuple(ollama_models), **kwargs)
    else:
        found = module.discover(**kwargs)
    if not module.needs_network:
        return found
    for candidate in found:
        candidate.needs_network = True
    return found


def proc_owners(module: str) -> tuple[str, ...]:
    """Processus surveillés pour ce module, vide s'il n'en surveille aucun."""
    return PROC_OWNERS.get(module, ())


def proc_guard_state(module: str) -> tuple[set[str] | None, tuple[str, ...]]:
    """État des propriétaires : `(correspondances, surveillés)`.

    `set()` = interrogé, rien ne tourne. `None` = pas pu interroger. Sans
    propriétaire déclaré, rien n'est lancé du tout.
    """
    owners = proc_owners(module)
    if not owners:
        return set(), ()
    return procs.is_running(owners), owners


def proc_guard_skip(
    module: CleanModule,
    candidate: CleanCandidate,
    *,
    yes: bool,
    state: tuple[set[str] | None, tuple[str, ...]] | None = None,
) -> SkippedEntry | None:
    """Omission due à un propriétaire actif, ou `None` s'il faut agir.

    Seul `warn-and-skip` omet. `warn-only` avertit et laisse agir : la
    différence entre les deux forces tient entièrement ici, pas dans le texte de
    l'avertissement, qui est le même. `--yes` lève l'omission - l'utilisateur a
    répondu de son état de machine.
    """
    if module.proc_guard != PROC_GUARD_WARN_AND_SKIP or yes:
        return None
    matched, owners = proc_guard_state(module.name) if state is None else state
    if not procs.unknown_is_running(matched):
        return None
    reason = procs.owner_reason(matched, owners)
    return SkippedEntry(
        label=candidate.label,
        path=candidate.path,
        status=SKIP_RUNNING,
        reason=reason or "propriétaire actif",
    )
