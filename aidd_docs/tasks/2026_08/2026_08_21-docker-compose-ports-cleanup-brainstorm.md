# Docker — stacks compose, ports et ménage des ressources dormantes (brainstorm approuvé)

> Statut : **approuvé par le user le 2026-08-21** (sortie de `/aidd-refine:01-brainstorm`).
> Demande initiale : « pour docker j'ai plusieurs besoins : avoir une liste de tous
> les docker compose de tous les projets pour pouvoir les lancer en mode -d et éviter
> de devoir les gérer avec un terminal à part à chaque fois. ensuite indiquer les
> ports de chaque conteneur pour voir s'il y a des conflits dans l'usage (j'ai souvent
> besoin d'avoir plusieurs [stacks] qui tournent en parallèle). et enfin voir s'il y a
> des conteneurs, des images ou des volumes plus utilisés (car projets terminés ou en
> sommeil) et que je peux supprimer pour faire de la place. »

## Périmètre

Extension de l'onglet Docker existant (cf. `2026_08_19-docker-tab-brainstorm.md` et
`2026_08_19-docker-tab.processed.md`), **Linux uniquement**, **local uniquement**.
Les règles du brainstorm précédent restent en vigueur et ne sont pas rouvertes :
jamais de `--force`, jamais de `prune` global, confirmation avant toute suppression,
rafraîchissement manuel.

Trois besoins distincts, indépendants les uns des autres :

- **A** — un lanceur de stacks compose, pour ne plus dédier un terminal au pilotage ;
- **B** — la visibilité des ports et de leurs conflits, avant lancement ;
- **C** — le repérage des ressources dormantes et leur suppression groupée.

## A — Section « Stacks » (nouvelle, en haut de l'onglet)

### Détection

- Scan du **home** (`$HOME`) à la recherche des fichiers compose.
- Scan **à la demande** (bouton « Scanner »), résultat **mémorisé en configuration** —
  pas de scan intégral à chaque ouverture de l'onglet (le home contient des
  `node_modules`, caches et dossiers de synchronisation).
- **Profondeur limitée** (~6 niveaux) et **liste d'exclusions** : `node_modules`,
  `.git`, `target`, `.cache`, `.venv`, `dist`, `build`, corbeille.
- Fichiers reconnus : `docker-compose.yml` / `docker-compose.yaml` /
  `compose.yml` / `compose.yaml`.
- Les variantes (`docker-compose.override.yml`, `.prod.yml`…) sont **rattachées au
  même projet**, jamais listées comme des stacks distinctes.

### Contenu

Une ligne par stack : nom du projet, chemin sur disque, état
(**tourne** / **arrêtée** / **partielle**), ports déclarés.

### Actions

- **« Lancer »** : `up -d`.
- **« Arrêter »** (action principale) : `stop` — les conteneurs sont conservés,
  le redémarrage est instantané.
- **« Détruire »** (action secondaire, confirmation distincte) : `down` — conteneurs
  et réseaux supprimés. **Jamais** `-v` : les volumes ne se suppriment que par
  l'action ciblée existante.

## B — Ports et conflits

- Les ports sont lus **dans les fichiers compose** — donc visibles même quand la
  stack est arrêtée, ce qui est le seul moyen d'anticiper un conflit *avant*
  lancement — et complétés par les ports réellement publiés par les conteneurs actifs.
- **Ports variables** (`${WEB_PORT:-8080}`) : résolus via le `.env` du projet et les
  valeurs par défaut de la syntaxe compose. Un port **non résoluble** est **affiché
  brut et marqué indéterminé**, puis **exclu de la détection de conflit** — pas de
  faux positif silencieux.
- **Détection de conflit** : croisement stack ⇄ stack **et** stack ⇄ conteneurs
  actifs. Signalement visuel sur **les deux** lignes concernées.
- Nouvelle **colonne « Ports »** dans le tableau Conteneurs.
- Cas d'usage réels cités par le user : `lab` + `tasks`, `multisite-clients` +
  `API_mobile` tournant en parallèle.

## C — Ressources dormantes

### Contrainte technique reconnue

