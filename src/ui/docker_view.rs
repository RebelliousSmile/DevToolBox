//! Docker view shared types — the `AutomationRow` precedent
//! (`src/ui/automations_view.rs:37-51`) applied to the Docker tab (see
//! `aidd_docs/tasks/2026_08/2026_08_19-docker-tab.md`, Phase 1/2): the row
//! shapes returned to the UI are OS-neutral and declared here so every
//! build (including Windows/macOS, which have no Docker data source yet)
//! keeps compiling, while `crate::docker::engine` (Linux-only, compiles to
//! nothing elsewhere) is the only module that knows how to *produce* them
//! by mapping its private wire-format serde structs onto these types.
//!
//! Phase 2 adds the OS-neutral façade (`available()`/`fetch()`/the four
//! action wrappers, `automations_view.rs:66-160` layout), the pure
//! "data in, actions out" `render` view (`cleanup_view.rs` precedent) and
//! the `DockerAction` intent enum. Phase 3 wires all of this into
//! `EguiApp` (tab visibility, lazy fetch, confirmation modals,
//! `PendingAction` dispatch — see `src/ui/egui_app.rs`), so every item here
//! now has a production caller and the module-level `#![allow(dead_code)]`
//! that covered the pre-Phase-3 gap has been removed.

use std::collections::HashSet;

use eframe::egui;

use std::collections::BTreeSet;

use crate::net::ListeningPort;
use crate::ui::ports::{self, OwnerKind, PortAllocation, PortBinding, PortConflict, PortOwner};

/// One row of the Docker tab's "Conteneurs" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEntry {
    pub id: String,
    pub name: String,
    /// The image reference as reported by `docker ps -a` (repo:tag, a short
    /// ID, or a digest ref — whatever Docker itself printed), kept verbatim
    /// for display; used-image cross-referencing happens on the Linux side
    /// before this row is built.
    pub image: String,
    pub state: ContainerState,
    /// Free-text status as reported by Docker (e.g. `"Up 3 hours"`,
    /// `"Exited (0) 22 hours ago"`), shown alongside `state` since it
    /// carries detail no enum variant captures.
    pub status: String,
    /// The container's writable-layer size (e.g. `"767kB"`), already
    /// stripped of the `(virtual …)` suffix by `crate::docker::engine`'s
    /// `extract_rw_size` — what `docker rm` actually frees. Empty when
    /// unknown (should not normally happen: `docker ps -a --size` always
    /// fills this field on this reference machine, but a future CLI shape
    /// change degrading it to blank must not crash the confirm-message
    /// formatting).
    pub rw_size: String,
    /// Published host bindings, parsed from `docker ps -a`'s `Ports` field.
    ///
    /// **Not** empty for a stopped container, contrary to what this field's
    /// first version assumed: `docker ps -a` keeps printing the bindings of
    /// the last run. Reading them as current claims is what made four stopped
    /// `mysql` containers, none of which declares a host port, all report
    /// `0.0.0.0:32768->3306/tcp` and flag each other. [`declared_host_ports`]
    /// is the field that tells the two cases apart.
    ///
    /// [`declared_host_ports`]: ContainerEntry::declared_host_ports
    pub ports: Vec<PortBinding>,
    /// `(host_port, protocol)` the container explicitly requested, from
    /// `HostConfig.PortBindings`. A binding of [`ports`] missing here was
    /// picked by docker at start-up and will be picked again, differently, at
    /// the next one. Empty when the inspect pass came back without the
    /// container — which degrades to "nothing declared", never to a
    /// fabricated conflict.
    ///
    /// [`ports`]: ContainerEntry::ports
    pub declared_host_ports: BTreeSet<(u16, String)>,
    /// RFC3339 date of the container's last activity: `.State.FinishedAt`
    /// when it is not the `0001-01-01T00:00:00Z` zero value, otherwise
    /// `.Created` (a `created` container has never run). `None` when the
    /// grouped `docker inspect` could not resolve this id — a benign race
    /// against a removal, which must cost the badge, not the whole column.
    pub last_activity: Option<String>,
    /// `com.docker.compose.project` — the `-p` name this container was
    /// started under. `None` for anything not started by compose (here:
    /// `buildx_buildkit_mybuilder0`), which the Stacks section then ignores
    /// entirely.
    pub compose_project: Option<String>,
    /// `com.docker.compose.project.config_files`, split on `,`. Its entries
    /// are what a stack row's identity is matched against — a *path*, because
    /// two different compose files can resolve to the same project name (two
    /// `proxy` projects on this machine).
    ///
    /// The label is read from the grouped `docker inspect`'s `.Config.Labels`
    /// map, never from `docker ps`'s flat `Labels` string: that string joins
    /// labels with `,` while this value is itself a `,`-separated list, so a
    /// multi-file project cannot be recovered from it unambiguously.
    pub compose_files: Vec<String>,
    /// `com.docker.compose.service`. With [`compose_files`] it names the
    /// declaration this container instantiates; two containers sharing both
    /// come from one `ports:` line and are a duplicate project, not a port
    /// collision.
    ///
    /// [`compose_files`]: ContainerEntry::compose_files
    pub compose_service: Option<String>,
    /// The container's exit code, parsed out of [`ContainerEntry::status`] by
    /// [`parse_exit_code`]. `None` for anything that is not exited, and for a
    /// status shape this parse does not recognize.
    pub exit_code: Option<i32>,
}

/// The exit code inside a `docker ps` status such as
/// `"Exited (137) 22 hours ago"`.
///
/// Read from the *listing*, not from `docker inspect`: the listing is what
/// built the row in the first place, so a status is always present, whereas
/// the inspect pass is allowed to come back empty on a race. Anything that is
/// not an `Exited (<int>)` shape — `"Up 3 hours"`, `"Created"`, a future CLI
/// wording — yields `None`, which
/// [`crate::ui::compose_view::is_failing`] treats as "not proven healthy".
pub fn parse_exit_code(status: &str) -> Option<i32> {
    let start = status.find('(')?;
    let end = status[start..].find(')')? + start;
    status[start + 1..end].trim().parse().ok()
}

/// A container's lifecycle state, mapped from Docker's free-text `State`
/// field (see `crate::docker::engine`'s `ContainerState::from_raw`) into a
/// closed set the view can match on to decide which actions are offered.
/// `Unknown` is the conservative default for any value this mapping wasn't
/// built to recognize — Phase 2 gates both stop and remove off for it (per
/// the plan's Phase 1 task 8: "anything unknown ⇒ no action offered").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Paused,
    Restarting,
    Exited,
    Created,
    Dead,
    Unknown(String),
}

impl ContainerState {
    /// `true` for the states a running-container stop action is offered on
    /// (`running`/`paused`/`restarting`).
    pub fn is_stoppable(&self) -> bool {
        matches!(
            self,
            ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
        )
    }

    /// `true` for the states a container-removal action is offered on
    /// (`exited`/`created`/`dead`).
    pub fn is_removable(&self) -> bool {
        matches!(
            self,
            ContainerState::Exited | ContainerState::Created | ContainerState::Dead
        )
    }
}

/// One row of the Docker tab's "Images" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageEntry {
    pub id: String,
    /// Display identity: `"<repository>:<tag>"`, including the literal
    /// `"<none>:<none>"` Docker itself prints for an untagged image.
    pub identity: String,
    pub size: String,
    pub created: String,
    /// Conservative "in use" flag: `true` when at least one container
    /// references this image (by ID prefix or normalized repo:tag), or when
    /// any container's image reference could not be resolved at all (see
    /// the Risk register's "used-on-doubt" rule — deletion is never offered
    /// on uncertainty). Never `false` on doubt.
    pub used: bool,
    /// The exact reference `docker rmi` should receive: the row's
    /// `identity` (repo:tag) when tagged, the short `id` when untagged
    /// (`rmi <id>` refuses multi-tagged images without `--force`, which is
    /// banned; removing by tag untags cleanly, while an untagged row has no
    /// tag to remove by).
    pub rmi_reference: String,
    /// The image's `.Created` date, RFC3339, from the grouped
    /// `docker inspect` pass. `None` when the id could not be resolved.
    pub created_iso: Option<String>,
    /// Short ids of every container referencing this image — the same walk
    /// `compute_used` performs, kept instead of collapsed into `used`
    /// (`used == !used_by.is_empty()` except in the used-on-doubt case,
    /// where `used` is `true` with nothing to list).
    pub used_by: Vec<String>,
}

impl ImageEntry {
    /// `true` when this row is Docker's `<none>:<none>` placeholder for an
    /// untagged image. Its `rmi_reference` is the image ID, and removing it
    /// is an outright, unambiguous deletion — there is no tag to strip
    /// first. A tagged row's removal is comparatively ambiguous: it always
    /// untags the row's `identity`, but whether that also deletes the
    /// underlying image data depends on whether any other tag still points
    /// to it (information this row alone doesn't carry). Phase 3's
    /// confirmation-message wording (« l'image sera détaguée » vs
    /// « l'image sera supprimée ») branches on this accessor rather than on
    /// a new `DockerAction::RemoveImage` payload field, keeping the action
    /// enum a plain identifier carrier.
    pub fn is_untagged(&self) -> bool {
        self.identity == "<none>:<none>"
    }
}

/// One row of the Docker tab's "Volumes" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEntry {
    pub name: String,
    pub driver: String,
    /// `true` when this volume is a member of Docker's own "dangling"
    /// set (`docker volume ls -f dangling=true`) — the authoritative
    /// orphan/unused signal, not a re-derivation of it.
    pub orphan: bool,
    /// This volume's on-disk size, or `None` when it hasn't been computed
    /// yet — `docker volume ls` never reports it (always `"N/A"`, confirmed
    /// on this machine); it's only known once `DockerAction::ComputeVolumeSizes`
    /// has run `crate::docker::engine::volume_sizes` (a `docker system df -v`
    /// disk scan) and `EguiApp` has merged the result into the snapshot by
    /// name.
    pub size: Option<String>,
    /// The volume's `.CreatedAt` date. Measured on this machine, `docker
    /// volume inspect` returns a *local* offset (`2026-08-17T11:07:18+02:00`)
    /// where containers and images return `…Z`, which is precisely why
    /// [`parse_rfc3339`] exists instead of a string comparison.
    pub created_iso: Option<String>,
}

/// The full Docker tab snapshot: one `fetch()` call's worth of containers,
/// images and volumes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerSnapshot {
    pub containers: Vec<ContainerEntry>,
    pub images: Vec<ImageEntry>,
    pub volumes: Vec<VolumeEntry>,
}

impl Default for ContainerState {
    /// Matches the mapping's own conservative default: an absent/unparsed
    /// state is `Unknown`, never a guessed active/inactive variant.
    fn default() -> Self {
        ContainerState::Unknown(String::new())
    }
}

// ---------------------------------------------------------------------------
// OS-neutral façade — exact cfg layout of `automations_view.rs:66-160`
// ---------------------------------------------------------------------------

/// `true` when the Docker tab has a data source on this OS at all — drives
/// tab visibility in `EguiApp` (Phase 3). Linux resolves the real `docker`
/// binary; every other OS is hardcoded `false` for now (Risk register: "tab
/// button rendered only when `docker_available`, which is hardcoded `false`
/// on non-Linux for now").
pub fn available() -> bool {
    available_impl()
}

fn available_impl() -> bool {
    crate::docker::engine::binary_available()
}

/// Fetch the current Docker snapshot (containers/images/volumes) for this
/// OS. The error string is already a formatted, French, ready-to-display
/// message (`DockerError`'s `Display` impl on Linux) — callers (the view,
/// then `EguiApp`) never need to re-classify it; a daemon-unreachable error
/// and a plain command failure both just render as "the fetch failed, offer
/// Réessayer", which `render` below does unconditionally on any `Err`.
pub fn fetch() -> Result<DockerSnapshot, String> {
    fetch_impl()
}

fn fetch_impl() -> Result<DockerSnapshot, String> {
    crate::docker::engine::fetch().map_err(|error| error.to_string())
}

/// Stop a running/paused/restarting container by `id`. Plain `docker stop`
/// on Linux (`crate::docker::engine::stop_container`), no `--force`
/// equivalent, ever.
pub fn stop_container(id: &str) -> Result<(), String> {
    stop_container_impl(id)
}

fn stop_container_impl(id: &str) -> Result<(), String> {
    crate::docker::engine::stop_container(id).map_err(|error| error.to_string())
}

/// Remove a stopped/created/dead container by `id`. Plain `docker rm`, no
/// `-f`.
pub fn remove_container(id: &str) -> Result<(), String> {
    remove_container_impl(id)
}

fn remove_container_impl(id: &str) -> Result<(), String> {
    crate::docker::engine::remove_container(id).map_err(|error| error.to_string())
}

/// Remove an unused image by `reference` — the caller passes
/// [`ImageEntry::rmi_reference`] (repo:tag for a tagged row, ID for an
/// untagged `<none>:<none>` row). Plain `docker rmi`, no `-f`/`--force`.
pub fn remove_image(reference: &str) -> Result<(), String> {
    remove_image_impl(reference)
}

fn remove_image_impl(reference: &str) -> Result<(), String> {
    crate::docker::engine::remove_image(reference).map_err(|error| error.to_string())
}

/// Remove an orphan (dangling) volume by `name`. Plain `docker volume rm`,
/// no `-f`/`--force`.
pub fn remove_volume(name: &str) -> Result<(), String> {
    remove_volume_impl(name)
}

fn remove_volume_impl(name: &str) -> Result<(), String> {
    crate::docker::engine::remove_volume(name).map_err(|error| error.to_string())
}

/// Compute every volume's on-disk size via `docker system df -v` — a slow
/// disk scan (~4.6s measured on the Linux reference machine), never called
/// as part of `fetch`. Returns `(name, size)` pairs the caller merges into
/// its own snapshot by name.
pub fn volume_sizes() -> Result<Vec<(String, String)>, String> {
    volume_sizes_impl()
}

