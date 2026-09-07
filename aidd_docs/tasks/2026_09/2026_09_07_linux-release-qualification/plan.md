---
objective: "Les releases 0.10.0 et 0.11.0 de DevToolBox sont attestées par GitHub Actions, l’updater est vérifié contre une release publique 0.11.0, et la matrice Linux est qualifiée sous Wayland et Ubuntu 24.04 x64 avec des preuves datées."
status: in-progress
---

<!-- Fill or omit these sections; never add, rename, or reorder one. -->

# Plan: Qualification release Linux complète

## Overview

| Field | Value |
| --- | --- |
| **Goal** | Clore les quatre écarts de qualification Linux laissés par la preuve locale du 4 septembre : attestation GitHub, updater réel, Wayland et Ubuntu 24.04. La 0.10.0 publique fournit la base de récupération nécessaire à la 0.11.0. |
| **Source** | Demande utilisateur du 7 septembre 2026 : « les 4 », faisant suite à `aidd_docs/tasks/2026_09/2026_09_03_linux-local-qualification/plan.md`. |

## Phases

| # | Phase | File |
| --- | --- | --- |
| 1 | Préparer la candidate 0.11.0 et la porte de release | [`phase-1.md`](./phase-1.md) |
| 2 | Attester les artefacts et valider l’updater réel sur draft | [`phase-2.md`](./phase-2.md) |
| 3 | Qualifier Ubuntu 22.04 sous Wayland | [`phase-3.md`](./phase-3.md) |
| 4 | Qualifier Ubuntu 24.04 et consigner la matrice | [`phase-4.md`](./phase-4.md) |

## Resources

| Source | Verified |
| --- | --- |
| `docs/release-readiness.md` | La matrice exige une attestation GitHub, update AppImage, Wayland et Ubuntu 24.04 ; une porte manquante interdit la publication stable. |
| `.github/workflows/release.yml` | La CI crée un draft, atteste ses assets avec OIDC, génère le feed après 0.10.0 et n’autorise la publication qu’après approbation protégée. |
| `docs/updater-key-operations.md` | La rotation de clés exige deux versions mineures et 180 jours ; les clés privées ne doivent jamais entrer dans le dépôt. |

## Decisions

| Decision | Why |
| --- | --- |
| La validation de l’updater utilise une candidate `0.11.0`, pas `0.10.0`. | Le workflow traite explicitement `0.10.0` comme première installation sans feed ; un cycle réel d’update exige une version suivante. |
| La qualification utilise une release publique explicitement déclenchée dans `pre-release`, jamais la porte de stable. | L’AppImage doit atteindre un feed et des assets accessibles sans jeton pour prouver le parcours réel, tandis que la porte de stable reste réservée à la matrice QA complète. |
| Les secrets et l’approbation restent exclusivement dans l’environnement GitHub `production-release`. | Les clés Ed25519 privées sont hors dépôt ; les attestations utilisent une identité OIDC éphémère sans clé privée supplémentaire. |
| Ubuntu 24.04 est qualifié dans des sessions X11 et Wayland séparées. | La matrice de release demande ces deux serveurs d’affichage pour les paquets Linux ; un conteneur ne peut pas prouver l’intégration bureau, FUSE et le rendu natif. |
