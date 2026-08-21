//! `ports` module — OS-neutral, dependency-free model of Docker's published
//! ports, plus host-port conflict detection.
//!
//! The single source of truth is the `Ports` field of
//! `docker ps -a --format '{{json .}}'`, whose real shape is captured in
//! `src/linux/docker.rs`'s `REAL_PS_FIXTURE`:
//! `"0.0.0.0:5656->5656/tcp, [::]:5656->5656/tcp"`. Docker publishes the same
//! binding twice — once on the IPv4 wildcard, once on the IPv6 one — which is
//! why [`PortOwner::new`] is the only way to build an owner: it collapses that
//! pair, and without it every container would conflict with itself.
//!
//! Nothing here touches the filesystem, a process or a clock, so the whole
//! module is unit-tested on any OS. [`crate::net`] is the one import, and only
//! for its [`ListeningPort`] data type — the scanning itself stays over there,
//! so the cross-referencing functions below stay as pure as the rest.

use std::collections::{BTreeMap, BTreeSet};

use crate::net::ListeningPort;

/// One published binding: a host interface/port mapped to a container port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortBinding {
    /// Host interface the port is published on. Empty when docker did not
    /// name one; `::` for the IPv6 wildcard (brackets are stripped on parse).
    pub host_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    /// `tcp`, `udp`, `sctp`… lowercased, defaulting to `tcp`.
    pub protocol: String,
}

/// What kind of owner holds a set of bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerKind {
    RunningContainer,
    /// A container that is not up. Its bindings are **historical**: contrary
    /// to what this module first assumed, `docker ps -a` keeps printing the
    /// ports of a stopped container's last run. That is harmless for a port
    /// the container declared (it will ask for the same one again), but for a
    /// dynamic port it is pure archaeology — see [`PortOwner::declared`].
    StoppedContainer,
    /// A compose service declaring a published port while nothing of its
    /// stack is up — built by `crate::ui::compose_view::declared_owners`.
    /// This is the variant the conflict badge exists for: two *running*
    /// containers can never share a host port, since the kernel refuses the
    /// second bind.
    DeclaredStack,
}

/// A container (or a declared stack service) holding published bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortOwner {
    /// Identity used to tell owners apart — two bindings sharing a key are the
    /// same owner and can never conflict with each other.
    pub key: String,
    /// Human-readable name shown in the conflict hover text.
    pub label: String,
    pub kind: OwnerKind,
    pub bindings: Vec<PortBinding>,
    /// `(host_port, protocol)` this owner **asked** for, as opposed to what
    /// docker happened to give it. `None` means "unknown, assume all of them
    /// are declared" — the conservative reading, and the one every owner built
    /// before this field existed gets for free.
    pub declared: Option<BTreeSet<(u16, String)>>,
    /// Where this owner's ports are written down: the compose file, or an
    /// empty string for a container created outside compose. Display-only —
    /// [`declaration`] is what identity comparisons use.
    ///
    /// [`declaration`]: PortOwner::declaration
    pub source: String,
    /// Identity of the *declaration* behind this owner (compose file +
    /// service), when there is one. Two owners sharing it are two instances of
    /// a single `ports:` line, so no port reassignment could ever separate
    /// them: what they need is for one of the two to be deleted, which is a
    /// duplicate-project problem, not a port problem.
    pub declaration: Option<String>,
}

impl PortOwner {
    /// The **only** constructor, because it applies the wildcard de-duplication
    /// described in the module docs. A free `dedupe` helper would be one
    /// forgotten call away from flagging every row as conflicting with itself.
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        kind: OwnerKind,
        bindings: Vec<PortBinding>,
    ) -> Self {
        PortOwner {
            key: key.into(),
            label: label.into(),
            kind,
            bindings: dedupe_bindings(bindings),
            declared: None,
            declaration: None,
            source: String::new(),
        }
    }

    /// Name the file this owner's ports come from.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Narrow this owner's bindings to the ones it explicitly requested.
    pub fn with_declared(mut self, declared: BTreeSet<(u16, String)>) -> Self {
        self.declared = Some(declared);
        self
    }

    /// Tag this owner with the declaration it instantiates.
    pub fn with_declaration(mut self, declaration: impl Into<String>) -> Self {
        let declaration = declaration.into();
        self.declaration = (!declaration.trim().is_empty()).then_some(declaration);
        self
    }

    /// `true` when this owner asked for `(host_port, protocol)` itself.
    ///
    /// An owner with no `declared` set answers `true` for everything: not
    /// knowing must not silently drop conflicts.
    pub fn declares(&self, host_port: u16, protocol: &str) -> bool {
        match &self.declared {
            Some(declared) => declared.contains(&(host_port, protocol.to_string())),
            None => true,
        }
    }

    /// `true` when this binding is worth comparing against the other owners.
    ///
    /// A stopped container's *dynamic* port is the only thing filtered out:
    /// the number `docker ps -a` prints is the one docker handed it on its
    /// last run, and the next run will pick another free port. Keeping it
    /// manufactures conflicts between containers that can never collide.
    fn claims(&self, host_port: u16, protocol: &str) -> bool {
        self.kind != OwnerKind::StoppedContainer || self.declares(host_port, protocol)
    }
}

