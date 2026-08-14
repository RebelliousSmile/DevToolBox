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

Le niveau `aggressive` a **sa propre confirmation en plus** de celle du niveau,
pour le seul module `package-cache` (voir plus bas) : `--yes` n'y répond pas.

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

Niveau `aggressive` (n'apparaissent qu'avec `--level aggressive`) :

| Module | Découverte | Réseau | Ce qu'il propose |
| --- | --- | --- | --- |
| `recycle-bin` | corbeille par volume | non | les éléments de **votre** corbeille plus vieux que le plancher d'âge |
| `package-cache` | chemin fixe | non | `%ProgramData%\Package Cache` — **sans retour arrière** |
| `ollama-models` | API locale, opt-in | **oui** | les seuls modèles nommés exactement avec `--ollama-model` |

À ce niveau, les suppressions sont **directes** : `--recycle` est accepté et inerte.

### Modèles Ollama : sélection explicite uniquement

Les modèles sont des données utilisateur : `ollama-models` est donc exclu de
tous les runs larges, même `--level aggressive`. Copiez le nom canonique complet
depuis `ollama list`, tag compris (par exemple `qwen3:latest`), puis commencez
par la simulation :

```bash
python scripts/winclean/clean.py --level aggressive --only ollama-models --ollama-model MODEL
python scripts/winclean/clean.py --level aggressive --only ollama-models --ollama-model MODEL --apply
```

`--ollama-model` est répétable. `--top` ne tronque que l'affichage : sous
`--apply`, tous les noms explicitement fournis restent dans la portée.

Le démon Ollama local doit déjà tourner pour la simulation comme pour
l'application. Si winclean annonce qu'il est indisponible, vérifiez d'abord
`ollama list` et le service Ollama ; winclean ne démarre ni n'élève jamais un
service implicitement. Les modèles actifs visibles dans `ollama ps` sont refusés
et doivent être arrêtés explicitement. Seules les adresses HTTP loopback
(`localhost`, `127.0.0.1`, `::1`) sont acceptées : un `OLLAMA_HOST` distant est
refusé, car il ne libérerait pas ce disque.

winclean n'infère jamais qu'un modèle est « inutilisé » : Ollama ne fournit pas
de preuve de dernière utilisation. La suppression est sans retour arrière et la
restauration exige normalement un nouveau `ollama pull`. Les tailles du plan
sont logiques ; des modèles pouvant partager des blobs, les octets réellement
récupérés restent `unknown`.

Le nettoyage de blobs partiels ou orphelins est hors périmètre. La suppression
manuelle sous `models/blobs/` ou `models/manifests/` n'est pas prise en charge :
le module délègue toujours la suppression à l'API locale d'Ollama.

- **`recycle-bin` n'énumère que la corbeille du compte courant.** Elle est
  identifiée par son SID ; si le SID ne se résout pas, le module ne propose
  **rien** et le dit (`recycle-bin-sid-unknown`) au lieu de deviner. Les
  corbeilles des autres comptes ne sont jamais parcourues, et seuls les volumes
  que Windows rapporte comme disques fixes sont regardés.
- **Plancher d'âge de 7 jours par défaut** (`--trash-days`, qu'un fichier de
  configuration peut relever mais jamais abaisser). Un élément plus récent est
  conservé, et le plan nomme le plancher en vigueur. La date vient de l'en-tête
  `$I` de l'entrée ; illisible, elle retombe sur la date du `$R` apparié ;
  indisponible, l'entrée est **omise** et signalée, jamais supprimée par défaut.
- **`package-cache` casse les réparations futures.** Windows Installer garde là
  les charges MSI/MSP des produits installés : les supprimer fait qu'une
  réparation, une modification ou une désinstallation ultérieure réclame le média
  d'origine ou échoue. Ces octets ne se reconstituent pas localement. La
  conséquence **survit au run**, ce qu'aucun autre module ne fait — d'où une
  confirmation dédiée : une réponse interactive, ou `--yes-package-cache`. Sans
  l'un des deux (typiquement un appel sans terminal avec `--yes`), ce module
  **seul** est omis en `skipped-unconfirmed` et le reste du run se déroule
  normalement.

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

## Configuration

