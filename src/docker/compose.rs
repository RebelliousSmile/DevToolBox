//! Docker Compose data source (Linux-only).
//!
//! Same contract as [`crate::docker::engine`]: this module knows how to *talk*
//! to the compose plugin and maps its output onto the OS-neutral types of
//! [`crate::ui::compose_view`], which is where every type the UI names lives.
//! Nothing here is referenced from `src/ui/` except through that module's
//! façade, so a Windows build — where this file compiles to nothing — keeps
//! linking.
//!
//! # Why the CLI rather than a YAML parser
//!
//! `docker compose config` is the only thing that resolves a compose file the
//! way compose itself does: `extends`, `include`, multiple `-f` layers,
//! `.env` interpolation, profiles, the `${VAR:-default}` grammar. Parsing the
//! YAML here would agree with compose on the simple files and diverge silently
//! on the interesting ones. Measured at ~89 ms per file on this machine, and
//! **it needs no daemon** — which is what makes the Stacks list readable when
//! Docker is not even running.
//!
//! # stdout only
//!
//! `docker compose config --format json` writes its JSON to stdout and its
//! `level=warning` lines (unset `.env` variables, deprecated keys) to stderr,
//! exiting 0 all the same. Merging the two streams — the reflexive `2>&1` —
//! corrupts the parse on 6 of the 13 real compose files under `$HOME` here.
//! [`run_command_with_timeout`] already returns stdout alone; that is not an
//! incidental detail of the helper but a requirement of this caller.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;
use walkdir::{DirEntry, WalkDir};

use crate::docker::engine::{
    binary_available, run_command_with_timeout, DockerError, OperationClass,
};
use crate::ui::compose_view::{ScanOutcome, StackConfig, StackService};
use crate::ui::ports::PortBinding;

/// `docker compose config` on a cold plugin (first invocation loads the
/// plugin binary) measured well under a second here, but a file pulling a
/// large `include` tree can take longer. 15 s is generous enough that a
/// timeout means something is genuinely wrong, and short enough that a wedged
/// call does not hold the Stacks list hostage.
const COMPOSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Past this, the `$HOME` walk gets a visible warning rather than a silent
/// long pause. The real scan takes ~1 s here with the exclusions below; 20 s
/// means the exclusion list is missing something big on this machine.
const SCAN_WARN_MS: u128 = 20_000;

