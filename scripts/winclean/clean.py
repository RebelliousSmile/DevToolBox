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
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import guards, registry_mod, remove  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DROP_SANITY,
    LEVEL_ORDER,
    PROC_GUARD_WARN_AND_SKIP,
    SKIP_CHANGED,
    SKIP_GONE,
    CleanCandidate,
    CleanModule,
    CleanResult,
    CleanWarning,
    Level,
    Plan,
    RunReport,
    SkippedEntry,
    format_plan,
    format_result_report,
    free_space_by_volume,
    human_size,
    sum_known,
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
    "DISCOVERY_BUDGET_SECONDS",
    "PlatformError",
    "SanityAbort",
    "Output",
    "build_parser",
    "parse_size",
    "default_root",
    "resolve_roots",
    "is_sync_root",
    "build_plan",
    "apply_plan",
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
DEFAULT_MAX_DELETE_BYTES = 50 * 1024**3
#: Au-delà, la découverte est signalée avec son détail par module. Ce n'est pas
#: une limite qui avorte : c'est le seuil où le run nomme le responsable.
DISCOVERY_BUDGET_SECONDS = 60.0

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


def _positive_depth(text: str) -> int:
    try:
        value = int(text)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"--max-depth attend un entier, reçu {text!r}") from exc
    if value < 0:
        raise argparse.ArgumentTypeError("--max-depth attend un entier positif")
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
            "tous reconstructibles. Voir README.md."
        ),
    )
    parser.add_argument(
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
        type=parse_size,
        default=DEFAULT_MAX_DELETE_BYTES,
        metavar="TAILLE",
        help=(
            "Plafond du total du plan (défaut : "
            f"{human_size(DEFAULT_MAX_DELETE_BYTES)}). Au-dessus, le run s'arrête."
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
            "Passer par la corbeille quand le niveau le permet. Accepté et sans effet "
            "au niveau safe, où la suppression est directe."
        ),
    )
    parser.add_argument(
        "--no-recycle",
        dest="recycle",
        action="store_false",
        help="Suppression directe (défaut).",
    )
    parser.set_defaults(recycle=False)
    parser.add_argument(
        "--yes",
        action="store_true",
        help=(
            "Passer outre l'avertissement d'un processus propriétaire actif. "
            "N'implique jamais --apply et ne relève jamais le niveau."
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
# Construction du plan
# --------------------------------------------------------------------------- #


def build_plan(
    args: argparse.Namespace,
    registry: Mapping[str, CleanModule] | None = None,
) -> Plan:
    """Valider → découvrir → estampiller → garder → absorber → exclure → plafonner.

    L'ordre n'est pas négociable : la validation précède la découverte (une faute
    de frappe coûte une erreur, pas un parcours de disque suivi de « rien à
    nettoyer »), et le plafond passe en dernier, sur ce qui reste réellement.
    """
    level = Level(args.level)
    only = _split_names(args.only)
    skip = _split_names(args.skip)
    modules = registry_mod.select_modules(level, only, skip, registry)

    roots = resolve_roots(args.root)
    warnings: list[CleanWarning] = []
    for root in roots:
        if is_sync_root(root):
            warnings.append(CleanWarning(code="sync-root", fields={"root": str(root)}))
    if args.recycle and level is Level.SAFE:
        warnings.append(CleanWarning(code="recycle-inert", fields={"level": level.value}))

    candidates: list[CleanCandidate] = []
    durations: dict[str, float] = {}
    for module in modules:
        started = time.monotonic()
        found = registry_mod.discover_module(
            module,
            roots=roots,
            max_depth=args.max_depth,
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

    kept, dropped = guards.screen_candidates(candidates)
    kept, absorbed = guards.absorb_nested(kept)
    excluded = []
    if args.offline:
        kept, excluded = guards.filter_needs_network(kept)

    # Peut lever `CeilingExceeded` : le run s'arrête avant la première suppression.
    _total, ceiling_warnings = guards.enforce_ceiling(kept, args.max_delete_bytes)
    warnings.extend(ceiling_warnings)

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


def _remove_candidate(
    candidate: CleanCandidate,
    result: CleanResult,
    *,
    use_recycle: bool,
    failures: list[remove.RemovalError],
) -> None:
    """Retire un candidat porteur de chemin. La corbeille n'a **aucun** repli."""
    assert candidate.path is not None  # garanti par l'appelant
    if use_recycle and remove.can_recycle(candidate.path):
        outcome = remove.recycle(candidate.path)
        if outcome.ok:
            result.recycled = _add(result.recycled, candidate.estimated_bytes or 0)
            return
        # Décision 14 : un échec de corbeille est terminal, pas un repli.
        result.failed = _add(result.failed, candidate.estimated_bytes or 0)
        failures.extend(outcome.errors)
        return
    freed, failed, errors = remove.delete_tree(candidate.path)
    result.freed = _add(result.freed, freed)
    if failed:
        result.failed = _add(result.failed, failed)
    failures.extend(error for error in errors if not remove.is_partial_error(error))


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

    Un candidat sans chemin ne passe ni par le `stat` ni par `remove.py` : c'est
    le `clean()` du module qui agit, appelé une fois avec ses candidats.
    """
    known = registry_mod.MODULES if registry is None else registry
    grouped: dict[str, list[CleanCandidate]] = {}
    for candidate in plan.candidates:
        grouped.setdefault(candidate.module, []).append(candidate)

    use_recycle = bool(args.recycle) and Level(plan.level) is not Level.SAFE

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

        state = None
        if module.proc_guard == PROC_GUARD_WARN_AND_SKIP and not args.yes:
            # Une seule interrogation des processus par module, pas par candidat.
            state = registry_mod.proc_guard_state(name)

        pathless: list[CleanCandidate] = []
        for candidate in group:
            skipped = registry_mod.proc_guard_skip(
                module, candidate, yes=bool(args.yes), state=state
            )
            if skipped is not None:
                result.skipped.append(skipped)
                continue
            if candidate.path is None:
                pathless.append(candidate)
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
            _remove_candidate(
                candidate, result, use_recycle=use_recycle, failures=failures
            )

        if pathless and module.clean is not None:
            _merge_result(
                result,
                module.clean(
                    candidates=list(pathless),
                    recycle=use_recycle,
                    yes=bool(args.yes),
                ),
            )


# --------------------------------------------------------------------------- #
# Entrée
# --------------------------------------------------------------------------- #


def ensure_windows(platform: str | None = None) -> None:
    """Refuse tout autre système. Les primitives de `remove.py` sont Win32."""
    current = sys.platform if platform is None else platform
    if not str(current).startswith("win"):
        raise PlatformError(
            "winclean ne fonctionne que sous Windows : la suppression passe par les "
            f"chemins longs \\\\?\\ et par SHFileOperationW. Système détecté : {current}."
        )


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

    try:
        plan = build_plan(args)
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

    if failures and status == EXIT_OK:
        for error in failures:
            print(f"Échec de suppression : {error.path} - {error.message}", file=sys.stderr)
        status = EXIT_REMOVAL
    return status


if __name__ == "__main__":
    sys.exit(main())
