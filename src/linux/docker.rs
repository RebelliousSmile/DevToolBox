//! Docker CLI data source — Phase 1 of the Docker tab plan
//! (`aidd_docs/tasks/2026_08/2026_08_19-docker-tab.md`). Pure CLI layer, no
//! UI knowledge: everything here talks to the `docker` binary via
//! `std::process::Command` and returns the OS-neutral row types declared in
//! `crate::ui::docker_view` (the `AutomationRow` precedent — see that
//! module's doc comment).
//!
//! # Listings
//!
//! `docker ps -a` / `images` / `volume ls` are all invoked with
//! `--format '{{json .}}'` (NDJSON: one JSON object per line), not the
//! newer `--format json` shorthand — the plan's Stack section requires
//! compatibility with docker CLI ≥ 20.x, which only understands the
//! Go-template form. Parsing tolerance mirrors `TimerEntry`
//! (`src/linux/automations.rs:53-63`) and
//! `scripts/system_inventory/docker_native.py:110-143`: every wire field is
//! `#[serde(default)]`, and a line that fails to parse at all is skipped
//! rather than failing the whole listing.
//!
//! # Timeouts and error classification
//!
//! [`run_docker`] spawns the child and polls `try_wait()` (the
//! `run_command`/`run_report` precedent — `src/cleanup/spawn.rs:92-144`,
//! `src/applications/mod.rs:140-179`) rather than blocking on
//! `Command::output()`, so a hung daemon can be killed after a bounded
//! wait instead of freezing the caller indefinitely. The wait is capped
//! per *operation class* ([`OperationClass`]) rather than one constant for
//! everything: listings get ~5s (a healthy daemon answers instantly),
//! actions get ~30s (`docker stop` alone waits up to 10s of SIGTERM grace
//! before SIGKILL). A listing timing out is classified
//! [`DockerError::DaemonUnreachable`] (the daemon likely isn't answering at
//! all); an action timing out is classified [`DockerError::CommandFailed`]
//! — it must never present as daemon-unreachable, since the daemon
//! answering slowly to a stop/remove is a different situation from it not
//! answering to a listing at all (Risk register).
//!
//! # Used/orphan computation
//!
//! [`fetch`] cross-references `docker ps -a` image references against the
//! image list by ID prefix and normalized `repo:tag` ([`resolve_image_index`]).
//! Per the Risk register's conservative default, a container image
//! reference that cannot be resolved to any listed image does not just
//! leave that one relationship undetermined — it marks *every* image
//! used-on-doubt, since an unresolvable reference means the matching logic
//! itself can't be trusted for this snapshot, and deletion is never offered
//! on doubt. Volume `orphan` is simpler: exact membership in `docker volume
//! ls -f dangling=true`'s result set — Docker itself is the authority on
//! "dangling", not a re-derivation of it here.
//!
//! # Actions
//!
//! [`stop_container`], [`remove_container`], [`remove_image`] and
//! [`remove_volume`] build plain `docker stop/rm/rmi/volume rm` commands —
//! never `--force`, never `prune` (`command_builders_never_produce_force_or_prune`
//! test below). `remove_image` takes whatever reference the caller already
//! chose (repo:tag for a tagged row, ID for an untagged `<none>:<none>`
//! row) — [`build_image_entry`] is what actually picks that reference when
//! assembling a snapshot's [`crate::ui::docker_view::ImageEntry`] rows,
//! since `docker rmi <id>` refuses a multi-tagged image without `--force`.
//!
//! Phase 1 delivered this module standalone, with no caller yet. Phase 2
//! added the OS-neutral façade in `crate::ui::docker_view` that wraps
//! these functions, and Phase 3 wired that façade into `EguiApp`
//! (`src/ui/egui_app.rs`), so every public item here now has a production
//! caller and the module-level `#![allow(dead_code)]` that covered the
//! pre-Phase-3 gap has been removed.
//!
//! # Reclaimable space
//!
//! `docker ps -a --size`'s `Size` field is free (~40ms measured on this
//! machine — no extra round trip), so [`list_containers`] always passes
//! `--size`; [`extract_rw_size`] keeps only the writable-layer part (before
//! ` (virtual …)`), the part `docker rm` actually frees. `docker images`
//! already reports per-image size (unchanged, still on [`ImageWire::size`]).
//! `docker volume ls` has no size column at all — Docker only reports it via
//! `docker system df -v`, a disk scan (~4.6s measured on this machine, worse
//! elsewhere), so [`volume_sizes`] is a separate, explicitly-triggered
//! **Action**-class call (never bundled into [`fetch`]) rather than a fifth
//! listing every snapshot pays for.

use std::collections::HashSet;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::ui::docker_view::{
    ContainerEntry, ContainerState, DockerSnapshot, ImageEntry, VolumeEntry,
};

/// Failure modes a `docker` invocation can produce, deliberately coarser
/// than raw exit codes/stderr text so callers (the Phase 2 façade, later
/// the UI) can react without re-parsing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerError {
    /// The `docker` binary itself could not be found/spawned.
    BinaryMissing,
    /// The daemon is not answering: a non-zero exit whose stderr matches a
    /// known "can't reach the daemon" shape, or a *listing* call timing
    /// out.
    DaemonUnreachable(String),
    /// The daemon answered but refused the command (e.g. "image is in
    /// use"), or an *action* call timed out waiting for a reply.
    CommandFailed(String),
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerError::BinaryMissing => write!(f, "commande docker introuvable dans le PATH"),
            DockerError::DaemonUnreachable(detail) => {
                write!(f, "daemon Docker inaccessible: {detail}")
            }
            DockerError::CommandFailed(detail) => write!(f, "commande docker en échec: {detail}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Binary detection
// ---------------------------------------------------------------------------

/// `name -> Option<value>` environment lookup, injectable for testing —
/// same shape as `crate::platform::linux`'s `EnvLookup`.
type EnvLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

fn std_env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// `true` when a `docker` executable is resolvable somewhere on `PATH`.
/// Drives the Docker tab's visibility (Phase 3): the tab is hidden, not
/// shown-with-an-error, when this is `false`.
pub fn binary_available() -> bool {
    binary_available_with_env(&std_env_lookup)
}

fn binary_available_with_env(env: &EnvLookup) -> bool {
    resolve_docker_binary(env).is_some()
}

fn resolve_docker_binary(env: &EnvLookup) -> Option<PathBuf> {
    let path = env("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("docker"))
        .find(|candidate| candidate.is_file())
}

// ---------------------------------------------------------------------------
// Process spawning with a class-scoped timeout
// ---------------------------------------------------------------------------

/// Which timeout budget (and, on timeout, which [`DockerError`] variant)
/// applies to a given `docker` invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationClass {
    /// `ps -a` / `images` / `volume ls`: a healthy daemon answers near
    /// instantly, so a long wait here is itself evidence of an
    /// unreachable daemon.
    Listing,
    /// `stop` / `rm` / `rmi` / `volume rm`: `docker stop` alone can hold
    /// for up to 10s of SIGTERM grace before SIGKILL, so a slow reply here
    /// is expected behavior, not evidence of an unreachable daemon.
    Action,
}

const LISTING_TIMEOUT: Duration = Duration::from_secs(5);
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

fn timeout_for(class: OperationClass) -> Duration {
    match class {
        OperationClass::Listing => LISTING_TIMEOUT,
        OperationClass::Action => ACTION_TIMEOUT,
    }
}

/// Run `docker <args>` with the timeout budget for `class`, returning
/// stdout on success. Fails fast with [`DockerError::BinaryMissing`] if
/// `docker` isn't resolvable on `PATH` at all — [`run_command_with_timeout`]
/// does the actual spawn/poll/classify work and is generic over the
/// program name specifically so tests can exercise timeout behavior
/// against a trivial `sleep` child instead of a real docker daemon.
fn run_docker(args: &[&str], class: OperationClass) -> Result<String, DockerError> {
    if !binary_available() {
        return Err(DockerError::BinaryMissing);
    }
    run_command_with_timeout("docker", args, timeout_for(class), class)
}

/// Spawn `program args...`, polling `try_wait()` (the `run_command`
/// precedent in `src/cleanup/spawn.rs:92-144`) until it exits or `timeout`
/// elapses, in which case the child is killed. stdout/stderr are drained
/// concurrently on background threads so a child that fills its pipe
/// buffer can't deadlock the wait loop.
fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
    class: OperationClass,
) -> Result<String, DockerError> {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Err(DockerError::BinaryMissing),
    };

    let stdout_handle = child.stdout.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(pipe).read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr_handle = child.stderr.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(pipe).read_to_end(&mut bytes);
            bytes
        })
    });

    let expiry = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < expiry => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(_) => break None,
        }
    };

    let stdout_bytes = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr_bytes = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();

    let Some(status) = status else {
        let message = format!("délai d'attente dépassé ({timeout:?})");
        // An action timing out must never present as daemon-unreachable —
        // see the module doc's "Timeouts and error classification"
        // section and the Risk register.
        return Err(match class {
            OperationClass::Listing => DockerError::DaemonUnreachable(message),
            OperationClass::Action => DockerError::CommandFailed(message),
        });
    };

    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

    if status.success() {
        Ok(stdout)
    } else {
        Err(classify_stderr(&stderr))
    }
}

