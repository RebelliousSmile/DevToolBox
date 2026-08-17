---
source: aidd_docs/tasks/2026_08/2026_08_17-preferences-config-editor-spec.md
generated_at: 2026-08-17
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_08/2026_08_17-preferences-config-editor-spec.md`
Generated: `2026-08-17`

Total gaps: 15 | Blocker: 4 | Major: 7 | Minor: 4

---

## Warnings

- La source est la requête affinée post-brainstorm/challenge (texte rédigé cette session), pas un artefact pré-existant : les gaps ci-dessous ont été vérifiés contre l'état réel du code (`src/storage/*.rs`, `src/ui/egui_app.rs`) plutôt que déduits du seul texte.
- `src/storage/commands.rs` n'expose aujourd'hui que `toggle_favorite` — aucune fonction `add_command`/`update_command`/`remove_command` n'existe, malgré le CRUD complet demandé (voir *missing dependency*).
- Le champ `icon` (catégories comme actions) est aujourd'hui un texte libre ; aucune liste curée d'icônes n'existe dans le code (voir *ambiguous term*).

---

## Gaps by Category

### unstated assumption

**[major]** Should category creation continue using a manually-typed id in the unified view, or does it also switch to the auto-generated slug used for actions?
> Id de commande généré automatiquement (slug du nom + suffixe anti-collision), jamais saisi.

**[major]** Does recomposing the executable+arguments list into a single string always re-tokenize back to the exact same list for every possible argument value?
> recomposée automatiquement en une chaîne compatible avec le tokenizer existant

**[blocker]** What prevents a user from creating a real category literally named "Sans catégorie", colliding with the reserved orphan pseudo-category?
> pseudo-catégorie visible "Sans catégorie" (non supprimable/renommable)

### ambiguous term

**[major]** What happens when a user removes every row from the executable+arguments list, leaving no executable to run?
> exécutable+arguments (liste répétable de valeurs)

**[blocker]** Where does the curated icon list's source of truth come from — a hardcoded palette, a scan of the icons directory, or something else?
> icon (sélecteur à liste curée, partagé avec le champ icône des catégories qui est aujourd'hui un texte libre)

### missing edge case

**[minor]** What happens when the new editor lets a user assign a shortcut that's already in use by another action?
> shortcut

**[major]** What happens to an open action-edit form when its selected category gets deleted from underneath it in the same session?
> Actions orphelines d'une catégorie supprimée regroupées dans une pseudo-catégorie visible "Sans catégorie"

**[minor]** Can an auto-generated command slug ever collide with an existing category id, and if so what happens?
> Id de commande généré automatiquement (slug du nom + suffixe anti-collision)

### missing actor

**[minor]** Does the app guard against two running instances writing config.json at the same time once edits persist on every CRUD operation?
> Persistance sur disque au fil de l'eau

### missing failure mode

**[major]** What happens to the in-memory config when the on-disk save fails after a CRUD operation has already mutated it?
> Persistance sur disque au fil de l'eau

**[blocker]** Is the recomposed executable+arguments string validated against the tokenizer before being written to disk, or can a malformed command be saved silently?
> recomposée automatiquement en une chaîne compatible avec le tokenizer existant src/windows/process.rs::tokenize()

### missing acceptance criterion

**[major]** What test or acceptance criterion confirms that editing then reopening an action's executable+arguments list yields the exact same values?
> recomposée automatiquement en une chaîne compatible avec le tokenizer existant

**[minor]** How many icons, and from which source, must the curated picker ship with for this feature to be considered complete?
> icon (sélecteur à liste curée)

### missing dependency

**[blocker]** Which new storage-layer functions (add_command/update_command/remove_command) need to be built, since only toggle_favorite currently exists for commands?
> CRUD complet sur une action

**[major]** Which slug-generation utility (accent-folding plus collision-suffix logic) will this feature depend on, given none currently exists in the codebase?
> Id de commande généré automatiquement (slug du nom + suffixe anti-collision)