Emplacement par défaut : **`%APPDATA%\winclean\winclean.json`**, lu s'il existe.
`--config <fichier>` en désigne un autre — et un `--config` explicite pointant un
fichier absent **échoue** (`2`), là où l'absence du fichier par défaut est
normale : nommer un fichier est une intention, son silence serait une trahison.

Le fichier est lu comme des **données** (`json.load`, jamais `eval`/`import`) et
ne peut que **restreindre**. Il n'y a délibérément aucune clé pour `--apply`, le
niveau, `--yes` ou des racines : élargir un run est un acte par invocation.
Une clé inconnue arrête le run **avant toute découverte**, en code `2`.

| Clé | Type | Défaut | Effet | Résolution |
| --- | --- | --- | --- | --- |
| `TRASH_DAYS` | entier ≥ 0 | `7` | plancher d'âge de `recycle-bin` | CLI > fichier > défaut |
| `MAX_DELETE_BYTES` | entier ≥ 0 | `53687091200` (50 Gio) | plafond du total du plan | CLI > fichier > défaut |
| `PROTECTED_PATHS` | liste de chemins **absolus** | `[]` | chemins protégés en plus des dossiers de données de l'utilisateur | **union** avec la CLI |
| `DISABLED_MODULES` | liste de noms de modules connus | `[]` | modules retirés de toute sélection | **union** avec `--skip` |

Deux comportements à retenir :

- **Les scalaires suivent CLI > fichier > défaut** ; un `TRASH_DAYS: 0` écrit dans
  le fichier n'est pas confondu avec son absence. Mais un fichier ne peut jamais
  *desserrer* : le plafond ne peut être qu'abaissé, le plancher d'âge que relevé.
- **Les deux clés d'ensemble s'unissent avec la ligne de commande** au lieu d'être
  remplacées par elle. Une protection que le drapeau le plus courant peut défaire
  ne protège pas un run non surveillé. Corollaire assumé : nommer un module
  désactivé dans `--only` est une **erreur** non nulle, pas un plan vide qui se
  lirait « rien à nettoyer ».

`winclean.json.example` est du JSON strict — donc sans commentaire : le commentaire
de chaque clé est ce tableau. Tolérer une clé de commentaire contredirait la
validation de *toutes* les clés, qui est le point du fichier.

## Historique des runs destructeurs

