"""Modèle de données, agrégats de rapport et rendu texte / JSON de winclean.

Aucune mutation du système de fichiers ici : ce module ne fait que décrire et
mesurer.

Mécanisme d'import (identique à `scripts/system_inventory/inventory.py`) :
`python scripts/winclean/clean.py` place `scripts/winclean/` en `sys.path[0]`
et la racine du dépôt n'est nulle part ; on l'insère, puis on importe en chemin
complet `scripts.system_inventory.common`. C'est la seule orthographe qui
résolve sous les deux invocations (script direct et `unittest discover` depuis
la racine) : insérer la racine met `scripts/` sur le chemin comme répertoire
parent, pas comme entrée, donc un `system_inventory.common` nu échouerait.

Conventions de langue (décision 20) : tout ce qui est lu par un humain est en
français, tout ce qui est lu par une machine (clés `--json`, jetons de statut,
codes d'avertissement, codes de raison) reste en anglais.
"""

from __future__ import annotations

import os
import shutil
import stat as _stat
import sys
from dataclasses import dataclass, field, fields
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.system_inventory.common import dir_size_on_disk, human_size  # noqa: E402

__all__ = [
    "Level",
    "LEVEL_ORDER",
    "CleanCandidate",
    "CleanResult",
    "CompletedResource",
    "OperationFailure",
    "ModuleDiscoveryError",
    "CleanModule",
    "SkippedEntry",
    "DroppedEntry",
    "AbsorbedEntry",
    "ExcludedEntry",
    "CleanWarning",
    "Plan",
    "RunReport",
    "DISCOVERY_MODES",
    "PROC_GUARDS",
    "SKIP_TOKENS",
    "SKIP_UNATTEMPTED",
    "RECYCLE_FOOTER_LINES",
    "UNMEASURED_CELL",
    "DEFAULT_TRASH_DAYS",
    "estimate_path",
    "volume_of",
    "free_space_by_volume",
    "format_plan",
    "format_result_report",
    "render_warning",
    "to_json_payload",
    "human_size",
    "dir_size_on_disk",
]


# --------------------------------------------------------------------------- #
# Vocabulaire
# --------------------------------------------------------------------------- #


class Level(str, Enum):
    """Niveau d'agressivité d'un module (décision 3)."""

    SAFE = "safe"
    MODERATE = "moderate"
    AGGRESSIVE = "aggressive"

    def __str__(self) -> str:  # pragma: no cover - confort d'affichage
        return self.value


#: Du plus prudent au plus agressif. `--only` ne peut pas remonter ce tableau
#: (décision 12) et `modules_for_level()` prend tout ce qui est <= au niveau.
LEVEL_ORDER: tuple[Level, ...] = (Level.SAFE, Level.MODERATE, Level.AGGRESSIVE)

#: Plancher d'âge par défaut des éléments de la corbeille, en jours
#: (décision 15). Déclaré **ici** et non dans `config.py` : `mod_system` en a
#: besoin et `config` importe déjà `registry_mod`, qui importe `mod_system` —
#: l'y lire créerait un cycle d'import résolu par un module à moitié initialisé.
#: `config.py` le ré-exporte, il reste donc lisible comme
#: `config.DEFAULT_TRASH_DAYS`.
DEFAULT_TRASH_DAYS = 7

#: Mode de découverte déclaré par module (décision 15). Trois valeurs, pas un
#: booléen : `--root` ne borne que `walking`.
DISCOVERY_WALKING = "walking"
DISCOVERY_FIXED = "fixed"
DISCOVERY_PATHLESS = "pathless"
DISCOVERY_MODES: tuple[str, ...] = (DISCOVERY_WALKING, DISCOVERY_FIXED, DISCOVERY_PATHLESS)

#: Force du garde-fou de processus déclarée par module (décision 19).
PROC_GUARD_WARN_AND_SKIP = "warn-and-skip"
PROC_GUARD_WARN_ONLY = "warn-only"
PROC_GUARDS: tuple[str | None, ...] = (PROC_GUARD_WARN_AND_SKIP, PROC_GUARD_WARN_ONLY, None)

#: Jetons de statut d'omission. Aucun n'est une erreur et aucun n'influe sur le
#: code de sortie.
SKIP_RUNNING = "skipped-running"
SKIP_GONE = "skipped-gone"
SKIP_CHANGED = "skipped-changed"
SKIP_UNCONFIRMED = "skipped-unconfirmed"
SKIP_NO_UNDO = "no-undo"
SKIP_UNATTEMPTED = "skipped-unattempted"
SKIP_TOKENS: tuple[str, ...] = (
    SKIP_RUNNING,
    SKIP_GONE,
    SKIP_CHANGED,
    SKIP_UNCONFIRMED,
    SKIP_NO_UNDO,
    SKIP_UNATTEMPTED,
)

