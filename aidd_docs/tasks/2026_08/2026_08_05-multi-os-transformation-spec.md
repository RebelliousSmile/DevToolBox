---
name: multi_os_transformation_spec
status: validated
description: Requête validée (aidd-refine:01-brainstorm) — transformer DevToolBox d'un projet Windows-only en projet multi-OS (Windows + Linux)
argument-hint: N/A
---

# Requête validée : DevToolBox multi-OS (Windows + Linux)

> Sortie de `aidd-refine:01-brainstorm` (action `04-refine-and-validate`, approuvée via `05-confirm-approval`). Ce document persiste cette requête sur disque afin de servir de source à `aidd-refine:04-shadow-areas`.

## Périmètre

- Transformation couvre **tout le dépôt**, y compris les scripts Python `scripts/system_inventory/` et `scripts/winclean/` (pas seulement le binaire Rust principal).
- OS cibles : **Windows** et **Linux** dans un premier temps. macOS explicitement hors périmètre pour cet effort.

## Niveau de parité fonctionnelle

- **MVP minimal** : le launcher doit fonctionner sur les deux OS pour les fonctionnalités cœur (lancement de commandes, favoris, catégories, config JSON), sans exiger une parité fonctionnelle complète feature-par-feature dès cette première itération.
- **Critère d'acceptation** : sur Linux, les opérations suivantes doivent réussir de bout en bout pour valider le MVP — lancement d'une commande/action (y compris `@python`), ajout/suppression/bascule d'un favori, création/renommage/suppression d'une catégorie, persistance du `config.json` entre deux redémarrages, et démarrage automatique à la connexion. Le rendu visuel exact (pixel-perfect) et le look natif ne sont pas des critères de parité MVP.

## Stratégie UI

- Adoption d'**egui** comme toolkit UI cross-platform (immediate-mode, 100 % Rust), en remplacement des points d'intégration spécifiquement Win32 (rendu GDI dans `src/icons/gdi.rs`, `MessageBoxW`, `src/ui/card.rs`, `src/ui/mod.rs`, etc.).
- **Correction (exploration technique)** : `tao` (fork de `winit`) n'est pas compatible avec egui (qui cible `winit` nativement via `egui-winit`) et tire GTK3 sur Linux. `tao` est donc **remplacé par `eframe`**, qui embarque sa propre event loop `winit` + backend de rendu (glow/wgpu). Ce remplacement touche `main.rs`, `src/ui/mod.rs` et tout le bootstrap fenêtre — une seule UI egui/eframe pour les deux OS, pas de double implémentation UI.

## Démarrage automatique (startup)

- Sur Linux, utiliser l'**équivalent XDG autostart** (`~/.config/autostart/*.desktop`) comme pendant de la clé de Registre `HKCU\Software\...\Run` utilisée sur Windows.
- **Mode de dégradation** : si l'écriture du fichier `.desktop` échoue (répertoire non accessible en écriture) ou si l'environnement de bureau ignore la spécification XDG autostart au runtime, l'application journalise un avertissement non bloquant et continue de démarrer normalement — l'échec de l'autostart n'empêche jamais le lancement manuel de l'application.

## `system_inventory` / `winclean` sur Linux

- Portage vers de **vrais équivalents Linux**, pas des stubs :
  - Gestionnaires de paquets (apt/dnf/pacman) en lieu et place de Scoop/Choco.
  - `systemd` (services/timers) en lieu et place du Task Scheduler Windows.
  - Inventaire adapté aux mécanismes Linux équivalents aux `.vhdx` (WSL) : sur Linux, Docker s'exécute nativement (pas de VM à inventorier) — l'équivalent inventorié est l'usage disque des images/volumes/build-cache Docker natifs (`docker system df` ou lecture directe de `/var/lib/docker`), et non un fichier `.vhdx`.
  - Chemins équivalents à `%APPDATA%`/`%LOCALAPPDATA%` : voir section « Chemins de configuration/données (XDG) » ci-dessous.
