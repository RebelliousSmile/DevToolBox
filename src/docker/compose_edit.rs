//! `compose_edit` module — applying a [`ReassignmentPlan`] to the compose
//! files it names.
//!
//! This is the only place in the program that writes to a file the **user**
//! owns, which shapes every decision below.
//!
//! # Why a line rewrite and not a YAML round-trip
//!
//! [`crate::docker::compose`] deliberately reads compose files through
//! `docker compose config` rather than a YAML parser, because only compose
//! resolves `extends`, `include`, `.env` interpolation and profiles the way
//! compose does. That read path is useless for *writing*: `config` emits a
//! fully resolved, canonicalised document, so writing it back would replace
//! the user's file with a machine-generated one — comments gone, anchors
//! expanded, `${VAR}` frozen to whatever it happened to resolve to today.
//!
//! A serialise-and-write YAML round-trip has the same problem for the same
//! reason. So this module changes **one number on one line** and leaves every
//! other byte — comments, quoting style, indentation, CRLF line endings —
//! exactly as it found it.
//!
//! # What it refuses
//!
//! The price of that surgical approach is that it only understands what it
//! can see literally. A published port written `${WEB_PORT}` or `8080-8090`
//! is refused by name rather than guessed at, and so is a service whose
//! `ports:` block turns out to hold two entries matching the same host port.
//! Every refusal comes back in [`FileReport::refused`] and is shown to the
//! user: silently skipping one edit of a plan would leave a collision the
//! preview said was fixed.
//!
//! # The backup is not optional
//!
//! Every file is copied into `data_dir()/compose-backups/` before it is
//! touched, and a file whose backup fails is **not** rewritten. The copy
//! lands in the application's own directory rather than next to the original:
//! a `docker-compose.yml.bak` appearing in the user's repository is noise
//! they did not ask for, and would get committed by the next `git add -A`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ui::port_plan::PortMove;

/// One `ports:` entry actually rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    pub service: String,
    pub from: u16,
    pub to: u16,
    /// 1-based line number, so it can be read out next to an editor.
    pub line: usize,
}

/// What happened to one compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub file: String,
    pub applied: Vec<AppliedEdit>,
    /// Moves this file could not take, each with its reason, in display form.
    pub refused: Vec<String>,
    /// Where the untouched original was copied, when anything was written.
    pub backup: Option<String>,
}

/// The outcome of a pure rewrite: the new text plus what it did and did not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResult {
    pub text: String,
    pub applied: Vec<AppliedEdit>,
    pub refused: Vec<String>,
}

/// One `ports:` entry located in a line, reduced to what a rewrite needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PortLine {
    host_port: u16,
    /// `None` when the line does not state a protocol — the long `published:`
    /// form. Such a line matches any protocol, which is safe only because a
    /// duplicate match is refused rather than guessed.
    protocol: Option<String>,
    /// Byte range of the host port digits inside the line, so splicing the
    /// replacement in preserves quoting, spacing and any trailing comment.
    span: std::ops::Range<usize>,
}

/// Leading-space count. Tabs are invalid YAML indentation, so a line using
/// them simply never matches a block and its file is refused rather than
/// mis-parsed.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The line's content without its `\n` / `\r\n` terminator.
fn without_terminator(chunk: &str) -> &str {
    chunk
        .strip_suffix('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .unwrap_or(chunk)
}

/// Parses `- "8080:80"`, `- 8080:80/udp`, `- '127.0.0.1:8080:80'`.
///
/// Returns `None` for anything that is not a published short-form entry —
/// including the bare `- 80` container-only form, which publishes nothing and
/// must never be rewritten into a host binding.
fn short_form(line: &str) -> Option<PortLine> {
    let dash = line.find("- ")?;
    let after_dash = dash + 2;
    let rest = &line[after_dash..];
    let lead = rest.len() - rest.trim_start().len();
    let scalar_start = after_dash + lead;
    let body = &line[scalar_start..];

    // Quoted scalars end at their closing quote; bare ones at a trailing
    // comment or the end of the line.
    let (scalar, scalar_start) = match body.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let end = body[1..].find(quote)? + 1;
            (&body[1..end], scalar_start + 1)
        }
        _ => {
            let end = body.find(" #").unwrap_or(body.len());
            (body[..end].trim_end(), scalar_start)
        }
    };

    // `/proto` is a suffix of the whole scalar, never of the host side.
    let (mapping, protocol) = match scalar.split_once('/') {
        Some((left, proto)) => (left, proto.trim().to_lowercase()),
        None => (scalar, "tcp".to_string()),
    };

    // `[ip:]host:container` — the host side is the second field from the end,
    // and a bare `container` publishes nothing.
    let separators: Vec<usize> = mapping.match_indices(':').map(|(at, _)| at).collect();
    if separators.is_empty() {
        return None;
    }
    let host_end = *separators.last()?;
    let host_start = if separators.len() >= 2 {
        separators[separators.len() - 2] + 1
    } else {
        0
    };
    let host = &mapping[host_start..host_end];
    let host_port = host.trim().parse::<u16>().ok()?;

    Some(PortLine {
        host_port,
        protocol: Some(protocol),
        span: (scalar_start + host_start)..(scalar_start + host_end),
    })
}

