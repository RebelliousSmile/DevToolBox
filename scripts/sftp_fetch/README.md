# Récupération de fichiers par SFTP

Ce script télécharge uniquement les chemins déclarés dans un fichier YAML. Il accepte
des fichiers et, avec `recursive: true`, des dossiers complets.

## Installation

Depuis ce dossier :

```powershell
python -m venv .venv
.venv\Scripts\Activate.ps1
python -m pip install -r requirements.txt
Copy-Item config.example.yaml config.yaml
```

Adaptez `config.yaml`, puis placez le secret dans une variable d'environnement :

```powershell
$env:SFTP_KEY_PASSPHRASE = "secret"
python sftp_fetch.py config.yaml --dry-run
python sftp_fetch.py config.yaml
```

Chaque entrée détaillée peut recevoir un `name`. Ce nom permet de ne récupérer qu'un
bloc, ou plusieurs blocs en répétant l'option :

```powershell
python sftp_fetch.py config.yaml --only pro
python sftp_fetch.py config.yaml --only pro --only perso
```

Sans `--only`, toutes les entrées sont traitées. Les dossiers nommés `_code` sont
exclus des parcours récursifs par défaut. Pour les inclure explicitement :

```powershell
python sftp_fetch.py config.yaml --only pro --include-code
```

Les dossiers `.git` sont toujours exclus. Lorsqu'un `.gitignore` est rencontré, ses
règles (y compris les négations `!` et les règles imbriquées) sont utilisées pour
écarter les répertoires ignorés. Les fichiers ordinaires visés par un `.gitignore`
restent téléchargés : ce filtrage concerne uniquement les répertoires.

Pour une authentification par mot de passe, utilisez `password_env: SFTP_PASSWORD`
dans le YAML et définissez `$env:SFTP_PASSWORD`. Ne placez pas le secret dans le YAML.

Le serveur doit déjà être présent dans `known_hosts`. Par défaut, un fichier local
existant n'est jamais remplacé. Activez `overwrite: true` si ce comportement est voulu.

Avec `overwrite: true` et `skip_unchanged: true`, un fichier de même taille et de même
date que sa version distante n'est pas retransféré. La date distante est conservée après
chaque téléchargement. `parallel_downloads` règle le nombre de connexions simultanées
(4 par défaut, maximum 16). Commencez avec 4 et réduisez cette valeur si le serveur
limite le nombre de connexions par utilisateur.

Codes de sortie : `0` en cas de succès, `1` si au moins un téléchargement échoue,
et `2` pour une erreur de configuration ou de connexion.

## Tests

Les tests sont isolés du réseau et utilisent un serveur SFTP en mémoire :

```powershell
python -m unittest discover -s tests -v
```