#: Classes de raison du rejet par la chaîne de gardes. `protected` laisse le run
#: continuer à 0, `sanity` avorte (Part 1 Phase 4 tâche 4).
DROP_PROTECTED = "protected"
DROP_SANITY = "sanity"

#: Raison d'exclusion par `--offline` (décision 11).
EXCLUDE_NEEDS_NETWORK = "needs-network"


# --------------------------------------------------------------------------- #
# Modèle
# --------------------------------------------------------------------------- #


class ModuleDiscoveryError(Exception):
    """Erreur de frontière d'un module, avec identifiant machine stable."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


@dataclass
class CleanCandidate:
    """Une chose qu'un module propose de supprimer.

    `estimated_bytes` à `None` signifie *non mesurable*, jamais `0` : `None` est
    l'unique encodage de cet état dans tout le paquet, sans booléen compagnon.

    `path` à `None` marque un candidat sans chemin, qui récupère de l'espace via
    une commande externe (`docker system prune`, Part 2). La chaîne de gardes le
    laisse passer, `remove.py` ne le voit jamais, c'est le `clean()` du module
    qui agit.
    """

    module: str
    path: str | None
    label: str
    estimated_bytes: int | None
    level: Level
    reason: str
    no_undo: bool = False
    needs_network: bool = False
    stat_mtime: float | None = None
    resource_id: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.label, str) or not self.label.strip():
            raise ValueError(
                "label obligatoire : un candidat sans libellé est une ligne de plan "
                "qui ne nomme rien et une entrée d'audit inexploitable."
            )

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "path": self.path,
            "label": self.label,
            "estimated_bytes": self.estimated_bytes,
            "level": Level(self.level).value,
            "reason": self.reason,
            "no_undo": self.no_undo,
            "needs_network": self.needs_network,
            "stat_mtime": self.stat_mtime,
            "resource_id": self.resource_id,
        }


@dataclass
class SkippedEntry:
    """Un candidat non traité, avec son jeton de statut et sa raison.

    Volontairement sans colonne d'octets : des octets omis n'ont été ni libérés,
    ni mis en corbeille, ni en échec — l'estimation à laquelle ils appartiennent
    est déjà dans le plan.
    """

    label: str
    path: str | None
    status: str
    reason: str

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "label": self.label,
            "path": self.path,
            "status": self.status,
            "reason": self.reason,
        }


@dataclass(frozen=True)
class CompletedResource:
    """Ressource externe dont l'opération déléguée a réussi."""

    resource_id: str

    def to_json_payload(self) -> dict[str, str]:
        return {"resource_id": self.resource_id}


@dataclass(frozen=True)
class OperationFailure:
    """Échec d'une opération externe, sans inventer un nombre d'octets."""

    resource_id: str
    code: str
    reason: str

    def to_json_payload(self) -> dict[str, str]:
        return {
            "resource_id": self.resource_id,
            "code": self.code,
            "reason": self.reason,
        }


