# WinFXStart - Windows 11 Command Launcher

Une application Rust minimaliste pour lancer des commandes CLI Windows avec une interface graphique native WinUI 3.

## 🎯 Objectifs

- Lancer des commandes CLI facilement via des boutons/icônes personnalisables
- Gestion d'alias et de favoris
- Lancement automatique au démarrage de Windows 11
- Interface native Windows 11 (WinUI 3) - pas de WebView, pas de bloatware

## 🚀 Fonctionnalités

### Phase 1 : MVP (2-3 semaines)
- [x] Structure projet Rust + winit
- [ ] Intégration WinUI 3 XAML
- [ ] Command executor simple
- [ ] Stockage JSON pour commandes
- [ ] Lancement au démarrage via Registry

### Phase 2 : Personnalisation (1-2 semaines)
- [ ] Gestion icônes personnalisées PNG/SVG
- [ ] Catégories et groupes
- [ ] Favoris avec grille visuelle
- [ ] Recherche de commandes

### Phase 3 : UX/Polish (1 semaine)
- [ ] Animations et transitions natives
- [ ] Feedback visuel (succès/erreur)
- [ ] Raccourcis clavier personnalisables
- [ ] Thèmes (lumière/sombre)

### Phase 4 : Avancé (Optionnel)
- [ ] Logs et débogage configurables
- [ ] Scripts PowerShell personnalisés
- [ ] Templates de commandes avec variables
- [ ] Export/Import de configuration

## 🛠️ Stack Technique

- **Langage** : Rust
- **UI Framework** : WinUI 3 (Microsoft UI Library)
- **Fenêtre** : winit / Tao
- **Stockage** : JSON (serde)
- **Windows APIs** : Registry, Task Scheduler, Process

## 📁 Structure du projet

```
WinFXStart/
├── src/
│   ├── main.rs              # Entry point
│   ├── windows/             # Windows integration
│   │   ├── registry.rs      # Registry Run Keys
│   │   ├── task_scheduler.rs # Task planifiée
│   │   └── process.rs       # Command executor
│   ├── ui/                  # UI XAML + Rust bindings
│   │   ├── app.rs           # Application state
│   │   └── xaml_gen.rs      # Génération dynamique
│   ├── storage/             # Persistance données
│   │   ├── models.rs        # Command, Category, Settings
│   │   └── json.rs          # Load/save JSON
│   └── assets/              # Icônes personnalisées
├── Cargo.toml
├── README.md
└── config/
    └── default.json
```

## 📦 Installation

### Prérequis
- Windows 11 (22H2+)
- Visual Studio 2022 avec C++ build tools
- Rust toolchain (nightly recommandé)
- Windows SDK (10.0.22621.0+)

### Build
```bash
cargo build --release
```

## 📜 License

MIT License
