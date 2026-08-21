//! `port_plan` module — turning a port collision into a concrete, reviewable
//! list of edits.
//!
//! [`crate::ui::ports`] answers *« quel port pose problème ? »*. This module
//! answers the next question — *« et on met quoi à la place ? »* — by picking
//! a free replacement for every compose declaration that cannot keep the port
//! it asks for.
//!
//! # What it will and will not touch
//!
//! Only a **compose declaration** can be reassigned, because only a compose
//! declaration is written down somewhere this program could edit. A container
//! created by a hand-rolled `docker run` has no `ports:` line, so a collision
//! it takes part in comes back as a [`Blocked`] entry rather than a silent
//! omission: the user has to fix that one themselves, and needs to be told.
//!
//! # Three sources of occupancy, not one
//!
//! A replacement port is only useful if it is free for *everybody*, so the
//! candidate set excludes, all at once:
//!
//! 1. every host port a Docker owner holds or declares ([`PortOwner`]),
//! 2. every port the host itself is listening on ([`ListeningPort`] — this is
//!    what the `netstat` / `ss` scan was built for),
//! 3. every port already declared by another compose file, and every port
//!    handed out earlier **in this same plan** — otherwise a plan moving two
//!    services off 8080 would move them both onto 8081.
//!
//! # Nothing here writes anything
//!
//! The module is pure and unit-tested on any OS. Applying a plan — reading a
//! compose file, rewriting one number, writing it back — is
//! [`crate::docker::compose_edit`]'s job, and is gated behind a confirmation
//! dialog because it modifies files the user owns.

use std::collections::{BTreeMap, BTreeSet};

use crate::net::ListeningPort;
use crate::ui::ports::{self, OwnerKind, PortConflict, PortOwner};

/// Lowest port a replacement is ever picked from.
///
/// Below 1024 a bind needs root on Linux — the daemon has it, so the container
/// would start, but proposing 81 for a service that asked for 80 quietly makes
/// the stack root-dependent. Anything the user deliberately published low
/// keeps its number unless it collides; this floor only bounds the *search*.
pub const SEARCH_FLOOR: u16 = 1024;

/// Highest port a replacement is ever picked from.
///
/// 49152 is where the IANA — and Windows' default `netsh int ipv4 show
/// dynamicport tcp` — ephemeral range starts. A port taken from it is free
/// right now and gone at the next outbound connection, which is the single
/// nastiest intermittent failure this feature could manufacture.
pub const SEARCH_CEILING: u16 = 49151;

/// One published port, as a compose file declares it.
///
/// `file` + `service` is the identity of the `ports:` line — the same pair
/// [`PortOwner::declaration`] carries — so two entries sharing it are one
/// declaration seen twice, never two candidates to separate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeclaredPort {
    /// Absolute path of the compose file, as the Stacks list holds it.
    pub file: String,
    pub service: String,
    pub host_port: u16,
    pub container_port: u16,
    /// Lower-case, matching [`ports::PortBinding::protocol`].
    pub protocol: String,
}

impl DeclaredPort {
    /// `(file, service)`, the pair that identifies the `ports:` line.
    fn identity(&self) -> (&str, &str) {
        (self.file.as_str(), self.service.as_str())
    }
}

/// Why one declaration has to give its port up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveReason {
    /// Another compose declaration wants the same port. Holds a display form
    /// of the declaration that keeps it.
    TakenByStack(String),
    /// Something outside Docker is already listening on it. Holds the owner
    /// label the scan reported, or a fallback when the process is unknown.
    TakenByHost(String),
}

impl MoveReason {
    /// One-line justification, shown next to the row it explains.
    pub fn text(&self) -> String {
        match self {
            MoveReason::TakenByStack(other) => format!("port déjà déclaré par {other}"),
            MoveReason::TakenByHost(process) => format!("port déjà tenu par {process}"),
        }
    }
}

/// One edit the plan proposes: this file's this service moves this port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMove {
    pub file: String,
    pub service: String,
    pub protocol: String,
    pub from: u16,
    pub to: u16,
    pub reason: MoveReason,
}

/// A collision the plan deliberately does **not** fix, and why.
///
/// Listed rather than dropped: a plan that silently ignored half the problem
/// would read as "tout est réglé" while the next `up` still fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub host_port: u16,
    pub protocol: String,
    /// Who is involved, in display form.
    pub owners: Vec<String>,
    pub reason: String,
}

/// The whole proposal: what to change, and what is left for the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReassignmentPlan {
    pub moves: Vec<PortMove>,
    pub blocked: Vec<Blocked>,
}

