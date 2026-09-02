---
source: aidd_docs/tasks/2026_09/2026_09_02-native-ui-distribution-updater/plan.md
generated_at: 2026-09-02T16:07:29.266Z
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_09/2026_09_02-native-ui-distribution-updater/plan.md` et ses six phases liées
Generated: `2026-09-02T16:07:29.266Z`

Total gaps: 22 | Blocker: 6 | Major: 16 | Minor: 0

---

## Gaps by Category

### unstated assumption

**[blocker]** Can a phase be implementation-complete but validation-pending when its native hardware, human approval, or signing material is unavailable?
> Une phase atteint `done` seulement lorsque tous ses critères d'acceptation sont vérifiés, alors que plusieurs critères exigent des runners natifs, des machines réelles, une approbation visuelle ou des secrets de release.

### ambiguous term

**[major]** What adoption threshold and observation period make the key-rotation version sufficiently “measured” to switch to the new key?
> Signer les payloads suivants avec la nouvelle seulement après adoption mesurée de la version de chevauchement.

### missing edge case

**[blocker]** Which version becomes the first updater-enabled release when existing installations may already have a higher version than the current Cargo package?
> Employer `env!("CARGO_PKG_VERSION")` partout et refuser les tags/manifeste dont la version diverge.

**[major]** How does an existing user running an unpackaged binary migrate into NSIS, DMG, `.deb`, or AppImage without losing settings or creating a duplicate installation?
> Télécharger le paquet de son OS → Installer ou monter le paquet → Lancer DevToolBox depuis les applications.

**[major]** How does the window policy react when the user enables the operating system’s Reduce Transparency setting while DevToolBox is already running?
> Toute erreur d'API doit basculer vers `Opaque`, être journalisée et rester non bloquante.

**[major]** How does a client that skipped the key-overlap release regain automatic-update eligibility after releases start using only the new key?
> Cette version embarque les deux clés publiques. Signer les payloads suivants avec la nouvelle seulement après adoption mesurée de la version de chevauchement.

### missing actor

**[blocker]** Who has authority to approve `visual-contract.md` before view migration begins?
> faire approuver ce pivot avant de migrer les vues.

**[blocker]** Who procures, renews, stores, and revokes the Apple, Authenticode, and updater signing credentials?
> le propriétaire de release doit disposer de : Developer ID Application et identifiants de notarisation Apple, certificat Authenticode avec service d'horodatage, paire de clés updater conservée hors ligne.

**[major]** Who executes and signs off the native visual, installation, update, and uninstall matrix on the required physical hosts or VMs?
> Installer/désinstaller chaque format dans un compte ou VM jetable et vérifier raccourcis, données conservées, ressources et démarrage à la connexion.

### missing failure mode

**[major]** What recovery is offered when a verified update installs successfully but the new application fails to start or crashes before showing its first window?
> Confier l'installation au mécanisme compatible → Relancer ou guider l'utilisateur selon le format.

**[major]** How are disk exhaustion, power loss, or process termination handled between download verification and completed installation?
> un échec laisse le binaire courant et les données intacts.

**[major]** What happens to stable releases and installed clients when an Apple or Authenticode certificate expires or is revoked?
> Une release stable échoue fermée si une signature updater, la signature Authenticode horodatée, la signature/notarisation macOS [...] manque.

**[major]** How does background update checking respond to GitHub rate limiting, proxy authentication, captive portals, or persistently stale cached manifests?
> Utiliser l'asset `latest.json` de GitHub Releases comme endpoint stable, avec timeout, redirections bornées et taille maximale.

### missing acceptance criterion

**[major]** Which exact actions constitute the macOS “core functions” that must work rather than display an unavailable state?
> Le build macOS atteint l'écran principal, les actions coeur sont disponibles et chaque capacité non portée est nommée comme indisponible.

**[major]** What numeric contrast ratios, text sizes, focus visibility, and reduced-motion limits must every visual baseline satisfy?
> Définir palettes claire/sombre, contraste, typographie, rayons, espacements, ombres et durées de transition dans `theme.rs`.

**[major]** What startup-time, frame-time, memory, and animation-jank budgets define the requested fluid rendering on each supported machine class?
> Ajouter des transitions brèves via les animations egui existantes, sans boucle décorative permanente.

**[major]** What exact filesystem and OS-integration inventory must be absent after standard uninstall, “Préparer la désinstallation”, and “Supprimer mes données locales” respectively?
> les mécanismes de désinstallation retirent leurs fichiers, l'action de préparation retire les intégrations résiduelles et la suppression des données reste séparée et confirmée.

### missing dependency

**[blocker]** Which available macOS Ventura arm64/Intel, Windows 11, Ubuntu X11, and Ubuntu Wayland environments will execute the mandatory native tests?
> Tester visuellement macOS clair/sombre et vibrancy, Windows Mica/repli, Linux X11/Wayland opaque aux tailles et DPI de la matrice.

**[blocker]** Are the Apple Developer account, Developer ID certificate, notarization credentials, Authenticode certificate, timestamp service, and offline updater keys available before their dependent phases begin?
> Avant d'ouvrir la phase 6, le propriétaire de release doit disposer de [...].

**[major]** Which approved source artwork, icon master, font versions, and export process produce the required `.icns`, `.ico`, and PNG assets?
> Ajouter les icônes natives aux résolutions requises et vérifier leur rendu dans Finder, Explorer, menu Démarrer et lanceur Linux.

**[major]** Which GitHub repository, visibility, protected environment, release permissions, and runner entitlement host `latest.json` and the signed artifacts?
> Utiliser l'asset `latest.json` de GitHub Releases comme endpoint stable.

**[major]** Which pinned Rust toolchain, cargo-packager version, Python range, NSIS toolchain, Linux packaging tools, Xcode version, and notarization tooling define reproducible builds?
> Vérifier version Cargo/paquet, présence des ressources et absence de fichier sensible avant succès.
