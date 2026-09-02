---
status: done
---

# Instruction: Rétablir l'invariant thème-palette dans egui

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

```txt
.
├── docs
│   └── visual-contract.md            ✏️ préciser l'invariant entre préférence, thème actif et palette
└── src
    └── ui
        └── theme.rs                  ✏️ sélectionner et configurer chaque thème sans contamination croisée
```

## User Journey

```mermaid
---
title: Sélection cohérente du thème
---
flowchart TD
  Preference["Charger la préférence visuelle"]
  Resolve["Résoudre clair sombre ou système"]
  Select["Sélectionner le thème egui actif"]
  Configure["Configurer chaque style avec sa palette"]
  Render["Rendre une surface cohérente"]

  Preference --> Resolve
  Resolve --> Select
  Select --> Configure
  Configure --> Render

  style Preference fill:#dbeafe,color:#172554
  style Resolve fill:#e0e7ff,color:#1e1b4b
  style Select fill:#ede9fe,color:#2e1065
  style Configure fill:#f3e8ff,color:#3b0764
  style Render fill:#dcfce7,color:#14532d
```

## Test Scope

```mermaid
---
title: Test scope
---
journey
  section Setup
    Créer un contexte egui initialement sombre => contexte déterministe disponible: 5: system
  section Happy path
    Appliquer la préférence claire => thème actif fond texte et rayures utilisent la palette claire: 5: system
    Appliquer la préférence sombre => thème actif fond texte et rayures utilisent la palette sombre: 5: system
  section Edge case - thème système
    Résoudre la préférence système dans un contexte déterministe => le style actif correspond au thème résolu sans contamination: 1: system
```

## Wireframe

```txt
┌─────────────────────────────────────────────────────────────────┐
│ (1) Chrome de fenêtre                                           │
├─────────────────────────────────────────────────────────────────┤
│ (2) Navigation horizontale                                     │
├─────────────────────────────────────────────────────────────────┤
│ (3) En-tête de la vue                                          │
│ (4) Rangée d'actions                                           │
├─────────────────────────────────────────────────────────────────┤
│ (5) Tableau                                                    │
│     en-tête · lignes alternées · colonnes                       │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

1. Chrome : conserve le titre et les contrôles natifs de la fenêtre.
2. Navigation : garde les espaces principaux dans la coque compacte existante.
3. En-tête : maintient le titre et le contexte de la vue active.
4. Actions : regroupe les commandes propres à la vue.
5. Tableau : conserve la structure actuelle des données et de leurs colonnes.

## Tasks to do

### `1)` Reproduire l'incohérence par test

> Verrouiller le défaut observé avant de modifier l'application du thème.

1. Initialiser un contexte dont le thème actif contredit la préférence demandée.
2. Appliquer `ThemeMode::Light`, puis constater par assertions le thème actif, `dark_mode`, le fond, le texte et la couleur de rayure attendus.
3. Ajouter le scénario symétrique pour `ThemeMode::Dark` et couvrir la préférence système.

### `2)` Corriger l'application du thème

> Garantir que la préférence, le style actif et la palette décrivent toujours le même mode.

1. Remplacer la mutation du style actif par la sélection explicite du thème egui.
2. Configurer les styles clair et sombre séparément à partir de `palette(false)` et `palette(true)`.
3. Faire de `ThemeMode::System` une sélection explicite de `ThemePreference::System`, afin qu'elle annule aussi une préférence forcée antérieure.
4. Conserver les espacements, rayons, sélection et ratios de contraste existants.

### `3)` Formaliser l'invariant visuel

> Empêcher une future régression lors d'une évolution des matériaux natifs.

1. Documenter l'alignement obligatoire entre préférence, `Context::theme`, `Visuals::dark_mode` et palette.
2. Exécuter les tests ciblés de thème puis les assertions Rust avant commit.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Le test échoue avec l'implémentation actuelle en reproduisant un fond clair combiné à des tokens sombres, puis couvre les trois préférences. |
| 2 | Une préférence claire sélectionne un style entièrement clair et une préférence sombre un style entièrement sombre, indépendamment de l'état initial du contexte. |
| 3 | Le contrat documenté correspond aux assertions exécutables et les ratios AA existants restent verts. |
