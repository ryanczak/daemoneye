//! Searching and waiting on shell output: `find_in_shells` and the blocking observer `watch_shell`.

use super::*;

// ---------------------------------------------------------------------------
// Find in shells (M22 phase-03c)
// ---------------------------------------------------------------------------

/// Hard ceiling on matches returned by a single `find_in_shells` call. Total
/// across every shell searched, mirroring `pane.rs`'s `FIND_MAX_MATCHES`.
const FIND_MAX_MATCHES: usize = 50;

/// One matching line plus its ±1 line of context. 1-indexed `line_no`.
struct ShellBufferMatch {
    line_no: usize,
    before: Option<String>,
    line: String,
    after: Option<String>,
}

/// Pure helper: find up to `limit` matches in a recorded-output buffer.
/// Extracted so the match and context arithmetic can be tested without a
/// registry or a cast file. Deliberately a copy of `pane.rs`'s `search_buffer`
/// rather than a cross-module import: `pane.rs` is deleted wholesale in
/// phase 09 and a dependency on it would have to be unpicked then.
fn search_shell_buffer(buffer: &str, re: &regex::Regex, limit: usize) -> Vec<ShellBufferMatch> {
    let lines: Vec<&str> = buffer.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if out.len() >= limit {
            break;
        }
        if re.is_match(line) {
            out.push(ShellBufferMatch {
                line_no: i + 1,
                before: i.checked_sub(1).map(|j| lines[j].to_string()),
                after: lines.get(i + 1).map(|s| s.to_string()),
                line: (*line).to_string(),
            });
        }
    }
    out
}

/// Per-shell record of where matches came from, before capping.
struct ShellHits {
    id: String,
    matches: Vec<ShellBufferMatch>,
}

pub fn find_in_shells(pattern: &str, scope: Option<&str>, caller: &Owner) -> String {
    find_in_shells_in(crate::shell::registry(), pattern, scope, caller)
}

pub(super) fn find_in_shells_in(
    reg: &ShellRegistry,
    pattern: &str,
    scope: Option<&str>,
    caller: &Owner,
) -> String {
    // 1. Build the regex first, before any lock or file read — an invalid
    //    pattern costs nothing and every shell stays unread.
    let re = match shell_tool_regex(pattern) {
        Ok(re) => re,
        Err(e) => {
            return format!(
                "Error: invalid find_in_shells pattern /{pattern}/: {e}. \
                 The pattern is a Rust `regex`-crate regular expression; \
                 quote literal characters and check bracket/paren balance."
            );
        }
    };

    // 2. Resolve scope through the shared match, as `list_shells` does, so the
    //    two tools cannot disagree about what a scope value means.
    let rows = match resolve_scope(reg, scope, caller, "find_in_shells") {
        Ok(rows) => rows,
        Err(msg) => return msg,
    };

    if rows.is_empty() {
        return match scope {
            Some("all") => "No shells to search — the daemon is not running any. Start one \
                 with run_terminal_command, then search its recorded output."
                .to_string(),
            _ => "No shells to search — this session owns none. Call list_shells \
                 to see the daemon's live shells, or scope \"all\"."
                .to_string(),
        };
    }

    // 3. Search each shell's recorded output in id order, capping the grand
    //    total at FIND_MAX_MATCHES. A shell whose recording cannot be read is
    //    skipped — one bad cast file must not abort the other shells' search —
    //    and is named in the output rather than swallowed.
    let mut hits: Vec<ShellHits> = Vec::new();
    let mut unsearchable: Vec<String> = Vec::new();
    let mut total = 0usize;

    for row in &rows {
        if total >= FIND_MAX_MATCHES {
            break;
        }
        let out = match read_recorded_output(reg, &row.id, None) {
            Ok(out) => out,
            Err(_) => {
                unsearchable.push(row.id.clone());
                continue;
            }
        };
        let tail = tail_lines(&out.text, READ_SHELL_MAX_LINES);
        let limit = FIND_MAX_MATCHES - total;
        let matches = search_shell_buffer(&tail, &re, limit);
        if !matches.is_empty() {
            total += matches.len();
            hits.push(ShellHits {
                id: row.id.clone(),
                matches,
            });
        }
    }
    hits.sort_by_key(|h| ShellId::parse(&h.id).map(|s| s.index()).unwrap_or(u32::MAX));
    unsearchable.sort_by_key(|i| ShellId::parse(i).map(|s| s.index()).unwrap_or(u32::MAX));

    if total == 0 {
        let mut msg = format!(
            "No recorded shell output matched /{pattern}/ (searched {} shell{}).",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" }
        );
        if !unsearchable.is_empty() {
            msg.push_str(&format!(
                " Skipped (no readable recording): {}.",
                unsearchable.join(", ")
            ));
        }
        return msg;
    }

    // 4. Render: shell id, 1-indexed line number, ±1 line of context.
    let mut out = format!(
        "{} match(es) for /{pattern}/ across {} shell(s):\n",
        total,
        hits.len()
    );
    for h in &hits {
        out.push_str(&format!(
            "\nshell {} — {} match(es):\n",
            h.id,
            h.matches.len()
        ));
        let mut body_parts = Vec::new();
        for m in &h.matches {
            if let Some(before) = &m.before {
                body_parts.push(format!("{:>5}- {}", m.line_no - 1, before));
            }
            body_parts.push(format!("{:>5}: {}", m.line_no, m.line));
            if let Some(after) = &m.after {
                body_parts.push(format!("{:>5}- {}", m.line_no + 1, after));
            }
        }
        out.push_str(&mask_sensitive(&body_parts.join("\n")));
    }
    if total >= FIND_MAX_MATCHES {
        out.push_str(&format!(
            "\n[capped at {total_matches} matches across all shells — narrow the pattern]",
            total_matches = FIND_MAX_MATCHES
        ));
    }
    if !unsearchable.is_empty() {
        out.push_str(&format!(
            "\n[{} shell(s) skipped — no readable recording: {}]",
            unsearchable.len(),
            unsearchable.join(", ")
        ));
    }
    out
}

