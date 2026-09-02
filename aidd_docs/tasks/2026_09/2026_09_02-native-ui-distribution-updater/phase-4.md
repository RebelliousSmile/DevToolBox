---
status: done
---

# Instruction: Configurer les paquets, ressources et parcours de désinstallation

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

~~~txt
.
├── Cargo.toml                         ✏️ ajouter métadonnées et suite cargo-packager épinglée
├── Cargo.lock                         ✏️ verrouiller le resolver
├── packager.toml                      ✅ configurer DMG NSIS deb et AppImage
├── THIRD_PARTY_LICENSES.md            ✅ inventorier les ressources redistribuées
├── assets
│   └── app-icon
│       ├── devtoolbox.icns            ✅ icône bundle macOS
│       ├── devtoolbox.ico             ✅ icône Windows
│       └── devtoolbox.png             ✅ icône Linux
├── packaging
│   ├── linux
│   │   └── devtoolbox.desktop         ✅ entrée de bureau
│   └── macos
│       └── entitlements.plist         ✅ capacités minimales
├── scripts
│   ├── package.ps1                    ✅ construire et inspecter sous Windows
│   ├── package.sh                     ✅ construire et inspecter sous Unix
│   └── verify-package-config.py       ✅ valider la matrice sans outil natif
├── tools
│   └── icon-generator
│       ├── Cargo.toml                 ✅ épingler resvg ico et icns
│       ├── Cargo.lock                 ✅ verrouiller le générateur
│       └── src
│           └── main.rs                ✅ produire PNG ICO et ICNS depuis SVG
└── src
    ├── icons
    │   └── resolve.rs                 ✏️ résoudre les ressources installées
    ├── python_runtime.rs              ✏️ diagnostiquer Python local et versions
    ├── storage
    │   └── json.rs                    ✏️ charger les configurations en lecture seule
    └── uninstall.rs                   ✅ inventorier et nettoyer intégrations/données
~~~

## User Journey

~~~mermaid
flowchart TD
  A[Télécharger le format de son OS] --> B[Installer monter ou rendre exécutable]
  B --> C[Lancer hors du dépôt]
  C --> D[Retrouver ressources et données utilisateur]
  D --> E[Préparer la désinstallation]
  E --> F[Retirer le programme]
  F --> G[Conserver ou supprimer séparément les données]
~~~

## Test Scope

~~~mermaid
---
title: Test scope
---
journey
  section Setup
    Générer une arborescence installée fixture => ressources et données séparées: 5: cli
  section Happy path
    Valider Packager toml => cinq formats architectures et ressources attendus: 5: cli
    Résoudre depuis la fixture installée => configuration icônes et scripts retrouvés: 5: cli
    Préparer la désinstallation => seules les intégrations DevToolBox retirées: 5: cli
  section Edge case - dépendance absente
    Simuler Python ou FUSE absent => diagnostic actionnable sans téléchargement: 1: cli
  section Edge case - chemin hostile
    Injecter symlink ou fichier hors racine => suppression refusée: 1: cli
  section Teardown
    Supprimer la fixture => compte réel inchangé: 5: cli
~~~

## Tasks to do

### 1) Fixer identité et ressources

> Produire les mêmes métadonnées sur les trois OS.

1. Définir com.rebellioussmile.devtoolbox, version issue de Cargo, éditeur, licence et catégories.
2. Générer les icônes natives depuis le SVG source avec tools/icon-generator ; vérifier dimensions, transparence, empreintes et reproductibilité bit à bit sur deux exécutions.
3. Verrouiller les versions exactes dans tools/icon-generator/Cargo.lock, exécuter ensuite avec --locked et faire échouer la vérification si le lockfile change.
4. Inventorier source, version, empreinte et licence de chaque ressource.
5. Épingler cargo-packager et cargo-packager-resource-resolver à des versions compatibles ; conserver le même couple dans scripts locaux et CI.

### 2) Résoudre l'installation hors dépôt

> Séparer ressources immuables et données mutables.

1. Employer cargo-packager-resource-resolver, puis DEVTOOLBOX_HOME et le dépôt comme replis de développement.
2. Ne jamais écrire dans le bundle ; conserver configuration, logs et données dans platform.
3. Définir la plage Python supportée et diagnostiquer interpréteur absent ou incompatible.

### 3) Définir installation et retrait

> Rendre chaque format prévisible pour l'utilisateur final.

1. Configurer NSIS par utilisateur, deux DMG, deb et AppImage.
2. Déclarer les dépendances deb à partir de l'inspection du binaire et documenter AppImage avec ou sans FUSE.
3. Ajouter Préparer la désinstallation pour autostart et temporaires, puis une suppression de données distincte avec inventaire et confirmation.
4. Refuser toute suppression hors des racines DevToolBox après résolution des liens.
5. Détecter une autre installation DevToolBox par identifiant et chemin ; proposer remplacement ou annulation, sans maintenir silencieusement deux formats actifs.

### 4) Valider sans prétendre qualifier

> Prouver la configuration sur l'hôte disponible et préparer les autres.

1. Valider statiquement les cinq formats, noms, architectures, ressources et exclusions.
2. Construire le format natif de l'hôte courant lorsque son outil est disponible.
3. Produire les mêmes commandes pour CI sans faire du paquet non exécuté localement un critère de fin.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Les cinq formats dérivent identité, version et icônes de sources uniques ; cargo --locked laisse les lockfiles inchangés, la suite packager résout une seule fois, deux générations d'icônes reproduisent les mêmes empreintes et toutes les ressources ont une licence inventoriée. |
| 2 | Une fixture lancée hors dépôt retrouve toutes les ressources et n'écrit que dans les racines de données. |
| 3 | L'inventaire énumère chaque chemin programme, intégration, temporaire et donnée ; symlinks, chemins externes, installation concurrente et absence de dépendance restent sûrs. |
| 4 | Le validateur statique et le paquet natif disponible passent ; les quatre autres jobs sont décrits comme qualification CI à exécuter. |
