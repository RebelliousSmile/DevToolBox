---
source: aidd_docs/tasks/2026_08/2026_08_21-docker-compose-ports-cleanup-brainstorm.md
generated_at: 2026-08-21
status: clean
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_08/2026_08_21-docker-compose-ports-cleanup-brainstorm.md`
Generated: `2026-08-21`
Updated: `2026-08-21` — all 21 gaps closed; answers folded into the source (section « Décisions »).

Total gaps: 0 | Blocker: 0 | Major: 0 | Minor: 0 | Closed: 21

Measurements taken on this machine during detection (2026-08-21) — they back several resolutions below:

- `docker compose version` → `v5.5.0` (plugin form; no `docker-compose` v1 binary in PATH).
- `docker compose config --format json` on `smartlockers-lab` → **89 ms**, fully interpolated; **exit 0 with an unreachable `DOCKER_HOST`** (works daemon-down).
- Home scan, prune on `node_modules/.git/target/.cache/.venv/vendor`: **13** compose files, **3.3 s** unlimited depth; only **10** at depth ≤ 6. Depth histogram: 4 @4, 4 @5, 2 @6, **3 @7**.
- `docker ps -a --format '{{json .}}'` already carries `Ports` and the `com.docker.compose.project` / `.project.working_dir` / `.project.config_files` labels.
- `docker volume ls --format '{{json .}}'` has **no** `CreatedAt`; `docker volume inspect` does.
- `docker ps -a` exposes only relative text (`Exited (0) 20 hours ago`); `docker inspect` exposes `.State.FinishedAt` as ISO 8601.

---

## Gaps by Category

### unstated assumption

#### Closed

**[blocker]** Should port and project-name extraction delegate to `docker compose config --format json` instead of parsing the YAML and `.env` in-process?
> Les ports variables (`${WEB_PORT:-8080}`) sont résolus via le `.env` du projet et les valeurs par défaut de la syntaxe compose.

Résolution (décision, mesuré) : oui, délégation totale. 89 ms/stack, interpolation et override gérés par Docker ; aucun crate YAML. L'hypothèse « on résout le `.env` nous-mêmes » est annulée. Bonus mesuré : `config` répond daemon éteint, donc la section Stacks reste utilisable sans daemon.

**[major]** Which key links a scanned compose file to its running containers — the `com.docker.compose.project.config_files` label, or the project name?
> Une ligne par stack : nom du projet, chemin sur disque, état (**tourne** / **arrêtée** / **partielle**)

Résolution (fait vérifié) : le label `com.docker.compose.project.config_files` (chemin exact), déjà présent dans le `docker ps -a` du fetch existant — zéro appel supplémentaire. Repli sur `com.docker.compose.project`.

**[major]** Does the home scan follow symlinks, and how does it avoid re-entering a directory through a symlink loop?
> Scan du **home** (`$HOME`) à la recherche des fichiers compose.

Résolution (décision) : symlinks non suivis, donc aucune boucle possible. Dossier illisible ignoré sans faire échouer le scan. Dépendance `walkdir` ajoutée pour ces cas précis.

### ambiguous term

#### Closed

**[major]** Which compose keys count as declared ports — only `ports`, or also `expose` and `network_mode: host`?
> Les ports sont lus **dans les fichiers compose**

Résolution (décision) : `ports` seule alimente la détection. `expose` n'est pas une publication et n'est pas affiché. `network_mode: host` donne un avertissement dédié.

**[major]** Do two publications on the same host port conflict when they bind different host interfaces (`127.0.0.1:8080` vs `0.0.0.0:8080`) or different protocols (tcp vs udp)?
> **Détection de conflit** : croisement stack ⇄ stack **et** stack ⇄ conteneurs actifs.

Résolution (décision) : conflit = même port hôte + même protocole + interfaces qui se recouvrent. `0.0.0.0`/`::` recouvre tout ; `127.0.0.1:8080` et `192.168.1.10:8080` ne sont pas en conflit.

**[major]** What makes a stack "partielle" when a one-shot init container (`lab-db-init`, `mjson-db-init`) sits permanently at `Exited (0)` in a perfectly healthy stack?
> état (**tourne** / **arrêtée** / **partielle**)

Résolution (décision user) : partielle = au moins un conteneur en cours **et** au moins un en échec (`Exited` code non nul, ou `Restarting`). `Exited (0)` = tâche terminée normalement. `smartlockers-lab` reste « tourne » ; `pilotphone` (Restarting 255) est « partielle ».

**[minor]** Is the two-month threshold counted as 60 days or as calendar months?
> Seuil **2 mois, paramétrable**

Résolution (décision) : seuil exprimé en jours, défaut 60.

### missing edge case

#### Closed

**[major]** Should the scan depth exceed 6 levels, given that 3 of the 13 compose files in this home sit at depth 7?
> **Profondeur limitée** (~6 niveaux)

Résolution (décision user, mesuré) : pas de plafond de profondeur. Exclusions par nom de dossier + garde-fou de durée qui avertit au lieu de tronquer silencieusement.

**[major]** How is a stack displayed when the same compose file runs under two different project names (`docker compose -p`)?
> Les variantes (`docker-compose.override.yml`, `.prod.yml`…) sont **rattachées au même projet**

Résolution (décision) : une ligne par projet en cours, chacune marquée de son nom de projet effectif.

**[major]** Where are containers carrying no compose label at all (`buildx_buildkit_mybuilder0`, `chromium`) shown once a Stacks section exists?
> Nouvelle section **« Stacks »** en haut de l'onglet Docker

Résolution (décision) : jamais dans Stacks ; ils restent dans la section Conteneurs.

**[major]** What does the Stacks section show for a memorized stack whose compose file has since been deleted or moved on disk?
> résultat **mémorisé en configuration**

Résolution (décision) : ligne « fichier introuvable », actions grisées, bouton « Oublier ».

### missing actor

#### Closed

**[blocker]** Which view shows the output of `compose up -d` while it pulls or builds — the existing Terminal view, the Stacks section, or nothing at all?
> **« Lancer »** : `up -d`.

Résolution (décision user) : journal inline sous la ligne de la stack, dans l'onglet Docker. Le pull et le build défilent sans quitter la vue des stacks.

### missing failure mode

#### Closed

**[blocker]** How long may `compose up -d` run before being killed, given the existing 30 s Action-class timeout and the synchronous call model that freezes the UI meanwhile?
> **« Lancer »** : `up -d`.

Résolution (fait vérifié + décision) : `up -d` n'emprunte pas le chemin synchrone plafonné. Il passe par le pipeline de streaming existant (`TerminalEvent`, slot dédié façon `action_rx`, `src/ui/egui_app.rs:874-884`), non bloquant et sans plafond, avec annulation possible.

**[major]** What does the Stacks section display when `compose up -d` exits non-zero because a host port is already bound?
> **Détection de conflit** : croisement stack ⇄ stack **et** stack ⇄ conteneurs actifs.

Résolution (décision) : statut d'échec sur la ligne, stderr conservé dans le journal inline, état de la stack inchangé.

**[major]** Does a group deletion continue with the remaining items after one removal fails, or does it stop at the first error?
> **une seule confirmation** listant tout ce qui part et l'espace récupéré

Résolution (décision) : le lot continue ; compte-rendu final listant les suppressions réussies et les échecs avec leur motif.

**[minor]** What is displayed for a stack whose compose file fails to resolve (invalid YAML, mandatory variable unset)?
> Un port **non résoluble** est **affiché brut et marqué indéterminé**

Résolution (décision) : stack listée avec badge « configuration illisible » + message d'erreur, actions grisées (si `config` échoue, `up` échouerait aussi).

### missing acceptance criterion

#### Closed

**[major]** Where is the two-month threshold configured — the Preferences config editor, or a raw JSON config field?
> Seuil **2 mois, paramétrable**

Résolution (décision) : éditeur de Préférences, persisté dans `config.json`.

**[major]** Which ordering does a group deletion apply so that a dormant container is removed before the image it still references?
> **Multi-sélection avec suppression groupée**, limitée aux ressources marquées dormantes

Résolution (décision) : conteneurs → images → volumes.

### missing dependency

#### Closed

**[blocker]** Is the Stacks section hidden when the `docker compose` plugin is absent even though the `docker` binary exists, given tab visibility currently keys off the `docker` binary alone?
> Extension de l'onglet Docker existant […] **Linux uniquement**, **local uniquement**.

Résolution (décision) : détection séparée via `docker compose version` au lancement. Absent : onglet Docker entier et fonctionnel, section Stacks avec message « plugin `docker compose` introuvable » et bouton « Scanner » grisé.

**[major]** Which command supplies the volume creation date and the container stop date, given that `docker volume ls` and `docker ps -a` expose neither?
> **image** : dormante **seulement si** non utilisée **et** créée il y a plus de 2 mois

Résolution (fait vérifié) : `docker inspect` groupé — un appel pour tous les conteneurs (`.State.FinishedAt`, ISO 8601), un appel pour tous les volumes (`CreatedAt`).

**[major]** Should the recursive home walk add a crate such as `walkdir`, or use a hand-written `std::fs` traversal, given no filesystem-walking dependency exists in `Cargo.toml` today?
> Scan **à la demande** (bouton « Scanner »)

Résolution (décision) : `walkdir` ajouté. Il gère symlinks, permissions refusées et élagage de sous-arbres — exactement les cas relevés ; un parcours maison referait ce travail moins bien.
