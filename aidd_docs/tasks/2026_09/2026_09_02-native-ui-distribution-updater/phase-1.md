---
status: done
---

# Instruction: Stabiliser la version, les contrats de plateforme et le port macOS

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

~~~txt
.
├── .cargo
│   └── config.toml                    ✅ fixer la cible minimale macOS
├── Cargo.toml                         ✏️ passer à 0.10.0 et déclarer les dépendances ciblées
├── Cargo.lock                         ✏️ verrouiller le graphe résolu
├── rust-toolchain.toml                ✅ épingler Rust 1.93.0 et les composants
├── config
│   └── default.macos.json             ✅ fournir des actions macOS valides
└── src
    ├── main.rs                        ✏️ neutraliser version, titre, logs et modules
    ├── applications
    │   ├── macos.rs                   ✅ observer les exécutables actifs en best effort
    │   └── mod.rs                     ✏️ router le provider macOS
    ├── macos
    │   ├── autostart.rs               ✅ produire un LaunchAgent réversible et testable
    │   └── mod.rs                     ✅ exposer l'intégration macOS
    ├── platform
    │   ├── macos.rs                   ✅ résoudre chemins, machine et startup
    │   └── mod.rs                     ✏️ compléter toutes les façades par OS
    └── storage
        └── json.rs                    ✏️ sélectionner le défaut macOS
~~~

## User Journey

~~~mermaid
flowchart TD
  A[Lancer DevToolBox] --> B[Résoudre version chemins et configuration de l OS]
  B --> C[Afficher l écran principal]
  C --> D[Exécuter les capacités portables]
  D --> E[Présenter explicitement toute capacité non disponible]
~~~

## Test Scope

~~~mermaid
---
title: Test scope
---
journey
  section Setup
    Installer le toolchain épinglé et les cibles Rust => environnement reproductible: 5: cli
  section Happy path
    Tester les résolveurs avec environnements injectés => chemins macOS et LaunchAgent déterministes: 5: cli
    Vérifier les cibles macOS arm64 et Intel => aucun trou cfg dans le programme: 5: cli
    Charger les trois configurations livrées => un défaut valide par OS: 5: cli
  section Edge case - intégration absente
    Simuler un échec launchctl ou processus => résultat vide ou avertissement sans panique: 1: cli
  section Teardown
    Supprimer les fixtures temporaires => aucun fichier de compte réel touché: 5: cli
~~~

## Tasks to do

### 1) Fixer version et toolchain

> Établir une base SemVer supérieure aux releases existantes.

1. Passer le package à 0.10.0 et employer CARGO_PKG_VERSION dans l'application.
2. Épingler Rust 1.93.0, rustfmt, clippy, les cibles Apple et MACOSX_DEPLOYMENT_TARGET=13.0.
3. Distinguer la version applicative du champ de schéma des configurations existantes.

### 2) Compléter les façades macOS

> Faire compiler macOS sans router implicitement vers Linux.

1. Ajouter les chemins Application Support et Logs avec sources d'environnement injectables.
2. Résoudre l'identifiant machine sans panique, avec priorité à DEVTOOLBOX_MACHINE_ID.
3. Ajouter un provider de processus borné, sans privilège, qui revient vide en cas d'échec.
4. Remplacer les branches cfg ambiguës par des branches explicites.

### 3) Ajouter le démarrage macOS réversible

> Tester la génération et la suppression sans appeler le launchd réel.

1. Générer com.rebellioussmile.devtoolbox.plist vers le binaire réellement lancé.
2. Écrire atomiquement, inspecter sans effet de bord et rendre register/unregister idempotents.
3. Isoler l'appel launchctl derrière un exécuteur injecté ; un refus est journalisé sans bloquer l'application.

### 4) Livrer une configuration macOS honnête

> Rendre les fonctionnalités disponibles et indisponibles explicites.

1. Ajouter des commandes shell macOS sûres dans default.macos.json.
2. Définir la matrice coeur : Actions, Terminal et lecture Docker portables ; Automatisations, recommandations et Nettoyage affichent un état macOS indisponible tant que leur backend manque.
3. Retirer les titres globaux Windows et centraliser le chemin du journal via platform::state_log_path().

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Le package et l'UI annoncent 0.10.0 sous Rust 1.93.0, les tags antérieurs sont reconnus comme pré-updater, les builds macOS portent une deployment target 13.0 et le champ version des configurations conserve sa valeur de schéma lors des round trips. |
| 2 | cargo check atteint les deux cibles Apple sans fonction cfg manquante ; les tests injectés couvrent chaque chemin et fallback. |
| 3 | Les fixtures créent puis retirent uniquement le plist DevToolBox, et tout échec injecté reste non bloquant. |
| 4 | Chaque OS sélectionne exactement son fichier par défaut et macOS distingue par test les trois capacités coeur des vues indisponibles. |