@dataclass
class CleanResult:
    """Ce qu'un module a réellement fait.

    Les trois colonnes d'octets sont distinctes et nullables : un total est la
    somme des contributions *connues*, et vaut `None` seulement quand aucune ne
    l'était. Il n'existe aucun champ booléen de non-mesurabilité — on interroge
    la valeur.

    Les deux listes de chemins sont **séparées** et ne se recouvrent jamais : la
    colonne `failed` compte deux classes de défaillance, et une seule liste ne
    peut pas les porter sans mentir sur l'une des deux. `locked_paths` = entrées
    qu'un verrou de partage a laissées en place pendant `delete_tree` ;
    `recycle_failed_paths` = candidats dont la mise en corbeille a échoué, donc
    intacts sur le disque (décision 14).

    `recycle_events` compte les mises en corbeille réussies. C'est l'*événement*
    qui déclenche le pied de page de récupération différée, jamais une
    comparaison sur `recycled` — cette colonne vaut `None` quand la mesure
    d'avant opération a échoué alors que la corbeille a bien reçu les octets.

    `measured` est le **second avis** sur `estimated`, et pas une quatrième
    colonne d'octets : c'est la somme, sur les candidats du module, de
    `remove.measure_freed()` — donc d'une mesure prise juste avant l'opération et
    relue juste après, jamais de `estimated_bytes`. Nourrie de l'estimation, la
    comparaison que la Part 3 publie tiendrait deux fois le même nombre et son
    écart serait toujours nul. `None` veut dire « aucun candidat de ce module n'a
    produit de figure mesurable » — typiquement un module omis en entier — et ne
    doit jamais être rendu `0` : « pas tenté » et « rien récupéré » sont deux
    lectures différentes, et la seconde est la signature d'un verrou.
    """

    module: str
    estimated: int | None = None
    freed: int | None = None
    recycled: int | None = None
    failed: int | None = None
    measured: int | None = None
    skipped: list[SkippedEntry] = field(default_factory=list)
    locked_paths: list[str] = field(default_factory=list)
    recycle_failed_paths: list[str] = field(default_factory=list)
    recycle_events: int = 0
    completed_resources: list[CompletedResource] = field(default_factory=list)
    operation_failures: list[OperationFailure] = field(default_factory=list)

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "estimated": self.estimated,
            "freed": self.freed,
            "recycled": self.recycled,
            "failed": self.failed,
            "measured": self.measured,
            "skipped": [s.to_json_payload() for s in self.skipped],
            "locked_paths": list(self.locked_paths),
            "recycle_failed_paths": list(self.recycle_failed_paths),
            "recycle_events": self.recycle_events,
            "completed_resources": [r.to_json_payload() for r in self.completed_resources],
            "operation_failures": [f.to_json_payload() for f in self.operation_failures],
        }


@dataclass(frozen=True)
class CleanModule:
    """Enregistrement déclaratif d'un module.

    `discovery`, `proc_guard` et `needs_network` sont **sans valeur par défaut** :
    enregistrer un module sans le classer est un `TypeError` à l'import, pas un
    défaut silencieux. Leur unique site de déclaration est `registry_mod.py`.
    """

    name: str
    level: Level
    requires: tuple[str, ...]
    discover: Callable[..., list[CleanCandidate]]
    clean: Callable[..., CleanResult] | None
    discovery: str
    proc_guard: str | None
    needs_network: bool


@dataclass
class DroppedEntry:
    """Candidat écarté par la chaîne de gardes, avec sa classe de raison."""

    module: str
    label: str
    path: str | None
    reason_class: str
    detail: str = ""

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "label": self.label,
            "path": self.path,
            "reason_class": self.reason_class,
            "detail": self.detail,
        }


@dataclass
class AbsorbedEntry:
    """Candidat absorbé par un ancêtre (décision 13)."""

    module: str
    label: str
    path: str | None
    ancestor_module: str
    ancestor_label: str
    ancestor_path: str | None

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "label": self.label,
            "path": self.path,
            "ancestor_module": self.ancestor_module,
            "ancestor_label": self.ancestor_label,
            "ancestor_path": self.ancestor_path,
        }


@dataclass
class ExcludedEntry:
    """Candidat retiré par `--offline` (décision 11)."""

    module: str
    label: str
    path: str | None
    estimated_bytes: int | None
    reason: str = EXCLUDE_NEEDS_NETWORK

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "label": self.label,
            "path": self.path,
            "estimated_bytes": self.estimated_bytes,
            "reason": self.reason,
        }


@dataclass
class CleanWarning:
    """Un avertissement, porté par un code anglais stable et ses champs.

    Le texte français est un *rendu* de cette entrée, jamais sa source : la
    charge JSON transporte le code et les champs, pas la phrase.
    """

    code: str
    fields: dict[str, Any] = field(default_factory=dict)

    def to_json_payload(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"code": self.code}
        payload.update(self.fields)
        return payload


# --------------------------------------------------------------------------- #
# Mesure
# --------------------------------------------------------------------------- #


def estimate_path(path: str | os.PathLike[str]) -> int | None:
    """Tarife un chemin, ou `None` s'il est illisible à sa racine.

    `dir_size_on_disk` renvoie toujours un `int` : il avale chaque `OSError` par
    entrée, donc un sous-arbre interdit contribue `0` et la fonction ne peut pas
    renvoyer `None`. On tente donc un `os.scandir()` sur la racine du candidat
    avant de tarifer : si elle refuse, l'estimation est `None` (« inconnu »),
    et non `0` (« vide »).

    Sous cette racine l'estimation reste au mieux-effort et peut être
    silencieusement basse ; `dir_size_on_disk` somme `st_size`, la taille
    logique, donc les fichiers compressés ou creux sur-déclarent.
    """
    p = Path(path)
    try:
        st = p.stat()
    except OSError:
        return None
    if not _stat.S_ISDIR(st.st_mode):
        return st.st_size
    try:
        with os.scandir(p) as it:
            for _entry in it:
                break
    except OSError:
        return None
    return dir_size_on_disk(p)