/// Parses the long form's `published:` line, with or without a leading `- `.
fn long_form(line: &str) -> Option<PortLine> {
    let trimmed = line.trim_start();
    let body = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim_start();
    let offset = line.len() - body.len();
    let value = body.strip_prefix("published:")?;
    let value_offset = offset + "published:".len();

    let lead = value.len() - value.trim_start().len();
    let value = value.trim_start();
    let (digits, digits_offset) = match value.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let end = value[1..].find(quote)? + 1;
            (&value[1..end], value_offset + lead + 1)
        }
        _ => {
            let end = value
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(value.len());
            (&value[..end], value_offset + lead)
        }
    };
    let host_port = digits.trim().parse::<u16>().ok()?;
    Some(PortLine {
        host_port,
        protocol: None,
        span: digits_offset..(digits_offset + digits.len()),
    })
}

/// One candidate line for a move: where it is and what it holds.
struct Candidate {
    /// Index into the chunk list, i.e. 0-based line number.
    index: usize,
    port_line: PortLine,
}

/// Every `ports:` entry of `service`, in file order.
///
/// The walk is indentation-based rather than YAML-aware on purpose: it has to
/// agree with the file's *text*, which is what gets rewritten, and a real
/// parser would hand back a value tree with no byte offsets in it.
fn entries_of_service(chunks: &[&str], service: &str) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let mut in_services = false;
    let mut service_indent: Option<usize> = None;
    let mut current: Option<&str> = None;
    let mut ports_indent: Option<usize> = None;

    for (index, chunk) in chunks.iter().enumerate() {
        let line = without_terminator(chunk);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = indent_of(line);

        if indent == 0 {
            in_services = trimmed == "services:";
            service_indent = None;
            current = None;
            ports_indent = None;
            continue;
        }
        if !in_services {
            continue;
        }

        let depth = *service_indent.get_or_insert(indent);
        if indent < depth {
            // Dedented out of the services mapping entirely.
            in_services = false;
            continue;
        }
        if indent == depth {
            current = trimmed.strip_suffix(':');
            ports_indent = None;
            continue;
        }

        // Deeper than a service key: either a key of that service, or an item
        // of the `ports:` list we are currently inside.
        if ports_indent.is_some_and(|open| indent <= open) {
            ports_indent = None;
        }
        let Some(open) = ports_indent else {
            if trimmed == "ports:" {
                ports_indent = Some(indent);
            }
            continue;
        };
        let _ = open;
        if current != Some(service) {
            continue;
        }
        if let Some(port_line) = short_form(line).or_else(|| long_form(line)) {
            candidates.push(Candidate { index, port_line });
        }
    }
    candidates
}

