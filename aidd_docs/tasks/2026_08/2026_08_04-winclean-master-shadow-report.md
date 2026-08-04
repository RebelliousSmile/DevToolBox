---
source: aidd_docs/tasks/2026_08/2026_08_04-winclean-master.md
generated_at: 2026-08-04T13:59:44Z
---

# Shadow Areas Report

Source: `aidd_docs/tasks/2026_08/2026_08_04-winclean-master.md`
Generated: `2026-08-04T13:59:44Z`

Total gaps: 14 | Blocker: 1 | Major: 9 | Minor: 4

---

## Warnings

- The nominal source is the master plan, but its three child plans (`-part-1.md`, `-part-2.md`, `-part-3.md`) were read as part of the same artifact so that gaps already closed in a child plan are not reported as open. Snippets below may therefore be quoted from a child plan.

---

## Gaps by Category

### unstated assumption

**[major]** How does a user read a printed plan when `clean.py` is launched as a WinFXStart action rather than from a terminal?
> `@python scripts/winclean/clean.py`

The whole tool's primary output is a plan printed to stdout, and the host project is a GUI launcher whose console window disposition is never specified. `config/builtin-actions.json` is not mentioned in any of the three parts.

**[minor]** What is the free space on the target volume before and after an `--apply` run?
> Print the estimated-vs-freed report

The report accounts for bytes per module but never states the one number a user of a disk cleaner actually checks.

---

### ambiguous term

**[major]** Does "regenerable" mean regenerable offline, or regenerable only with network access?
> `safe` (regenerable only, no data loss possible)

`cargo-registry`, `pnpm-store` and `nuget-packages` are only regenerable with a working network. On a plane or an air-gapped machine, purging them at the level advertised as "no data loss possible" breaks every build until connectivity returns.

**[major]** Which value is the shipped default for `--max-delete-bytes`?
> `--max-delete-bytes` (default: a conservative ceiling, e.g. 50 GB)

"e.g." leaves the guard's threshold undecided. Too low and the tool aborts on any real dev machine; too high and the ceiling guard never fires.

**[major]** What is the maximum depth of the `--root` discovery walk?
> depth-capped search for `Cargo.toml`

"Depth-capped" appears three times across Part 1 Phase 3 with no number. Two implementers will pick different caps, changing which candidates are found and how long a plan takes.

**[minor]** How is "fixed local volume" determined in `can_recycle()`?
> `False` if the long-path form exceeds MAX_PATH, or the path is not on a fixed local volume

No API is named (`GetDriveTypeW` versus a heuristic on the path prefix).

---

### missing edge case

**[blocker]** Does `--only recycle-bin` without `--level aggressive` raise an error, or silently escalate the level?
> `python scripts/winclean/clean.py --apply --only pycache --root <repo>`

The interaction between `--only` and `--level` is never specified, and the plans' own acceptance commands use `--only` with no `--level`. If `--only` escalates implicitly, the confirmation gate that frozen decision 3 attaches to `moderate`/`aggressive` is bypassable from the command line; if it errors, several stated acceptance criteria cannot run as written. An implementer cannot resolve this without a product decision.

**[major]** Which module owns a path claimed by two modules, such as a `__pycache__` inside `%TEMP%` or a `target/` under a `--root`?
> `docker_wsl.py` is the sole owner of `.vhdx` bytes

The parent `system_inventory` plan solved exactly this with an `exclude_paths` ownership rule and called double-counting out in its risk register. winclean inherits the same overlap problem — `user-temp`, `pycache`, `dotnet-binobj` and `cargo-target` can nest — but no ownership rule is carried over, so estimates double-count and a module deleting a parent makes the next one fail on a vanished path.

**[major]** What happens when a build writes into `target/` between the plan and the `--apply` that deletes it?
> a sibling `target/` qualifies only with a `CACHEDIR.TAG` or a `debug`/`release` child

`procs.is_running` arrives in Part 2 and covers browsers and editors only, while `cargo-target` ships in Part 1. A concurrent `cargo build` during `--apply` corrupts the build tree, and nothing in Part 1 detects or reports it.

---

### missing actor

**[major]** Who empties the Recycle Bin so that bytes recycled by a `moderate` run become free space?
> a recycled total is never added to `freed`

The only module that empties the Bin is `recycle-bin`, which ships in Part 3, is gated at `aggressive`, and skips anything newer than `TRASH_DAYS`. As specified, a `moderate --apply` therefore frees nothing at all until an unspecified later action, and no actor or step is named to close that loop.

**[minor]** Should `clean.py` expose a `--check` exit-code mode for CI, as `deps_audit` already does?
> `--check` is destined for continuous integration: the script fails (code `1`) as soon as a dependency seems unused

The sibling script in the same repo establishes this convention; winclean's CLI list omits it without saying whether the omission is deliberate.

---

### missing failure mode

**[major]** What does the tool do when `SHFileOperationW` returns non-zero or sets `fAnyOperationsAborted`?
> check the return code and `fAnyOperationsAborted`

The plan says to check both but never says what follows. A silent fallback to direct deletion would destroy the undo guarantee the level was chosen for; abandoning the candidate leaves bytes unreclaimed. The two outcomes are materially different and the choice is unwritten.

**[major]** Which record is written when an `--apply` run is interrupted by Ctrl+C or a crash after some modules have already deleted?
> Append the history line after the report, including the process exit status

History is written once, at the end. An interrupted run destroys data and leaves no trace, which defeats the audit purpose the history exists for.

---

### missing acceptance criterion

**[major]** What is the maximum acceptable wall-clock duration for a plan run on this machine?
> Discovery roots are explicit (`--root`, repeatable; default: the repo root and `%USERPROFILE%\Documents`)

Every acceptance criterion is about correctness; none bounds time. A plan that walks `%USERPROFILE%\Documents` recursively could take minutes, and no criterion would fail.

**[minor]** Which test asserts that `scripts/system_inventory/` is still read-only after winclean is added?
> `scripts/system_inventory/` stays **strictly read-only**

Part 1 protects a comparable invariant with a source-level test (no `mod_*.py` imports `remove`), but the frozen decision that the imported package stays unmodified has no equivalent check.