fn volume_sizes_impl() -> Result<Vec<(String, String)>, String> {
    crate::docker::engine::volume_sizes().map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Dates and dormancy — pure, clock-injected, no `chrono`
// ---------------------------------------------------------------------------

/// Docker's "never" date: what `.State.FinishedAt` holds for a container that
/// has never run. Treated as *no date at all*, never as a very old one.
pub const ZERO_DOCKER_DATE: &str = "0001-01-01T00:00:00Z";

const SECONDS_PER_DAY: i64 = 86_400;

/// Days elapsed since the civil epoch (1970-01-01), Howard Hinnant's
/// `days_from_civil`. Valid for any proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_number(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

/// RFC3339 timestamp to epoch seconds, without pulling in `chrono`.
///
/// A real parser is required rather than a lexicographic comparison: measured
/// on this machine, `docker volume inspect` returns a local offset
/// (`2026-08-17T11:07:18+02:00`) while container and image inspects return
/// `…Z`, and comparing those two shapes as strings gives wrong answers around
/// the offset. Returns `None` on anything unparsable — including
/// [`ZERO_DOCKER_DATE`] — so an undatable row is never badged rather than
/// wrongly badged.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    if text == ZERO_DOCKER_DATE {
        return None;
    }
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    if !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }

    let year = parse_number(&text[0..4])?;
    let month = parse_number(&text[5..7])?;
    let day = parse_number(&text[8..10])?;
    let hour = parse_number(&text[11..13])?;
    let minute = parse_number(&text[14..16])?;
    let second = parse_number(&text[17..19])?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    // Optional fractional seconds, dropped: sub-second precision is noise at
    // a day-granularity dormancy threshold.
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        rest = &fraction[digits..];
    }

    let offset_seconds = match rest.as_bytes().first() {
        Some(b'Z') | Some(b'z') if rest.len() == 1 => 0,
        Some(sign @ (b'+' | b'-')) => {
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours = parse_number(&rest[1..3])?;
            let minutes = parse_number(&rest[4..6])?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let magnitude = hours * 3_600 + minutes * 60;
            if *sign == b'-' {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    Some(days * SECONDS_PER_DAY + hour * 3_600 + minute * 60 + second - offset_seconds)
}

/// The epoch second before which a resource counts as dormant.
pub fn cutoff_epoch(now_epoch_secs: i64, days: u32) -> i64 {
    now_epoch_secs - i64::from(days) * SECONDS_PER_DAY
}

/// `true` when `date` is parseable *and* older than `cutoff`. `None` or an
/// unparsable date yields `false` — never badge what could not be dated.
pub fn is_dormant(date: Option<&str>, cutoff: i64) -> bool {
    date.and_then(parse_rfc3339)
        .is_some_and(|epoch| epoch < cutoff)
}

/// Whole days between `date` and `now_epoch_secs`, for the `dormant · N j`
/// badge. `None` when the date is unparsable; negative spans clamp to 0.
pub fn days_since(date: &str, now_epoch_secs: i64) -> Option<i64> {
    let epoch = parse_rfc3339(date)?;
    Some(((now_epoch_secs - epoch) / SECONDS_PER_DAY).max(0))
}

/// Dormancy never stands alone — it *refines* the existing unused/orphan
/// signals. Docker stores no "last used" date for images or volumes, which is
/// exactly why the user's "2 months" criterion cannot be a standalone filter:
/// a live container's image is not dormant however old it is.
pub fn container_is_dormant(entry: &ContainerEntry, cutoff: i64) -> bool {
    // `is_stoppable()` covers running/paused/restarting — all three are alive
    // in the sense that matters here, so none of them can be dormant.
    !entry.state.is_stoppable() && is_dormant(entry.last_activity.as_deref(), cutoff)
}

pub fn image_is_dormant(entry: &ImageEntry, cutoff: i64) -> bool {
    !entry.used && is_dormant(entry.created_iso.as_deref(), cutoff)
}

pub fn volume_is_dormant(entry: &VolumeEntry, cutoff: i64) -> bool {
    entry.orphan && is_dormant(entry.created_iso.as_deref(), cutoff)
}

/// The identity of the compose declaration a container instantiates, or
/// `None` when it was not created by compose (a hand-rolled `docker run`, or
/// a project whose labels were stripped).
///
/// The `\u{1f}` separator is a unit separator rather than a path character: a
/// compose file path can contain almost anything, and joining on `:` or `#`
/// would let two different (file, service) pairs collapse into one key.
fn compose_declaration(entry: &ContainerEntry) -> Option<String> {
    let service = entry.compose_service.as_deref()?;
    if entry.compose_files.is_empty() {
        return None;
    }
    Some(format!("{}\u{1f}{service}", entry.compose_files.join(",")))
}

/// Every container of a snapshot that publishes something, as port owners
/// ready for [`ports::find_conflicts`]. `compose_view::declared_owners`
/// appends its `DeclaredStack` owners to the same slice before the call.
///
/// Stopped containers are included but tagged [`OwnerKind::StoppedContainer`]
/// and carry their declared set, so the detector can drop the dynamic ports
/// they merely *used* to hold.
pub fn container_port_owners(snapshot: &DockerSnapshot) -> Vec<PortOwner> {
    snapshot
        .containers
        .iter()
        .filter(|entry| !entry.ports.is_empty())
        .map(|entry| {
            let kind = if entry.state.is_stoppable() {
                OwnerKind::RunningContainer
            } else {
                OwnerKind::StoppedContainer
            };
            let mut owner = PortOwner::new(
                entry.id.clone(),
                entry.name.clone(),
                kind,
                entry.ports.clone(),
            )
            .with_declared(entry.declared_host_ports.clone())
            .with_source(entry.compose_files.join(", "));
            if let Some(declaration) = compose_declaration(entry) {
                owner = owner.with_declaration(declaration);
            }
            owner
        })
        .collect()
}

/// Conflicts touching one container, as `(host_port, other owner labels)`.
fn conflicts_for(conflicts: &[PortConflict], label: &str) -> Vec<String> {
    conflicts
        .iter()
        .filter(|conflict| conflict.owners.iter().any(|owner| owner == label))
        .map(|conflict| {
            let others: Vec<&str> = conflict
                .owners
                .iter()
                .filter(|owner| owner.as_str() != label)
                .map(String::as_str)
                .collect();
            format!(
                "{}/{} également utilisé par {}",
                conflict.host_port,
                conflict.protocol,
                others.join(", ")
            )
        })
        .collect()
}

/// The `dormant · N j` badge text, given the row's date.
fn dormant_badge_text(date: Option<&str>, now_epoch_secs: i64) -> String {
    match date.and_then(|raw| days_since(raw, now_epoch_secs)) {
        Some(days) => format!("dormant · {days} j"),
        None => "dormant".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Human-readable sizes — docker prints SI units
// ---------------------------------------------------------------------------

/// Size suffixes as `docker` itself prints them, longest first so `MB` is
/// never matched as `B`.
///
/// **`kB` is 1000, not 1024**: docker's `units.HumanSize` is decimal. Reading
/// it as binary inflates every figure by 2.4 % per order of magnitude — on a
/// dialog shown right before a destructive action, that is a lie about how
/// much disk is at stake.
///
/// The third field marks the canonical spelling: `KB` is accepted on the way
/// in (older docker builds print it) but never produced on the way out, so
/// one table serves both directions instead of two that can drift.
const SIZE_UNITS: &[(&str, u64, bool)] = &[
    ("PB", 1_000_000_000_000_000, true),
    ("TB", 1_000_000_000_000, true),
    ("GB", 1_000_000_000, true),
    ("MB", 1_000_000, true),
    ("kB", 1_000, true),
    ("KB", 1_000, false),
    ("B", 1, true),
];

/// Parse one docker size string into bytes, or `None` when it carries no
/// readable number (`N/A`, `""`, `-`).
///
/// The container form `767kB (virtual 148MB)` is accepted by reading only
/// what precedes the parenthesis: the virtual size counts the image layers,
/// which `docker rm` does not free. (`ContainerEntry::rw_size` is normally
/// already stripped by `linux::docker::extract_rw_size`; this is the belt to
/// that braces, and costs one `split`.)
pub fn parse_human_size(text: &str) -> Option<u64> {
    let text = text.split(" (").next().unwrap_or(text).trim();
    let (number, multiplier) = SIZE_UNITS.iter().find_map(|(suffix, multiplier, _)| {
        text.strip_suffix(suffix)
            .map(|number| (number, *multiplier))
    })?;
    let value: f64 = number.trim().parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some((value * multiplier as f64).round() as u64)
}

/// Render a byte count the way docker would, so the batch total and the
/// per-row sizes it was summed from read in the same units.
pub fn format_human_size(bytes: u64) -> String {
    for (suffix, multiplier, canonical) in SIZE_UNITS {
        if !canonical || bytes < *multiplier {
            continue;
        }
        if *multiplier == 1 {
            return format!("{bytes}B");
        }
        return format!("{:.1}{suffix}", bytes as f64 / *multiplier as f64);
    }
    "0B".to_string()
}

/// Render a batch total, prefixed with `≥` when at least one row's size was
/// unreadable.
///
/// An unparsable row is **not** counted as zero silently: the total would then
/// under-report what the batch frees, on the one screen where that number is
/// the basis for an irreversible decision.
pub fn format_selection_size(bytes: u64, partial: bool) -> String {
    if partial {
        format!("≥ {}", format_human_size(bytes))
    } else {
        format_human_size(bytes)
    }
}

// ---------------------------------------------------------------------------
// Batch selection and deletion
// ---------------------------------------------------------------------------

/// Which family a selected row belongs to.
///
/// **The declaration order is the deletion order** — [`order_targets`] sorts
/// on it, and containers must go before the images they hold, which must go
/// before... nothing, volumes being independent but last so a failed image
/// does not delay them. Reordering these variants silently reorders the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceKind {
    Container,
    Image,
    Volume,
}

/// One selected row, identified by whatever string its removal façade takes:
/// the container id, the image's `rmi_reference`, the volume name. Never a
/// display label — that lives in [`BatchTarget::label`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionKey {
    pub kind: ResourceKind,
    pub id: String,
}

impl SelectionKey {
    pub fn container(id: impl Into<String>) -> Self {
        Self {
            kind: ResourceKind::Container,
            id: id.into(),
        }
    }

    pub fn image(reference: impl Into<String>) -> Self {
        Self {
            kind: ResourceKind::Image,
            id: reference.into(),
        }
    }

    pub fn volume(name: impl Into<String>) -> Self {
        Self {
            kind: ResourceKind::Volume,
            id: name.into(),
        }
    }
}

/// A selection key plus the name the report will show for it — resolved from
/// the snapshot at batch-build time, since the batch outlives the snapshot it
/// came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTarget {
    pub key: SelectionKey,
    pub label: String,
}

/// What one target's deletion did. The error is a `String`, not a
/// `DockerError`: `EguiApp` holds these and compiles on Windows too, where
/// that type does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOutcome {
    pub label: String,
    pub result: Result<(), String>,
}

/// `true` when this container already has an enabled « Supprimer » button —
/// the single source of truth the checkbox reuses, so a batch can never
/// target something a single deletion would have refused.
fn container_is_selectable(entry: &ContainerEntry) -> bool {
    entry.state.is_removable()
}

/// Same, for a volume: only Docker's own dangling set is offered.
fn volume_is_selectable(entry: &VolumeEntry) -> bool {
    entry.orphan
}

/// `true` when this image may be checked, given what is currently selected.
///
/// Beyond the single-row rule (`!used`), an image whose every dependent
/// container is itself selected becomes selectable: the ordered batch deletes
/// those containers first, so the image is genuinely free by the time its turn
/// comes. Without it the headline use case — « projet fini : le conteneur et
/// son image en un lot » — is impossible, since the image reads `used` in the
/// pre-batch snapshot.
///
/// The used-on-doubt case (`used` with an empty `used_by`, set when a
/// container's image reference could not be resolved at all) stays
/// unselectable: there is no set of containers whose selection would free it.
pub fn image_is_selectable(image: &ImageEntry, selection: &HashSet<SelectionKey>) -> bool {
    if !image.used {
        return true;
    }
    if image.used_by.is_empty() {
        return false;
    }
    image
        .used_by
        .iter()
        .all(|id| selection.contains(&SelectionKey::container(id)))
}

/// Drop from `selection` everything the current snapshot no longer allows.
///
/// Run after every selection change *and* every refetch, which makes two
/// guarantees hold at every instant: a key whose resource vanished stops
/// targeting anything, and deselecting a container immediately unchecks any
/// image that depended on it (images are validated against the containers
/// this pass already kept, never against the raw input).
pub fn sanitize_selection(
    selection: &HashSet<SelectionKey>,
    snapshot: &DockerSnapshot,
) -> HashSet<SelectionKey> {
    let mut kept = HashSet::new();
    for container in &snapshot.containers {
        let key = SelectionKey::container(&container.id);
        if container_is_selectable(container) && selection.contains(&key) {
            kept.insert(key);
        }
    }
    for volume in &snapshot.volumes {
        let key = SelectionKey::volume(&volume.name);
        if volume_is_selectable(volume) && selection.contains(&key) {
            kept.insert(key);
        }
    }
    // Images last: their legality is a function of the containers above.
    for image in &snapshot.images {
        let key = SelectionKey::image(&image.rmi_reference);
        if selection.contains(&key) && image_is_selectable(image, &kept) {
            kept.insert(key);
        }
    }
    kept
}

/// Every deletable row currently badged dormant — what the « Tout
/// sélectionner (dormants) » shortcut checks.
///
/// Strictly the badged rows, nothing more: the shortcut ticks what the user
/// can see is dormant. An image held by a dormant container is therefore left
/// out — [`image_is_dormant`] requires `!used`, so it carries no badge — but
/// its checkbox does become enabled once that container is ticked, and the
/// user can add it in one click.
///
/// The three passes still cannot be reordered: [`image_is_selectable`] reads
/// the containers this function has already collected.
pub fn dormant_selection(snapshot: &DockerSnapshot, cutoff: i64) -> HashSet<SelectionKey> {
    let mut selection = HashSet::new();
    for container in &snapshot.containers {
        if container_is_selectable(container) && container_is_dormant(container, cutoff) {
            selection.insert(SelectionKey::container(&container.id));
        }
    }
    for volume in &snapshot.volumes {
        if volume_is_selectable(volume) && volume_is_dormant(volume, cutoff) {
            selection.insert(SelectionKey::volume(&volume.name));
        }
    }
    for image in &snapshot.images {
        if image_is_dormant(image, cutoff) && image_is_selectable(image, &selection) {
            selection.insert(SelectionKey::image(&image.rmi_reference));
        }
    }
    selection
}

/// How many containers, images and volumes the selection holds.
pub fn selection_counts(selection: &HashSet<SelectionKey>) -> (usize, usize, usize) {
    let count = |kind: ResourceKind| selection.iter().filter(|key| key.kind == kind).count();
    (
        count(ResourceKind::Container),
        count(ResourceKind::Image),
        count(ResourceKind::Volume),
    )
}