/// Apply every move of one file to its text, without touching the disk.
///
/// A move matching no entry, or matching two, changes nothing and comes back
/// in [`RewriteResult::refused`]: a half-applied plan is worse than a refused
/// one, because the preview said the collision was gone.
pub fn rewrite(text: &str, moves: &[PortMove]) -> RewriteResult {
    let mut chunks: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    let mut applied = Vec::new();
    let mut refused = Vec::new();

    for port_move in moves {
        let borrowed: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let matching: Vec<Candidate> = entries_of_service(&borrowed, &port_move.service)
            .into_iter()
            .filter(|candidate| {
                candidate.port_line.host_port == port_move.from
                    && candidate
                        .port_line
                        .protocol
                        .as_deref()
                        .is_none_or(|protocol| protocol == port_move.protocol)
            })
            .collect();

        match matching.len() {
            1 => {
                let candidate = &matching[0];
                let line = &mut chunks[candidate.index];
                line.replace_range(candidate.port_line.span.clone(), &port_move.to.to_string());
                applied.push(AppliedEdit {
                    service: port_move.service.clone(),
                    from: port_move.from,
                    to: port_move.to,
                    line: candidate.index + 1,
                });
            }
            0 => refused.push(format!(
                "{} : port {}/{} introuvable en clair dans le fichier (variable, plage, ou service absent) — à modifier à la main",
                port_move.service, port_move.from, port_move.protocol
            )),
            count => refused.push(format!(
                "{} : {count} entrées publient {}/{}, réattribution ambiguë — à modifier à la main",
                port_move.service, port_move.from, port_move.protocol
            )),
        }
    }

    RewriteResult {
        text: chunks.concat(),
        applied,
        refused,
    }
}

/// Directory the pre-edit copies go into.
fn backup_dir() -> PathBuf {
    crate::platform::data_dir().join("compose-backups")
}

/// A file name that cannot collide with another path's: the whole original
/// path is folded into it, not just its base name — half the compose files on
/// a machine are called `docker-compose.yml`.
fn backup_name(file: &str, stamp: u64) -> String {
    let flattened: String = file
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '.' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("{stamp}-{flattened}")
}

