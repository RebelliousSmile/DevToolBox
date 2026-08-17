# Préférences — éditeur de configuration (catégories + actions)

Requête affinée et corrigée (brainstorm validé + 3 deal breakers du challenge
corrigés + 4 blockers du shadow-areas scan corrigés) pour devtoolbox :
ajouter dans l'écran Préférences une vue unifiée catégories+actions avec CRUD
complet sur une action.

## Portée fonctionnelle

- Vue unifiée dans l'écran Préférences : catégories et, pour chacune, la
  liste de ses actions.
- CRUD complet sur une action (créer, lire, modifier, supprimer).
- Suppression d'action avec confirmation bloquante (réutilise le pattern
  existant de suppression de catégorie, `apply_category_action`).
- Actions orphelines d'une catégorie supprimée regroupées dans une
  pseudo-catégorie visible "Sans catégorie" (non supprimable, non
  renommable), réassignables via le dropdown catégorie.
- **Nom réservé** : l'id et le nom "Sans catégorie" (comparaison
  insensible à la casse/accents) sont réservés à cette pseudo-catégorie.
  `add_category`/`rename_category` rejettent toute catégorie utilisateur qui
  tenterait de prendre ce nom ou cet id, sur le même principe que le rejet
  actuel des ids dupliqués (`storage::categories::add_category`).

## Champs éditables par action

- `name`
- Exécutable + arguments : liste répétable de valeurs, recomposée
  automatiquement en une chaîne compatible avec le tokenizer cross-platform
  existant (`src/ui/terminal_view.rs::tokenize()` — pas
  `src/windows/process.rs::tokenize()`, qui vit dans un module `#![cfg(windows)]`
  et ne compile pas sur Linux ; les deux implémentent les mêmes règles de
  guillemets, mais seule celle de `terminal_view.rs` est utilisable
  cross-platform par l'éditeur).
- `category` : dropdown limité aux catégories existantes.
- `icon` : sélecteur à liste curée, partagé avec le champ icône des
  catégories (qui est aujourd'hui un texte libre). **Source de la liste** :
  un ensemble fixe et codé en dur dans le binaire (const Rust), pas un scan
  du disque (`icons_dirs()`) — comportement déterministe, indépendant de ce
  qui est installé sur la machine. Le choix précis des icônes qui composent
  cet ensemble reste à faire lors du dimensionnement/implémentation.
- `is_favorite`
- `shortcut`

La convention `@python ...` (Applications perso, lancement via interpréteur
Python résolu dynamiquement) est traitée comme un exécutable normal : le
chemin du script devient le premier élément de la liste d'arguments — pas de
mode dédié.

**Validation avant sauvegarde** : avant toute écriture disque, la chaîne
recomposée à partir de la liste exécutable+arguments est re-tokenisée via
`terminal_view::tokenize()` et le résultat est comparé à la liste d'origine. Si le
round-trip ne restitue pas exactement les mêmes valeurs (ex. valeur
contenant un guillemet littéral non gérable), la sauvegarde est bloquée et
une erreur inline est affichée sur le champ concerné — aucune commande non
conforme au tokenizer ne peut atteindre le disque silencieusement.

## Hors scope explicite (cette itération)

- Champs avancés `variant_group` / `group_name` / `variant_label` /
  `machine_specific` (édition JSON uniquement).
- `config/builtin-actions.json` (actions injectées au build).
- Champs `default_settings` (`theme` / `launch_at_startup` /
  `show_descriptions`) — restent édités à la main dans le JSON pour cette
  itération.

## Comportements

- Id de commande généré automatiquement (slug du nom + suffixe
  anti-collision), jamais saisi.
- Persistance sur disque au fil de l'eau. **Correction (vérifié dans le
  code)** : `render_actions_view` reconstruit `build_display_groups` à
  chaque frame (`src/ui/egui_app.rs:1200`) — l'écran Actions reflète donc les
  changements **immédiatement**, sans redémarrage, exactement comme le CRUD
  catégories déjà en place. L'affirmation initiale ("redémarrage requis")
  était fausse et est retirée de la spec.

## Dépendances techniques (explicites)

- `src/storage/commands.rs` n'expose aujourd'hui que `toggle_favorite`. Le
  CRUD complet nécessite d'y ajouter `add_command`, `update_command` et
  `remove_command`, sur le même principe que `add_category`/
  `rename_category`/`remove_category` dans `src/storage/categories.rs`
  (rejet d'id dupliqué à la création, erreur explicite si l'id est
  introuvable en modification/suppression).
- La génération de slug (repli ASCII des accents + suffixe anti-collision)
  n'existe pas encore dans le code ; c'est un utilitaire à écrire pour cette
  fonctionnalité, pas une brique réutilisable existante.
