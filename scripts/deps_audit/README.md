# Audit des dépendances obsolètes

Ce script propose au nettoyage les librairies et outils de code qui ne sont plus
référencés par les sources. Il est **indicatif** : il ne modifie aucun fichier et ne
contacte jamais le réseau. À vous de décider ce qui peut réellement être retiré.

Deux écosystèmes sont inspectés depuis la racine du projet :

- **Rust** : les crates déclarées dans `Cargo.toml` (`dependencies`, `dev-dependencies`,
  `build-dependencies`, y compris les tables `target.*`) sont comparées aux identifiants
  présents dans les fichiers `*.rs`.
- **Python** : pour chaque `scripts/*/requirements.txt`, les paquets déclarés sont comparés
  aux modules importés par les `*.py` voisins.

## Utilisation

Depuis n'importe quel dossier du dépôt :

```powershell
python scripts/deps_audit/audit.py
```

Options :

```powershell
python scripts/deps_audit/audit.py --json      # sortie JSON
python scripts/deps_audit/audit.py --check     # code de sortie 1 si des candidats existent
python scripts/deps_audit/audit.py --project-root C:\chemin\DevToolBox
```

`--check` est destiné à une intégration continue : le script échoue (code `1`) dès qu'une
dépendance semble inutilisée, `0` sinon, `2` si aucun `Cargo.toml` n'est trouvé.

## Limites

L'analyse est statique et par correspondance de nom. Une crate utilisée uniquement via une
macro, une feature, ou un chemin dynamique peut être signalée à tort. Vérifiez toujours une
entrée avant de la supprimer.

## Tests

```powershell
python -m unittest discover -s tests -v
```
