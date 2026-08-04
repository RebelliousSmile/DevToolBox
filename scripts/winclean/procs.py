"""Détection de processus. Informative, jamais coercitive.

Ce module lit la liste des processus et n'agit sur aucun : aucune terminaison,
aucune suspension, aucune élévation. Un propriétaire actif fait **omettre** un
candidat, il ne fait pas fermer le programme de l'utilisateur. Le nom des
commandes qui tueraient un processus est absent de ce fichier, jusque dans les
commentaires : un test le vérifie sur la source, et une mention en prose
suffirait à faire passer une assertion censée constater une absence réelle.

Le contrat entier tient dans la différence entre `set()` et `None` : un ensemble
vide signifie « la question a été posée et rien ne correspond », `None` signifie
« la question n'a pas pu être posée ». Les deux sont faux au sens booléen, donc
un appelant qui teste la vérité seule lit « inconnu » comme « rien ne tourne » —
et supprime un `target/` pendant qu'un `cargo build` écrit dedans, sur les
machines les moins capables de rendre leur liste de processus. On teste
`is None` avant la vérité, jamais un second drapeau.
"""

from __future__ import annotations

import csv
import io
import subprocess
import sys
from typing import Iterable

__all__ = [
    "TASKLIST_COMMAND",
    "TASKLIST_TIMEOUT_SECONDS",
    "parse_image_names",
    "is_running",
    "unknown_is_running",
]

#: `/NH` retire l'en-tête, `/FO CSV` évite la colonne à largeur fixe que la
#: sortie par défaut tronque au-delà de ~25 caractères de nom d'image.
TASKLIST_COMMAND: tuple[str, ...] = ("tasklist", "/FO", "CSV", "/NH")

#: Au-delà, on préfère « inconnu » à un run suspendu : un garde qui bloque est
#: pire que le même garde qui avertit.
TASKLIST_TIMEOUT_SECONDS = 15

_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0) if sys.platform == "win32" else 0


def parse_image_names(csv_text: str) -> set[str]:
    """Noms d'image d'une sortie `tasklist /FO CSV /NH`, en minuscules.

    Le CSV est parsé par le module `csv` et non découpé sur les virgules : un
    titre de fenêtre ou un nom d'image peut en contenir, entre guillemets.
    """
    names: set[str] = set()
    for row in csv.reader(io.StringIO(csv_text)):
        if not row:
            continue
        image = row[0].strip()
        if image:
            names.add(image.lower())
    return names


def _run_tasklist() -> str | None:
    """Sortie brute de `tasklist`, ou `None` si la commande n'a pas abouti.

    Indirection déliberée : les tests la remplacent par une capture réelle
    plutôt que de dépendre de la liste de processus de la machine de test.
    """
    try:
        completed = subprocess.run(
            list(TASKLIST_COMMAND),
            capture_output=True,
            text=True,
            timeout=TASKLIST_TIMEOUT_SECONDS,
            creationflags=_NO_WINDOW,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    return completed.stdout


def is_running(names: Iterable[str]) -> set[str] | None:
    """Ceux de `names` qui tournent, `set()` si aucun, `None` si on ne sait pas.

    La comparaison est insensible à la casse ; les noms rendus gardent
    l'orthographe de l'appelant, puisqu'ils finissent dans une raison affichée.
    """
    wanted = {name for name in names if name}
    if not wanted:
        # Rien n'a été demandé : rien ne peut tourner. Pas d'inconnu ici, et pas
        # de processus lancé pour une question vide.
        return set()
    output = _run_tasklist()
    if output is None:
        return None
    running = parse_image_names(output)
    return {name for name in wanted if name.lower() in running}


def unknown_is_running(matched: set[str] | None) -> bool:
    """Verdict d'un garde `warn-and-skip` : l'état inconnu vaut « actif ».

    Un garde qui n'a pas pu être évalué protège encore, sinon il ne protège que
    les machines où il était de toute façon inutile. L'appelant reste chargé de
    rapporter *quelle* des deux situations il a rencontrée : la raison affichée
    nomme l'état inconnu ou le processus trouvé, jamais les deux à la fois.
    """
    return matched is None or bool(matched)