**Docker ne stocke aucune date de « dernière utilisation »**, ni pour les images ni
pour les volumes. Le seuil « 2 mois » ne peut donc pas s'appuyer sur un vrai
« last used » : il affine des signaux existants au lieu de déclencher seul.

### Critère retenu (conservateur)

Seuil **2 mois, paramétrable** :

- **conteneur** : arrêté depuis plus de 2 mois ⇒ dormant (signal fiable — la date
  d'arrêt est réellement disponible) ;
- **image** : dormante **seulement si** non utilisée **et** créée il y a plus de
  2 mois ;
- **volume** : dormant **seulement si** orphelin **et** créé il y a plus de 2 mois.

La date ne déclenche jamais seule. Une image ancienne mais utilisée n'est jamais
signalée.

### Suppression

- **Multi-sélection avec suppression groupée**, limitée aux ressources marquées
  dormantes : cases à cocher, **une seule confirmation** listant tout ce qui part et
  l'espace récupéré.
- Les suppressions individuelles existantes restent inchangées.
- Toujours sans `--force` et sans `prune`.

## Présentation

- Nouvelle section **« Stacks »** en haut de l'onglet Docker, au-dessus de
  « Conteneurs ».
- Colonne **« Ports »** ajoutée au tableau « Conteneurs ».

## Hors périmètre (explicitement)

- Reconduit du brainstorm précédent : registre distant (recherche/pull), logs,
  réseaux, `prune` global, support Podman, portage Windows.
- Nouveau : édition des fichiers compose, création de stacks, ports occupés par des
  **processus non-Docker** (un serveur de dev lancé à la main sur 8080 ne sera pas
  détecté comme conflit dans cette itération).

---

## Décisions (2026-08-21, après `/aidd-refine:04-shadow-areas`)

Le rapport de zones d'ombre a relevé 21 manques (4 bloquants). Les décisions ci-dessous
les referment toutes. **Elles corrigent le corps du document ci-dessus là où il se
contredit** : en cas de divergence, cette section fait foi. Chaque décision marquée
*(mesuré)* s'appuie sur une mesure faite sur la machine de référence le 2026-08-21.

### Source de vérité des fichiers compose

- **Aucun parsing YAML côté DevToolBox.** Ports, nom de projet, services et fusion des
  fichiers override viennent tous de `docker compose config --format json`
  — **89 ms par stack** *(mesuré)*, et l'interpolation `${VAR:-défaut}` / `.env` /
  `extends` est faite par Docker lui-même. Aucun crate YAML n'entre dans `Cargo.toml`.
  L'hypothèse « résolus via le `.env` du projet » du corps du document est **annulée**.
- `docker compose config` **fonctionne daemon éteint** *(mesuré : exit 0 avec
  `DOCKER_HOST` injoignable)* : la section Stacks liste les stacks et leurs ports même
  quand le daemon ne répond pas. Seuls l'état et les actions exigent le daemon.
- Une stack dont `compose config` échoue (YAML invalide, variable obligatoire absente)
  est **listée** avec un badge « configuration illisible » et le message d'erreur ; ses
  boutons d'action sont grisés (si `config` échoue, `up` échouerait aussi).

### Scan du home

- **Pas de plafond de profondeur.** 6 niveaux auraient raté 3 des 13 fichiers compose du
  home *(mesuré : 13 fichiers, dont 3 à la profondeur 7 ; 3,3 s en profondeur illimitée)*.
  L'hypothèse « ~6 niveaux » du corps du document est **annulée**.
- Exclusions par **nom de dossier** : `node_modules`, `.git`, `target`, `.cache`,
  `.venv`, `vendor`, corbeille. **Symlinks non suivis** (pas de boucle possible) ;
  un dossier illisible est ignoré sans faire échouer le scan.
- Garde-fou de durée : au-delà d'un plafond de temps, le scan s'arrête et affiche ce
  qu'il a trouvé avec un avertissement explicite — jamais de troncature silencieuse.
- Dépendance ajoutée : **`walkdir`** (gestion des symlinks, des permissions refusées et
  du filtrage de sous-arbres). Un parcours `std::fs` maison referait ce travail moins bien.

### Rattachement stack ⇄ conteneurs

- Clé de liaison : le label **`com.docker.compose.project.config_files`** (chemin exact
  du fichier), lu dans le `docker ps -a` **déjà récupéré** par le fetch existant — zéro
  appel supplémentaire. Repli sur `com.docker.compose.project` si le label est absent.
- Le même fichier compose lancé sous plusieurs noms de projet (`-p`) produit **une ligne
  par projet en cours**, chacune marquée de son nom de projet effectif.
- Les conteneurs **sans aucun label compose** (`buildx_buildkit_mybuilder0`…) n'entrent
  jamais dans la section Stacks ; ils restent dans la section Conteneurs.
- Une stack mémorisée dont le fichier a disparu du disque affiche « fichier introuvable »,
  actions grisées, et un bouton « Oublier » pour la retirer de la liste.

### État d'une stack

- **tourne** : au moins un conteneur en cours, aucun en échec.
- **partielle** : au moins un conteneur en cours **et** au moins un en échec — `Exited`
  avec code non nul, ou `Restarting` en boucle.
- **arrêtée** : aucun conteneur en cours.
- Un conteneur `Exited (0)` est une **tâche terminée normalement**, jamais une panne :
  les conteneurs d'init (`lab-db-init`, `mjson-db-init`) ne rendent pas leur stack
  partielle.

### Lancement et arrêt

- `compose up -d` n'emprunte **pas** le chemin des appels docker synchrones plafonnés à
  30 s — un pull ou un build les dépasserait et figerait l'UI. Il emprunte le pipeline de
  streaming existant (`TerminalEvent` : `Started`/`Output`/`Finished`, slot dédié à la
  manière de `action_rx`, `src/ui/egui_app.rs:874-884`), non bloquant et sans plafond.
- La sortie s'affiche dans un **journal inline sous la ligne de la stack**, dans l'onglet
  Docker : le pull et le build défilent sans quitter la vue des stacks.
- Une opération en cours est **annulable** (le processus est tué).
- Échec de `up -d` (port déjà pris, `.env` manquant, image introuvable) : statut d'échec
  sur la ligne, stderr conservé dans le journal, l'état de la stack reste inchangé.

### Ports et conflits

- Seule la clé **`ports`** (publications sur l'hôte) alimente la détection. `expose`
  n'est pas une publication et n'est pas affiché. `network_mode: host` est signalé par un
  avertissement dédié (le service peut occuper n'importe quel port).
- **Règle de conflit** : même port hôte **et** même protocole (`tcp`/`udp`) **et**
  interfaces qui se recouvrent. `0.0.0.0` (ou `::`) recouvre toute interface ;
  `127.0.0.1:8080` et `192.168.1.10:8080` ne sont **pas** en conflit.
- Les ports des conteneurs actifs viennent du champ `Ports` déjà présent dans la sortie
  de `docker ps -a --format '{{json .}}'` *(mesuré)* — aucun appel supplémentaire.

### Dormance et suppression

- Seuil exprimé **en jours** (défaut **60**), réglable dans l'éditeur de Préférences et
  persisté dans `config.json`. « 2 mois » = 60 jours, pas des mois calendaires.
- Les dates ne sont pas dans les listings : `docker volume ls` n'expose pas `CreatedAt` et
  `docker ps -a` ne donne qu'un texte relatif (« Exited (0) 20 hours ago ») *(mesuré)*.
  Elles proviennent d'un **`docker inspect` groupé** — un appel pour tous les conteneurs
  (`.State.FinishedAt`, ISO 8601), un appel pour tous les volumes (`CreatedAt`).
- Suppression groupée : ordre **conteneurs → images → volumes** (une image ne peut partir
  avant le conteneur qui la référence). Un échec **n'interrompt pas** le lot ; un
  compte-rendu final liste ce qui a été supprimé et ce qui a échoué, avec le motif.

### Prérequis

- La visibilité de l'onglet Docker continue de dépendre du seul binaire `docker`.
- Le **plugin `docker compose`** est détecté séparément (`docker compose version`) au
  lancement. Absent : l'onglet Docker reste entier et fonctionnel, seule la section Stacks
  affiche « plugin `docker compose` introuvable » avec le bouton « Scanner » grisé.
