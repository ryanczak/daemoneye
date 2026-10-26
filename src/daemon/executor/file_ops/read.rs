use super::super::ToolCallOutcome;
use crate::ai::mask_sensitive;
use crate::shell::Owner;

/// Extract lines between a unique start marker and end marker from pane output.
fn extract_marked(snap: &str, start: &str, end: &str) -> Option<String> {
    let lines: Vec<&str> = snap.lines().collect();
    let s_idx = lines.iter().position(|l| l.trim() == start)?;
    let e_idx = lines.iter().rposition(|l| l.trim() == end)?;
    if e_idx <= s_idx {
        return None;
    }
    Some(lines[s_idx + 1..e_idx].join("\n"))
}

/// Build the shell command to read `path` from a remote pane with markers.
fn build_remote_read_cmd(path: &str, start: usize, end: usize, pattern: Option<&str>) -> String {
    let safe_path = super::sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", super::sq_escape(p)))
        .unwrap_or_default();
    format!(
        "echo '__DE_S__'; sed -n '{},{}p' '{}' 2>&1{}; echo '__DE_E__'; echo '__DE_DONE__'",
        start, end, safe_path, grep_part
    )
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub async fn run_read_file(
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<&str>,
    host: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    if path.contains("..") {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain '..'.".to_string(),
        ));
    }
    if super::contains_control(path) {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain control characters.".to_string(),
        ));
    }
    if pattern.is_some_and(super::contains_control) {
        return Ok(ToolCallOutcome::Result(
            "Error: grep pattern must not contain control characters.".to_string(),
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Ok(ToolCallOutcome::Result(
            "Error: path must be absolute (e.g. /var/log/syslog).".to_string(),
        ));
    }

    if super::is_blocked_secret_path(path) {
        return Ok(ToolCallOutcome::Result(
            "Error: read_file cannot access well-known credential files \
             (~/.ssh, .env, private keys). This is not a security boundary — \
             use run_terminal_command only if you truly need to inspect one."
                .to_string(),
        ));
    }

    {
        let de_dir = crate::config::config_dir();
        let candidate = super::resolve_path_for_guard(path);
        if candidate.starts_with(&de_dir) {
            let blocked = [
                de_dir.join("etc").join("config.toml"),
                de_dir.join("etc").join("prompts").join("sre.toml"),
            ];
            if blocked.contains(&candidate) {
                return Ok(ToolCallOutcome::Result(
                    "Error: read_file cannot access daemoneye credential files \
                     (etc/config.toml, etc/prompts/sre.toml). These may contain \
                     sensitive API keys. Use read_script, read_runbook, read_memory, \
                     or search_repository for other daemoneye-managed data."
                        .to_string(),
                ));
            }
        }
    }

    const MAX_LINES: usize = 500;
    const DEFAULT_LINES: usize = 200;
    let limit_n = match limit {
        Some(n) if n > 0 => (n as usize).min(MAX_LINES),
        _ => DEFAULT_LINES,
    };
    let offset_n = offset.map(|o| (o as usize).saturating_sub(1)).unwrap_or(0);

    // ── Host resolution: `None` / "local" is the daemon host ───────────────
    let host_id = match host {
        None => crate::host::HostId::Local,
        Some(name) => match crate::host::HostId::parse(name) {
            Ok(parsed) => parsed,
            Err(msg) => return Ok(ToolCallOutcome::Result(format!("Error: {msg}"))),
        },
    };
    match &host_id {
        crate::host::HostId::Local => {
            // ── Local path: read directly from daemon-host filesystem ─────
            let real_path =
                std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
            let raw = match std::fs::read_to_string(&real_path) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error reading {}: {}",
                        path, e
                    )));
                }
            };

            let all_lines: Vec<&str> = raw.lines().collect();
            let total = all_lines.len();
            let sliced = &all_lines[offset_n.min(total)..];
            let limited: Vec<&str> = sliced.iter().take(limit_n).copied().collect();
            let limited_len = limited.len();

            let filtered: Vec<&str> = if let Some(pat) = pattern {
                match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
                    Ok(re) => limited.into_iter().filter(|l| re.is_match(l)).collect(),
                    Err(e) => {
                        return Ok(ToolCallOutcome::Result(format!(
                            "Error: invalid pattern regex: {}",
                            e
                        )));
                    }
                }
            } else {
                limited
            };

            if filtered.is_empty() {
                return Ok(ToolCallOutcome::Result(format!(
                    "{}: no lines matched (total {} lines in file)",
                    path, total
                )));
            }

            let body = mask_sensitive(&filtered.join("\n"));
            if pattern.is_some() {
                Ok(ToolCallOutcome::Result(format!(
                    "{} ({} matching lines, searched lines {}-{} of {}):\n{}",
                    path,
                    filtered.len(),
                    offset_n + 1,
                    (offset_n + limited_len).min(total),
                    total,
                    body
                )))
            } else {
                Ok(ToolCallOutcome::Result(format!(
                    "{} (lines {}-{} of {}):\n{}",
                    path,
                    offset_n + 1,
                    (offset_n + filtered.len()).min(total),
                    total,
                    body
                )))
            }
        }
        crate::host::HostId::Named(name) => {
            let start = offset_n + 1;
            let end = offset_n + limit_n;
            let cmd = build_remote_read_cmd(path, start, end, pattern);
            let snap = match super::run_in_host_shell(
                crate::shell::registry(),
                Owner::Chat(host_session_key()),
                host_id.clone(),
                &cmd,
                REMOTE_READ_TIMEOUT_SECS,
                |id| {
                    let (rows, cols) = crate::shell::spawn_size();
                    crate::shell::spawn_ssh_host(id, name, rows, cols)
                },
            )
            .await
            {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
            };
            let extracted =
                extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap_or_else(|| snap.clone());
            Ok(format_remote_read_result(
                path, name, start, end, pattern, &extracted,
            ))
        }
    }
}

/// The owner key a `read_file { host }` run claims its shell under.
///
/// `run_read_file` receives no session id, so the shells a remote read
/// claims are keyed to the tool itself: `chat:read_file`. Keying on the
/// tool rather than inventing a per-session key is what makes test 6's
/// one-shell-per-host reuse property hold without threading `session_id`
/// through every read.
fn host_session_key() -> String {
    "read_file".to_string()
}

/// The remote read's result label. The host name is in it so a model reading
/// two files from two machines can tell them apart.
fn format_remote_read_result(
    path: &str,
    host: &str,
    start: usize,
    end: usize,
    pattern: Option<&str>,
    content: &str,
) -> ToolCallOutcome {
    if content.trim().is_empty() {
        return ToolCallOutcome::Result(format!(
            "{}: no output (file may be empty or lines out of range)",
            path
        ));
    }
    let body = mask_sensitive(content.trim_end());
    let label = if pattern.is_some() {
        format!(
            "{} on host {} (remote grep, lines {}-{}):\n{}",
            path, host, start, end, body
        )
    } else {
        format!(
            "{} on host {} (lines {}-{}):\n{}",
            path, host, start, end, body
        )
    };
    ToolCallOutcome::Result(label)
}
/// The timeout one remote read is given, in seconds — the pane path's own
/// value, carried over unchanged.
const REMOTE_READ_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
