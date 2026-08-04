# winclean is its own package, not a mode of `system_inventory`

- Date: 2026-08-04
- Status: Accepted

## Context

`scripts/system_inventory/` already walks the machine's disks and prices what it
finds. A cleaner needs exactly that walk, so the cheapest move was to add a
`--clean` mode to it. But `system_inventory` is **read-only by construction** —
that property is what makes it safe to run anywhere, at any time, and it is the
reason its output can be trusted as evidence. A tool that deletes user data
cannot share a process with it without putting that property at the mercy of
every future change.

## Decision

A new package `scripts/winclean/` imports `system_inventory` as a **read-only
discovery layer** and owns everything destructive itself. All three levels
(`safe` / `moderate` / `aggressive`) shipped in v1, and **dry-run is the default
at every level** — destruction requires an explicit `--apply`.

## Alternatives

- **`system_inventory --clean`** — lost: one bug in the cleaner would make the
  inventory unsafe, and there would be no boundary to test.
- **`safe` only in v1, the rest later** — lost: the guard layer (protected paths,
  `\\?\` prefixing, path sanity, `--max-delete-bytes`, Recycle Bin interop) has
  to exist before the *first* deletion, so building it for one level and then
  widening it costs more than covering all three at once. The risk lives in the
  guards, not in the number of modules.
- **`--apply` by default with a `--dry-run` flag** — lost: the safe spelling must
  be the short one. A forgotten flag then prints instead of deleting.

## Consequences

- `git diff --quiet HEAD -- scripts/system_inventory` exiting `0` is a
  **release gate** for winclean: any change on that side means the boundary was
  crossed. It is asserted in the plan's final checkpoint.
- Two byte figures per module, from **two independent walks**: `estimated`
  (before) and `measured` (a per-candidate pre-operation measurement, after).
  Feeding the second from the first makes them agree by construction and audits
  nothing. Consequence accepted: they legitimately differ on a correct run, so
  no test may assert equality.
- `None` is the package's **single** encoding of *unmeasurable* — JSON `null`,
  never `0`, never a companion boolean. `0` means "measured, and it was zero".
- `--max-delete-bytes` guards against an order-of-magnitude path bug, not an
  exact budget: a denied subtree inside a readable candidate makes the estimate
  silently low. `measured` is the authority after the fact.
- Everything a human reads is French; every machine token (JSON keys, status
  values, warning codes, module names) is English.
