# Onglet « Docker » — tableau de bord local minimal (brainstorm approuvé)

> Statut : **approuvé par le user le 2026-08-19** (sortie de `/aidd-refine:01-brainstorm`).
> Demande initiale : « peut-être faudrait-il un onglet à part pour docker, avec la
> liste des images, voir ce qui est actif, et pouvoir purger une image / volume qui
> n'est plus utile. un docker hub en très très basique. »

## Périmètre

- Nouvel onglet dans la barre de navigation, **Linux uniquement** dans un premier
  temps (portage Windows plus tard, depuis la machine Windows, conformément à la
  convention du repo : les chemins Windows ne sont pas compile-vérifiables ici).
- **Local uniquement** : aucun échange avec un registre distant (pas de recherche,
  pas de pull).

## Visibilité de l'onglet

- L'onglet est **visible dès que le binaire `docker` est installé** sur la machine
  (détection au lancement de l'app — l'installation du binaire ne change pas en
  cours de session).
- Si le **daemon ne répond pas** (service arrêté, permission refusée…), l'onglet
  reste visible et affiche un message clair (« daemon Docker inaccessible ») avec
  un bouton **« Réessayer »**. Un daemon démarré après coup devient donc
  accessible sans relancer DevToolBox.
- L'onglet n'est **masqué** que si le binaire `docker` est introuvable.

## Contenu — trois sections

- **Conteneurs** : tous, actifs et arrêtés (nom, image, état). Actions :
  **arrêter** un conteneur actif, **supprimer** un conteneur arrêté — chacune
  avec confirmation.
- **Images** : liste (repo:tag, taille, date) avec **badge « utilisée »** si au
  moins un conteneur, même arrêté, s'appuie dessus.
- **Volumes** : liste avec indication des **orphelins** (rattachés à aucun
  conteneur).

## Purge

- **Suppression ciblée uniquement** (une image, un volume), toujours précédée
  d'une **confirmation**.
- Image « utilisée » : bouton **grisé** — il faut d'abord supprimer les
  conteneurs qui en dépendent. Jamais de `--force`.
- Volume rattaché à un conteneur : même logique, seuls les orphelins sont
  supprimables.
- **Pas** de bouton « prune » global.

## Comportement

- Rafraîchissement **manuel** : chargement à l'ouverture de l'onglet + bouton
  « Actualiser » (même modèle que la vue Automatisations).
- Toute erreur d'une commande docker en cours de session (daemon tombé,
  permission…) s'affiche dans l'onglet sans le faire disparaître, avec
  possibilité de réessayer.

## Hors périmètre (explicitement)

- Registre distant (recherche/pull), démarrage/création de conteneurs, logs,
  réseaux, prune global, support Podman, portage Windows (fera l'objet d'une
  suite).
