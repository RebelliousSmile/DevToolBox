# Docker — réassociation après déplacement des projets

Nettoyage effectué le 1er septembre 2026 après déplacement des répertoires de
projets. Les conteneurs Compose ont été regroupés sous des noms stables et leurs
volumes de données ont été préservés.

## État final

| Projet Compose | Volume principal | État laissé |
| --- | --- | --- |
| `scriptami` | `scriptami_mysql` | actif, MySQL sain, WordPress validé |
| `mauceri` | `mauceri_mysql` | arrêté, WordPress validé |
| `arbre-de-jade` | `arbre-de-jade_mysql` | arrêté, WordPress validé |
| `webpool` | `webpool_webpool_pgdata` | arrêté, PostgreSQL validé |
| `suddenly` | `suddenly_pgdata-dev` | arrêté, PostgreSQL validé |
| `kelenaya-diag` | `kelenaya-diag_db_data` | arrêté, MariaDB validé |

`dev-phpmyadmin` est l'unique instance phpMyAdmin, accessible sur
`http://localhost:8893`. Elle utilise `PMA_ARBITRARY=1`; les scripts de démarrage
affichent le serveur `host.docker.internal:<port MySQL>` à saisir.

## Sauvegardes

- Dossier : `C:\Users\fxgui\Documents\DockerBackups\2026-09-01-reassociation`
- 22 archives de volumes, état Docker JSON/texte et manifeste SHA-256.
- Les 22 archives ont été relues avec `tar -tzf` avant toute suppression.

## Changements durables

- `scriptami/wp-2026/scripts/start.ps1` fixe `COMPOSE_PROJECT_NAME=scriptami`.
- Les scripts `mauceri` et `scriptami` utilisent le phpMyAdmin partagé.
- `webpool/docker-compose.yml` fixe `name: webpool`.
- `suddenly/app/docker-compose.dev.yml` fixe `name: suddenly`.
- `kelenaya/_diag/docker-compose.yml` fixe `name: kelenaya-diag` et nomme le
  volume MariaDB `db_data`.

## Conservé volontairement

- Le conteneur et le volume autonomes `suddenly-review-pg` restent intacts : leur
  fonction n'était pas assez certaine pour autoriser leur suppression.
- Deux versions MariaDB subsistent : `mariadb:lts` pour wp-env et `mariadb:10.11`
  pour la compatibilité explicite de Kelenaya.