Emplacement : **`%LOCALAPPDATA%\winclean\history.jsonl`** — `%LOCALAPPDATA%` et
non `%APPDATA%`, un journal de machine ne se synchronise pas d'un poste à l'autre.
Une ligne JSON par run, ajoutée à la fin, **500 lignes** conservées au plus
(élagage par lignes, donc une ligne corrompue survit au lieu d'être escamotée).

**Qui écrit une ligne** : un run qui a **tenté** une suppression, et lui seul. Une
simulation n'écrit rien. Un run `--apply` dont tous les candidats ont été omis, ou
qui s'est arrêté avant la boucle (plafond, confirmation refusée), n'écrit rien non
plus. À l'inverse un run qui n'a rien pu mesurer (`docker-light`) ou qui n'a fait
que recycler (`freed: 0`) écrit bien sa ligne : le déclencheur est la tentative,
pas le total d'octets. Un échec d'écriture est un **avertissement** sur la sortie
d'erreur, jamais un code de sortie ni un statut.

```json
{"timestamp":"2026-08-04T22:13:53Z","level":"safe","status":"completed",
 "estimated_bytes":4096,"freed_bytes":4096,"recycled_bytes":0,"failed_bytes":0,
 "modules":{"pycache":{"estimated":4096,"measured":4096}}}
```

`timestamp` est UTC, suffixé `Z`. `status` vaut `completed` ou `interrupted`. Il
n'y a **pas** de champ « mode » : une simulation n'écrivant rien, la colonne serait
constante. Un total non mesurable est `null`, jamais `0`.

`--history N` relit les N derniers runs et sort : aucune découverte, aucun chemin
touché. Elle est incompatible avec `--apply` par construction.

## Estimé, puis mesuré

Le rapport d'un run `--apply` imprime une table `estimé / mesuré / écart`, une
ligne par module. Les deux chiffres viennent de **deux parcours indépendants à
deux instants** : l'estimation au plan, la mesure juste avant et juste après
chaque suppression. L'égalité n'est donc pas une propriété d'un run correct — un
arbre qui a grossi entre-temps mesure plus que son estimation, et c'est le fait,
pas un défaut.

Un module qui n'a **rien tenté** (omis en entier) n'a rien mesuré : sa cellule
`mesuré` et son écart affichent tous deux `—`, et le `--json` porte `null`. Un
nombre *plus petit* que l'estimation veut dire autre chose : le candidat a été
tenté et partiellement empêché — le cas du fichier verrouillé. Les deux se lisent
donc sans ambiguïté.

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
| `--yes` | acquitte l'avertissement de processus propriétaire **et** la confirmation de niveau ; jamais `--apply`, jamais le niveau, jamais `package-cache` |
| `--yes-package-cache` | répond d'avance à la seule confirmation propre à `package-cache` |
| `--config <fichier>` | fichier de configuration JSON (défaut `%APPDATA%\winclean\winclean.json` s'il existe) |
| `--trash-days N` | plancher d'âge de `recycle-bin`, 7 jours par défaut ; `0` prend tout ce qui est éligible |
| `--history N` | affiche les N derniers runs destructeurs et sort ; ne découvre rien, exclusif de `--apply` |
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

## Ce que winclean ne fait **pas**

Cette liste est un engagement, pas un retard de développement. Chaque entrée est
un endroit où un nettoyeur peut casser Windows ou mentir sur ce qu'il a récupéré.

- **WinSxS / le magasin de composants** (`%WINDIR%\WinSxS`). Ce n'est pas un
  cache : les fichiers y sont *liés en dur* dans le système en service, et sa
  taille apparente n'est pas de l'espace récupérable. Seul `DISM
  /Cleanup-Image` sait ce qui y est encore référencé — et il exige une élévation.
- **Le cache de Windows Update** (`%WINDIR%\SoftwareDistribution`). Le supprimer
  à la main pendant qu'un service le tient produit des mises à jour qui échouent
  en boucle. Cela passe par l'arrêt des services concernés, donc par une
  élévation, et par le nettoyage de disque de Windows.
- **Les clichés instantanés (VSS) et les points de restauration.** Ils sont la
  dernière chance de revenir en arrière, y compris après un nettoyage raté. Un
  outil qui détruit le filet en même temps que la poussière n'est pas un outil de
  nettoyage.
- **`hiberfil.sys`, `pagefile.sys`, `swapfile.sys`.** Ce sont des réglages du
  système (veille prolongée, mémoire virtuelle), pas des fichiers : leur taille
  se change par `powercfg` ou les propriétés système, et les effacer casse la
  fonction qui les crée.
- **L'intérieur des disques virtuels `.vhdx`** (WSL, Docker Desktop). Aucun
  module n'en émet ; récupérer l'espace *dedans* passe par les commandes de
  l'hôte concerné.
- **Tout ce qui exige une élévation.** winclean tourne avec les droits de la
  session, jamais plus : les caches système, le `%TEMP%` d'autres comptes, les
  journaux d'événements sont hors de portée et ne sont pas proposés.
- **Vider la corbeille implicitement.** Cela n'arrive qu'avec `--level aggressive`
  et le module `recycle-bin`, nommément.
- **Les volumes Docker**, et la ligne `Total reclaimed space:` que Docker
  imprime : ce serait une mesure inventée à partir du texte d'un tiers.

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
- **Chemins longs** : la suppression directe passe par le préfixe `\\?\` et gère
  donc les chemins au-delà de `MAX_PATH` ; la corbeille, non (voir plus haut).
- **Estimations.** Une taille vaut `null`/`inconnu` quand elle n'a pas pu être
  mesurée. Ce n'est jamais rendu par `0` : un total incomplet est signalé comme
  tel plutôt que présenté comme exact.
- **Encodage : `--out` écrit en UTF-8, stdout suit la console.** Sur une console
  Windows en page de code héritée (cp1252), un `--json` **redirigé** sort dans
  cette page de code et un lecteur qui suppose UTF-8 échoue sur le premier
  accent. `--out <fichier>` est toujours en UTF-8 : c'est le canal à utiliser pour
  un consommateur automatique, et c'est déjà celui qu'impose une invocation
  depuis un lanceur.

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
