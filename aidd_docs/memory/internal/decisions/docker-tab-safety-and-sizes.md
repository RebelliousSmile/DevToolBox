# Docker tab: targeted removals only, and on-demand volume sizes

- Date: 2026-08-19
- Status: Accepted

## Context

The Docker tab (brainstorm `aidd_docs/tasks/2026_08/2026_08_19-docker-tab-brainstorm.md`)
is a minimal local dashboard with destructive actions (stop/remove). Two
recurring temptations had to be settled: reaching for `--force`/`prune` when a
removal is inconvenient, and fetching volume sizes eagerly even though the only
source (`docker system df -v`) takes ~4.6 s on this machine and would freeze
the synchronous fetch.

## Decision

- **Never `--force`, never `prune`** — targeted, confirmed removals only. A
  unit test on the argument builders guarantees no `--force` can appear; there
  is no prune code path at all. Gating is done in the UI (`ui.add_enabled` +
  disabled-hover text), computed against the container list: an image is
  "used" (and un-removable) if any container references it, with a
  **global used-on-doubt fallback** — one unresolvable reference marks every
  image used rather than guessing.
- **Volume sizes are on-demand**: `docker volume ls` always reports
  `Size:"N/A"`; only `docker system df -v` (~4.6 s, Action class) has them. A
  header button (« Calculer les tailles ») triggers it, and results are merged
  into the snapshot by volume name without refetching. Container sizes are
  cheap (`docker ps -a --size`, ~37 ms) so they ride along with every fetch —
  the part before ` (` in `"767kB (virtual 148MB)"` is the writable layer,
  which is what `docker rm` actually frees.
- **Removal confirmations state the space reclaimed** — and the image wording
  depends on how many snapshot entries share the image id: a sole tag really
  deletes the image (show size), other tags remaining means an untag only (no
  space freed).

## Consequences

- Any future "clean everything" affordance must be a new, explicitly approved
  feature — the current design deliberately has no bulk path.
- The size column can be stale relative to a later refresh (refresh drops it
  until the button is clicked again); that trade-off is accepted to keep
  fetch fast.
