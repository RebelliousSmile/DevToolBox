# 🗺️ Roadmap WinFXStart v0.1.0

Cette roadmap détaille le développement de WinFXStart, un launcher de commandes CLI pour Windows 11 avec interface native WinUI 3.

## 📋 Objectifs principaux

- ✅ Application Rust minimaliste et performante
- ✅ Interface graphique native Windows 11 (WinUI 3)
- ✅ Lancement de commandes CLI via boutons/icônes personnalisables
- ✅ Gestion d'alias, catégories et favoris
- ✅ Lancement automatique au démarrage de Windows
- ✅ Léger, rapide, pas de WebView ou bloatware

---

## 🎯 Phase 1 : MVP (2-3 semaines)

### Objectifs
Fondations techniques et fonctionnalités de base.

### Tasks
- [x] Structure projet Rust + winit
- [ ] Intégration WinUI 3 XAML avec Tao/winit
- [ ] Command executor simple (lancement processus)
- [ ] Stockage JSON pour commandes et config
- [ ] Lancement au démarrage via Registry Run Keys

### Livrables
- Application compilable en release
- Interface basique fonctionnelle
- Persistance des données JSON

---

## 🎨 Phase 2 : Personnalisation (1-2 semaines)

### Objectifs
Améliorer l'expérience utilisateur avec personnalisation.

### Tasks
- [ ] Gestion icônes personnalisées PNG/SVG (~256x256px)
- [ ] Système de catégories et groupes
- [ ] Grille visuelle des favoris
- [ ] Barre de recherche de commandes
- [ ] Éditeur inline d'alias (commande + raccourci)

### Livrables
- Interface riche avec icônes personnalisables
- Organisation logique des commandes
- Recherche rapide

---

## ✨ Phase 3 : UX/Polish (1 semaine)

### Objectifs
Améliorer l'expérience utilisateur et la fluidité.

### Tasks
- [ ] Animations et transitions natives WinUI 3
- [ ] Feedback visuel (succès/échec, durée d'exécution)
- [ ] Raccourcis clavier personnalisables
- [ ] Thèmes (lumière/sombre)
- [ ] Logs optionnels configurables

### Livrables
- Application fluide et réactive
- Expérience utilisateur polie
- Feedback clair à l'utilisateur

---

## 🚀 Phase 4 : Fonctionnalités Avancées (Optionnel)

### Objectifs
Étendre les capacités de l'application.

### Tasks
- [ ] Logs et débogage détaillés
- [ ] Scripts PowerShell personnalisés
- [ ] Templates de commandes avec variables
- [ ] Export/Import de configuration
- [ ] Intégration avec Windows Terminal
- [ ] Mode "batch mode" (exécution silencieuse)

### Livrables
- Fonctionnalités avancées pour utilisateurs experts
- Flexibilité maximale

---

## 🛠️ Stack Technique

| Composant | Choix | Justification |
|-----------|-------|---------------|
| Langage | Rust | Performance, sécurité, mémoire |
| UI Framework | WinUI 3 | Intégration native Windows 11 |
| Fenêtre | Tao/winit | Abstraction légère |
| Stockage | JSON (serde) | Simple, lisible, portable |
| Windows APIs | Registry, Task Scheduler | Fiables, natifs |

---

## 📊 Métriques de performance cibles

- Taille binaire : < 20 MB
- Démarrage : < 3 secondes
- Rendu UI : 60 FPS (GPU)
- Mémoire : < 100 MB
- Exécution commande : < 50ms overhead

---

## 📝 Notes de version

### v0.1.0 (En cours)
- MVP avec WinUI 3
- Gestion de base des commandes
- Lancement au démarrage

### v0.2.0 (Planifié)
- Personnalisation complète
- Recherche et favoris
- UX polish

### v0.3.0 (À venir)
- Fonctionnalités avancées
- Scripts personnalisés
- Templates

---

**Maintenu par :** RebelliousSmile  
**Dernière mise à jour :** 17 avril 2026
