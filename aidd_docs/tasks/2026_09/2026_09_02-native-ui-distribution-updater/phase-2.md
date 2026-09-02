---
status: in-progress
---

# Instruction: Construire le contrat visuel vérifiable et la nouvelle coque egui

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

~~~txt
.
├── assets
│   └── brand
│       └── devtoolbox.svg             ✅ fournir une source vectorielle déterministe
├── docs
│   └── visual-contract.md             ✅ figer tokens, métriques, états et références
└── src
    └── ui
        ├── applications_view.rs       ✏️ employer les composants communs
        ├── automations_view.rs        ✏️ employer les composants communs
        ├── cleanup_view.rs            ✏️ employer les composants communs
        ├── components.rs              ✅ centraliser cartes, boutons, badges et états
        ├── dialogs.rs                 ✏️ harmoniser les dialogues
        ├── docker_view.rs             ✏️ employer les composants communs
        ├── egui_app.rs                ✏️ introduire la coque et la navigation adaptative
        ├── fonts.rs                   ✏️ charger police système locale ou fallback embarqué
        ├── mod.rs                     ✏️ exposer les primitives
        ├── models_view.rs             ✏️ employer les composants communs
        ├── terminal_view.rs           ✏️ employer les composants communs
        ├── theme.rs                   ✅ définir les tokens et politiques de mouvement
        └── visual_harness.rs          ✅ rendre les états de référence en test
~~~

## User Journey

~~~mermaid
flowchart TD
  A[Ouvrir DevToolBox] --> B[Voir une coque stable et lisible]
  B --> C[Choisir une vue dans la navigation]
  C --> D[Identifier titre action contenu et état]
  D --> E[Changer thème ou taille sans perdre d action]
~~~

## Test Scope

~~~mermaid
---
title: Test scope
---
journey
  section Setup
    Charger le harness avec renderer police et échelle épinglés => rendu reproductible: 5: system
  section Happy path
    Rendre thèmes clair sombre et système => tokens et contrastes conformes: 5: system
    Parcourir la coque à 400x300 et 1280x800 => toutes les vues restent atteignables: 5: system
    Laisser l interface inactive => aucune animation ne réclame de repaint permanent: 5: system
  section Edge case - ancienne configuration
    Charger les valeurs light ou dark historiques => préférence conservée sans migration destructive: 1: system
~~~

## Wireframe

~~~txt
┌─────────────────────────────────────────────────────────────────┐
│ (1) Identité · contexte · actions globales                      │
├───────────────┬─────────────────────────────────────────────────┤
│ (2) Navigation│ (3) En-tête de la vue                          │
│               ├─────────────────────────────────────────────────┤
│               │ (4) Contenu : cartes · listes · formulaires    │
│               │                                                 │
│               ├─────────────────────────────────────────────────┤
│               │ (5) État contextuel                            │
└───────────────┴─────────────────────────────────────────────────┘
~~~

1. Identité : rattache le contenu à la fenêtre sans recréer ses contrôles.
2. Navigation : expose les espaces principaux et se replie aux petites tailles.
3. En-tête : réserve le titre, les filtres et l'action principale.
4. Contenu : applique les mêmes primitives à toutes les vues.
5. État : stabilise progression, succès, indisponibilité et erreur.

## Tasks to do

### 1) Écrire le contrat visuel exécutable

> Remplacer le jugement beau par des règles contrôlables.

1. Documenter palette, typographie, grille 4 px, rayons, ombres, iconographie et états.
2. Exiger WCAG AA : 4,5:1 pour texte normal, 3:1 pour grand texte et composants.
3. Définir transitions de 120–180 ms, zéro boucle décorative et arrêt des repaints après stabilisation.
4. Utiliser une police système locale sur macOS lorsqu'elle est lisible et décodable ; si la découverte, la lecture ou le décodage échoue, conserver sans erreur le fallback proportionnel egui et Noto Emoji existant, sans redistribuer de police Apple.
5. Définir un benchmark reproductible : démarrage, rendu et idle ; réserver les seuils natifs de 2,5 s au premier frame, 16,7 ms au 95e percentile pendant une transition et 1 % CPU idle à la qualification.

### 2) Construire thème et primitives

> Éliminer les widgets par défaut incohérents.

1. Centraliser couleurs, espacements, tailles, rayons et mouvement dans theme.rs.
2. Créer en-tête, carte, bouton principal/secondaire, badge, progression, état vide/indisponible et message de statut.
3. Dessiner l'iconographie de coque avec des formes egui ; réserver emoji et images aux contenus configurables.

### 3) Recomposer la coque

> Installer une hiérarchie stable sans changer les contrats métier.

1. Remplacer la rangée d'onglets par une navigation latérale adaptative.
2. Conserver les labels accessibles, raccourcis, confirmations et commandes existants.
3. Replier la navigation à 400x300 sans masquer la vue active ni les actions globales.

### 4) Tester et diffuser

> Faire des états de référence une preuve de code, pas une approbation humaine.

1. Rendre les primitives dans un harness épinglé et vérifier tokens, contraste, focus et géométrie.
2. Tester qu'une UI inactive cesse de demander des repaints et qu'aucune transition ne dépasse 180 ms.
3. Ajouter une commande de benchmark harness avec environnement et métriques sérialisées, sans transformer les seuils dépendants du matériel en test unitaire instable.
4. Migrer les vues une par une sans modifier leur backend.
5. Réserver les captures, mesures natives et l'approbation du propriétaire à la qualification décrite en phase 6.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Chaque token et seuil possède une définition unique et testable ; une police macOS absente, illisible ou invalide sélectionne le fallback embarqué sans modifier la configuration. |
| 2 | Les composants de même rôle partagent rendu et états hover/focus/disabled, avec les ratios AA calculés par test. |
| 3 | Toutes les vues sont accessibles au clavier et à la souris aux deux tailles de référence sans chevauchement. |
| 4 | Les tests fonctionnels existants restent verts, le harness couvre les états de référence, produit les métriques attendues et l'UI stabilisée ne repeint pas en boucle. |
