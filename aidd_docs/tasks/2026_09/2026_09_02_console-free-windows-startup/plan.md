---
objective: "Garantir que DevToolBox ne crée aucune fenêtre console au démarrage Windows, quel que soit le profil de compilation, et que l'entrée Run active cible le binaire release."
status: in-progress
---

# Plan: Supprimer la console vide au démarrage Windows

## Overview

| Field      | Value |
| ---------- | ----- |
| **Goal**   | Lier les binaires Windows debug et release au sous-système GUI, puis remplacer l'ancien chemin debug enregistré au démarrage par le chemin du binaire release. |
| **Source** | GitHub issue [#32](https://github.com/RebelliousSmile/DevToolBox/issues/32) — « Console vide au démarrage Windows quand lancé en build debug via le Run key » |

## Phases

| #   | Phase | File |
| --- | ----- | ---- |
| 1   | Sous-système GUI pour tous les builds et rafraîchissement de l'entrée Run | [`phase-1.md`](./phase-1.md) |

## Resources

| Source | Verified |
| ------ | -------- |
| https://github.com/RebelliousSmile/DevToolBox/issues/32 | Établit le symptôme, la cause racine, le correctif attendu et le besoin de remplacer le chemin debug dans la valeur `DevToolBox` du Run key. |
| https://doc.rust-lang.org/reference/runtime.html#the-windows_subsystem-attribute | Confirme que `windows_subsystem = "windows"` sélectionne le sous-système GUI, évite la console au lancement et est ignoré sur les cibles non-Windows. |
