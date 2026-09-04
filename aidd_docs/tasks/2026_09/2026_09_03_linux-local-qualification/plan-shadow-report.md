---
source: aidd_docs/tasks/2026_09/2026_09_03_linux-local-qualification/plan.md
generated_at: 2026-09-03
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_09/2026_09_03_linux-local-qualification/plan.md`
Generated: `2026-09-03`

Total gaps: 10 | Blocker: 1 | Major: 7 | Minor: 2

---

## Gaps by Category

### unstated assumption

**[major]** Is sudo/root access confirmed available on this machine before phase 2 runs?
> sudo dpkg -i dist/devtoolbox_0.10.0_amd64.deb

### ambiguous term

**[minor]** What minimum free disk space, in GB, counts as sufficient before building both packages?
> Vérifier l'espace disque libre avant build => marge suffisante confirmée: 5: system

**[minor]** Which specific @python action should be used to prove the packaged scripts/ path resolves correctly?
> Déclencher une action @python (ex. rapport d'applications ou orchestrateur de modèles) depuis une carte et confirmer un résultat exploitable dans l'UI

### missing edge case

**[major]** Does the custom font declared under packager.toml's `assets/fonts` resource actually render after a real .deb/AppImage install, rather than silently falling back to a system font?

### missing failure mode

**[major]** What is the recovery step when dpkg -i fails on an unmet dependency — apt --fix-broken install, or reinstalling via apt install ./file.deb?
> Vérifier les dépendances déclarées (libc6 (>= 2.35), libx11-6, libwayland-client0) sont satisfaites (dpkg -s sur chacune)

**[major]** What happens to the plan if cargo packager produces the .deb but fails to produce the AppImage, or vice versa?
> dist/ contient un .deb x64 et un .AppImage x64 issus de la version 0.10.0, sans erreur de verify-package-config.py ni de cargo packager.

**[major]** Does the plan move to status: blocked if qualification surfaces a blocking bug outside resource resolution (e.g. a crash or rendering failure), or is a new task opened?
> N'agir que si la tâche 1 ou 2 révèle un écart réel — ne pas anticiper de correctif.

### missing acceptance criterion

**[major]** Should the corrective task in phase 2/3 require cargo fmt, clippy, and test to pass before the rebuilt package is reinstalled?
> Si scripts/config ne sont pas sous /usr/lib/devtoolbox/, ajuster les cibles resources de packager.toml

**[major]** Should phase 4 explicitly require both a light-theme and a dark-theme screenshot, matching the Windows qualification's two captures?
> Lier les captures et sorties de evidence/ (menu GNOME, exécution AppImage, layout dpkg, montage FUSE)

### missing dependency

**[blocker]** Is the AppImage build-time toolchain (squashfs-tools/mksquashfs, or cargo-packager's bundled appimagetool/linuxdeploy download) confirmed available on this machine before phase 1 attempts the AppImage build?
> cargo install cargo-packager --version 0.11.8 --locked
