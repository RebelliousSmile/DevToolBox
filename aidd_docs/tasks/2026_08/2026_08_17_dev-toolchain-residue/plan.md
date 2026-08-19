---
objective: "La vue Nettoyage inventorie les versions d'outillage de développement et les paquets des gestionnaires d'installation, et en désinstalle celles que l'utilisateur coche, via l'outil natif de chaque éditeur."
status: pending
---

# Plan: Résidus d'outillage de développement

## Overview

| Field      | Value                                                                                                                                                            |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Goal**   | Récupérer l'espace pris par les versions d'outillage inactives et les caches de gestionnaires, sans jamais supprimer un fichier appartenant à un éditeur tiers.     |
| **Source** | Brainstorm du 2026-08-17 « contenus Windows obsolètes » recadré en résidus d'outillage de développement, plus la demande « ajouter choco, winget et tous les autres gestionnaires d'installation ». |

## Phases

| #   | Phase                                      | File                         |
| --- | ------------------------------------------ | ---------------------------- |
| 1   | Inventaire lecture seule de l'outillage    | [`phase-1.md`](./phase-1.md) |
| 2   | Caches de gestionnaires manquants          | [`phase-2.md`](./phase-2.md) |
| 3   | Module winclean de désinstallation déléguée | [`phase-3.md`](./phase-3.md) |
| 4   | Section « Outillage installé » de la vue    | [`phase-4.md`](./phase-4.md) |
| 5   | Élévation explicite pour le périmètre machine | [`phase-5.md`](./phase-5.md) |

## Resources

| Source                                                                                            | Verified                                                                                                                          |
| ------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| https://rust-lang.github.io/rustup/environment-variables.html                                      | Les toolchains vivent sous `%USERPROFILE%\.rustup\toolchains\<nom>` ; aucune élévation documentée, tout est dans le profil.          |
| https://github.com/rust-lang/rustup/blob/master/src/cli/rustup_mode.rs                              | `rustup toolchain list` / `rustup toolchain uninstall <nom>` ; **aucun `--json` dans toute la CLI** — la sortie doit être parsée.    |
| https://learn.microsoft.com/en-us/dotnet/core/additional-tools/uninstall-tool-overview              | `dotnet-core-uninstall` est le seul outil supporté pour retirer un SDK .NET, et il **exige des droits administrateur**.              |
| https://learn.microsoft.com/en-us/dotnet/core/additional-tools/uninstall-tool-cli-remove            | `remove --sdk <version>`, `--all-but`, `--dry-run`, `--yes` ; ne gère pas les SDK posés par le VS Installer 16.4+.                   |
| https://learn.microsoft.com/en-us/dotnet/core/install/remove-runtime-sdk-versions                   | `dotnet --list-sdks` / `--list-runtimes` est la source d'inventaire ; supprimer le dossier `dotnet` à la main casse Visual Studio.   |
| https://learn.microsoft.com/en-us/nuget/consume-packages/managing-the-global-packages-and-cache-folders | Quatre caches NuGet distincts et leurs variables ; `dotnet nuget locals <dossier> --clear` ; aucun élagage sélectif natif.       |
| https://docs.gradle.org/current/userguide/directory_layout.html                                    | `~/.gradle` avec purge automatique (30 j / 7 j) ; **aucune commande CLI de purge** — seulement le réglage de rétention.              |
| https://maven.apache.org/plugins/maven-dependency-plugin/purge-local-repository-mojo.html          | `dependency:purge-local-repository` est projet-scopé et re-télécharge par défaut ; ce n'est pas un videur de cache global.           |
| https://github.com/coreybutler/nvm-windows                                                          | `nvm list` / `nvm uninstall <version>` ; **élévation requise** (création de liens symboliques).                                      |
| https://docs.volta.sh/reference/list                                                                | `volta list --format plain` : seule sortie lisible machine de tout le lot. `volta uninstall` ne prend pas de version.                |
| https://learn.microsoft.com/en-us/windows/package-manager/winget/uninstall                          | `winget uninstall --id --version --product-code --silent --scope` retire aussi les applications non installées par winget.           |
| https://learn.microsoft.com/en-us/windows/package-manager/winget/                                   | L'index complet des commandes ne contient **ni `cache` ni `clean`** : winget n'a aucune purge de cache.                              |
| https://github.com/chocolatey/choco/blob/develop/src/chocolatey/infrastructure.app/commands/ChocolateyCacheCommand.cs | `choco cache remove` ne touche que le cache HTTP, jamais le cache de téléchargement (`cacheLocation`).       |
| https://github.com/chocolatey/choco/blob/develop/src/chocolatey/infrastructure.app/ApplicationParameters.cs | Cache HTTP utilisateur `%USERPROFILE%\.chocolatey\http-cache`, système `ProgramData\chocolatey\HttpCache` ; `lib-bad` / `lib-bkp`. |
| https://github.com/ScoopInstaller/Scoop/blob/master/libexec/scoop-cleanup.ps1                       | `scoop cleanup <app|*> -k` élague les anciennes versions et le cache périmé ; `scoop cache rm` vide tout.                            |
| https://learn.microsoft.com/en-us/visualstudio/install/disable-or-move-the-package-cache            | Le cache VS ne se vide de façon supportée que par `vs_installer.exe --nocache`, jamais à la main.                                    |
| https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/remove-windows-app-sdk-versions      | Le Windows App SDK est MSIX : `get-appxpackage` / `remove-appxpackage`, invisible dans Applications et fonctionnalités.              |
| https://learn.microsoft.com/en-us/windows/security/identity-protection/user-account-control/how-it-works | Un processus déjà lancé ne peut pas être élevé : l'élévation crée toujours un **nouveau** processus.                            |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/start-process   | `-Verb RunAs` exclut `-RedirectStandardOutput` mais autorise `-Wait -PassThru` : code de sortie récupérable, stdout non.             |
| https://learn.microsoft.com/en-us/windows/win32/msi/uninstall-registry-key                          | Seule surface documentée par Microsoft pour énumérer les JDK et Windows SDK installés.                                              |

