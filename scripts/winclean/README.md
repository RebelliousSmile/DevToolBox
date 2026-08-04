# winclean

Nettoyeur de disque pour Windows : il **propose** un plan de suppression, chiffré
et justifié, et ne supprime que si on le lui demande explicitement.

Inspiré de [`sysclean`](https://github.com/RebelliousSmile/sysclean) (Linux, bash),
réécrit pour Windows en Python, sur la couche de découverte de
`scripts/system_inventory/` — qui reste **strictement en lecture seule** : winclean
l'importe, ne le modifie pas.

```powershell
python scripts\winclean\clean.py                      # plan, aucune suppression
python scripts\winclean\clean.py --apply              # supprime le plan ci-dessus
python scripts\winclean\clean.py --json --out plan.json
```

## Le contrat de simulation

**Sans `--apply`, rien n'est supprimé — à tous les niveaux, `safe` compris.** Il
n'y a pas de niveau assez inoffensif pour se passer de la simulation : c'est le
même chemin de code, la même sélection de modules, le même total. `--apply` est
le seul interrupteur qui autorise une écriture sur le disque.

Corollaire : un plan vide n'est pas une erreur, et un plan refusé (plafond
dépassé, chemin invraisemblable) sort en code non nul **avant** la première
suppression, jamais au milieu.

## Niveaux

| Niveau | Sans `--apply` | Avec `--apply` | Mode de suppression | Confirmation |
| --- | --- | --- | --- | --- |
| `safe` (défaut) | plan affiché, **zéro suppression** | supprime | **direct** — recycler un `target/` de 8 Go ne libère rien | aucune |
| `moderate` | plan affiché, **zéro suppression** | supprime | **corbeille** quand elle peut le prendre, sinon omis en `no-undo` | interactive, ou `--yes` |
| `aggressive` | plan affiché, **zéro suppression** | supprime | **direct** — mettre la corbeille à la corbeille n'a pas de sens | interactive, plus une seconde pour `package-cache` |

Un niveau est cumulatif : `--level moderate` sélectionne les modules `safe`
**et** `moderate`.

`--recycle` est **accepté et sans effet** à `safe` et à `aggressive` ; le run le
dit en une ligne au lieu d'échouer. `moderate` est le seul niveau dont le mode de
suppression peut en dépendre — et à ce niveau la corbeille est **le défaut** :
seul `--no-recycle` la désarme.

`--level` est le seul portail du niveau. `--only <module moderate>` sans
`--level moderate` **échoue** en nommant le niveau à passer : sélectionner un
module ne monte jamais le niveau à votre place, et la confirmation n'est même pas
posée.

> Les modules `aggressive` arrivent avec la partie 3 du plan. Le niveau existe
> déjà dans le CLI ; aujourd'hui il ne sélectionne rien de plus que `moderate`.

## Modules livrés

| Module | Découverte | Réseau | Ce qu'il propose |
| --- | --- | --- | --- |
| `cargo-target` | sous les racines | non | le `target/` frère d'un `Cargo.toml` |
| `pycache` | sous les racines | non | les `__pycache__/` |
| `dotnet-binobj` | sous les racines | non | les `bin/`/`obj/` d'un projet .NET |
| `cargo-registry` | chemin par utilisateur | **oui** | le cache de sources/paquets de Cargo |
| `npm-cache` | chemin par utilisateur | **oui** | le cache npm |
| `pnpm-store` | chemin par utilisateur | **oui** | le store pnpm (nécessite `pnpm` installé) |
| `yarn-cache` | chemin par utilisateur | **oui** | le cache Yarn |
| `bun-cache` | chemin par utilisateur | **oui** | le cache Bun |
| `pip-cache` | chemin par utilisateur | **oui** | le cache pip |
| `uv-cache` | chemin par utilisateur | **oui** | le cache uv |
| `nuget-packages` | chemin par utilisateur | **oui** | le dossier de paquets NuGet |

Niveau `moderate` (n'apparaissent qu'avec `--level moderate`) :

| Module | Découverte | Réseau | Ce qu'il propose |
| --- | --- | --- | --- |
| `browser-cache` | chemin par utilisateur | non | les caches Chrome / Edge / Vivaldi / Firefox, par profil |
| `vscode-cache` | chemin par utilisateur | non | les caches de VS Code (`Cache`, `CachedData`, `GPUCache`…) |
| `user-temp` | chemin par utilisateur | non | les entrées de **premier niveau** de `%TEMP%`, fichiers compris |
| `crashdumps` | chemin par utilisateur | non | `%LOCALAPPDATA%\CrashDumps` |
| `docker-light` | commande | non | `docker system prune -f` — **sans chemin, sans taille, sans retour arrière** |

