"""Orchestrateur CLI : construit un plan, ne supprime que sous `--apply`.

Trois propriétés de ce fichier sont des décisions, pas des détails :

1. **La racine par défaut est dérivée du fichier, jamais de `cwd`.** Un lanceur
   pose le répertoire courant sur le dossier du script, un terminal sur
   n'importe quoi : `Path.cwd()` se tromperait donc de deux façons opposées,
   silence d'un côté (le plan porte sur les sources de winclean, l'utilisateur
   lit « ma machine est propre »), sur-portée de l'autre.
2. **Aucun nom de module n'est écrit ici.** Le CLI lit `discovery`, `proc_guard`
   et `needs_network` sur l'enregistrement du registre ; ajouter un module ne
   demande aucune modification de ce fichier.
3. **Le compte rendu d'exécution sort d'un `finally`.** Un `KeyboardInterrupt`
   après la première suppression doit rendre compte des octets déjà partis,
   sinon l'appelant enregistre un nettoyage qui n'a pas eu lieu.

Un `--out` reçoit exactement ce que stdout a reçu : JSON avec `--json`, texte
sinon. Il n'y a pas de troisième format, et il ne remplace jamais stdout.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import config as config_mod  # noqa: E402
from scripts.winclean import guards, history, registry_mod, remove  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DROP_SANITY,
    LEVEL_ORDER,
    PROC_GUARD_WARN_AND_SKIP,
    SKIP_CHANGED,
    SKIP_GONE,
    SKIP_NO_UNDO,
    SKIP_UNCONFIRMED,
    CleanCandidate,
    CleanModule,
    CleanResult,
    CleanWarning,
    Level,
    Plan,
    RunReport,
    SkippedEntry,
    estimate_path,
    format_plan,
    format_result_report,
    free_space_by_volume,
    human_size,
    sum_known,
    volume_of,
)

__all__ = [
    "EXIT_OK",
    "EXIT_VALIDATION",
    "EXIT_CEILING",
    "EXIT_SANITY",
    "EXIT_REMOVAL",
    "EXIT_INTERRUPTED",
    "EXIT_PLATFORM",
    "DEFAULT_MAX_DEPTH",
    "DEFAULT_MAX_DELETE_BYTES",
    "DEFAULT_TRASH_DAYS",
    "DISCOVERY_BUDGET_SECONDS",
    "PlatformError",
    "SanityAbort",
    "Output",
    "build_parser",
    "explicit_flags",
    "parse_size",
    "default_root",
    "resolve_roots",
    "is_sync_root",
    "recycling_enabled",
    "confirm_level",
    "bin_allowance_warnings",
    "build_plan",
    "apply_plan",
    "show_history",
    "ensure_windows",
    "main",
]

# --------------------------------------------------------------------------- #
# Codes de sortie
# --------------------------------------------------------------------------- #

#: Zéro couvre tout ce qui n'est pas dans la liste ci-dessous, **y compris**
#: chaque omission (`skipped-*`) et chaque candidat écarté comme `protected` :
#: une omission n'est pas une erreur, et un `PROTECTED_PATHS` utilisateur est une
#: cause légitime qu'un code non nul rendrait inutilisable.
EXIT_OK = 0
#: Nom de module inconnu, module au-dessus du niveau actif, argument refusé.
EXIT_VALIDATION = 2
#: Total du plan au-dessus de `--max-delete-bytes`.
EXIT_CEILING = 3
#: Un `discover()` a proposé une racine de lecteur, la racine de profil ou un
#: chemin trop court : le composant qui calcule les cibles est défectueux.
EXIT_SANITY = 4
#: Échec de suppression autre qu'un verrou de partage.
EXIT_REMOVAL = 5
#: Interruption après la première suppression.
EXIT_INTERRUPTED = 6
#: Hors Windows.
EXIT_PLATFORM = 7

# --------------------------------------------------------------------------- #
# Valeurs figées (décision 15)
# --------------------------------------------------------------------------- #

DEFAULT_MAX_DEPTH = 6
#: Alias, pas une seconde déclaration : le plafond et le plancher d'âge sont
#: résolus par `config.py` (CLI > fichier > défaut), donc leurs valeurs par défaut
#: n'ont qu'un site de vérité, celui que la liste blanche valide.
DEFAULT_MAX_DELETE_BYTES = config_mod.DEFAULT_MAX_DELETE_BYTES
DEFAULT_TRASH_DAYS = config_mod.DEFAULT_TRASH_DAYS
#: Au-delà, la découverte est signalée avec son détail par module. Ce n'est pas
#: une limite qui avorte : c'est le seuil où le run nomme le responsable.
DISCOVERY_BUDGET_SECONDS = 60.0

#: Décision 4 : part du volume au-delà de laquelle un candidat destiné à la
#: corbeille est signalé. **Approximation assumée** — le quota réel vit dans la
#: configuration du shell et n'est pas lisible depuis la bibliothèque standard,
#: donc le message dit « peut dépasser » et son silence ne prouve rien.
BIN_ALLOWANCE_RATIO = 0.10

#: Composants de chemin qui trahissent un arbre synchronisé. La règle, pas la
#: liste, est ce qui compte : l'avertissement est informatif et ne bloque rien,
#: donc un faux positif coûte une ligne et un faux négatif coûte l'avertissement.
#: La liste grandira ; le test porte sur la règle.
SYNC_COMPONENT_PREFIXES: tuple[str, ...] = ("onedrive",)
SYNC_COMPONENTS: frozenset[str] = frozenset(
    {"dropbox", "icloud drive", "iclouddrive", "mega", "megasync", "syncthing"}
)
#: Le coffre MEGA de cette machine : `Perso` **sous** `Documents`. Le composant
#: seul est trop commun pour déclencher quoi que ce soit.
SYNC_NESTED_COMPONENTS: tuple[tuple[str, str], ...] = (("documents", "perso"),)

_SIZE_UNITS: dict[str, int] = {
    "": 1,
    "b": 1,
    "k": 1024,
    "kib": 1024,
    "kb": 1024,
    "m": 1024**2,
    "mib": 1024**2,
    "mb": 1024**2,
    "g": 1024**3,
    "gib": 1024**3,
    "gb": 1024**3,
    "t": 1024**4,
    "tib": 1024**4,
    "tb": 1024**4,
}


class PlatformError(Exception):
    """Le programme ne tourne pas sur Windows."""


class SanityAbort(Exception):
    """Un candidat a échoué au test de bon sens : le run s'arrête non nul."""


