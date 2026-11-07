use super::super::ToolCallOutcome;
use crate::ai::mask_sensitive;
use crate::shell::Owner;

/// A local `read_file` never reads more than this many bytes in, even to
/// return only its 200-line window: a multi-gigabyte log cannot be pulled
/// through memory for a page of it (M36 finding S8). When the cap stops the
/// count, the reported total is written `N+` instead of an exact `N`.
const READ_BYTE_CAP: usize = 8 * 1024 * 1024;

/// What the blocking-pool read of one local file produced.
enum LocalRead {
    /// The path exists but is not a regular file — device, FIFO, socket,
    /// directory. Refused before anything is opened, so a FIFO cannot block.
    NotRegular,
    /// The lines read, the total number of lines counted (the `N` in `N+`
    /// when the cap stopped the count), whether EOF was reached within the
    /// cap, and whether the cap cut the read.
    Window(ReadWindow),
}

/// The streamed result of reading one regular file under [`READ_BYTE_CAP`].
struct ReadWindow {
    lines: Vec<String>,
    counted: usize,
    at_eof: bool,
    capped: bool,
}

/// Stream a local file into a `offset..offset+limit` line window, counting
/// lines only as far as [`READ_BYTE_CAP`].
///
/// Never reads the file in one go: bytes arrive line by line through a
/// buffered reader, and after the window is complete the stream continues
/// only to count the rest, still stopping at the cap. A partial line at the
/// cap is dropped, and a cap reached before the window is complete marks the
/// window `capped` so the caller can report where the read stopped.
fn read_window(path: &std::path::Path, offset: usize, limit: usize) -> std::io::Result<ReadWindow> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Ok(ReadWindow {
            lines: Vec::new(),
            counted: 0,
            at_eof: true,
            capped: false,
        });
    }

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(1 << 16, file);
    let mut window = ReadWindow {
        lines: Vec::new(),
        counted: 0,
        at_eof: false,
        capped: false,
    };
    let mut buf = Vec::with_capacity(8192);
    let mut total_read = 0usize;
    loop {
        let remaining = READ_BYTE_CAP - total_read;
        if remaining == 0 {
            window.capped = true;
            break;
        }
        let Some(line) = read_line_into(&mut reader, &mut buf, remaining)? else {
            window.at_eof = true;
            break;
        };
        total_read += line.bytes;
        if !line.terminated && total_read >= READ_BYTE_CAP {
            // The cap cut this line off: drop it, and do not count it.
            window.capped = true;
            break;
        }
        // An unterminated line under the cap is the file's last line; the
        // next read sees EOF.
        window.counted += 1;
        if window.counted > offset && window.lines.len() < limit {
            let text = String::from_utf8_lossy(&buf);
            window
                .lines
                .push(text.trim_end_matches(['\n', '\r']).to_string());
        }
    }
    Ok(window)
}

/// One line as [`read_line_into`] delivered it.
struct LineRead {
    /// Bytes consumed, the newline included when there was one.
    bytes: usize,
    /// Whether the line ended in `\n`. A line that did not either met EOF
    /// (a last line without a newline) or met the byte budget (cut off).
    terminated: bool,
}

/// Read one line, at most `max` bytes, from `reader` into `buf`. `None` at EOF
/// with nothing read.
fn read_line_into<R: std::io::BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<Option<LineRead>> {
    use std::io::{BufRead as _, Read as _};
    buf.clear();
    let bytes = reader.by_ref().take(max as u64).read_until(b'\n', buf)?;
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(LineRead {
        bytes,
        terminated: buf.last() == Some(&b'\n'),
    }))
}

/// Run the whole local read — canonicalize, metadata check, streamed window —
/// on the blocking pool, so a device or FIFO never occupies a runtime worker.
/// The canonicalization doubles as the symlink resolution: the read carries only
/// one canonical `Path`, and anything it points at must still be a regular file.
fn read_local(path: &str, offset: usize, limit: usize) -> std::io::Result<LocalRead> {
    let real = std::fs::canonicalize(path)?;
    if !std::fs::metadata(&real)?.is_file() {
        return Ok(LocalRead::NotRegular);
    }
    Ok(LocalRead::Window(read_window(&real, offset, limit)?))
}

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
        let candidate = super::resolve_path_for_guard(path);
        let blocked = super::guard_roots().into_iter().flat_map(|root| {
            [
                root.join("etc").join("config.toml"),
                root.join("etc").join("prompts").join("sre.toml"),
            ]
        });
        if blocked.into_iter().any(|b| b == candidate) {
            return Ok(ToolCallOutcome::Result(
                "Error: read_file cannot access daemoneye credential files \
                 (etc/config.toml, etc/prompts/sre.toml). These may contain \
                 sensitive API keys. Use read_script, read_runbook, read_memory, \
                 or search_repository for other daemoneye-managed data."
                    .to_string(),
            ));
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
            // ── Local path: streamed on the blocking pool (M36 S8) ─────────
            let read = match tokio::task::spawn_blocking({
                let path = path.to_string();
                move || read_local(&path, offset_n, limit_n)
            })
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error: read_file blocking task failed: {e}"
                    )));
                }
            };
            let window = match read {
                Ok(LocalRead::Window(w)) => w,
                Ok(LocalRead::NotRegular) => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error: {path} is not a regular file (read_file reads only regular files)."
                    )));
                }
                Err(e) => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error: read_file blocking task failed: {e}"
                    )));
                }
            };

            const CAP_NOTE: &str = "[read stopped at the 8 MiB cap]";
            let kept_before_filter = window.lines.len();
            let kept = window.lines;
            let window_incomplete = window.capped && kept_before_filter < limit_n;
            let total = window.counted;
            let total_text = if window.capped {
                format!("{total}+")
            } else {
                format!("{total}")
            };

            let filtered: Vec<String> = if let Some(pat) = pattern {
                match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
                    Ok(re) => kept.into_iter().filter(|l| re.is_match(l)).collect(),
                    Err(e) => {
                        return Ok(ToolCallOutcome::Result(format!(
                            "Error: invalid pattern regex: {}",
                            e
                        )));
                    }
                }
            } else {
                kept
            };

            if window_incomplete && filtered.is_empty() {
                return Ok(ToolCallOutcome::Result(format!("{}: {}", path, CAP_NOTE)));
            }

            if filtered.is_empty() {
                return Ok(ToolCallOutcome::Result(format!(
                    "{}: no lines matched (total {} lines in file)",
                    path, total_text
                )));
            }

            let mut body = mask_sensitive(&filtered.join("\n"));
            if window_incomplete {
                body.push('\n');
                body.push_str(CAP_NOTE);
            }
            if pattern.is_some() {
                Ok(ToolCallOutcome::Result(format!(
                    "{} ({} matching lines, searched lines {}-{} of {}):\n{}",
                    path,
                    filtered.len(),
                    offset_n + 1,
                    (offset_n + kept_before_filter).min(total),
                    total_text,
                    body
                )))
            } else {
                Ok(ToolCallOutcome::Result(format!(
                    "{} (lines {}-{} of {}):\n{}",
                    path,
                    offset_n + 1,
                    (offset_n + filtered.len()).min(total),
                    total_text,
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
