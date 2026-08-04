"""Couche de sûreté : normalisation, chemins protégés, bon sens, imbrication, plafond.

Rien n'est supprimé ici. Cette couche décide seulement de ce qui a le droit
d'arriver jusqu'à `remove.py`, et **rend compte de ce qu'elle a retiré** : un
garde peut refuser un candidat, il n'a pas le droit de le faire disparaître.

La chaîne s'applique aux **candidats**, jamais aux racines `--root` : `path_sanity()`
refuse tout chemin de moins de 10 caractères, donc filtrer les racines avec elle
avorterait le run sur un `--root C:\\dev` parfaitement légitime. Une racine
dangereuse **avertit** (racine synchronisée dans le cloud), elle ne refuse pas.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Iterable, Mapping, Sequence

from scripts.winclean.common import (
    AbsorbedEntry,
    CleanCandidate,
    CleanWarning,
    DROP_PROTECTED,
    DROP_SANITY,
    DroppedEntry,
    human_size,
    sum_known,
)

__all__ = [
    "CeilingExceeded",
    "MIN_PATH_LENGTH",
    "DEFAULT_PROTECTED",
    "default_protected",
    "normalise",
    "is_protected",
    "path_sanity",
    "screen_candidates",
    "absorb_nested",
    "enforce_ceiling",
]

#: En deçà, un chemin ne peut pas désigner une cible de suppression plausible.
MIN_PATH_LENGTH = 10


class CeilingExceeded(Exception):
    """Le total du plan dépasse `--max-delete-bytes`. Le run doit s'arrêter."""


# --------------------------------------------------------------------------- #
# Normalisation
# --------------------------------------------------------------------------- #


def normalise(path: str | os.PathLike[str]) -> str:
    """Forme absolue, résolue et `normcase` d'un chemin.

    Toute comparaison de chemins du paquet passe par ici. La résolution est ce
    qui permet d'attraper une jonction pointant dans un arbre protégé ; le
    `normcase` est appliqué en dernier, ce qui rend la fonction idempotente.
    """
    return os.path.normcase(str(Path(path).resolve()))


def _require_normalised(path: str) -> None:
    if path != normalise(path):
        raise ValueError(
            f"chemin non normalisé : {path!r} - appelez normalise() avant toute comparaison"
        )


def _under(path: str, root: str) -> bool:
    root = root.rstrip("\\/")
    if not root:
        return False
    return path == root or path.startswith(root + os.sep)


# --------------------------------------------------------------------------- #
# Chemins protégés
# --------------------------------------------------------------------------- #

#: Dossiers de données utilisateur. La racine de profil n'y est **pas** :
#: `path_sanity()` la refuse comme *candidat*, donc aucun module ne peut la
#: proposer, et ce n'est pas à la protection de le dire — la protection
#: s'applique aux sous-arbres, et protéger la racine de profil,
#: `%LOCALAPPDATA%` ou `%APPDATA%` annulerait silencieusement les modules qui
#: émettent précisément des candidats à l'intérieur.
_USER_DATA_FOLDERS: tuple[str, ...] = (
    "Documents",
    "Desktop",
    "Pictures",
    "Videos",
    "Music",
    "Downloads",
)


def default_protected(env: Mapping[str, str] | None = None) -> tuple[str, ...]:
    """Racines de données utilisateur, lues dans l'environnement et normalisées.

    Jamais écrites en dur sous la forme `C:\\Users\\<nom>\\...`.
    """
    environment = os.environ if env is None else env
    bases: list[str] = []
    for key in ("USERPROFILE", "OneDrive", "OneDriveCommercial", "OneDriveConsumer"):
        value = environment.get(key)
        if value and value not in bases:
            bases.append(value)
    roots: list[str] = []
    for base in bases:
        for name in _USER_DATA_FOLDERS:
            roots.append(normalise(os.path.join(base, name)))
    return tuple(dict.fromkeys(roots))


DEFAULT_PROTECTED: tuple[str, ...] = default_protected()


def is_protected(path: str, protected: Iterable[str]) -> bool:
    """Vrai si `path` est un chemin protégé ou se trouve sous l'un d'eux.

    N'accepte que des entrées déjà passées par `normalise()`, et le vérifie.
    """
    _require_normalised(path)
    for root in protected:
        if _under(path, root):
            return True
    return False


def path_sanity(path: str) -> str | None:
    """`None` si le chemin est une cible plausible, sinon un jeton de détail.

    Refuse : moins de 10 caractères, la racine de profil elle-même, toute racine
    de lecteur, toute racine de partage UNC.
    """
    _require_normalised(path)
    profile = os.environ.get("USERPROFILE")
    if profile and path == normalise(profile):
        return "profile-root"
    drive, rest = os.path.splitdrive(path)
    if drive and not rest.strip("\\/"):
        # Testé avant la longueur : `C:\` mesure 3 caractères et serait rapporté
        # `too-short`, un jeton exact mais qui nomme la mauvaise règle.
        return "unc-share-root" if drive.startswith("\\\\") else "drive-root"
    if len(path) < MIN_PATH_LENGTH:
        return "too-short"
    return None