/// The four names compose itself looks for with no `-f` flag.
const COMPOSE_FILE_NAMES: [&str; 4] = [
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

/// Stems a variant file may carry, i.e. the part before the middle segment.
const COMPOSE_STEMS: [&str; 2] = ["docker-compose", "compose"];

/// Extensions a compose file may carry.
const COMPOSE_EXTENSIONS: [&str; 2] = ["yml", "yaml"];

/// The one middle segment that never denotes a stack of its own.
///
/// `docker-compose.override.yml` is loaded *automatically* alongside the
/// canonical file, so it is a fragment by definition: listing it separately
/// would put a second row on screen for a stack that is already there, and
/// its `config` would describe the override in isolation rather than the
/// merge compose actually runs. Every other middle segment (`dev`, `prod`,
/// `test`, …) is only ever loaded through an explicit `-f`, which is exactly
/// what DevToolBox does, so those are real launchable stacks.
const OVERRIDE_SEGMENT: &str = "override";

/// Directory names never descended into.
///
/// `filter_entry` prunes these *without walking them*, which is the whole
/// reason [`walkdir`] is a dependency: a `node_modules` tree can hold more
/// entries than the rest of `$HOME` combined, and a compose file inside one
/// belongs to a vendored package, not to the user's projects.
const EXCLUDED_DIRS: [&str; 14] = [
    "node_modules",
    "vendor",
    "target",
    "build",
    "dist",
    "venv",
    ".venv",
    "__pycache__",
    "site-packages",
    "Trash",
    "snap",
    ".cache",
    "go",
    ".steam",
];

// ---------------------------------------------------------------------------
// Plugin detection
// ---------------------------------------------------------------------------

/// `true` when the `docker compose` **plugin** answers.
///
/// The legacy standalone `docker-compose` binary is deliberately not probed:
/// its `config` has no `--format json`, so every downstream assumption here
/// would break. Better an explicit "plugin introuvable" than a Stacks list
/// that fails one file at a time.
pub fn plugin_available() -> bool {
    if !binary_available() {
        return false;
    }
    run_command_with_timeout(
        "docker",
        &["compose", "version"],
        Duration::from_secs(5),
        OperationClass::Listing,
    )
    .is_ok()
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn is_compose_file(entry: &DirEntry) -> bool {
    entry.file_type().is_file() && entry.file_name().to_str().is_some_and(is_compose_file_name)
}

/// Whether `name` denotes a compose file DevToolBox can launch.
///
/// Two shapes qualify: the canonical [`COMPOSE_FILE_NAMES`], and the
/// `<stem>.<middle>.<ext>` variants (`docker-compose.dev.yml`,
/// `compose.prod.yaml`, …) minus the [`OVERRIDE_SEGMENT`] fragment.
///
/// Variants are included because ignoring them does not make their stacks go
/// away — it makes them arrive through the back door. A container started
/// from `docker-compose.dev.yml` still carries that path in its
/// `config_files` label, so the Stacks list showed it as a run with no file
/// behind it (« hors scan · services et ports inconnus »): no services, no
/// declared ports, no conflict detection, and a disabled action button.
/// Measured on this machine: `suddenly`'s three containers, whose
/// `docker-compose.dev.yml` sits right next to the canonical file.
///
/// Whether a variant is genuinely standalone is **not** decided here — the
/// `config` call that follows discovery already decides it, and stores the
/// error on the row when a fragment cannot stand alone.
///
/// Matching stays case-sensitive, as it was for the canonical names alone:
/// compose itself is case-sensitive about them, and following the Windows
/// filesystem's case-insensitivity here would make the two OSes disagree
/// about which files exist.
fn is_compose_file_name(name: &str) -> bool {
    if COMPOSE_FILE_NAMES.contains(&name) {
        return true;
    }

    let segments: Vec<&str> = name.split('.').collect();
    // `<stem>.<middle…>.<ext>`: at least one middle segment, or this is a
    // canonical name that the check above already answered.
    if segments.len() < 3 {
        return false;
    }
    let (stem, extension) = (segments[0], segments[segments.len() - 1]);
    let middle = &segments[1..segments.len() - 1];

    COMPOSE_STEMS.contains(&stem)
        && COMPOSE_EXTENSIONS.contains(&extension)
        && middle.iter().all(|segment| !segment.is_empty())
        && !middle.contains(&OVERRIDE_SEGMENT)
}

/// Whether the walk should descend into `entry`.
///
/// Files always pass — the decision is about directories only. Hidden
/// directories are pruned wholesale (`.git`, `.cargo`, `.local`, `.config`…):
/// none of them holds a project the user would launch, and `.git` alone is
/// tens of thousands of entries per repository. `depth == 0` is the scan root
/// itself, which must never be pruned even when it is hidden.
fn should_descend(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() || entry.depth() == 0 {
        return true;
    }
    let Some(name) = entry.file_name().to_str() else {
        return true;
    };
    !name.starts_with('.') && !EXCLUDED_DIRS.contains(&name)
}

/// Walk `root` for compose files.
///
/// Never fails: an unreadable directory is counted in
/// [`ScanOutcome::denied_dirs`] and skipped. A scan that finds nothing is a
/// legitimate answer, not an error.
pub fn discover(root: &Path) -> ScanOutcome {
    let started = Instant::now();
    let mut outcome = ScanOutcome::default();

    for result in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend)
    {
        match result {
            Ok(entry) => {
                if entry.file_type().is_dir() {
                    outcome.visited_dirs += 1;
                } else if is_compose_file(&entry) {
                    outcome.files.push(entry.path().display().to_string());
                }
            }
            // A directory that vanished mid-walk or that this user cannot
            // read. Counted, never fatal.
            Err(_) => outcome.denied_dirs += 1,
        }
    }

    outcome.files.sort();
    outcome.elapsed_ms = started.elapsed().as_millis();
    if outcome.elapsed_ms > SCAN_WARN_MS {
        outcome.warning = Some(format!(
            "scan lent ({} s pour {} dossiers) — un dossier volumineux échappe aux exclusions",
            outcome.elapsed_ms / 1000,
            outcome.visited_dirs
        ));
    }
    outcome
}

// ---------------------------------------------------------------------------
// Config reading
// ---------------------------------------------------------------------------

/// `published` is a **string** in all 13 compose files measured here
/// (`"8081"`), but the schema allows a number and other compose versions emit
/// one. Accepting both costs one enum and removes a whole class of
/// "works on my machine" parse failures.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PublishedPort {
    Text(String),
    Number(u64),
}

