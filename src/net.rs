//! Which host ports are held **right now**, Docker or not.
//!
//! `crate::ui::ports` answers "which container asked for which port". That is
//! only half of what picking a free port needs: a port can also be held by
//! something Docker knows nothing about — IIS on 80, a hand-started `node` on
//! 3000, a local Postgres on 5432 — and a compose file that declares it will
//! simply fail to come up.
//!
//! # Two tools, one shape
//!
//! There is no portable API for this, so each OS gets its own command and its
//! own parser, both of them pure functions over the captured text:
//!
//! - Windows — `netstat -ano`, then `tasklist /FO CSV /NH` to turn the PIDs
//!   into names. Two calls because `netstat` reports no name at all.
//! - Linux — `ss -lntuHp`, which reports the name inline (when the user may
//!   see it) and needs no second call.
//!
//! Only [`scan`] is `#[cfg]`-gated. Both parsers compile and are tested on
//! every OS, against real captured output — a Windows-only parser could not be
//! covered by the Linux CI, and vice versa.
//!
//! # What "listening" means here
//!
//! Not the state word. `netstat`'s `LISTENING` is English on the French
//! Windows this was measured on, but relying on that is one locale away from a
//! silently empty table. The locale-proof shape is used instead: a TCP row is
//! listening when its *remote* endpoint has port 0 (`0.0.0.0:0`, `[::]:0`),
//! which is exactly what a socket with no peer prints. UDP is connectionless
//! and has no state to read, so every UDP row is a bound port.
//!
//! # Best effort, never fatal
//!
//! Every failure degrades to "unknown", never to a wrong answer: a missing
//! tool, a killed child, an unparsable line and an unresolvable PID all just
//! remove information. The one thing this module must never do is claim a port
//! is free when it is not, which is why an unreadable line is dropped rather
//! than guessed at.

use std::collections::HashMap;
use std::time::Duration;

use crate::command_runner::{run_capturing, RunError};

/// Budget for one scan. `netstat` and `ss` answer in well under a second on a
/// healthy machine; this runs on the UI thread, so the point of the deadline
/// is to bound a pathological case, not to be a realistic duration.
const SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// One host port currently bound by some process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningPort {
    pub port: u16,
    /// Lower-case `"tcp"` or `"udp"`, matching `PortBinding::protocol`.
    pub protocol: String,
    /// Owning process id, `None` when the OS would not say — `ss` without the
    /// privileges to look, or a `netstat` line whose PID column is missing.
    pub pid: Option<u32>,
    /// Owning executable, `None` for the same reasons plus a `tasklist` that
    /// no longer lists the PID (the process exited between the two calls).
    pub process: Option<String>,
}

impl ListeningPort {
    /// How the owner is worth showing: the name, the bare PID when only that
    /// is known, and `None` when neither is.
    pub fn owner_label(&self) -> Option<String> {
        match (&self.process, self.pid) {
            (Some(name), Some(pid)) => Some(format!("{name} (PID {pid})")),
            (Some(name), None) => Some(name.clone()),
            (None, Some(pid)) => Some(format!("PID {pid}")),
            (None, None) => None,
        }
    }
}

/// Every port bound on this host, sorted by port then protocol.
///
/// `Err` means the scan could not run at all (the tool is missing, or it
/// overran its deadline). An empty `Ok` means it ran and found nothing —
/// possible in a locked-down container, and a very different statement.
#[cfg(windows)]
pub fn scan() -> Result<Vec<ListeningPort>, String> {
    let raw = capture("netstat", &["-ano"])?;
    let mut ports = parse_netstat(&raw);
    // Names are a bonus: a `tasklist` that fails leaves every row with its
    // PID, which is still enough to identify the culprit in the task manager.
    if let Ok(listing) = capture("tasklist", &["/FO", "CSV", "/NH"]) {
        let names = parse_tasklist(&listing);
        for port in &mut ports {
            port.process = port.pid.and_then(|pid| names.get(&pid).cloned());
        }
    }
    Ok(sorted(ports))
}

/// See [`scan`] (Windows variant); this is the Linux variant.
///
/// `-l` listening, `-n` numeric (no DNS round-trip), `-t`/`-u` both
/// protocols, `-H` no header, `-p` the owning process. `-p` is not an error
/// without privileges: `ss` simply omits the column, and the rows come back
/// with `None` owners.
#[cfg(not(windows))]
pub fn scan() -> Result<Vec<ListeningPort>, String> {
    let raw = capture("ss", &["-lntuHp"])?;
    Ok(sorted(parse_ss(&raw)))
}

