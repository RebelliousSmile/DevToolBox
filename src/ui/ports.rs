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
//! module is unit-tested on any OS.

use std::collections::BTreeMap;

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
///
/// Part 1 only ever builds [`OwnerKind::RunningContainer`] owners;
/// [`OwnerKind::DeclaredStack`] exists so a compose file that is *not* running
/// yet can be compared against the live ones without changing this API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerKind {
    RunningContainer,
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
        }
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