# --------------------------------------------------------------------------- #
# Sortie
# --------------------------------------------------------------------------- #


class Output:
    """stdout, et le même contenu dans `--out` s'il est demandé.

    Le fichier est réécrit à chaque émission avec **tout** ce que stdout a reçu
    jusque-là : un run interrompu laisse donc au moins son plan dans le fichier.
    Réécriture, jamais ajout — l'historique est le travail de `history.jsonl`
    (Part 3).
    """

    def __init__(self, out_path: str | os.PathLike[str] | None = None) -> None:
        self.out_path = Path(out_path) if out_path else None
        self.chunks: list[str] = []

    def write(self, text: str) -> None:
        print(text)
        self.chunks.append(text)
        self.flush()

    def flush(self) -> None:
        if self.out_path is None:
            return
        parent = self.out_path.parent
        if str(parent) and not parent.exists():
            parent.mkdir(parents=True, exist_ok=True)
        self.out_path.write_text("\n\n".join(self.chunks) + "\n", encoding="utf-8")


def _dump(payload: Mapping[str, Any]) -> str:
    return json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=False)


# --------------------------------------------------------------------------- #
# Arguments
# --------------------------------------------------------------------------- #


def parse_size(text: str) -> int:
    """Taille en octets depuis `50GiB`, `500m`, `1024`. Lève sur le reste."""
    raw = str(text).strip().replace(" ", "").replace("_", "")
    if not raw:
        raise argparse.ArgumentTypeError("taille vide")
    digits = raw
    suffix = ""
    while digits and not (digits[-1].isdigit() or digits[-1] == "."):
        suffix = digits[-1] + suffix
        digits = digits[:-1]
    factor = _SIZE_UNITS.get(suffix.lower())
    if factor is None or not digits:
        raise argparse.ArgumentTypeError(
            f"taille illisible : {text!r} - attendu par exemple 50GiB, 500MiB ou un "
            "nombre d'octets"
        )
    try:
        value = float(digits)
    except ValueError as exc:  # pragma: no cover - filtré par la boucle ci-dessus
        raise argparse.ArgumentTypeError(f"taille illisible : {text!r}") from exc
    if value < 0:
        raise argparse.ArgumentTypeError("une taille négative n'a pas de sens")
    return int(value * factor)


def _positive_top(text: str) -> int:
    try:
        value = int(text)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"--top attend un entier, reçu {text!r}") from exc
    if value < 0:
        raise argparse.ArgumentTypeError("--top attend un entier positif")
    return value


class _TrackedStore(argparse.Action):
    """`store` qui note, dans `args.explicit_flags`, que le drapeau a été tapé.

    Nécessaire à la résolution *CLI > fichier > défaut* d'un drapeau qui porte
    déjà un défaut : `args.max_delete_bytes` seul ne dit pas si les 50 Gio
    viennent de l'utilisateur ou du parseur. Comparer la valeur à la constante ne
    suffirait pas — taper exactement `50GiB` alors qu'un fichier abaisse le
    plafond doit gagner, et une comparaison conclurait « non tapé ».
    """

    def __call__(self, parser, namespace, values, option_string=None):  # type: ignore[override]
        setattr(namespace, self.dest, values)
        seen = getattr(namespace, "explicit_flags", None)
        if not isinstance(seen, set):
            seen = set()
            setattr(namespace, "explicit_flags", seen)
        seen.add(self.dest)


def explicit_flags(args: argparse.Namespace) -> frozenset[str]:
    """Les `dest` que la ligne de commande a réellement fixés."""
    seen = getattr(args, "explicit_flags", None)
    return frozenset(seen) if isinstance(seen, set) else frozenset()


def _positive_days(text: str) -> int:
    try:
        value = int(text)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"--trash-days attend un entier, reçu {text!r}") from exc
    if value < 0:
        raise argparse.ArgumentTypeError("--trash-days attend un entier positif ou nul")
    return value


def _positive_depth(text: str) -> int:
    try:
        value = int(text)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"--max-depth attend un entier, reçu {text!r}") from exc
    if value < 0:
        raise argparse.ArgumentTypeError("--max-depth attend un entier positif")
    return value


def _positive_history(text: str) -> int:
    try:
        value = int(text)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"--history attend un entier, reçu {text!r}") from exc
    if value < 1:
        raise argparse.ArgumentTypeError("--history attend un entier supérieur ou égal à 1")
    return value


