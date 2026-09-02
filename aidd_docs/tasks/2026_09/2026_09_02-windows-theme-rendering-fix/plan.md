---
objective: "DevToolBox affiche sous Windows un thème cohérent, replie Mica sans perte de lisibilité et peut être réinstallé sans que le répertoire du programme menace les données utilisateur."
status: in-progress
---

# Plan: Corriger le thème et le rendu natif Windows

## Overview

| Field      | Value |
| ---------- | ----- |
| **Goal**   | Supprimer le mélange de styles clair/sombre, rendre Mica dépendant du backend compatible, séparer les données du programme installé, puis qualifier le paquet Windows corrigé. |
| **Source** | Signalement utilisateur et capture Windows du 2026-09-02, complétés par le journal local et le diagnostic de `src/ui/theme.rs`. |

## Phases

| #   | Phase | File |
| --- | ----- | ---- |
| 1 | Rétablir l'invariant thème-palette dans egui | [`phase-1.md`](./phase-1.md) |
| 2 | Sécuriser Mica et son repli Windows | [`phase-2.md`](./phase-2.md) |
| 3 | Séparer les données, empaqueter et qualifier Windows | [`phase-3.md`](./phase-3.md) |

## Resources

| Source | Verified |
| ------ | -------- |
| https://docs.rs/egui/0.35.0/egui/struct.Context.html#method.set_theme | `set_theme` sélectionne explicitement le style clair ou sombre actif ; `set_visuals` ne remplace que les couleurs du style déjà actif. |
| https://docs.rs/eframe/0.35.0/eframe/struct.NativeOptions.html | `NativeOptions::wgpu_options` permet de personnaliser la sélection de l'adaptateur avant la création de l'application. |
| https://docs.rs/egui-wgpu/0.35.0/egui_wgpu/struct.WgpuConfiguration.html | La configuration wgpu expose la création et la sélection de l'adaptateur utilisé par la surface. |
| https://docs.rs/window-vibrancy/0.8.0/window_vibrancy/fn.apply_mica.html | Mica est un effet Windows 11 best effort qui doit rester indépendant de la lisibilité du contenu egui. |
| https://docs.rs/cargo-packager/0.11.8/cargo_packager/config/struct.NsisConfig.html | cargo-packager accepte un template NSIS personnalisé pour exécuter la préparation contrôlée avant la suppression des fichiers. |

## Decisions

| Decision | Why |
| -------- | --- |
| Sélectionner le thème avec `Context::set_theme`, puis configurer séparément les styles clair et sombre à partir de leur propre palette. | Un thème explicite ne doit jamais dépendre du thème egui actif avant l'appel, et une mutation des deux styles ne doit jamais leur appliquer la même palette. |
| N'autoriser Mica que lorsque l'adaptateur Windows réellement retenu satisfait la politique de rendu ; toute autre combinaison peint une surface opaque. | Un effet cosmétique ne doit jamais produire une fenêtre illisible, même avec Vulkan, un override `WGPU_BACKEND`, un pilote atypique ou une API DWM indisponible. |
| Déplacer l'état local Windows vers `%LOCALAPPDATA%\RebelliousSmile\DevToolBox` et migrer uniquement les fichiers connus avant toute désinstallation. | Le paquet actuel installe ses binaires dans `%LOCALAPPDATA%\DevToolBox`, qui contient aussi logs et historique ; des racines distinctes rendent la conservation vérifiable et durable. |
| Conserver la version `0.10.0` pour cette correction. | `0.10.0` n'est pas encore publiée ; le défaut est corrigé avant sa première release plutôt que de créer artificiellement une `0.10.1`. |