impl ReassignmentPlan {
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty() && self.blocked.is_empty()
    }

    /// The distinct compose files a plan would rewrite.
    pub fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.moves.iter().map(|entry| entry.file.clone()).collect();
        files.sort();
        files.dedup();
        files
    }
}

/// `true` when `declaration` — a [`PortOwner::declaration`] key — names this
/// compose file and service.
///
/// The key's left half is `compose_files.join(",")`, so a project started with
/// several `-f` layers carries a comma-separated list there while a Stacks row
/// only ever holds one path: membership, not equality.
fn declaration_matches(declaration: &str, file: &str, service: &str) -> bool {
    let Some((files, declared_service)) = declaration.split_once('\u{1f}') else {
        return false;
    };
    declared_service == service && files.split(',').any(|candidate| candidate == file)
}

/// `true` when a container built from this declaration is up right now.
///
/// A live declaration keeps its port: moving it means recreating a stack that
/// currently works, to spare one that does not start anyway.
fn is_live(owners: &[PortOwner], declaration: &DeclaredPort) -> bool {
    owners.iter().any(|owner| {
        owner.kind == OwnerKind::RunningContainer
            && owner.declaration.as_deref().is_some_and(|key| {
                declaration_matches(key, &declaration.file, &declaration.service)
            })
    })
}

/// The first port `protocol` is free on, searching upward from `from` and
/// wrapping back to [`SEARCH_FLOOR`] rather than giving up near the ceiling.
///
/// Upward from the original on purpose: 8080 → 8081 keeps a mental link with
/// what the service used to be reachable on, which a random free port would
/// not.
fn next_free_port(from: u16, protocol: &str, taken: &BTreeSet<(u16, String)>) -> Option<u16> {
    let start = from.saturating_add(1).max(SEARCH_FLOOR);
    (start..=SEARCH_CEILING)
        .chain(SEARCH_FLOOR..start)
        .find(|candidate| !taken.contains(&(*candidate, protocol.to_string())))
}

/// The label to blame in a [`MoveReason::TakenByHost`].
fn host_holder(listeners: &[ListeningPort], port: u16, protocol: &str) -> Option<String> {
    listeners
        .iter()
        .find(|listener| listener.port == port && listener.protocol == protocol)
        .map(|listener| {
            listener
                .owner_label()
                .unwrap_or_else(|| "un processus de l'hôte".to_string())
        })
}

/// Build the reassignment proposal.
///
/// `declarations` must cover **every** discovered compose file, running or
/// not: a running stack's declaration is what wins the port it holds, so
/// leaving it out would move the wrong side of the collision.
pub fn plan_reassignment(
    declarations: &[DeclaredPort],
    owners: &[PortOwner],
    listeners: &[ListeningPort],
) -> ReassignmentPlan {
    let mut taken: BTreeSet<(u16, String)> = BTreeSet::new();
    for owner in owners {
        for binding in &owner.bindings {
            taken.insert((binding.host_port, binding.protocol.clone()));
        }
    }
    for listener in listeners {
        taken.insert((listener.port, listener.protocol.clone()));
    }
    for declaration in declarations {
        taken.insert((declaration.host_port, declaration.protocol.clone()));
    }

    // (port, protocol) → the declarations asking for it, one per `ports:`
    // line. `BTreeMap` plus a sorted, deduplicated group make the output
    // stable: the same machine state must always yield the same plan, or a
    // preview means nothing.
    let mut groups: BTreeMap<(u16, String), Vec<&DeclaredPort>> = BTreeMap::new();
    for declaration in declarations {
        let group = groups
            .entry((declaration.host_port, declaration.protocol.clone()))
            .or_default();
        if group
            .iter()
            .any(|other| other.identity() == declaration.identity())
        {
            continue;
        }
        group.push(declaration);
    }

    let mut plan = ReassignmentPlan::default();
    for ((host_port, protocol), mut group) in groups {
        // Live first, then by file and service: the stack that is up keeps its
        // port, and the tie-break is total so no frame reshuffles the plan.
        group.sort_by(|left, right| {
            is_live(owners, right)
                .cmp(&is_live(owners, left))
                .then_with(|| left.identity().cmp(&right.identity()))
        });

        let Some((keeper, movers)) = group.split_first() else {
            continue;
        };

        let mut reasons: Vec<(&DeclaredPort, MoveReason)> = movers
            .iter()
            .map(|mover| {
                (
                    *mover,
                    MoveReason::TakenByStack(format!("{} ({})", keeper.service, keeper.file)),
                )
            })
            .collect();

        // The keeper only moves when nothing of its own is up and the port is
        // held by something outside Docker: if it *is* up, the listener the
        // scan saw is its own published port, and reassigning it would chase
        // its own tail.
        if !is_live(owners, keeper) {
            if let Some(process) = host_holder(listeners, host_port, &protocol) {
                reasons.insert(0, (*keeper, MoveReason::TakenByHost(process)));
            }
        }

        for (mover, reason) in reasons {
            match next_free_port(host_port, &protocol, &taken) {
                Some(to) => {
                    taken.insert((to, protocol.clone()));
                    plan.moves.push(PortMove {
                        file: mover.file.clone(),
                        service: mover.service.clone(),
                        protocol: protocol.clone(),
                        from: host_port,
                        to,
                        reason,
                    });
                }
                None => plan.blocked.push(Blocked {
                    host_port,
                    protocol: protocol.clone(),
                    owners: vec![format!("{} ({})", mover.service, mover.file)],
                    reason: format!(
                        "aucun port {protocol} libre entre {SEARCH_FLOOR} et {SEARCH_CEILING}"
                    ),
                }),
            }
        }
    }

    plan.blocked
        .extend(unfixable_conflicts(declarations, owners));
    plan.blocked.sort_by(|left, right| {
        (left.host_port, &left.protocol).cmp(&(right.host_port, &right.protocol))
    });
    plan
}