def build_parser() -> argparse.ArgumentParser:
    """Le CLI. Toutes les chaînes en français, les jetons machine en anglais.

    Il n'y a **pas** de `--check` : une machine de développement porte toujours
    au moins un `__pycache__` ou un `target\\`, donc un `--check` sortirait `1` à
    chaque run et ne porterait aucun signal. L'absence est volontaire.
    """
    parser = argparse.ArgumentParser(
        prog="clean.py",
        description=(
            "Nettoyeur d'artefacts régénérables. Par défaut il n'imprime qu'un plan : "
            "rien n'est supprimé sans --apply, à aucun niveau."
        ),
        epilog=(
            "Niveaux : safe = artefacts de build et caches de gestionnaires de paquets, "
            "tous reconstructibles. moderate ajoute les caches d'applications "
            "(navigateurs, éditeurs), %TEMP%, les vidages mémoire et le nettoyage léger "
            "de Docker ; il passe par la corbeille par défaut et exige une confirmation "
            "sous --apply. aggressive ajoute la corbeille de l'utilisateur (plancher "
            "d'âge de 7 jours, voir --trash-days) et le cache d'installation MSI ; les "
            "deux suppriment en direct et le second demande sa propre confirmation. "
            "Voir README.md."
        ),
    )
    # `--apply` et `--history` sont exclusifs par construction, pas par
    # convention : le premier détruit, le second se contente de relire un
    # journal. Les accepter ensemble obligerait à trancher lequel gagne, et
    # n'importe quel arbitrage ferait d'une faute de frappe une suppression.
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--apply",
        action="store_true",
        help="Supprimer réellement. Sans lui, le plan est une simulation.",
    )
    parser.add_argument(
        "--level",
        choices=[level.value for level in LEVEL_ORDER],
        default=Level.SAFE.value,
        help="Niveau d'agressivité (défaut : safe).",
    )
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="MODULE",
        help="Limiter à ces modules (répétable, ou séparés par des virgules).",
    )
    parser.add_argument(
        "--skip",
        action="append",
        default=[],
        metavar="MODULE",
        help="Retirer ces modules de la sélection, après --only.",
    )
    parser.add_argument(
        "--root",
        action="append",
        default=[],
        metavar="CHEMIN",
        help=(
            "Racine de recherche (répétable). Ne borne que les modules qui parcourent "
            "l'arborescence : les caches par utilisateur sont trouvés quelles que "
            "soient les racines."
        ),
    )
    parser.add_argument(
        "--max-depth",
        type=_positive_depth,
        default=DEFAULT_MAX_DEPTH,
        metavar="N",
        help=f"Profondeur maximale de parcours sous chaque racine (défaut : {DEFAULT_MAX_DEPTH}).",
    )
    parser.add_argument(
        "--max-delete-bytes",
        action=_TrackedStore,
        type=parse_size,
        default=DEFAULT_MAX_DELETE_BYTES,
        metavar="TAILLE",
        help=(
            "Plafond du total du plan (défaut : "
            f"{human_size(DEFAULT_MAX_DELETE_BYTES)}). Au-dessus, le run s'arrête. "
            "Un fichier de configuration peut l'abaisser ; ce drapeau gagne sur lui."
        ),
    )
    parser.add_argument(
        "--config",
        default=None,
        metavar="FICHIER",
        help=(
            # `%%` et non `%` : argparse passe chaque `help` par un `%`-formatage
            # (`_expand_help`), donc un `%A` littéral y lève `ValueError` et fait
            # planter `--help` en entier — la première commande qu'on tape.
            "Fichier de configuration JSON (défaut : "
            "%%APPDATA%%\\winclean\\winclean.json s'il existe). Il ne peut que "
            "restreindre : désactiver des modules, ajouter des chemins protégés, "
            "abaisser le plafond, relever le plancher d'âge de la corbeille."
        ),
    )
    parser.add_argument(
        "--trash-days",
        type=_positive_days,
        default=None,
        metavar="N",
        help=(
            "Plancher d'âge des éléments de la corbeille au niveau aggressive "
            f"(défaut : {DEFAULT_TRASH_DAYS} jours). 0 vide tout ce qui est éligible."
        ),
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="Exclure ce dont la reconstitution exige le réseau. Les exclus restent listés.",
    )
    parser.add_argument(
        "--recycle",
        dest="recycle",
        action="store_true",
        help=(
            "Passer par la corbeille. C'est déjà le défaut au niveau moderate ; "
            "accepté et sans effet aux niveaux safe et aggressive, où la suppression "
            "est directe."
        ),
    )
    parser.add_argument(
        "--no-recycle",
        dest="recycle",
        action="store_false",
        help="Suppression directe, même au niveau moderate où la corbeille est le défaut.",
    )
    # Tri-état volontaire : `None` = « non exprimé », donc la corbeille par défaut
    # au niveau moderate. Un défaut `False` rendrait `--no-recycle` indiscernable
    # de son absence, et la corbeille cesserait d'être le défaut de ce niveau.
    parser.set_defaults(recycle=None)
    parser.add_argument(
        "--yes",
        action="store_true",
        help=(
            "Passer outre l'avertissement d'un processus propriétaire actif, et "
            "répondre d'avance à la confirmation exigée par --apply à partir du "
            "niveau moderate. N'implique jamais --apply et ne relève jamais le niveau."
        ),
    )
    parser.add_argument(
        "--yes-package-cache",
        dest="yes_package_cache",
        action="store_true",
        help=(
            "Répondre d'avance à la confirmation propre au module package-cache. "
            "Drapeau distinct de --yes : la suppression du cache d'installation "
            "casse les réparations et désinstallations futures, et cette "
            "conséquence survit au run."
        ),
    )
    parser.add_argument(
        "--out",
        default=None,
        metavar="FICHIER",
        help="Écrire le compte rendu dans ce fichier, au format que stdout reçoit.",
    )
    parser.add_argument(
        "--json",
        dest="as_json",
        action="store_true",
        help="Sortie JSON à la place du texte.",
    )
    parser.add_argument(
        "--top",
        type=_positive_top,
        default=None,
        metavar="N",
        help=(
            "N'imprimer que les N plus gros éléments. Filtre d'affichage : le total "
            "et, avec --apply, la suppression portent sur le plan complet."
        ),
    )
    mode.add_argument(
        "--history",
        type=_positive_history,
        default=None,
        metavar="N",
        help=(
            "Afficher les N derniers runs destructeurs enregistrés, puis sortir. "
            "Ne découvre rien et ne touche à aucun chemin. Seuls les runs --apply "
            "qui ont tenté une suppression y figurent : une simulation n'écrit rien."
        ),
    )
    return parser


def _split_names(values: Iterable[str]) -> list[str]:
    """Aplatit `--only a --only b,c` en `[a, b, c]`, sans doublon, ordre gardé."""
    names: list[str] = []
    for value in values:
        for part in str(value).split(","):
            name = part.strip()
            if name and name not in names:
                names.append(name)
    return names


