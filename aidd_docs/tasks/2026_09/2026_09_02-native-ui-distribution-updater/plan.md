---
objective: "DevToolBox contient une interface soignée avec replis natifs, des paquets installables sur trois OS, un updater signé et une automatisation prête à être qualifiée puis publiée par les responsables de release."
status: in-progress
---

# Plan: Interface native et distribution qualifiable sur trois OS

## Overview

| Field      | Value |
| ---------- | ----- |
| **Goal**   | Implémenter et vérifier hors secrets le rendu, le port macOS, les paquets, l'updater et la chaîne de release, puis laisser la publication stable derrière des portes de qualification explicites. |
| **Source** | Replan du 2026-09-02 fondé sur le brainstorm initial et le rapport de zones d'ombre adjacent, après blocage de l'implémentation précédente sur matériel, approbations et secrets externes. |

## Phases

| #   | Phase | File |
| --- | ----- | ---- |
| 1 | Stabiliser la version, les contrats de plateforme et le port macOS | [phase-1.md](./phase-1.md) |
| 2 | Construire le contrat visuel vérifiable et la nouvelle coque egui | [phase-2.md](./phase-2.md) |
| 3 | Ajouter les matériaux natifs avec un repli opaque testable | [phase-3.md](./phase-3.md) |
| 4 | Configurer les paquets, ressources et parcours de désinstallation | [phase-4.md](./phase-4.md) |
| 5 | Implémenter l'updater signé et ses chemins de récupération | [phase-5.md](./phase-5.md) |
| 6 | Livrer la CI, les portes de release et le dossier de qualification | [phase-6.md](./phase-6.md) |

## Resources

| Source | Verified |
| ------ | -------- |
| https://docs.rs/eframe/0.35.0/eframe/struct.NativeOptions.html | Le hook de construction de fenêtre permet de conserver eframe tout en ajoutant les attributs natifs. |
| https://docs.rs/winit/latest/winit/platform/macos/trait.WindowAttributesExtMacOS.html | winit expose le titre transparent et le contenu pleine hauteur sur macOS. |
| https://docs.rs/winit/latest/winit/platform/windows/trait.WindowExtWindows.html | Le backdrop système Windows peut être demandé sans remplacer le chrome DWM. |
| https://docs.rs/window-vibrancy/latest/window_vibrancy/ | La vibrancy macOS et le fallback sur erreur peuvent être isolés derrière une politique de fenêtre. |
| https://docs.rs/cargo-packager/latest/cargo_packager/enum.PackageFormat.html | cargo-packager produit app/DMG, NSIS, deb et AppImage. |
| https://docs.rs/crate/cargo-packager-updater/0.2.3 | L'updater accepte app, NSIS/WiX et AppImage, mais pas deb. |
| https://docs.github.com/en/actions/reference/runners/github-hosted-runners | Les labels de runners et leurs architectures doivent être épinglés puis contrôlés au début des jobs. |
| https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution | La notarisation directe exige Developer ID, Hardened Runtime, horodatage et ticket agrafé. |

## Decisions

| Decision | Why |
| -------- | --- |
| Séparer « implémenté » de « qualifié pour publication » : ce plan se termine quand le code, les tests hors secrets, les paquets configurés et les workflows sont prêts ; la release stable reste bloquée par un environnement GitHub protégé. | Le code peut être terminé sans prétendre qu'un Mac réel, un certificat ou une approbation humaine ont été observés par l'exécuteur. |
| Faire de 0.10.0 la première version installable avec updater, après les tags existants jusqu'à v0.9.1 ; l'installation initiale depuis une ancienne version ou un binaire brut est manuelle. | Cela supprime le risque de downgrade créé par le 0.1.0 actuel et donne une origine claire au protocole de mise à jour. |
| Conserver eframe/egui et introduire un design system commun, puis vibrancy sur macOS, Mica sur Windows 11 et un fond opaque partout ailleurs. | Le coeur Rust actuel reste intact et l'effet cosmétique ne conditionne jamais la lisibilité. |
| Certifier macOS 13+ arm64/Intel, Windows 11 23H2+ x64 et Ubuntu 22.04/24.04 x64 X11/Wayland ; Windows 10 reste best effort opaque. | La matrice est assez petite pour être qualifiée et couvre les cibles demandées sans promettre toutes les distributions. |
| Publier deux DMG, un NSIS, un deb et un AppImage avec cargo-packager ; deb délègue les mises à jour au gestionnaire de paquets. | Chaque OS possède un parcours reconnu d'installation et de retrait, tandis que l'AppImage fournit l'auto-update Linux. |
| Construire et tester l'updater avec des clés fixtures ; sans trousseau public de production injecté au build, l'UI indique que les mises à jour sont indisponibles. | Aucune clé privée ni fausse clé « production » n'est créée pour terminer l'implémentation. |
| Pour la rotation, publier des signatures ancien/nouveau pendant deux versions mineures et au moins 180 jours ; un client ayant manqué cette fenêtre passe par une réinstallation manuelle signée par l'OS. | Le seuil est déterministe et ne dépend d'aucune télémétrie absente. |
| Le propriétaire du dépôt approuve le rendu au moment de la qualification ; le mainteneur de release garde les certificats et clés ; l'opérateur QA exécute la matrice native. | Chaque action externe a un acteur, sans transformer son intervention en critère de fin du code. |
| Conserver les données utilisateur lors de la désinstallation standard et séparer le nettoyage des intégrations de la suppression confirmée des données. | Une désinstallation ou mise à jour ne doit jamais effacer silencieusement les préférences. |
| Préparer l'implémentation v2 depuis un commit snapshot synthétique contenant uniquement les fichiers suivis du working tree actuel, créé par plomberie Git sur une nouvelle branche sans modifier, stasher ni committer le workspace principal ; comparer son tree au tree indexé attendu avant de commencer, puis copier le plan séparément. | Les changements utilisateur actuels font partie de l'état fonctionnel à préserver, recouvrent plusieurs fichiers du plan et ne doivent ni disparaître ni être attribués à la phase 1 ; les fichiers non suivis et secrets potentiels restent exclus. |
| Générer les icônes par un outil Rust séparé, verrouillé par son propre manifest et lockfile. | Les formats binaires restent reproductibles sans ajouter de dépendance de rendu à l'application finale. |
| Une build stable doit embarquer un trousseau public fourni par l'environnement protégé et publier ses empreintes ; une build sans trousseau reste non publiable. | L'updater désactivé est acceptable en développement, jamais dans un artefact stable. |