/// A host port claimed by two or more distinct owners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortConflict {
    pub host_port: u16,
    pub protocol: String,
    /// Labels of every owner involved, in the order the owners were supplied.
    pub owners: Vec<String>,
    /// `true` only when every owner involved is a running container — i.e. the
    /// collision is happening *now*, as opposed to one that would happen if a
    /// declared stack were started.
    pub active: bool,
}

/// `true` when `ip` binds every interface: docker writes that as `0.0.0.0`,
/// `::` / `[::]`, `*`, or omits it entirely.
fn is_wildcard(ip: &str) -> bool {
    matches!(strip_brackets(ip), "" | "0.0.0.0" | "::" | "*")
}

fn strip_brackets(ip: &str) -> &str {
    ip.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(ip)
}

/// `true` when two host interfaces can collide: a wildcard on either side
/// covers everything, otherwise the interfaces must be identical.
///
/// `127.0.0.1:8080` and `192.168.1.10:8080` therefore do **not** conflict.
pub fn interfaces_overlap(a: &str, b: &str) -> bool {
    if is_wildcard(a) || is_wildcard(b) {
        return true;
    }
    strip_brackets(a) == strip_brackets(b)
}

/// Collapses the `0.0.0.0` / `[::]` double binding docker emits, and any exact
/// duplicate, keeping the first occurrence of each.
fn dedupe_bindings(bindings: Vec<PortBinding>) -> Vec<PortBinding> {
    let mut seen: Vec<(String, u16, u16, String)> = Vec::new();
    let mut kept = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let interface = if is_wildcard(&binding.host_ip) {
            "*".to_string()
        } else {
            strip_brackets(&binding.host_ip).to_string()
        };
        let key = (
            interface,
            binding.host_port,
            binding.container_port,
            binding.protocol.clone(),
        );
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        kept.push(binding);
    }
    kept
}

/// Parses one `start-end` range, or one bare port, into an inclusive pair.
fn parse_range(text: &str) -> Option<(u16, u16)> {
    match text.split_once('-') {
        Some((start, end)) => {
            let start = start.trim().parse::<u16>().ok()?;
            let end = end.trim().parse::<u16>().ok()?;
            if start > end {
                return None;
            }
            Some((start, end))
        }
        None => {
            let port = text.trim().parse::<u16>().ok()?;
            Some((port, port))
        }
    }
}

/// Parses a single `", "`-separated entry of the `Ports` field.
///
/// Returns an empty vector for anything that is not a *published* binding —
/// notably the bare `5656/tcp` form, which is an `EXPOSE` declaration docker
/// never bound to the host and which must therefore never feed conflict
/// detection.
fn parse_ps_entry(entry: &str) -> Vec<PortBinding> {
    let entry = entry.trim();
    let Some((left, right)) = entry.split_once("->") else {
        return Vec::new();
    };

    let (container_spec, protocol) = match right.trim().split_once('/') {
        Some((ports, proto)) => (ports, proto.trim().to_lowercase()),
        None => (right.trim(), "tcp".to_string()),
    };
    if protocol.is_empty() {
        return Vec::new();
    }

    // The host side is `ip:port`, `[ipv6]:port` or a bare `port`; the last
    // colon is the separator in every shape, brackets included.
    let left = left.trim();
    let (host_ip, host_spec) = match left.rsplit_once(':') {
        Some((ip, ports)) => (strip_brackets(ip.trim()).to_string(), ports),
        None => (String::new(), left),
    };

    let Some((host_start, host_end)) = parse_range(host_spec) else {
        return Vec::new();
    };
    let Some((container_start, container_end)) = parse_range(container_spec) else {
        return Vec::new();
    };
    // A range whose two ends disagree in width is dropped rather than guessed.
    if host_end - host_start != container_end - container_start {
        return Vec::new();
    }

    (0..=(host_end - host_start))
        .map(|offset| PortBinding {
            host_ip: host_ip.clone(),
            host_port: host_start + offset,
            container_port: container_start + offset,
            protocol: protocol.clone(),
        })
        .collect()
}