# --------------------------------------------------------------------------- #
# Racines
# --------------------------------------------------------------------------- #


def default_root() -> Path:
    """La racine du dépôt, dérivée du fichier comme l'amorçage d'import.

    **Jamais `Path.cwd()`** : sous le lanceur le répertoire courant est le
    dossier du script, depuis un terminal c'est un dossier arbitraire. Les deux
    coïncident avec cette valeur seulement quand l'outil est lancé depuis la
    racine du dépôt.
    """
    return _REPO_ROOT


def resolve_roots(roots: Sequence[str] | None) -> list[Path]:
    """Racines résolues, dédoublonnées, ou la racine par défaut si aucune."""
    if not roots:
        return [default_root()]
    resolved: list[Path] = []
    seen: set[str] = set()
    for raw in roots:
        path = Path(raw).expanduser()
        try:
            path = path.resolve()
        except OSError:  # pragma: no cover - chemin illisible
            path = path.absolute()
        key = os.path.normcase(str(path))
        if key not in seen:
            seen.add(key)
            resolved.append(path)
    return resolved


def is_sync_root(path: str | os.PathLike[str], env: Mapping[str, str] | None = None) -> bool:
    """Vrai si la racine tombe dans un arbre synchronisé dans le cloud.

    Règle et non liste : un composant du chemin résolu correspond à un dossier
    de fournisseur connu, ou la racine se trouve sous un chemin nommé par
    `%OneDrive%` / `%OneDriveCommercial%`. Les paires imbriquées
    (`Perso` sous `Documents`) évitent qu'un composant trop commun déclenche
    seul.
    """
    environment = os.environ if env is None else env
    normalised = guards.normalise(path)
    parts = [part.strip("\\/").lower() for part in Path(normalised).parts]
    for part in parts:
        if part in SYNC_COMPONENTS:
            return True
        if any(part.startswith(prefix) for prefix in SYNC_COMPONENT_PREFIXES):
            return True
    for outer, inner in SYNC_NESTED_COMPONENTS:
        if outer in parts and inner in parts and parts.index(outer) < parts.index(inner):
            return True
    for key in ("OneDrive", "OneDriveCommercial", "OneDriveConsumer"):
        value = environment.get(key)
        if not value:
            continue
        base = guards.normalise(value)
        if normalised == base or normalised.startswith(base.rstrip("\\/") + os.sep):
            return True
    return False


# --------------------------------------------------------------------------- #
# Corbeille et confirmation
# --------------------------------------------------------------------------- #


def recycling_enabled(level: Level, recycle: bool | None) -> bool:
    """Vrai si ce run passe par la corbeille. `moderate` est le seul niveau concerné.

    `safe` et `aggressive` suppriment en direct (décision 4) : à `moderate` la
    corbeille est le **défaut**, et seul un `--no-recycle` explicite — donc
    `recycle is False`, pas simplement absent — la désarme.
    """
    if Level(level) is not Level.MODERATE:
        return False
    return recycle is not False


def confirm_level(level: Level, *, yes: bool, stream: Any = None) -> bool:
    """Confirmation exigée par `--apply` à partir de `moderate` (décision 12).

    `--yes` répond d'avance. Sinon la question est posée sur un terminal, et
    **sans terminal, sans `--yes`, la réponse est non** : un run planifié ou
    branché sur un tube ne peut pas répondre, et le considérer comme consentant
    ferait du silence un accord au niveau qui touche les données applicatives.
    """
    if Level(level) is Level.SAFE:
        return True
    if yes:
        return True
    source = sys.stdin if stream is None else stream
    isatty = getattr(source, "isatty", None)
    if isatty is None or not isatty():
        return False
    print(
        f"Niveau {Level(level).value} avec --apply : confirmer la suppression ? "
        "[oui/non] ",
        end="",
    )
    try:
        answer = source.readline()
    except (OSError, ValueError):  # pragma: no cover - stdin refermé
        return False
    return answer.strip().lower() in ("o", "oui", "y", "yes")


def confirm_module(
    extra: registry_mod.ExtraConfirm,
    *,
    answered: bool,
    stream: Any = None,
) -> bool:
    """Seconde confirmation d'un module, au-delà de celle du niveau.

    `answered` porte **son** drapeau, jamais `--yes` : la question qu'elle pose
    n'est pas « supprimer ? » mais « accepter une conséquence qui survit au
    run ? », et deux questions différentes ne peuvent pas partager une réponse.

    Comme `confirm_level`, sans terminal et sans le drapeau la réponse est
    **non** : le module est alors omis, le reste du niveau s'exécute, et le run
    sort `0` — l'utilisateur n'a rien demandé de contradictoire, il a lancé un
    niveau dont un module réclame plus qu'il n'a donné.
    """
    if answered:
        return True
    source = sys.stdin if stream is None else stream
    isatty = getattr(source, "isatty", None)
    if isatty is None or not isatty():
        return False
    print(f"{extra.question} [oui/non] ", end="")
    try:
        answer = source.readline()
    except (OSError, ValueError):  # pragma: no cover - stdin refermé
        return False
    return answer.strip().lower() in ("o", "oui", "y", "yes")


