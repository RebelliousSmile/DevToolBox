"""Fichier de configuration : une liste blanche qui ne peut que **restreindre**.

Trois propriétés de ce fichier sont des décisions, pas des détails :

1. **Le fichier est lu comme des données, jamais exécuté.** `json.load`, et rien
   d'autre : ni `eval`, ni `exec`, ni `import`. Un nettoyeur qui sourcerait sa
   configuration donnerait l'exécution de code à quiconque peut écrire dans
   `%APPDATA%`.
2. **Aucune clé n'élargit un run.** Il n'y en a pas pour `--apply`, pour le
   niveau ni pour `--yes`, et aucune ne nomme des racines ou une portée. La
   clé `ROOTS` a existé dans le plan de cette part et a été retirée : c'était la
   seule qui *ajoutait* des cibles de découverte, et choisir les arbres à
   nettoyer reste un acte par invocation (`--root`). Un `ROOTS` résiduel dans un
   fichier existant échoue donc bruyamment, comme n'importe quelle clé inconnue.
   La propriété est vérifiée à l'import par `_assert_restrictive()`, pas
   seulement dans les tests : une clé ajoutée un jour sans y penser fait échouer
   l'import du paquet.
3. **`None` sur un champ de `Config` veut dire « la clé était absente »**, pas
   « zéro ». C'est ce qui permet la résolution *CLI > fichier > défaut* sans
   confondre un `TRASH_DAYS: 0` écrit dans le fichier avec son absence.

Les deux clés à valeur d'ensemble (`PROTECTED_PATHS`, `DISABLED_MODULES`) ne
suivent **pas** la règle scalaire : elles s'unissent avec ce que la ligne de
commande dit au lieu d'être remplacées par elle. Ce sont des protections, et une
protection que le drapeau le plus courant peut défaire ne protège pas un run non
surveillé. Corollaire assumé : nommer un module désactivé dans `--only` est une
erreur non nulle (`registry_mod.validate_names`), jamais un plan vide qui se
lirait comme « rien à nettoyer ».

Le JSON strict n'admet pas de commentaire : `winclean.json.example` est donc du
JSON valide sans commentaire, et le commentaire de chaque clé vit dans le
README. Tolérer une clé de commentaire contredirait la validation de **toutes**
les clés, qui est le point de ce fichier.
"""

from __future__ import annotations

import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import guards, registry_mod  # noqa: E402
from scripts.winclean.common import CleanModule  # noqa: E402

__all__ = [
    "ConfigError",
    "DEFAULT_TRASH_DAYS",
    "DEFAULT_MAX_DELETE_BYTES",
    "ALLOWED_KEYS",
    "FORBIDDEN_KEY_FRAGMENTS",
    "Config",
    "DEFAULT_CONFIG",
    "default_config_path",
    "load_config",
    "resolve_trash_days",
    "resolve_max_delete_bytes",
    "protected_union",
    "disabled_union",
]

#: Décision 15, plancher d'âge de la corbeille. Non nul : une entrée supprimée il
#: y a cinq minutes doit survivre à un run non surveillé.
DEFAULT_TRASH_DAYS = 7
#: Décision 15, plafond d'octets du plan. Le fichier ne peut que l'abaisser.
DEFAULT_MAX_DELETE_BYTES = 50 * 1024**3


class ConfigError(Exception):
    """Configuration refusée : le run s'arrête avant toute découverte."""


# --------------------------------------------------------------------------- #
# Validateurs
# --------------------------------------------------------------------------- #


def _type_name(value: Any) -> str:
    return type(value).__name__


def _int_at_least(minimum: int) -> Callable[..., int]:
    def check(key: str, value: Any, _registry: Mapping[str, CleanModule]) -> int:
        # `bool` est un `int` en Python : `true` dans le fichier passerait pour 1.
        if isinstance(value, bool) or not isinstance(value, int):
            raise ConfigError(f"{key} attend un entier, reçu {_type_name(value)}")
        if value < minimum:
            raise ConfigError(f"{key} attend un entier >= {minimum}, reçu {value}")
        return value

    return check


