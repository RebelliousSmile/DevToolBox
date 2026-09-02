# Release readiness — DevToolBox 0.10+

Le code et les workflows peuvent être marqués « implémentés » sans qu'une release soit
qualifiée. Une release stable reste un draft tant que toutes les lignes ci-dessous ne
sont pas accompagnées d'une preuve datée. La commande hors secrets est :

```sh
python scripts/verify-package-config.py
python scripts/verify-release-config.py
python scripts/generate-update-manifest.py --self-test
python scripts/verify-release-manifest.py --self-test
```

## Responsabilités et portes

| Porte | Responsable | Preuve attendue | État initial | Message en cas d'absence |
| --- | --- | --- | --- | --- |
| Approbation produit et rendu | Propriétaire du dépôt | captures clair/sombre et décision signée | à qualifier | conserver le draft |
| Signature et publication | Mainteneur de release | journaux CI, empreintes des clés, checksums | à qualifier | aucun asset stable |
| Matrice native | Opérateur QA | fiche par OS, architecture et serveur d'affichage | à qualifier | matrice incomplète |
| Apple | Mainteneur de release | Developer ID, notarisation, ticket agrafé | externe | DMG non publiable |
| Windows | Mainteneur de release | Authenticode et horodatage vérifiés | externe | NSIS non publiable |
| Updater | Mainteneur de release | double signature en rotation, recovery vérifiée | externe | updater désactivé ou draft |

L'environnement GitHub `production-release` doit imposer une approbation humaine. Il
contient `UPDATE_PUBLIC_KEYS_JSON`, `UPDATE_PRIVATE_KEYS_JSON`,
`MINISIGN_PRIVATE_KEY`; `QA_EVIDENCE_SHA256` désigne le dossier de preuves archivé et
`NATIVE_QUALIFICATION_COMPLETE=true` n'est posé qu'après validation des signatures OS.
Les clés privées restent hors dépôt. Les certificats Apple et
Authenticode, leur horodatage, leur révocation et les alertes d'expiration à J-90 et
J-30 sont sous la responsabilité du mainteneur de release.

## Matrice de qualification

- macOS 13+ sur arm64 et Intel : installation DMG, premier rendu, vibrancy puis
  fallback opaque, update, récupération et retrait en conservant les données.
- Windows 11 23H2+ x64 : installation/désinstallation NSIS par utilisateur, Mica
  puis fallback opaque, élévation expliquée, update et réinstallation de secours.
- Ubuntu 22.04 et 24.04 x64 : deb sous X11/Wayland et AppImage, présence/absence de
  FUSE, vérification Minisign, update AppImage, rollback `.previous`, retrait.
- Chaque cible : thèmes clair/sombre, réduction des animations et transparence,
  navigation clavier, contraste, aucune zone illisible.

Les mesures requises par `docs/visual-contract.md` sont : premier frame, frame-time
des transitions et CPU au repos. Elles sont jointes aux captures et à la version du
matériel. Une porte absente bloque uniquement la publication stable, jamais les tests
hors secrets ni l'état « implemented » du plan.

## Ordre de publication et récupération

1. Construire les cinq paquets dans des artefacts CI privés.
2. Créer la release en draft, signer et vérifier les octets, puis joindre les paquets.
3. À partir de 0.11.0, récupérer et revérifier les payloads 0.10.0 de secours.
4. Générer et comparer `latest.json`, puis le téléverser en dernier.
5. Publier seulement après approbation de l'environnement et de la matrice QA.

0.10.0 est la première installation compatible avec l'updater : son installation
initiale et toute migration depuis 0.9.x restent manuelles. Après une fenêtre de
rotation manquée (deux versions mineures ou 180 jours), réinstaller un paquet signé.

## Qualification locale Windows — 2 septembre 2026

La version `0.10.0` a été construite en NSIS x64 puis désinstallée et réinstallée
silencieusement sur Windows 11. L'ancien désinstalleur et le nouveau désinstalleur
contrôlé ont chacun retourné `0`; l'entrée « Applications installées », le binaire et
`uninstall.exe` ont été recréés. Avant le premier lancement, les trois empreintes
prises avant retrait étaient inchangées après migration vers
`%LOCALAPPDATA%\RebelliousSmile\DevToolBox` :

| Fichier | SHA-256 conservé |
| --- | --- |
| `application-usage.json` | `A246D55491A28CC5A7CD4A863255C4D3AA4DB8E41DE466758E7D45A2FD9FD0F2` |
| `devtoolbox.log` | `60B54275CF328FA911ECD98E12785F58F8D7F2D24A90DC7896C8B0CD2B7950D4` |
| `devtoolbox.log.old` | `0F6A8A2F66561425A8008BB449ADD8AAD3B3E4327D983436A16DF4A2CE71BC0E` |

Le pilote NVIDIA de la machine expose DX12, mais WGPU signale qu'aucun
`CompositeAlphaMode` transparent n'est disponible. eframe 0.35 n'expose pas cette
capacité au code applicatif : demander une fenêtre transparente rendait la fenêtre
maximisée invisible. Windows utilise donc le repli opaque tant que ce support ne peut
pas être prouvé. Les captures [claire](../aidd_docs/tasks/2026_09/2026_09_02-windows-theme-rendering-fix/evidence/windows-light-opaque.png)
et [sombre](../aidd_docs/tasks/2026_09/2026_09_02-windows-theme-rendering-fix/evidence/windows-dark-opaque.png)
montrent des palettes cohérentes. Les interactions Compose et Docker sont couvertes
par respectivement 33 et 96 tests ciblés ; la largeur des tableaux est recalculée au
redimensionnement. Mica reste non qualifié sur ce matériel, et cette preuve locale ne
qualifie ni macOS ni Linux.