impl PublishedPort {
    fn as_str(&self) -> String {
        match self {
            PublishedPort::Text(text) => text.clone(),
            PublishedPort::Number(number) => number.to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PortWire {
    #[serde(default)]
    published: Option<PublishedPort>,
    #[serde(default)]
    target: Option<u64>,
    #[serde(default)]
    protocol: Option<String>,
    /// Absent from every published port measured here; present in the schema.
    #[serde(default)]
    host_ip: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ServiceWire {
    /// **Absent**, not `null`, for a service that publishes nothing — measured
    /// on `cert-provider`. `Option` + `#[serde(default)]` covers both shapes.
    #[serde(default)]
    ports: Option<Vec<PortWire>>,
    #[serde(default)]
    network_mode: Option<String>,
}

/// `docker compose config --format json`, reduced to what the Stacks section
/// reads. A `BTreeMap` rather than a `HashMap`: without the `preserve_order`
/// feature `serde_json` does not keep object order, and a service list that
/// reshuffles between two scans would make the rows flicker.
#[derive(Debug, Clone, Deserialize, Default)]
struct ConfigWire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    services: BTreeMap<String, ServiceWire>,
}

/// Ask compose to resolve `file` and map the result.
pub fn read_config(file: &Path) -> Result<StackConfig, DockerError> {
    if !binary_available() {
        return Err(DockerError::BinaryMissing);
    }
    let path = file.display().to_string();
    let stdout = run_command_with_timeout(
        "docker",
        &["compose", "-f", &path, "config", "--format", "json"],
        COMPOSE_TIMEOUT,
        // `Action`, not `Listing`: `config` never touches the daemon, so a
        // timeout here says nothing about the daemon's health and must not be
        // reported as `DaemonUnreachable`.
        OperationClass::Action,
    )?;
    parse_config(&stdout)
}

/// Pure mapping of one `config --format json` payload — the whole reason
/// `read_config` is three lines.
fn parse_config(raw: &str) -> Result<StackConfig, DockerError> {
    let wire: ConfigWire = serde_json::from_str(raw.trim()).map_err(|error| {
        DockerError::CommandFailed(format!(
            "sortie de `docker compose config` illisible: {error}"
        ))
    })?;

    let services = wire
        .services
        .into_iter()
        .map(|(name, service)| {
            let host_network = service.network_mode.as_deref() == Some("host");
            StackService {
                name,
                ports: service
                    .ports
                    .unwrap_or_default()
                    .iter()
                    .filter_map(binding_from_wire)
                    .collect(),
                host_network,
            }
        })
        .collect();

    Ok(StackConfig {
        name: wire.name,
        services,
    })
}

/// One declared port, or `None` when it declares no comparable host binding.
///
/// A `ports: ["8080"]` short form with no host side, or a range
/// (`"8000-8010"`), resolves to a host port compose picks at runtime: there is
/// nothing to compare against a running container, so the entry is dropped
/// rather than guessed. Dropping it costs a conflict warning; guessing it
/// would invent one.
fn binding_from_wire(wire: &PortWire) -> Option<PortBinding> {
    let published = wire.published.as_ref()?.as_str();
    let host_port: u16 = published.trim().parse().ok()?;
    Some(PortBinding {
        host_ip: wire
            .host_ip
            .clone()
            .unwrap_or_default()
            .trim_matches(['[', ']'])
            .to_string(),
        host_port,
        container_port: wire.target.unwrap_or(host_port as u64) as u16,
        protocol: wire
            .protocol
            .clone()
            .unwrap_or_else(|| "tcp".to_string())
            .to_ascii_lowercase(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A payload in the exact shape `docker compose config --format json`
    /// produced here, `network_mode: host` service included (project
    /// `qtproxy`, the one real occurrence on this machine).
    const CONFIG_JSON: &str = r#"{
      "name": "smartlockers-lab",
      "services": {
        "web": {
          "image": "nginx",
          "ports": [
            {"mode": "ingress", "target": 80, "published": "8080", "protocol": "tcp"},
            {"mode": "ingress", "target": 443, "published": "8443", "protocol": "tcp"}
          ]
        },
        "cert-provider": {
          "image": "certbot"
        },
        "proxy": {
          "image": "haproxy",
          "network_mode": "host"
        }
      }
    }"#;

    #[test]
    fn parse_config_maps_name_services_and_bindings() {
        let config = parse_config(CONFIG_JSON).expect("must parse");
        assert_eq!(config.name, "smartlockers-lab");
        // BTreeMap ordering: alphabetical, stable between two scans.
        let names: Vec<&str> = config
            .services
            .iter()
            .map(|service| service.name.as_str())
            .collect();
        assert_eq!(names, vec!["cert-provider", "proxy", "web"]);

        let web = &config.services[2];
        assert_eq!(web.ports.len(), 2);
        assert_eq!(web.ports[0].host_port, 8080);
        assert_eq!(web.ports[0].container_port, 80);
        assert_eq!(web.ports[0].protocol, "tcp");
        assert!(
            web.ports[0].host_ip.is_empty(),
            "no host_ip is measured here"
        );
    }

    #[test]
    fn a_service_with_no_ports_key_parses_to_an_empty_list() {
        // Measured: the key is *absent*, not `null`.
        let config = parse_config(CONFIG_JSON).expect("must parse");
        let cert = &config.services[0];
        assert_eq!(cert.name, "cert-provider");
        assert!(cert.ports.is_empty());
        assert!(!cert.host_network);
    }

    #[test]
    fn a_null_ports_key_also_parses_to_an_empty_list() {
        let config =
            parse_config(r#"{"name":"a","services":{"web":{"ports":null}}}"#).expect("must parse");
        assert!(config.services[0].ports.is_empty());
    }

    #[test]
    fn host_network_mode_is_flagged() {
        let config = parse_config(CONFIG_JSON).expect("must parse");
        let proxy = &config.services[1];
        assert_eq!(proxy.name, "proxy");
        assert!(proxy.host_network);
        assert!(
            proxy.ports.is_empty(),
            "a host-network service publishes nothing to compare"
        );
    }

    #[test]
    fn published_parses_from_both_a_string_and_a_number() {
        let as_text =
            parse_config(r#"{"name":"a","services":{"w":{"ports":[{"published":"8080"}]}}}"#)
                .expect("string form");
        let as_number =
            parse_config(r#"{"name":"a","services":{"w":{"ports":[{"published":8080}]}}}"#)
                .expect("number form");
        assert_eq!(as_text.services[0].ports[0].host_port, 8080);
        assert_eq!(as_number.services[0].ports[0].host_port, 8080);
        // With no `target`, the container port falls back to the host port.
        assert_eq!(as_text.services[0].ports[0].container_port, 8080);
    }

    #[test]
    fn a_port_with_no_resolvable_host_side_is_dropped_not_guessed() {
        for payload in [
            r#"{"name":"a","services":{"w":{"ports":[{"target":80}]}}}"#,
            r#"{"name":"a","services":{"w":{"ports":[{"target":80,"published":""}]}}}"#,
            r#"{"name":"a","services":{"w":{"ports":[{"target":80,"published":"8000-8010"}]}}}"#,
        ] {
            let config = parse_config(payload).expect("must parse");
            assert!(
                config.services[0].ports.is_empty(),
                "a runtime-assigned host port must not be invented: {payload}"
            );
        }
    }

    #[test]
    fn a_bracketed_ipv6_host_ip_loses_its_brackets() {
        let config = parse_config(
            r#"{"name":"a","services":{"w":{"ports":[{"published":"80","host_ip":"[::]"}]}}}"#,
        )
        .expect("must parse");
        assert_eq!(config.services[0].ports[0].host_ip, "::");
    }

    #[test]
    fn an_unparsable_payload_is_a_command_failure_not_a_panic() {
        // The `2>&1` trap: compose's own `level=warning` line prepended to
        // otherwise valid JSON. It must fail this one file, loudly, and never
        // take the scan down with it.
        let error =
            parse_config("time=\"2026-08-21\" level=warning msg=\"unset\"\n{\"name\":\"a\"}")
                .expect_err("must fail");
        assert!(matches!(error, DockerError::CommandFailed(_)));
    }

    #[test]
    fn an_empty_services_map_still_yields_the_project_name() {
        let config = parse_config(r#"{"name":"lonely","services":{}}"#).expect("must parse");
        assert_eq!(config.name, "lonely");
        assert!(config.services.is_empty());
    }

    // --- discovery -----------------------------------------------------------

    fn temp_tree(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("devtoolbox-compose-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp root");
        root
    }

    /// Truth table for the file-name rule, canonical and variant alike.
    #[test]
    fn compose_file_names_accept_canonical_and_variant_shapes() {
        for name in [
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yml",
            "compose.yaml",
            "docker-compose.dev.yml",
            "docker-compose.prod.yaml",
            "compose.test.yml",
            // Several middle segments are still one variant, not a fragment.
            "docker-compose.dev.local.yml",
        ] {
            assert!(is_compose_file_name(name), "should accept {name}");
        }

        for name in [
            // Loaded automatically with the canonical file: a fragment, and a
            // duplicate row if listed.
            "docker-compose.override.yml",
            "compose.override.yaml",
            "docker-compose.dev.override.yml",
            // Not compose files at all.
            "docker-compose.txt",
            "docker-compose.yml.bak",
            "compose.json",
            "notes.yml",
            "my-docker-compose.yml",
            // Empty middle segment: `docker-compose..yml`.
            "docker-compose..yml",
        ] {
            assert!(!is_compose_file_name(name), "should reject {name}");
        }
    }

    /// The measured `suddenly` case: a variant sitting next to the canonical
    /// file. Both are real, launchable stacks and both must be listed —
    /// before this, the variant's running containers showed up as a run with
    /// no file behind it.
    #[test]
    fn discover_lists_a_variant_alongside_its_canonical_sibling() {
        let root = temp_tree("variant-sibling");
        let app = root.join("app");
        fs::create_dir_all(&app).expect("project dir");
        fs::write(app.join("docker-compose.yml"), "services: {}").expect("write");
        fs::write(app.join("docker-compose.dev.yml"), "services: {}").expect("write");
        fs::write(app.join("docker-compose.override.yml"), "services: {}").expect("write");

        let outcome = discover(&root);

        assert_eq!(outcome.files.len(), 2, "found: {:?}", outcome.files);
        assert!(outcome
            .files
            .iter()
            .any(|file| Path::new(file).ends_with("app/docker-compose.yml")));
        assert!(outcome
            .files
            .iter()
            .any(|file| Path::new(file).ends_with("app/docker-compose.dev.yml")));
        assert!(
            !outcome.files.iter().any(|file| file.contains("override")),
            "the override fragment must not get a row: {:?}",
            outcome.files
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_finds_the_four_compose_names_and_ignores_others() {
        let root = temp_tree("names");
        for (index, name) in COMPOSE_FILE_NAMES.iter().enumerate() {
            let dir = root.join(format!("p{index}"));
            fs::create_dir_all(&dir).expect("project dir");
            fs::write(dir.join(name), "services: {}").expect("write");
        }
        fs::write(root.join("docker-compose.txt"), "nope").expect("write");
        fs::write(root.join("compose.json"), "nope").expect("write");

        let outcome = discover(&root);
        assert_eq!(outcome.files.len(), 4, "found: {:?}", outcome.files);
        assert!(outcome.warning.is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_prunes_excluded_and_hidden_directories() {
        let root = temp_tree("prune");
        for buried in ["node_modules/pkg", ".git/objects", "target/debug", ".cache"] {
            let dir = root.join(buried);
            fs::create_dir_all(&dir).expect("buried dir");
            fs::write(dir.join("docker-compose.yml"), "services: {}").expect("write");
        }
        let kept = root.join("app");
        fs::create_dir_all(&kept).expect("kept dir");
        fs::write(kept.join("docker-compose.yml"), "services: {}").expect("write");

        let outcome = discover(&root);
        assert_eq!(outcome.files.len(), 1, "found: {:?}", outcome.files);
        // Compared as a `Path`, not as a `String`: `files` holds native
        // separators, so a literal `str::ends_with("app/docker-compose.yml")`
        // only ever matched on Linux.
        assert!(
            Path::new(&outcome.files[0]).ends_with("app/docker-compose.yml"),
            "found: {:?}",
            outcome.files
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_on_a_hidden_root_still_walks_it() {
        // `depth == 0` is the scan root: pruning it because its name starts
        // with a dot would return an empty scan for a perfectly valid root.
        let root = temp_tree(".hidden");
        fs::write(root.join("compose.yml"), "services: {}").expect("write");
        assert_eq!(discover(&root).files.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_on_a_missing_root_reports_a_denied_entry_and_no_files() {
        let root = std::env::temp_dir().join(format!("devtoolbox-absent-{}", std::process::id()));
        let outcome = discover(&root);
        assert!(outcome.files.is_empty());
        assert_eq!(outcome.denied_dirs, 1, "the unreadable root is counted");
    }

    #[test]
    fn discover_returns_a_sorted_stable_list() {
        let root = temp_tree("sorted");
        for name in ["zeta", "alpha", "mid"] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).expect("dir");
            fs::write(dir.join("compose.yml"), "services: {}").expect("write");
        }
        let first = discover(&root).files;
        let second = discover(&root).files;
        assert_eq!(first, second, "two scans of the same tree must agree");
        let mut sorted = first.clone();
        sorted.sort();
        assert_eq!(first, sorted);
        let _ = fs::remove_dir_all(&root);
    }

    // The test below is `#[ignore]`d: it walks the developer's REAL `$HOME`
    // and shells out to `docker compose config` for every file it finds. It
    // exists so a human can run `cargo test -- --ignored real_home_scan` once
    // and read the measured numbers (file count, wall clock, denied dirs)
    // that Checkpoint 2 asks for, without a GUI.
    #[test]
    #[ignore = "walks the real $HOME and runs docker compose config on every hit; run manually"]
    fn real_home_scan_reports_its_files_and_wall_clock() {
        let home = PathBuf::from(std::env::var("HOME").expect("$HOME is set"));
        let outcome = discover(&home);
        eprintln!(
            "{} fichiers, {} dossiers visités, {} refusés, {} ms",
            outcome.files.len(),
            outcome.visited_dirs,
            outcome.denied_dirs,
            outcome.elapsed_ms
        );
        for file in &outcome.files {
            match read_config(Path::new(file)) {
                Ok(config) => eprintln!(
                    "  OK   {file} -> {} ({} services)",
                    config.name,
                    config.services.len()
                ),
                Err(error) => eprintln!("  ERR  {file} -> {error}"),
            }
        }
        assert!(
            outcome.warning.is_none(),
            "the scan should stay under SCAN_WARN_MS on a normal $HOME"
        );
    }
}
