# Rapport de priorité de suppression

`scripts.app_recommendations` produit un rapport local, en lecture seule, destiné à
la vue **Applications** de DevToolBox. Il classe des applications pour aider à
l'arbitrage ; il ne désinstalle rien. Les commandes proposées sont des textes à
relire puis à copier volontairement. DevToolBox ne les transmet jamais à un shell.

## Exécution

Depuis la racine du paquet livré :

```bash
python3 -m scripts.app_recommendations --json --history /chemin/usage.json
```

`stdout` contient exclusivement le JSON de schéma `1`. Les erreurs globales vont
sur `stderr`; une source locale indisponible devient une entrée `source_errors` et
n'empêche pas les autres sources d'être rendues. Il n'existe aucune option de
suppression. Le runtime Rust cherche la racine via `DEVTOOLBOX_HOME`, puis parmi
les ancêtres du répertoire courant et du binaire. La distribution doit donc
conserver le dossier `scripts/app_recommendations` avec ses dépendances
`scripts/system_inventory`.

## Sources locales par OS

| OS | Sources | Mesure principale | Limite importante |
|---|---|---|---|
| Linux | entrées `.desktop` reliées à APT/dpkg, Snap, Flatpak | taille déclarée du paquet, archive Snap active ou taille Flatpak | données utilisateur, dépendances et runtimes partagés généralement exclus |
| Windows | clés Uninstall 32/64 bits HKCU/HKLM, MSIX/AppX, Scoop, Chocolatey | taille déclarée ou répertoire d'installation/gestionnaire | déclaration éditeur parfois absente ou approximative |

La collecte n'utilise pas le réseau. Une source manquante ou expirée reste visible
comme erreur partielle.

## Score v1 et confiance

Le score est la somme de deux axes, chacun plafonné à 50. Il exprime une
**priorité d'examen**, jamais une certitude.

| Empreinte installée | Points |
|---|---:|
| inconnue ou < 250 Mio | 0 |
| 250 Mio à < 1 Gio | 10 |
| 1 à < 5 Gio | 25 |
| 5 à < 10 Gio | 35 |
| ≥ 10 Gio | 50 |

| Jours d'usage effectivement couverts | Points |
|---|---:|
| inconnus ou < 30 | 0 |
| 30 à < 90 | 10 |
| 90 à < 180 | 25 |
| 180 à < 365 | 40 |
| ≥ 365 | 50 |

Les jours couverts ne sont pas le temps calendaire écoulé : un jour compte
uniquement si au moins un échantillon local de processus a réussi après le début
du suivi ou le dernier usage observé. L'historique est donc prospectif et
opportuniste. Une application jamais observée pendant 90 jours calendaires mais
avec seulement 12 jours échantillonnés a une couverture de 12 jours, pas 90.

La confiance du candidat est la plus faible de ses preuves de taille et d'usage.
Une taille inconnue ou un historique sans couverture ne devient jamais une preuve
d'inactivité.

## Empreinte et espace récupérable

`installed_bytes` est l'empreinte attribuée à l'application selon la méthode
indiquée. `reclaimable_bytes` est une estimation distincte du gain réellement
récupérable et reste souvent `null`. L'empreinte ne doit pas être lue comme une
promesse de libération : fichiers partagés, runtimes, dépendances, caches et données
utilisateur peuvent modifier fortement le résultat final.

## Protections et confidentialité

Les composants système, dépendances APT automatiques, bases Snap, frameworks ou
ressources MSIX, mises à jour, drivers et runtimes partagés détectés sont protégés :
score forcé à zéro et commande supprimée. Une chaîne Uninstall du registre est
étiquetée `publisher_unverified`; une commande construite depuis un gestionnaire
connu est `manager_verified`. Même vérifiée, elle reste une aide manuelle.

L'historique enregistre uniquement, par identifiant d'application, `tracked_since`
et `last_seen`, plus un compteur agrégé par jour couvert. Il ne conserve ni liste
de processus chronologique, ni durée d'utilisation, ni document ouvert.

## Validation portable

Les fixtures Linux et Windows testent les parseurs sur toute plateforme, mais ne
remplacent pas une validation Windows réelle. Avant une livraison portable, rejouer
sur Windows : registre 32/64 bits, MSIX protégé/non protégé, Scoop/Chocolatey
présents et absents, historique sous `%LOCALAPPDATA%`, collecte de processus, vue
native et copie de commande sans exécution.
