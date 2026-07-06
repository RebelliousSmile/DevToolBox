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
| `appdata` / `dotfolder` / `programdata` / `scoop-choco` | `%LOCALAPPDATA%` / `%APPDATA%` premier niveau, dossiers-points `%USERPROFILE%`, `%ProgramData%` premier niveau, arbres Scoop/Chocolatey — tailles **mesurées sur disque**, pas déclarées. | 🚧 prévu Partie 2 |
| `path` / `docker-wsl` | Entrées `PATH` (utilisateur + système, avec détection des entrées mortes), fichiers `.vhdx` Docker/WSL2 et distros WSL enregistrées. | 🚧 prévu Partie 3 |

### Caveat important — `registry`

La taille de chaque entrée `registry` vient de la valeur `EstimatedSize` du
registre : c'est une donnée **auto-déclarée par l'installeur de l'application**,
pas une mesure du disque. Elle peut être absente (l'élément est alors classé
en fin de liste, taille « unknown », jamais fabriquée à `0`), périmée depuis
une mise à jour, ou simplement fausse. Ne la traitez pas comme une vérité
terrain — les sources des Parties 2/3 (mesure directe via `os.stat`/parcours de
répertoire) sont plus fiables sur ce point précis, quand elles seront
disponibles.

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
python scripts/system_inventory/inventory.py --top 20               # limite l'affichage aux 20 plus gros éléments
```

`--source` est répétable (`--source registry --source appdata` une fois la
Partie 2 livrée) ; omis, il vaut « toutes les sources actuellement
enregistrées » — soit uniquement `registry` pour l'instant.

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
  toute autre plateforme, la source `registry` est indisponible (voir codes
  de sortie ci-dessus).
- Aucune notion de doublon inter-sources n'est encore gérée dans cette
  Partie 1 (une seule source existe) ; les Parties 2/3 introduiront
  `exclude_paths` pour éviter qu'un même `.vhdx` Docker/WSL ne soit compté à
  la fois par sa propre source et par un scan `appdata` générique.

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
