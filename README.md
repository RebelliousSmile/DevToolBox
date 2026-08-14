# DevToolBox - Lanceur d'actions personnalisées (Windows / Linux)

Une application Rust minimaliste pour centraliser et lancer des commandes, scripts et
actions personnalisées, avec une interface graphique native cross-plateforme
(`eframe`/`egui`) - pas de WebView, pas de bloatware.

## Actions de script

Les commandes classiques restent acceptées telles quelles. Une action Python utilise
le préfixe `@python` et un chemin relatif à la racine DevToolBox :

```text
@python scripts/sftp_fetch/sftp_fetch.py config.yaml --only pro
```

DevToolBox utilise en priorité `.venv/Scripts/python.exe` (Windows) ou `.venv/bin/python`
(Linux) à côté du script, puis la variable `DEVTOOLBOX_PYTHON`, et enfin `python3`
disponible dans le système. Le script est exécuté depuis son propre dossier, ce qui rend
ses fichiers de configuration relatifs portables.

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
│   ├── ui/                     # UI egui (grille de cartes, dialogs, terminal)
│   │   └── egui_app.rs
│   └── storage/                 # Persistance données (Command, Category, Settings)
│       └── json.rs
├── Cargo.toml
├── README.md
└── config/
    └── default.json
```

## 📦 Installation

### Prérequis communs
- Rust toolchain (edition 2021)

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