/// Classify a non-zero-exit `docker` invocation's stderr: the well-known
/// "can't reach the daemon" shapes (a downed daemon, or a permissions
/// problem reaching its socket) become [`DockerError::DaemonUnreachable`];
/// everything else (a daemon refusal, e.g. "image is in use") becomes
/// [`DockerError::CommandFailed`].
fn classify_stderr(stderr: &str) -> DockerError {
    let lowered = stderr.to_ascii_lowercase();
    if lowered.contains("cannot connect") || lowered.contains("permission denied") {
        DockerError::DaemonUnreachable(stderr.trim().to_string())
    } else {
        DockerError::CommandFailed(stderr.trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// Wire types (private — `docker`'s own NDJSON shape, not the UI's)
// ---------------------------------------------------------------------------

/// One row of `docker ps -a --format '{{json .}}'`. Every field is
/// `#[serde(default)]` so a docker CLI version emitting a slightly
/// different shape degrades to blank fields for that row instead of
/// aborting the whole listing (the `TimerEntry` precedent, see the module
/// doc).
#[derive(Debug, Clone, Deserialize, Default)]
struct ContainerWire {
    #[serde(default, rename = "ID")]
    id: String,
    #[serde(default, rename = "Names")]
    names: String,
    #[serde(default, rename = "Image")]
    image: String,
    #[serde(default, rename = "State")]
    state: String,
    #[serde(default, rename = "Status")]
    status: String,
    /// Only present because [`list_containers`] passes `--size`: e.g.
    /// `"767kB (virtual 148MB)"` (a real value on this machine) — the part
    /// before ` (` is the writable layer `docker rm` actually frees, see
    /// [`extract_rw_size`].
    #[serde(default, rename = "Size")]
    size: String,
    /// Captured for wire-fidelity/debug parity with `docker ps -a`'s own
    /// output (`Debug`-derived), but not surfaced on [`ContainerEntry`] —
    /// `status` already carries the free-text detail the view displays, and
    /// nothing else consumes a container's creation timestamp today.
    #[serde(default, rename = "CreatedAt")]
    #[allow(dead_code)]
    created_at: String,
}

/// One row of `docker images --format '{{json .}}'`.
#[derive(Debug, Clone, Deserialize, Default)]
struct ImageWire {
    #[serde(default, rename = "ID")]
    id: String,
    #[serde(default, rename = "Repository")]
    repository: String,
    #[serde(default, rename = "Tag")]
    tag: String,
    #[serde(default, rename = "Size")]
    size: String,
    #[serde(default, rename = "CreatedAt")]
    created_at: String,
    /// `docker images`'s own container count for this image — a hint only
    /// (the Risk register: it reads `"N/A"` on some daemon configurations,
    /// e.g. this machine's non-containerd image store, confirmed against
    /// this machine's real captured fixture below). The authoritative
    /// signal is the `ps -a` cross-reference in [`compute_used`]; this
    /// field isn't consumed by it, kept only for potential future
    /// corroboration/display.
    #[serde(default, rename = "Containers")]
    #[allow(dead_code)]
    containers_hint: String,
}

/// One row of `docker volume ls --format '{{json .}}'`.
#[derive(Debug, Clone, Deserialize, Default)]
struct VolumeWire {
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Driver")]
    driver: String,
    /// Captured for wire-fidelity/debug parity with `docker volume ls`'s own
    /// output, but not surfaced on [`VolumeEntry`] — `orphan` (derived from
    /// `dangling=true` membership) is the only signal the view needs.
    #[serde(default, rename = "Mountpoint")]
    #[allow(dead_code)]
    mountpoint: String,
}

/// One volume row of `docker system df -v --format '{{json .}}'`'s
/// `"Volumes"` array — a much smaller slice of that command's own wire shape
/// than [`DfWire`] (which also carries `"Images"`/`"Containers"`/
/// `"BuildCache"`, none of which [`volume_sizes`] needs).
#[derive(Debug, Clone, Deserialize, Default)]
struct DfVolumeWire {
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Size")]
    size: String,
}

/// `docker system df -v --format '{{json .}}'`'s top-level shape: **one**
/// JSON object (not NDJSON — unlike every listing above), of which only the
/// `"Volumes"` array is consumed. `#[serde(default)]` so a docker CLI
/// version that omits/renames the other top-level keys, or ships an empty
/// `"Volumes"` array, still parses.
#[derive(Debug, Clone, Deserialize, Default)]
struct DfWire {
    #[serde(default, rename = "Volumes")]
    volumes: Vec<DfVolumeWire>,
}

/// Parse NDJSON (one JSON object per line): blank lines are skipped, and a
/// line that fails to deserialize is skipped rather than failing the whole
/// batch (mirrors `scripts/system_inventory/docker_native.py:110-143`'s
/// per-line tolerance).
fn parse_ndjson<T: DeserializeOwned>(raw: &str) -> Vec<T> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<T>(line).ok())
        .collect()
}

/// Parse `docker system df -v --format '{{json .}}'`'s single JSON object
/// into `(volume name, size)` pairs — deliberately **not** [`parse_ndjson`]:
/// this command emits one object, not one-per-line. Tolerant the same way
/// the NDJSON path is: a body that fails to parse at all (unexpected shape)
/// degrades to an empty result via [`DfWire`]'s `#[serde(default)]` rather
/// than failing the caller, and a blank/whitespace-only `name` field is
/// dropped since it can't be matched back to a [`VolumeEntry`] by name.
fn parse_volume_sizes(raw: &str) -> Vec<(String, String)> {
    let wire: DfWire = serde_json::from_str(raw.trim()).unwrap_or_default();
    wire.volumes
        .into_iter()
        .filter(|volume| !volume.name.trim().is_empty())
        .map(|volume| (volume.name, volume.size))
        .collect()
}

// ---------------------------------------------------------------------------
// Listings
// ---------------------------------------------------------------------------

fn list_containers() -> Result<Vec<ContainerWire>, DockerError> {
    let raw = run_docker(
        &["ps", "-a", "--size", "--format", "{{json .}}"],
        OperationClass::Listing,
    )?;
    Ok(parse_ndjson(&raw))
}

fn list_images() -> Result<Vec<ImageWire>, DockerError> {
    let raw = run_docker(
        &["images", "--format", "{{json .}}"],
        OperationClass::Listing,
    )?;
    Ok(parse_ndjson(&raw))
}

fn list_volumes() -> Result<Vec<VolumeWire>, DockerError> {
    let raw = run_docker(
        &["volume", "ls", "--format", "{{json .}}"],
        OperationClass::Listing,
    )?;
    Ok(parse_ndjson(&raw))
}

/// Names of volumes Docker itself considers dangling
/// (`volume ls -f dangling=true`) — the authoritative orphan set consumed
/// by [`build_volume_entry`].
fn list_dangling_volume_names() -> Result<HashSet<String>, DockerError> {
    let raw = run_docker(
        &[
            "volume",
            "ls",
            "-f",
            "dangling=true",
            "--format",
            "{{json .}}",
        ],
        OperationClass::Listing,
    )?;
    let wires: Vec<VolumeWire> = parse_ndjson(&raw);
    Ok(wires.into_iter().map(|wire| wire.name).collect())
}

// ---------------------------------------------------------------------------
// Container -> image matching (used/orphan computation)
// ---------------------------------------------------------------------------

/// Split a Docker image reference into `(repository, tag)`. The tricky
/// part is a registry host with an explicit port
/// (`gitlab.smartlockers.io:5050/smartlockers/qt.proxy:2.6.1`, a real
/// reference on this machine): the `:5050` after the host must not be
/// mistaken for a tag separator, so only a `:` found *after* the last `/`
/// counts as the repo/tag boundary. No `/` at all still allows a bare
/// `repo:tag` (or tagless `repo`) split on the first `:`.
fn split_repo_tag(reference: &str) -> (String, Option<String>) {
    if let Some(slash_idx) = reference.rfind('/') {
        if let Some(colon_offset) = reference[slash_idx..].find(':') {
            let colon_idx = slash_idx + colon_offset;
            return (
                reference[..colon_idx].to_string(),
                Some(reference[colon_idx + 1..].to_string()),
            );
        }
        (reference.to_string(), None)
    } else if let Some(colon_idx) = reference.find(':') {
        (
            reference[..colon_idx].to_string(),
            Some(reference[colon_idx + 1..].to_string()),
        )
    } else {
        (reference.to_string(), None)
    }
}

/// `true` for a short/full hex image ID (`docker ps`'s `Image` field is one
/// when a container's image has no tag Docker can show, e.g. an ID or
/// `sha256:`-digest reference) — used to decide whether to try an ID-prefix
/// match before falling back to a repo:tag match.
fn is_hex_id_like(text: &str) -> bool {
    text.len() >= 6 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolve one container's `Image` reference (`docker ps -a`'s `Image`
/// field — a repo:tag, a short/full ID, or a `sha256:`-prefixed digest) to
/// an index into `images`, trying an ID-prefix match first, then a
/// normalized repo:tag match (tagless references default to `latest`, same
/// as the Docker daemon itself). `None` means the reference is
/// unresolvable — see [`compute_used`] for what that triggers.
fn resolve_image_index(images: &[ImageWire], reference: &str) -> Option<usize> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return None;
    }

    let id_candidate = trimmed.strip_prefix("sha256:").unwrap_or(trimmed);
    if is_hex_id_like(id_candidate) {
        if let Some(index) = images.iter().position(|image| {
            !image.id.is_empty()
                && (image.id.starts_with(id_candidate) || id_candidate.starts_with(&image.id))
        }) {
            return Some(index);
        }
    }

    let (repo, tag) = split_repo_tag(trimmed);
    let tag = tag.unwrap_or_else(|| "latest".to_string());
    images
        .iter()
        .position(|image| image.repository == repo && image.tag == tag)
}