- `scripts/winclean/` : **réimplémentation en Python** de la logique de `sysclean` (l'outil bash Linux original de l'auteur) au sein du même package cross-platform que la version Windows, plutôt qu'un simple appel à `sysclean` — architecture unifiée avec un registre déclaratif de modules (`CleanModule`) partagé, chaque module portant son propre `discover`/`clean` par OS. Cibles `safe`/`moderate`/`aggressive` de référence pour la découverte Linux, par analogie avec les niveaux déjà frozen côté Windows :
  - `safe` : caches de gestionnaires de paquets rebuildables (`~/.cache/pip`, `~/.cache/pnpm`, etc.), fichiers `__pycache__`/`target/` (déjà couverts côté build tooling).
  - `moderate` : caches de navigateurs, `~/.cache/*` générique, archives de paquets déjà installés (`/var/cache/apt/archives` sous Debian/Ubuntu).
  - `aggressive` : logs systemd journalés (`journalctl --vacuum`), corbeille utilisateur (`~/.local/share/Trash`).
  - Ces cibles reprennent la distinction `needs_network` déjà frozen dans le plan winclean (decision 11) : tout cache de gestionnaire de paquets est marqué `needs_network: true`.

## Backend d'icônes

- Introduction d'un **backend d'icônes portable**, abstrayant le rendu actuellement couplé à GDI (`src/icons/gdi.rs`), tout en conservant le décodage déjà neutre en OS (`src/icons/decode.rs`, crate `image`).
- **Résolution des icônes sur Linux** : recherche par lookup dans le thème d'icônes freedesktop actif (spécification [Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/)), avec repli sur l'icône embarquée dans le binaire/script cible si présente, puis sur une icône générique par défaut si aucune correspondance n'est trouvée. Il n'existe pas d'équivalent direct à l'extraction d'icône depuis un `.exe` via GDI — ce mécanisme est donc Windows-only et n'a pas de pendant Linux à porter.

## Résolution d'interpréteur Python (`@python`)

- La résolution d'environnement virtuel et d'interpréteur (`src/windows/process.rs`) doit être étendue **dans le même effort** pour couvrir Linux : `.venv/bin/python` (Linux) en plus de `.venv\Scripts\python.exe` (Windows), avec la même cascade de repli (variable d'environnement dédiée, puis `python3`, puis `python` si `python3` est absent du PATH — cas de certaines distributions minimalistes).

## Distribution

- **Build depuis les sources** uniquement pour cette itération — pas de binaires précompilés, d'installeurs ou de paquets (`.deb`, Flatpak, etc.) à produire à ce stade.
- **Distribution Linux de référence** pour le développement et la validation manuelle : Ubuntu LTS (dernière version supportée), base glibc, avec sélection automatique du backend de fenêtrage X11/Wayland par `winit` (aucune configuration manuelle requise). Les autres distributions (Fedora, Arch) ne sont pas activement testées pour cette itération mais ne sont pas explicitement exclues si le build passe.

## Chemins de configuration/données (XDG)

- Sur Linux, les chemins suivants remplacent `%APPDATA%`/`%LOCALAPPDATA%`, conformément à la [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html) :
  - `config.json` : `$XDG_CONFIG_HOME/devtoolbox/config.json`, replié sur `~/.config/devtoolbox/config.json` si la variable n'est pas définie (pendant de `%APPDATA%\DevToolBox\config.json`).
  - Dossier `icons/` : `$XDG_DATA_HOME/devtoolbox/icons/`, replié sur `~/.local/share/devtoolbox/icons/` (pendant de `%APPDATA%\DevToolBox\icons\`).
  - Fichier de log `devtoolbox.log` : `$XDG_STATE_HOME/devtoolbox/devtoolbox.log`, replié sur `~/.local/state/devtoolbox/devtoolbox.log` (pendant de `%LOCALAPPDATA%\DevToolBox\devtoolbox.log`).

## Vue Automations sur Linux

- `src/ui/app.rs` pilote actuellement la vue Automations via PowerShell (`Get-ScheduledTask`/`Stop-ScheduledTask`) contre le Task Scheduler Windows. Sur Linux, un **équivalent direct via `systemctl list-timers --output=json`** est implémenté (mêmes colonnes fonctionnelles : nom, catégorie, prochaine exécution, état), et non une vue vide ou masquée — même périmètre fonctionnel que sur Windows pour ce MVP.

## Validation du build Linux

- En l'absence de CI/CD (cf. section « Hors périmètre / différé »), la validation du MVP sur Linux (critères listés en section « Niveau de parité fonctionnelle ») est effectuée manuellement par le développeur sur la distribution de référence (Ubuntu LTS) avant de considérer le lot multi-OS comme terminé.

## Hors périmètre / différé

- macOS.
- Packaging/distribution binaire (installeurs, paquets système).
- CI/CD (aucun pipeline n'existe actuellement dans le projet, cf. `aidd_docs/memory/deployment.md` et `aidd_docs/memory/testing.md`).
- Test automatisé multi-distributions Linux (Fedora, Arch, etc.) — seule Ubuntu LTS est validée pour cette itération.