/// Bytes the selection reclaims, and whether at least one row's size was
/// unreadable.
///
/// A container contributes its **writable layer only** (`rw_size`): its image
/// layers are freed by the image row, not by `docker rm`. Counting both would
/// double-count the very case this feature exists for.
pub fn selection_size(selection: &HashSet<SelectionKey>, snapshot: &DockerSnapshot) -> (u64, bool) {
    let mut total = 0u64;
    let mut partial = false;
    let mut add = |raw: Option<&str>| match raw.and_then(parse_human_size) {
        Some(bytes) => total = total.saturating_add(bytes),
        None => partial = true,
    };
    for key in selection {
        match key.kind {
            ResourceKind::Container => add(snapshot
                .containers
                .iter()
                .find(|entry| entry.id == key.id)
                .map(|entry| entry.rw_size.as_str())),
            ResourceKind::Image => add(snapshot
                .images
                .iter()
                .find(|entry| entry.rmi_reference == key.id)
                .map(|entry| entry.size.as_str())),
            ResourceKind::Volume => add(snapshot
                .volumes
                .iter()
                .find(|entry| entry.name == key.id)
                .and_then(|entry| entry.size.as_deref())),
        }
    }
    (total, partial)
}

/// Resolve the selection into the batch's targets, each carrying the label
/// its report line will show, in execution order.
pub fn selection_targets(
    selection: &HashSet<SelectionKey>,
    snapshot: &DockerSnapshot,
) -> Vec<BatchTarget> {
    let targets = selection
        .iter()
        .map(|key| {
            let label = match key.kind {
                ResourceKind::Container => snapshot
                    .containers
                    .iter()
                    .find(|entry| entry.id == key.id)
                    .map(|entry| container_label(entry).to_string()),
                ResourceKind::Image => snapshot
                    .images
                    .iter()
                    .find(|entry| entry.rmi_reference == key.id)
                    .map(|entry| image_label(entry).to_string()),
                ResourceKind::Volume => snapshot
                    .volumes
                    .iter()
                    .find(|entry| entry.name == key.id)
                    .map(|entry| entry.name.clone()),
            };
            BatchTarget {
                // The id is a usable last resort: it is exactly what docker
                // was asked to delete.
                label: label.unwrap_or_else(|| key.id.clone()),
                key: key.clone(),
            }
        })
        .collect::<Vec<_>>();
    order_targets(&targets)
}

/// Sort into containers -> images -> volumes, the only order in which the
/// dependencies resolve themselves.
///
/// The sort is **stable**, so whatever order the caller built inside one
/// family survives; only the families move.
pub fn order_targets(targets: &[BatchTarget]) -> Vec<BatchTarget> {
    let mut ordered = targets.to_vec();
    ordered.sort_by_key(|target| target.key.kind);
    ordered
}

/// Run `remove` once per target, in [`order_targets`] order, **never**
/// stopping on an error: one outcome comes back per input.
///
/// Split out from [`remove_batch`] so the two guarantees that matter —
/// ordering and continue-on-failure — are testable on any OS with no daemon
/// involved: the tests inject a closure that records its calls and fails on a
/// chosen item.
pub fn remove_batch_with<F>(targets: &[BatchTarget], mut remove: F) -> Vec<BatchOutcome>
where
    F: FnMut(&BatchTarget) -> Result<(), String>,
{
    order_targets(targets)
        .into_iter()
        .map(|target| BatchOutcome {
            result: remove(&target),
            label: target.label,
        })
        .collect()
}

/// Delete every selected resource, ordered, continuing past failures.
///
/// Deliberately **not** `docker rm a b c`: per-item calls are what make
/// per-item reporting and continue-on-failure possible at all, and the counts
/// here are tens, not thousands. Each call keeps its existing 30 s timeout, so
/// one hung deletion cannot stall the batch.
pub fn remove_batch(targets: &[BatchTarget]) -> Vec<BatchOutcome> {
    remove_batch_impl(targets)
}

fn remove_batch_impl(targets: &[BatchTarget]) -> Vec<BatchOutcome> {
    remove_batch_with(targets, |target| match target.key.kind {
        ResourceKind::Container => remove_container(&target.key.id),
        ResourceKind::Image => remove_image(&target.key.id),
        ResourceKind::Volume => remove_volume(&target.key.id),
    })
}

/// The name a container is shown and reported under: its name, or its id when
/// Docker reported none.
fn container_label(entry: &ContainerEntry) -> &str {
    if entry.name.is_empty() {
        &entry.id
    } else {
        &entry.name
    }
}

/// The name an image is shown and reported under. An untagged row displays its
/// short id — `<none>:<none>` identifies nothing.
fn image_label(entry: &ImageEntry) -> &str {
    if entry.is_untagged() {
        &entry.id
    } else {
        &entry.identity
    }
}

// ---------------------------------------------------------------------------
// Pure view — "data in, actions out" (`cleanup_view.rs` precedent)
// ---------------------------------------------------------------------------

const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
/// Amber: a host-port collision is a warning, not a failure — the containers
/// involved are running fine, one of them just did not get its port.
const CONFLICT_COLOR: egui::Color32 = egui::Color32::from_rgb(0xB7, 0x6E, 0x00);
/// Grey: dormancy is informational, and must not read as an error on a row
/// the user deliberately keeps around.
const DORMANT_COLOR: egui::Color32 = egui::Color32::from_rgb(0x80, 0x80, 0x80);

/// Which of the three resource lists the Docker tab is currently showing.
///
/// The three used to be stacked in a single scroll area, which meant scrolling
/// past a long container table to reach the volumes; they are tabs now. Owned
/// by `EguiApp` as session state only — a list choice is not worth a
/// `config.json` write, and reopening the app on « Conteneurs » is the right
/// default anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DockerList {
    #[default]
    Containers,
    Images,
    Volumes,
    /// The host-port allocation table. Read-only and derived: it owns no
    /// resource, so it takes no part in the selection or the batch deletion.
    Ports,
}

/// Everything `render` needs to draw one frame of the Docker tab. Owned and
/// assembled by `EguiApp` (Phase 3) from its own `docker: Option<Result<DockerSnapshot,
/// String>>` field plus a busy flag — this type never touches `EguiApp`
/// itself (the `cleanup_view::CleanupViewState` precedent).
pub struct DockerViewState<'a> {
    /// `None` before the first fetch has completed (successfully or not).
    pub snapshot: Option<&'a DockerSnapshot>,
    /// Last fetch failure to show in the banner, with a « Réessayer »
    /// button — set whenever the last `fetch()` returned `Err`, regardless
    /// of which `DockerError` variant produced the string (see `fetch`'s
    /// doc comment: the façade already did the classification the view
    /// would otherwise have to redo).
    pub error: Option<&'a str>,
    /// Global single-command-slot guard: disables every button while an
    /// action or a refetch is in flight.
    pub busy: bool,
    /// Dormancy threshold in days, straight from `Settings`. Held here rather
    /// than baked into the entries so changing it in Préférences updates every
    /// badge on the next frame, with no refetch.
    pub dormant_after_days: u32,
    /// Injected clock (`SystemTime::now()` in production, a fixed value in
    /// tests) — the view stays pure and its badges stay assertable.
    pub now_epoch_secs: i64,
    /// Port owners this view does not know about — the declared ports of the
    /// *stopped* compose stacks, computed by `compose_view::declared_owners`.
    ///
    /// They participate in the conflict detection so a container's port badge
    /// also lights up when the collision is with a stack that is merely
    /// declared, not running. Empty in every test that only exercises the
    /// container/image/volume sections.
    pub extra_port_owners: &'a [PortOwner],
    /// Rows currently ticked for the batch. Owned by `EguiApp` because it
    /// must survive a refetch (and be pruned against it); the view only reads
    /// it and emits toggles, staying pure.
    pub selection: &'a HashSet<SelectionKey>,
    /// Result of the last batch, one line per target. Empty until a batch has
    /// run; cleared by `EguiApp` on the next selection change or manual
    /// refresh — never on a timer, so a report cannot vanish while it is
    /// being read.
    pub batch_report: &'a [BatchOutcome],
    /// The list tab currently open. Only its section is rendered, so the two
    /// others cost nothing to lay out — but see `render_list_tabs`: their
    /// selection counts must stay visible, otherwise a batch spanning several
    /// lists becomes half-invisible.
    pub active_list: DockerList,
    /// Ports held on this machine at the last `DockerAction::ScanHostPorts`,
    /// or `None` when none has run yet.
    ///
    /// `None` and `Some(&[])` are two different statements — "never looked"
    /// and "looked, found nothing" — and the Ports section shows its « Hôte »
    /// column only in the second case, the same way the volume list hides its
    /// size column until a scan has filled it.
    pub host_ports: Option<&'a [ListeningPort]>,
}

/// One user intent emitted by `render`. Each destructive variant's `String`
/// is the exact identifier the façade action wrapper expects (container ID,
/// `rmi` reference, volume name) — never a display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerAction {
    Refresh,
    Retry,
    StopContainer(String),
    RemoveContainer(String),
    RemoveImage(String),
    RemoveVolume(String),
    /// Not destructive — `EguiApp` must never open a confirm dialog for
    /// this one (see its dispatch in `render_docker_view`). Triggers a
    /// `docker system df -v` disk scan whose result gets merged into the
    /// current snapshot by volume name.
    ComputeVolumeSizes,
    /// Tick or untick one row. Not destructive.
    ToggleSelection(SelectionKey),
    /// Tick every deletable dormant row. Not destructive.
    SelectDormant,
    /// Untick everything. Not destructive.
    ClearSelection,
    /// Destructive: delete every target, in the order given.
    DeleteSelection(Vec<BatchTarget>),
    /// Switch the visible list tab. Not destructive, and not a refetch: the
    /// snapshot already holds all three lists.
    SelectList(DockerList),
    /// Not destructive — reads the host's listening sockets (`netstat -ano`
    /// or `ss -lntuHp`, see [`crate::net`]) and answers a question no Docker
    /// command can: whether a declared port is already taken by something
    /// that is not a container.
    ScanHostPorts,
}

/// Reason a container's « Arrêter » button is disabled, or `None` when
/// stopping is offered. Mirrors `ContainerState::is_stoppable`.
fn stop_disabled_reason(state: &ContainerState) -> Option<&'static str> {
    if state.is_stoppable() {
        return None;
    }
    match state {
        ContainerState::Unknown(_) => Some("état du conteneur inconnu"),
        _ => Some("conteneur déjà arrêté"),
    }
}

/// Reason a container's « Supprimer » button is disabled, or `None` when
/// removal is offered. Mirrors `ContainerState::is_removable`.
fn remove_disabled_reason(state: &ContainerState) -> Option<&'static str> {
    if state.is_removable() {
        return None;
    }
    match state {
        ContainerState::Unknown(_) => Some("état du conteneur inconnu"),
        _ => Some("conteneur non arrêté"),
    }
}

fn container_state_label(state: &ContainerState) -> String {
    match state {
        ContainerState::Running => "en cours".to_string(),
        ContainerState::Paused => "en pause".to_string(),
        ContainerState::Restarting => "redémarrage".to_string(),
        ContainerState::Exited => "arrêté".to_string(),
        ContainerState::Created => "créé".to_string(),
        ContainerState::Dead => "mort".to_string(),
        ContainerState::Unknown(raw) if raw.is_empty() => "état inconnu".to_string(),
        ContainerState::Unknown(raw) => format!("état inconnu ({raw})"),
    }
}

/// Attach `.on_disabled_hover_text(reason)` only when the button is
/// state-disabled (`reason.is_some()`) — a button disabled purely by the
/// global `busy` guard gets no tooltip, since the section-level spinner
/// already explains that.
fn with_disabled_reason(button: egui::Response, reason: Option<&'static str>) -> egui::Response {
    match reason {
        Some(reason) => button.on_disabled_hover_text(reason),
        None => button,
    }
}

/// A small coloured badge with an explanatory hover text.
fn render_badge(ui: &mut egui::Ui, text: &str, color: egui::Color32, hover: &str) {
    ui.colored_label(color, text).on_hover_text(hover);
}

/// Lay a grid's header out so the table spans the window instead of hugging
/// its text.
///
/// `Grid` sizes each column to its widest cell and has a single global
/// `min_col_width`, so the only way to give one column more room than another
/// is to widen a cell — and the header is the one cell every column has. The
/// weights sum to 1 and are applied to whatever width is left once the column
/// spacing and the scrollbar are paid for; each column keeps a 60 px floor so
/// a narrow window degrades into the horizontal scroll it already had rather
/// than into unreadable slivers.
///
/// The cell is a sized `allocate_ui_with_layout`, **not** a `horizontal` with
/// `set_min_width`: the latter widens the column just the same but leaves the
/// grid's later rows mis-hit — the buttons render at the right place and stop
/// responding to clicks (caught by the two `*_click_emits_*` tests).
fn header_row(ui: &mut egui::Ui, total_width: f32, columns: &[(&str, f32)]) {
    let spacing = ui.spacing().item_spacing.x;
    // The spacing between columns and the vertical scrollbar are both paid
    // for here: a budget that ignores them overflows the viewport by a few
    // pixels, which is enough to arm the horizontal scroll for nothing.
    let usable = (total_width - spacing * (columns.len() + 1) as f32 - 20.0).max(0.0);
    for (text, weight) in columns {
        let width = (usable * weight).max(60.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.strong(*text),
        );
    }
    ui.end_row();
}

/// What every row of every section needs, whatever its family: the tick
/// state, the global busy guard and the dormancy clock.
///
/// One struct rather than four parameters repeated three times — the three
/// sections were already taking the same values in the same order, and the
/// list had grown past what the signature could carry.
struct RowContext<'a> {
    selection: &'a HashSet<SelectionKey>,
    buttons_enabled: bool,
    cutoff: i64,
    now_epoch_secs: i64,
}

/// The per-row tick box, shared by the three sections.
///
/// `enabled` mirrors the row's own « Supprimer » button, so a row can never be
/// batch-deleted when a single deletion would have been refused; `reason`
/// explains the refusal on hover, exactly like [`with_disabled_reason`].
///
/// The box carries no text — the row already names the resource — which also
/// means it is not reachable by label in kittest; per-row selection is covered
/// by the pure helpers ([`sanitize_selection`], [`image_is_selectable`]) while
/// the harness drives the bar's named buttons.
fn selection_checkbox(
    ui: &mut egui::Ui,
    key: SelectionKey,
    ctx: &RowContext<'_>,
    enabled: bool,
    reason: Option<&'static str>,
    actions: &mut Vec<DockerAction>,
) {
    let mut checked = ctx.selection.contains(&key);
    let response = with_disabled_reason(
        ui.add_enabled(enabled, egui::Checkbox::without_text(&mut checked)),
        reason,
    );
    if response.changed() {
        actions.push(DockerAction::ToggleSelection(key));
    }
}