def _string_list(key: str, value: Any) -> list[str]:
    if isinstance(value, str) or not isinstance(value, (list, tuple)):
        raise ConfigError(
            f"{key} attend une liste de chaînes, reçu {_type_name(value)}"
        )
    for item in value:
        if not isinstance(item, str) or not item.strip():
            raise ConfigError(
                f"{key} : entrée invalide {item!r} - une chaîne non vide est attendue"
            )
    return [str(item).strip() for item in value]


def _absolute_paths(
    key: str, value: Any, _registry: Mapping[str, CleanModule]
) -> tuple[str, ...]:
    """Chemins absolus normalisés. Un chemin relatif est refusé, pas résolu.

    Le résoudre le rendrait dépendant du répertoire courant, c'est-à-dire de
    l'endroit d'où le lanceur a démarré le processus : la protection couvrirait
    un arbre différent selon l'appelant.
    """
    entries = _string_list(key, value)
    normalised: list[str] = []
    for entry in entries:
        if not os.path.isabs(entry):
            raise ConfigError(
                f"{key} : chemin relatif refusé : {entry!r} - un chemin protégé "
                "doit être absolu, sinon il désigne un arbre différent selon le "
                "répertoire courant"
            )
        normalised.append(guards.normalise(entry))
    return tuple(dict.fromkeys(normalised))


def _module_names(
    key: str, value: Any, registry: Mapping[str, CleanModule]
) -> tuple[str, ...]:
    entries = _string_list(key, value)
    unknown = [name for name in entries if name not in registry]
    if unknown:
        raise ConfigError(
            f"{key} : module inconnu : "
            + ", ".join(sorted(unknown))
            + " - noms valides : "
            + ", ".join(registry)
        )
    return tuple(dict.fromkeys(entries))


#: Clé du fichier → `(champ de `Config`, validateur)`. Unique site de vérité de
#: ce qu'un fichier de configuration peut dire.
ALLOWED_KEYS: dict[str, tuple[str, Callable[..., Any]]] = {
    "TRASH_DAYS": ("trash_days", _int_at_least(0)),
    "MAX_DELETE_BYTES": ("max_delete_bytes", _int_at_least(1)),
    "PROTECTED_PATHS": ("protected_paths", _absolute_paths),
    "DISABLED_MODULES": ("disabled_modules", _module_names),
}

#: Fragments qu'un nom de clé ne peut pas porter : chacun désignerait un réglage
#: qui **élargit** le run. La liste est la formulation exécutable de la propriété
#: « aucune clé n'élargit » ; c'est elle, et non une énumération des clés
#: actuelles, que le test vérifie — la prochaine clé ajoutée est justement celle
#: qu'une liste figée n'aurait pas couverte.
FORBIDDEN_KEY_FRAGMENTS: tuple[str, ...] = (
    "APPLY",
    "LEVEL",
    "YES",
    "CONFIRM",
    "FORCE",
    "ROOT",
    "SCOPE",
    "TARGET",
    "INCLUDE",
    "EXTRA",
)


def _assert_restrictive() -> None:
    """Vérifie la propriété 2 du module à l'import, pas seulement dans les tests."""
    for key in ALLOWED_KEYS:
        upper = key.upper()
        for fragment in FORBIDDEN_KEY_FRAGMENTS:
            if fragment in upper:
                raise RuntimeError(
                    f"clé de configuration refusée : {key} contient {fragment!r} - "
                    "une clé qui élargit le run n'a pas sa place dans la liste blanche"
                )


_assert_restrictive()


# --------------------------------------------------------------------------- #
# Modèle
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class Config:
    """Ce qu'un fichier a dit. `None` = clé absente, jamais « zéro »."""

    source: str | None = None
    trash_days: int | None = None
    max_delete_bytes: int | None = None
    protected_paths: tuple[str, ...] = ()
    disabled_modules: tuple[str, ...] = ()