def screen_candidates(
    candidates: Iterable[CleanCandidate],
    protected: Iterable[str] | None = None,
) -> tuple[list[CleanCandidate], list[DroppedEntry]]:
    """Applique `normalise` → `is_protected` → `path_sanity` à chaque candidat.

    Renvoie les candidats retenus et ceux écartés, avec leur classe de raison
    (`protected` ou `sanity`) pour la section « écartés » du plan. Les candidats
    sans chemin traversent la chaîne intacts.
    """
    protected_roots = DEFAULT_PROTECTED if protected is None else tuple(protected)
    kept: list[CleanCandidate] = []
    dropped: list[DroppedEntry] = []
    for candidate in candidates:
        if candidate.path is None:
            kept.append(candidate)
            continue
        normalised = normalise(candidate.path)
        if is_protected(normalised, protected_roots):
            dropped.append(
                DroppedEntry(
                    module=candidate.module,
                    label=candidate.label,
                    path=candidate.path,
                    reason_class=DROP_PROTECTED,
                    detail="",
                )
            )
            continue
        detail = path_sanity(normalised)
        if detail is not None:
            dropped.append(
                DroppedEntry(
                    module=candidate.module,
                    label=candidate.label,
                    path=candidate.path,
                    reason_class=DROP_SANITY,
                    detail=detail,
                )
            )
            continue
        kept.append(candidate)
    return kept, dropped


# --------------------------------------------------------------------------- #
# Imbrication (décision 13 : l'ancêtre gagne)
# --------------------------------------------------------------------------- #


def _rank_of(module: str, rank: Mapping[str, int] | None) -> int:
    if rank is not None:
        return rank.get(module, len(rank))
    from scripts.winclean import registry_mod  # import tardif : évite un cycle

    return registry_mod.ownership_rank(module)


def absorb_nested(
    candidates: Sequence[CleanCandidate],
    rank: Mapping[str, int] | None = None,
) -> tuple[list[CleanCandidate], list[AbsorbedEntry]]:
    """Écarte tout candidat descendant d'un autre et le rapporte comme absorbé.

    La profondeur seule décide de l'imbrication : l'ancêtre garde le candidat,
    ses descendants sont absorbés, et les octets ne sont comptés qu'une fois.
    Le rang de possession — l'index du module dans `MODULE_ORDER` — ne tranche
    que l'égalité de chemin **exacte**, jamais l'ordre d'insertion dans la liste.
    """
    path_typed = [c for c in candidates if c.path is not None]
    pathless = [c for c in candidates if c.path is None]

    def sort_key(candidate: CleanCandidate) -> tuple[int, int, str]:
        normalised = normalise(candidate.path or "")
        return (
            len(Path(normalised).parts),
            _rank_of(candidate.module, rank),
            normalised,
        )

    kept: list[CleanCandidate] = []
    kept_normalised: list[str] = []
    absorbed: list[AbsorbedEntry] = []
    for candidate in sorted(path_typed, key=sort_key):
        normalised = normalise(candidate.path or "")
        ancestor: CleanCandidate | None = None
        for existing, existing_normalised in zip(kept, kept_normalised):
            if _under(normalised, existing_normalised):
                ancestor = existing
                break
        if ancestor is None:
            kept.append(candidate)
            kept_normalised.append(normalised)
            continue
        absorbed.append(
            AbsorbedEntry(
                module=candidate.module,
                label=candidate.label,
                path=candidate.path,
                ancestor_module=ancestor.module,
                ancestor_label=ancestor.label,
                ancestor_path=ancestor.path,
            )
        )
    return kept + pathless, absorbed


# --------------------------------------------------------------------------- #
# Plafond d'octets
# --------------------------------------------------------------------------- #


def enforce_ceiling(
    plan: Iterable[CleanCandidate],
    max_bytes: int,
) -> tuple[int, list[CleanWarning]]:
    """Vérifie le total du plan contre `--max-delete-bytes`.

    Le total est la somme des estimations **connues** : une estimation à `None`
    ne contribue rien, car `sum()` sur une liste en contenant lève, et la
    convertir en `0` serait la même affirmation faite en silence. Une estimation
    inconnue **n'avorte pas** le run — sinon un seul module sans chemin mettrait
    son veto à tout plan où il apparaît — mais le total est alors rapporté comme
    partiel, avec le nom des modules qui n'ont fourni aucun chiffre
    (avertissement `ceiling-total-partial`).

    Lève `CeilingExceeded` dès un octet au-dessus de la limite, avec un message
    nommant le total, la limite en vigueur **et** `--max-delete-bytes` : un plan
    agressif sur une machine réellement pleine peut légitimement dépasser
    50 Gio, et un plafond qui arrête le run sans nommer la molette est une
    impasse.
    """
    candidates = list(plan)
    total = sum_known(c.estimated_bytes for c in candidates) or 0
    warnings: list[CleanWarning] = []
    unpriced: list[str] = []
    for candidate in candidates:
        if candidate.estimated_bytes is None and candidate.module not in unpriced:
            unpriced.append(candidate.module)
    if unpriced:
        warnings.append(
            CleanWarning(
                code="ceiling-total-partial",
                fields={"modules": ", ".join(unpriced), "module_names": unpriced},
            )
        )
    if total > max_bytes:
        raise CeilingExceeded(
            f"Plan refusé : total estimé {human_size(total)} ({total} octets) "
            f"au-dessus de la limite en vigueur {human_size(max_bytes)} "
            f"({max_bytes} octets). Relevez --max-delete-bytes pour passer outre."
        )
    return total, warnings
