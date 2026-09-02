# DevToolBox - Lanceur d'actions personnalisées (macOS / Windows / Linux)

Une application Rust minimaliste pour centraliser et lancer des commandes, scripts et
actions personnalisées, avec une interface graphique native cross-plateforme
(`eframe`/`egui`) - pas de WebView, pas de bloatware.

## Actions de script

Les commandes classiques restent acceptées telles quelles. Une action Python intégrée
utilise le préfixe `@python` et un chemin relatif à la racine interne de DevToolBox :

```text
@python scripts/sftp_fetch/sftp_fetch.py config.yaml --only pro
```

DevToolBox utilise en priorité `.venv/Scripts/python.exe` (Windows) ou `.venv/bin/python`
(Linux) à côté du script, puis la variable `DEVTOOLBOX_PYTHON`, et enfin `python3`
disponible dans le système. Le script est exécuté depuis son propre dossier, ce qui rend
ses fichiers de configuration relatifs portables.

Un dossier distinct pour les scripts de l'utilisateur peut être sélectionné avec
**Préférences → Actions et scripts → Parcourir…**. Une action telle que
`@python sauvegarde.py` est alors résolue dans ce dossier. Les chemins absolus restent
acceptés. Ce réglage ne modifie jamais la racine des outils intégrés ni
`DEVTOOLBOX_HOME`, qui reste géré par le programme.

Le bouton **Scanner** parcourt récursivement cette bibliothèque, ignore les marqueurs
de paquet et les environnements cachés, puis présente les scripts Python comme des
propositions sélectionnables. Leur ajout explicite crée les actions dans la catégorie
« Scripts utilisateur ».

## 🎯 Objectifs

- Lancer des commandes CLI facilement via des boutons/icônes personnalisables
- Gestion d'alias et de favoris
- Lancement automatique au démarrage (Registry Run-key sous Windows, `.desktop`
  autostart XDG sous Linux)
- Interface native cross-plateforme (`eframe`/`egui`)

## 🚀 Fonctionnalités

- [x] Grille de cartes, favoris, catégories, recherche
- [x] Icônes personnalisées PNG/JPEG/BMP/GIF (SVG en suivi, décision D1)
- [x] Stockage JSON pour commandes
- [x] Lancement au démarrage (Windows Registry / Linux XDG autostart)
- [x] Vue Automations (Tâches planifiées Windows / unités `systemd --user` sous Linux)
- [x] Vue Terminal intégrée
- [x] Vue Applications : rapport consultatif multi-OS classé par empreinte et
  usage local couvert, avec explications, protections et copie manuelle de commande

La vue Applications collecte uniquement des métadonnées locales. Sous Linux, elle
couvre APT/dpkg, Snap et Flatpak ; sous Windows, Uninstall Registry, MSIX/AppX,
Scoop et Chocolatey. Le score indique une priorité d'examen et la confiance reste
affichée séparément. L'empreinte installée n'est pas une promesse de gain :
l'estimation récupérable est présentée à part lorsqu'elle existe. Le suivi d'usage
commence à partir de l'activation de la fonctionnalité et compte seulement les jours
où au moins un échantillon a réussi, pas tous les jours calendaires écoulés.

DevToolBox ne désinstalle aucune application. Une suggestion peut uniquement être
copiée dans le presse-papiers pour relecture et exécution manuelle. Voir le
[contrat et les limites du rapport](scripts/app_recommendations/README.md).

## 🛠️ Stack Technique

- **Langage** : Rust
- **UI Framework** : `eframe`/`egui` (remplace l'ancienne pile `tao` + WinUI 3, abandonnée
  lors de la transformation multi-OS)
- **Stockage** : JSON (serde)
- **Windows APIs** : Registry, Task Scheduler, Process (`windows` crate, `cfg(windows)`)
- **Linux APIs** : XDG Base Directory spec, `.desktop` autostart, `systemd --user`
  (`cfg(target_os = "linux")`)

## 📁 Structure du projet