/// Per-image `used` flags, one per entry of `images`, in the same order.
///
/// Every container's image reference is resolved via
/// [`resolve_image_index`]; a resolved reference marks that one image
/// used. Per the Risk register's conservative default, *any* unresolvable
/// reference marks **every** image used-on-doubt — an unresolvable
/// reference means this snapshot's matching can't be trusted, and deletion
/// must never be offered on doubt, so the whole snapshot falls back to the
/// safe answer rather than only the one relationship that couldn't be
/// determined.
fn compute_used(images: &[ImageWire], containers: &[ContainerWire]) -> Vec<bool> {
    let mut used = vec![false; images.len()];
    let mut any_unresolvable = false;

    for container in containers {
        match resolve_image_index(images, &container.image) {
            Some(index) => used[index] = true,
            None => any_unresolvable = true,
        }
    }

    if any_unresolvable {
        used.iter_mut().for_each(|flag| *flag = true);
    }

    used
}

// ---------------------------------------------------------------------------
// Container state mapping
// ---------------------------------------------------------------------------

/// Map `docker ps -a`'s free-text `State` field onto the closed
/// [`ContainerState`] set (case-insensitive — observed values on this
/// machine are lowercase, but nothing guarantees that across CLI
/// versions). Anything unrecognized becomes `Unknown`, the conservative
/// default the plan requires ("anything unknown ⇒ no action offered" — see
/// `ContainerState::is_stoppable`/`is_removable`).
fn container_state_from_raw(raw: &str) -> ContainerState {
    let trimmed = raw.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "running" => ContainerState::Running,
        "paused" => ContainerState::Paused,
        "restarting" => ContainerState::Restarting,
        "exited" => ContainerState::Exited,
        "created" => ContainerState::Created,
        "dead" => ContainerState::Dead,
        _ => ContainerState::Unknown(trimmed.to_string()),
    }
}