/// Copy `file` into the backup directory, returning where it landed.
fn back_up(file: &Path) -> Result<PathBuf, String> {
    let directory = backup_dir();
    fs::create_dir_all(&directory)
        .map_err(|error| format!("sauvegarde impossible ({}) : {error}", directory.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let target = directory.join(backup_name(&file.to_string_lossy(), stamp));
    fs::copy(file, &target).map_err(|error| format!("sauvegarde impossible : {error}"))?;
    Ok(target)
}

/// Apply a whole plan, one report per file.
///
/// Files are handled independently: one unreadable compose file must not cost
/// the user the edits every other file could take.
pub fn apply(moves: &[PortMove]) -> Vec<FileReport> {
    let mut by_file: BTreeMap<String, Vec<PortMove>> = BTreeMap::new();
    for port_move in moves {
        by_file
            .entry(port_move.file.clone())
            .or_default()
            .push(port_move.clone());
    }

    by_file
        .into_iter()
        .map(|(file, moves)| apply_to_file(&file, &moves))
        .collect()
}

fn apply_to_file(file: &str, moves: &[PortMove]) -> FileReport {
    let path = Path::new(file);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return FileReport {
                file: file.to_string(),
                applied: Vec::new(),
                refused: vec![format!("lecture impossible : {error}")],
                backup: None,
            }
        }
    };

    let result = rewrite(&text, moves);
    if result.applied.is_empty() {
        // Nothing changed, so nothing is written and nothing is backed up —
        // rewriting a file with identical content would still bump its mtime
        // and, on a watched project, trigger a rebuild for no reason.
        return FileReport {
            file: file.to_string(),
            applied: result.applied,
            refused: result.refused,
            backup: None,
        };
    }

    let backup = match back_up(path) {
        Ok(target) => target,
        Err(error) => {
            let mut refused = result.refused;
            refused.push(format!("{error} — fichier laissé intact"));
            return FileReport {
                file: file.to_string(),
                applied: Vec::new(),
                refused,
                backup: None,
            };
        }
    };

    match fs::write(path, &result.text) {
        Ok(()) => FileReport {
            file: file.to_string(),
            applied: result.applied,
            refused: result.refused,
            backup: Some(backup.to_string_lossy().into_owned()),
        },
        Err(error) => {
            let mut refused = result.refused;
            refused.push(format!("écriture impossible : {error}"));
            FileReport {
                file: file.to_string(),
                applied: Vec::new(),
                refused,
                backup: Some(backup.to_string_lossy().into_owned()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::port_plan::MoveReason;

    /// Shaped after the real compose files under `$HOME` on the reference
    /// machine: quoted and unquoted entries, an interface-qualified one, a
    /// long-form entry, a comment, and CRLF endings — the Windows default.
    const REAL_COMPOSE: &str = concat!(
        "services:\r\n",
        "  web:\r\n",
        "    image: nginx\r\n",
        "    ports:\r\n",
        "      # exposé sur le LAN\r\n",
        "      - \"8080:80\"\r\n",
        "      - 8443:443\r\n",
        "    restart: unless-stopped\r\n",
        "  db:\r\n",
        "    image: postgres:16\r\n",
        "    ports:\r\n",
        "      - 127.0.0.1:5432:5432\r\n",
        "  dns:\r\n",
        "    ports:\r\n",
        "      - \"53:53/udp\"\r\n",
        "  api:\r\n",
        "    ports:\r\n",
        "      - target: 3000\r\n",
        "        published: 3000\r\n",
        "        protocol: tcp\r\n",
    );

    fn port_move(service: &str, from: u16, to: u16, protocol: &str) -> PortMove {
        PortMove {
            file: "/compose.yml".to_string(),
            service: service.to_string(),
            protocol: protocol.to_string(),
            from,
            to,
            reason: MoveReason::TakenByHost("test".to_string()),
        }
    }

    #[test]
    fn a_quoted_entry_keeps_its_quotes_and_its_container_port() {
        let result = rewrite(REAL_COMPOSE, &[port_move("web", 8080, 8081, "tcp")]);
        assert!(result.text.contains("- \"8081:80\""), "{}", result.text);
        assert!(result.refused.is_empty());
        assert_eq!(result.applied[0].line, 6);
    }

    #[test]
    fn an_unquoted_entry_stays_unquoted() {
        let result = rewrite(REAL_COMPOSE, &[port_move("web", 8443, 8444, "tcp")]);
        assert!(result.text.contains("- 8444:443"));
        assert!(!result.text.contains("8443"));
    }

    #[test]
    fn an_interface_qualified_entry_keeps_its_interface() {
        let result = rewrite(REAL_COMPOSE, &[port_move("db", 5432, 5433, "tcp")]);
        assert!(
            result.text.contains("- 127.0.0.1:5433:5432"),
            "{}",
            result.text
        );
    }

    #[test]
    fn a_udp_entry_keeps_its_protocol_suffix() {
        let result = rewrite(REAL_COMPOSE, &[port_move("dns", 53, 1053, "udp")]);
        assert!(result.text.contains("- \"1053:53/udp\""), "{}", result.text);
    }

    #[test]
    fn the_long_form_published_key_is_rewritten_too() {
        let result = rewrite(REAL_COMPOSE, &[port_move("api", 3000, 3001, "tcp")]);
        assert!(result.text.contains("published: 3001"), "{}", result.text);
        assert!(
            result.text.contains("target: 3000"),
            "the container side must not move"
        );
    }

    /// Every byte outside the number changes, or the module has failed at its
    /// one job.
    #[test]
    fn nothing_but_the_number_changes() {
        let result = rewrite(REAL_COMPOSE, &[port_move("web", 8080, 8081, "tcp")]);
        assert_eq!(
            result.text.replace("8081", "8080"),
            REAL_COMPOSE,
            "comments, CRLF and quoting must survive untouched"
        );
    }

    #[test]
    fn crlf_endings_survive() {
        let result = rewrite(REAL_COMPOSE, &[port_move("web", 8080, 8081, "tcp")]);
        assert_eq!(
            result.text.matches("\r\n").count(),
            REAL_COMPOSE.matches("\r\n").count()
        );
    }

    #[test]
    fn a_service_that_does_not_publish_the_port_is_refused_not_guessed() {
        let result = rewrite(REAL_COMPOSE, &[port_move("db", 8080, 8081, "tcp")]);
        assert!(result.applied.is_empty());
        assert_eq!(result.refused.len(), 1);
        assert!(result.refused[0].contains("introuvable"));
        assert_eq!(result.text, REAL_COMPOSE);
    }

    /// The refusal that matters most: an interpolated port is exactly the
    /// shape a naive text replacement would corrupt.
    #[test]
    fn an_interpolated_port_is_refused() {
        let text = "services:\n  web:\n    ports:\n      - \"${WEB_PORT}:80\"\n";
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert!(result.applied.is_empty());
        assert_eq!(result.text, text);
    }

    #[test]
    fn a_port_range_is_refused() {
        let text = "services:\n  web:\n    ports:\n      - 8080-8090:80-90\n";
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert!(result.applied.is_empty());
        assert_eq!(result.text, text);
    }

    #[test]
    fn two_entries_on_the_same_port_are_ambiguous_and_refused() {
        let text = "services:\n  web:\n    ports:\n      - \"8080:80\"\n      - \"8080:81\"\n";
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert!(result.applied.is_empty());
        assert!(result.refused[0].contains("ambiguë"));
        assert_eq!(result.text, text);
    }

    /// A container-only `- 80` publishes nothing; turning it into a host
    /// binding would change what the file means.
    #[test]
    fn a_container_only_entry_is_never_matched() {
        let text = "services:\n  web:\n    ports:\n      - 8080\n";
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert!(result.applied.is_empty());
        assert_eq!(result.text, text);
    }

    /// `expose:` and `environment:` hold numbers that look exactly like a
    /// port, one line away from the block being edited.
    #[test]
    fn only_the_ports_block_is_read() {
        let text = concat!(
            "services:\n",
            "  web:\n",
            "    expose:\n",
            "      - 8080\n",
            "    environment:\n",
            "      PORT: 8080\n",
            "    ports:\n",
            "      - \"8080:80\"\n",
        );
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert_eq!(result.applied.len(), 1);
        assert!(
            result.text.contains("      - 8080\n"),
            "expose must not move"
        );
        assert!(
            result.text.contains("PORT: 8080"),
            "the env var must not move"
        );
        assert!(result.text.contains("- \"8081:80\""));
    }

    /// A same-named key under another top-level section would make the edit
    /// look ambiguous — and get the whole move refused — if the walk did not
    /// stop at `services:`' closing brace.
    #[test]
    fn a_top_level_key_after_services_ends_the_walk() {
        let text = concat!(
            "services:\n",
            "  web:\n",
            "    ports:\n",
            "      - \"8080:80\"\n",
            "x-template:\n",
            "  web:\n",
            "    ports:\n",
            "      - \"8080:80\"\n",
        );
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert_eq!(result.applied.len(), 1);
        assert_eq!(
            result.text,
            concat!(
                "services:\n",
                "  web:\n",
                "    ports:\n",
                "      - \"8081:80\"\n",
                "x-template:\n",
                "  web:\n",
                "    ports:\n",
                "      - \"8080:80\"\n",
            )
        );
    }

    #[test]
    fn several_moves_on_one_file_all_land() {
        let result = rewrite(
            REAL_COMPOSE,
            &[
                port_move("web", 8080, 8081, "tcp"),
                port_move("db", 5432, 5433, "tcp"),
            ],
        );
        assert_eq!(result.applied.len(), 2);
        assert!(result.text.contains("- \"8081:80\""));
        assert!(result.text.contains("- 127.0.0.1:5433:5432"));
    }

    #[test]
    fn a_trailing_comment_on_an_entry_survives() {
        let text = "services:\n  web:\n    ports:\n      - 8080:80 # front\n";
        let result = rewrite(text, &[port_move("web", 8080, 8081, "tcp")]);
        assert_eq!(
            result.text,
            "services:\n  web:\n    ports:\n      - 8081:80 # front\n"
        );
    }

    #[test]
    fn short_form_reads_every_published_shape() {
        assert_eq!(
            short_form("      - \"8080:80\"").map(|p| p.host_port),
            Some(8080)
        );
        assert_eq!(
            short_form("      - 8080:80/udp").map(|p| p.protocol),
            Some(Some("udp".to_string()))
        );
        assert_eq!(
            short_form("      - '0.0.0.0:8080:80'").map(|p| p.host_port),
            Some(8080)
        );
        assert_eq!(short_form("      - 80").map(|p| p.host_port), None);
        assert_eq!(short_form("    image: nginx").map(|p| p.host_port), None);
    }

    #[test]
    fn backup_name_folds_the_whole_path_in() {
        let left = backup_name("/home/a/docker-compose.yml", 42);
        let right = backup_name("/home/b/docker-compose.yml", 42);
        assert_ne!(left, right);
        assert!(left.starts_with("42-"));
    }

    #[test]
    fn apply_on_a_missing_file_reports_rather_than_panicking() {
        let reports = apply(&[PortMove {
            file: "/definitely/not/here/compose.yml".to_string(),
            ..port_move("web", 8080, 8081, "tcp")
        }]);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].applied.is_empty());
        assert_eq!(reports[0].refused.len(), 1);
    }
}