def volume_of(path: str | os.PathLike[str]) -> str:
    """Racine de volume d'un chemin (`C:\\`, `\\\\serveur\\partage\\`)."""
    drive, _rest = os.path.splitdrive(str(path))
    if not drive:
        return str(Path(path).anchor) or str(path)
    return drive + os.sep


def free_space_by_volume(paths: Iterable[str | os.PathLike[str]]) -> dict[str, int]:
    """Espace libre par volume touché, en octets. Les volumes illisibles sont omis.

    Le dédoublonnage est insensible à la casse : les chemins arrivent tels que
    les modules les ont produits, et `c:\\` et `C:\\` sont le même volume — sans
    quoi le même espace libre est annoncé deux fois.
    """
    out: dict[str, int] = {}
    seen: set[str] = set()
    for p in paths:
        if p is None:
            continue
        vol = volume_of(p)
        key = os.path.normcase(vol)
        if key in seen:
            continue
        seen.add(key)
        try:
            out[vol] = shutil.disk_usage(vol).free
        except OSError:
            continue
    return out


def sum_known(values: Iterable[int | None]) -> int | None:
    """Somme des contributions connues, `None` si aucune ne l'était."""
    known = [v for v in values if v is not None]
    if not known:
        return None
    return sum(known)


# --------------------------------------------------------------------------- #
# Agrégats de sortie
# --------------------------------------------------------------------------- #


@dataclass
class Plan:
    """Ce qu'un run a trouvé, et ce qu'il a écarté en chemin.

    Les trois sections `dropped`, `absorbed` et `excluded` correspondent aux
    trois seules étapes qui retirent un candidat qu'un module a réellement
    trouvé. Sans elles, un module qui découvre beaucoup et ne rapporte rien est
    indiscernable d'un module qui n'a rien trouvé.
    """

    candidates: list[CleanCandidate] = field(default_factory=list)
    dropped: list[DroppedEntry] = field(default_factory=list)
    absorbed: list[AbsorbedEntry] = field(default_factory=list)
    excluded: list[ExcludedEntry] = field(default_factory=list)
    warnings: list[CleanWarning] = field(default_factory=list)
    free_space: dict[str, int] = field(default_factory=dict)
    roots: list[str] = field(default_factory=list)
    level: Level = Level.SAFE
    apply: bool = False
    top: int | None = None
    discovery_by_module: dict[str, str] = field(default_factory=dict)

    def sorted_candidates(self) -> list[CleanCandidate]:
        return sort_candidates(self.candidates)

    def total_estimated(self) -> int | None:
        """Total annoncé, `None` si aucun candidat n'a pu être mesuré.

        Un plan **vide** vaut `0` et non `unknown` : il n'y a rien
        d'inmesurable, il n'y a rien du tout. Sans ce cas, un run qui ne trouve
        aucun candidat affiche « Total estimé : unknown », ce qui se lit comme
        un échec de mesure.
        """
        if not self.candidates:
            return 0
        return sum_known(c.estimated_bytes for c in self.candidates)

    def unpriced_modules(self) -> list[str]:
        seen: list[str] = []
        for c in self.candidates:
            if c.estimated_bytes is None and c.module not in seen:
                seen.append(c.module)
        return seen

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "level": Level(self.level).value,
            "apply": self.apply,
            "roots": list(self.roots),
            "candidates": [c.to_json_payload() for c in self.sorted_candidates()],
            "total_estimated_bytes": self.total_estimated(),
            "unpriced_modules": self.unpriced_modules(),
            "dropped": [d.to_json_payload() for d in self.dropped],
            "absorbed": [a.to_json_payload() for a in self.absorbed],
            "excluded": [e.to_json_payload() for e in self.excluded],
            "warnings": [w.to_json_payload() for w in self.warnings],
            "free_space_before": dict(self.free_space),
        }