/// Run one tool, mapping both failure modes to a message a user can act on.
fn capture(program: &str, args: &[&str]) -> Result<String, String> {
    match run_capturing(program, args, SCAN_TIMEOUT) {
        // A non-zero exit still gets its stdout read: `netstat` can complain
        // about one adapter and list every other one correctly.
        Ok(output) => Ok(output.stdout),
        Err(RunError::SpawnFailed) => Err(format!("{program} introuvable sur cette machine.")),
        Err(RunError::TimedOut(budget)) => Err(format!("{program} n'a pas répondu en {budget:?}.")),
    }
}

fn sorted(mut ports: Vec<ListeningPort>) -> Vec<ListeningPort> {
    ports.sort_by(|left, right| {
        left.port
            .cmp(&right.port)
            .then_with(|| left.protocol.cmp(&right.protocol))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    ports.dedup();
    ports
}

/// Parse `netstat -ano`.
///
/// ```text
///   Proto  Adresse locale         Adresse distante       État
///   TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       1960
///   TCP    [::]:445               [::]:0                 LISTENING       4
///   UDP    0.0.0.0:5353           *:*                                    2345
/// ```
///
/// Note the header is localised and, on the machine this was captured from,
/// short one column — which is exactly why the parser reads by position from
/// the *left* (proto, local, remote) and takes the PID as the last field,
/// never by counting columns against a header it cannot trust.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_netstat(raw: &str) -> Vec<ListeningPort> {
    let mut ports = Vec::new();
    for line in raw.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // proto + local + remote is the minimum; UDP has no state column, TCP
        // has one, and both may or may not be followed by the PID.
        if fields.len() < 3 {
            continue;
        }
        let protocol = match fields[0].to_ascii_lowercase().as_str() {
            "tcp" => "tcp",
            "udp" => "udp",
            // The header line and the blank lines land here, as does anything
            // a future Windows adds.
            _ => continue,
        };
        let Some(port) = endpoint_port(fields[1]) else {
            continue;
        };
        if protocol == "tcp" && endpoint_port(fields[2]) != Some(0) {
            // An established or waiting connection, not a listener.
            continue;
        }
        ports.push(ListeningPort {
            port,
            protocol: protocol.to_string(),
            pid: fields.last().and_then(|field| field.parse().ok()),
            process: None,
        });
    }
    ports
}

/// Parse `tasklist /FO CSV /NH` into a PID → image-name map.
///
/// ```text
/// "System Idle Process","0","Services","0","8 Ko"
/// "chrome.exe","14288","Console","1","236 812 Ko"
/// ```
///
/// A hand-rolled reader rather than a CSV crate: the two fields needed are the
/// first two, neither can contain a comma (a Windows image name cannot), and
/// adding a dependency for that would be the larger cost.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_tasklist(raw: &str) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    for line in raw.lines() {
        let mut fields = line
            .split("\",\"")
            .map(|field| field.trim_matches('"').trim());
        let (Some(name), Some(pid)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let Ok(pid) = pid.parse::<u32>() {
            if !name.is_empty() {
                names.insert(pid, name.to_string());
            }
        }
    }
    names
}

/// Parse `ss -lntuHp`.
///
/// ```text
/// udp UNCONN 0 0    127.0.0.53%lo:53  0.0.0.0:* users:(("systemd-resolve",pid=165,fd=14))
/// udp UNCONN 0 0        [::1]:323        [::]:*
/// tcp LISTEN 0 1000 10.255.255.254:53  0.0.0.0:*
/// ```
///
/// `-l` already filters to listeners, so unlike [`parse_netstat`] there is no
/// state to second-guess. The local address may carry an interface scope
/// (`%lo`) or be bracketed IPv6 — both handled by [`endpoint_port`], which
/// only ever looks at what follows the last colon.
#[cfg_attr(windows, allow(dead_code))]
fn parse_ss(raw: &str) -> Vec<ListeningPort> {
    let mut ports = Vec::new();
    for line in raw.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // netid, state, recv-q, send-q, local, peer.
        if fields.len() < 6 {
            continue;
        }
        let protocol = match fields[0].to_ascii_lowercase().as_str() {
            "tcp" => "tcp",
            "udp" => "udp",
            _ => continue,
        };
        let Some(port) = endpoint_port(fields[4]) else {
            continue;
        };
        let users = fields.get(6).copied().unwrap_or_default();
        ports.push(ListeningPort {
            port,
            protocol: protocol.to_string(),
            pid: ss_pid(users),
            process: ss_process(users),
        });
    }
    ports
}