/// Extract a container's writable-layer size from `docker ps -a --size`'s
/// `Size` field, e.g. `"767kB (virtual 148MB)"` (a real value on this
/// machine): the part before ` (` is what `docker rm` actually frees — the
/// `(virtual …)` suffix is the image's shared read-only layers, never
/// reclaimed by removing one container. A field with no ` (` (or empty)
/// is returned trimmed as-is, so a docker CLI version that ever drops the
/// `(virtual …)` suffix degrades to the whole string rather than an empty
/// one.
fn extract_rw_size(raw: &str) -> String {
    match raw.find(" (") {
        Some(idx) => raw[..idx].trim().to_string(),
        None => raw.trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Wire -> view type mapping
// ---------------------------------------------------------------------------

fn build_container_entry(wire: ContainerWire) -> ContainerEntry {
    ContainerEntry {
        id: wire.id,
        name: wire.names,
        image: wire.image,
        state: container_state_from_raw(&wire.state),
        status: wire.status,
        rw_size: extract_rw_size(&wire.size),
    }
}

/// Build one [`ImageEntry`], including the reference [`crate::linux::docker::remove_image`]
/// must receive if this row's delete action is confirmed: the tagged
/// identity (`repo:tag`) for a tagged row, since `docker rmi <id>` refuses
/// a multi-tagged image without `--force` (banned) while removing by tag
/// untags cleanly; the short ID for an untagged `<none>:<none>` row, which
/// has no tag to remove by at all.
fn build_image_entry(wire: ImageWire, used: bool) -> ImageEntry {
    let repository = if wire.repository.is_empty() {
        "<none>".to_string()
    } else {
        wire.repository
    };
    let tag = if wire.tag.is_empty() {
        "<none>".to_string()
    } else {
        wire.tag
    };
    let identity = format!("{repository}:{tag}");
    let is_untagged = repository == "<none>" || tag == "<none>";
    let rmi_reference = if is_untagged {
        wire.id.clone()
    } else {
        identity.clone()
    };

    ImageEntry {
        id: wire.id,
        identity,
        size: wire.size,
        created: wire.created_at,
        used,
        rmi_reference,
    }
}

fn build_volume_entry(wire: VolumeWire, dangling: &HashSet<String>) -> VolumeEntry {
    VolumeEntry {
        orphan: dangling.contains(&wire.name),
        name: wire.name,
        driver: wire.driver,
        // `docker volume ls` never reports a real size (always "N/A" on this
        // machine); it's only filled in later by `EguiApp` merging the
        // result of a separate `volume_sizes()` disk scan into the snapshot.
        size: None,
    }
}

// ---------------------------------------------------------------------------
// Snapshot assembly
// ---------------------------------------------------------------------------

/// Fetch containers, images and volumes in one pass and assemble the full
/// [`DockerSnapshot`], computing each image's `used` flag
/// ([`compute_used`]) and each volume's `orphan` flag
/// ([`build_volume_entry`]) along the way. Fails only if one of the four
/// underlying listings fails (binary missing, daemon unreachable, or a
/// listing timeout) — a snapshot is all-or-nothing, since a partial one
/// (e.g. images without knowing which are used) would be actively
/// misleading for a destructive-action UI.
pub fn fetch() -> Result<DockerSnapshot, DockerError> {
    let container_wires = list_containers()?;
    let image_wires = list_images()?;
    let volume_wires = list_volumes()?;
    let dangling_names = list_dangling_volume_names()?;

    let used_flags = compute_used(&image_wires, &container_wires);
    let images = image_wires
        .into_iter()
        .zip(used_flags)
        .map(|(wire, used)| build_image_entry(wire, used))
        .collect();
    let volumes = volume_wires
        .into_iter()
        .map(|wire| build_volume_entry(wire, &dangling_names))
        .collect();
    let containers = container_wires
        .into_iter()
        .map(build_container_entry)
        .collect();

    Ok(DockerSnapshot {
        containers,
        images,
        volumes,
    })
}

// ---------------------------------------------------------------------------
// Actions — plain stop/rm/rmi/volume rm, never --force, never prune
// ---------------------------------------------------------------------------

fn stop_container_args(id: &str) -> Vec<&str> {
    vec!["stop", id]
}

fn remove_container_args(id: &str) -> Vec<&str> {
    vec!["rm", id]
}

fn remove_image_args(reference: &str) -> Vec<&str> {
    vec!["rmi", reference]
}

fn remove_volume_args(name: &str) -> Vec<&str> {
    vec!["volume", "rm", name]
}

/// Stop a running/paused/restarting container. Plain `docker stop`, never
/// `-t 0`/`--force`-equivalent — the daemon's own default grace period
/// applies.
pub fn stop_container(id: &str) -> Result<(), DockerError> {
    run_docker(&stop_container_args(id), OperationClass::Action).map(|_| ())
}

/// Remove a stopped/created/dead container. Plain `docker rm`, never `-f`.
pub fn remove_container(id: &str) -> Result<(), DockerError> {
    run_docker(&remove_container_args(id), OperationClass::Action).map(|_| ())
}

/// Remove an unused image by `reference` (the caller — [`build_image_entry`]'s
/// `rmi_reference`, or Phase 2's equivalent — has already chosen repo:tag
/// vs ID). Plain `docker rmi`, never `-f`/`--force`.
pub fn remove_image(reference: &str) -> Result<(), DockerError> {
    run_docker(&remove_image_args(reference), OperationClass::Action).map(|_| ())
}

/// Remove an orphan (dangling) volume by name. Plain `docker volume rm`,
/// never `-f`/`--force`.
pub fn remove_volume(name: &str) -> Result<(), DockerError> {
    run_docker(&remove_volume_args(name), OperationClass::Action).map(|_| ())
}

// ---------------------------------------------------------------------------
// Volume sizes — `docker system df -v`, computed on demand only
// ---------------------------------------------------------------------------

fn volume_sizes_args() -> Vec<&'static str> {
    vec!["system", "df", "-v", "--format", "{{json .}}"]
}

/// `name -> size` for every volume Docker currently knows about, via
/// `docker system df -v` — the only source of volume sizes at all (`docker
/// volume ls` always reports `"Size":"N/A"`, confirmed on this machine).
/// Deliberately not part of [`fetch`]: this is a disk scan (~4.6s measured
/// on this machine, potentially much longer elsewhere), unlike every other
/// listing here. Still an [`OperationClass::Action`] rather than `Listing`
/// so a slow scan classifies as [`DockerError::CommandFailed`], never
/// [`DockerError::DaemonUnreachable`] — a slow reply here is expected
/// behavior, exactly the rationale [`OperationClass::Action`] documents for
/// `stop`/`rm`.
pub fn volume_sizes() -> Result<Vec<(String, String)>, DockerError> {
    let raw = run_docker(&volume_sizes_args(), OperationClass::Action)?;
    Ok(parse_volume_sizes(&raw))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- binary_available --------------------------------------------------

    #[test]
    fn binary_available_with_env_false_when_path_unset() {
        let env = |_: &str| -> Option<String> { None };
        assert!(!binary_available_with_env(&env));
    }

    #[test]
    fn binary_available_with_env_false_when_no_path_entry_has_docker() {
        let env = |name: &str| -> Option<String> {
            match name {
                "PATH" => Some("/nonexistent/bin:/also/nonexistent".to_string()),
                _ => None,
            }
        };
        assert!(!binary_available_with_env(&env));
    }

    #[test]
    fn binary_available_with_env_true_when_docker_on_path() {
        // `which docker` -> `/usr/bin/docker` on this reference machine
        // (Docker 29.7.2, verified while implementing this module).
        let env = |name: &str| -> Option<String> {
            match name {
                "PATH" => Some("/usr/bin".to_string()),
                _ => None,
            }
        };
        assert!(binary_available_with_env(&env));
    }

    // --- run_command_with_timeout / classification --------------------------

    #[test]
    fn action_timeout_classifies_as_command_failed_not_daemon_unreachable() {
        // `sleep 2` deliberately outlives the 100ms timeout so this stays
        // fast: run_command_with_timeout doesn't care what binary it
        // spawns, only that it doesn't exit in time.
        let result = run_command_with_timeout(
            "sleep",
            &["2"],
            Duration::from_millis(100),
            OperationClass::Action,
        );
        match result {
            Err(DockerError::CommandFailed(_)) => {}
            other => panic!("expected CommandFailed on an action timeout, got {other:?}"),
        }
    }

    #[test]
    fn listing_timeout_classifies_as_daemon_unreachable() {
        let result = run_command_with_timeout(
            "sleep",
            &["2"],
            Duration::from_millis(100),
            OperationClass::Listing,
        );
        match result {
            Err(DockerError::DaemonUnreachable(_)) => {}
            other => panic!("expected DaemonUnreachable on a listing timeout, got {other:?}"),
        }
    }

    #[test]
    fn a_fast_command_succeeds_before_its_timeout() {
        let result =
            run_command_with_timeout("true", &[], Duration::from_secs(5), OperationClass::Action);
        assert_eq!(result, Ok(String::new()));
    }

    #[test]
    fn a_missing_program_yields_binary_missing() {
        let result = run_command_with_timeout(
            "this-program-does-not-exist-devtoolbox",
            &[],
            Duration::from_secs(1),
            OperationClass::Action,
        );
        assert_eq!(result, Err(DockerError::BinaryMissing));
    }

    // --- classify_stderr -----------------------------------------------------

    #[test]
    fn classify_stderr_detects_cannot_connect() {
        let stderr = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. \
                       Is the docker daemon running?";
        assert_eq!(
            classify_stderr(stderr),
            DockerError::DaemonUnreachable(stderr.to_string())
        );
    }

    #[test]
    fn classify_stderr_detects_permission_denied() {
        let stderr = "Got permission denied while trying to connect to the Docker daemon \
                       socket at unix:///var/run/docker.sock";
        assert_eq!(
            classify_stderr(stderr),
            DockerError::DaemonUnreachable(stderr.to_string())
        );
    }

    #[test]
    fn classify_stderr_is_case_insensitive() {
        let stderr = "CANNOT CONNECT to the docker daemon";
        assert!(matches!(
            classify_stderr(stderr),
            DockerError::DaemonUnreachable(_)
        ));
    }

    #[test]
    fn classify_stderr_falls_back_to_command_failed() {
        let stderr = "Error response from daemon: conflict: unable to remove repository \
                       reference \"nginx:alpine\" (must force) - container 69f4a8ba84c3 is \
                       using its referenced image b76de378d572";
        assert_eq!(
            classify_stderr(stderr),
            DockerError::CommandFailed(stderr.to_string())
        );
    }

    // --- split_repo_tag / is_hex_id_like --------------------------------------

    #[test]
    fn split_repo_tag_handles_registry_with_port_and_tag() {
        // A real reference on this machine: the `:5050` after the host
        // must not be mistaken for the tag separator.
        assert_eq!(
            split_repo_tag("gitlab.smartlockers.io:5050/smartlockers/qt.proxy:2.6.1"),
            (
                "gitlab.smartlockers.io:5050/smartlockers/qt.proxy".to_string(),
                Some("2.6.1".to_string())
            )
        );
    }

    #[test]
    fn split_repo_tag_handles_plain_repo_tag() {
        assert_eq!(
            split_repo_tag("nginx:alpine"),
            ("nginx".to_string(), Some("alpine".to_string()))
        );
    }

    #[test]
    fn split_repo_tag_handles_repo_with_no_tag() {
        assert_eq!(
            split_repo_tag("smartlockers-lab-web"),
            ("smartlockers-lab-web".to_string(), None)
        );
    }

    #[test]
    fn split_repo_tag_handles_registry_with_port_and_no_tag() {
        assert_eq!(
            split_repo_tag("gitlab.smartlockers.io:5050/smartlockers/qt.proxy"),
            (
                "gitlab.smartlockers.io:5050/smartlockers/qt.proxy".to_string(),
                None
            )
        );
    }

    #[test]
    fn is_hex_id_like_accepts_short_id() {
        assert!(is_hex_id_like("581c17389e54"));
    }

    #[test]
    fn is_hex_id_like_rejects_repo_name() {
        assert!(!is_hex_id_like("nginx"));
    }

    #[test]
    fn is_hex_id_like_rejects_too_short() {
        assert!(!is_hex_id_like("abc"));
    }

    // --- resolve_image_index / compute_used -----------------------------------

    fn image(id: &str, repository: &str, tag: &str) -> ImageWire {
        ImageWire {
            id: id.to_string(),
            repository: repository.to_string(),
            tag: tag.to_string(),
            size: String::new(),
            created_at: String::new(),
            containers_hint: String::new(),
        }
    }

    fn container(id: &str, image: &str) -> ContainerWire {
        ContainerWire {
            id: id.to_string(),
            names: format!("container-{id}"),
            image: image.to_string(),
            state: "running".to_string(),
            status: String::new(),
            size: String::new(),
            created_at: String::new(),
        }
    }

    #[test]
    fn resolve_image_index_matches_by_repo_tag() {
        let images = vec![image("581c17389e54", "proxy-pilotphone", "latest")];
        assert_eq!(
            resolve_image_index(&images, "proxy-pilotphone:latest"),
            Some(0)
        );
    }

    #[test]
    fn resolve_image_index_matches_tagless_reference_as_latest() {
        let images = vec![image("581c17389e54", "proxy-pilotphone", "latest")];
        assert_eq!(resolve_image_index(&images, "proxy-pilotphone"), Some(0));
    }

    #[test]
    fn resolve_image_index_matches_by_id_prefix() {
        let images = vec![image(
            "0b1ee39de203abcd",
            "moby/buildkit",
            "buildx-stable-1",
        )];
        assert_eq!(resolve_image_index(&images, "0b1ee39de203"), Some(0));
    }

    #[test]
    fn resolve_image_index_matches_sha256_digest_prefix() {
        let images = vec![image("5b1bca854d79", "gitlab/qt.proxy", "2.6.1")];
        assert_eq!(
            resolve_image_index(
                &images,
                "sha256:5b1bca854d7965a11694e26cb6d07aeaa66064640d3fa81eb2dff9044c7529ed"
            ),
            Some(0)
        );
    }

    #[test]
    fn resolve_image_index_none_when_nothing_matches() {
        let images = vec![image("581c17389e54", "proxy-pilotphone", "latest")];
        assert_eq!(resolve_image_index(&images, "totally-unrelated:v9"), None);
    }

    #[test]
    fn compute_used_marks_only_referenced_images() {
        let images = vec![
            image("aaa", "used-image", "latest"),
            image("bbb", "unused-image", "latest"),
        ];
        let containers = vec![container("c1", "used-image:latest")];
        assert_eq!(compute_used(&images, &containers), vec![true, false]);
    }

    #[test]
    fn compute_used_with_no_containers_marks_nothing_used() {
        let images = vec![image("aaa", "some-image", "latest")];
        assert_eq!(compute_used(&images, &[]), vec![false]);
    }

    #[test]
    fn compute_used_unresolvable_reference_marks_every_image_used_on_doubt() {
        // The Risk register's conservative default: a container image
        // reference this logic can't resolve at all must never let *any*
        // image look safely deletable, even one that in reality has no
        // relationship to that container.
        let images = vec![
            image("aaa", "unrelated-image-one", "latest"),
            image("bbb", "unrelated-image-two", "latest"),
        ];
        let containers = vec![container("c1", "some-digest-or-reference-nothing-matches")];
        assert_eq!(compute_used(&images, &containers), vec![true, true]);
    }

    // --- container_state_from_raw ----------------------------------------------

    #[test]
    fn container_state_mapping_table() {
        let cases = [
            ("running", ContainerState::Running),
            ("paused", ContainerState::Paused),
            ("restarting", ContainerState::Restarting),
            ("exited", ContainerState::Exited),
            ("created", ContainerState::Created),
            ("dead", ContainerState::Dead),
        ];
        for (raw, expected) in cases {
            assert_eq!(container_state_from_raw(raw), expected, "raw state {raw:?}");
        }
    }

    #[test]
    fn container_state_mapping_is_case_insensitive() {
        assert_eq!(container_state_from_raw("RUNNING"), ContainerState::Running);
    }

    #[test]
    fn container_state_unknown_value_maps_to_unknown_variant() {
        assert_eq!(
            container_state_from_raw("some-future-state"),
            ContainerState::Unknown("some-future-state".to_string())
        );
    }

    #[test]
    fn stoppable_and_removable_states_never_overlap() {
        let all = [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
            ContainerState::Unknown("weird".to_string()),
        ];
        for state in &all {
            assert!(
                !(state.is_stoppable() && state.is_removable()),
                "state {state:?} must not be both stoppable and removable"
            );
        }
        assert!(!ContainerState::Unknown("weird".to_string()).is_stoppable());
        assert!(!ContainerState::Unknown("weird".to_string()).is_removable());
    }

    // --- extract_rw_size ---------------------------------------------------------

    #[test]
    fn extract_rw_size_strips_the_virtual_suffix() {
        // A real value on this machine (`docker ps -a --size`).
        assert_eq!(extract_rw_size("767kB (virtual 148MB)"), "767kB");
    }

    #[test]
    fn extract_rw_size_passes_through_a_value_with_no_virtual_suffix() {
        assert_eq!(extract_rw_size("767kB"), "767kB");
    }

    #[test]
    fn extract_rw_size_on_empty_field_yields_empty_string() {
        assert_eq!(extract_rw_size(""), "");
    }

    #[test]
    fn extract_rw_size_trims_surrounding_whitespace() {
        assert_eq!(extract_rw_size("  0B (virtual 202MB)  "), "0B");
    }

    // --- build_container_entry: rw_size mapping -----------------------------------

    #[test]
    fn build_container_entry_maps_size_to_rw_size() {
        let wire = ContainerWire {
            id: "abc".to_string(),
            names: "web".to_string(),
            image: "nginx:alpine".to_string(),
            state: "running".to_string(),
            status: "Up 3 hours".to_string(),
            size: "767kB (virtual 148MB)".to_string(),
            created_at: String::new(),
        };
        assert_eq!(build_container_entry(wire).rw_size, "767kB");
    }

    // --- build_image_entry: rmi reference selection -----------------------------

    #[test]
    fn build_image_entry_tagged_row_uses_repo_tag_as_rmi_reference() {
        let entry = build_image_entry(image("581c17389e54", "proxy-pilotphone", "latest"), false);
        assert_eq!(entry.identity, "proxy-pilotphone:latest");
        assert_eq!(entry.rmi_reference, "proxy-pilotphone:latest");
    }

    #[test]
    fn build_image_entry_untagged_row_uses_id_as_rmi_reference() {
        let entry = build_image_entry(image("581c17389e54", "<none>", "<none>"), false);
        assert_eq!(entry.identity, "<none>:<none>");
        assert_eq!(entry.rmi_reference, "581c17389e54");
    }

    #[test]
    fn build_image_entry_missing_repository_field_treated_as_none() {
        // `#[serde(default)]` degrades a missing Repository/Tag field to
        // "" rather than "<none>" — the mapping must still land on the
        // untagged/by-ID path.
        let entry = build_image_entry(image("581c17389e54", "", ""), false);
        assert_eq!(entry.identity, "<none>:<none>");
        assert_eq!(entry.rmi_reference, "581c17389e54");
    }

    #[test]
    fn build_image_entry_propagates_used_flag() {
        assert!(build_image_entry(image("a", "img", "latest"), true).used);
        assert!(!build_image_entry(image("a", "img", "latest"), false).used);
    }

    // --- build_volume_entry: orphan computation ----------------------------------

    #[test]
    fn build_volume_entry_marks_dangling_membership_as_orphan() {
        let dangling: HashSet<String> = ["orphan-vol".to_string()].into_iter().collect();
        let orphan = build_volume_entry(
            VolumeWire {
                name: "orphan-vol".to_string(),
                driver: "local".to_string(),
                mountpoint: "/var/lib/docker/volumes/orphan-vol/_data".to_string(),
            },
            &dangling,
        );
        assert!(orphan.orphan);

        let attached = build_volume_entry(
            VolumeWire {
                name: "attached-vol".to_string(),
                driver: "local".to_string(),
                mountpoint: "/var/lib/docker/volumes/attached-vol/_data".to_string(),
            },
            &dangling,
        );
        assert!(!attached.orphan);
    }

    // --- command builders: never --force, never prune ----------------------------

    #[test]
    fn command_builders_never_produce_force_or_prune() {
        let forbidden = ["--force", "-f", "prune"];
        let built: Vec<Vec<&str>> = vec![
            stop_container_args("abc123"),
            remove_container_args("abc123"),
            remove_image_args("repo:tag"),
            remove_volume_args("some-volume"),
            // `system df -v` is neither destructive nor a prune, but its
            // args still go through the same builder-list guard so the
            // guard covers every command this module can invoke, not just
            // the four destructive ones.
            volume_sizes_args(),
        ];
        for args in &built {
            for token in args {
                assert!(
                    !forbidden.contains(token),
                    "action command builder produced a forbidden token {token:?} in {args:?}"
                );
            }
        }
    }

    #[test]
    fn stop_container_args_is_plain_stop() {
        assert_eq!(stop_container_args("abc123"), vec!["stop", "abc123"]);
    }

    #[test]
    fn remove_container_args_is_plain_rm() {
        assert_eq!(remove_container_args("abc123"), vec!["rm", "abc123"]);
    }

    #[test]
    fn remove_image_args_is_plain_rmi() {
        assert_eq!(remove_image_args("repo:tag"), vec!["rmi", "repo:tag"]);
    }

    #[test]
    fn remove_volume_args_is_plain_volume_rm() {
        assert_eq!(
            remove_volume_args("some-volume"),
            vec!["volume", "rm", "some-volume"]
        );
    }

    #[test]
    fn volume_sizes_args_is_plain_system_df_v() {
        assert_eq!(
            volume_sizes_args(),
            vec!["system", "df", "-v", "--format", "{{json .}}"]
        );
    }

    // --- NDJSON parsing: tolerance -------------------------------------------

    #[test]
    fn parse_ndjson_skips_blank_lines() {
        let raw = "\n\n{\"ID\":\"abc\",\"Names\":\"x\"}\n\n";
        let entries: Vec<ContainerWire> = parse_ndjson(raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "abc");
    }

    #[test]
    fn parse_ndjson_skips_garbage_lines_without_failing_the_batch() {
        let raw = "not json at all\n{\"ID\":\"abc\"}\n{broken\n{\"ID\":\"def\"}\n";
        let entries: Vec<ContainerWire> = parse_ndjson(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "abc");
        assert_eq!(entries[1].id, "def");
    }

    #[test]
    fn parse_ndjson_on_entirely_empty_input_yields_empty_vec() {
        let entries: Vec<ContainerWire> = parse_ndjson("");
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_ndjson_tolerates_missing_fields_via_serde_default() {
        let raw = "{\"ID\":\"abc\"}\n";
        let entries: Vec<ContainerWire> = parse_ndjson(raw);
        assert_eq!(entries[0].id, "abc");
        assert_eq!(entries[0].names, "");
        assert_eq!(entries[0].image, "");
        assert_eq!(entries[0].state, "");
    }

    // --- NDJSON parsing: real fixtures captured on this machine ------------------
    //
    // Docker 29.7.2, captured while implementing this module via
    // `docker ps -a --size / images / volume ls --format '{{json .}}'`. Kept
    // verbatim (including the elided `…` chars in `Mounts`, the
    // `>`-escaped `>` in `Ports`, and the `null` `Platform` field) as
    // real-world tolerance material, not reconstructed/simplified. `Size` was
    // already present in `docker ps -a`'s JSON output even before `--size`
    // was added to the args here (recaptured with `--size` explicitly for
    // this milestone, values unchanged) — Docker always fills the field, the
    // flag only affects the human-readable table view.

    const REAL_PS_FIXTURE: &str = r#"{"Command":"\"./launch.sh\"","CreatedAt":"2026-08-18 09:13:38 +0200 CEST","HealthStatus":"none","ID":"023f0aceae80","Image":"gitlab.smartlockers.io:5050/smartlockers/qt.proxy:2.6.1","Labels":"com.docker.compose.config-hash=5e73812569dbf855dcad494dd5a3766666153ad2ce92d5c0d45097c40b2570e0,com.docker.compose.container-number=1,com.docker.compose.depends_on=cert-provider:service_started:false,com.docker.compose.image=sha256:5b1bca854d7965a11694e26cb6d07aeaa66064640d3fa81eb2dff9044c7529ed,com.docker.compose.oneoff=False,com.docker.compose.project.config_files=/home/tnn/Projets/SmartLockers/onet/pilotphone/proxy/docker-compose.yml,com.docker.compose.project.working_dir=/home/tnn/Projets/SmartLockers/onet/pilotphone/proxy,com.docker.compose.project=proxy,com.docker.compose.replace=proxy,com.docker.compose.service=proxy,com.docker.compose.version=5.5.0","LocalVolumes":"4","Mounts":"proxy_certs,proxy_proxy-da…,proxy_proxy-da…,proxy_proxy-sv…","Names":"proxy","Networks":"proxy_socket-net","Platform":null,"Ports":"0.0.0.0:5656->5656/tcp, [::]:5656->5656/tcp","RunningFor":"28 hours ago","Size":"767kB (virtual 148MB)","State":"running","Status":"Up 4 hours"}
{"Command":"\"buildkitd\"","CreatedAt":"2024-09-03 11:28:58 +0200 CEST","HealthStatus":"none","ID":"b3cf9638b07f","Image":"moby/buildkit:buildx-stable-1","Labels":"","LocalVolumes":"1","Mounts":"buildx_buildki…","Names":"buildx_buildkit_mybuilder0","Networks":"bridge","Platform":null,"Ports":"","RunningFor":"23 months ago","Size":"0B (virtual 202MB)","State":"running","Status":"Up 4 hours"}"#;

    const REAL_IMAGES_FIXTURE: &str = r#"{"Containers":"1","CreatedAt":"2026-08-17 11:07:13 +0200 CEST","CreatedSince":"2 days ago","Digest":"<none>","ID":"581c17389e54","Repository":"proxy-pilotphone","SharedSize":"N/A","Size":"192MB","Tag":"latest","UniqueSize":"N/A"}
{"Containers":"1","CreatedAt":"2026-02-05 01:07:18 +0100 CET","CreatedSince":"6 months ago","Digest":"<none>","ID":"b76de378d572","Repository":"nginx","SharedSize":"N/A","Size":"62.1MB","Tag":"alpine","UniqueSize":"N/A"}
{"Containers":"1","CreatedAt":"2024-08-15 17:24:38 +0200 CEST","CreatedSince":"2 years ago","Digest":"<none>","ID":"0b1ee39de203","Repository":"moby/buildkit","SharedSize":"N/A","Size":"202MB","Tag":"buildx-stable-1","UniqueSize":"N/A"}"#;

    const REAL_VOLUMES_FIXTURE: &str = r#"{"Availability":"N/A","Driver":"local","Group":"N/A","Labels":"com.docker.volume.anonymous=","Links":"N/A","Mountpoint":"/var/lib/docker/volumes/2a0d72d8666787c8353bf98a8b8ee04083563202c534ac137deb6a8a0a177795/_data","Name":"2a0d72d8666787c8353bf98a8b8ee04083563202c534ac137deb6a8a0a177795","Scope":"local","Size":"N/A","Status":"N/A"}
{"Availability":"N/A","Driver":"local","Group":"N/A","Labels":"com.docker.compose.config-hash=ca71a0505b1ac5f04fe4329fb080da4681b9299ad536767e63a66e189a9da3c9,com.docker.compose.project=proxy,com.docker.compose.version=2.32.4,com.docker.compose.volume=certs","Links":"N/A","Mountpoint":"/var/lib/docker/volumes/proxy_certs/_data","Name":"proxy_certs","Scope":"local","Size":"N/A","Status":"N/A"}"#;

    /// `docker system df -v --format '{{json .}}'` output is one JSON
    /// object, not NDJSON; captured on this machine and trimmed to its
    /// `"Volumes"` array's first three real entries (~4.6s full-command
    /// runtime, ~30 volumes on this machine — see [`volume_sizes`]'s doc
    /// comment) plus one deliberately-zero-size volume (`"Size":"0B"`), kept
    /// verbatim as real-world tolerance material.
    const REAL_DF_V_FIXTURE: &str = r#"{"Volumes":[{"Availability":"N/A","Driver":"local","Group":"N/A","Labels":"com.docker.compose.config-hash=3598cb10b4813b62c8d89ca630064e2baaec2683b14f6aeb54dd77d37fcdd4b0,com.docker.compose.project=proxy,com.docker.compose.version=2.32.4,com.docker.compose.volume=dist","Links":"0","Mountpoint":"/var/lib/docker/volumes/proxy_dist/_data","Name":"proxy_dist","Scope":"local","Size":"199.9MB","Status":"N/A"},{"Availability":"N/A","Driver":"local","Group":"N/A","Labels":"com.docker.compose.config-hash=c206b51586ac29f38580dd9fdfe50c01fe12039e0d161590151f629f7b33c56c,com.docker.compose.project=proxy,com.docker.compose.version=2.32.4,com.docker.compose.volume=proxy-svg-resource","Links":"2","Mountpoint":"/var/lib/docker/volumes/proxy_proxy-svg-resource/_data","Name":"proxy_proxy-svg-resource","Scope":"local","Size":"6.64MB","Status":"N/A"},{"Availability":"N/A","Driver":"local","Group":"N/A","Labels":"com.docker.compose.config-hash=a02a8f6651cce9b2355ff98dd8d67046de0c1db1c97b6df582d1c4d8de9a377e,com.docker.compose.project=suddenly,com.docker.compose.version=5.1.3,com.docker.compose.volume=pip-cache","Links":"0","Mountpoint":"/var/lib/docker/volumes/suddenly_pip-cache/_data","Name":"suddenly_pip-cache","Scope":"local","Size":"0B","Status":"N/A"}]}"#;

    #[test]
    fn real_ps_fixture_parses_without_error() {
        let entries: Vec<ContainerWire> = parse_ndjson(REAL_PS_FIXTURE);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "023f0aceae80");
        assert_eq!(entries[0].names, "proxy");
        assert_eq!(
            entries[0].image,
            "gitlab.smartlockers.io:5050/smartlockers/qt.proxy:2.6.1"
        );
        assert_eq!(entries[0].state, "running");
        assert_eq!(entries[0].created_at, "2026-08-18 09:13:38 +0200 CEST");
        assert_eq!(entries[0].size, "767kB (virtual 148MB)");
        assert_eq!(extract_rw_size(&entries[0].size), "767kB");
        assert_eq!(entries[1].names, "buildx_buildkit_mybuilder0");
        assert_eq!(entries[1].image, "moby/buildkit:buildx-stable-1");
        assert_eq!(extract_rw_size(&entries[1].size), "0B");
    }

    #[test]
    fn real_images_fixture_parses_without_error() {
        let entries: Vec<ImageWire> = parse_ndjson(REAL_IMAGES_FIXTURE);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].repository, "proxy-pilotphone");
        assert_eq!(entries[0].tag, "latest");
        assert_eq!(entries[2].id, "0b1ee39de203");
        assert_eq!(entries[2].repository, "moby/buildkit");
        assert_eq!(entries[2].containers_hint, "1");
    }

    #[test]
    fn real_volumes_fixture_parses_without_error() {
        let entries: Vec<VolumeWire> = parse_ndjson(REAL_VOLUMES_FIXTURE);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].name, "proxy_certs");
        assert_eq!(entries[1].driver, "local");
        assert_eq!(
            entries[1].mountpoint,
            "/var/lib/docker/volumes/proxy_certs/_data"
        );
    }

    // --- parse_volume_sizes (docker system df -v) ---------------------------------

    #[test]
    fn real_df_v_fixture_parses_into_name_size_pairs() {
        let sizes = parse_volume_sizes(REAL_DF_V_FIXTURE);
        assert_eq!(
            sizes,
            vec![
                ("proxy_dist".to_string(), "199.9MB".to_string()),
                ("proxy_proxy-svg-resource".to_string(), "6.64MB".to_string()),
                ("suddenly_pip-cache".to_string(), "0B".to_string()),
            ]
        );
    }

    #[test]
    fn parse_volume_sizes_on_absent_volumes_key_yields_empty_vec() {
        // No "Volumes" key at all — e.g. a daemon reporting only
        // Images/Containers/BuildCache for some reason.
        assert!(parse_volume_sizes(r#"{"Images":[],"Containers":[]}"#).is_empty());
    }

    #[test]
    fn parse_volume_sizes_on_empty_volumes_array_yields_empty_vec() {
        assert!(parse_volume_sizes(r#"{"Volumes":[]}"#).is_empty());
    }

    #[test]
    fn parse_volume_sizes_on_unparseable_body_yields_empty_vec_not_panic() {
        assert!(parse_volume_sizes("not json at all").is_empty());
        assert!(parse_volume_sizes("").is_empty());
    }

    #[test]
    fn parse_volume_sizes_drops_entries_with_a_blank_name() {
        let raw = r#"{"Volumes":[{"Name":"","Size":"10MB"},{"Name":"real-vol","Size":"5MB"}]}"#;
        assert_eq!(
            parse_volume_sizes(raw),
            vec![("real-vol".to_string(), "5MB".to_string())]
        );
    }

    #[test]
    fn real_fixture_cross_reference_marks_moby_buildkit_used() {
        // On this real machine, `buildx_buildkit_mybuilder0`'s Image field
        // ("moby/buildkit:buildx-stable-1") must resolve to the
        // `moby/buildkit` image row by repo:tag — end-to-end proof that
        // real captured ps/images data cross-references correctly, not
        // just synthetic fixtures.
        let containers: Vec<ContainerWire> = parse_ndjson(REAL_PS_FIXTURE);
        let images: Vec<ImageWire> = parse_ndjson(REAL_IMAGES_FIXTURE);
        let used = compute_used(&images, &containers);
        let moby_index = images
            .iter()
            .position(|image| image.repository == "moby/buildkit")
            .expect("fixture must contain the moby/buildkit image");
        assert!(
            used[moby_index],
            "moby/buildkit:buildx-stable-1 is referenced by buildx_buildkit_mybuilder0 and must \
             be marked used"
        );
    }

    // --- Real-machine tests (acceptance criteria) --------------------------------
    //
    // Gated on binary_available(): skip cleanly (no failure) when docker
    // isn't installed, and — per the plan's Phase 1 acceptance criteria —
    // assert row well-formedness rather than list non-emptiness, so a
    // machine with docker installed but zero containers/images/volumes
    // still passes.

    #[test]
    fn real_fetch_returns_well_formed_rows_when_docker_present() {
        if !binary_available() {
            eprintln!("docker introuvable sur cette machine: test ignoré");
            return;
        }

        let snapshot =
            fetch().expect("fetch() doit réussir avec un daemon Docker actif sur cette machine");

        for container in &snapshot.containers {
            assert!(
                !container.id.is_empty(),
                "chaque conteneur doit avoir un ID"
            );
        }
        for image in &snapshot.images {
            assert!(!image.id.is_empty(), "chaque image doit avoir un ID");
            assert!(
                image.identity.contains(':'),
                "l'identité d'image doit être au format repo:tag, obtenu {:?}",
                image.identity
            );
        }
        for volume in &snapshot.volumes {
            assert!(!volume.name.is_empty(), "chaque volume doit avoir un nom");
        }

        // Manual-check acceptance criterion ("fetch() on this machine
        // returns non-empty containers and images, each image's used flag
        // consistent with docker ps -a"): printed for `--nocapture`
        // inspection rather than asserted, since a real machine's docker
        // state isn't something this test suite controls.
        eprintln!(
            "fetch() réel: {} conteneur(s), {} image(s) ({} marquée(s) utilisée(s)), {} \
             volume(s) ({} orpheline(s))",
            snapshot.containers.len(),
            snapshot.images.len(),
            snapshot.images.iter().filter(|i| i.used).count(),
            snapshot.volumes.len(),
            snapshot.volumes.iter().filter(|v| v.orphan).count(),
        );
        for image in &snapshot.images {
            eprintln!("  image {:<45} used={}", image.identity, image.used);
        }
    }

    #[test]
    fn real_volume_sizes_returns_well_formed_pairs_when_docker_present() {
        if !binary_available() {
            eprintln!("docker introuvable sur cette machine: test ignoré");
            return;
        }

        let sizes = volume_sizes()
            .expect("volume_sizes() doit réussir avec un daemon Docker actif sur cette machine");
        for (name, size) in &sizes {
            assert!(!name.is_empty(), "chaque volume doit avoir un nom");
            assert!(
                !size.is_empty(),
                "chaque volume doit avoir une taille non vide, obtenu vide pour {name:?}"
            );
        }

        // Manual-check acceptance criterion ("volume_sizes() on this machine
        // returns ~31 entries with non-empty sizes"): printed for
        // `--nocapture` inspection, since real machine state isn't
        // controlled by this test suite.
        eprintln!("volume_sizes() réel: {} volume(s)", sizes.len());
        for (name, size) in &sizes {
            eprintln!("  volume {name:<45} size={size}");
        }
    }

    #[test]
    fn real_binary_available_does_not_panic() {
        // Exercises the real std::env-backed public function at least
        // once, mirroring `platform::linux`'s
        // `public_functions_do_not_panic` — no assertion on the outcome,
        // since whether docker is installed is outside this test's
        // control.
        let _ = binary_available();
    }
}
