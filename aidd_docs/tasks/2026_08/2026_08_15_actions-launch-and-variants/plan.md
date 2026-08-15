---
objective: "Restaurer, sur la vue Actions du launcher DevToolBox, le clic-pour-lancer sur les cartes et le regroupement par variante avec menu déroulant, deux capacités perdues lors du portage multi-OS tao/WinUI3/GDI → eframe/egui."
status: in-progress
---

# Plan: Clic-pour-lancer et regroupement par variante sur la vue Actions

## Overview

| Field      | Value                                                                                                                                                                                 |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Goal**   | Réparer deux régressions du portage multi-OS sur la vue Actions : les cartes n'ont aucun moyen de lancer une commande, et les commandes partageant un `variant_group` s'affichent à plat au lieu d'une carte groupée avec sélecteur |
| **Source** | Découvert en session pendant la validation manuelle Windows du portage (`aidd_docs/tasks/2026_08/2026_08_15_windows-build-validation/`, phase-3, tâches 2.2/2.3/3/5.1/5.3/6 bloquées faute de clic-pour-lancer) ; confirmé par l'utilisateur, qui a rappelé le menu déroulant de variantes existant côté UI Win32 pré-port (commit `11a7a72`) |

Le modèle `storage::Command` porte toujours `variant_group`/`group_name`/`variant_label` (`src/storage/models.rs:46-51`) et `config/builtin-actions.json` les renseigne toujours pour ses 14 commandes `@python` (groupes `sftp-sync`, `email-to-markdown`, `lyremember`) — aucune donnée n'a été perdue au portage, seule la logique d'affichage (`build_display_groups`, `render_card`, dans `src/ui/egui_app.rs`) les ignore.

## Phases

| #   | Phase                                             | File                          |
| --- | -------------------------------------------------- | ------------------------------ |
| 1   | Clic-pour-lancer sur les cartes simples             | [`phase-1.md`](./phase-1.md)   |
| 2   | Regroupement par variante avec menu déroulant       | [`phase-2.md`](./phase-2.md)   |

## Decisions

| Decision                                                                                     | Why                                                                                                                                                                                                 |
| ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Carte groupée : bouton "Lancer" explicite plutôt que clic sur le corps de la carte             | Le corps d'une carte groupée doit rester manipulable pour changer la sélection du menu déroulant sans déclencher un lancement accidentel ; un clic-corps=lancement, comme pour les cartes simples, serait ambigu avec l'ouverture du menu. |
| Le lancement carte réutilise `terminal_view::launch_captured` plutôt qu'un nouveau pipeline de spawn | Ce module est déjà cross-platform, gère la cascade de résolution `@python` (`resolve_action`) et le streaming d'événements — dupliquer ce code pour les cartes serait une régression de maintenabilité sans bénéfice. |
| État de lancement dédié (`action_rx`/`action_running`), séparé de `terminal_rx`/`terminal_running` de la vue Terminal | Un lancement depuis une carte Actions ne doit pas interférer avec une commande en cours dans la vue Terminal, ni réciproquement — chaque vue garde son propre slot de réception d'événements.        |
