# 📋 Issue Template - Roadmap DevToolBox

## 🎯 Contexte

Cette issue documente la roadmap de développement de DevToolBox, un launcher de commandes CLI pour Windows 11 avec interface native WinUI 3.

## 📊 État actuel

- **Version** : v0.1.0 (MVP)
- **Statut** : En développement
- **Priorité** : Haute

## 🗺️ Roadmap détaillée

### Phase 1 : MVP (2-3 semaines)
- [x] Structure projet Rust + winit
- [ ] Intégration WinUI 3 XAML avec Tao/winit
- [ ] Command executor simple
- [ ] Stockage JSON pour commandes
- [ ] Lancement au démarrage via Registry

### Phase 2 : Personnalisation (1-2 semaines)
- [ ] Gestion icônes personnalisées PNG/SVG
- [ ] Système de catégories et groupes
- [ ] Grille visuelle des favoris
- [ ] Barre de recherche
- [ ] Éditeur inline d'alias

### Phase 3 : UX/Polish (1 semaine)
- [ ] Animations et transitions WinUI 3
- [ ] Feedback visuel (succès/échec)
- [ ] Raccourcis clavier personnalisables
- [ ] Thèmes lumière/sombre
- [ ] Logs optionnels

### Phase 4 : Avancé (Optionnel)
- [ ] Logs et débogage détaillés
- [ ] Scripts PowerShell personnalisés
- [ ] Templates de commandes avec variables
- [ ] Export/Import de configuration

## 🛠️ Stack technique

- **Langage** : Rust
- **UI** : WinUI 3 (Microsoft UI Library)
- **Fenêtre** : Tao/winit
- **Stockage** : JSON (serde)
- **Windows APIs** : Registry, Task Scheduler, Process

## 📊 Métriques cibles

| Métrique | Cible |
|----------|-------|
| Taille binaire | < 20 MB |
| Démarrage | < 3 secondes |
| Rendu UI | 60 FPS (GPU) |
| Mémoire | < 100 MB |
| Overhead exécution | < 50ms |

## 📝 Questions pour la communauté

1. **Framework XAML** : Templates Microsoft UI Library ou contrôles personnalisés ?
2. **Gestion des icônes** : Fichiers PNG externes ou génération dynamique ?
3. **Interface principale** : Grille de favoris en premier plan ou liste avec catégories ?
4. **Lancement au démarrage** : Registry Run Keys ou Task Scheduler (ou les deux) ?

## 📚 Ressources

- [Microsoft WinUI 3 Documentation](https://docs.microsoft.com/en-us/windows/apps/winui/)
- [Tao Rust Framework](https://github.com/Ice1000/tao)
- [winit](https://github.com/rust-windowing/winit)

---

**Créé le :** 17 avril 2026  
**Maintenu par :** RebelliousSmile