@dataclass
class RunReport:
    """Ce qu'un run `--apply` a réellement fait, imprimé depuis un `finally`."""

    results: list[CleanResult] = field(default_factory=list)
    estimated_total: int | None = None
    free_space_after: dict[str, int] = field(default_factory=dict)
    warnings: list[CleanWarning] = field(default_factory=list)
    interrupted: bool = False
    removal_attempted: bool = False
    """Une suppression a été **tentée**, réussie ou non. Déclenche l'historique.

    Positionné par la boucle d'application juste avant d'agir, jamais déduit des
    totaux d'octets : `freed` vaut `None` quand la récupération n'est pas
    mesurable, et un run `moderate` met en corbeille donc libère zéro. Un
    historique conditionné aux octets manquerait le run destructeur le plus
    courant. Voir `history.py`.
    """

    @property
    def status(self) -> str:
        return "interrupted" if self.interrupted else "completed"

    @property
    def recycle_happened(self) -> bool:
        """Au moins une mise en corbeille a réussi. L'événement, pas un total.

        `total_recycled()` peut valoir `None` alors que la corbeille a reçu des
        octets : la mesure d'avant opération avait échoué. Clé du pied de page de
        décision 18 sur `recycled > 0`, ce run perdrait la seule sortie qu'on lui
        propose.
        """
        return any(r.recycle_events for r in self.results)

    def total_freed(self) -> int | None:
        return sum_known(r.freed for r in self.results)

    def total_recycled(self) -> int | None:
        return sum_known(r.recycled for r in self.results)

    def total_failed(self) -> int | None:
        return sum_known(r.failed for r in self.results)

    def to_json_payload(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "results": [r.to_json_payload() for r in self.results],
            "estimated_total_bytes": self.estimated_total,
            "freed_total_bytes": self.total_freed(),
            "recycled_total_bytes": self.total_recycled(),
            "recycle_happened": self.recycle_happened,
            "failed_total_bytes": self.total_failed(),
            "free_space_after": dict(self.free_space_after),
            "warnings": [w.to_json_payload() for w in self.warnings],
        }


def sort_candidates(candidates: Iterable[CleanCandidate]) -> list[CleanCandidate]:
    """Tri par octets estimés décroissants, `None` en dernier."""
    return sorted(
        candidates,
        key=lambda c: (
            c.estimated_bytes is None,
            -(c.estimated_bytes if c.estimated_bytes is not None else 0),
            c.label,
        ),
    )


# --------------------------------------------------------------------------- #
# Rendu des avertissements
# --------------------------------------------------------------------------- #

WARNING_TEMPLATES: dict[str, str] = {
    "ceiling-total-partial": (
        "Total du plan partiel : aucune estimation fournie par {modules}. "
        "Le plafond est vérifié sur un total possiblement sous-évalué."
    ),
    "sync-root": (
        "Racine synchronisée dans le cloud : {root}. Toute suppression y est "
        "répliquée à distance et la corbeille ne défait pas le côté distant."
    ),
    "proc-running": "Processus propriétaire actif ({processes}) : {label}.",
    "proc-unknown": (
        "État des processus inconnu (tasklist indisponible) : {label}. "
        "Traité comme actif."
    ),
    "discovery-slow": (
        "Découverte longue : {seconds} s au total. Détail par module : {per_module}."
    ),
    "recycle-inert": (
        "--recycle est accepté mais sans effet au niveau {level} : la suppression y "
        "est directe."
    ),
    # Décision 4. Les 10 % sont une **approximation** : le quota réel de la
    # corbeille vit dans la configuration du shell et n'est pas lisible depuis la
    # bibliothèque standard. D'où l'impératif au conditionnel — et d'où le fait
    # que l'absence de cet avertissement ne prouve rien.
    "recycle-bin-allowance": (
        "{label} pèse {ratio:.0%} du volume {volume} : sa mise en corbeille peut "
        "dépasser le quota de la corbeille, auquel cas le shell supprime "
        "définitivement et sans le dire. L'annulation ne peut pas être garantie."
    ),
    "recycle-bin-allowance-unknown": (
        "{label} n'a pas pu être mesuré sur le volume {volume} : on ignore donc si "
        "la corbeille l'acceptera, et l'annulation ne peut pas être garantie."
    ),
    # Texte de plan du module `recycle-bin` (partie 3, phase 3). Un `discover_*()`
    # ne rend que des candidats : ces quatre codes sont son seul canal vers le
    # plan, et trois d'entre eux parlent précisément des cas où il n'y a **pas**
    # de candidat à porter le message.
    "recycle-bin-floor": (
        "Corbeille : plancher d'âge de {days} jour(s). Les éléments plus récents "
        "sont conservés."
    ),
    "recycle-bin-floor-none": (
        "Corbeille : aucun plancher d'âge (--trash-days 0). Tout élément déjà en "
        "corbeille est éligible."
    ),
    # Décision 18. La mention de `--trash-days 0` est **obligatoire** ici : c'est
    # la seule chose qui rende éligibles les octets qu'un run `moderate` vient de
    # mettre en corbeille, et un utilisateur qui arrive de ce run lit sinon un
    # module qui ne vide rien sans savoir quoi changer.
    "recycle-bin-not-yet-eligible": (
        "Corbeille : {count} élément(s) plus récents que {days} jour(s), "
        "{size} au total, ne sont pas encore éligibles. --trash-days 0 les rend "
        "éligibles immédiatement."
    ),
    "recycle-bin-sid-unknown": (
        "Corbeille ignorée : le SID du compte courant n'a pas pu être résolu, donc "
        "aucun candidat n'est proposé. winclean n'énumère jamais les corbeilles des "
        "autres comptes."
    ),
    "recycle-bin-undatable": (
        "Corbeille : {count} élément(s) sans horodatage exploitable, omis. Une "
        "entrée qu'on ne sait pas dater n'est jamais supposée ancienne."
    ),
}


