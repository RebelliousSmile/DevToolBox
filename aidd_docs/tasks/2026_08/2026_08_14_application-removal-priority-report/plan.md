---
objective: Ajouter à DevToolBox un rapport natif multi-OS, explicable et strictement en lecture seule qui classe les applications à désinstaller selon leur empreinte disque et leur ancienneté d’usage estimée.
status: implemented
---

# Plan

## Overview

DevToolBox doit faire apparaître dans son interface native une nouvelle vue « Applications » qui aide à arbitrer les désinstallations sans en déclencher. Le rapport agrège les applications visibles par les mécanismes d’installation de chaque OS, estime leur empreinte et leur dernier usage, applique des protections explicites, puis produit un classement justifié avec une commande de désinstallation uniquement copiable.

La logique métier reste commune aux plateformes : schéma JSON versionné, score déterministe, niveaux de confiance, raisons du classement, exclusions et protections. Seuls les collecteurs et le suivi des processus sont spécifiques à Linux et Windows. Une absence de date d’usage ne doit jamais être assimilée à une longue inactivité, une dépendance ou un runtime partagé ne doit jamais devenir un candidat prioritaire, et une taille installée ne doit pas être présentée comme un gain garanti lorsqu’elle inclut des éléments partagés.

Le premier relevé exploitera les signaux déjà accessibles et indiquera leur confiance. Une fois ce relevé chargé, il fournira au suivi natif une liste bornée d’identifiants applicatifs et de chemins exécutables attendus ; DevToolBox pourra alors constituer un historique local et prospectif des seules applications connues, sans réseau ni télémétrie. La collecte et le calcul s’exécuteront hors du thread d’interface ; une source indisponible produira un rapport partiel avec une erreur localisée plutôt qu’un échec global.

## Phases

1. [Contrat de rapport et moteur de classement](phase-1.md) — définir le modèle multi-OS, les invariants de sûreté, le score explicable, l’historique tolérant aux erreurs et une CLI JSON en lecture seule.
2. [Collecteurs d’applications Linux](phase-2.md) — inventorier les applications APT/dpkg exposées par des lanceurs de bureau, les applications Snap et Flatpak, sans classer les bibliothèques ni les runtimes.
3. [Collecteurs d’applications Windows](phase-3.md) — inventorier les entrées de désinstallation, MSIX/AppX, Scoop et Chocolatey en filtrant les composants système et les frameworks.
4. [Historique local d’usage multi-OS](phase-4.md) — observer périodiquement les exécutables actifs, les rapprocher des identifiants applicatifs et persister un historique machine local.
5. [Vue native Applications et pont asynchrone](phase-5.md) — charger le rapport hors du thread UI et afficher résumé, filtres, classement, preuves, protections et commande copiable dans egui.
6. [Intégration, documentation et validation multi-OS](phase-6.md) — verrouiller le contrat de bout en bout, documenter les limites et exécuter les validations disponibles sur Linux et les contrôles de portabilité Windows.

## Resources

- [Flatpak command reference](https://docs.flatpak.org/en/latest/flatpak-command-reference.html) — confirme `flatpak list --app`, les colonnes de taille et d’installation, ainsi que la distinction entre applications et runtimes.
- [Snap get started](https://snapcraft.io/docs/tutorials/get-started/) — confirme les informations exposées par `snap list` et la gestion des paquets installés.
- [Snap revisions](https://snapcraft.io/docs/revisions/) — précise les révisions conservées et évite de confondre empreinte installée et espace immédiatement récupérable.
- [Get-AppxPackage](https://learn.microsoft.com/en-us/powershell/module/appx/get-appxpackage?view=windowsserver2025-ps) — confirme l’inventaire MSIX/AppX et les types de paquets, notamment Main et Framework.
- [EnumProcesses](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-enumprocesses) — confirme l’énumération native des identifiants de processus sous Windows.
- [QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew) — confirme la récupération du chemin exécutable à partir d’un processus Windows accessible.
- [proc_pid_exe(5)](https://www.man7.org/linux/man-pages/man5/proc_pid_exe.5.html) — confirme que `/proc/<pid>/exe` expose le chemin de l’exécutable actif sous Linux, sous réserve des permissions.

## Decisions

- Le rapport est strictement en lecture seule : aucun bouton, appel système ou chemin de code ne lance une désinstallation. La commande proposée reste une chaîne affichée et copiable.
- Le score v1 additionne deux composantes de 0 à 50, sans multiplicateur caché de confiance. Empreinte : moins de 250 Mio ou inconnue = 0, 250 Mio–1 Gio = 10, 1–5 Gio = 25, 5–10 Gio = 35, au moins 10 Gio = 50. Inactivité couverte : moins de 30 jours actifs ou inconnue = 0, 30–89 = 10, 90–179 = 25, 180–364 = 40, au moins 365 = 50. Score, raisons et confiance restent séparés et une protection annule l’éligibilité.
- Les protections priment sur le score. Composants système, frameworks, runtimes, dépendances partagées et paquets non supprimables sont exclus du classement prioritaire ou clairement marqués comme protégés.
- Sous Linux, les paquets APT/dpkg candidats partent des entrées de bureau et de leur exécutable propriétaire, afin de classer des applications utilisateur plutôt que toute la base de paquets.
- Chaque taille conserve sa méthode de mesure, son périmètre et sa confiance. Les tailles sont libellées comme empreintes installées ; le gain récupérable n’est annoncé que lorsqu’il est mesurable sans double comptage, et les révisions, données utilisateur ou ressources partagées restent distinctes.
- Le suivi d’usage est local, prospectif et opportuniste pendant l’exécution de DevToolBox. Il ne démarre qu’après réception de cibles issues d’un rapport, échantillonne à fréquence bornée et conserve par application uniquement `tracked_since` et `last_seen`, plus un compteur global d’échantillons réussis par jour sur 400 jours. L’inactivité est exprimée en jours réellement couverts, jamais en simple temps calendaire pendant lequel DevToolBox était arrêté.
- Le paquet Python `scripts/app_recommendations` porte la collecte et le classement multi-OS ; le code Rust porte l’observation native des processus, l’orchestration asynchrone et la vue egui.
- Le pont Rust réutilise une résolution commune de la racine DevToolBox et de l’interpréteur (`.venv`, `DEVTOOLBOX_PYTHON`, puis PATH), lance le module depuis la racine et affiche une indisponibilité explicite si Python ou les scripts ne sont pas livrés.
- Un élément protégé n’expose aucune commande copiable. Les commandes des gestionnaires sont construites par adaptateur avec des arguments échappés ; une `UninstallString` du registre reste une chaîne éditeur signalée comme non vérifiée et n’est jamais interpolée dans un shell.
- Les collecteurs échouent indépendamment. Le rapport conserve les résultats valides et décrit les sources indisponibles ou partielles.
- Chaque collecteur possède un délai maximal ; chaque rafraîchissement porte un identifiant de génération afin qu’une réponse ancienne ou tardive ne remplace jamais le rapport le plus récent.
- Un indice d’usage doit identifier l’exécutable propre à l’application. Un wrapper partagé comme `flatpak`, `snap`, PowerShell ou un lanceur générique n’est jamais rapproché seul ; sans indice spécifique fiable, le dernier usage reste inconnu.
- La compatibilité Windows sera couverte sur Linux par des fixtures, des tests de parsing et des gardes de compilation ; une validation finale sur une machine Windows reste nécessaire avant de déclarer la fonctionnalité portable en production.
