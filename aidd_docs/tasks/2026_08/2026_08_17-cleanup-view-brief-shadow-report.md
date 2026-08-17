---
source: aidd_docs/tasks/2026_08/2026_08_17-cleanup-view-brief.md
generated_at: 2026-08-17
status: clean
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_08/2026_08_17-cleanup-view-brief.md`
Generated: `2026-08-17`
Updated: `2026-08-17` — all gaps closed, answers folded into the brief (section « Décisions »).

Total gaps: 9 | Blocker: 0 | Major: 0 | Minor: 0 | Closed: 9

---

## Closed gaps

### unstated assumption

**[closed]** Is every module the `--json` report lists an actual `--apply` deletion target at level safe, or are the per-user caches (uv, pnpm, npm, pip, cargo, bun) informational discovery that `--only <module> --apply` would not remove?
> les caches globaux par utilisateur (uv, pnpm, npm, pip, cargo, bun) comme les artefacts sous les racines […] Chaque ligne peut être nettoyée individuellement

Résolution (fait vérifié, `registry_mod.py`) : oui — tous les modules cache sont `Level.SAFE`, découverte fixe, cibles réelles de `--only <module> --apply`. `pnpm-store` exige le binaire pnpm. `needs_network` est exposé dans le JSON.

**[closed]** Where does the output of a per-module clean go — streamed into the existing integrated Terminal view, or captured and summarized inside the Nettoyage view itself?
> puis `--only <module> --apply`

Résolution (décision) : la vue lance `--only X --apply --json` et consomme le payload `run` elle-même ; pas de streaming vers le Terminal. Les cartes Actions gardent leur sortie texte.

### ambiguous term

**[closed]** Does « tout le plan winclean » include moderate-level modules in the displayed list even though cleaning is safe-only, and if so are their rows shown without a clean button?
> Section Bibliothèques = **tout le plan winclean** par module

Résolution (décision) : Analyser lance `--json --level moderate` (plan seul) ; lignes moderate grisées sans bouton, pilotées par le champ `level` des candidats. Aggressive exclu.

### missing edge case

**[closed]** What happens when « Analyser » or a per-module clean is triggered while another command is already running, given the app allows a single running action at a time (`action_running`)?
> bouton « Analyser » lançant `clean.py --json` en arrière-plan

Résolution (décision) : slot unique `action_running` réutilisé ; boutons désactivés tant qu'occupé.

**[closed]** How does the Bibliothèques section render a tool that is not installed or whose cache path does not exist — zero-size row, hidden row, or disabled row?
> listant, module par module, tout ce que winclean sait nettoyer

Résolution (fait vérifié) : outil absent → aucun candidat émis → aucune ligne. La vue affiche ce que le JSON renvoie ; `unpriced_modules` pour une mention « non mesurable ».

### missing failure mode

**[closed]** What does the view show when `clean.py --json` exits non-zero, times out, or emits unparsable JSON?
> bouton « Analyser » lançant `clean.py --json` en arrière-plan (spinner pendant le scan)

Résolution (décision) : bandeau rouge « Analyse échouée (code N) » + dernières lignes stderr + bouton Réessayer ; même traitement pour JSON invalide ; tailles précédentes marquées obsolètes.

**[closed]** How does the view report a partial failure of `--only <module> --apply` (locked files, leftover bytes), and is the module size re-measured afterwards?
> Chaque ligne peut être nettoyée individuellement

Résolution (fait vérifié + décision) : le payload `run` expose `freed`, `failed`, `measured`, `locked_paths`, `operation_failures` ; la ligne affiche « X libérés, Y en échec » et sa taille est rafraîchie depuis `measured` sans réanalyse complète.

### missing acceptance criterion

**[closed]** What observable outcome marks a module row as successfully cleaned — size re-measured near zero, row removed, or a per-run summary line?
> Nettoyage par ligne : niveau **safe uniquement**, précédé d'un dialogue de confirmation UI

Résolution (décision) : la ligne reste, taille mise à jour depuis `measured`, badge « Nettoyé : X libérés » jusqu'à la prochaine analyse ; succès = `failed == 0`.

### missing dependency

**[closed]** Are the Linux winclean modules (`mod_linux_*`) exposed through the same `--only` names and the same `--json` shape as the Windows ones, so the view stays OS-agnostic?
> La brique commune — et multi-OS — est `scripts/winclean`

Résolution (fait vérifié + décision) : mêmes `--only`/`--json`, noms distincts (`pip-cache-linux`, `pnpm-store-linux`, `apt-cache`) ; la vue ne code aucun nom en dur et repasse au script les noms lus dans le JSON.