def bin_allowance_warnings(
    candidates: Iterable[CleanCandidate],
    level: Level,
    recycle: bool | None,
) -> list[CleanWarning]:
    """Avertit pour chaque candidat destiné à la corbeille et lourd sur son volume.

    Informatif : ne bloque rien, ne change ni le mode de suppression ni le code de
    sortie. Fire sur le chemin `moderate` **par défaut**, pas seulement sous un
    `--recycle` explicite — le défaut est justement là où l'utilisateur suppose
    que l'annulation existe.

    Deux exclusions, chacune pour une raison différente. Un candidat **sans
    chemin** n'a pas de volume : `splitdrive(None)` planterait, et cette étape
    tourne *avant* la répartition de `apply_plan`, donc rien ne le rattraperait.
    Un candidat **sans taille** n'a pas de rapport à calculer : il n'est pas
    comparé, il est signalé sous son propre code — traiter l'inconnu comme `0`
    produirait un « pas d'avertissement » rassurant sur le seul candidat que
    personne n'a su mesurer.
    """
    if not recycling_enabled(level, recycle):
        return []
    warnings: list[CleanWarning] = []
    for candidate in candidates:
        if candidate.path is None:
            continue
        volume = volume_of(candidate.path)
        if candidate.estimated_bytes is None:
            warnings.append(
                CleanWarning(
                    code="recycle-bin-allowance-unknown",
                    fields={
                        "label": candidate.label,
                        "path": candidate.path,
                        "volume": volume,
                    },
                )
            )
            continue
        try:
            total = shutil.disk_usage(volume).total
        except OSError:  # pragma: no cover - volume illisible
            continue
        if total <= 0:  # pragma: no cover - volume dégénéré
            continue
        ratio = candidate.estimated_bytes / total
        if ratio > BIN_ALLOWANCE_RATIO:
            warnings.append(
                CleanWarning(
                    code="recycle-bin-allowance",
                    fields={
                        "label": candidate.label,
                        "path": candidate.path,
                        "volume": volume,
                        "ratio": ratio,
                    },
                )
            )
    return warnings


# --------------------------------------------------------------------------- #
# Construction du plan
# --------------------------------------------------------------------------- #


def build_plan(
    args: argparse.Namespace,
    registry: Mapping[str, CleanModule] | None = None,
    config: config_mod.Config | None = None,
) -> Plan:
    """Valider → découvrir → estampiller → garder → absorber → exclure → plafonner.

    L'ordre n'est pas négociable : la validation précède la découverte (une faute
    de frappe coûte une erreur, pas un parcours de disque suivi de « rien à
    nettoyer »), et le plafond passe en dernier, sur ce qui reste réellement.

    La configuration n'entre que par trois portes, toutes restrictives : la
    sélection (`DISABLED_MODULES`, uni à `--skip`), la protection
    (`PROTECTED_PATHS`, uni à `DEFAULT_PROTECTED`) et le plafond
    (`MAX_DELETE_BYTES`, que la ligne de commande peut relever). Aucune ne touche
    aux racines : les choisir reste un acte par invocation.
    """
    settings = config_mod.DEFAULT_CONFIG if config is None else config
    level = Level(args.level)
    only = _split_names(args.only)
    skip = _split_names(args.skip)
    modules = registry_mod.select_modules(
        level,
        only,
        skip,
        registry,
        disabled=settings.disabled_modules,
        config_source=settings.source,
    )

    roots = resolve_roots(args.root)
    warnings: list[CleanWarning] = []
    for root in roots:
        if is_sync_root(root):
            warnings.append(CleanWarning(code="sync-root", fields={"root": str(root)}))
    if args.recycle and level is not Level.MODERATE:
        # `moderate` est le seul niveau qui met en corbeille : ailleurs le drapeau
        # est accepté et inerte, et le dire vaut mieux qu'un run qui supprime en
        # direct pendant que l'appelant croit avoir armé une annulation.
        warnings.append(CleanWarning(code="recycle-inert", fields={"level": level.value}))

    candidates: list[CleanCandidate] = []
    durations: dict[str, float] = {}
    # `notes` est le canal d'un `discover_*()` vers le texte du plan : il ne rend
    # que des candidats, et ce que `recycle-bin` a à dire porte justement sur ce
    # qu'il n'a **pas** retenu — un SID irrésolu, des entrées trop récentes, des
    # entrées indatables. Aucun candidat ne pourrait porter ces messages.
    for module in modules:
        started = time.monotonic()
        found = registry_mod.discover_module(
            module,
            roots=roots,
            max_depth=args.max_depth,
            trash_days=config_mod.resolve_trash_days(args.trash_days, settings),
            notes=warnings,
        )
        durations[module.name] = time.monotonic() - started
        candidates.extend(found)

    elapsed = sum(durations.values())
    if elapsed > DISCOVERY_BUDGET_SECONDS:
        detail = ", ".join(
            f"{name} {seconds:.1f} s"
            for name, seconds in sorted(durations.items(), key=lambda kv: -kv[1])
        )
        warnings.append(
            CleanWarning(
                code="discovery-slow",
                fields={
                    "seconds": f"{elapsed:.1f}",
                    "per_module": detail,
                    "per_module_seconds": {k: round(v, 3) for k, v in durations.items()},
                },
            )
        )

    kept, dropped = guards.screen_candidates(
        candidates, protected=config_mod.protected_union(settings)
    )
    kept, absorbed = guards.absorb_nested(kept)
    excluded = []
    if args.offline:
        kept, excluded = guards.filter_needs_network(kept)

    cli_ceiling = (
        args.max_delete_bytes if "max_delete_bytes" in explicit_flags(args) else None
    )
    ceiling = config_mod.resolve_max_delete_bytes(cli_ceiling, settings)
    # Peut lever `CeilingExceeded` : le run s'arrête avant la première suppression.
    _total, ceiling_warnings = guards.enforce_ceiling(kept, ceiling)
    warnings.extend(ceiling_warnings)

    # Décision 4, au plan : ce qui partira en corbeille et pèse lourd sur son
    # volume est signalé avant toute suppression, y compris sans `--apply`.
    warnings.extend(bin_allowance_warnings(kept, level, args.recycle))

    touched: list[str] = [str(root) for root in roots]
    touched.extend(c.path for c in kept if c.path)
    return Plan(
        candidates=kept,
        dropped=dropped,
        absorbed=absorbed,
        excluded=excluded,
        warnings=warnings,
        free_space=free_space_by_volume(touched),
        roots=[str(root) for root in roots],
        level=level,
        apply=bool(args.apply),
        top=args.top,
        discovery_by_module={m.name: m.discovery for m in modules},
    )


