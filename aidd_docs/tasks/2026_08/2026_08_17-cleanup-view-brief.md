# Vue Nettoyage

La vue « Applications » retrouve son intention d'origine et devient « Nettoyage » : un tableau de bord pour repérer ce qui consomme du disque et le récupérer. Elle garde en haut le relevé des applications installées (purement informatif, inchangé) et gagne une section « Bibliothèques » listant, module par module, tout ce que winclean sait nettoyer : les caches globaux par utilisateur (uv, pnpm, npm, pip, cargo, bun) comme les artefacts sous les racines (node_modules, target/, __pycache__…), chacun avec sa taille et son chemin. Chaque ligne peut être nettoyée individuellement ; le run global reste disponible via les cartes Actions existantes.

Le principe directeur : aucune logique de scan ou de suppression en Rust. La brique commune — et multi-OS — est `scripts/winclean` (`clean.py --json` pour mesurer, `--only <module> --apply` pour nettoyer un seul outil), déjà segmentée par module (`CacheSpec` dans `mod_dev.py`, modules Linux dédiés). La vue et les cartes Actions consomment le même script et évoluent indépendamment.

## Ce qui est acquis

- Renommage : onglet « Applications » → « Nettoyage » ; liste d'apps conservée telle quelle (pas de désinstallation).
- Section Bibliothèques = **tout le plan winclean** par module, pas seulement les 6 caches globaux.
- Tailles mesurées à la demande : bouton « Analyser » lançant `clean.py --json` en arrière-plan (spinner pendant le scan), pas de scan automatique à l'ouverture.
- Nettoyage par ligne : niveau **safe uniquement**, précédé d'un dialogue de confirmation UI récapitulant taille et chemin, puis `--only <module> --apply`.
- Cartes Actions « Nettoyage fichiers dev » conservées ; vue et cartes partagent les briques winclean sans se coupler.
- DRY : tout ce que la vue affiche ou déclenche passe par clean.py ; rien de dupliqué côté Rust.
- Contrat JSON vérifié (`common.py`) : chaque candidat expose `module`, `path`, `label`, `estimated_bytes`, `level`, `needs_network` ; le plan ajoute `total_estimated_bytes`, `unpriced_modules`, `warnings` ; un run `--apply` ajoute par module `freed`, `failed`, `measured`, `locked_paths`, `operation_failures`. La vue agrège les candidats par `module` (un module walking peut en produire plusieurs).

## Décisions (issues du shadow report)

- **Portée de l'analyse** : Analyser lance `clean.py --json --level moderate` (plan seul, aucun risque). Les lignes moderate sont affichées grisées, sans bouton Nettoyer — le champ `level` de chaque candidat pilote l'affichage. Aggressive exclu (modules opt-in).
- **Nettoyage par ligne** : la vue lance `--only <module> --apply --json` et consomme le payload `run` elle-même (pas de streaming vers le Terminal ; les cartes Actions gardent leur sortie texte).
- **Concurrence** : slot unique `action_running` réutilisé ; Analyser et les boutons Nettoyer sont désactivés tant qu'une commande tourne.
- **Outil absent** : aucun candidat émis → aucune ligne ; la vue affiche ce que le JSON renvoie. `unpriced_modules` peut alimenter une mention « non mesurable ».
- **Échec d'analyse** : bandeau rouge « Analyse échouée (code N) » + dernières lignes stderr + bouton Réessayer ; même traitement pour JSON invalide ; tailles précédentes conservées mais marquées obsolètes.
- **Échec partiel d'apply** : ligne affiche « X libérés, Y en échec (fichiers verrouillés) » depuis `freed` et le compte `locked_paths` + `operation_failures` (`failed` est un total d'octets, éventuellement `None`, jamais un compte) ; taille rafraîchie depuis `measured` sans relancer l'analyse complète.
- **Succès observable** : la ligne reste, taille mise à jour depuis `measured`, badge « Nettoyé : X libérés » jusqu'à la prochaine analyse ; succès = `locked_paths` et `operation_failures` vides et run non interrompu (`status == "completed"`).
- **Multi-OS** : aucun nom de module en dur — la vue affiche les modules du JSON et repasse ce même nom à `--only` (les modules Linux ont des noms distincts : `pip-cache-linux`, `pnpm-store-linux`, `apt-cache`).
- `needs_network` (exposé par le JSON) affiché comme « re-téléchargement requis ».

## Encore ouvert

- Un bouton « Tout nettoyer » dans la vue elle-même : redondant avec les cartes, différé.
- Disposition fine (tri par taille, regroupements) : à trancher au moment du design.

## Prochain pas

Planifier la vue : renommage de l'onglet, section Bibliothèques, appels clean.py en arrière-plan et parsing du JSON.