/// The last `max` non-empty-collapsed lines of `text`.
fn tail_lines(text: &str, max: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max {
        return text.to_string();
    }
    lines[lines.len() - max..].join("\n")
}

// ---------------------------------------------------------------------------
// watch_shell — the blocking observer (M22 phase-04b)
// ---------------------------------------------------------------------------

/// How often `watch_shell` polls the cast file and the shell's state. The
/// removed `watch_pane` used the same interval — its shape proved it out.
pub(super) const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// How long `watch_shell` tolerates an `Idle` shell it has never seen
/// `Running`: a command sent to the PTY has not taken it yet — the removed
/// `watch_pane`'s start grace.
pub(super) const WATCH_START_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// `watch_shell`'s hard ceiling on `timeout_secs` — `await_agent_result`'s cap.
pub(super) const WATCH_MAX_SECS: u64 = 3600;

/// How many lines of what `watch_shell` saw it reports on a timeout or a
/// gone host.
pub(super) const WATCH_TAIL_LINES: usize = 50;

/// The string `read_recorded_output`, `close_shell_with` and `watch_shell` all show
/// for an id not in `reg` — one 03b sentence when the registry is empty, one
/// naming the live ids otherwise. Written once so the wording cannot drift
/// between callers.
pub(super) fn unknown_shell_error(reg: &ShellRegistry, id: &str) -> String {
    let live = live_shell_ids(reg);
    if live.is_empty() {
        format!(
            "Error: no shell with id {id:?} is in the registry. No shells are \
             live; call list_shells to confirm, or run a command first — \
             run_terminal_command creates a shell on demand."
        )
    } else {
        format!(
            "Error: no shell with id {id:?} is in the registry — it may have \
             exited and aged out, or the listing you are working from is \
             stale. Live shell ids: {}. Call list_shells to refresh.",
            live.join(", ")
        )
    }
}

/// Wait on a live shell's arriving output: with a `pattern`, until the (ANSI-
/// and carriage-return-normalised) output written **after this call** matches
/// it; without one, until the shell's running command finishes, the shell
/// turns out to have none, or it exits/pauses/dies/`timeout_secs` elapses.
///
/// `pub` rather than `pub(super)`: `knowledge/mod.rs` re-exports it for
/// `executor` the way `list_shells` and its siblings are re-exported, and a
/// `pub(super)` item cannot be given wider visibility on re-export.
pub async fn watch_shell(shell: &str, pattern: Option<&str>, timeout_secs: u64) -> String {
    watch_shell_with(
        crate::shell::registry(),
        shell,
        pattern,
        timeout_secs,
        WATCH_POLL,
        WATCH_START_GRACE,
    )
    .await
}

