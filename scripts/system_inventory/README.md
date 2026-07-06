# Inventaire système (disque, machine de dev Windows)

Cet outil dresse un inventaire **en lecture seule** et **hors ligne** des traces
disque laissées par l'outillage de développement sur une machine Windows. Il ne
modifie jamais rien : aucun fichier écrit, aucune écriture registre, aucune
modification du `PATH`. Il n'existe pas de mode `--apply` / `--fix`.

**v1 est un inventaire brut.** Il n'émet aucun verdict « actif / inactif /
obsolète » : c'est explicitement hors périmètre de cette version (une v2
future pourrait détecter l'inactivité, cf. le plan parent). Ici, on affiche
simplement ce qui existe, sa taille, et sa source — à l'utilisateur de juger.

## Sources

| Source (`--source`) | Ce qu'elle rapporte | Statut |
| -------------------- | -------------------- | ------ |
| `registry` | Le registre « Programmes et fonctionnalités » (clés `Uninstall`, HKLM + HKCU, vues 32/64-bit) : nom, date d'installation, taille estimée, emplacement d'installation. | ✅ disponible (Partie 1) |
| `appdata` / `dotfolder` / `programdata` / `scoop-choco` | `%LOCALAPPDATA%` / `%APPDATA%` premier niveau, dossiers-points `%USERPROFILE%`, `%ProgramData%` premier niveau, arbres Scoop/Chocolatey — tailles **mesurées sur disque**, pas déclarées. | ✅ disponible (Partie 2) |
| `path` | Entrées `PATH` utilisateur (`HKCU\Environment`) + système (`HKLM\...\Session Manager\Environment`) : chemin brut (non expansé), origine (`user`/`system`), et un flag `alive`/`dead` selon que le dossier existe réellement sur disque (`%VAR%` non résolues comptent comme mortes). Pas de taille (`size_bytes` toujours `null`) — ce n'est pas un consommateur d'espace disque, juste une source de bruit/redondance dans le `PATH`. | ✅ disponible (Partie 3) |
| `docker-wsl` | Fichiers `.vhdx` Docker Desktop/WSL2 : glob sous `%LOCALAPPDATA%\Docker\wsl\` (racine + `data`/`disk`) et distros WSL enregistrées (`HKCU\...\Lxss\*`, `DistributionName` + `BasePath\ext4.vhdx`) — couvre aussi bien Docker Desktop que les distros installées via Microsoft Store (Ubuntu, Debian...). Taille = taille du fichier `.vhdx` sur disque (allocation sparse, pas la taille virtuelle annoncée par WSL). | ✅ disponible (Partie 3) |

### Caveat important — `registry`

La taille de chaque entrée `registry` vient de la valeur `EstimatedSize` du
registre : c'est une donnée **auto-déclarée par l'installeur de l'application**,
pas une mesure du disque. Elle peut être absente (l'élément est alors classé
en fin de liste, taille « unknown », jamais fabriquée à `0`), périmée depuis
une mise à jour, ou simplement fausse. Ne la traitez pas comme une vérité
terrain — les sources des Parties 2/3 (mesure directe via `os.stat`/parcours de
répertoire) sont plus fiables sur ce point précis, quand elles seront
disponibles.

### Caveat important — `appdata` / `dotfolder` / `programdata` / `scoop-choco`

Contrairement à `registry`, ces quatre sources mesurent la taille **sur
disque**, par parcours récursif de répertoire (`os.stat`/`os.scandir`, cf.
`common.dir_size_on_disk`) — pas de valeur auto-déclarée ici.

Seul le **premier niveau** de chaque racine est énuméré (jamais la racine
elle-même, jamais un niveau plus profond) : chaque élément listé est un
dossier immédiatement sous `%LOCALAPPDATA%`/`%APPDATA%` (`appdata`), un
dossier-point immédiatement sous `%USERPROFILE%` (`dotfolder`), un dossier
immédiatement sous `%ProgramData%` (`programdata`), ou une app/un paquet
immédiatement sous `scoop\apps`/`chocolatey\lib` (`scoop-choco`). La taille de
chaque élément reste calculée par parcours **entièrement récursif** en
dessous de ce premier niveau — seule l'énumération s'arrête au premier
niveau, pas le calcul de taille.

`programdata` exclut délibérément l'entrée `chocolatey` (comparaison
insensible à la casse) : cet arbre est déjà itemisé app par app par
`scoop-choco`, donc le sommer une seconde fois ici compterait les mêmes
octets deux fois dans le total général (cf. le registre des risques du
plan — double comptage).

`scoop-choco` est une source « présente si installée » : sur une machine sans
Scoop ni Chocolatey, elle ne contribue silencieusement aucun élément — ce
n'est pas une erreur, et aucun message n'est émis sur `stderr` dans ce cas.

### Caveat important — `path`

Chaque entrée `PATH` est rapportée **telle quelle**, non expansée (une entrée
contenant `%VARIABLE%` non résolue est conservée littéralement dans `name`/
`path`) ; seule la vérification d'existence (`alive`/`dead`) l'expanse en
interne via `os.path.expandvars`. Les entrées `user` et `system` sont
dédupliquées indépendamment (une même valeur répétée dans la même origine ne
sort qu'une fois), mais une entrée présente à l'identique dans les deux
origines apparaît deux fois (une fois par origine) — c'est volontaire : ce
sont deux emplacements distincts du registre, potentiellement désynchronisés
un jour l'un de l'autre. Aucune taille n'est calculée pour cette source.

### Caveat important — `docker-wsl` et double comptage avec `appdata`

Un `.vhdx` Docker/WSL2 peut être physiquement niché sous un dossier de
premier niveau que le scan `appdata` générique somme aussi (`Docker\wsl\...`
sous `%LOCALAPPDATA%`, ou `Packages\<PackageFamilyName>\LocalState\...` pour
une distro Microsoft Store). Quand `docker-wsl` et `appdata` sont tous deux
actifs (c'est le cas par défaut, sans `--source`), l'orchestrateur
(`inventory.py`) exécute `docker-wsl` en premier, collecte le chemin absolu
résolu de chaque élément qu'il émet, et transmet cet ensemble à
`scan_appdata(exclude_paths=...)` — ces octets précis sont alors ignorés par
le parcours récursif d'`appdata`, pour n'être comptés qu'une seule fois, sous
`docker-wsl`. En filtrant avec `--source appdata` seul (sans `docker-wsl`),
cette exclusion ne s'applique pas : le dossier `Docker`/`Packages\...\Ubuntu\...`
est alors sommé en entier par `appdata`, sans le savoir — un sur-comptage
documenté, accepté dans cette vue filtrée.

Quand un `.vhdx` de distro est atteignable à la fois par l'énumération Lxss
et par le glob générique (le cas de `docker-desktop`, dont le `BasePath` est
sous `Docker\wsl\`), un seul élément est émis — celui portant le nom de la
distro (`detail.distro`), strictement plus informatif que l'élément de glob
nu (`detail.kind: "vhdx"`).

## Utilisation

Depuis la racine du dépôt :

```powershell
python scripts/system_inventory/inventory.py
```

Affiche chaque élément trié par taille décroissante (`[source] nom — taille
(emplacement)`), suivi d'un total général.

Options :

```powershell
python scripts/system_inventory/inventory.py --json                # sortie JSON
python scripts/system_inventory/inventory.py --source registry     # restreint à une source (répétable)
python scripts/system_inventory/inventory.py --source path --source docker-wsl  # PATH + Docker/WSL uniquement
python scripts/system_inventory/inventory.py --top 20               # limite l'affichage aux 20 plus gros éléments
```

`--source` est répétable (`--source registry --source appdata`) ; omis, il
vaut « toutes les sources actuellement enregistrées » — soit, à ce stade
(toutes les parties livrées), les sept sources : `registry`, `appdata`,
`dotfolder`, `programdata`, `scoop-choco`, `path` et `docker-wsl`.

`--top N` limite le nombre d'éléments **affichés** (texte ou JSON), triés du
plus gros au plus petit. Il n'affecte jamais le total général, qui reste
toujours calculé sur l'ensemble des éléments scannés — pas seulement ceux
affichés.

### Sortie `--json`

```powershell
python scripts/system_inventory/inventory.py --json | python -m json.tool
```

émet un objet JSON unique :

```json
{
  "items": [
    {"source": "registry", "name": "...", "path": "...", "size_bytes": 123, "detail": {"...": "..."}}
  ],
  "total_bytes": 123456,
  "total_human": "120.6 KB"
}
```

`items` est le tableau trié par `size_bytes` décroissant ; `total_bytes` /
`total_human` portent le total général (toujours sur l'ensemble des éléments
scannés, cf. `--top` ci-dessus).

### Codes de sortie

- `0` : inventaire produit avec succès (y compris un inventaire vide).
- `2` : aucune source valide disponible — soit `--source` demande une source
  requérant un composant absent de la plateforme (le registre Windows via
  `winreg`, sur un système non-Windows), soit la liste de sources actives
  résolue est vide pour toute autre raison. Message explicatif sur `stderr`.
  `2` est aussi le code renvoyé par `argparse` lui-même pour un argument
  invalide (ex. `--source` avec un nom de source inconnu, `--top` négatif).

## Limites

- Windows uniquement : `winreg` est un module stdlib réservé à Windows ; sur
  toute autre plateforme, les sources `registry`, `path` et `docker-wsl` sont
  indisponibles (voir codes de sortie ci-dessus) — `appdata`, `dotfolder`,
  `programdata` et `scoop-choco` restent utilisables (aucune ne dépend de
  `winreg`).
- Deux doublons inter-sources sont gérés explicitement :
  - `programdata` exclut l'entrée `chocolatey`, déjà itemisée app par app par
    `scoop-choco` (voir le caveat `appdata`/`dotfolder`/`programdata`/
    `scoop-choco` ci-dessus) ;
  - `docker-wsl` et `appdata`, quand tous deux actifs, s'excluent
    mutuellement via `exclude_paths` (voir le caveat `docker-wsl` ci-dessus).
  Aucun autre chevauchement n'est détecté ou corrigé automatiquement — par
  exemple, un outil installé à la fois via Scoop et une trace résiduelle sous
  `%LOCALAPPDATA%` serait compté deux fois.

## Tests

```powershell
python -m unittest discover -s scripts/system_inventory/tests -v
```

Les tests de `test_inventory.py` n'accèdent jamais au registre réel : ils
injectent une source factice à la place de `scan_registry()`, afin que les
assertions sur le tri, le total et la forme JSON soient indépendantes des
programmes réellement installés sur la machine exécutant la suite. Seul
`test_registry.py` contient un test de fumée (« smoke test ») optionnel qui
appelle le vrai `scan_registry()`, gardé par `unittest.skipUnless` (Windows
uniquement).