```
DevToolBox/
├── src/
│   ├── main.rs               # Entry point (eframe::run_native)
│   ├── platform/              # Abstraction OS-neutre (chemins config/data/state,
│   │   ├── mod.rs              # trait StartupProvider), dispatch cfg(windows) / linux
│   │   ├── windows.rs
│   │   └── linux.rs
│   ├── windows/                # Intégration Windows
│   │   ├── registry.rs         # Registry Run keys
│   │   └── process.rs          # Command executor
│   ├── linux/                  # Intégration Linux
│   │   ├── autostart.rs        # XDG .desktop autostart
│   │   ├── automations.rs      # Unités systemd --user
│   │   └── icon_theme.rs       # Résolution icônes via le thème freedesktop
│   ├── icons/                  # Décodage/rendu d'icônes (backend egui)
│   ├── applications/           # Suivi local d'usage et collecte de processus
│   ├── python_runtime.rs       # Résolution partagée des modules Python livrés
│   ├── ui/                     # UI egui (grille, terminal, applications)
│   │   ├── egui_app.rs
│   │   └── applications_view.rs
│   └── storage/                 # Persistance données (Command, Category, Settings)
│       └── json.rs
├── scripts/app_recommendations/ # Collecte/scoring Python multi-OS en lecture seule
├── Cargo.toml
├── README.md
└── config/
    └── default.json
```

## 📦 Installation

Les releases GitHub fournissent cinq artefacts : deux DMG (Apple Silicon et Intel),
un installeur NSIS x64, un paquet deb x64 et une AppImage x64. La version 0.10.0 est
la première base compatible avec les mises à jour intégrées ; depuis 0.9.x ou un
binaire brut, installez-la manuellement.

- macOS 13+ : ouvrir le DMG correspondant à l'architecture et déplacer DevToolBox
  dans Applications. La vibrancy est native ; elle devient automatiquement opaque
  si l'API ou l'accessibilité l'exige.
- Windows 11 x64 : exécuter le NSIS par utilisateur. La désinstallation passe par
  Applications installées et conserve les préférences.
- Ubuntu 22.04/24.04 x64 : installer le deb avec le gestionnaire de paquets, ou
  rendre l'AppImage exécutable. Selon l'hôte, l'AppImage peut nécessiter FUSE 2.

Avant un premier lancement Linux, télécharger l'asset, son `.minisig`, puis vérifier :

```sh
minisign -Vm DevToolBox_0.10.0_linux_x86_64.AppImage \
  -P '<clé publique et empreinte publiées dans la release>'
```

Le deb se met à jour via le gestionnaire de paquets. NSIS et AppImage proposent la
mise à jour dans **Préférences → Général** après un clic explicite ; macOS télécharge
et vérifie les archives avant le handoff d'installation. Les téléchargements sont
acceptés uniquement depuis un tag immuable du dépôt et après SHA-256 + signature
Ed25519. Sans trousseau de production, l'updater reste visiblement désactivé.

La désinstallation standard conserve configuration, historique et scripts utilisateur.
Le bouton **Préparer la désinstallation** retire d'abord l'autostart et les fichiers
temporaires. La suppression des données est une opération séparée et confirmée :
`devtoolbox --delete-user-data --confirm-delete-data`. Python 3.10 à 3.13 reste requis
pour les outils Python embarqués ; un diagnostic est disponible dans les préférences.

En cas d'update interrompue, l'AppImage conserve une copie `.previous`; NSIS et macOS
exigent un payload de la version courante, signé et vérifié avant toute installation.
Sinon, DevToolBox refuse l'auto-installation et indique la réinstallation manuelle.
Voir [le dossier de qualification](docs/release-readiness.md).

### Prérequis communs
- Rust toolchain (edition 2021)

### macOS
- macOS 13 Ventura ou plus récent
- DMG `aarch64` pour Apple Silicon, `x86_64` pour Mac Intel

### Windows
- Windows 11 (22H2+)
- Visual Studio 2022 avec C++ build tools
- Windows SDK (10.0.22621.0+)

### Linux (Ubuntu/Debian - autres distributions non testées)
- Bibliothèques système requises par le backend `eframe`/`winit` :
  ```bash
  sudo apt-get install libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev \
      libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
  ```

### Build
```bash
cargo build --release
```

## 📜 License

MIT License