/// `watch_shell` with the registry, poll interval and start grace injected —
/// the seam the tests drive.
pub(super) async fn watch_shell_with(
    reg: &ShellRegistry,
    shell: &str,
    pattern: Option<&str>,
    timeout_secs: u64,
    poll: Duration,
    grace: Duration,
) -> String {
    // 1. Refusals first, cheapest first: an invalid pattern costs nothing.
    let re = match pattern.map(shell_tool_regex) {
        Some(Ok(re)) => Some(re),
        Some(Err(e)) => {
            return format!(
                "Error: invalid watch_shell pattern /{pattern:?}/: {e}. The pattern \
                 is a Rust `regex`-crate regular expression applied to the shell's \
                 new output, not a literal substring."
            );
        }
        None => None,
    };

    // 2. The id must be in the registry.
    let started_unix = with_shells(reg, |store| store.get(shell).map(|r| r.started_unix));
    let Some(record_started_unix) = started_unix else {
        return unknown_shell_error(reg, shell);
    };

    // 3. Resolve the cast by name, never by the record's `started_unix`: the
    //    record and the cast are stamped in different processes, and a spawn
    //    crossing a second boundary leaves `cast_path(id, record.started_unix)`
    //    naming no file.
    let cast = crate::shell::resolve_cast(shell, record_started_unix);
    let mut offset = match std::fs::metadata(&cast) {
        Ok(m) => m.len(),
        // No cast yet is not an error — the host creates it when it has
        // something to record.
        Err(_) => 0,
    };

    // 4. Poll until the pattern hits, the shell's command ends, or the
    //    timeout elapses. The whole loop runs inside one `tokio::time::timeout`
    //    like `await_agent_result`'s, so nothing here can run past the cap.
    let capped_secs = timeout_secs.min(WATCH_MAX_SECS);
    let deadline = tokio::time::Duration::from_secs(capped_secs);
    let started = std::time::Instant::now();
    let mut seen_running = false;
    let mut buf = String::new();
    let outcome = tokio::time::timeout(deadline, async {
        loop {
            let (raw, new_offset) =
                crate::shell::read_output_since(&cast, offset).unwrap_or((Vec::new(), offset));
            offset = new_offset;
            let text = visible_text(&String::from_utf8_lossy(&raw));
            buf.push_str(&text);
            buf.push('\n');
            if let Some(r) = re.as_ref()
                && r.is_match(&buf)
            {
                let matches = search_shell_buffer(&buf, r, 1);
                let rendered = render_match(matches.first());
                return WatchOutcome::Pattern(rendered);
            }
            if pattern.is_none() {
                match crate::shell::status(&socket_path(shell), SIGNAL_TIMEOUT).await {
                    Ok(crate::shell::ShellResponse::Status { state, .. }) => match state {
                        crate::shell::ShellState::Running => seen_running = true,
                        crate::shell::ShellState::Idle => {
                            if seen_running {
                                return WatchOutcome::Finished;
                            }
                            if started.elapsed() >= grace {
                                return WatchOutcome::NothingRunning;
                            }
                        }
                        crate::shell::ShellState::Paused => return WatchOutcome::Paused,
                        crate::shell::ShellState::Exited => return WatchOutcome::Exited,
                    },
                    Ok(_) => {}
                    Err(_) => return WatchOutcome::Unreachable,
                }
            }
            if !host_is_alive(&crate::shell::lock_path(shell)) {
                return WatchOutcome::HostGone;
            }
            tokio::time::sleep(poll).await;
        }
    })
    .await;

    let body = match outcome {
        Ok(WatchOutcome::Pattern(rendered_match)) => {
            format!(
                "watch_shell {shell}: matched {} after {} s. The line numbers are \
                 relative to the output seen since this call started, not the \
                 log's own.\n{}",
                pattern.unwrap_or_default(),
                capped_secs,
                rendered_match
            )
        }
        Ok(WatchOutcome::Finished) => {
            format!(
                "watch_shell {shell}: the command running in it finished after {} s.\n{}",
                capped_secs,
                tail_of(&buf, WATCH_TAIL_LINES)
            )
        }
        Ok(WatchOutcome::NothingRunning) => format!(
            "watch_shell {shell}: nothing was running in it for {} s, so there was \
             no command to wait for. Start the command (run_terminal_command with \
             `wait: false` creates a dedicated shell), then watch it. This \
             tool watches output that arrives after the call — read_shell reads \
             what was already recorded.",
            capped_secs
        ),
        Ok(WatchOutcome::Paused) => format!(
            "watch_shell {shell}: its job is stopped (SUSPENDED), so it will not \
             finish on its own. The tool returns now rather than burn the {} s.\n{}",
            capped_secs,
            tail_of(&buf, WATCH_TAIL_LINES)
        ),
        Ok(WatchOutcome::Exited) => format!(
            "watch_shell {shell}: the shell itself ended while watching.\n{}",
            tail_of(&buf, WATCH_TAIL_LINES)
        ),
        Ok(WatchOutcome::HostGone) => format!(
            "watch_shell {shell}: its host is gone.{}\n{}",
            if buf.trim().is_empty() {
                String::new()
            } else {
                " The tail of what it had written:".to_string()
            },
            tail_of(&buf, WATCH_TAIL_LINES)
        ),
        Ok(WatchOutcome::Unreachable) => format!(
            "watch_shell {shell}: the shell is unreachable — its socket exists in \
             the registry but the host did not answer a status round-trip. Do not \
             keep watching it; list_shells and close_shell are the tools that act \
             on a shell in this state."
        ),
        Err(_) => {
            if buf.trim().is_empty() {
                format!(
                    "watch_shell {shell}: timed out after {capped_secs} s. The shell \
                     produced no output after the call started. If you expected a \
                     match in output written before the call, that is not what this \
                     tool watches — read_shell reads what was already recorded."
                )
            } else {
                format!(
                    "watch_shell {shell}: timed out after {capped_secs} s without the \
                     pattern matching. What it saw:\n{}",
                    tail_of(&buf, WATCH_TAIL_LINES)
                )
            }
        }
    };
    mask_sensitive(&body).to_string()
}