def render_warning(warning: CleanWarning) -> str:
    """Rend un avertissement en français depuis son code et ses champs."""
    template = WARNING_TEMPLATES.get(warning.code)
    if template is None:
        details = ", ".join(f"{k}={v}" for k, v in sorted(warning.fields.items()))
        return f"[{warning.code}] {details}" if details else f"[{warning.code}]"
    try:
        return template.format(**warning.fields)
    except (KeyError, IndexError, ValueError, TypeError):  # pragma: no cover - filet
        details = ", ".join(f"{k}={v}" for k, v in sorted(warning.fields.items()))
        return f"[{warning.code}] {details}"


# --------------------------------------------------------------------------- #
# Rendu texte
# --------------------------------------------------------------------------- #

_LABEL_WIDTH = 22
_SIZE_WIDTH = 10

#: Cellule de la comparaison estimé/mesuré quand il n'y a rien à montrer.
#: Distincte du `unknown` de `human_size` : celui-ci dit « la mesure a échoué »
#: sur une colonne d'octets, celle-ci dit « il n'y a pas de comparaison » sur une
#: ligne dont un des deux termes manque. Jamais `0` — Part 3 Phase 4 tâche 1 :
#: écrire `0` ferait lire « rien n'a été récupéré » là où le module n'a pas été
#: tenté du tout.
UNMEASURED_CELL = "—"


def _compare_cell(value: int | None) -> str:
    """Une case de la table de comparaison. `None` → `—`, jamais `0`."""
    return UNMEASURED_CELL if value is None else human_size(value)


def _delta_cell(estimated: int | None, measured: int | None) -> str:
    """L'écart, **rendu et non dérivé** quand un des deux termes manque.

    Le seul autre nombre à portée serait `estimated - 0`, qui imprime toute
    l'estimation du module comme un manque : la table dirait « rien n'a été
    récupéré » d'un module que le rapport déclare non tenté. Le signe est affiché
    sans être jugé — un `measured` supérieur à l'estimation est légitime (l'arbre
    a grossi entre le plan et l'application, ou le module a retiré des octets que
    l'estimation ne cotait pas).
    """
    if estimated is None or measured is None:
        return UNMEASURED_CELL
    delta = measured - estimated
    if delta == 0:
        return human_size(0)
    return f"{'+' if delta > 0 else '-'}{human_size(abs(delta))}"

#: Pied de page de récupération différée (décision 18). Les trois sorties sont
#: dans cet ordre, et le `--trash-days 0` de la troisième est **obligatoire** :
#: `recycle-bin` applique un plancher d'âge de 7 jours, donc sans lui la commande
#: imprimée ignore précisément les octets que le run vient de mettre en corbeille.
#: winclean ne vide jamais la corbeille implicitement.
RECYCLE_FOOTER_LINES: tuple[str, ...] = (
    "Ces octets sont toujours sur le disque : la corbeille diffère la récupération, "
    "elle ne la réalise pas, et elle ne garantit pas l'annulation.",
    "Trois façons de les récupérer, dans l'ordre du moins au plus explicite :",
    "  1. --no-recycle au prochain run : la suppression est alors immédiate.",
    "  2. la corbeille de Windows, vidée depuis son interface.",
    "  3. python scripts/winclean/clean.py --level aggressive --only recycle-bin "
    "--apply --trash-days 0",
)


def _as_plan(plan: Plan | Sequence[CleanCandidate]) -> Plan:
    if isinstance(plan, Plan):
        return plan
    return Plan(candidates=list(plan))