/// Parses the whole `Ports` field of `docker ps`. Unparsable entries are
/// skipped, never fatal — a malformed line must not cost the user the column.
pub fn parse_ps_ports(raw: &str) -> Vec<PortBinding> {
    raw.split(',').flat_map(parse_ps_entry).collect::<Vec<_>>()
}

/// Every host port claimed by two or more **distinct** owners on overlapping
/// interfaces, sorted by port then protocol so the output is stable.
pub fn find_conflicts(owners: &[PortOwner]) -> Vec<PortConflict> {
    // (host_port, protocol) → the owners claiming it, in input order.
    let mut claims: BTreeMap<(u16, String), Vec<(usize, &str)>> = BTreeMap::new();
    for (index, owner) in owners.iter().enumerate() {
        for binding in &owner.bindings {
            if !owner.claims(binding.host_port, &binding.protocol) {
                continue;
            }
            claims
                .entry((binding.host_port, binding.protocol.clone()))
                .or_default()
                .push((index, binding.host_ip.as_str()));
        }
    }

    let mut conflicts = Vec::new();
    for ((host_port, protocol), claimants) in claims {
        // Owner indices participating in at least one cross-owner overlap.
        let mut involved: Vec<usize> = Vec::new();
        for (i, (left_owner, left_ip)) in claimants.iter().enumerate() {
            for (right_owner, right_ip) in claimants.iter().skip(i + 1) {
                if owners[*left_owner].key == owners[*right_owner].key {
                    continue;
                }
                if same_declaration(&owners[*left_owner], &owners[*right_owner]) {
                    continue;
                }
                if !interfaces_overlap(left_ip, right_ip) {
                    continue;
                }
                for candidate in [*left_owner, *right_owner] {
                    if !involved.contains(&candidate) {
                        involved.push(candidate);
                    }
                }
            }
        }
        if involved.len() < 2 {
            continue;
        }
        involved.sort_unstable();
        conflicts.push(PortConflict {
            host_port,
            protocol,
            owners: involved
                .iter()
                .map(|index| owners[*index].label.clone())
                .collect(),
            active: involved
                .iter()
                .all(|index| owners[*index].kind == OwnerKind::RunningContainer),
        });
    }
    conflicts
}

/// The live listener holding this row's port, if the host was scanned and one
/// is.
///
/// A row and a listener match on `(port, protocol)` alone — the address is
/// deliberately ignored. A container publishing on `127.0.0.1:8080` and a
/// service listening on `0.0.0.0:8080` do *not* collide in the kernel's eyes,
/// but comparing them properly would mean modelling wildcard versus specific
/// binds on two OSes; over-reporting "occupé" costs the reader one look, while
/// under-reporting it costs them a container that will not start.
pub fn host_listener<'a>(
    listeners: &'a [ListeningPort],
    row: &PortAllocation,
) -> Option<&'a ListeningPort> {
    listeners
        .iter()
        .find(|listener| listener.port == row.host_port && listener.protocol == row.protocol)
}

/// The listeners no Docker owner explains — the other half of the picture.
///
/// This is what makes the table answer "is 8080 free?" rather than only "does
/// a container want 8080?": IIS, a hand-started `node`, a local Postgres. On
/// Windows a *running* container's port is held by Docker Desktop's proxy and
/// so does match an allocation row, which is why it does not show up here.
pub fn listeners_outside_docker<'a>(
    listeners: &'a [ListeningPort],
    allocations: &[PortAllocation],
) -> Vec<&'a ListeningPort> {
    listeners
        .iter()
        .filter(|listener| {
            !allocations
                .iter()
                .any(|row| row.host_port == listener.port && row.protocol == listener.protocol)
        })
        .collect()
}

/// One row of the host-port allocation table: a single owner's claim on a
/// single `(host_port, protocol)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortAllocation {
    pub host_port: u16,
    pub protocol: String,
    /// Owner label, as shown in the container/stack lists.
    pub owner: String,
    pub kind: OwnerKind,
    /// `true` when the owner asked for this exact host port, `false` when
    /// docker picked it at start-up. A dynamic row is informational only: the
    /// number will change at the next start, and [`find_conflicts`] ignores it
    /// on a stopped owner for exactly that reason.
    pub declared: bool,
    /// File the port is written in, empty when there is none.
    pub source: String,
    /// `true` when this `(port, protocol)` is one [`find_conflicts`] reported
    /// **and** this owner is named in it.
    pub conflicting: bool,
}