#: La configuration d'un run sans fichier. Aucun appelant n'a besoin de savoir
#: qu'il n'y en avait pas.
DEFAULT_CONFIG = Config()


def default_config_path(env: Mapping[str, str] | None = None) -> Path | None:
    """`%APPDATA%\\winclean\\winclean.json`, ou `None` si `%APPDATA%` est absent."""
    environment = os.environ if env is None else env
    base = environment.get("APPDATA")
    if not base:
        return None
    return Path(base) / "winclean" / "winclean.json"


def load_config(
    path: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
    registry: Mapping[str, CleanModule] | None = None,
) -> Config:
    """Lit et valide le fichier. Abandonne à la **première** clé ou valeur fautive.

    Un fichier absent à l'emplacement par défaut n'est pas une erreur : la
    configuration est facultative. Un `--config` qui **nomme** un fichier absent
    en est une, en revanche, et pour la même raison qu'un `--only` mal
    orthographié : le run continuerait sur les défauts en laissant croire que les
    restrictions demandées s'appliquent.
    """
    known = registry_mod.MODULES if registry is None else registry
    explicit = path is not None
    target = Path(path) if explicit else default_config_path(env)
    if target is None:
        return Config()
    if not target.exists():
        if explicit:
            raise ConfigError(f"fichier de configuration introuvable : {target}")
        return Config()

    try:
        raw = target.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigError(f"{target} : lecture impossible - {exc.strerror or exc}") from exc
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ConfigError(
            f"{target} : JSON illisible ligne {exc.lineno}, colonne {exc.colno} - {exc.msg}"
        ) from exc
    if not isinstance(data, dict):
        raise ConfigError(
            f"{target} : l'objet racine doit être un objet JSON, reçu {_type_name(data)}"
        )

    values: dict[str, Any] = {}
    for key, value in data.items():
        rule = ALLOWED_KEYS.get(key)
        if rule is None:
            raise ConfigError(
                f"{target} : clé inconnue : {key} - clés admises : "
                + ", ".join(ALLOWED_KEYS)
            )
        field_name, validator = rule
        try:
            values[field_name] = validator(key, value, known)
        except ConfigError as exc:
            raise ConfigError(f"{target} : {exc}") from None
    return Config(source=str(target), **values)


# --------------------------------------------------------------------------- #
# Résolution
# --------------------------------------------------------------------------- #


def resolve_trash_days(cli: int | None, config: Config = DEFAULT_CONFIG) -> int:
    """CLI > fichier > défaut, **sans exception**.

    Un fichier qui a relevé le plancher à 30 jours est quand même battu par
    `--trash-days 0` : sinon la commande de clôture de la décision 18 deviendrait
    un no-op silencieux sur toute machine portant un tel fichier. L'asymétrie est
    sûre — relever le plancher dans un fichier protège les runs non surveillés,
    tandis que `--trash-days 0` est un acte tapé, par invocation.
    """
    if cli is not None:
        return cli
    if config.trash_days is not None:
        return config.trash_days
    return DEFAULT_TRASH_DAYS


def resolve_max_delete_bytes(cli: int | None, config: Config = DEFAULT_CONFIG) -> int:
    """CLI > fichier > défaut. `cli is None` veut dire « drapeau non tapé »."""
    if cli is not None:
        return cli
    if config.max_delete_bytes is not None:
        return config.max_delete_bytes
    return DEFAULT_MAX_DELETE_BYTES


def protected_union(config: Config = DEFAULT_CONFIG) -> tuple[str, ...]:
    """`DEFAULT_PROTECTED` **plus** ceux du fichier. Jamais un remplacement."""
    return tuple(dict.fromkeys(tuple(guards.DEFAULT_PROTECTED) + config.protected_paths))


def disabled_union(
    skip: Sequence[str] = (), config: Config = DEFAULT_CONFIG
) -> list[str]:
    """`--skip` **plus** `DISABLED_MODULES`. Jamais un remplacement."""
    return list(dict.fromkeys([*skip, *config.disabled_modules]))