`docker-light` est le seul module qui n'émet **aucun chemin** : il délègue à
`docker system prune -f`. Conséquences assumées : la corbeille ne s'y applique
pas, aucune colonne d'octets n'est renseignée (`unknown` partout, jamais `0 B`),
et la ligne `Total reclaimed space:` que Docker imprime n'est **pas** relue — ce
serait une mesure inventée à partir du texte d'un tiers. Les volumes Docker ne
sont jamais touchés.

`--only` et `--skip` prennent ces noms, répétables ou séparés par des virgules.
Un nom inconnu arrête le run avant toute découverte, en listant les noms valides.

## Ce que `--root` borne, et ce qu'il ne borne pas

`--root` (répétable) borne **les seuls modules qui marchent dans l'arborescence**
— la colonne « sous les racines » ci-dessus. Les modules à chemin fixe résolvent
un emplacement documenté par utilisateur et **continuent de proposer leur
candidat même s'il est hors des racines** ; le plan les liste dans une section à
part, « Par utilisateur, hors de vos racines ». `--root` n'est donc pas un
confinement du run.

Sans `--root`, la racine par défaut est **la racine de ce dépôt**, et elle seule.
`%USERPROFILE%\Documents` n'est jamais une racine par défaut. Nettoyer un autre
arbre de projets est une décision explicite, prise invocation par invocation.

Quand une racine résolue tombe sous une arborescence synchronisée dans le cloud
(`OneDrive*`, `Dropbox`, `iCloudDrive`, `Documents\Perso` pour le coffre MEGA de
cette machine, ou un chemin nommé par `%OneDrive%`/`%OneDriveCommercial%`),
l'en-tête du plan le signale : une suppression y sera propagée au cloud.
L'avertissement est informatif, il ne bloque jamais.

## Gardes

Elles s'appliquent dans cet ordre, et chaque candidat écarté est **affiché**,
jamais silencieusement retiré :

1. **Chemins protégés** — aucun candidat sous les dossiers de données de
   l'utilisateur (documents, bureau, images, vidéos, musique, téléchargements),
   lus dans l'environnement. Un candidat qui y tombe est écarté (`protected`) et
   le run continue en code `0`.
2. **Vraisemblance du chemin** — un candidat trop court (moins de 10
   caractères), la racine du profil, une racine de volume ou la racine d'un
   partage UNC sont le signe d'un module défectueux, pas d'un nettoyage : le run
   **s'arrête** en code non nul, en nommant le module. Cette borne porte sur les
   candidats, pas sur les racines : `--root C:\dev` est légitime.
3. **Absorption** — un candidat imbriqué dans un autre est absorbé par son
   ancêtre, qui est nommé, pour ne pas compter deux fois les mêmes octets.
4. **`--offline`** — les candidats marqués « réseau requis » sont exclus, avec
   leur estimation, dans une section dédiée (voir plus bas).
5. **Plafond d'octets** — au-delà de `--max-delete-bytes` (50 Gio par défaut), le
   run s'arrête en nommant le total, la limite et l'option à lever.
