---
objective: "Le paquet .deb et l'AppImage 0.10.0 sont construits, installés/exécutés réellement sur cette machine Ubuntu 22.04 X11, leur comportement hors arbre de dev est vérifié et corrigé si besoin, et la preuve datée est consignée dans docs/release-readiness.md."
status: in-progress
---

<!-- Fill or omit these sections; never add, rename, or reorder one. -->

# Plan: Qualification locale Linux (.deb + AppImage)

## Overview

| Field      | Value                   |
| ---------- | ----------------------- |
| **Goal**   | Construire et installer réellement le `.deb` et l'AppImage 0.10.0 sur cette machine Ubuntu 22.04 X11, vérifier que l'app fonctionne hors de l'arbre de développement (résolution des ressources, intégration bureau, données XDG), corriger ce qui casse, et documenter une preuve datée dans `docs/release-readiness.md` sur le modèle de la qualification Windows du 2 septembre 2026. |
| **Source** | Brainstorm en conversation (`/aidd-refine:01-brainstorm`, non persisté) — décision finale : « Qualification locale, comme Windows hier », avec Minisign, activation réelle de l'updater, Wayland et Ubuntu 24.04 explicitement différés. |

## Phases

| #   | Phase                              | File                          |
| --- | ----------------------------------- | ------------------------------ |
| 1   | Outillage et build des paquets      | [`phase-1.md`](./phase-1.md)  |
| 2   | Qualification du paquet .deb        | [`phase-2.md`](./phase-2.md)  |
| 3   | Qualification de l'AppImage         | [`phase-3.md`](./phase-3.md)  |
| 4   | Documentation de la preuve datée    | [`phase-4.md`](./phase-4.md)  |

## Resources

<!-- External sources only (URLs, docs), not code files. Omit if none consulted. -->

| Source | Verified          |
| ------ | ----------------- |
| `cargo search cargo-packager` (crates.io) | `cargo-packager 0.11.8` existe et correspond à la version épinglée dans `scripts/package.sh` — installable via `cargo install cargo-packager --version 0.11.8 --locked`. |
| `~/.cargo/registry/.../cargo-packager-resource-resolver-0.1.2/src/lib.rs` | `resources_dir()` résout `PackageFormat::Deb`/`Pacman` vers `/usr/lib/<exe_name>/`, et `PackageFormat::AppImage` vers `{$APPDIR}/usr/lib/<exe_name>/` avec un contrôle de sécurité exigeant que `current_exe()` soit sous un montage `.mount_` — c'est la convention de chemin que `packager.toml` doit satisfaire pour que `scripts/`/`config/` soient trouvés après installation réelle. |

## Decisions

<!-- Architecture-magnitude only, one you'd regret reversing. Omit if none qualify. -->

| Decision   | Why   |
| ---------- | ----- |
| Le périmètre exclut Minisign, l'activation réelle de l'updater, Wayland et Ubuntu 24.04. | Aucune release GitHub n'existe encore pour tester une vraie mise à jour ; la machine est en X11 sous 22.04 uniquement. Ce travail est reporté à une tâche séparée : première release publiée. |
| Aucun changement de code n'est présupposé avant l'installation réelle. | `src/python_runtime.rs::action_root()` priorise déjà la racine de ressources empaquetée quand elle contient `scripts/`, et c'est déjà testé génériquement — un correctif n'est justifié que si l'installation réelle prouve un écart avec la convention `/usr/lib/<exe_name>/` du resolver. |
| Installer le `.deb` via `sudo apt install ./devtoolbox_*.deb` plutôt que `dpkg -i`. | `dpkg -i` ne résout pas les dépendances déclarées (`libc6`, `libx11-6`, `libwayland-client0`) et laisse le paquet « unconfigured » si l'une manque ; `apt install` sur un chemin de fichier local résout et installe les dépendances manquantes en une seule commande. Repli documenté en phase 2 : `sudo dpkg -i` puis `sudo apt --fix-broken install` si `apt` n'a pas accès au réseau. |
| Un écart bloquant qui n'est pas une question de résolution de ressources (crash, rendu cassé, régression fonctionnelle) fait passer le plan en `status: blocked` plutôt que d'être corrigé dans les tâches 3 des phases 2/3. | Les tâches 3 des phases 2/3 ont un périmètre de correctif volontairement étroit (uniquement `packager.toml`/`python_runtime.rs`, sur preuve d'un écart réel) ; un bug d'une autre nature dépasse ce périmètre et mérite sa propre tâche plutôt qu'un correctif anticipé et non cadré ici. |
