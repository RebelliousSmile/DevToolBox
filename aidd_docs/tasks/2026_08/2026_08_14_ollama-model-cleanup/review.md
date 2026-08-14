# Review: Safe Ollama model cleanup

- **Verdict**: approve
- **Diff**: `5b2a4db...feature/multi-os/part-3-linux-integrations`
- **Axes run**: code, functional, relevancy
- **Date**: 2026_08_14
- **Findings**: 0 critical, 0 warning, 0 minor

## Phases

### Phase 1 — Build the fail-closed Ollama adapter

- [x] No test can make the adapter contact a remote or structurally unsafe URL, hang past its timeout, accept malformed data, or leak an untyped discovery exception — `scripts/winclean/mod_ollama.py:35`, `scripts/winclean/mod_ollama.py:44`, `scripts/winclean/mod_ollama.py:170`, `scripts/winclean/tests/test_mod_ollama.py:80`.
- [x] Discovery returns exactly the explicitly requested installed and stopped models, with each exact name in `resource_id` and no direct path under an Ollama model store — `scripts/winclean/mod_ollama.py:167`, `scripts/winclean/tests/test_mod_ollama.py:102`.
- [x] Apply rechecks each exact model, preserves completed/failed/`skipped-unattempted` outcomes, never claims estimated size as freed bytes, and has full-suite coverage — `scripts/winclean/mod_ollama.py:224`, `scripts/winclean/tests/test_mod_ollama.py:161`.

### Phase 2 — Wire explicit targeting into winclean

- [x] `--ollama-model` is repeatable, exact, validated before discovery, and restricted to the final aggressive Ollama-only selection — `scripts/winclean/clean.py:308`, `scripts/winclean/clean.py:761`, `scripts/winclean/tests/test_clean.py:1843`.
- [x] `ollama-models` is excluded from broad cleanup and reachable only by aggressive explicit selection with a target — `scripts/winclean/registry_mod.py:322`, `scripts/winclean/registry_mod.py:398`, `scripts/winclean/tests/test_clean.py:1830`.
- [x] Existing gates and outputs preserve completed, failed, and unattempted outcomes across text, JSON, history, stderr, and exit status without inventing bytes — `scripts/winclean/common.py:921`, `scripts/winclean/history.py:170`, `scripts/winclean/clean.py:1325`, `scripts/winclean/tests/test_clean.py:1993`.
- [x] Documentation provides dry-run-first commands and destructive limitations; the implementation diff leaves `scripts/system_inventory/` unchanged — `scripts/winclean/README.md:95`, `scripts/winclean/README.md:124`.

## Findings

| Sev | Kind | Phase | Location | Issue | Fix |
| --- | ---- | ----- | -------- | ----- | --- |
| - | - | - | - | None. | - |

## Verification

| Metric        | Value                                             |
| ------------- | ------------------------------------------------- |
| Verified      | 100% (7/7) |
| Files checked | `plan.md`, `phase-1.md`, `phase-2.md`, `scripts/winclean/README.md`, `clean.py`, `common.py`, `history.py`, `mod_ollama.py`, `registry_mod.py`, `test_clean.py`, `test_common.py`, `test_history.py`, `test_mod_dev.py`, `test_mod_ollama.py`, `test_registry_mod.py` |
| Unchecked     | none |
| Unplanned     | none |