/// Flatten every owner's bindings into one table, sorted by port, then
/// protocol, then owner — a stable order, so the view never reshuffles
/// between two frames.
///
/// Unlike [`find_conflicts`] this keeps *everything*, dynamic ports of
/// stopped containers included: the table's job is to explain the machine's
/// port usage, and a row the detector deliberately ignored is precisely what
/// a reader needs to see to understand why no badge appeared.
pub fn port_allocations(owners: &[PortOwner], conflicts: &[PortConflict]) -> Vec<PortAllocation> {
    let mut rows: Vec<PortAllocation> = Vec::new();
    for owner in owners {
        for binding in &owner.bindings {
            let conflicting = conflicts.iter().any(|conflict| {
                conflict.host_port == binding.host_port
                    && conflict.protocol == binding.protocol
                    && conflict.owners.contains(&owner.label)
            });
            rows.push(PortAllocation {
                host_port: binding.host_port,
                protocol: binding.protocol.clone(),
                owner: owner.label.clone(),
                kind: owner.kind,
                declared: owner.declares(binding.host_port, &binding.protocol),
                source: owner.source.clone(),
                conflicting,
            });
        }
    }
    rows.sort_by(|left, right| {
        left.host_port
            .cmp(&right.host_port)
            .then_with(|| left.protocol.cmp(&right.protocol))
            .then_with(|| left.owner.cmp(&right.owner))
    });
    rows.dedup();
    rows
}

