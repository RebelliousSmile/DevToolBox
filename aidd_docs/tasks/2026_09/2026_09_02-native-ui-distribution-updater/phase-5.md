---
status: in-progress
---

# Instruction: Implémenter l'updater signé et ses chemins de récupération

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

~~~txt
.
├── Cargo.toml                         ✏️ ajouter updater HTTP semver et signature
├── Cargo.lock                         ✏️ verrouiller les dépendances
├── build.rs                           ✅ valider et embarquer le trousseau public au build
├── docs
│   └── updater-key-operations.md      ✅ documenter rotation et compromission
├── tests
│   └── fixtures
│       └── updater                    ✅ fournir manifests payloads et clés de test
└── src
    ├── main.rs                        ✏️ exposer version et configuration d'update
    ├── ui
    │   ├── dialogs.rs                 ✏️ présenter confirmation et récupération
    │   └── egui_app.rs                ✏️ piloter la machine d'état
    └── update
        ├── keys.rs                    ✅ exposer clés embarquées et empreintes
        ├── manifest.rs                ✅ valider schéma plateforme et signatures
        ├── mod.rs                     ✅ exposer le service
        └── service.rs                 ✅ vérifier télécharger et installer hors UI
~~~

## User Journey

~~~mermaid
flowchart TD
  A[Démarrer 0.10.0 ou vérifier manuellement] --> B{Clé de production embarquée}
  B -->|non| C[Afficher updater non configuré]
  B -->|oui| D[Lire latest json]
  D --> E{Payload compatible et signé}
  E -->|non| F[Refuser sans modifier l installation]
  E -->|oui| G[Présenter version notes et taille]
  G --> H[Confirmer télécharger vérifier et installer]
  H --> I[Relancer ou proposer récupération manuelle]
~~~

## Test Scope

~~~mermaid
---
title: Test scope
---
journey
  section Setup
    Servir fixtures et clés de test locales => transport déterministe: 5: cli
  section Happy path
    Vérifier un payload plus récent à double signature => clé connue choisie et bytes validés: 5: system
    Accepter l installation simulée => progression ordonnée hors thread UI: 5: system
  section Edge case - installation historique
    Détecter une version avant 0.10.0 ou format inconnu => installation manuelle proposée: 1: system
  section Edge case - interruption
    Injecter disque plein arrêt ou relance échouée => version courante ou récupération conservée: 1: system
  section Edge case - manifeste hostile
    Injecter downgrade clé inconnue URL externe ou taille excessive => refus avant exécution: 1: system
  section Teardown
    Arrêter serveur et nettoyer téléchargements => aucun temporaire restant: 5: cli
~~~

## Wireframe

~~~txt
┌──────────────────────────────────────────────┐
│ (1) État de mise à jour                     │
├──────────────────────────────────────────────┤
│ (2) Version · notes · taille · provenance   │
│ (3) Progression ou diagnostic               │
├──────────────────────────────────────────────┤
│ (4) Reporter              (5) Continuer     │
└──────────────────────────────────────────────┘
~~~

1. État : disponible, à jour, non configuré ou récupération.
2. Résumé : permet de vérifier la cible avant téléchargement.
3. Diagnostic : rend réseau, signature et installation observables.
4. Reporter : conserve l'application utilisable.
5. Continuer : exige une action explicite avant installation ou élévation.

## Tasks to do

### 1) Définir le protocole

> Rendre version et confiance non ambiguës.

1. Définir schéma, version minimale 0.10.0, OS, architecture, format, taille, URL GitHub HTTPS et signatures par key_id.
2. Fixer l'endpoint au dépôt RebelliousSmile/DevToolBox et borner timeout, redirections et taille.
3. Refuser versions égales ou inférieures, schémas inconnus, clés inconnues et clients trop anciens.
4. Accepter ancien et nouveau pendant deux versions mineures et 180 jours ; documenter la réinstallation après fenêtre.
5. Faire lire à build.rs le trousseau public de production fourni par l'environnement protégé, générer des constantes dans OUT_DIR et exposer les empreintes dans les métadonnées de build.
6. Épingler cargo-packager-updater à la version compatible avec le packager et le resolver de la phase 4, puis tester leur format de manifeste commun.

### 2) Isoler le service

> Ne jamais bloquer le rendu ni remplacer directement le binaire.

1. Modéliser idle, checking, available, downloading, verifying, installing, restart-required, recovery et failed.
2. Injecter transport, horloge, trousseau et installateur ; borner les événements.
3. Utiliser cargo-packager-updater pour app, NSIS et AppImage ; déléguer deb.
4. Détecter les emplacements non inscriptibles et passer par le mécanisme du packager ou de l'OS.

### 3) Préserver et récupérer

> Traiter interruption et échec de relance comme des chemins normaux.

1. Télécharger dans un temporaire adjacent ou sûr, synchroniser, vérifier puis installer atomiquement quand le format le permet.
2. Pour AppImage, copier le fichier courant avant remplacement et le conserver jusqu'au lancement sain.
3. Pour NSIS et app macOS, télécharger avant installation le payload de récupération de la version courante depuis son URL de tag immuable, puis vérifier plateforme, taille et signature avec une clé déjà connue.
4. Écrire un marqueur de lancement sain ; au démarrage suivant, choisir la récupération définie pour le format.
5. Nettoyer les temporaires après succès, annulation ou erreur.
6. Si le payload de récupération courant est absent ou invalide, interdire l'auto-installation et proposer uniquement le téléchargement ou la réinstallation manuelle.

### 4) Intégrer l'expérience et les tests

> Garder l'update explicite et désactivable.

1. Vérifier après le premier rendu au maximum une fois par 24 heures avec horodatage persistant et jitter borné ; Vérifier maintenant ignore cette fenêtre.
2. Appliquer un backoff borné sur 403 ou 429, proxy, portail captif et cache obsolète sans décaler la commande manuelle.
3. Exiger un clic avant téléchargement ou exécution et expliquer fermeture ou élévation.
4. Désactiver proprement en développement, avant 0.10.0, format inconnu ou trousseau production absent.
5. Couvrir toutes les frontières avec clés et installateurs fixtures.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Seul un payload plus récent, compatible et signé par une clé connue est accepté ; la fenêtre de rotation est calculée sans télémétrie, et les tests prouvent que clés et empreintes compilées correspondent à l'entrée du build script. |
| 2 | Réseau et installation ne bloquent jamais l'UI, et deb ou emplacement non inscriptible déclenchent un handoff explicite. |
| 3 | Disque plein, interruption et échec du premier lancement sélectionnent par test le rollback AppImage, la réinstallation NSIS ou la restauration app macOS ; un payload de récupération absent ou invalide interdit l'auto-installation. |
| 4 | Les fixtures couvrent cadence 24 heures, commande manuelle, jitter, 403, 429, proxy, cache, downgrade, mauvais OS ou architecture, signatures, annulation et absence de clé de production. |