/// The state that ended a `watch_shell` loop — a server-side summary of the
/// branch taken so the caller's sentence is written once, after the wait.
enum WatchOutcome {
    /// The pattern matched; carries the match rendered with ±1 line of
    /// context, because rendering must happen where the regex already is.
    Pattern(String),
    Finished,
    /// No command was running and none ever appeared within the grace.
    NothingRunning,
    /// The shell's foreground job was stopped and will not finish on its own.
    Paused,
    Exited,
    HostGone,
    /// A `status` round-trip failed — the socket is gone.
    Unreachable,
}

/// The one match rendered with its line and ±1 line of context, the shape
/// `find_in_shells` renders with (the same half-open edges, the same
/// `+/-` markers) so the two tools' bodies look alike.
fn render_match(m: Option<&ShellBufferMatch>) -> String {
    let Some(m) = m else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(before) = &m.before {
        out.push_str(&format!("{:>5}- {}", m.line_no - 1, before));
        out.push('\n');
    }
    out.push_str(&format!("{:>5}: {}", m.line_no, m.line));
    out.push('\n');
    if let Some(after) = &m.after {
        out.push_str(&format!("{:>5}- {}", m.line_no + 1, after));
        out.push('\n');
    }
    out
}

/// The last `max` non-empty-collapsed lines of `text` — the same shape
/// `tail_lines` computes; `buf` is cleared between reads so `tail_lines`
/// itself is reused where the text is already trimmed.
/// (Kept separate because `tail_lines` takes `&str` while `buf` accumulates.)
fn tail_of(text: &str, max: usize) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return "(the shell produced no output after the call started)".to_string();
    }
    tail_lines(trimmed, max)
}

/// Strip ANSI escape sequences: CSI, OSC and the short two-character forms.
fn strip_ansi(text: &str, re: &regex::Regex) -> String {
    re.replace_all(text, "").into_owned()
}

/// Raw PTY output → what a human reads: escape sequences gone, and each line
/// reduced to what follows its **last** carriage return, because a progress
pub(crate) fn visible_text(raw: &str) -> String {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let ansi = ANSI.get_or_init(|| {
        // INVARIANT: literal is a valid regex
        regex::Regex::new(concat!(
            r"\x1b\[[0-9;?]*[ -/]*[@-~]",           // CSI
            r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?", // OSC (BEL or ST)
            r"|\x1b[@-Z\\-_]",                      // two-character escape
        ))
        .expect("ANSI-strip regex is valid")
    });
    let stripped = strip_ansi(raw, ansi);
    stripped
        .lines()
        .map(|line| match line.rfind('\r') {
            Some(i) => &line[i + 1..],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The shell ids currently in `reg`, numerically sorted. `read_recorded_output`,
/// `close_shell_with` and the new `watch_shell` all need this; extracted here
/// at the third copy rather than a second round of three.
pub(super) fn live_shell_ids(reg: &ShellRegistry) -> Vec<String> {
    with_shells(reg, |store| {
        let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
        ids.sort_by_key(|i| ShellId::parse(i).map(|s| s.index()).unwrap_or(u32::MAX));
        ids
    })
}