6. **Processus propriétaire**, en **deux forces** déclarées par module :
   - `warn-and-skip` (builds : `cargo-target`, `dotnet-binobj`) — sous `--apply`,
     si un propriétaire tourne (ou que la liste des processus est illisible), le
     candidat est omis en `skipped-running` ;
   - `warn-only` (caches d'applications : `browser-cache`, `vscode-cache`) — le
     propriétaire actif est **signalé mais la suppression est tentée**. Un
     navigateur ouvert garde des fichiers de cache ouverts, il ne réécrit pas
     l'arbre : le résultat attendu est un fichier verrouillé rapporté nommément,
     pas l'omission silencieuse de tout le module. Fermer le navigateur reste la
     façon d'aller au bout.

   `--yes` vaut acquittement de cet avertissement, et ne veut jamais dire
   « applique » ni « monte le niveau ».
7. **Confirmation de niveau** — sous `--apply`, tout niveau autre que `safe`
   demande une confirmation (`oui`/`non`) sur le terminal. Sans terminal et sans
   `--yes`, la réponse est **non** : le run sort en code `2` sans avoir rien
   supprimé, plutôt que de supposer un consentement.
8. **Contrôle juste avant suppression** — la date de modification relevée au plan
   est revérifiée : différente, le candidat est omis en `skipped-changed` ;
   disparu, en `skipped-gone`.

## Verrous : deux classes de défaillance, deux sections

La colonne `failed` agrège deux choses très différentes, que le rapport sépare
nommément — dans le texte **et** sous sa propre clé du `--json` :

- **`locked_paths`** — le fichier était en cours d'utilisation
  (`ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`). C'est un fait sur l'état
  de la machine, pas une panne : le chemin est laissé en place, listé sous
  « Verrouillés », et **le code de sortie reste `0`**. Un répertoire non vide
  parce qu'une de ses entrées était verrouillée relève du même cas.
- **`recycle_failed_paths`** — la corbeille a refusé le déplacement. C'est
  **terminal** : aucun repli sur une suppression directe, le candidat est laissé
  intact, et le run sort en code `5`.

Toute autre `OSError` (accès refusé, chemin illisible…) reste un échec de
suppression : code `5`, chemin nommé sur la sortie d'erreur.

Une omission n'est pas une erreur : elle est comptée et affichée, elle ne change
pas le code de sortie.

## Corbeille : recycler n'est pas libérer

Le rapport distingue trois colonnes : `freed` (octets réellement rendus au
volume), `recycled` (octets déplacés vers la corbeille — **toujours sur le
disque**) et `failed`. Un total recyclé n'est jamais ajouté au total libéré.

La mise en garde vaut **dans les deux sens**, parce que la bibliothèque standard
ne sait pas lire l'allocation par volume de la corbeille et que le seuil de 10 %
n'en est qu'une approximation :

- un candidat **signalé** comme dépassant le seuil peut malgré tout dépasser
  l'allocation réelle de la corbeille : Windows le supprimerait alors
  définitivement, sans prévenir ;
- un candidat **non signalé** n'est pas pour autant garanti récupérable : le seuil
  n'est pas la quota.

Dès qu'un run a mis quelque chose en corbeille, il **ferme la boucle** au lieu de
laisser l'utilisateur avec un `freed: 0` inexpliqué : le rapport imprime les trois
façons de récupérer réellement les octets, du moins au plus explicite —
`--no-recycle` au prochain run, la corbeille de Windows vidée depuis son
interface, ou le module `recycle-bin` du niveau `aggressive`. Cette troisième
commande porte **obligatoirement** `--trash-days 0` : `recycle-bin` applique
sinon un plancher d'âge de 7 jours et ignorerait précisément les octets que le run
vient de déplacer. Le pied de page se déclenche sur l'**événement** de mise en
corbeille, pas sur un total non nul : recycler une quantité non mesurable le doit
tout autant.

De plus, la corbeille refuse les chemins longs (au-delà de `MAX_PATH`, elle
n'accepte pas le préfixe `\\?\`) et tout volume que Windows ne rapporte pas comme
disque fixe. Un candidat dans ce cas est marqué `no-undo` : il faut un
`--no-recycle` explicite pour le supprimer directement. **Un envoi à la corbeille
qui échoue ne se rabat jamais sur une suppression directe** : le candidat est
abandonné, compté en échec avec sa taille mesurée.

winclean ne vide jamais la corbeille implicitement.

## Réseau : `needs_network` et `--offline`

Certains éléments `safe` sont regénérables, mais **seulement en ligne** : vider
un cache de paquets veut dire re-télécharger. Ces candidats sont marqués « réseau
requis pour reconstituer » dans le plan. `--offline` les exclut, et les liste
avec leur estimation dans une section « Exclus par `--offline` » — dans le texte
comme dans la sortie `--json`. Ils sont exclus, pas escamotés.

## Options

| Option | Effet |
| --- | --- |
| `--apply` | autorise les suppressions ; sans elle, simulation |
| `--level {safe,moderate,aggressive}` | niveau, `safe` par défaut |
| `--only` / `--skip` | restreint / retire des modules (répétables, virgules acceptées) |
| `--root` | racine de recherche des modules qui marchent (répétable) |
| `--max-depth` | profondeur de descente, 6 par défaut |
| `--max-delete-bytes` | plafond du plan, `50GiB` par défaut (`500m`, `1024`, `2TiB`…) |
| `--offline` | exclut les candidats qui exigent le réseau |
| `--recycle` / `--no-recycle` | force / interdit la corbeille (inerte hors `moderate`) |
| `--yes` | acquitte l'avertissement de processus propriétaire **et** la confirmation de niveau |
| `--out <chemin>` | écrit le rapport dans un fichier, dans le format de stdout ; parent créé, fichier écrasé, stdout conservé |
| `--json` | sortie machine, un seul document JSON |
| `--top N` | tronque **l'affichage** aux N plus gros candidats ; le total, le plafond et l'ensemble supprimé restent le plan complet, et le pied de tableau dit combien de lignes sont masquées |

Il n'y a **pas** de `--check` : un plan winclean n'est pas une assertion
booléenne comme dans `deps_audit`, un plan vide est un résultat normal.

## Codes de sortie

| Code | Sens |
| --- | --- |
| `0` | plan affiché, ou application terminée (omissions comprises) |
| `2` | argument ou nom de module invalide, niveau non confirmé, ou module `moderate` demandé sans `--level moderate` |
| `3` | plafond `--max-delete-bytes` dépassé |
| `4` | candidat au chemin invraisemblable — module défectueux |
| `5` | au moins une suppression a échoué |
| `6` | run interrompu (Ctrl-C, ou erreur en cours d'application après des octets déjà libérés) |
| `7` | plateforme non Windows |

Le rapport chiffré est émis **même en cas d'interruption** : des octets ont déjà
disparu, un rapport vide serait un mensonge.

## Limites connues

- **Le garde anti-TOCTOU est secondaire, pas une garantie.** La date de
  modification d'un répertoire ne suit que ses entrées directes : réécrire le
  contenu d'un fichier existant, ou créer un fichier deux niveaux plus bas, la
  laisse intacte — l'écriture n'est donc pas détectée. Ne pas lancer `--apply`
  pendant un build ; c'est la seule vraie protection pour `cargo-target` et
  `dotnet-binobj`, où le garde primaire est la détection de processus.
- **`%TEMP%` porte le même risque résiduel, sans garde primaire.** `user-temp`
  n'a **aucun** processus propriétaire à surveiller : `%TEMP%` n'appartient à
  personne en particulier, tout le monde y écrit. Un installateur ou un test en
  cours qui remplit un sous-dossier de `%TEMP%` pendant le run n'est repéré que
  si l'entrée de premier niveau change de date. Les candidats sont d'ailleurs les
  entrées de **premier niveau**, fichiers compris, pas les seuls répertoires.
- **Les octets rapportés sont mesurés à l'application, jamais l'estimation du
  plan.** Un arbre qui a grossi entre les deux est rapporté à sa taille réelle :
  l'écart entre `estimated` et `freed`/`recycled` est normal et voulu.
- **Aucune élévation.** winclean tourne avec les droits de la session. Ce qui
  exige un administrateur (caches système, `%WINDIR%\Temp` d'autres comptes) est
  hors de portée et n'est pas proposé.
- **Les disques virtuels `.vhdx` (WSL, Docker Desktop) ne sont jamais touchés.**
  Aucun module n'en émet, et la récupération d'espace *à l'intérieur* d'un
  `.vhdx` n'est pas du ressort de cet outil : elle passe par les commandes de
  l'hôte concerné.
- **Chemins longs** : la suppression directe passe par le préfixe `\\?\` et gère
  donc les chemins au-delà de `MAX_PATH` ; la corbeille, non (voir plus haut).
- **Estimations.** Une taille vaut `null`/`inconnu` quand elle n'a pas pu être
  mesurée. Ce n'est jamais rendu par `0` : un total incomplet est signalé comme
  tel plutôt que présenté comme exact.

## Invocation

**Le terminal est le chemin d'invocation officiel.** Les actions lancées depuis
un lanceur graphique portent `CREATE_NO_WINDOW` : la sortie du plan, les
avertissements et les éventuelles confirmations seraient invisibles, ce qui est
exactement ce qu'un outil de suppression ne doit pas être. Utilisé depuis un
lanceur, winclean n'a de sens qu'avec `--json --out <fichier>`, et sans `--apply`.

## Tests

```powershell
python -m unittest discover -s scripts/winclean/tests
```
