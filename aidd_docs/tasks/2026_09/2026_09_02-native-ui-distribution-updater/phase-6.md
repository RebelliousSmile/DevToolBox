---
status: done
---

# Instruction: Livrer la CI, les portes de release et le dossier de qualification

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

~~~txt
.
├── .github
│   └── workflows
│       ├── ci.yml                     ✅ vérifier code et config sur trois OS
│       └── release.yml                ✅ construire en draft et imposer les portes
├── CHANGELOG.md                       ✏️ préparer 0.10.0
├── README.md                          ✏️ documenter installation update retrait et support
├── docs
│   └── release-readiness.md           ✅ nommer acteurs preuves et prérequis externes
├── scripts
│   ├── generate-update-manifest.py    ✅ agréger les artefacts signés
│   ├── verify-release-config.py       ✅ tester les portes sans secrets réels
│   └── verify-release-manifest.py     ✅ comparer manifeste et assets
└── aidd_docs
    └── memory
        ├── architecture.md            ✏️ enregistrer plateformes et updater
        ├── codebase-map.md            ✏️ enregistrer les modules
        ├── deployment.md              ✏️ enregistrer build et qualification
        ├── design.md                  ✏️ enregistrer le contrat visuel
        └── testing.md                 ✏️ enregistrer tests et matrice native
~~~

## User Journey

~~~mermaid
flowchart TD
  A[Pousser code et tag 0.10.0] --> B[Exécuter CI sans secrets]
  B --> C[Construire les cinq formats en draft]
  C --> D{Environnement release approuvé et secrets présents}
  D -->|non| E[Conserver le draft non publié avec diagnostic]
  D -->|oui| F[Signer notariser et qualifier]
  F --> G{Toutes les preuves présentes}
  G -->|non| E
  G -->|oui| H[Publier release puis latest json]
~~~

## Test Scope

~~~mermaid
---
title: Test scope
---
journey
  section Setup
    Charger fixtures de secrets présents absents et invalides => portes déterministes: 5: cli
  section Happy path
    Valider workflows et scripts localement => jobs matrices et dépendances cohérents: 5: cli
    Simuler cinq artefacts signés => manifeste exact et publication autorisée: 5: cli
  section Edge case - qualification absente
    Retirer une preuve native ou signature => release stable refusée avant publication: 1: cli
  section Edge case - version incohérente
    Diverger tag Cargo ou assets => draft refusé sans latest json: 1: cli
  section Teardown
    Supprimer fixtures générées => dépôt sans secret ni artefact temporaire: 5: cli
~~~

## Tasks to do

### 1) Installer la CI sans secrets

> Vérifier le code et préparer les paquets sur des runners explicites.

1. Exécuter format, check, clippy, tests et builds sur Windows x64, Ubuntu 22.04 x64, macOS arm64 et Intel.
2. Vérifier OS, architecture et toolchain au début de chaque job et ne pas utiliser latest en release.
3. Épingler chaque action tierce à un SHA complet, déclarer permissions en lecture par défaut et accorder contents:write uniquement au job de publication protégé.
4. Sur PR externe, ne charger aucun secret et valider seulement configuration, tests et paquets non signés.

### 2) Construire une release draft

> Ne rendre aucun asset visible avant agrégation complète.

1. Refuser toute divergence entre tag v0.10.0 ou supérieur, Cargo, paquets et manifeste.
2. Construire NSIS, deux DMG, deb, AppImage et payloads d'update.
3. Agréger dans un draft ; générer signatures et latest.json seulement dans l'environnement protégé.
4. Exiger le trousseau public de production, comparer ses empreintes aux métadonnées produites par build.rs et refuser tout artefact construit en mode updater désactivé.
5. Publier latest.json en dernier après comparaison octet pour octet des assets.

### 3) Rendre les portes explicites et testables

> Représenter les dépendances humaines sans bloquer la fin du code.

1. Nommer propriétaire du dépôt, mainteneur de release et opérateur QA avec leurs responsabilités.
2. Lister Apple Developer, Developer ID, notarisation, Authenticode, horodatage, clés updater hors ligne et accès aux machines.
3. Exiger preuves Ventura arm64 et Intel, Windows 11, Ubuntu X11 et Wayland, installation, update, retrait et captures clair ou sombre.
4. Exiger les mesures de premier frame, frame-time de transition et CPU idle selon les seuils du contrat visuel.
5. Documenter dates d'expiration, alertes à 90 et 30 jours, renouvellement et procédure de révocation des certificats OS.
6. Tester le gate avec fixtures ; un vrai secret absent laisse un draft et n'empêche pas ce plan d'être implemented.

### 4) Fermer documentation et récupération

> Décrire honnêtement ce qui est implémenté et ce qui reste à qualifier.

1. Documenter 0.10.0 comme première installation manuelle compatible updater.
2. Documenter formats, FUSE, Python, données conservées, préparation du retrait et récupération.
3. Pour deb et AppImage, publier signature Minisign détachée, empreinte de clé et commande de vérification du premier téléchargement avant exécution.
4. Enregistrer architecture, design, déploiement et tests dans la mémoire projet.
5. Fournir la commande de qualification et les preuves attendues sans annoncer de release stable avant leur obtention.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Les workflows parsés contiennent les quatre cibles, Rust 1.93.0, des actions fixées par SHA, des permissions minimales et aucune exposition de secret aux PR externes. |
| 2 | Les fixtures de cinq artefacts produisent un manifeste exact ; clé production absente, empreinte divergente, updater désactivé ou autre incohérence empêche la transition draft vers publiée. |
| 3 | Chaque porte externe possède un acteur, une preuve, un état et un message ; son absence bloque uniquement la publication stable. |
| 4 | README, changelog, mémoire et dossier readiness distinguent sans ambiguïté implémentation, qualification et publication ; Linux documente une vérification Minisign reproductible avant la première exécution. |