def sanity_drops(plan: Plan) -> list[Any]:
    """Les candidats refusés pour bon sens. Non vide = run défectueux, sortie non nulle."""
    return [d for d in plan.dropped if d.reason_class == DROP_SANITY]


# --------------------------------------------------------------------------- #
# Application
# --------------------------------------------------------------------------- #


def _current_mtime(path: str) -> float | None:
    """mtime actuel, ou `None` si le chemin a disparu."""
    try:
        return os.stat(path).st_mtime
    except OSError:
        return None


def _add(current: int | None, delta: int) -> int:
    return delta if current is None else current + delta


def _merge_result(target: CleanResult, other: CleanResult | None) -> None:
    """Fond le `CleanResult` rendu par le `clean()` d'un module dans le nôtre."""
    if other is None:
        return
    for column in ("freed", "recycled", "failed"):
        value = getattr(other, column)
        if value is not None:
            setattr(target, column, _add(getattr(target, column), value))
    target.skipped.extend(other.skipped)
    target.locked_paths.extend(other.locked_paths)
    target.recycle_failed_paths.extend(other.recycle_failed_paths)
    target.recycle_events += other.recycle_events


def _record(result: CleanResult, column: str, value: int | None) -> None:
    """Verse une contribution *mesurée* dans une colonne d'octets.

    `value is None` veut dire « la mesure a échoué », pas « zéro » : la colonne
    passe alors à `None` tant qu'aucune contribution connue n'y figure, ce qui est
    la doctrine de `sum_known` appliquée à une colonne. Coercer l'inconnu en `0`
    afficherait « rien mis en corbeille » sur un run qui vient d'en remplir une.
    """
    current = getattr(result, column)
    if value is None:
        if not current:
            setattr(result, column, None)
        return
    setattr(result, column, _add(current, value))


def _measure(result: CleanResult, before: int | None, path: str) -> None:
    """Verse dans `measured` la contribution d'un candidat, après opération.

    Somme des contributions **connues** : une contribution non mesurable n'est pas
    versée, donc `measured` reste `None` jusqu'à ce qu'une seule le soit — et vaut
    `None` en propre pour un module dont aucun candidat n'a été tenté. Une
    contribution de `0` octet est connue et se verse : ce n'est pas la même chose
    qu'un module non mesurable, et le rapport les distingue.

    `_record()` ne convient pas ici : sa branche `None` remet la colonne à `None`
    dès qu'elle vaut `0`, ce qui est juste pour « rien mis en corbeille » et faux
    pour une somme de mesures dont l'une a échoué après une autre qui valait zéro.
    """
    value = remove.measure_freed(before, path)
    if value is None:
        return
    result.measured = _add(result.measured, value)


def _remove_candidate(
    candidate: CleanCandidate,
    result: CleanResult,
    *,
    use_recycle: bool,
    failures: list[remove.RemovalError],
    report: RunReport | None = None,
) -> SkippedEntry | None:
    """Retire un candidat porteur de chemin. La corbeille n'a **aucun** repli.

    Rend l'omission à enregistrer, ou `None` si le candidat a été traité. Les
    octets viennent d'une mesure prise **juste avant** l'opération, jamais de
    `estimated_bytes` : entre le plan et l'application l'arbre a pu grossir, et
    rapporter l'estimation ferait passer une mesure pour une prédiction.

    `report` sert à marquer la tentative de suppression, et le marquage arrive
    **après** le refus de corbeille : un candidat que la corbeille ne peut pas
    recevoir est omis, rien n'a été tenté sur lui, et l'inscrire au journal y
    poserait un run qui n'a touché à rien.
    """
    assert candidate.path is not None  # garanti par l'appelant
    if use_recycle:
        if not remove.can_recycle(candidate.path):
            return SkippedEntry(
                label=candidate.label,
                path=candidate.path,
                status=SKIP_NO_UNDO,
                reason=(
                    "la corbeille ne peut pas recevoir ce chemin (volume non fixe, "
                    "chemin trop long) ; --no-recycle pour supprimer en direct"
                ),
            )
        if report is not None:
            report.removal_attempted = True
        before = estimate_path(candidate.path)
        outcome = remove.recycle(candidate.path)
        if outcome.ok:
            result.recycle_events += 1
            _record(result, "recycled", before)
            _measure(result, before, candidate.path)
            return None
        # Décision 14 : un échec de corbeille est terminal, pas un repli. Le
        # chemin est intact, et il est nommé dans sa propre section — le confondre
        # avec un verrou enverrait l'utilisateur fermer une application innocente.
        _record(result, "failed", before)
        _measure(result, before, candidate.path)
        result.recycle_failed_paths.append(candidate.path)
        failures.extend(outcome.errors)
        return None
    if report is not None:
        report.removal_attempted = True
    before = estimate_path(candidate.path)
    freed, failed, errors = remove.delete_tree(candidate.path)
    _measure(result, before, candidate.path)
    result.freed = _add(result.freed, freed)
    if failed:
        result.failed = _add(result.failed, failed)
    for error in errors:
        if remove.is_lock_error(error):
            result.locked_paths.append(error.path)
    failures.extend(error for error in errors if not remove.is_partial_error(error))
    return None