/// `true` when both owners instantiate the same compose file + service.
fn same_declaration(left: &PortOwner, right: &PortOwner) -> bool {
    match (&left.declaration, &right.declaration) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// The column's display form: `0.0.0.0:5656→5656/tcp`, joined by `", "`.
/// Empty string when nothing is published.
pub fn format_bindings(bindings: &[PortBinding]) -> String {
    // Deduplicated here too, not just in `PortOwner::new`: docker reports one
    // publish twice, once IPv4 and once IPv6 (`0.0.0.0:5656->…, [::]:5656->…`),
    // and printing both doubles the column width for no added meaning. The raw
    // `ContainerEntry.ports` stays faithful to what docker said; only the
    // rendering collapses the pair.
    dedupe_bindings(bindings.to_vec())
        .iter()
        .map(|binding| {
            let host = if binding.host_ip.is_empty() {
                String::new()
            } else if binding.host_ip.contains(':') {
                format!("[{}]:", binding.host_ip)
            } else {
                format!("{}:", binding.host_ip)
            };
            // `->`, not `→`: egui's default font has no U+2192 glyph, so the
            // arrow renders as a tofu box in the Ports column.
            format!(
                "{host}{}->{}/{}",
                binding.host_port, binding.container_port, binding.protocol
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from `REAL_PS_FIXTURE` in `src/linux/docker.rs`.
    const REAL_DOUBLE_BINDING: &str = "0.0.0.0:5656->5656/tcp, [::]:5656->5656/tcp";

    fn binding(host_ip: &str, host_port: u16, container_port: u16, protocol: &str) -> PortBinding {
        PortBinding {
            host_ip: host_ip.to_string(),
            host_port,
            container_port,
            protocol: protocol.to_string(),
        }
    }

    fn owner(key: &str, bindings: Vec<PortBinding>) -> PortOwner {
        PortOwner::new(key, key, OwnerKind::RunningContainer, bindings)
    }

    fn stopped(key: &str, bindings: Vec<PortBinding>) -> PortOwner {
        PortOwner::new(key, key, OwnerKind::StoppedContainer, bindings)
    }

    fn declared(ports: &[(u16, &str)]) -> BTreeSet<(u16, String)> {
        ports
            .iter()
            .map(|(port, proto)| (*port, proto.to_string()))
            .collect()
    }

    // --- dynamic ports of stopped containers ---------------------------------

    /// The bug this filter exists for, reproduced from the real machine: four
    /// stopped `mysql` containers, none declaring a host port, all printing
    /// `0.0.0.0:32768->3306/tcp` because that is what docker handed the last
    /// one to run. Docker will pick a free port for each on the next `up`.
    #[test]
    fn stopped_containers_sharing_a_dynamic_port_do_not_conflict() {
        let owners = vec![
            stopped(
                "mauceri-mysql-1",
                vec![binding("0.0.0.0", 32768, 3306, "tcp")],
            )
            .with_declared(BTreeSet::new()),
            stopped(
                "scriptami-mysql-1",
                vec![binding("0.0.0.0", 32768, 3306, "tcp")],
            )
            .with_declared(BTreeSet::new()),
        ];
        assert!(find_conflicts(&owners).is_empty());
    }

    /// The other half of the same rule: a stopped container that *asked* for
    /// its host port will ask again at the next start, so it still collides.
    #[test]
    fn stopped_containers_sharing_a_declared_port_still_conflict() {
        let owners = vec![
            stopped("app-db-1", vec![binding("0.0.0.0", 5432, 5432, "tcp")])
                .with_declared(declared(&[(5432, "tcp")])),
            stopped(
                "suddenly-review-pg",
                vec![binding("0.0.0.0", 5432, 5432, "tcp")],
            )
            .with_declared(declared(&[(5432, "tcp")])),
        ];
        let conflicts = find_conflicts(&owners);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].host_port, 5432);
        assert!(
            !conflicts[0].active,
            "neither container is running, so the collision is not happening now"
        );
    }

    /// A *running* container's dynamic port is bound right now — the kernel
    /// is holding it — so it is never filtered, whatever it declared.
    #[test]
    fn a_running_containers_dynamic_port_is_still_a_claim() {
        let owners = vec![
            owner("live", vec![binding("0.0.0.0", 32768, 3306, "tcp")])
                .with_declared(BTreeSet::new()),
            stopped("other", vec![binding("0.0.0.0", 32768, 3306, "tcp")])
                .with_declared(declared(&[(32768, "tcp")])),
        ];
        assert_eq!(find_conflicts(&owners).len(), 1);
    }

    #[test]
    fn an_owner_with_no_declared_set_claims_every_binding() {
        let owner = stopped("legacy", vec![binding("0.0.0.0", 8080, 80, "tcp")]);
        assert!(
            owner.declares(8080, "tcp"),
            "not knowing what was declared must never drop a conflict"
        );
    }

    // --- one declaration, two projects ---------------------------------------

    /// Measured: `.wp-env/525f87…/docker-compose.yml`'s `wordpress` service
    /// backs both `525f87…-wordpress-1` and `arbre-de-jade-code-wordpress-1`,
    /// which therefore publish the same 8514. Reassigning ports cannot help —
    /// there is a single `ports:` line — so this is not a port conflict.
    #[test]
    fn two_projects_from_one_declaration_do_not_conflict() {
        let file = "C:/Users/fxgui/.wp-env/525f87/docker-compose.yml\u{1f}wordpress";
        let owners = vec![
            owner("old", vec![binding("0.0.0.0", 8514, 80, "tcp")]).with_declaration(file),
            owner("new", vec![binding("0.0.0.0", 8514, 80, "tcp")]).with_declaration(file),
        ];
        assert!(find_conflicts(&owners).is_empty());
    }

    /// Two *different* services of the same file are two `ports:` lines, and
    /// a genuine conflict a port reassignment would fix.
    #[test]
    fn two_services_of_one_file_still_conflict() {
        let owners = vec![
            owner("web", vec![binding("0.0.0.0", 8080, 80, "tcp")])
                .with_declaration("compose.yml\u{1f}web"),
            owner("admin", vec![binding("0.0.0.0", 8080, 80, "tcp")])
                .with_declaration("compose.yml\u{1f}admin"),
        ];
        assert_eq!(find_conflicts(&owners).len(), 1);
    }

    /// An owner with no declaration (a hand-rolled `docker run`) conflicts
    /// with everything it collides with, including another one.
    #[test]
    fn undeclared_owners_are_never_treated_as_siblings() {
        let owners = vec![
            owner("a", vec![binding("0.0.0.0", 5432, 5432, "tcp")]),
            owner("b", vec![binding("0.0.0.0", 5432, 5432, "tcp")]),
        ];
        assert_eq!(find_conflicts(&owners).len(), 1);
    }

    // --- host listeners ------------------------------------------------------

    fn listener(port: u16, protocol: &str, process: &str) -> ListeningPort {
        ListeningPort {
            port,
            protocol: protocol.to_string(),
            pid: Some(1),
            process: Some(process.to_string()),
        }
    }

    #[test]
    fn host_listener_matches_on_port_and_protocol() {
        let listeners = vec![
            listener(80, "tcp", "httpd.exe"),
            listener(53, "udp", "svchost.exe"),
        ];
        let rows = port_allocations(
            &[owner("web", vec![binding("0.0.0.0", 80, 80, "tcp")])],
            &[],
        );
        assert_eq!(
            host_listener(&listeners, &rows[0]).and_then(|l| l.process.clone()),
            Some("httpd.exe".to_string())
        );
    }

    #[test]
    fn host_listener_does_not_cross_protocols() {
        let listeners = vec![listener(53, "udp", "svchost.exe")];
        let rows = port_allocations(
            &[owner("dns", vec![binding("0.0.0.0", 53, 53, "tcp")])],
            &[],
        );
        assert!(host_listener(&listeners, &rows[0]).is_none());
    }

    /// The case the whole scan exists for: a stopped container declares 5432
    /// while a local Postgres already holds it. Nothing in the Docker data
    /// says so — the container simply fails at the next `up`.
    #[test]
    fn host_listener_catches_a_stopped_owners_taken_port() {
        let listeners = vec![listener(5432, "tcp", "postgres.exe")];
        let owners = vec![
            stopped("app-db-1", vec![binding("0.0.0.0", 5432, 5432, "tcp")])
                .with_declared(declared(&[(5432, "tcp")])),
        ];
        let rows = port_allocations(&owners, &find_conflicts(&owners));
        assert!(
            !rows[0].conflicting,
            "one owner alone is no Docker conflict"
        );
        assert!(
            host_listener(&listeners, &rows[0]).is_some(),
            "yet the port is taken, which only the host scan can tell"
        );
    }

    #[test]
    fn listeners_outside_docker_drops_the_ones_a_row_explains() {
        let listeners = vec![
            listener(80, "tcp", "httpd.exe"),
            listener(8080, "tcp", "com.docker.backend.exe"),
        ];
        let rows = port_allocations(
            &[owner("web", vec![binding("0.0.0.0", 8080, 80, "tcp")])],
            &[],
        );
        let outside = listeners_outside_docker(&listeners, &rows);
        assert_eq!(outside.len(), 1);
        assert_eq!(outside[0].port, 80);
    }

    #[test]
    fn listeners_outside_docker_keeps_everything_when_nothing_is_published() {
        let listeners = vec![listener(80, "tcp", "httpd.exe")];
        assert_eq!(listeners_outside_docker(&listeners, &[]).len(), 1);
    }

    // --- port_allocations ----------------------------------------------------

    #[test]
    fn port_allocations_sorts_by_port_then_protocol_then_owner() {
        let owners = vec![
            owner("zeta", vec![binding("0.0.0.0", 8080, 80, "tcp")]),
            owner("alpha", vec![binding("0.0.0.0", 8080, 80, "tcp")]),
            owner("udp-one", vec![binding("0.0.0.0", 53, 53, "udp")]),
            owner("tcp-one", vec![binding("0.0.0.0", 53, 53, "tcp")]),
        ];
        let rows = port_allocations(&owners, &[]);
        let order: Vec<(u16, &str, &str)> = rows
            .iter()
            .map(|row| (row.host_port, row.protocol.as_str(), row.owner.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![
                (53, "tcp", "tcp-one"),
                (53, "udp", "udp-one"),
                (8080, "tcp", "alpha"),
                (8080, "tcp", "zeta"),
            ]
        );
    }

    /// The table keeps what the detector drops — otherwise the four stopped
    /// `mysql` rows would vanish with no explanation of where 32768 went.
    #[test]
    fn port_allocations_keeps_the_dynamic_rows_find_conflicts_ignores() {
        let owners = vec![
            stopped(
                "mauceri-mysql-1",
                vec![binding("0.0.0.0", 32768, 3306, "tcp")],
            )
            .with_declared(BTreeSet::new()),
            stopped(
                "scriptami-mysql-1",
                vec![binding("0.0.0.0", 32768, 3306, "tcp")],
            )
            .with_declared(BTreeSet::new()),
        ];
        let conflicts = find_conflicts(&owners);
        assert!(conflicts.is_empty());

        let rows = port_allocations(&owners, &conflicts);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| !row.declared));
        assert!(rows.iter().all(|row| !row.conflicting));
    }

    #[test]
    fn port_allocations_flags_only_the_owners_named_in_a_conflict() {
        let owners = vec![
            owner("a", vec![binding("0.0.0.0", 5432, 5432, "tcp")]),
            owner("b", vec![binding("0.0.0.0", 5432, 5432, "tcp")]),
            owner("c", vec![binding("0.0.0.0", 6379, 6379, "tcp")]),
        ];
        let conflicts = find_conflicts(&owners);
        let rows = port_allocations(&owners, &conflicts);
        let flagged: Vec<&str> = rows
            .iter()
            .filter(|row| row.conflicting)
            .map(|row| row.owner.as_str())
            .collect();
        assert_eq!(flagged, vec!["a", "b"]);
    }

    /// `PortOwner::new` already collapses the IPv4/IPv6 pair docker publishes,
    /// so one publish is one row — not two.
    #[test]
    fn port_allocations_yields_one_row_per_publish() {
        let owners = vec![owner("web", parse_ps_ports(REAL_DOUBLE_BINDING))];
        assert_eq!(port_allocations(&owners, &[]).len(), 1);
    }

    #[test]
    fn port_allocations_carries_the_source_file() {
        let owners = vec![owner("web", vec![binding("0.0.0.0", 80, 80, "tcp")])
            .with_source("/srv/app/docker-compose.yml")];
        assert_eq!(
            port_allocations(&owners, &[])[0].source,
            "/srv/app/docker-compose.yml"
        );
    }

    #[test]
    fn with_declaration_ignores_a_blank_value() {
        let owner = owner("a", vec![binding("0.0.0.0", 80, 80, "tcp")]).with_declaration("   ");
        assert_eq!(owner.declaration, None);
    }

    #[test]
    fn parse_ps_ports_reads_the_real_double_binding() {
        assert_eq!(
            parse_ps_ports(REAL_DOUBLE_BINDING),
            vec![
                binding("0.0.0.0", 5656, 5656, "tcp"),
                binding("::", 5656, 5656, "tcp"),
            ]
        );
    }

    #[test]
    fn parse_ps_ports_skips_expose_only_entries() {
        assert!(parse_ps_ports("5656/tcp").is_empty());
        assert_eq!(
            parse_ps_ports("5656/tcp, 0.0.0.0:8080->80/tcp"),
            vec![binding("0.0.0.0", 8080, 80, "tcp")]
        );
    }

    #[test]
    fn parse_ps_ports_expands_a_range() {
        assert_eq!(
            parse_ps_ports("0.0.0.0:8000-8002->8000-8002/tcp"),
            vec![
                binding("0.0.0.0", 8000, 8000, "tcp"),
                binding("0.0.0.0", 8001, 8001, "tcp"),
                binding("0.0.0.0", 8002, 8002, "tcp"),
            ]
        );
    }

    #[test]
    fn parse_ps_ports_drops_a_range_whose_ends_disagree() {
        assert!(parse_ps_ports("0.0.0.0:8000-8002->9000-9005/tcp").is_empty());
    }

    #[test]
    fn parse_ps_ports_skips_garbage_without_panicking() {
        assert!(parse_ps_ports("").is_empty());
        assert!(parse_ps_ports("nonsense").is_empty());
        assert!(parse_ps_ports("0.0.0.0:70000->80/tcp").is_empty());
        assert!(parse_ps_ports("0.0.0.0:->80/tcp").is_empty());
        assert_eq!(
            parse_ps_ports("garbage->, 0.0.0.0:53->53/udp"),
            vec![binding("0.0.0.0", 53, 53, "udp")]
        );
    }

    #[test]
    fn parse_ps_ports_defaults_the_protocol_to_tcp() {
        assert_eq!(
            parse_ps_ports("127.0.0.1:8080->80"),
            vec![binding("127.0.0.1", 8080, 80, "tcp")]
        );
    }

    #[test]
    fn interfaces_overlap_truth_table() {
        assert!(interfaces_overlap("0.0.0.0", "127.0.0.1"));
        assert!(interfaces_overlap("127.0.0.1", "0.0.0.0"));
        assert!(interfaces_overlap("", "192.168.1.10"));
        assert!(interfaces_overlap("[::]", "192.168.1.10"));
        assert!(interfaces_overlap("::", "0.0.0.0"));
        assert!(interfaces_overlap("127.0.0.1", "127.0.0.1"));
        assert!(!interfaces_overlap("127.0.0.1", "192.168.1.10"));
    }

    #[test]
    fn port_owner_collapses_the_wildcard_double_binding() {
        let owner = owner("abc", parse_ps_ports(REAL_DOUBLE_BINDING));
        assert_eq!(owner.bindings, vec![binding("0.0.0.0", 5656, 5656, "tcp")]);
    }

    #[test]
    fn a_container_never_conflicts_with_itself() {
        let owners = vec![owner("abc", parse_ps_ports(REAL_DOUBLE_BINDING))];
        assert!(find_conflicts(&owners).is_empty());
    }

    #[test]
    fn the_same_owner_key_twice_is_not_a_conflict() {
        let owners = vec![
            owner("abc", parse_ps_ports("0.0.0.0:5656->5656/tcp")),
            owner("abc", parse_ps_ports("0.0.0.0:5656->5656/tcp")),
        ];
        assert!(find_conflicts(&owners).is_empty());
    }

    #[test]
    fn two_running_containers_on_the_same_port_conflict() {
        let owners = vec![
            PortOwner::new(
                "abc",
                "lab",
                OwnerKind::RunningContainer,
                parse_ps_ports(REAL_DOUBLE_BINDING),
            ),
            PortOwner::new(
                "def",
                "tasks",
                OwnerKind::RunningContainer,
                parse_ps_ports("0.0.0.0:5656->3000/tcp"),
            ),
        ];
        let conflicts = find_conflicts(&owners);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].host_port, 5656);
        assert_eq!(conflicts[0].protocol, "tcp");
        assert_eq!(
            conflicts[0].owners,
            vec!["lab".to_string(), "tasks".to_string()]
        );
        assert!(conflicts[0].active);
    }

    #[test]
    fn a_declared_stack_makes_the_conflict_inactive() {
        let owners = vec![
            PortOwner::new(
                "abc",
                "lab",
                OwnerKind::RunningContainer,
                parse_ps_ports("0.0.0.0:5656->5656/tcp"),
            ),
            PortOwner::new(
                "stack",
                "tasks (déclaré)",
                OwnerKind::DeclaredStack,
                parse_ps_ports("0.0.0.0:5656->5656/tcp"),
            ),
        ];
        let conflicts = find_conflicts(&owners);
        assert_eq!(conflicts.len(), 1);
        assert!(!conflicts[0].active);
    }

    #[test]
    fn the_same_port_on_different_protocols_is_not_a_conflict() {
        let owners = vec![
            owner("abc", parse_ps_ports("0.0.0.0:53->53/tcp")),
            owner("def", parse_ps_ports("0.0.0.0:53->53/udp")),
        ];
        assert!(find_conflicts(&owners).is_empty());
    }

    #[test]
    fn the_same_port_on_disjoint_interfaces_is_not_a_conflict() {
        let owners = vec![
            owner("abc", parse_ps_ports("127.0.0.1:8080->80/tcp")),
            owner("def", parse_ps_ports("192.168.1.10:8080->80/tcp")),
        ];
        assert!(find_conflicts(&owners).is_empty());
    }

    #[test]
    fn a_third_owner_on_a_disjoint_interface_stays_out_of_the_conflict() {
        let owners = vec![
            PortOwner::new(
                "a",
                "a",
                OwnerKind::RunningContainer,
                parse_ps_ports("127.0.0.1:8080->80/tcp"),
            ),
            PortOwner::new(
                "b",
                "b",
                OwnerKind::RunningContainer,
                parse_ps_ports("127.0.0.1:8080->80/tcp"),
            ),
            PortOwner::new(
                "c",
                "c",
                OwnerKind::RunningContainer,
                parse_ps_ports("192.168.1.10:8080->80/tcp"),
            ),
        ];
        let conflicts = find_conflicts(&owners);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].owners, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn conflicts_are_sorted_by_port() {
        let owners = vec![
            owner(
                "a",
                parse_ps_ports("0.0.0.0:9000->80/tcp, 0.0.0.0:80->80/tcp"),
            ),
            owner(
                "b",
                parse_ps_ports("0.0.0.0:9000->80/tcp, 0.0.0.0:80->80/tcp"),
            ),
        ];
        let ports: Vec<u16> = find_conflicts(&owners)
            .iter()
            .map(|conflict| conflict.host_port)
            .collect();
        assert_eq!(ports, vec![80, 9000]);
    }

    #[test]
    fn format_bindings_renders_the_column_form() {
        assert_eq!(format_bindings(&[]), "");
        assert_eq!(
            format_bindings(&parse_ps_ports(REAL_DOUBLE_BINDING)),
            "0.0.0.0:5656->5656/tcp",
            "the IPv4 and IPv6 halves of one publish collapse to a single entry"
        );
        assert_eq!(
            format_bindings(&[binding("", 8080, 80, "tcp")]),
            "8080->80/tcp"
        );
        assert_eq!(
            format_bindings(&[
                binding("127.0.0.1", 8080, 80, "tcp"),
                binding("192.168.1.5", 8080, 80, "tcp"),
            ]),
            "127.0.0.1:8080->80/tcp, 192.168.1.5:8080->80/tcp",
            "two distinct interfaces are two real publishes, never collapsed"
        );
    }
}