/// The bar above the sections: what is selected, how much it frees, and the
/// three buttons that act on it.
///
/// Drawn *outside* the vertical scroll area on purpose — a selection built by
/// scrolling through three tables must still be actionable without scrolling
/// back up.
fn render_selection_bar(
    ui: &mut egui::Ui,
    state: &DockerViewState<'_>,
    snapshot: &DockerSnapshot,
    cutoff: i64,
    buttons_enabled: bool,
    actions: &mut Vec<DockerAction>,
) {
    let (containers, images, volumes) = selection_counts(state.selection);
    let total = containers + images + volumes;
    let (bytes, partial) = selection_size(state.selection, snapshot);

    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{total} sélectionné(s) · ≈ {} récupérables",
            format_selection_size(bytes, partial)
        ));
        let dormant = dormant_selection(snapshot, cutoff);
        if with_disabled_reason(
            ui.add_enabled(
                buttons_enabled && !dormant.is_empty(),
                egui::Button::new("Tout sélectionner (dormants)"),
            ),
            dormant
                .is_empty()
                .then_some("aucune ressource dormante supprimable"),
        )
        .clicked()
        {
            actions.push(DockerAction::SelectDormant);
        }
        if ui
            .add_enabled(
                buttons_enabled && total > 0,
                egui::Button::new("Effacer la sélection"),
            )
            .clicked()
        {
            actions.push(DockerAction::ClearSelection);
        }
        if ui
            .add_enabled(
                buttons_enabled && total > 0,
                egui::Button::new("Supprimer la sélection"),
            )
            .clicked()
        {
            actions.push(DockerAction::DeleteSelection(selection_targets(
                state.selection,
                snapshot,
            )));
        }
    });

    if state.batch_report.is_empty() {
        return;
    }
    let failures = state
        .batch_report
        .iter()
        .filter(|outcome| outcome.result.is_err())
        .count();
    ui.group(|ui| {
        ui.strong(format!(
            "Dernier lot : {} réussite(s), {failures} échec(s)",
            state.batch_report.len() - failures
        ));
        for outcome in state.batch_report {
            match &outcome.result {
                // A failure keeps docker's own message: « conflict: unable to
                // remove… » tells the user what to do next, « échec » does not.
                Err(error) => {
                    ui.colored_label(ERROR_COLOR, format!("✗ {} — {error}", outcome.label))
                }
                Ok(()) => ui.label(format!("✓ {}", outcome.label)),
            };
        }
    });
}

/// The tab strip that selects which list is shown.
///
/// Each label carries its row count **and**, when the batch selection reaches
/// into that list, its selected count. That second number is not decoration:
/// the grouped deletion spans the three lists (ticking a container is what
/// makes its image selectable), so hiding two of them would otherwise hide
/// part of the batch the « Supprimer la sélection » button is about to run.
fn render_list_tabs(
    ui: &mut egui::Ui,
    state: &DockerViewState<'_>,
    snapshot: &DockerSnapshot,
    port_rows: usize,
    actions: &mut Vec<DockerAction>,
) {
    let (containers, images, volumes) = selection_counts(state.selection);
    let tabs = [
        (
            DockerList::Containers,
            "Conteneurs",
            snapshot.containers.len(),
            containers,
        ),
        (DockerList::Images, "Images", snapshot.images.len(), images),
        (
            DockerList::Volumes,
            "Volumes",
            snapshot.volumes.len(),
            volumes,
        ),
        // Nothing is selectable here, so the `sél.` half of the label never
        // applies: a hard 0 rather than a count that could not move.
        (DockerList::Ports, "Ports", port_rows, 0),
    ];
    ui.horizontal(|ui| {
        for (list, name, total, selected) in tabs {
            let label = if selected > 0 {
                format!("{name} ({total} · {selected} sél.)")
            } else {
                format!("{name} ({total})")
            };
            if ui
                .selectable_label(state.active_list == list, label)
                .clicked()
            {
                actions.push(DockerAction::SelectList(list));
            }
        }
    });
}

fn render_containers_section(
    ui: &mut egui::Ui,
    containers: &[ContainerEntry],
    ctx: &RowContext<'_>,
    conflicts: &[PortConflict],
    actions: &mut Vec<DockerAction>,
) {
    // No heading: the tab label above already names the list (and counts it).
    if containers.is_empty() {
        ui.label("Aucun conteneur.");
        return;
    }
    // Read before entering the scroll area: inside one, `available_width`
    // is the *virtual* width, which is unbounded.
    let total_width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt("docker-containers-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-containers-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    header_row(
                        ui,
                        total_width,
                        &[
                            ("Sél.", 0.04),
                            ("Nom", 0.16),
                            ("Image", 0.22),
                            ("État", 0.08),
                            ("Statut", 0.17),
                            ("Ports", 0.22),
                            ("Action", 0.11),
                        ],
                    );
                    for container in containers {
                        let label = container_label(container);
                        let remove_reason = remove_disabled_reason(&container.state);
                        selection_checkbox(
                            ui,
                            SelectionKey::container(&container.id),
                            ctx,
                            ctx.buttons_enabled && remove_reason.is_none(),
                            remove_reason,
                            actions,
                        );
                        ui.horizontal(|ui| {
                            ui.label(label);
                            let collisions = conflicts_for(conflicts, label);
                            if !collisions.is_empty() {
                                render_badge(
                                    ui,
                                    "⚠ conflit",
                                    CONFLICT_COLOR,
                                    &collisions.join("\n"),
                                );
                            }
                            if container_is_dormant(container, ctx.cutoff) {
                                render_badge(
                                    ui,
                                    &dormant_badge_text(
                                        container.last_activity.as_deref(),
                                        ctx.now_epoch_secs,
                                    ),
                                    DORMANT_COLOR,
                                    container
                                        .last_activity
                                        .as_deref()
                                        .unwrap_or("date inconnue"),
                                );
                            }
                        });
                        ui.label(&container.image);
                        ui.label(container_state_label(&container.state));
                        ui.label(&container.status);
                        let bindings = ports::format_bindings(&container.ports);
                        ui.label(if bindings.is_empty() {
                            "—".to_string()
                        } else {
                            bindings
                        });
                        ui.horizontal(|ui| {
                            let stop_reason = stop_disabled_reason(&container.state);
                            let stop_button = with_disabled_reason(
                                ui.add_enabled(
                                    ctx.buttons_enabled && stop_reason.is_none(),
                                    egui::Button::new("Arrêter"),
                                ),
                                stop_reason,
                            );
                            if stop_button.clicked() {
                                actions.push(DockerAction::StopContainer(container.id.clone()));
                            }

                            let remove_button = with_disabled_reason(
                                ui.add_enabled(
                                    ctx.buttons_enabled && remove_reason.is_none(),
                                    egui::Button::new("Supprimer"),
                                ),
                                remove_reason,
                            );
                            if remove_button.clicked() {
                                actions.push(DockerAction::RemoveContainer(container.id.clone()));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
}

fn render_images_section(
    ui: &mut egui::Ui,
    images: &[ImageEntry],
    ctx: &RowContext<'_>,
    actions: &mut Vec<DockerAction>,
) {
    if images.is_empty() {
        ui.label("Aucune image.");
        return;
    }
    let total_width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt("docker-images-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-images-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    header_row(
                        ui,
                        total_width,
                        &[
                            ("Sél.", 0.04),
                            ("Image", 0.38),
                            ("Taille", 0.10),
                            ("Créée le", 0.16),
                            ("Utilisée", 0.20),
                            ("Action", 0.12),
                        ],
                    );
                    for image in images {
                        // Untagged `<none>:<none>` rows display their short
                        // ID as identity (plan Phase 2 task 6) — the
                        // repo:tag identity is meaningless for them.
                        let label = image_label(image);
                        let selectable = image_is_selectable(image, ctx.selection);
                        selection_checkbox(
                            ui,
                            SelectionKey::image(&image.rmi_reference),
                            ctx,
                            ctx.buttons_enabled && selectable,
                            (!selectable).then_some(
                                "image utilisée : sélectionnez d'abord tous ses conteneurs",
                            ),
                            actions,
                        );
                        ui.horizontal(|ui| {
                            ui.label(label);
                            if image_is_dormant(image, ctx.cutoff) {
                                render_badge(
                                    ui,
                                    &dormant_badge_text(
                                        image.created_iso.as_deref(),
                                        ctx.now_epoch_secs,
                                    ),
                                    DORMANT_COLOR,
                                    image.created_iso.as_deref().unwrap_or("date inconnue"),
                                );
                            }
                        });
                        ui.label(&image.size);
                        ui.label(&image.created);
                        ui.label(if image.used { "oui" } else { "non" });
                        let reason = image
                            .used
                            .then_some("utilisée par un ou plusieurs conteneurs (N indisponible)");
                        let remove_button = with_disabled_reason(
                            ui.add_enabled(
                                ctx.buttons_enabled && reason.is_none(),
                                egui::Button::new("Supprimer l'image"),
                            ),
                            reason,
                        );
                        if remove_button.clicked() {
                            actions.push(DockerAction::RemoveImage(image.rmi_reference.clone()));
                        }
                        ui.end_row();
                    }
                });
        });
}

/// The host-port allocation table: one row per owner per published port,
/// sorted by port.
///
/// Read-only by design. It answers "who holds 8080, where is it written, and
/// is that number pinned or handed out by docker" — the three facts needed to
/// pick a free port by hand. It does not reassign anything: rewriting a
/// `ports:` line would not touch the containers already created from it, so an
/// automatic fix would silently do nothing until a `--force-recreate`.
fn render_ports_section(
    ui: &mut egui::Ui,
    rows: &[PortAllocation],
    host_ports: Option<&[ListeningPort]>,
    buttons_enabled: bool,
    actions: &mut Vec<DockerAction>,
) {
    // Above the empty-table short-circuit on purpose: "no container publishes
    // anything" is exactly when knowing what the *host* holds is most useful,
    // since every port is then free as far as Docker is concerned.
    if ui
        .add_enabled(
            buttons_enabled,
            egui::Button::new(match host_ports {
                None => "Scanner les ports de l'hôte",
                Some(_) => "Rescanner l'hôte",
            }),
        )
        .clicked()
    {
        actions.push(DockerAction::ScanHostPorts);
    }

    if rows.is_empty() {
        ui.label("Aucun port publié.");
        render_host_only_ports(ui, host_ports, rows);
        return;
    }
    ui.label(format!(
        "{} port(s) publié(s). « Dynamique » = docker choisit un port libre à chaque démarrage, le numéro affiché est celui du dernier lancement.",
        rows.len()
    ));

    let mut columns = vec![
        ("Port", 0.08),
        ("Proto", 0.07),
        ("Attribution", 0.11),
        ("Propriétaire", 0.26),
        ("Type", 0.13),
        ("Source", 0.25),
    ];
    if host_ports.is_some() {
        columns.push(("Hôte", 0.10));
    }
    let total_width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt("docker-ports-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-ports-grid")
                .striped(true)
                .min_col_width(70.0)
                .show(ui, |ui| {
                    header_row(ui, total_width, &columns);
                    for row in rows {
                        ui.horizontal(|ui| {
                            ui.label(row.host_port.to_string());
                            if row.conflicting {
                                render_badge(
                                    ui,
                                    "⚠",
                                    CONFLICT_COLOR,
                                    "Ce port est réclamé par plusieurs propriétaires",
                                );
                            }
                        });
                        ui.label(&row.protocol);
                        ui.label(if row.declared {
                            "déclaré"
                        } else {
                            "dynamique"
                        });
                        ui.label(&row.owner);
                        ui.label(match row.kind {
                            OwnerKind::RunningContainer => "conteneur actif",
                            OwnerKind::StoppedContainer => "conteneur arrêté",
                            OwnerKind::DeclaredStack => "stack déclaré",
                        });
                        // An empty source is a container created outside
                        // compose: there is no file to point at, and an em
                        // dash says so without suggesting the lookup failed.
                        ui.label(if row.source.is_empty() {
                            "—"
                        } else {
                            row.source.as_str()
                        });
                        if let Some(listeners) = host_ports {
                            render_host_cell(ui, listeners, row);
                        }
                        ui.end_row();
                    }
                });
        });
    render_host_only_ports(ui, host_ports, rows);
}

/// The « Hôte » cell: whether this port is bound on the machine right now.
///
/// A *running* container's port is of course bound — by Docker's own proxy —
/// so that case is stated plainly and left uncoloured. The one worth a warning
/// is a stopped container or a declared-only stack whose port is already
/// taken: nothing in the Docker data hints at it, and the next `up` is what
/// discovers it.
fn render_host_cell(ui: &mut egui::Ui, listeners: &[ListeningPort], row: &PortAllocation) {
    let Some(listener) = ports::host_listener(listeners, row) else {
        ui.label("libre");
        return;
    };
    let owner = listener
        .owner_label()
        .unwrap_or_else(|| "processus inconnu".to_string());
    if row.kind == OwnerKind::RunningContainer {
        ui.label("occupé").on_hover_text(owner);
        return;
    }
    render_badge(
        ui,
        "occupé",
        CONFLICT_COLOR,
        &format!("Port déjà pris par {owner} — ce port ne sera pas disponible au démarrage"),
    );
}

/// The listeners no container or stack explains — everything else on the
/// machine that is holding a port.
///
/// Rendered under the table rather than merged into it: these rows have no
/// owner, no source and no declaration, so half the columns would be empty,
/// and mixing them in would make the allocation table stop being a table of
/// *allocations*.
fn render_host_only_ports(
    ui: &mut egui::Ui,
    host_ports: Option<&[ListeningPort]>,
    rows: &[PortAllocation],
) {
    let Some(listeners) = host_ports else {
        return;
    };
    let outside = ports::listeners_outside_docker(listeners, rows);
    ui.separator();
    if outside.is_empty() {
        ui.label("Aucun port de l'hôte en dehors de Docker.");
        return;
    }
    ui.label(format!(
        "{} port(s) tenu(s) par l'hôte, hors Docker. Un port listé ici est indisponible pour un conteneur.",
        outside.len()
    ));
    let columns = [("Port", 0.12), ("Proto", 0.12), ("Processus", 0.76)];
    let total_width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt("docker-host-ports-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-host-ports-grid")
                .striped(true)
                .min_col_width(70.0)
                .show(ui, |ui| {
                    header_row(ui, total_width, &columns);
                    for listener in outside {
                        ui.label(listener.port.to_string());
                        ui.label(&listener.protocol);
                        ui.label(
                            listener
                                .owner_label()
                                .unwrap_or_else(|| "processus inconnu".to_string()),
                        );
                        ui.end_row();
                    }
                });
        });
}