def apply_plan(
    plan: Plan,
    args: argparse.Namespace,
    report: RunReport,
    failures: list[remove.RemovalError],
    registry: Mapping[str, CleanModule] | None = None,
) -> None:
    """Supprime le plan **complet**, module par module, en remplissant `report`.

    `report` est passé et non rendu : il doit être lisible depuis le `finally` de
    l'appelant même si cette fonction ne revient jamais. `--top` n'a aucun effet
    ici — c'est un filtre d'affichage, jamais de portée.

    Un module qui porte un `clean()` reçoit **tous** ses candidats en un appel, y
    compris ceux qui portent un chemin : `remove.py` ne sait retirer qu'un chemin
    à la fois, et l'unité de suppression de `recycle-bin` est une paire. Les
    gardes (processus, chemin disparu, horodatage changé) s'appliquent avant, à
    l'identique.
    """
    known = registry_mod.MODULES if registry is None else registry
    grouped: dict[str, list[CleanCandidate]] = {}
    for candidate in plan.candidates:
        grouped.setdefault(candidate.module, []).append(candidate)

    use_recycle = recycling_enabled(Level(plan.level), args.recycle)

    for name, group in grouped.items():
        module = known.get(name)
        # Les colonnes que cette boucle remplit elle-même partent de zéro, pas de
        # `None` : pour un module dont nous retirons les chemins, « rien mis en
        # corbeille » est une valeur mesurée, et l'afficher `unknown` ferait lire
        # un défaut de mesure là où le niveau supprime simplement en direct. Un
        # groupe entièrement sans chemin garde `None` : ce que son `clean()` ne
        # sait pas, nous ne le savons pas non plus.
        counted = 0 if any(c.path for c in group) else None
        result = CleanResult(
            module=name,
            estimated=sum_known(c.estimated_bytes for c in group),
            freed=counted,
            recycled=counted,
            failed=counted,
        )
        report.results.append(result)
        if module is None:  # pragma: no cover - le plan sort du registre
            continue

        # Confirmation propre au module, avant son premier candidat : la poser
        # après en aurait déjà supprimé. Refusée, elle omet **ce module** et
        # laisse le reste du niveau agir — c'est une réserve sur une conséquence
        # particulière, pas un désaveu du run.
        extra = registry_mod.extra_confirm(name)
        if extra is not None and not confirm_module(
            extra, answered=bool(getattr(args, "yes_package_cache", False))
        ):
            for candidate in group:
                result.skipped.append(
                    SkippedEntry(
                        label=candidate.label,
                        path=candidate.path,
                        status=SKIP_UNCONFIRMED,
                        reason=(
                            "confirmation propre au module absente - répondre « oui » "
                            f"sur un terminal, ou passer {extra.flag}"
                        ),
                    )
                )
            continue

        state = None
        if module.proc_guard == PROC_GUARD_WARN_AND_SKIP and not args.yes:
            # Une seule interrogation des processus par module, pas par candidat.
            state = registry_mod.proc_guard_state(name)

        # Un module qui porte son propre `clean()` retire **tous** ses candidats,
        # avec ou sans chemin : `recycle-bin` supprime par paires `$I`/`$R` et un
        # candidat n'a qu'un `path`, donc le chemin passe par lui, pas par
        # `remove.py`. Les gardes ci-dessous s'appliquent quand même d'abord — la
        # délégation porte sur la suppression, pas sur les conditions de l'agir.
        delegated: list[CleanCandidate] = []
        for candidate in group:
            skipped = registry_mod.proc_guard_skip(
                module, candidate, yes=bool(args.yes), state=state
            )
            if skipped is not None:
                result.skipped.append(skipped)
                continue
            if candidate.path is None:
                delegated.append(candidate)
                continue
            current = _current_mtime(candidate.path)
            if current is None:
                result.skipped.append(
                    SkippedEntry(
                        label=candidate.label,
                        path=candidate.path,
                        status=SKIP_GONE,
                        reason="chemin absent au moment de l'application",
                    )
                )
                continue
            if candidate.stat_mtime is not None and current != candidate.stat_mtime:
                # Garde secondaire : il ne s'applique pas à un candidat dont le
                # plan n'a rien relevé (`stat_mtime is None`), et le traiter
                # comme modifié n'omettrait pas un risque, il omettrait tout.
                result.skipped.append(
                    SkippedEntry(
                        label=candidate.label,
                        path=candidate.path,
                        status=SKIP_CHANGED,
                        reason="l'horodatage a bougé depuis l'établissement du plan",
                    )
                )
                continue
            if module.clean is not None:
                delegated.append(candidate)
                continue
            no_undo = _remove_candidate(
                candidate, result, use_recycle=use_recycle, failures=failures, report=report
            )
            if no_undo is not None:
                result.skipped.append(no_undo)

        if delegated and module.clean is not None:
            # Appeler le `clean()` d'un module est une tentative de suppression :
            # `docker-light` n'a aucune branche d'omission, son `prune` part dès
            # qu'il est appelé. Marquer d'après ses octets serait faux, il n'en
            # rend aucun de mesurable.
            report.removal_attempted = True
            # Le second avis vaut aussi pour un module qui supprime lui-même : la
            # mesure d'avant est prise ici, avant l'appel, et relue après. Sans
            # cela `recycle-bin` — dont tous les candidats sont délégués — serait
            # le seul module dont la colonne `measured` reste vide alors qu'il a
            # bien retiré des octets.
            before = {c.path: estimate_path(c.path) for c in delegated if c.path}
            _merge_result(
                result,
                module.clean(
                    candidates=list(delegated),
                    recycle=use_recycle,
                    yes=bool(args.yes),
                ),
            )
            for path, value in before.items():
                _measure(result, value, path)


# --------------------------------------------------------------------------- #
# Entrée
# --------------------------------------------------------------------------- #


def ensure_windows(platform: str | None = None) -> None:
    """Refuse tout système hors Windows et Linux, les deux seuls que `remove.py` sait traiter.

    Nommée `ensure_windows` pour rester le point d'appel historique de `main()` -
    winclean est né Windows-only - mais la garde couvre désormais aussi Linux
    depuis que `remove.py` route sa suppression vers `trash_linux.py` sur ce
    système. macOS et le reste restent refusés : aucun des deux paquets de
    primitives ne les couvre.
    """
    current = sys.platform if platform is None else platform
    text = str(current)
    if text.startswith("win") or text.startswith("linux"):
        return
    raise PlatformError(
        "winclean ne fonctionne que sous Windows ou Linux : la suppression passe par "
        "les chemins longs \\\\?\\ et SHFileOperationW (Windows), ou par la corbeille "
        f"utilisateur freedesktop.org (Linux). Système détecté : {current}."
    )