def _row(candidate: CleanCandidate) -> str:
    size = human_size(candidate.estimated_bytes)
    label = candidate.label
    line = f"  {label:<{_LABEL_WIDTH}} {size:>{_SIZE_WIDTH}}  {candidate.reason}"
    if candidate.path:
        line += f"\n  {'':<{_LABEL_WIDTH}} {'':>{_SIZE_WIDTH}}  {candidate.path}"
    flags = []
    if candidate.needs_network:
        flags.append("réseau requis pour reconstituer")
    if candidate.no_undo:
        flags.append("sans retour arrière")
    if flags:
        line += f"\n  {'':<{_LABEL_WIDTH}} {'':>{_SIZE_WIDTH}}  [{' / '.join(flags)}]"
    return line


def format_plan(plan: Plan | Sequence[CleanCandidate]) -> str:
    """Rend le plan en texte : en-tête, tableau trié, puis les trois sections.

    Accepte un `Plan` ou une simple liste de candidats.
    """
    plan = _as_plan(plan)
    out: list[str] = []
    out.append(f"Niveau : {Level(plan.level).value}")
    if plan.roots:
        out.append("Racines de recherche : " + ", ".join(plan.roots))
    for volume, free in sorted(plan.free_space.items()):
        out.append(f"Espace libre sur {volume} : {human_size(free)}")
    for warning in plan.warnings:
        out.append(f"Avertissement : {render_warning(warning)}")
    out.append("")

    ordered = plan.sorted_candidates()
    hidden = 0
    if plan.top is not None and plan.top >= 0 and len(ordered) > plan.top:
        hidden = len(ordered) - plan.top
        shown = ordered[: plan.top]
    else:
        shown = ordered

    if not ordered:
        out.append("Aucun élément trouvé.")
    else:
        under_roots: list[CleanCandidate] = []
        outside_roots: list[CleanCandidate] = []
        for c in shown:
            mode = plan.discovery_by_module.get(c.module, DISCOVERY_WALKING)
            (under_roots if mode == DISCOVERY_WALKING else outside_roots).append(c)
        if under_roots:
            out.append("Sous vos racines :")
            out.extend(_row(c) for c in under_roots)
        if outside_roots:
            if under_roots:
                out.append("")
            out.append("Par utilisateur, hors de vos racines :")
            out.extend(_row(c) for c in outside_roots)

    total = plan.total_estimated()
    out.append("")
    out.append(f"Total estimé : {human_size(total)} sur {len(ordered)} élément(s)")
    if hidden:
        out.append(
            f"{hidden} ligne(s) masquée(s) par --top ; le total et, avec --apply, "
            "la suppression portent sur le plan complet."
        )

    if plan.dropped:
        out.append("")
        out.append("Écartés par les gardes :")
        for d in plan.dropped:
            detail = f" ({d.detail})" if d.detail else ""
            out.append(f"  [{d.reason_class}]{detail} {d.label} - {d.path or '-'}")

    if plan.absorbed:
        out.append("")
        out.append("Absorbés par un ancêtre (octets comptés une seule fois) :")
        for a in plan.absorbed:
            out.append(
                f"  {a.label} - {a.path or '-'} absorbé par "
                f"{a.ancestor_label} ({a.ancestor_path or '-'})"
            )

    if plan.excluded:
        out.append("")
        out.append("Exclus par --offline (reconstitution impossible hors ligne) :")
        for e in plan.excluded:
            out.append(f"  {e.label} - {human_size(e.estimated_bytes)} - {e.path or '-'}")

    out.append("")
    if plan.apply:
        out.append("--apply est actif : les éléments ci-dessus vont être supprimés.")
    else:
        out.append("Simulation : rien n'a été supprimé. Ajoutez --apply pour agir.")
    return "\n".join(out)