/// `users:(("systemd-resolve",pid=165,fd=14))` → `165`.
#[cfg_attr(windows, allow(dead_code))]
fn ss_pid(users: &str) -> Option<u32> {
    users
        .split("pid=")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// `users:(("systemd-resolve",pid=165,fd=14))` → `systemd-resolve`.
#[cfg_attr(windows, allow(dead_code))]
fn ss_process(users: &str) -> Option<String> {
    let name = users.split('"').nth(1)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The port of an endpoint, whatever shape the address took.
///
/// `0.0.0.0:80`, `[::]:445`, `127.0.0.53%lo:53` and `*:*` all differ in their
/// address half and agree on their port half, so the split is on the **last**
/// colon: an IPv6 address is full of the others.
fn endpoint_port(endpoint: &str) -> Option<u16> {
    endpoint.rsplit_once(':')?.1.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured verbatim from `netstat -ano` on the French Windows 11 this was
    /// developed on — header included, so the parser is proven to skip a
    /// localised one rather than trip on it.
    const REAL_NETSTAT: &str = "\r
Connexions actives\r
\r
  Proto  Adresse locale         Adresse distante       État\r
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       1960\r
  TCP    0.0.0.0:445            0.0.0.0:0              LISTENING       4\r
  TCP    127.0.0.1:5432         0.0.0.0:0              LISTENING       7112\r
  TCP    192.168.1.24:52310     140.82.121.6:443       ESTABLISHED     14288\r
  TCP    [::]:445               [::]:0                 LISTENING       4\r
  UDP    0.0.0.0:5353           *:*                                    2345\r
";

    /// Captured verbatim from `ss -lntuHp` run as root under WSL.
    const REAL_SS: &str = concat!(
        "udp UNCONN 0      0          127.0.0.54:53  0.0.0.0:* users:((\"systemd-resolve\",pid=165,fd=16))\n",
        "udp UNCONN 0      0       127.0.0.53%lo:53  0.0.0.0:* users:((\"systemd-resolve\",pid=165,fd=14))\n",
        "udp UNCONN 0      0      10.255.255.254:53  0.0.0.0:*                                          \n",
        "udp UNCONN 0      0               [::1]:323    [::]:*                                          \n",
        "tcp LISTEN 0      1000   10.255.255.254:53  0.0.0.0:*\n",
    );

    // --- netstat -------------------------------------------------------------

    #[test]
    fn parse_netstat_reads_the_real_capture() {
        let ports = sorted(parse_netstat(REAL_NETSTAT));
        assert_eq!(
            ports,
            vec![
                ListeningPort {
                    port: 80,
                    protocol: "tcp".into(),
                    pid: Some(1960),
                    process: None
                },
                ListeningPort {
                    port: 445,
                    protocol: "tcp".into(),
                    pid: Some(4),
                    process: None
                },
                ListeningPort {
                    port: 5353,
                    protocol: "udp".into(),
                    pid: Some(2345),
                    process: None
                },
                ListeningPort {
                    port: 5432,
                    protocol: "tcp".into(),
                    pid: Some(7112),
                    process: None
                },
            ],
            "the IPv4 and IPv6 rows of 445 collapse into one, and the \
             established connection is not a listener"
        );
    }

    /// The whole point of not reading the state word: a localised `netstat`
    /// must yield the same table.
    #[test]
    fn parse_netstat_ignores_a_localised_state_word() {
        let localised =
            "  TCP    0.0.0.0:8080           0.0.0.0:0              À_L_ÉCOUTE      42\r\n";
        let ports = parse_netstat(localised);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 8080);
        assert_eq!(ports[0].pid, Some(42));
    }

    #[test]
    fn parse_netstat_skips_an_established_connection() {
        let row =
            "  TCP    192.168.1.24:52310     140.82.121.6:443       ESTABLISHED     14288\r\n";
        assert!(parse_netstat(row).is_empty());
    }

    #[test]
    fn parse_netstat_survives_a_row_with_no_pid() {
        let row = "  UDP    0.0.0.0:5353           *:*\r\n";
        let ports = parse_netstat(row);
        assert_eq!(ports.len(), 1);
        assert_eq!(
            ports[0].pid, None,
            "the last field is the remote endpoint, which is not a number"
        );
    }

    #[test]
    fn parse_netstat_skips_garbage_without_panicking() {
        for junk in ["", "   ", "TCP", "TCP nope nope", "🙂 🙂 🙂"] {
            let _ = parse_netstat(junk);
        }
    }

    // --- tasklist ------------------------------------------------------------

    #[test]
    fn parse_tasklist_maps_pids_to_names() {
        let raw = "\"System Idle Process\",\"0\",\"Services\",\"0\",\"8 Ko\"\r\n\
                   \"chrome.exe\",\"14288\",\"Console\",\"1\",\"236 812 Ko\"\r\n";
        let names = parse_tasklist(raw);
        assert_eq!(
            names.get(&0).map(String::as_str),
            Some("System Idle Process")
        );
        assert_eq!(names.get(&14288).map(String::as_str), Some("chrome.exe"));
    }

    #[test]
    fn parse_tasklist_skips_a_line_it_cannot_read() {
        assert!(parse_tasklist("INFO: no tasks are running\r\n").is_empty());
    }

    // --- ss ------------------------------------------------------------------

    #[test]
    fn parse_ss_reads_the_real_capture() {
        let ports = sorted(parse_ss(REAL_SS));
        assert_eq!(
            ports,
            vec![
                ListeningPort {
                    port: 53,
                    protocol: "tcp".into(),
                    pid: None,
                    process: None
                },
                ListeningPort {
                    port: 53,
                    protocol: "udp".into(),
                    pid: None,
                    process: None
                },
                ListeningPort {
                    port: 53,
                    protocol: "udp".into(),
                    pid: Some(165),
                    process: Some("systemd-resolve".into())
                },
                ListeningPort {
                    port: 323,
                    protocol: "udp".into(),
                    pid: None,
                    process: None
                },
            ],
            "an interface-scoped address and a bracketed IPv6 one both parse, \
             and a row `ss` would not attribute keeps a `None` owner"
        );
    }

    #[test]
    fn parse_ss_skips_garbage_without_panicking() {
        for junk in ["", "tcp LISTEN", "nope nope nope nope nope nope"] {
            let _ = parse_ss(junk);
        }
    }

    // --- endpoints -----------------------------------------------------------

    #[test]
    fn endpoint_port_reads_every_address_shape() {
        assert_eq!(endpoint_port("0.0.0.0:80"), Some(80));
        assert_eq!(endpoint_port("[::]:445"), Some(445));
        assert_eq!(endpoint_port("[::1]:323"), Some(323));
        assert_eq!(endpoint_port("127.0.0.53%lo:53"), Some(53));
        assert_eq!(endpoint_port("*:*"), None);
        assert_eq!(endpoint_port("nonsense"), None);
        assert_eq!(
            endpoint_port("0.0.0.0:99999"),
            None,
            "a port that does not fit a u16 is not a port"
        );
    }

    // --- the live machine ----------------------------------------------------

    /// Not run by the suite: it depends on what happens to be listening. Kept
    /// as the way to eyeball a real scan — `cargo test -- --ignored live_scan
    /// --nocapture` — because the parsers above can only ever prove they read
    /// the samples they were written from.
    #[test]
    #[ignore = "depends on the machine's live sockets"]
    fn live_scan_prints_what_this_host_is_holding() {
        let ports = scan().expect("scan should run on a developer machine");
        for port in &ports {
            println!(
                "{:>6}/{} {}",
                port.port,
                port.protocol,
                port.owner_label().unwrap_or_else(|| "?".into())
            );
        }
        assert!(!ports.is_empty(), "a desktop always listens on something");
    }

    // --- owner label ---------------------------------------------------------

    #[test]
    fn owner_label_degrades_one_step_at_a_time() {
        let mut port = ListeningPort {
            port: 80,
            protocol: "tcp".into(),
            pid: Some(1960),
            process: Some("httpd.exe".into()),
        };
        assert_eq!(port.owner_label().as_deref(), Some("httpd.exe (PID 1960)"));
        port.process = None;
        assert_eq!(port.owner_label().as_deref(), Some("PID 1960"));
        port.pid = None;
        assert_eq!(port.owner_label(), None);
    }
}