def show_history(limit: int, out: Output, *, as_json: bool = False) -> int:
    """Imprime les `limit` derniers runs destructeurs. Mode requête, pas de run.

    Aucune découverte, aucun `stat` de cible, aucune configuration chargée : les
    trois coûtent un parcours de disque ou peuvent refuser le run, et aucun des
    trois ne change ce que le journal contient. Sort toujours `0`, y compris sans
    journal du tout : « aucun run enregistré » est une réponse, pas une erreur.
    """
    location = history.default_history_path()
    records = history.read_runs(limit)
    if as_json:
        out.write(
            _dump(
                {
                    "history": list(records),
                    "path": None if location is None else str(location),
                }
            )
        )
    else:
        out.write(history.format_history(records, location))
    return EXIT_OK


def _write_history(plan: Plan, report: RunReport) -> None:
    """Ajoute une ligne au journal. Un échec est un avertissement, jamais un statut.

    Appelée depuis le `finally` du run : si elle levait, elle masquerait le
    chemin de sortie réel — jusqu'à transformer une interruption en `traceback`
    de journalisation. Le run a déjà supprimé ce qu'il a supprimé ; ne pas savoir
    l'écrire ne change rien à ce fait et ne doit pas changer son code de sortie.
    """
    try:
        history.append_run(history.build_record(plan.level, report))
    except Exception as exc:  # noqa: BLE001 - voir la docstring
        print(f"Historique non écrit : {exc}", file=sys.stderr)


def _volumes_touched(plan: Plan) -> list[str]:
    touched = list(plan.roots)
    touched.extend(c.path for c in plan.candidates if c.path)
    return touched


def main(argv: Sequence[str] | None = None) -> int:
    """Point d'entrée. Renvoie le code de sortie, ne lève pas pour ses causes."""
    parser = build_parser()
    args = parser.parse_args(argv)
    out = Output(args.out)

    try:
        ensure_windows()
    except PlatformError as exc:
        print(str(exc), file=sys.stderr)
        return EXIT_PLATFORM

    # Avant le chargement de la configuration : relire le journal ne dépend
    # d'aucun réglage, et une configuration illisible ferait sortir `2` une
    # commande qui ne demandait qu'à lire un fichier de log.
    if args.history is not None:
        return show_history(args.history, out, as_json=args.as_json)

    try:
        settings = config_mod.load_config(args.config)
    except config_mod.ConfigError as exc:
        # Avant toute découverte : une configuration refusée ne doit pas coûter un
        # parcours de disque, et surtout pas un run mené sur les défauts pendant
        # que l'utilisateur croit ses restrictions appliquées.
        print(str(exc), file=sys.stderr)
        return EXIT_VALIDATION

    try:
        plan = build_plan(args, config=settings)
    except registry_mod.ValidationError as exc:
        print(str(exc), file=sys.stderr)
        return EXIT_VALIDATION
    except guards.CeilingExceeded as exc:
        print(str(exc), file=sys.stderr)
        return EXIT_CEILING

    plan_payload = plan.to_json_payload()
    if not args.as_json:
        out.write(format_plan(plan))

    refused = sanity_drops(plan)
    if refused:
        if args.as_json:
            out.write(_dump(plan_payload))
        for drop in refused:
            print(
                f"Plan refusé : le module {drop.module} a proposé {drop.path!r} "
                f"({drop.detail}). Aucune suppression n'a eu lieu.",
                file=sys.stderr,
            )
        return EXIT_SANITY

    if not args.apply:
        if args.as_json:
            out.write(_dump(plan_payload))
        return EXIT_OK

    # Portail de confirmation (décision 12) : après le plan — il a été imprimé,
    # donc la question porte sur quelque chose de lisible — et avant la première
    # suppression. `--only <module moderate>` sans `--level moderate` n'arrive
    # jamais ici : `build_plan` a déjà refusé en nommant le niveau requis.
    if not confirm_level(Level(plan.level), yes=bool(args.yes)):
        if args.as_json:
            out.write(_dump(plan_payload))
        print(
            f"Confirmation absente pour le niveau {Level(plan.level).value} : rien n'a "
            "été supprimé. Répondre « oui » sur un terminal, ou passer --yes.",
            file=sys.stderr,
        )
        return EXIT_VALIDATION

    report = RunReport(estimated_total=plan.total_estimated())
    failures: list[remove.RemovalError] = []
    status = EXIT_OK
    try:
        apply_plan(plan, args, report, failures)
    except KeyboardInterrupt:
        report.interrupted = True
        status = EXIT_INTERRUPTED
        print("Interruption demandée : arrêt après le dernier élément traité.", file=sys.stderr)
    except Exception:
        report.interrupted = True
        if (report.total_freed() or 0) > 0:
            # Des octets sont déjà partis : rendre compte vaut mieux qu'un
            # traceback qui laisse l'appelant sans chiffre.
            status = EXIT_INTERRUPTED
            traceback.print_exc()
        else:
            raise
    finally:
        report.free_space_after = free_space_by_volume(_volumes_touched(plan))
        if args.as_json:
            out.write(_dump({**plan_payload, "run": report.to_json_payload()}))
        else:
            out.write(format_result_report(report))
        # Le drapeau, pas les octets : voir `history.py`. Un run interrompu qui a
        # tenté quelque chose est journalisé — c'est même le cas où le journal
        # sert le plus.
        if report.removal_attempted:
            _write_history(plan, report)

    if failures and status == EXIT_OK:
        for error in failures:
            print(f"Échec de suppression : {error.path} - {error.message}", file=sys.stderr)
        status = EXIT_REMOVAL
    return status


if __name__ == "__main__":
    sys.exit(main())