fn render_volumes_section(
    ui: &mut egui::Ui,
    volumes: &[VolumeEntry],
    ctx: &RowContext<'_>,
    actions: &mut Vec<DockerAction>,
) {
    // The button stays inside the section rather than moving up to the
    // selection bar: it only makes sense for the list it recomputes.
    if ui
        .add_enabled(
            ctx.buttons_enabled,
            egui::Button::new("Calculer les tailles"),
        )
        .clicked()
    {
        actions.push(DockerAction::ComputeVolumeSizes);
    }
    if volumes.is_empty() {
        ui.label("Aucun volume.");
        return;
    }
    // Sizes come from a ~6 s `docker system df -v` scan run only on demand,
    // so before the first « Calculer les tailles » there is nothing to show.
    // The column is then dropped entirely rather than filled with
    // placeholders: a grid of « ? » reads as a failed measurement, when in
    // fact no measurement was ever asked for. Nom absorbs the freed width.
    let sizes_known = volumes.iter().any(|volume| volume.size.is_some());
    let mut columns = vec![
        ("Sél.", 0.04),
        ("Nom", if sizes_known { 0.42 } else { 0.54 }),
        ("Driver", 0.12),
        ("Orphelin", 0.12),
    ];
    if sizes_known {
        columns.push(("Taille", 0.12));
    }
    columns.push(("Action", 0.18));

    let total_width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt("docker-volumes-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-volumes-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    header_row(ui, total_width, &columns);
                    for volume in volumes {
                        let reason = (!volume.orphan).then_some("volume rattaché à un conteneur");
                        selection_checkbox(
                            ui,
                            SelectionKey::volume(&volume.name),
                            ctx,
                            ctx.buttons_enabled && reason.is_none(),
                            reason,
                            actions,
                        );
                        ui.horizontal(|ui| {
                            ui.label(&volume.name);
                            if volume_is_dormant(volume, ctx.cutoff) {
                                render_badge(
                                    ui,
                                    &dormant_badge_text(
                                        volume.created_iso.as_deref(),
                                        ctx.now_epoch_secs,
                                    ),
                                    DORMANT_COLOR,
                                    volume.created_iso.as_deref().unwrap_or("date inconnue"),
                                );
                            }
                        });
                        ui.label(&volume.driver);
                        ui.label(if volume.orphan { "oui" } else { "non" });
                        if sizes_known {
                            // `docker system df -v` can still skip a volume it
                            // cannot stat; an em dash marks that hole without
                            // claiming the whole scan failed.
                            ui.label(volume.size.as_deref().unwrap_or("—"));
                        }
                        let remove_button = with_disabled_reason(
                            ui.add_enabled(
                                ctx.buttons_enabled && reason.is_none(),
                                egui::Button::new("Supprimer le volume"),
                            ),
                            reason,
                        );
                        if remove_button.clicked() {
                            actions.push(DockerAction::RemoveVolume(volume.name.clone()));
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Draw one frame of the Docker tab from `state` and return every intent the
/// user clicked this frame — zero `EguiApp` access, exactly the
/// `cleanup_view::render` contract.
pub fn render(ui: &mut egui::Ui, state: &DockerViewState<'_>) -> Vec<DockerAction> {
    let mut actions = Vec::new();
    let buttons_enabled = !state.busy;

    ui.horizontal(|ui| {
        ui.heading("Docker");
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Actualiser"))
            .clicked()
        {
            actions.push(DockerAction::Refresh);
        }
        if state.busy {
            ui.spinner();
        }
    });

    if let Some(error) = state.error {
        ui.colored_label(ERROR_COLOR, error);
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Réessayer"))
            .clicked()
        {
            actions.push(DockerAction::Retry);
        }
        return actions;
    }

    let Some(snapshot) = state.snapshot else {
        ui.label(if state.busy {
            "Chargement des données Docker…"
        } else {
            "Aucune donnée Docker chargée."
        });
        return actions;
    };

    // Every publishing container takes part, running or not — a stopped one
    // still holds the host port it declared, and would fail to start next to a
    // rival. What `find_conflicts` filters out is narrower: the *dynamic*
    // ports of stopped containers, which docker will reassign anyway.
    let mut owners = container_port_owners(snapshot);
    owners.extend_from_slice(state.extra_port_owners);
    let conflicts = ports::find_conflicts(&owners);
    let allocations = ports::port_allocations(&owners, &conflicts);
    let cutoff = cutoff_epoch(state.now_epoch_secs, state.dormant_after_days);

    let ctx = RowContext {
        selection: state.selection,
        buttons_enabled,
        cutoff,
        now_epoch_secs: state.now_epoch_secs,
    };

    ui.separator();
    render_selection_bar(ui, state, snapshot, cutoff, buttons_enabled, &mut actions);
    ui.separator();
    render_list_tabs(ui, state, snapshot, allocations.len(), &mut actions);

    egui::ScrollArea::vertical()
        // Fills the tab in both directions: the sections below size their
        // grids from the width this hands them, so a shrinking scroll area
        // would make the tables narrower than the window on every frame.
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.separator();
            match state.active_list {
                DockerList::Containers => render_containers_section(
                    ui,
                    &snapshot.containers,
                    &ctx,
                    &conflicts,
                    &mut actions,
                ),
                DockerList::Images => {
                    render_images_section(ui, &snapshot.images, &ctx, &mut actions)
                }
                DockerList::Volumes => {
                    render_volumes_section(ui, &snapshot.volumes, &ctx, &mut actions)
                }
                DockerList::Ports => render_ports_section(
                    ui,
                    &allocations,
                    state.host_ports,
                    buttons_enabled,
                    &mut actions,
                ),
            }
        });

    actions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    // --- ImageEntry::is_untagged --------------------------------------------

    #[test]
    fn is_untagged_detects_the_none_none_placeholder() {
        let entry = image_entry("581c17389e54", "<none>:<none>", false, "581c17389e54");
        assert!(entry.is_untagged());
    }

    #[test]
    fn is_untagged_is_false_for_a_tagged_row() {
        let entry = image_entry("581c17389e54", "nginx:alpine", false, "nginx:alpine");
        assert!(!entry.is_untagged());
    }

    // --- stop_disabled_reason / remove_disabled_reason -----------------------

    #[test]
    fn stop_disabled_reason_is_none_for_every_stoppable_state() {
        for state in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
        ] {
            assert_eq!(stop_disabled_reason(&state), None, "state {state:?}");
        }
    }

    #[test]
    fn stop_disabled_reason_is_some_for_every_removable_state() {
        for state in [
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
        ] {
            assert!(stop_disabled_reason(&state).is_some(), "state {state:?}");
        }
    }

    #[test]
    fn stop_disabled_reason_explains_unknown_state_specifically() {
        let reason = stop_disabled_reason(&ContainerState::Unknown("mystère".to_string()))
            .expect("unknown state must disable stop");
        assert!(reason.contains("inconnu"));
    }

    #[test]
    fn remove_disabled_reason_is_none_for_every_removable_state() {
        for state in [
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
        ] {
            assert_eq!(remove_disabled_reason(&state), None, "state {state:?}");
        }
    }

    #[test]
    fn remove_disabled_reason_is_some_for_every_stoppable_state() {
        for state in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
        ] {
            assert!(remove_disabled_reason(&state).is_some(), "state {state:?}");
        }
    }

    #[test]
    fn remove_disabled_reason_explains_unknown_state_specifically() {
        let reason = remove_disabled_reason(&ContainerState::Unknown("mystère".to_string()))
            .expect("unknown state must disable remove");
        assert!(reason.contains("inconnu"));
    }

    // --- container_state_label ------------------------------------------------

    #[test]
    fn container_state_label_covers_every_known_variant_without_panicking() {
        let states = [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
            ContainerState::Unknown("weird".to_string()),
            ContainerState::Unknown(String::new()),
        ];
        for state in &states {
            assert!(!container_state_label(state).is_empty());
        }
    }

    // --- façade — Linux delegation sanity (this dev machine has docker) ------

    #[test]
    fn available_delegates_to_linux_binary_detection() {
        assert_eq!(available(), crate::docker::engine::binary_available());
    }

    #[test]
    fn fetch_through_facade_succeeds_on_this_machine_when_docker_is_available() {
        if !available() {
            eprintln!("docker introuvable sur cette machine: test ignoré");
            return;
        }
        // Smoke test: the façade must compile the Result<_, DockerError> ->
        // Result<_, String> conversion end to end without panicking, and a
        // successful fetch on this reference machine (Docker 29.7.2, daemon
        // reachable — see `crate::docker::engine`'s own real-machine tests)
        // must actually come back `Ok`.
        let snapshot = fetch();
        assert!(
            snapshot.is_ok(),
            "expected fetch() to succeed on this machine, got {snapshot:?}"
        );
    }

    #[test]
    fn remove_container_through_facade_reports_a_readable_error_for_a_bogus_id() {
        if !available() {
            eprintln!("docker introuvable sur cette machine: test ignoré");
            return;
        }
        let result = remove_container("this-container-id-does-not-exist-devtoolbox");
        let error =
            result.expect_err("removing a nonexistent container id must fail, not panic/succeed");
        assert!(!error.is_empty(), "façade error string must not be empty");
    }

    // --- render(): fixtures ----------------------------------------------------

    fn container_entry(id: &str, name: &str, state: ContainerState) -> ContainerEntry {
        ContainerEntry {
            id: id.to_string(),
            name: name.to_string(),
            image: format!("{name}-image:latest"),
            state,
            status: "Up 3 hours".to_string(),
            rw_size: "767kB".to_string(),
            ports: Vec::new(),
            last_activity: None,
            compose_project: None,
            compose_files: Vec::new(),
            compose_service: None,
            declared_host_ports: BTreeSet::new(),
            exit_code: None,
        }
    }

    fn image_entry(id: &str, identity: &str, used: bool, rmi_reference: &str) -> ImageEntry {
        ImageEntry {
            id: id.to_string(),
            identity: identity.to_string(),
            size: "10MB".to_string(),
            created: "hier".to_string(),
            used,
            rmi_reference: rmi_reference.to_string(),
            created_iso: None,
            used_by: Vec::new(),
        }
    }

    fn volume_entry(name: &str, orphan: bool) -> VolumeEntry {
        VolumeEntry {
            name: name.to_string(),
            driver: "local".to_string(),
            orphan,
            size: None,
            created_iso: None,
        }
    }

    fn volume_entry_with_size(name: &str, orphan: bool, size: &str) -> VolumeEntry {
        VolumeEntry {
            size: Some(size.to_string()),
            ..volume_entry(name, orphan)
        }
    }

    /// A fixed clock for every render test: 2026-08-21T00:00:00Z. Injected
    /// rather than read from the system so the dormancy badges are assertable.
    const TEST_NOW: i64 = 1_787_270_400;

    struct State {
        snapshot: Option<DockerSnapshot>,
        error: Option<String>,
        busy: bool,
        actions: Vec<DockerAction>,
        dormant_after_days: u32,
        now_epoch_secs: i64,
        selection: HashSet<SelectionKey>,
        batch_report: Vec<BatchOutcome>,
        active_list: DockerList,
        host_ports: Option<Vec<ListeningPort>>,
    }

    impl State {
        fn with_snapshot(snapshot: DockerSnapshot) -> Self {
            State {
                snapshot: Some(snapshot),
                error: None,
                busy: false,
                actions: Vec::new(),
                dormant_after_days: 60,
                now_epoch_secs: TEST_NOW,
                selection: HashSet::new(),
                batch_report: Vec::new(),
                active_list: DockerList::Containers,
                host_ports: None,
            }
        }

        /// Hand the view a host scan result, as `EguiApp` does after a
        /// `ScanHostPorts`. `None` (the default) is "never scanned".
        fn with_host_ports(mut self, ports: Vec<ListeningPort>) -> Self {
            self.host_ports = Some(ports);
            self
        }

        /// Open the harness straight on one list, for the many tests whose
        /// subject is an image or a volume row: clicking through the tab
        /// strip first would only re-test `render_list_tabs`.
        fn on_list(mut self, list: DockerList) -> Self {
            self.active_list = list;
            self
        }
    }

    fn build_harness(state: State) -> Harness<'static, State> {
        // A busy view renders a spinner, which keeps requesting repaints
        // forever; `Harness::run()`'s settle-check would never succeed for
        // it (it panics past `max_steps`), so busy harnesses are stepped a
        // fixed number of frames instead — still enough for widgets to be
        // laid out and queryable/clickable.
        let busy = state.busy;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1000.0, 900.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let view_state = DockerViewState {
                        snapshot: state.snapshot.as_ref(),
                        error: state.error.as_deref(),
                        busy: state.busy,
                        dormant_after_days: state.dormant_after_days,
                        now_epoch_secs: state.now_epoch_secs,
                        extra_port_owners: &[],
                        selection: &state.selection,
                        batch_report: &state.batch_report,
                        active_list: state.active_list,
                        host_ports: state.host_ports.as_deref(),
                    };
                    let emitted = render(ui, &view_state);
                    // Applied here, like `EguiApp` does, so a test can click a
                    // tab and see the next frame show that list.
                    for action in &emitted {
                        if let DockerAction::SelectList(list) = action {
                            state.active_list = *list;
                        }
                    }
                    state.actions.extend(emitted);
                },
                state,
            );
        if busy {
            harness.run_steps(2);
        } else {
            harness.run();
        }
        harness
    }

    // --- render(): gating — used image / non-orphan volume / running-        --
    // --- container removal / unknown state never emit a destructive action ---

    #[test]
    fn used_image_delete_button_exists_but_is_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![image_entry("aaa", "nginx:alpine", true, "nginx:alpine")],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Images));
        assert_eq!(
            harness.query_all_by_label("Supprimer l'image").count(),
            1,
            "the button must still be rendered, just disabled"
        );
        harness.get_by_label("Supprimer l'image").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a used image must never emit a RemoveImage action"
        );
    }

    #[test]
    fn non_orphan_volume_delete_button_exists_but_is_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("proxy_certs", false)],
        };
        let mut harness =
            build_harness(State::with_snapshot(snapshot).on_list(DockerList::Volumes));
        assert_eq!(
            harness.query_all_by_label("Supprimer le volume").count(),
            1,
            "the button must still be rendered, just disabled"
        );
        harness.get_by_label("Supprimer le volume").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a non-orphan volume must never emit a RemoveVolume action"
        );
    }

    #[test]
    fn running_container_remove_button_exists_but_is_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Running)],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(
            harness.query_all_by_label("Supprimer").count(),
            1,
            "the remove button must still be rendered, just disabled"
        );
        harness.get_by_label("Supprimer").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a running container must never emit a RemoveContainer action"
        );
        // Sanity: the stop button on the same row is the enabled one.
        assert_eq!(harness.query_all_by_label("Arrêter").count(), 1);
    }

    #[test]
    fn unknown_state_container_has_both_buttons_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry(
                "c1",
                "mystery",
                ContainerState::Unknown("future-state".to_string()),
            )],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("Arrêter").count(), 1);
        assert_eq!(harness.query_all_by_label("Supprimer").count(), 1);
        harness.get_by_label("Arrêter").click();
        harness.get_by_label("Supprimer").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "an unknown container state must offer no action at all"
        );
    }

    // --- render(): active buttons emit exactly the right action --------------

    #[test]
    fn stoppable_container_stop_click_emits_stop_container_with_its_id() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Running)],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Arrêter").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::StopContainer("c1".to_string())]
        );
    }

    #[test]
    fn removable_container_remove_click_emits_remove_container_with_its_id() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Exited)],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Supprimer").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveContainer("c1".to_string())]
        );
    }

    #[test]
    fn unused_image_remove_click_emits_remove_image_with_its_rmi_reference() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![image_entry("aaa", "nginx:alpine", false, "nginx:alpine")],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Images));
        harness.get_by_label("Supprimer l'image").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveImage("nginx:alpine".to_string())]
        );
    }

    #[test]
    fn untagged_image_remove_click_emits_remove_image_with_its_id() {
        // Untagged `<none>:<none>` rows are removed by ID (`rmi_reference`),
        // never by their meaningless placeholder identity.
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![image_entry(
                "581c17389e54",
                "<none>:<none>",
                false,
                "581c17389e54",
            )],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Images));
        // The row identity shown must be the short ID, not "<none>:<none>".
        assert_eq!(harness.query_all_by_label("581c17389e54").count(), 1);
        assert_eq!(harness.query_all_by_label("<none>:<none>").count(), 0);
        harness.get_by_label("Supprimer l'image").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveImage("581c17389e54".to_string())]
        );
    }

    #[test]
    fn orphan_volume_remove_click_emits_remove_volume_with_its_name() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let mut harness =
            build_harness(State::with_snapshot(snapshot).on_list(DockerList::Volumes));
        harness.get_by_label("Supprimer le volume").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveVolume("dangling-vol".to_string())]
        );
    }

    #[test]
    fn volume_size_column_is_absent_until_sizes_are_computed() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Volumes));
        assert_eq!(
            harness.query_all_by_label("Taille").count(),
            0,
            "no size has been computed, so the column must not be drawn"
        );
        assert_eq!(
            harness.query_all_by_label("?").count(),
            0,
            "an uncomputed size must never render as a placeholder"
        );
    }

    #[test]
    fn volume_size_column_shows_computed_size_when_present() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry_with_size("dangling-vol", true, "6.64MB")],
        };
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Volumes));
        assert_eq!(harness.query_all_by_label("Taille").count(), 1);
        assert_eq!(harness.query_all_by_label("6.64MB").count(), 1);
        assert_eq!(harness.query_all_by_label("?").count(), 0);
    }

    /// One volume measured is enough to raise the column; the volumes
    /// `docker system df -v` skipped keep an em dash, not a « ? ».
    #[test]
    fn volume_size_column_marks_the_entries_the_scan_skipped() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![
                volume_entry_with_size("measured-vol", true, "6.64MB"),
                volume_entry("skipped-vol", true),
            ],
        };
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Volumes));
        assert_eq!(harness.query_all_by_label("Taille").count(), 1);
        assert_eq!(harness.query_all_by_label("—").count(), 1);
    }

    #[test]
    fn calculer_les_tailles_click_emits_compute_volume_sizes() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let mut harness =
            build_harness(State::with_snapshot(snapshot).on_list(DockerList::Volumes));
        harness.get_by_label("Calculer les tailles").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::ComputeVolumeSizes]
        );
    }

    #[test]
    fn calculer_les_tailles_is_rendered_even_with_no_volumes() {
        // The button lives at the top of the Volumes tab, not gated by an
        // empty list.
        let harness = build_harness(
            State::with_snapshot(DockerSnapshot::default()).on_list(DockerList::Volumes),
        );
        assert_eq!(
            harness.query_all_by_label("Calculer les tailles").count(),
            1
        );
    }

    // --- render(): refresh / retry / busy / empty / error --------------------

    #[test]
    fn actualiser_button_emits_refresh() {
        let mut harness = build_harness(State::with_snapshot(DockerSnapshot::default()));
        harness.get_by_label("Actualiser").click();
        harness.run();
        assert_eq!(harness.state().actions, vec![DockerAction::Refresh]);
    }

    #[test]
    fn error_state_shows_reessayer_and_emits_retry_on_click() {
        let state = State {
            snapshot: None,
            error: Some("daemon Docker inaccessible: connection refused".to_string()),
            busy: false,
            actions: Vec::new(),
            dormant_after_days: 60,
            now_epoch_secs: TEST_NOW,
            selection: HashSet::new(),
            batch_report: Vec::new(),
            active_list: DockerList::Containers,
            host_ports: None,
        };
        let mut harness = build_harness(state);
        assert_eq!(harness.query_all_by_label("Réessayer").count(), 1);
        // Nothing below the banner is rendered while an error is shown —
        // neither the selection bar nor the tab strip, hence neither list.
        assert_eq!(
            harness
                .query_all_by_label("Tout sélectionner (dormants)")
                .count(),
            0
        );
        assert_eq!(harness.query_all_by_label("Conteneurs (0)").count(), 0);
        harness.get_by_label("Réessayer").click();
        harness.run();
        assert_eq!(harness.state().actions, vec![DockerAction::Retry]);
    }

    /// One list is on screen at a time now, so each list's buttons are
    /// checked on its own tab. The tab strip itself stays clickable while
    /// busy — looking at another list changes nothing on the daemon — so
    /// `SelectList` is excluded from the "nothing was emitted" assertion.
    #[test]
    fn busy_state_disables_every_button_so_no_action_is_emitted() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Running)],
            images: vec![image_entry("aaa", "nginx:alpine", false, "nginx:alpine")],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        for (list, buttons) in [
            (DockerList::Containers, &["Actualiser", "Arrêter"][..]),
            (DockerList::Images, &["Supprimer l'image"][..]),
            (
                DockerList::Volumes,
                &["Supprimer le volume", "Calculer les tailles"][..],
            ),
        ] {
            let state = State {
                snapshot: Some(snapshot.clone()),
                error: None,
                busy: true,
                actions: Vec::new(),
                dormant_after_days: 60,
                now_epoch_secs: TEST_NOW,
                selection: HashSet::new(),
                batch_report: Vec::new(),
                active_list: list,
                host_ports: None,
            };
            let mut harness = build_harness(state);
            for button in buttons {
                harness.get_by_label(button).click();
            }
            // Same spinner-driven continuous-repaint reason as in
            // build_harness: step a fixed number of frames rather than
            // `run()`'s settle-check.
            harness.run_steps(2);
            let emitted: Vec<_> = harness
                .state()
                .actions
                .iter()
                .filter(|action| !matches!(action, DockerAction::SelectList(_)))
                .collect();
            assert!(
                emitted.is_empty(),
                "busy must gate every button of {list:?}, including otherwise-allowed ones: {emitted:?}"
            );
        }
    }

    #[test]
    fn empty_snapshot_shows_a_french_placeholder_on_each_list_tab() {
        for (list, placeholder) in [
            (DockerList::Containers, "Aucun conteneur."),
            (DockerList::Images, "Aucune image."),
            (DockerList::Volumes, "Aucun volume."),
        ] {
            let harness =
                build_harness(State::with_snapshot(DockerSnapshot::default()).on_list(list));
            assert_eq!(harness.query_all_by_label(placeholder).count(), 1);
        }
    }

    // --- list tabs ----------------------------------------------------------

    #[test]
    fn each_tab_is_labelled_with_its_row_count_and_shows_only_its_own_list() {
        let snapshot = DockerSnapshot {
            containers: vec![
                container_entry("c1", "proxy", ContainerState::Running),
                container_entry("c2", "db", ContainerState::Exited),
            ],
            images: vec![image_entry("aaa", "nginx:alpine", false, "nginx:alpine")],
            volumes: vec![
                volume_entry("dangling-vol", true),
                volume_entry("kept-vol", false),
                volume_entry("other-vol", true),
            ],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("Conteneurs (2)").count(), 1);
        assert_eq!(harness.query_all_by_label("Images (1)").count(), 1);
        assert_eq!(harness.query_all_by_label("Volumes (3)").count(), 1);
        // Containers is the default tab: its rows are up, the other lists'
        // are not laid out at all.
        assert_eq!(harness.query_all_by_label("proxy").count(), 1);
        assert_eq!(harness.query_all_by_label("nginx:alpine").count(), 0);

        harness.get_by_label("Images (1)").click();
        harness.run();
        assert_eq!(
            harness.state().active_list,
            DockerList::Images,
            "clicking a tab must emit SelectList"
        );
        assert_eq!(harness.query_all_by_label("nginx:alpine").count(), 1);
        assert_eq!(harness.query_all_by_label("proxy").count(), 0);
    }

    /// The batch spans the three lists, so a tab whose rows are hidden still
    /// has to say how many of them are ticked — otherwise « Supprimer la
    /// sélection » would delete resources the user cannot see selected.
    #[test]
    fn a_tab_reports_the_rows_selected_in_its_own_list() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Exited)],
            images: vec![image_entry("aaa", "nginx:alpine", false, "nginx:alpine")],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let mut state = State::with_snapshot(snapshot);
        state.selection.insert(SelectionKey {
            kind: ResourceKind::Volume,
            id: "dangling-vol".to_string(),
        });
        let harness = build_harness(state);
        assert_eq!(
            harness.query_all_by_label("Volumes (1 · 1 sél.)").count(),
            1
        );
        // A list with nothing ticked keeps the plain count.
        assert_eq!(harness.query_all_by_label("Conteneurs (1)").count(), 1);
    }

    #[test]
    fn no_snapshot_and_no_error_shows_a_loading_or_empty_placeholder() {
        let state = State {
            snapshot: None,
            error: None,
            busy: false,
            actions: Vec::new(),
            dormant_after_days: 60,
            now_epoch_secs: TEST_NOW,
            selection: HashSet::new(),
            batch_report: Vec::new(),
            active_list: DockerList::Containers,
            host_ports: None,
        };
        let harness = build_harness(state);
        assert_eq!(
            harness
                .query_all_by_label("Aucune donnée Docker chargée.")
                .count(),
            1
        );
    }

    // --- parse_rfc3339 ---------------------------------------------------------

    #[test]
    fn parse_rfc3339_reads_the_utc_form_returned_by_container_and_image_inspect() {
        // 2026-08-21T00:00:00Z is TEST_NOW by construction.
        assert_eq!(parse_rfc3339("2026-08-21T00:00:00Z"), Some(TEST_NOW));
        assert_eq!(parse_rfc3339("2026-08-21t00:00:00z"), Some(TEST_NOW));
    }

    #[test]
    fn parse_rfc3339_applies_a_positive_offset_as_returned_by_volume_inspect() {
        // Measured on this machine: `docker volume inspect` returns a local
        // offset. 02:00+02:00 is the same instant as 00:00Z.
        assert_eq!(parse_rfc3339("2026-08-21T02:00:00+02:00"), Some(TEST_NOW));
    }

    #[test]
    fn parse_rfc3339_applies_a_negative_offset() {
        assert_eq!(
            parse_rfc3339("2026-08-20T18:30:00-05:30"),
            Some(TEST_NOW),
            "18:30-05:30 the day before is midnight UTC"
        );
    }

    #[test]
    fn parse_rfc3339_drops_fractional_seconds_without_rejecting_the_stamp() {
        assert_eq!(
            parse_rfc3339("2026-08-21T00:00:00.123456789Z"),
            Some(TEST_NOW)
        );
        assert_eq!(parse_rfc3339("2026-08-21T00:00:00.5+00:00"), Some(TEST_NOW));
    }

    #[test]
    fn parse_rfc3339_rejects_the_docker_zero_date() {
        assert_eq!(parse_rfc3339(ZERO_DOCKER_DATE), None);
    }

    #[test]
    fn parse_rfc3339_rejects_garbage_rather_than_guessing() {
        for garbage in [
            "",
            "hier",
            "2026-08-21",
            "2026-08-21T00:00:00",      // no zone at all
            "2026-08-21T00:00:00+0200", // no colon in the offset
            "2026-13-01T00:00:00Z",     // month out of range
            "2026-08-32T00:00:00Z",     // day out of range
            "2026-08-21T24:00:00Z",     // hour out of range
            "2026-08-21T00:60:00Z",     // minute out of range
            "2026-08-21T00:00:00.Z",    // dot with no digit
            "20260821T000000Z",         // no separators
        ] {
            assert_eq!(parse_rfc3339(garbage), None, "garbage: {garbage:?}");
        }
    }

    #[test]
    fn parse_rfc3339_handles_a_leap_day_and_a_month_boundary() {
        let leap_day = parse_rfc3339("2024-02-29T00:00:00Z").expect("2024 is a leap year");
        let march_first = parse_rfc3339("2024-03-01T00:00:00Z").expect("valid date");
        assert_eq!(march_first - leap_day, 86_400);

        let end_of_january = parse_rfc3339("2026-01-31T00:00:00Z").expect("valid date");
        let start_of_february = parse_rfc3339("2026-02-01T00:00:00Z").expect("valid date");
        assert_eq!(start_of_february - end_of_january, 86_400);
    }

    // --- cutoff_epoch / days_since / is_dormant --------------------------------

    #[test]
    fn cutoff_epoch_walks_back_whole_days() {
        assert_eq!(cutoff_epoch(TEST_NOW, 0), TEST_NOW);
        assert_eq!(cutoff_epoch(TEST_NOW, 60), TEST_NOW - 60 * 86_400);
    }

    #[test]
    fn days_since_counts_whole_days_across_a_month_boundary() {
        // 2026-07-22 -> 2026-08-21 is 30 days (July has 31 days).
        assert_eq!(days_since("2026-07-22T00:00:00Z", TEST_NOW), Some(30));
    }

    #[test]
    fn days_since_counts_whole_days_across_a_leap_year() {
        // 2024-02-29 counted at 2025-02-28: 365 days.
        let now = parse_rfc3339("2025-02-28T00:00:00Z").expect("valid date");
        assert_eq!(days_since("2024-02-29T00:00:00Z", now), Some(365));
    }

    #[test]
    fn days_since_clamps_a_future_date_to_zero_instead_of_going_negative() {
        assert_eq!(days_since("2027-01-01T00:00:00Z", TEST_NOW), Some(0));
    }

    #[test]
    fn days_since_is_none_on_an_undatable_stamp() {
        assert_eq!(days_since(ZERO_DOCKER_DATE, TEST_NOW), None);
        assert_eq!(days_since("hier", TEST_NOW), None);
    }

    #[test]
    fn is_dormant_never_badges_a_row_that_could_not_be_dated() {
        let cutoff = cutoff_epoch(TEST_NOW, 60);
        assert!(!is_dormant(None, cutoff));
        assert!(!is_dormant(Some(ZERO_DOCKER_DATE), cutoff));
        assert!(!is_dormant(Some("n/a"), cutoff));
    }

    #[test]
    fn is_dormant_is_exclusive_on_the_cutoff_itself() {
        let date = "2026-06-22T00:00:00Z";
        let epoch = parse_rfc3339(date).expect("valid date");
        assert!(
            !is_dormant(Some(date), epoch),
            "a date landing exactly on the cutoff is not yet dormant"
        );
        assert!(
            is_dormant(Some(date), epoch + 1),
            "one second past the cutoff"
        );
        assert!(!is_dormant(Some(date), epoch - 1), "one second before it");
    }

    // --- the three dormancy predicates ----------------------------------------

    #[test]
    fn a_running_container_is_never_dormant_however_old_its_dates_are() {
        let cutoff = cutoff_epoch(TEST_NOW, 60);
        for state in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
        ] {
            let mut entry = container_entry("abc", "lab-db", state);
            entry.last_activity = Some("2020-01-01T00:00:00Z".to_string());
            assert!(
                !container_is_dormant(&entry, cutoff),
                "a live container must never be badged dormant"
            );
        }
    }

    #[test]
    fn a_stopped_container_older_than_the_cutoff_is_dormant() {
        let cutoff = cutoff_epoch(TEST_NOW, 60);
        let mut entry = container_entry("abc", "lab-db", ContainerState::Exited);
        entry.last_activity = Some("2026-01-01T00:00:00Z".to_string());
        assert!(container_is_dormant(&entry, cutoff));

        entry.last_activity = Some("2026-08-20T00:00:00Z".to_string());
        assert!(
            !container_is_dormant(&entry, cutoff),
            "yesterday is not dormant"
        );
    }

    #[test]
    fn a_used_image_is_never_dormant_however_old_it_is() {
        let cutoff = cutoff_epoch(TEST_NOW, 60);
        let mut entry = image_entry("aaa", "nginx:alpine", true, "nginx:alpine");
        entry.created_iso = Some("2019-01-01T00:00:00Z".to_string());
        assert!(!image_is_dormant(&entry, cutoff));

        entry.used = false;
        assert!(image_is_dormant(&entry, cutoff));
    }

    #[test]
    fn a_non_orphan_volume_is_never_dormant_however_old_it_is() {
        let cutoff = cutoff_epoch(TEST_NOW, 60);
        let mut entry = volume_entry("lab-data", false);
        entry.created_iso = Some("2019-01-01T00:00:00Z".to_string());
        assert!(!volume_is_dormant(&entry, cutoff));

        entry.orphan = true;
        assert!(volume_is_dormant(&entry, cutoff));
    }

    // --- parse_exit_code -------------------------------------------------------

    #[test]
    fn parse_exit_code_reads_the_code_inside_an_exited_status() {
        assert_eq!(parse_exit_code("Exited (0) 3 hours ago"), Some(0));
        assert_eq!(parse_exit_code("Exited (137) 2 days ago"), Some(137));
    }

    #[test]
    fn parse_exit_code_also_reads_a_restarting_status_which_carries_one_too() {
        // `Restarting (1) 5 seconds ago` is not an `Exited` status, but the
        // code in its parentheses is the last exit code all the same — and
        // `is_failing` needs it to tell a crash-loop from a healthy restart.
        assert_eq!(parse_exit_code("Restarting (1) 5 seconds ago"), Some(1));
    }

    #[test]
    fn parse_exit_code_yields_none_when_the_status_carries_no_code() {
        assert_eq!(parse_exit_code("Up 4 hours"), None);
        assert_eq!(parse_exit_code("Created"), None);
        // Malformed shapes must return None, never panic on the slice.
        assert_eq!(parse_exit_code("Exited (abc) 1 hour ago"), None);
        assert_eq!(parse_exit_code("Exited (12 1 hour ago"), None);
    }

    // --- container_port_owners / conflicts_for --------------------------------

    #[test]
    fn container_port_owners_skips_containers_that_publish_nothing() {
        let mut published = container_entry("aaa", "lab", ContainerState::Running);
        published.ports = ports::parse_ps_ports("0.0.0.0:5656->5656/tcp");
        let silent = container_entry("bbb", "tasks", ContainerState::Running);
        let snapshot = DockerSnapshot {
            containers: vec![published, silent],
            images: vec![],
            volumes: vec![],
        };
        let owners = container_port_owners(&snapshot);
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].label, "lab");
        assert!(matches!(owners[0].kind, OwnerKind::RunningContainer));
    }

    // --- render(): the Ports column and the two badges ------------------------

    #[test]
    fn the_ports_column_shows_the_real_bindings_and_an_em_dash_otherwise() {
        let mut published = container_entry("aaa", "lab", ContainerState::Running);
        published.ports = ports::parse_ps_ports("0.0.0.0:5656->5656/tcp");
        let silent = container_entry("bbb", "tasks", ContainerState::Running);
        let snapshot = DockerSnapshot {
            containers: vec![published, silent],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("Ports").count(), 1, "header");
        assert_eq!(
            harness.query_all_by_label("0.0.0.0:5656->5656/tcp").count(),
            1
        );
        assert_eq!(
            harness.query_all_by_label("—").count(),
            1,
            "a container publishing nothing shows an em dash"
        );
    }

    #[test]
    fn two_running_containers_on_the_same_host_port_both_get_a_conflict_badge() {
        let mut lab = container_entry("aaa", "lab", ContainerState::Running);
        lab.ports = ports::parse_ps_ports("0.0.0.0:5656->5656/tcp");
        let mut tasks = container_entry("bbb", "tasks", ContainerState::Running);
        tasks.ports = ports::parse_ps_ports("0.0.0.0:5656->3000/tcp");
        let snapshot = DockerSnapshot {
            containers: vec![lab, tasks],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(
            harness.query_all_by_label("⚠ conflit").count(),
            2,
            "both sides of a collision must be flagged"
        );
    }

    #[test]
    fn a_container_alone_on_its_host_port_gets_no_conflict_badge() {
        let mut lab = container_entry("aaa", "lab", ContainerState::Running);
        lab.ports = ports::parse_ps_ports("0.0.0.0:5656->5656/tcp");
        let mut tasks = container_entry("bbb", "tasks", ContainerState::Running);
        tasks.ports = ports::parse_ps_ports("0.0.0.0:5657->5656/tcp");
        let snapshot = DockerSnapshot {
            containers: vec![lab, tasks],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("⚠ conflit").count(), 0);
    }

    // --- the ports tab -------------------------------------------------------

    /// A stopped container that never declared a host port is exactly the
    /// case the conflict detector drops. The table must still list it, and
    /// say *why* it was dropped — otherwise the port simply vanishes.
    #[test]
    fn the_ports_tab_lists_a_dynamic_port_as_dynamic_and_unflagged() {
        let mut mysql = container_entry("aaa", "mauceri-mysql-1", ContainerState::Exited);
        mysql.ports = ports::parse_ps_ports("0.0.0.0:32768->3306/tcp");
        let snapshot = DockerSnapshot {
            containers: vec![mysql],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Ports));
        assert_eq!(harness.query_all_by_label("Ports (1)").count(), 1);
        assert_eq!(harness.query_all_by_label("32768").count(), 1);
        assert_eq!(harness.query_all_by_label("dynamique").count(), 1);
        assert_eq!(harness.query_all_by_label("conteneur arrêté").count(), 1);
        assert_eq!(
            harness.query_all_by_label("⚠").count(),
            0,
            "a port docker will reassign is not a conflict"
        );
    }

    #[test]
    fn the_ports_tab_names_the_compose_file_and_flags_a_real_collision() {
        let mut lab = container_entry("aaa", "lab", ContainerState::Running);
        lab.ports = ports::parse_ps_ports("0.0.0.0:5656->5656/tcp");
        lab.compose_files = vec!["/srv/lab/docker-compose.yml".to_string()];
        lab.declared_host_ports = BTreeSet::from([(5656, "tcp".to_string())]);
        let mut tasks = container_entry("bbb", "tasks", ContainerState::Running);
        tasks.ports = ports::parse_ps_ports("0.0.0.0:5656->3000/tcp");
        let snapshot = DockerSnapshot {
            containers: vec![lab, tasks],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Ports));
        assert_eq!(harness.query_all_by_label("Ports (2)").count(), 1);
        assert_eq!(
            harness.query_all_by_label("⚠").count(),
            2,
            "both sides of a collision are flagged in the table too"
        );
        assert_eq!(
            harness
                .query_all_by_label("/srv/lab/docker-compose.yml")
                .count(),
            1
        );
        assert_eq!(
            harness.query_all_by_label("—").count(),
            1,
            "the container created outside compose has no file to point at"
        );
    }

    #[test]
    fn the_ports_tab_is_empty_when_nothing_publishes() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("aaa", "lab", ContainerState::Running)],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Ports));
        assert_eq!(harness.query_all_by_label("Ports (0)").count(), 1);
        assert_eq!(harness.query_all_by_label("Aucun port publié.").count(), 1);
    }

    // --- render(): the Ports tab's host scan ----------------------------------

    fn host(port: u16, protocol: &str, process: &str) -> ListeningPort {
        ListeningPort {
            port,
            protocol: protocol.to_string(),
            pid: Some(4242),
            process: Some(process.to_string()),
        }
    }

    fn publishing_snapshot(name: &str, mapping: &str, state: ContainerState) -> DockerSnapshot {
        let mut container = container_entry("aaa", name, state);
        container.ports = ports::parse_ps_ports(mapping);
        DockerSnapshot {
            containers: vec![container],
            images: vec![],
            volumes: vec![],
        }
    }

    /// The « Hôte » column only exists once a scan has produced something to
    /// put in it — same rule as the volume list's size column, and for the
    /// same reason: an empty column reads as a failed measurement.
    #[test]
    fn the_host_column_is_absent_until_a_scan_has_run() {
        let snapshot = publishing_snapshot("lab", "0.0.0.0:8080->80/tcp", ContainerState::Running);
        let harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Ports));
        assert_eq!(harness.query_all_by_label("Hôte").count(), 0);
        assert_eq!(
            harness
                .query_all_by_label("Scanner les ports de l'hôte")
                .count(),
            1,
            "the button is what makes it appear"
        );
    }

    #[test]
    fn clicking_the_scan_button_emits_scan_host_ports() {
        let snapshot = publishing_snapshot("lab", "0.0.0.0:8080->80/tcp", ContainerState::Running);
        let mut harness = build_harness(State::with_snapshot(snapshot).on_list(DockerList::Ports));
        harness.get_by_label("Scanner les ports de l'hôte").click();
        harness.run();
        assert!(harness
            .state()
            .actions
            .contains(&DockerAction::ScanHostPorts));
    }

    /// The point of the whole scan: a *stopped* container declares 5432, a
    /// host Postgres already holds it, and no Docker command would ever say
    /// so — `find_conflicts` sees a single owner and reports nothing.
    #[test]
    fn a_stopped_owner_whose_port_the_host_holds_is_flagged_occupied() {
        let snapshot =
            publishing_snapshot("app-db", "0.0.0.0:5432->5432/tcp", ContainerState::Exited);
        let harness = build_harness(
            State::with_snapshot(snapshot)
                .on_list(DockerList::Ports)
                .with_host_ports(vec![host(5432, "tcp", "postgres.exe")]),
        );
        assert_eq!(harness.query_all_by_label("Hôte").count(), 1);
        assert_eq!(harness.query_all_by_label("occupé").count(), 1);
        assert_eq!(
            harness.query_all_by_label("libre").count(),
            0,
            "the only published port is the taken one"
        );
    }

    #[test]
    fn a_published_port_no_one_holds_reads_as_free() {
        let snapshot = publishing_snapshot("lab", "0.0.0.0:8080->80/tcp", ContainerState::Exited);
        let harness = build_harness(
            State::with_snapshot(snapshot)
                .on_list(DockerList::Ports)
                .with_host_ports(vec![host(80, "tcp", "httpd.exe")]),
        );
        assert_eq!(
            harness.query_all_by_label("libre").count(),
            1,
            "8080 is not 80, whatever the container port says"
        );
    }

    /// Everything the allocation table does not explain goes in its own
    /// block — that is what turns "which container wants 80" into "is 80
    /// free at all".
    #[test]
    fn host_listeners_no_container_explains_get_their_own_block() {
        let snapshot = publishing_snapshot("lab", "0.0.0.0:8080->80/tcp", ContainerState::Running);
        let harness = build_harness(
            State::with_snapshot(snapshot)
                .on_list(DockerList::Ports)
                .with_host_ports(vec![
                    host(80, "tcp", "httpd.exe"),
                    host(8080, "tcp", "com.docker.backend.exe"),
                ]),
        );
        assert_eq!(
            harness.query_all_by_label("httpd.exe (PID 4242)").count(),
            1
        );
        assert_eq!(
            harness
                .query_all_by_label("com.docker.backend.exe (PID 4242)")
                .count(),
            0,
            "8080 is published by the container, so its proxy is not \"outside Docker\""
        );
    }

    /// A machine with no published port is precisely when the host block is
    /// most useful, so the empty table must not swallow it.
    #[test]
    fn the_host_block_still_shows_when_nothing_is_published() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("aaa", "lab", ContainerState::Running)],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(
            State::with_snapshot(snapshot)
                .on_list(DockerList::Ports)
                .with_host_ports(vec![host(80, "tcp", "httpd.exe")]),
        );
        assert_eq!(harness.query_all_by_label("Aucun port publié.").count(), 1);
        assert_eq!(
            harness.query_all_by_label("httpd.exe (PID 4242)").count(),
            1
        );
    }

    #[test]
    fn a_scan_that_found_nothing_says_so_rather_than_showing_nothing() {
        let snapshot = publishing_snapshot("lab", "0.0.0.0:8080->80/tcp", ContainerState::Running);
        let harness = build_harness(
            State::with_snapshot(snapshot)
                .on_list(DockerList::Ports)
                .with_host_ports(Vec::new()),
        );
        assert_eq!(
            harness
                .query_all_by_label("Aucun port de l'hôte en dehors de Docker.")
                .count(),
            1
        );
        assert_eq!(
            harness.query_all_by_label("Rescanner l'hôte").count(),
            1,
            "a scan that ran turns the button into a rescan"
        );
    }

    #[test]
    fn a_dormant_container_image_and_volume_each_show_a_dated_badge() {
        let mut container = container_entry("aaa", "lab", ContainerState::Exited);
        container.last_activity = Some("2026-06-21T00:00:00Z".to_string());
        let mut image = image_entry("bbb", "old:tag", false, "old:tag");
        image.created_iso = Some("2026-06-21T00:00:00Z".to_string());
        let mut volume = volume_entry("orphan-data", true);
        volume.created_iso = Some("2026-06-21T00:00:00Z".to_string());
        let snapshot = DockerSnapshot {
            containers: vec![container],
            images: vec![image],
            volumes: vec![volume],
        };
        // One family per tab now, so the badge is checked once per tab
        // rather than three times on one screen.
        // 2026-06-21 -> 2026-08-21 is 61 days, past the default 60-day threshold.
        for list in [
            DockerList::Containers,
            DockerList::Images,
            DockerList::Volumes,
        ] {
            let harness = build_harness(State::with_snapshot(snapshot.clone()).on_list(list));
            assert_eq!(
                harness.query_all_by_label("dormant · 61 j").count(),
                1,
                "one badge per family, here {list:?}"
            );
        }
    }

    #[test]
    fn raising_the_threshold_removes_the_dormant_badges() {
        let mut container = container_entry("aaa", "lab", ContainerState::Exited);
        container.last_activity = Some("2026-06-21T00:00:00Z".to_string());
        let snapshot = DockerSnapshot {
            containers: vec![container],
            images: vec![],
            volumes: vec![],
        };
        let state = State {
            dormant_after_days: 90,
            ..State::with_snapshot(snapshot)
        };
        let harness = build_harness(state);
        assert_eq!(harness.query_all_by_label("dormant · 61 j").count(), 0);
    }

    #[test]
    fn an_undated_dormant_candidate_is_not_badged_at_all() {
        // `last_activity: None` — a container docker could not date must stay
        // unbadged rather than be badged with an unknown age.
        let container = container_entry("aaa", "lab", ContainerState::Exited);
        let snapshot = DockerSnapshot {
            containers: vec![container],
            images: vec![],
            volumes: vec![],
        };
        let harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("dormant").count(), 0);
    }

    // --- parse_human_size / format_human_size --------------------------------

    #[test]
    fn parse_human_size_reads_the_si_units_docker_prints() {
        assert_eq!(parse_human_size("767kB"), Some(767_000));
        assert_eq!(parse_human_size("192MB"), Some(192_000_000));
        assert_eq!(parse_human_size("1.5GB"), Some(1_500_000_000));
        assert_eq!(parse_human_size("0B"), Some(0));
        // Older CLIs spell it `KB`; still decimal, still 1000.
        assert_eq!(parse_human_size("2KB"), Some(2_000));
        assert_eq!(parse_human_size(" 12 MB "), Some(12_000_000));
    }

    #[test]
    fn parse_human_size_drops_the_virtual_part_of_a_container_size() {
        // Only the writable layer is freed by `docker rm`; the virtual size
        // belongs to the image row.
        assert_eq!(parse_human_size("767kB (virtual 148MB)"), Some(767_000));
    }

    #[test]
    fn parse_human_size_is_none_when_there_is_no_number() {
        for text in ["N/A", "", "-", "?", "abcMB"] {
            assert_eq!(parse_human_size(text), None, "text {text:?}");
        }
    }

    #[test]
    fn format_human_size_round_trips_through_parse() {
        for text in ["1.5GB", "192.0MB", "767.0kB"] {
            let bytes = parse_human_size(text).expect("fixture parses");
            assert_eq!(format_human_size(bytes), text);
        }
        assert_eq!(format_human_size(0), "0B");
        assert_eq!(format_human_size(999), "999B");
        assert_eq!(format_human_size(1_000), "1.0kB");
        // Rounds to one decimal, never promotes to the next unit early.
        assert_eq!(format_human_size(1_949_000_000), "1.9GB");
    }

    #[test]
    fn a_batch_total_with_an_unreadable_entry_is_prefixed_with_at_least() {
        let snapshot = DockerSnapshot {
            containers: Vec::new(),
            images: Vec::new(),
            volumes: vec![
                volume_entry_with_size("mesure", true, "5MB"),
                // No `docker system df -v` run yet: size unknown.
                volume_entry("inconnu", true),
            ],
        };
        let selection = HashSet::from([
            SelectionKey::volume("mesure"),
            SelectionKey::volume("inconnu"),
        ]);
        let (bytes, partial) = selection_size(&selection, &snapshot);
        assert_eq!(bytes, 5_000_000);
        assert!(partial, "an unreadable size must mark the total as partial");
        assert_eq!(format_selection_size(bytes, partial), "≥ 5.0MB");
        assert_eq!(format_selection_size(bytes, false), "5.0MB");
    }

    // --- order_targets / remove_batch_with -----------------------------------

    fn target(kind: ResourceKind, id: &str) -> BatchTarget {
        BatchTarget {
            key: SelectionKey {
                kind,
                id: id.to_string(),
            },
            label: id.to_string(),
        }
    }

    #[test]
    fn order_targets_puts_containers_first_and_volumes_last() {
        let shuffled = vec![
            target(ResourceKind::Volume, "v1"),
            target(ResourceKind::Image, "i1"),
            target(ResourceKind::Container, "c1"),
            target(ResourceKind::Image, "i2"),
            target(ResourceKind::Container, "c2"),
        ];
        let labels: Vec<String> = order_targets(&shuffled)
            .into_iter()
            .map(|target| target.label)
            .collect();
        // Families reordered, and the order *inside* each family preserved:
        // the sort is stable, so `i1` still precedes `i2`.
        assert_eq!(labels, ["c1", "c2", "i1", "i2", "v1"]);
    }

    #[test]
    fn remove_batch_with_continues_past_a_failure_and_reports_every_target() {
        let targets = vec![
            target(ResourceKind::Volume, "v1"),
            target(ResourceKind::Container, "c1"),
            target(ResourceKind::Image, "i1"),
        ];
        let mut calls = Vec::new();
        let outcomes = remove_batch_with(&targets, |target| {
            calls.push(target.key.id.clone());
            if target.key.id == "i1" {
                Err("conflict: image is being used".to_string())
            } else {
                Ok(())
            }
        });
        assert_eq!(calls, ["c1", "i1", "v1"], "execution follows order_targets");
        assert_eq!(
            outcomes,
            vec![
                BatchOutcome {
                    label: "c1".to_string(),
                    result: Ok(()),
                },
                BatchOutcome {
                    label: "i1".to_string(),
                    result: Err("conflict: image is being used".to_string()),
                },
                // Reached despite the failure just above — the whole point.
                BatchOutcome {
                    label: "v1".to_string(),
                    result: Ok(()),
                },
            ]
        );
    }

    // --- selection rules -----------------------------------------------------

    fn image_used_by(identity: &str, used_by: &[&str]) -> ImageEntry {
        ImageEntry {
            used: true,
            used_by: used_by.iter().map(|id| id.to_string()).collect(),
            ..image_entry("abc123", identity, true, identity)
        }
    }

    #[test]
    fn an_image_becomes_selectable_once_all_its_containers_are() {
        let image = image_used_by("nginx:alpine", &["c1", "c2"]);
        assert!(
            !image_is_selectable(&image, &HashSet::new()),
            "a used image is not selectable on its own"
        );
        let partial = HashSet::from([SelectionKey::container("c1")]);
        assert!(
            !image_is_selectable(&image, &partial),
            "one container short is still not enough"
        );
        let full = HashSet::from([SelectionKey::container("c1"), SelectionKey::container("c2")]);
        assert!(image_is_selectable(&image, &full));
    }

    #[test]
    fn a_used_image_with_no_known_container_is_never_selectable() {
        // `compute_used` marks every image used when a reference could not be
        // resolved: no selection can free this one.
        let image = image_used_by("mystere:latest", &[]);
        let everything = HashSet::from([SelectionKey::container("c1")]);
        assert!(!image_is_selectable(&image, &everything));
    }

    #[test]
    fn dropping_a_container_unselects_the_image_it_was_holding() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "web", ContainerState::Exited)],
            images: vec![image_used_by("nginx:alpine", &["c1"])],
            volumes: Vec::new(),
        };
        let both = HashSet::from([
            SelectionKey::container("c1"),
            SelectionKey::image("nginx:alpine"),
        ]);
        assert_eq!(sanitize_selection(&both, &snapshot), both);

        // The user unticks the container; the image must follow on the same
        // pass, or the batch would try to remove an image still in use.
        let image_only = HashSet::from([SelectionKey::image("nginx:alpine")]);
        assert!(sanitize_selection(&image_only, &snapshot).is_empty());
    }

    #[test]
    fn sanitize_selection_drops_keys_whose_resource_vanished() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "web", ContainerState::Exited)],
            images: Vec::new(),
            volumes: vec![volume_entry("data", true)],
        };
        let stale = HashSet::from([
            SelectionKey::container("c1"),
            // Deleted from another terminal since the last fetch.
            SelectionKey::container("disparu"),
            SelectionKey::volume("data"),
            SelectionKey::volume("disparu"),
        ]);
        assert_eq!(
            sanitize_selection(&stale, &snapshot),
            HashSet::from([SelectionKey::container("c1"), SelectionKey::volume("data")])
        );
    }

    #[test]
    fn a_selection_can_never_hold_a_row_a_single_delete_would_refuse() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "web", ContainerState::Running)],
            images: vec![image_used_by("nginx:alpine", &[])],
            volumes: vec![volume_entry("attache", false)],
        };
        // Exactly the three rows whose « Supprimer » button is disabled.
        let forbidden = HashSet::from([
            SelectionKey::container("c1"),
            SelectionKey::image("nginx:alpine"),
            SelectionKey::volume("attache"),
        ]);
        assert!(sanitize_selection(&forbidden, &snapshot).is_empty());
    }

    #[test]
    fn dormant_selection_ticks_exactly_the_badged_rows() {
        let old = "2020-01-01T00:00:00Z";
        let snapshot = DockerSnapshot {
            containers: vec![ContainerEntry {
                last_activity: Some(old.to_string()),
                ..container_entry("c1", "web", ContainerState::Exited)
            }],
            images: vec![ImageEntry {
                created_iso: Some(old.to_string()),
                ..image_used_by("nginx:alpine", &["c1"])
            }],
            volumes: vec![VolumeEntry {
                created_iso: Some(old.to_string()),
                ..volume_entry("data", true)
            }],
        };
        let cutoff = cutoff_epoch(TEST_NOW, 60);
        assert_eq!(
            dormant_selection(&snapshot, cutoff),
            HashSet::from([SelectionKey::container("c1"), SelectionKey::volume("data"),]),
            "the image is held by c1, so it carries no dormant badge and the \
             shortcut leaves it out — its checkbox is enabled all the same"
        );
        // ... and ticking it by hand on top of the shortcut is legal.
        let image = &snapshot.images[0];
        assert!(image_is_selectable(
            image,
            &dormant_selection(&snapshot, cutoff)
        ));
    }

    #[test]
    fn dormant_selection_takes_an_unused_image_before_it_is_needed() {
        // The same pass, with the image free of any container: badged, and
        // therefore ticked.
        let old = "2020-01-01T00:00:00Z";
        let snapshot = DockerSnapshot {
            containers: Vec::new(),
            images: vec![ImageEntry {
                created_iso: Some(old.to_string()),
                ..image_entry("abc123", "vieille:1.0", false, "vieille:1.0")
            }],
            volumes: Vec::new(),
        };
        assert_eq!(
            dormant_selection(&snapshot, cutoff_epoch(TEST_NOW, 60)),
            HashSet::from([SelectionKey::image("vieille:1.0")])
        );
    }

    // --- render(): the selection bar -----------------------------------------

    #[test]
    fn deleting_the_selection_emits_ordered_targets() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "web", ContainerState::Exited)],
            images: vec![image_entry("abc123", "nginx:alpine", false, "nginx:alpine")],
            volumes: vec![volume_entry("data", true)],
        };
        let mut state = State::with_snapshot(snapshot);
        state.selection = HashSet::from([
            SelectionKey::volume("data"),
            SelectionKey::image("nginx:alpine"),
            SelectionKey::container("c1"),
        ]);
        let mut harness = build_harness(state);
        harness.get_by_label("Supprimer la sélection").click();
        harness.run();
        let targets = harness
            .state()
            .actions
            .iter()
            .find_map(|action| match action {
                DockerAction::DeleteSelection(targets) => Some(targets.clone()),
                _ => None,
            })
            .expect("clicking must emit DeleteSelection");
        assert_eq!(
            targets
                .iter()
                .map(|target| target.key.kind)
                .collect::<Vec<_>>(),
            [
                ResourceKind::Container,
                ResourceKind::Image,
                ResourceKind::Volume
            ]
        );
        // Labels are the displayed names, not the façade identifiers.
        assert_eq!(targets[0].label, "web");
    }

    #[test]
    fn the_bar_counts_the_selection_and_what_it_frees() {
        let snapshot = DockerSnapshot {
            containers: Vec::new(),
            images: Vec::new(),
            volumes: vec![volume_entry_with_size("data", true, "5MB")],
        };
        let mut state = State::with_snapshot(snapshot);
        state.selection = HashSet::from([SelectionKey::volume("data")]);
        let harness = build_harness(state);
        harness.get_by_label("1 sélectionné(s) · ≈ 5.0MB récupérables");
    }

    #[test]
    fn select_dormant_and_clear_are_emitted_not_confirmed() {
        let snapshot = DockerSnapshot {
            containers: Vec::new(),
            images: Vec::new(),
            volumes: vec![VolumeEntry {
                created_iso: Some("2020-01-01T00:00:00Z".to_string()),
                ..volume_entry("data", true)
            }],
        };
        let mut state = State::with_snapshot(snapshot);
        state.selection = HashSet::from([SelectionKey::volume("data")]);
        let mut harness = build_harness(state);
        harness.get_by_label("Tout sélectionner (dormants)").click();
        harness.run();
        harness.get_by_label("Effacer la sélection").click();
        harness.run();
        assert!(harness
            .state()
            .actions
            .contains(&DockerAction::SelectDormant));
        assert!(harness
            .state()
            .actions
            .contains(&DockerAction::ClearSelection));
    }

    #[test]
    fn the_report_shows_one_line_per_target_with_the_docker_error() {
        let snapshot = DockerSnapshot {
            containers: Vec::new(),
            images: Vec::new(),
            volumes: vec![volume_entry("data", true)],
        };
        let mut state = State::with_snapshot(snapshot);
        state.batch_report = vec![
            BatchOutcome {
                label: "web".to_string(),
                result: Ok(()),
            },
            BatchOutcome {
                label: "nginx:alpine".to_string(),
                result: Err("conflict: image is being used".to_string()),
            },
        ];
        let harness = build_harness(state);
        harness.get_by_label("Dernier lot : 1 réussite(s), 1 échec(s)");
        harness.get_by_label("✓ web");
        harness.get_by_label("✗ nginx:alpine — conflict: image is being used");
    }
}
