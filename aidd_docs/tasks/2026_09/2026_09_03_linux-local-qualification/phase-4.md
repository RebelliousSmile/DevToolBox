---
status: pending
---

<!-- Fill or omit these sections; never add, rename, or reorder one. -->

# Instruction: Documentation de la preuve datée

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

```txt
.
├── docs/
│   └── release-readiness.md                  ✏️ (ajout de la section « Qualification locale Linux — <date> »)
└── aidd_docs/tasks/2026_09/2026_09_03_linux-local-qualification/
    └── evidence/
        ├── linux-theme-light.png              ✅ (capture en thème clair)
        └── linux-theme-dark.png                ✅ (capture en thème sombre)
```

## User Journey

```mermaid
flowchart TD
  A[Rassembler les résultats et captures des phases 1-3] --> B[Capturer les thèmes clair et sombre à l'écran]
  B --> C[Rédiger la section datée dans release-readiness.md]
  C --> D[Lier les captures depuis evidence/]
  D --> E[Nommer explicitement ce qui reste non couvert]
```

## Test Scope

<!-- Required for every phase. Keep Setup, Happy path, any qualifying Edge cases, and any required Teardown in this one journey. -->

```mermaid
---
title: Test scope
---
journey
  %% Every task has exactly one actor: browser, api, cli, or system.
  section Setup
    Réunir les preuves des phases 1-3 (captures, sorties dpkg/mount) => matériel de preuve complet: 5: system
  section Happy path
    Capturer linux-theme-light.png et linux-theme-dark.png depuis le paquet installé => les deux thèmes sont couverts par une preuve visuelle, comme pour Windows: 5: system
    Rédiger « Qualification locale Linux — <date> » sur le modèle de la section Windows => section datée présente avec liens vers evidence/: 5: cli
    Lister explicitement Minisign, updater réel, Wayland et Ubuntu 24.04 comme non couverts => portée de la preuve non surinterprétée: 5: cli
  section Teardown
    Relire la matrice de qualification existante => aucune ligne existante n'est modifiée, seule la section datée est ajoutée: 5: system
```

## Tasks to do

### `1)` Rédiger la section de preuve

> Suivre exactement la forme de « Qualification locale Windows — 2 septembre 2026 » : ce qui a été construit, installé/désinstallé, avec quel résultat, et les limites explicites.

1. Ajouter une section `## Qualification locale Linux — <date du jour>` en fin de `docs/release-readiness.md`
2. Décrire la construction (`.deb` + AppImage 0.10.0), l'installation/désinstallation réelles, et le résultat de la résolution des ressources (`/usr/lib/devtoolbox/` pour le deb, `$APPDIR/usr/lib/devtoolbox/` pour l'AppImage)
3. Capturer `evidence/linux-theme-light.png` et `evidence/linux-theme-dark.png` (thème clair puis sombre, basculé dans les Préférences) depuis l'application installée par le `.deb` (résultat de la phase 2), à l'image des deux captures de la section Windows
4. Lier les captures et sorties de `evidence/` (thèmes clair/sombre, menu GNOME, exécution AppImage, layout dpkg, montage FUSE)
5. Nommer explicitement ce que cette preuve NE qualifie PAS : Minisign, activation réelle de l'updater, Wayland, Ubuntu 24.04 — sur le modèle de la phrase finale de la section Windows

## Test acceptance criteria

<!-- Each criterion is an observable behavior, not a command. -->

| Task | Acceptance criteria              |
| ---- | -------------------------------- |
| 1... | `docs/release-readiness.md` contient une section datée du jour, avec les captures thème clair et thème sombre liées, les autres preuves liées, et une phrase explicite sur ce qui reste non couvert (Minisign, updater, Wayland, Ubuntu 24.04). |