## Decisions

| Decision                                                                                                                    | Why                                                                                                                                                                                                     |
| --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| L'inventaire par version vit dans `system_inventory`, la désinstallation dans `winclean`.                                     | L'énumération est lecture seule par nature, la désinstallation ne l'est pas. C'est la répartition que `winclean-separate-package.md` décrit : `winclean` importe `system_inventory` comme couche de découverte et garde tout le destructeur.                |
| La phase 1 est livrée et validée **seule**, avant que quoi que ce soit de `winclean` ne s'y appuie.                           | Le gate `git diff --quiet HEAD -- scripts/system_inventory` dit qu'un lot `winclean` ne doit pas faire bouger l'inventaire. Étendre `system_inventory` reste légitime — c'est sa raison d'être — mais pas dans le même lot que les phases 2 à 5, sinon le gate ne prouve plus rien. Il est revérifié sur le lot 2-5. |
| La phase 5 n'a pas de validation automatique : elle est recettée à la main.                                                   | Le dialogue UAC exige un humain ; aucun test ne peut accorder ni refuser un consentement. Ses critères d'acceptation sont donc une procédure de recette, et le plan le dit plutôt que de laisser croire à une couverture.                                    |
| Un cache n'obtient son propre module que s'il n'est pas déjà un candidat de `user-temp`.                                      | `discover_user_temp` énumère chaque entrée de premier niveau de `%TEMP%` sans exclusion. Un module ancré sur un sous-dossier de `%TEMP%` entre en égalité de chemin exacte avec lui et gagne par rang `MODULE_ORDER`, ce qui ferait glisser ce contenu de `moderate` à `safe` en silence. Les trois candidats concernés — WinGet, NuGetScratch, chocolatey — sont donc écartés.                                          |
| Un `tool=` n'est déclaré que si sa commande sort un chemin absolu nu, non interactif.                                         | `_ask_tool` retient la dernière ligne de stdout et exige `is_absolute()`. Mesuré sur cette machine : `dotnet nuget locals http-cache --list` la préfixe de `http-cache: `, et `choco config get` demande une confirmation. Les deux rendraient `None` — l'un immédiatement, l'autre après 30 s de timeout.                                          |
| Aucun fichier d'éditeur tiers n'est supprimé : chaque retrait passe par la CLI native de l'outil.                             | `chocolatey\lib`, `WinGet\Packages`, `Program Files\dotnet` et `Package Cache` sont des arbres vivants dont la suppression manuelle casse l'outil ou sa réparation ; aucun éditeur n'endosse ce geste.     |
| Les éléments par version sont des candidats **sans chemin**, identifiés par `resource_id`, sur le modèle de `ollama-models`.  | L'agrégation Rust `module_rows` regroupe par module ; une ligne par version exige un identifiant propre, et le contrat `Candidate.path: Option<String>` le prévoit déjà.                                    |
| La section UI a son propre client Rust (`src/toolchains/`) au lieu d'étendre `src/cleanup/rows.rs`.                           | `module_rows` somme par module et perdrait la granularité par version ; `src/applications/` est le précédent d'un second client autonome sur la même vue.                                                 |
| winclean n'élève jamais son propre processus : c'est l'application qui relance un enfant élevé, et l'enfant écrit son JSON dans un fichier. | Un processus lancé ne peut pas gagner de privilèges, et `-Verb RunAs` interdit la redirection de stdout. Le fichier de sortie est le seul canal de retour fiable.                                |
| Le non-objectif « aucune élévation » du README winclean est amendé, pas contourné en silence.                                 | Le README promet aujourd'hui que winclean tourne toujours avec les droits de la session ; livrer l'élévation sans corriger cette phrase laisserait une garantie fausse dans la documentation publiée.       |
| Rien n'est présélectionné : l'inventaire montre la preuve, l'utilisateur coche.                                               | Les retraits sont irréversibles hors re-téléchargement, et aucune heuristique d'inactivité ne distingue de façon fiable une version oubliée d'une version gardée exprès.                                   |
