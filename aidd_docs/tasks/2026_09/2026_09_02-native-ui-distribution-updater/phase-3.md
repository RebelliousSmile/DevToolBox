---
status: done
---

# Instruction: Ajouter les matériaux natifs avec un repli opaque testable

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

~~~txt
.
├── Cargo.toml                         ✏️ ajouter window-vibrancy et winit ciblés
├── Cargo.lock                         ✏️ verrouiller les dépendances
└── src
    ├── main.rs                        ✏️ configurer les attributs de fenêtre
    ├── storage
    │   └── models.rs                  ✏️ ajouter native_effects rétrocompatible
    └── ui
        ├── egui_app.rs                ✏️ synchroniser thème et préférence
        ├── mod.rs                     ✏️ exposer la politique de fenêtre
        ├── native_window.rs           ✅ décider puis appliquer l'effet en best effort
        └── theme.rs                   ✏️ fournir surfaces translucides et opaques
~~~

## User Journey

~~~mermaid
flowchart TD
  A[Créer la fenêtre] --> B[Lire OS thème accessibilité et préférence]
  B --> C{Matériau autorisé et disponible}
  C -->|oui| D[Appliquer Vibrancy ou Mica]
  C -->|non| E[Peindre le fond opaque]
  D --> F[Conserver contraste et chrome natif]
  E --> F
~~~

## Test Scope

~~~mermaid
---
title: Test scope
---
journey
  section Setup
    Injecter OS capacités thème et préférences => politique inspectable: 5: cli
  section Happy path
    Simuler macOS ou Windows 11 compatibles => profil natif unique sélectionné: 5: cli
    Simuler Linux ou Windows ancien => profil opaque complet sélectionné: 5: cli
  section Edge case - accessibilité dynamique
    Activer Reduce Transparency pendant l exécution => matériau retiré et fond repeint: 1: system
  section Edge case - API refusée
    Injecter un échec natif => avertissement unique et repli opaque: 1: cli
~~~

## Wireframe

~~~txt
┌─────────────────────────────────────────────────────────────────┐
│ (1) Chrome natif conservé                                      │
├───────────────┬─────────────────────────────────────────────────┤
│ (2) Matériau  │ (3) Surface de contenu contrastée              │
│ ou opaque     │                                                 │
│               │ (4) Cartes opaques ou translucides sûres       │
│               │                                                 │
└───────────────┴─────────────────────────────────────────────────┘
~~~

1. Chrome : conserve traffic lights, DWM, déplacement et redimensionnement.
2. Matériau : limite la vibrancy/Mica à la zone de navigation.
3. Surface : garantit la lisibilité indépendamment du fond système.
4. Cartes : partagent le même layout dans tous les profils.

## Tasks to do

### 1) Formaliser la politique pure

> Séparer décision testable et appel natif.

1. Définir MacVibrancy, WindowsMica et Opaque.
2. Ajouter native_effects avec défaut vrai sans casser les anciens JSON.
3. Prendre en compte thème, Reduce Transparency, version/capacité et changement à chaud.
4. Sélectionner et épingler des versions de window-vibrancy, winit et raw-window-handle compatibles avec eframe 0.35 ; ajouter un test de compilation qui empêche deux versions incompatibles de handles natifs.

### 2) Intégrer macOS

> Employer les API publiques sans fausse fenêtre custom.

1. Étendre le contenu sous un titre transparent et masquer uniquement le texte.
2. Appliquer NSVisualEffectView via window-vibrancy.
3. Ne jamais masquer traffic lights, ombre, déplacement, redimensionnement ou plein écran.

### 3) Intégrer Windows et Linux

> Fournir une qualité cohérente avec des capacités différentes.

1. Demander Mica sur Windows 11 compatible et laisser DWM gérer le chrome.
2. Conserver un fond opaque sur Windows non compatible.
3. Peindre intégralement le fond sous X11 et Wayland sans protocole de blur.

### 4) Prouver les replis

> Empêcher l'effet cosmétique de rendre l'application inutilisable.

1. Injecter toutes les capacités et erreurs dans les tests de politique.
2. Vérifier activation, désactivation et changement d'accessibilité à chaud.
3. Vérifier qu'aucun profil ne laisse une zone transparente sans peinture de secours.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Toute combinaison injectée produit exactement un profil, toute désactivation produit Opaque et le graphe Cargo contient une seule génération compatible de raw-window-handle pour les appels natifs. |
| 2 | La cible macOS compile avec titre intégré et appel vibrancy borné derrière cfg. |
| 3 | Les cibles Windows/Linux compilent avec Mica conditionnel ou fond opaque complet. |
| 4 | Les erreurs et changements dynamiques reviennent à Opaque sans crash, spam de logs ni perte de contraste. |
