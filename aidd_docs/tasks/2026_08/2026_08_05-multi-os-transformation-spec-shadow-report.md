---
source: aidd_docs/tasks/2026_08/2026_08_05-multi-os-transformation-spec.md
generated_at: 2026-08-05
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_08/2026_08_05-multi-os-transformation-spec.md`
Generated: `2026-08-05`

Total gaps: 10 | Blocker: 1 | Major: 7 | Minor: 2

---

## Gaps by Category

### unstated assumption

**[minor]** Does the Docker/WSL `.vhdx` inventory logic need a native Linux replacement, given Linux Docker has no VM disk image to inventory?
> Inventaire adapté aux mécanismes Linux équivalents aux `.vhdx` (WSL)

### ambiguous term

**[major]** Which specific cross-platform UI toolkit should replace the Win32-specific rendering in `src/icons/gdi.rs` and the `MessageBoxW` dialogs in `src/ui/app.rs`?
> Adoption d'un **toolkit UI cross-platform**, en remplacement des points d'intégration spécifiquement Win32

### missing edge case

**[major]** Which specific Linux cache and temp paths should winclean's `safe`/`moderate`/`aggressive` levels discover on Linux?
> Portage vers de vrais équivalents Linux, pas des stubs

**[minor]** Does the interpreter-resolution cascade fall back to a bare `python` executable when `python3` is absent from PATH?
> cascade de repli (variable d'environnement dédiée, puis `python3`)

### missing actor

**[major]** Who validates the Linux build against the MVP acceptance bar before it is considered done, given there is no CI pipeline?

### missing failure mode

**[major]** What should DevToolBox do when a Linux desktop environment does not honor the XDG autostart `.desktop` entry?
> utiliser l'équivalent XDG autostart (`~/.config/autostart/*.desktop`)

### missing acceptance criterion

**[major]** Which specific launcher features must produce identical behavior on Linux to count as MVP minimal parity?
> sans exiger une parité fonctionnelle complète feature-par-feature dès cette première itération

### missing dependency

**[blocker]** Which XDG Base Directory variables replace `%APPDATA%\DevToolBox\config.json`, the icons folder, and `%LOCALAPPDATA%\DevToolBox\devtoolbox.log` on Linux?

**[major]** Which reference Linux distribution and minimum library versions does the build-from-source target assume?
> Build depuis les sources uniquement pour cette itération

**[major]** How are command icons resolved on Linux, given there is no equivalent to Windows executable icon extraction via GDI?
> Introduction d'un backend d'icônes portable, abstrayant le rendu actuellement couplé à GDI