/// Collisions no compose file can resolve: at least one side is a container
/// created outside compose, so there is no `ports:` line to rewrite.
fn unfixable_conflicts(declarations: &[DeclaredPort], owners: &[PortOwner]) -> Vec<Blocked> {
    ports::find_conflicts(owners)
        .into_iter()
        .filter(|conflict: &PortConflict| {
            // A conflict every participant of which is declared somewhere is
            // already covered by the grouping above.
            !conflict.owners.iter().all(|label| {
                owners
                    .iter()
                    .filter(|owner| &owner.label == label)
                    .any(|owner| {
                        owner.declaration.as_deref().is_some_and(|key| {
                            declarations.iter().any(|declaration| {
                                declaration_matches(key, &declaration.file, &declaration.service)
                            })
                        })
                    })
            })
        })
        .map(|conflict| Blocked {
            host_port: conflict.host_port,
            protocol: conflict.protocol,
            owners: conflict.owners,
            reason: "au moins un conteneur n'a pas de déclaration compose : à corriger à la main"
                .to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ports::PortBinding;

    fn declaration(file: &str, service: &str, host_port: u16) -> DeclaredPort {
        DeclaredPort {
            file: file.to_string(),
            service: service.to_string(),
            host_port,
            container_port: 80,
            protocol: "tcp".to_string(),
        }
    }

    fn binding(host_port: u16, protocol: &str) -> PortBinding {
        PortBinding {
            host_ip: "0.0.0.0".to_string(),
            host_port,
            container_port: 80,
            protocol: protocol.to_string(),
        }
    }

    fn running(label: &str, file: &str, service: &str, host_port: u16) -> PortOwner {
        PortOwner::new(
            label,
            label,
            OwnerKind::RunningContainer,
            vec![binding(host_port, "tcp")],
        )
        .with_declaration(format!("{file}\u{1f}{service}"))
    }

    fn listener(port: u16, protocol: &str, process: &str) -> ListeningPort {
        ListeningPort {
            port,
            protocol: protocol.to_string(),
            pid: Some(7),
            process: Some(process.to_string()),
        }
    }

    #[test]
    fn nothing_collides_nothing_moves() {
        let declarations = vec![
            declaration("/a.yml", "web", 8080),
            declaration("/b.yml", "api", 8081),
        ];
        let plan = plan_reassignment(&declarations, &[], &[]);
        assert!(plan.is_empty());
    }

    #[test]
    fn two_stacks_on_one_port_move_the_second_one() {
        let declarations = vec![
            declaration("/a.yml", "web", 8080),
            declaration("/b.yml", "web", 8080),
        ];
        let plan = plan_reassignment(&declarations, &[], &[]);
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].file, "/b.yml");
        assert_eq!(plan.moves[0].from, 8080);
        assert_eq!(plan.moves[0].to, 8081);
        assert!(matches!(plan.moves[0].reason, MoveReason::TakenByStack(_)));
    }

    /// The whole point of consulting the owners: the stack that is *up* keeps
    /// its port, whatever the alphabetical order says.
    #[test]
    fn the_running_stack_keeps_the_port() {
        let declarations = vec![
            declaration("/a.yml", "web", 8080),
            declaration("/b.yml", "web", 8080),
        ];
        let owners = vec![running("b-web-1", "/b.yml", "web", 8080)];
        let plan = plan_reassignment(&declarations, &owners, &[]);
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(
            plan.moves[0].file, "/a.yml",
            "the stopped one is the one that moves"
        );
    }

    /// A port no container fights over, but the host already holds — the case
    /// only the `netstat` / `ss` scan can see.
    #[test]
    fn a_host_listener_alone_is_enough_to_move_a_declaration() {
        let declarations = vec![declaration("/a.yml", "db", 5432)];
        let listeners = vec![listener(5432, "tcp", "postgres.exe (PID 7)")];
        let plan = plan_reassignment(&declarations, &[], &listeners);
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].to, 5433);
        assert!(matches!(plan.moves[0].reason, MoveReason::TakenByHost(_)));
    }

    /// Docker's own proxy holds a running container's port, so a scan always
    /// reports it. Treating that as a collision would propose moving every
    /// stack that currently works.
    #[test]
    fn a_running_declarations_own_listener_is_not_a_reason_to_move() {
        let declarations = vec![declaration("/a.yml", "web", 8080)];
        let owners = vec![running("a-web-1", "/a.yml", "web", 8080)];
        let listeners = vec![listener(8080, "tcp", "com.docker.backend.exe")];
        assert!(plan_reassignment(&declarations, &owners, &listeners).is_empty());
    }

    #[test]
    fn a_replacement_never_lands_on_something_else_that_is_taken() {
        let declarations = vec![
            declaration("/a.yml", "web", 8080),
            declaration("/b.yml", "web", 8080),
            declaration("/c.yml", "api", 8081),
        ];
        let listeners = vec![listener(8082, "tcp", "node.exe")];
        let plan = plan_reassignment(&declarations, &[], &listeners);
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].to, 8083, "8081 is declared, 8082 is held");
    }

    /// Three declarations on one port must land on three *different* ports.
    #[test]
    fn two_movers_do_not_land_on_the_same_replacement() {
        let declarations = vec![
            declaration("/a.yml", "web", 8080),
            declaration("/b.yml", "web", 8080),
            declaration("/c.yml", "web", 8080),
        ];
        let plan = plan_reassignment(&declarations, &[], &[]);
        assert_eq!(plan.moves.len(), 2);
        assert_ne!(plan.moves[0].to, plan.moves[1].to);
    }

    #[test]
    fn protocols_are_independent() {
        let mut udp = declaration("/a.yml", "dns", 53);
        udp.protocol = "udp".to_string();
        let declarations = vec![declaration("/a.yml", "web", 53), udp];
        assert!(plan_reassignment(&declarations, &[], &[]).is_empty());
    }

    /// Two containers created by hand fighting over a port: nothing to
    /// rewrite, so the plan says so rather than coming back empty.
    #[test]
    fn a_collision_with_no_compose_declaration_is_reported_as_blocked() {
        let owners = vec![
            PortOwner::new(
                "id-1",
                "solo-1",
                OwnerKind::RunningContainer,
                vec![binding(9000, "tcp")],
            ),
            PortOwner::new(
                "id-2",
                "solo-2",
                OwnerKind::StoppedContainer,
                vec![binding(9000, "tcp")],
            ),
        ];
        let plan = plan_reassignment(&[], &owners, &[]);
        assert!(plan.moves.is_empty());
        assert_eq!(plan.blocked.len(), 1);
        assert_eq!(plan.blocked[0].host_port, 9000);
        assert_eq!(plan.blocked[0].owners.len(), 2);
    }

    #[test]
    fn next_free_port_wraps_rather_than_giving_up_near_the_ceiling() {
        let taken = BTreeSet::new();
        assert_eq!(
            next_free_port(SEARCH_CEILING, "tcp", &taken),
            Some(SEARCH_FLOOR)
        );
    }

    #[test]
    fn next_free_port_never_proposes_a_privileged_port() {
        let taken = BTreeSet::new();
        let port = next_free_port(80, "tcp", &taken).expect("a port exists");
        assert!(port >= SEARCH_FLOOR, "{port} is below the floor");
    }

    #[test]
    fn declaration_matches_finds_a_service_inside_a_multi_file_project() {
        let key = "/a.yml,/a.override.yml\u{1f}web";
        assert!(declaration_matches(key, "/a.override.yml", "web"));
        assert!(!declaration_matches(key, "/a.override.yml", "api"));
        assert!(!declaration_matches(key, "/b.yml", "web"));
    }

    #[test]
    fn files_lists_each_touched_file_once() {
        let declarations = vec![
            declaration("/keep.yml", "web", 8080),
            declaration("/move.yml", "web", 8080),
            declaration("/move.yml", "api", 8080),
        ];
        let plan = plan_reassignment(&declarations, &[], &[]);
        assert_eq!(plan.files(), vec!["/move.yml".to_string()]);
    }
}