def format_result_report(
    report: RunReport | Sequence[CleanResult],
) -> str:
    """Rend le rapport d'exécution : trois colonnes d'octets, puis les omissions."""
    if isinstance(report, RunReport):
        run = report
    else:
        run = RunReport(results=list(report))

    out: list[str] = []
    out.append("Résultat par module :")
    header = (
        f"  {'module':<{_LABEL_WIDTH}} {'libéré':>{_SIZE_WIDTH}} "
        f"{'corbeille':>{_SIZE_WIDTH}} {'échec':>{_SIZE_WIDTH}}"
    )
    out.append(header)
    for r in run.results:
        out.append(
            f"  {r.module:<{_LABEL_WIDTH}} {human_size(r.freed):>{_SIZE_WIDTH}} "
            f"{human_size(r.recycled):>{_SIZE_WIDTH}} {human_size(r.failed):>{_SIZE_WIDTH}}"
        )

    out.append("")
    out.append(
        f"Total libéré : {human_size(run.total_freed())} - "
        f"mis en corbeille : {human_size(run.total_recycled())} - "
        f"en échec : {human_size(run.total_failed())}"
    )
    out.append(f"Estimé au plan : {human_size(run.estimated_total)}")

    out.append("")
    out.append("Estimé vs mesuré :")
    out.append(
        f"  {'module':<{_LABEL_WIDTH}} {'estimé':>{_SIZE_WIDTH}} "
        f"{'mesuré':>{_SIZE_WIDTH}} {'écart':>{_SIZE_WIDTH}}"
    )
    for r in run.results:
        out.append(
            f"  {r.module:<{_LABEL_WIDTH}} {_compare_cell(r.estimated):>{_SIZE_WIDTH}} "
            f"{_compare_cell(r.measured):>{_SIZE_WIDTH}} "
            f"{_delta_cell(r.estimated, r.measured):>{_SIZE_WIDTH}}"
        )

    skipped = [(r.module, s) for r in run.results for s in r.skipped]
    if skipped:
        out.append("")
        out.append("Omis :")
        for module, s in skipped:
            out.append(f"  [{s.status}] {module} - {s.label} - {s.path or '-'} : {s.reason}")

    locked = [(r.module, p) for r in run.results for p in r.locked_paths]
    if locked:
        out.append("")
        out.append("Verrouillés (fichier en cours d'utilisation, laissé en place) :")
        for module, path in locked:
            out.append(f"  {module} - {path}")

    refused = [(r.module, p) for r in run.results for p in r.recycle_failed_paths]
    if refused:
        out.append("")
        out.append(
            "Mise en corbeille refusée - le chemin est laissé en place, rien n'a été "
            "supprimé :"
        )
        for module, path in refused:
            out.append(f"  {module} - {path}")

    completed = [(r.module, item) for r in run.results for item in r.completed_resources]
    if completed:
        out.append("")
        out.append("Ressources externes supprimées :")
        for module, item in completed:
            out.append(f"  {module} - {item.resource_id}")

    operation_failures = [
        (r.module, failure) for r in run.results for failure in r.operation_failures
    ]
    if operation_failures:
        out.append("")
        out.append("Opérations externes en échec :")
        for module, failure in operation_failures:
            out.append(
                f"  [{failure.code}] {module} - {failure.resource_id} : {failure.reason}"
            )

    if run.recycle_happened:
        # Décision 18 : déclenché par l'événement, pas par `recycled > 0`.
        total = run.total_recycled()
        out.append("")
        if total is None:
            out.append(
                "Des octets sont partis en corbeille, en quantité inconnue : la mesure "
                "d'avant opération a échoué."
            )
        else:
            out.append(f"Mis en corbeille : {human_size(total)}.")
        out.extend(RECYCLE_FOOTER_LINES)

    for warning in run.warnings:
        out.append(f"Avertissement : {render_warning(warning)}")

    for volume, free in sorted(run.free_space_after.items()):
        out.append(f"Espace libre sur {volume} après opération : {human_size(free)}")

    if run.interrupted:
        out.append("")
        out.append("Run interrompu : le rapport ci-dessus est partiel.")
    return "\n".join(out)


def to_json_payload(obj: Plan | RunReport | CleanResult | CleanCandidate) -> dict[str, Any]:
    """Charge JSON d'un plan, d'un rapport, d'un résultat ou d'un candidat.

    Règle générale, valable pour toutes les parties : ce que le texte énonce
    comme *donnée* a une entrée ici, ce qu'il énonce comme *prose* n'en a pas.
    Donnée : les candidats et leurs champs, les colonnes d'octets, la liste des
    omissions, les avertissements (code + champs), l'espace libre par volume.
    Prose : le tableau des niveaux, le pied de simulation, toute phrase
    explicative — un appelant a les données dont elles sont dérivées.
    """
    payload_method = getattr(obj, "to_json_payload", None)
    if payload_method is None:  # pragma: no cover - garde-fou de typage
        raise TypeError(f"pas de charge JSON pour {type(obj).__name__}")
    return payload_method()


def field_names(cls: type) -> tuple[str, ...]:
    """Noms des champs d'une dataclasse, pour les assertions structurelles."""
    return tuple(f.name for f in fields(cls))
