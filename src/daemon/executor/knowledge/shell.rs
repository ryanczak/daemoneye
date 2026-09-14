//! `list_shells` — the model-facing listing of the PTY shells the daemon is
//! running. Reads **only** the shell registry: no socket round-trips, no
//! screen-grid access and no asciicast reads — those are the buffer readers'
//! substrate (03b).

use crate::ai::filter::mask_sensitive;
use crate::shell::{
    Owner, ShellId, ShellRecord, ShellRegistry, await_ready, cast_path, host_is_alive,
    meta_path_for, socket_path, spawn_for, with_shells,
};
use std::time::{Duration, Instant};

/// One collected shell, shaped for the formatter. Holds the owner's `key()`
/// string rather than the `Owner` itself, so the formatter can never compare
/// inner ids across owner kinds.
pub(super) struct ShellRow {
    pub id: String,
    pub owner_key: String,
    pub started_unix: u64,
    pub interactive: bool,
    pub dedicated: bool,
    pub exited_unix: Option<u64>,
}

/// The numeric part of a shell id. The ordering key: `s2` must list before
/// `s10`, which comparing the id strings gets backwards.
struct ShellIndex(u32);

impl ShellIndex {
    fn new(id: &str) -> Self {
        ShellId::parse(id)
            .map(|parsed| ShellIndex(parsed.index()))
            .unwrap_or(ShellIndex(u32::MAX))
    }
}

/// Resolve `scope` against the registry and return the formatted listing.
///
/// `None` and `Some("mine")` list the shells `caller` owns; `Some("all")` lists
/// every live shell. Matching is on [`Owner::key`], so a chat session and a
/// ghost job that share an id string never see each other's shells. Any other
/// value — including `"run"`, `"session"`, `""` and casing variants — is an
/// error naming the bad value; it never falls back, because a fallback to
/// `"all"` would leak another session's shells on a typo and a fallback to
/// `"mine"` would silently answer a different question than the one asked.
///
/// `pub` rather than `pub(super)`: `knowledge/mod.rs` re-exports it for
/// `executor` the way every sibling module's functions are re-exported, and a
/// `pub(super)` item cannot be given wider visibility on re-export.
pub fn list_shells(scope: Option<&str>, caller: &Owner) -> String {
    list_shells_in(crate::shell::registry(), scope, caller, now_unix())
}

/// The same listing against an injected registry, with the clock injected with
/// it — the seam tests drive instead of the process-wide singleton.
fn list_shells_in(
    reg: &ShellRegistry,
    scope: Option<&str>,
    caller: &Owner,
    now_unix: u64,
) -> String {
    let rows = match resolve_scope(reg, scope, caller, "list_shells") {
        Ok(rows) => rows,
        Err(msg) => return msg,
    };
    format_shell_rows(&rows, scope, now_unix)
}

/// The `scope` match both shell tools share. `None` and `Some("mine")` select
/// the shells `caller` owns; `Some("all")` selects every live shell; anything
/// else is an error naming `tool` — the message must tell the model *whose*
/// argument was bad, so the same sentence shapes both tools' rejections
/// without hardcoding a name one of them did not use.
fn resolve_scope(
    reg: &ShellRegistry,
    scope: Option<&str>,
    caller: &Owner,
    tool: &str,
) -> Result<Vec<ShellRow>, String> {
    let key = caller.key();
    match scope {
        None | Some("mine") => Ok(with_shells(reg, |store| {
            store
                .values()
                .filter(|record| record.owner.key() == key)
                .map(collect_row)
                .collect::<Vec<_>>()
        })),
        Some("all") => Ok(with_shells(reg, |store| {
            store.values().map(collect_row).collect::<Vec<_>>()
        })),
        Some(other) => Err(format!(
            "Error: unknown {tool} scope {other:?}. Accepted values: \
             \"mine\" (this session's shells, the default) or \"all\" (every \
             shell the daemon is running)."
        )),
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Copy what the formatter needs out of a record. Runs inside `with_shells`,
/// so everything it costs is under the lock the collector already holds.
fn collect_row(record: &ShellRecord) -> ShellRow {
    ShellRow {
        id: record.id.as_str().to_string(),
        owner_key: record.owner.key(),
        started_unix: record.started_unix,
        interactive: record.interactive,
        dedicated: record.dedicated,
        exited_unix: record.exited_unix,
    }
}

/// Format already-collected rows. Pure: takes `now_unix` rather than reading
/// the clock and never sees the registry, so tests hand-build a `Vec<ShellRow>`
/// and assert on the text.
fn format_shell_rows(rows: &[ShellRow], scope: Option<&str>, now_unix: u64) -> String {
    let scope_label = match scope {
        Some("all") => "all",
        _ => "mine",
    };
    if rows.is_empty() {
        return format!(
            "No shells to show (scope \"{scope_label}\"): no shell records match this scope. \
             run_terminal_command creates them on demand."
        );
    }

    let mut sorted: Vec<&ShellRow> = rows.iter().collect();
    sorted.sort_by_key(|row| ShellIndex::new(&row.id).0);

    let mut out = format!(
        "{} shell{} (scope \"{scope_label}\"):\n",
        sorted.len(),
        if sorted.len() == 1 { "" } else { "s" }
    );
    for row in sorted {
        let age_part = if row.started_unix > 0 && now_unix >= row.started_unix {
            format!("  age:{}", format_age(now_unix - row.started_unix))
        } else {
            String::new()
        };
        let mut flags = String::new();
        if row.interactive {
            flags.push_str(" [interactive]");
        }
        if row.dedicated {
            flags.push_str(" [dedicated]");
        }
        if row.exited_unix.is_some() {
            flags.push_str(" [exited]");
        }
        out.push_str(&format!(
            "  {}  owner:{}{age_part}{flags}\n",
            row.id, row.owner_key
        ));
    }
    out
}

fn format_age(age_secs: u64) -> String {
    if age_secs < 60 {
        format!("{age_secs}s")
    } else if age_secs < 3600 {
        format!("{}m", age_secs / 60)
    } else if age_secs < 86_400 {
        format!("{}h{}m", age_secs / 3600, (age_secs % 3600) / 60)
    } else {
        format!("{}d", age_secs / 86_400)
    }
}

// ---------------------------------------------------------------------------
// read_shell (M22 phase-03b)
// ---------------------------------------------------------------------------

/// Default tail depth when `lines` is omitted. Same values as `read_pane`'s,
/// which is the analogue this tool's parameters were measured against.
const READ_SHELL_DEFAULT_LINES: usize = 200;
/// Hard ceiling on a single `read_shell` capture. Same value as `read_pane`'s.
const READ_SHELL_MAX_LINES: usize = 2000;

/// Pure: the command indices a reader should read. `since_command` selects
/// from that index onward (absent = every recorded command); no filesystem
/// access, no registry — it only sees the loaded `MetaIndex`.
fn select_command_indices(
    meta: &crate::shell::MetaIndex,
    since_command: Option<u32>,
) -> Option<Vec<u32>> {
    let Some(since) = since_command else {
        return Some(meta.commands.iter().map(|c| c.index).collect());
    };
    let highest = meta.commands.iter().map(|c| c.index).max();
    match highest {
        Some(h) if since <= h => Some(
            meta.commands
                .iter()
                .filter(|c| c.index >= since)
                .map(|c| c.index)
                .collect(),
        ),
        _ => None,
    }
}

/// One shell's recorded output — the text `read_shell` renders and what
/// `find_in_shells` searches, so a hit's line number means the same thing to
/// both.
struct ShellOutput {
    owner_key: String,
    /// How many commands the output was assembled from. Searchable text is the
    /// concatenation of a subset: empty output can mean either "recorded
    /// nothing yet" (0) or "the selected commands wrote nothing" (non-zero).
    command_count: usize,
    /// Raw (lossy-UTF-8) recorded output of the selected commands, in command
    /// order. Normalization lives with each caller's rendering.
    text: String,
}

/// The recorded-output read `read_shell` and `find_in_shells` share:
/// registry lookup → cast path → `MetaIndex` → byte-range read, decoded and
/// normalized. `since_command` selects a suffix of the commands; `None` reads
/// every one. `Err` carries the exact sentence the caller shows — five
/// distinct failure reasons end here, and a second copy of them would drift.
/// A live shell that has recorded no commands is a success with `command_count`
/// 0, not an error: a search over it finds nothing, while `read_shell` says so
/// in its own words.
fn read_shell_log(
    reg: &ShellRegistry,
    id: &str,
    since_command: Option<u32>,
) -> Result<ShellOutput, String> {
    let resolved = with_shells(reg, |store| {
        store
            .get(id)
            .map(|record| (record.started_unix, record.owner.key()))
    });
    let Some((started_unix, owner_key)) = resolved else {
        return Err(unknown_shell_error(reg, id));
    };
    let cast = cast_path(id, started_unix);
    let meta_path = meta_path_for(&cast);
    let Some(meta) = crate::shell::MetaIndex::load(&meta_path).ok() else {
        if std::fs::metadata(&cast).is_err() {
            return Err(format!(
                "Error: shell {id} ({owner_key}) has no recorded cast log at {} yet — \
                 nothing this shell ran has been recorded. Run a command in it, or \
                 check the shell survived restart.",
                meta_path.display()
            ));
        }
        return Err(format!(
            "Error: shell {id} ({owner_key}) has a cast log but its command index \
             {} is missing or unreadable. The command index is rebuilt by the \
             shell host; retry, or list_shells and use an id that reports commands.",
            meta_path.display()
        ));
    };
    if meta.commands.is_empty() {
        return Ok(ShellOutput {
            owner_key,
            command_count: 0,
            text: String::new(),
        });
    }
    let Some(indices) = select_command_indices(&meta, since_command) else {
        let highest = meta.commands.iter().map(|c| c.index).max().unwrap_or(0);
        return Err(format!(
            "Error: since_command {since:?} is beyond every recorded command \
             of shell {id} — the highest recorded index is {highest}. Call \
             with a smaller index (0 is always valid: it is the shell's \
             first recorded command).",
            since = since_command.unwrap_or_default(),
        ));
    };
    let mut bytes = Vec::new();
    for index in &indices {
        match crate::shell::read_command_output(&cast, &meta, *index) {
            Ok(mut out) => bytes.append(&mut out),
            Err(e) => {
                return Err(format!(
                    "Error: failed reading shell {id}'s recorded output at command \
                     index {index}: {e}. The cast file may have been rotated; \
                     list_shells and re-read."
                ));
            }
        }
    }
    Ok(ShellOutput {
        owner_key,
        command_count: indices.len(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Pure: turn the recorded output + the optional `lines` clamp and
/// `filter` into the body text. Everything above this function is I/O;
/// this is the part tests can hand-build. `Ok(None)` is the filtered-empty
/// case.
fn build_body(
    text: &str,
    lines: Option<u64>,
    filter: Option<&regex::Regex>,
) -> Result<Option<String>, String> {
    let matched: Vec<&str> = if let Some(re) = filter {
        text.lines().filter(|l| re.is_match(l)).collect()
    } else {
        text.lines().collect()
    };
    let matched: String = {
        let depth = compile_depth(lines);
        let start = matched.len().saturating_sub(depth);
        matched[start..].join("\n")
    };
    let trimmed = matched.trim_end();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(mask_sensitive(trimmed).to_string()))
}

/// Pure: the tail depth `lines` requests, with `read_pane`'s defaults (200,
/// capped at 2000, `0` or absent → default). Split from `build_body` so the
/// clamp arithmetic is assertable directly.
fn compile_depth(requested: Option<u64>) -> usize {
    match requested {
        Some(n) if n > 0 => (n as usize).min(READ_SHELL_MAX_LINES),
        _ => READ_SHELL_DEFAULT_LINES,
    }
}

/// The registry + cast-file half of `read_shell`. `registry` is
/// a parameter rather than a module read: `list_shells`'s `list_shells_in` seam,
/// so the lookup can be driven from a hand-built registry without touching the
/// process-global one.
fn read_shell_in(
    reg: &ShellRegistry,
    id: &str,
    lines: Option<u64>,
    since_command: Option<u32>,
    filter: Option<&str>,
) -> String {
    let out = match read_shell_log(reg, id, since_command) {
        Ok(out) => out,
        Err(msg) => return msg,
    };
    if out.command_count == 0 {
        return format!(
            "Shell {id} ({}) is live but has recorded no commands yet — \
             the recording starts with the first command run in it.",
            out.owner_key
        );
    }
    let filter_re = match filter {
        Some(p) => match regex::RegexBuilder::new(p).size_limit(1 << 20).build() {
            Ok(re) => Some(re),
            Err(e) => {
                return format!(
                    "Error: invalid filter regex {p:?}: {e}. Fix the expression and \
                     call again; the filter is applied as a regex, not a literal."
                );
            }
        },
        None => None,
    };
    let body = match build_body(&out.text, lines, filter_re.as_ref()) {
        Ok(body) => body,
        Err(msg) => return msg,
    };
    let header = format!(
        "Shell {id} ({}) — {} command{}{}{}:",
        out.owner_key,
        out.command_count,
        if out.command_count == 1 { "" } else { "s" },
        match since_command {
            Some(i) => format!(" from command index {i}"),
            None => String::new(),
        },
        match filter {
            Some(p) => format!(" matching /{p}/"),
            None => String::new(),
        },
    );
    match body {
        Some(b) => {
            let line_count = b.lines().count().max(1);
            format!(
                "{header} {line_count} line{}.\n{b}",
                if line_count == 1 { "" } else { "s" }
            )
        }
        None => {
            let depth = compile_depth(lines);
            if let Some(p) = filter {
                format!(
                    "{header} no lines matched /{p}/ in the last {depth} lines. \
                     Record the pattern without anchors, check the previous \
                     commands' output with list_shells, or use read_pane for a \
                     live pane."
                )
            } else {
                format!(
                    "{header} the recorded output is empty — nothing to show in the \
                     last {depth} lines."
                )
            }
        }
    }
}

/// Resolve a shell id to its recorded output: registry lookup → cast path →
/// meta index → byte-range slice → clamp → filter → mask. Splits like
/// `list_shells`: the registry and filesystem access live here, the selection
/// and formatting below it are pure and hand-buildable in tests.
pub fn read_shell(
    id: &str,
    lines: Option<u64>,
    since_command: Option<u32>,
    filter: Option<&str>,
) -> String {
    read_shell_in(crate::shell::registry(), id, lines, since_command, filter)
}

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

fn find_in_shells_in(
    reg: &ShellRegistry,
    pattern: &str,
    scope: Option<&str>,
    caller: &Owner,
) -> String {
    // 1. Build the regex first, before any lock or file read — an invalid
    //    pattern costs nothing and every shell stays unread.
    let re = match regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
    {
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
        let out = match read_shell_log(reg, &row.id, None) {
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

/// How often `watch_shell` polls the cast file and the shell's state.
/// `watch_pane`'s poll interval — its shape proved it out.
pub(super) const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// How long `watch_shell` tolerates an `Idle` shell it has never seen
/// `Running`: a command sent to the PTY has not taken it yet. `watch_pane`'s
/// start grace.
pub(super) const WATCH_START_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// `watch_shell`'s hard ceiling on `timeout_secs` — `await_agent_result`'s cap.
pub(super) const WATCH_MAX_SECS: u64 = 3600;

/// How many lines of what `watch_shell` saw it reports on a timeout or a
/// gone host.
pub(super) const WATCH_TAIL_LINES: usize = 50;

/// The string `read_shell_log`, `close_shell_with` and `watch_shell` all show
/// for an id not in `reg` — one 03b sentence when the registry is empty, one
/// naming the live ids otherwise. Written once so the wording cannot drift
/// between callers.
fn unknown_shell_error(reg: &ShellRegistry, id: &str) -> String {
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

/// The `read_shell`-sentence shape `watch_shell` returns for an id not in
/// `reg` — the shared 03b wording. A value, never `Err`.
fn watch_shell_unknown_error(reg: &ShellRegistry, shell: &str) -> String {
    unknown_shell_error(reg, shell)
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
    let re = match pattern.map(|p| regex::RegexBuilder::new(p).size_limit(1 << 20).build()) {
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
        return watch_shell_unknown_error(reg, shell);
    };

    // 3. Resolve the cast by name, never by the record's `started_unix`: the
    //    record and the cast are stamped in different processes, and a spawn
    //    crossing a second boundary leaves `cast_path(id, record.started_unix)`
    //    naming no file.
    let cast = resolve_live_cast(shell, record_started_unix);
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
             background=true creates a dedicated shell), then watch it. This \
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

/// The cast file a live shell's host is writing right now: the highest
/// `started_unix` any `<id>-<ts>.cast` in the logs dir names for this id,
/// falling back to `cast_path(id, record.started_unix)`. The fallback covers a
/// host whose logs dir has not been written at all yet.
fn resolve_live_cast(id: &str, record_started_unix: u64) -> std::path::PathBuf {
    let logs = crate::config::shell_logs_dir();
    let mut best: Option<u64> = None;
    if let Ok(entries) = std::fs::read_dir(&logs) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some((pid, ts)) = crate::shell::parse_cast_name(&name.to_string_lossy())
                && pid.as_str() == id
                && best.is_none_or(|b| ts > b)
            {
                best = Some(ts);
            }
        }
    }
    match best {
        Some(ts) => cast_path(id, ts),
        None => cast_path(id, record_started_unix),
    }
}

/// Strip ANSI escape sequences: CSI, OSC and the short two-character forms.
fn strip_ansi(text: &str, re: &regex::Regex) -> String {
    re.replace_all(text, "").into_owned()
}

/// Raw PTY output → what a human reads: escape sequences gone, and each line
/// reduced to what follows its **last** carriage return, because a progress
fn visible_text(raw: &str) -> String {
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

/// The shell ids currently in `reg`, numerically sorted. `read_shell_log`,
/// `close_shell_with` and the new `watch_shell` all need this; extracted here
/// at the third copy rather than a second round of three.
fn live_shell_ids(reg: &ShellRegistry) -> Vec<String> {
    with_shells(reg, |store| {
        let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
        ids.sort_by_key(|i| ShellId::parse(i).map(|s| s.index()).unwrap_or(u32::MAX));
        ids
    })
}
// open_shell + close_shell (M22 phase-04a)
// ---------------------------------------------------------------------------

/// The default readiness timeout for a freshly spawned shell host.
pub(super) const SHELL_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// How long `send_signal` may wait for the host to acknowledge the frame.
/// Short on purpose: the acknowledgement is a socket round-trip, not the job
/// reacting to the signal.
const SIGNAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The PTY geometry this module's test host spawns with — the same 24×80
/// `shell_exec` uses, so both fixtures produce interchangeable hosts.
const TEST_ROWS: u16 = 24;
/// The PTY geometry this module's test host spawns with.
const TEST_COLS: u16 = 80;

/// Renders an anyhow chain as `top-level (source) (source)` — the shape
/// `open_shell`'s readiness and cap errors use — and is plain for a chainless
/// error.
fn format_err_chain(e: &anyhow::Error) -> String {
    let mut msg = e.to_string();
    for cause in e.chain().skip(1) {
        msg.push_str(&format!(" ({cause})"));
    }
    msg
}

/// Open a new PTY shell for `caller` and return the sentence the model reads.
///
/// The public form uses the daemon's registry, the configured
/// `[shells] max_per_owner` and the standard readiness timeout; the `_in`
/// form takes all three plus the spawn step, mirroring how `list_shells` /
/// `list_shells_in` already split.
pub async fn open_shell(caller: &Owner) -> String {
    let max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    open_shell_with(
        crate::shell::registry(),
        caller,
        max_per_owner,
        SHELL_READY_TIMEOUT,
        |id| crate::shell::spawn_detached_host(id, TEST_ROWS, TEST_COLS),
    )
    .await
}

/// `open_shell` with every effect injected: the registry, the per-owner cap,
/// the readiness timeout and the spawn step. Tests replace `spawn` with an
/// in-process host because `spawn_detached_host` re-invokes `current_exe()`,
/// which under a test binary is not the daemon (07b F4).
pub(super) async fn open_shell_with<F, Fut>(
    reg: &ShellRegistry,
    caller: &Owner,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> String
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // Neither reuse helper is used: the reuse scan would hand the same id
    // back twice, and the dedicated path sets `dedicated: true`, which
    // excludes a shell from that scan forever. The `false` below is what
    // keeps a freshly opened shell reachable by the reuse rule.
    let id = match spawn_for(reg, caller.clone(), max_per_owner, false, spawn).await {
        Ok(id) => id,
        // The per-owner cap lives in `spawn_for`'s `check_owner_cap` breach,
        // returned as text rather than an `Err`: every tool result is a value
        // the model reads, so the refusal must name the configured limit.
        Err(e) => return open_shell_error(max_per_owner, &e),
    };
    // `spawn_for` reserved the record and rolled back only a *spawn* failure,
    // so a host that never came up leaves a record behind that would be
    // handed to the next caller as a live shell. Undo the reservation here.
    let socket = socket_path(id.as_str());
    if let Err(e) = await_ready(&socket, Instant::now() + ready_timeout).await {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return format!(
            "Error: shell {} did not become ready, and its empty registry record \
             was removed, so nothing can reach a dead shell: {}.",
            id.as_str(),
            format_err_chain(&e)
        );
    }
    let s = format!(
        "Opened shell {}. It is now registered and readable — list_shells lists \
         it, read_shell and find_in_shells read its recorded output, close_shell \
         ends it. Note: a command that needs a shell reuses this session's \
         lowest-numbered open shell, which may not be the one just opened — if \
         s3 is already open, commands keep going to s3.",
        id.as_str()
    );
    s
}

/// The cap-breach sentence for `open_shell`. `max_per_owner == 0` means
/// unlimited, so the limit the text names must be the message's own wording,
/// not a number that reads as a finite cap.
fn open_shell_error(max_per_owner: u32, e: &anyhow::Error) -> String {
    let limit = if max_per_owner == 0 {
        "unlimited".to_string()
    } else {
        format!("{max_per_owner}")
    };
    format!(
        "Error: could not open a shell: {} — the per-owner shell limit is \
         {limit}. Close one of your shells with close_shell, or stop asking for \
         new shells this turn.",
        format_err_chain(e)
    )
}

/// Close one of `caller`'s PTY shells and return the sentence the model reads.
///
/// The public form uses the daemon's registry and the standard signal
/// timeout; the `_in` form takes both, mirroring the `list_shells` /
/// `list_shells_in` split.
pub async fn close_shell(shell: &str, caller: &Owner) -> String {
    close_shell_with(crate::shell::registry(), shell, caller, SIGNAL_TIMEOUT).await
}

/// `close_shell` with the registry and the signal timeout injected.
pub(super) async fn close_shell_with(
    reg: &ShellRegistry,
    shell: &str,
    caller: &Owner,
    signal_timeout: Duration,
) -> String {
    // Ownership first, and on `Owner::key()` rather than the inner string: a
    // chat session and a ghost job can share an id string, and the kinds are
    // what the cap groups, so they are what the owner check must compare.
    let owner_in_registry: Option<String> = with_shells(reg, |store| {
        store.get(shell).map(|record| record.owner.key())
    });
    let Some(owner_key) = owner_in_registry else {
        let live = live_shell_ids(reg);
        return if live.is_empty() {
            format!(
                "Error: no shell with id {shell:?} is in the registry (no shells are \
                 live — call list_shells to confirm)."
            )
        } else {
            format!(
                "Error: no shell with id {shell:?} is in the registry. Live shell ids: \
                 {}. Call list_shells to refresh.",
                live.join(", ")
            )
        };
    };
    if owner_key != caller.key() {
        return format!(
            "Error: shell {shell} is not yours (it belongs to {owner_key}), so this \
             session cannot close it. If it needs closing, the session that owns \
             it does that; list_shells shows which shells are whose."
        );
    }
    // The wire carries only Int | Term | Stop | Cont — there is no SIGHUP and
    // no kill frame, so Term is the strongest closure this tool can send, and
    // it is sent exactly once. Whether the shell actually leaves is observed
    // afterwards, not assumed; a host that lingers still holds the lock, so
    // the tool says so instead of claiming success.
    let socket = socket_path(shell);
    if let Err(e) =
        crate::shell::send_signal(&socket, crate::shell::ShellSignal::Term, signal_timeout).await
    {
        return format!(
            "Error: shell {shell} could not be signalled, so it is still open: \
             {}. The shell is unchanged; list_shells shows its state, and a \
             later close_shell attempt is safe. If the socket is gone but the \
             shell still lists, its host may have died without the registry \
             noticing.",
            format_err_chain(&e)
        );
    }
    // The signal is one-way; what closes the shell is the host's PTY ending.
    // Give it a bounded moment and observe, rather than asserting either way.
    let lock = crate::shell::lock_path(shell);
    closed_by_observed_end(shell, &lock)
}

/// The observable outcome of one close: poll the lock up to 1.5s from now
/// and report what is actually seen. **`host_is_alive` can never turn false
/// while the still-bound socket holds the host's task alive in-process** — the
/// fixture's host task never exits — so this observe-poll genuinely produces
/// "still alive" in tests and "gone" in production, and the tool's sentence
/// must not claim a closure it did not see.
fn closed_by_observed_end(shell: &str, lock: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        if !host_is_alive(lock) {
            return format!(
                "Closed shell {shell}: its host is gone and the shell is no longer live."
            );
        }
        if std::time::Instant::now() >= deadline {
            return format!(
                "Sent TERM to shell {shell}, but its host is still holding its lock — \
                 the shell may still be finishing (there is no kill frame on this \
                 wire, so escalation is not possible). list_shells still shows it; \
                 check again before assuming it is closed."
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
pub(crate) mod lifecycle_test_support {
    //! Shared by `shell_exec.rs` and `bg_shell.rs`'s own copies — not by this
    //! module's tests, which use the module-local `spawn_in_process_host`. A
    //! fourth caller is the point where the three copies are expected to be
    //! lifted here.

    use anyhow::Context;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// The PTY geometry the in-process test host uses — the same 24×80 the
    /// daemon's real spawn uses.
    pub const ROWS: u16 = 24;
    /// The PTY geometry the in-process test host uses.
    pub const COLS: u16 = 80;

    /// A real in-process PTY host serving the configured socket for `id`: a
    /// bash session whose `.cast` the returned path points at, with the
    /// per-id instance lock held (leaked) so `host_is_alive` reads it as live
    /// exactly as a real host would. `spawn_detached_host` cannot run under a
    /// test (07b F4).
    pub async fn spawn_with_cast(id: crate::shell::ShellId) -> anyhow::Result<PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::shell::ShellRegistry;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// A throwaway `HOME` so `cast_path` (which resolves through
    /// `shell_logs_dir()`) lands inside a temp directory the test owns. Every
    /// read_shell test holds this for its whole body, so a module-level
    /// `std::path` touch never reaches the real home.
    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("de_kshell_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
    }
    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_home<F: FnOnce() -> R, R>(tmp: &TmpHome, f: F) -> R {
        let _guard = crate::test_home_guard();
        let old = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp.0);
        }
        let out = f();
        match old {
            Some(v) => unsafe {
                std::env::set_var("HOME", v);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
        out
    }

    fn row(
        id: &str,
        owner_key: &str,
        started_unix: u64,
        interactive: bool,
        dedicated: bool,
        exited_unix: Option<u64>,
    ) -> ShellRow {
        ShellRow {
            id: id.to_string(),
            owner_key: owner_key.to_string(),
            started_unix,
            interactive,
            dedicated,
            exited_unix,
        }
    }

    fn insert(reg: &ShellRegistry, index: u32, owner: Owner, started_unix: u64) {
        with_shells(reg, |store| {
            store.insert(
                format!("s{index}"),
                ShellRecord {
                    id: ShellId::from_index(index),
                    owner,
                    started_unix,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                },
            );
        });
    }

    const NOW: u64 = 200_000;

    /// A real in-process PTY host serving the configured socket for `id`
    /// (third copy of the fixture; see `lifecycle_test_support`). The `.cast`
    /// path the host writes is deliberately leaked with it, like the
    /// `shell_exec`/`bg_shell` originals.
    async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
        let cast = lifecycle_test_support::spawn_with_cast(id.clone()).await?;
        std::mem::forget(cast);
        Ok(())
    }

    /// A spawn substitute that resolves successfully but binds no socket: the
    /// shape of a host that reserved its record and then never came up.
    async fn spawn_never_binds(_id: ShellId) -> anyhow::Result<()> {
        Ok(())
    }

    /// A spawn substitute that records the fact it ran. Used to prove a cap
    /// breach is refused *before* any spawn is attempted.
    fn counting_spawn<F, Fut>(ran: &'static AtomicBool, inner: F) -> F
    where
        F: FnOnce(ShellId) -> Fut,
    {
        let _ = (&ran, &inner);
        inner
    }

    /// Whether the record `id` is in `reg`.
    fn has_record(reg: &ShellRegistry, id: &str) -> bool {
        with_shells(reg, |store| store.contains_key(id))
    }

    /// Point `HOME` at a fresh temp dir **and** create the dirs the shell
    /// substrate resolves through it — the same shape `shell_exec`'s
    /// `test_home()` gives, so `socket_path`/`lock_path` land inside the
    /// sandbox. Must be held for the test's whole body.
    pub(super) fn lifecycle_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        (guard, dir)
    }

    #[test]
    fn list_shells_defaults_to_the_callers_own_shells() {
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("a".into()), NOW - 100);
        insert(&reg, 4, Owner::Ghost("a".into()), NOW - 100);
        insert(&reg, 5, Owner::Chat("b".into()), NOW - 100);
        let out = list_shells_in(&reg, None, &Owner::Chat("a".into()), NOW);
        assert!(out.contains("s3"), "chat a's own shell must appear: {out}");
        assert!(
            !out.contains("s4"),
            "ghost a is a different owner key: {out}"
        );
        assert!(!out.contains("s5"), "b's shell must not leak to a: {out}");
    }

    #[test]
    fn list_shells_all_scope_includes_other_owners() {
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("a".into()), NOW - 100);
        insert(&reg, 5, Owner::Chat("b".into()), NOW - 100);
        insert(&reg, 9, Owner::Adopted, NOW - 100);
        let out = list_shells_in(&reg, Some("all"), &Owner::Chat("a".into()), NOW);
        assert!(out.contains("s3"), "own shell listed under all: {out}");
        assert!(out.contains("s5"), "another session's shell listed: {out}");
        assert!(
            out.contains("adopted"),
            "adopted shells list under all: {out}"
        );
    }

    #[test]
    fn list_shells_rejects_an_unknown_scope() {
        for bad in ["run", "session", ""] {
            let out = list_shells_in(
                &ShellRegistry::default(),
                Some(bad),
                &Owner::Chat("a".into()),
                NOW,
            );
            assert!(
                out.starts_with("Error:") && out.contains(bad),
                "scope {bad:?} must be rejected with the value named: {out}"
            );
            assert!(
                !out.contains("s3"),
                "a rejected scope must not list any shell: {out}"
            );
        }
    }

    #[test]
    fn list_shells_scope_match_is_case_sensitive() {
        for bad in ["MINE", "All", "ALL", "Mine"] {
            let out = list_shells_in(
                &ShellRegistry::default(),
                Some(bad),
                &Owner::Chat("a".into()),
                NOW,
            );
            assert!(
                out.starts_with("Error:"),
                "{bad:?} is not an accepted alias — matching is case-sensitive: {out}"
            );
        }
    }

    #[test]
    fn list_shells_orders_numerically_not_lexicographically() {
        let reg = ShellRegistry::default();
        insert(&reg, 10, Owner::Chat("a".into()), NOW - 200);
        insert(&reg, 2, Owner::Chat("a".into()), NOW - 100);
        let out = list_shells_in(&reg, None, &Owner::Chat("a".into()), NOW);
        let s2 = out.find("  s2 ").expect("s2 must be listed");
        let s10 = out.find("  s10 ").expect("s10 must be listed");
        assert!(s2 < s10, "s2 must sort before s10, got: {out}");
    }

    #[test]
    fn list_shells_empty_registry_says_so() {
        let reg = ShellRegistry::default();
        let out = list_shells_in(&reg, None, &Owner::Chat("a".into()), NOW);
        assert!(
            out.starts_with("No shells") && out.ends_with("demand."),
            "an empty registry needs a sentence, not a bare header or empty string: {out}"
        );

        // Only *another* owner's shells present: `mine` is empty for this caller.
        insert(&reg, 7, Owner::Chat("b".into()), NOW - 100);
        let out = list_shells_in(&reg, Some("mine"), &Owner::Chat("a".into()), NOW);
        assert!(
            out.contains("No shells"),
            "another owner's shells must not satisfy mine: {out}"
        );
        assert!(!out.contains("s7"));
    }

    #[test]
    fn formatter_marks_flags_and_exits_without_the_registry() {
        let rows = vec![
            row("s3", "chat:a", NOW - 100, false, false, None),
            row("s4", "ghost:a", 0, true, true, Some(NOW - 10)),
        ];
        let out = format_shell_rows(&rows, Some("all"), NOW);
        assert!(out.contains("[interactive]"), "flag carried: {out}");
        assert!(out.contains("[dedicated]"), "flag carried: {out}");
        assert!(
            out.contains("[exited]"),
            "an exited shell is kept visible: {out}"
        );
        assert!(out.contains("owner:ghost:a"), "owner key shown: {out}");
        assert!(out.contains("2 shells"), "count header: {out}");
        assert!(out.contains("age:1m"), "age from started_unix: {out}");
        // The s4 line has no age part: started_unix 0 is not a real start time.
        let s4 = out.find("  s4 ").expect("s4 listed");
        let s4_line = out[s4..].lines().next().unwrap();
        assert!(
            !s4_line.contains("age:"),
            "no age when started_unix is 0: {s4_line}"
        );
    }

    #[test]
    fn formatter_reports_a_single_shell_without_plural_splitting() {
        let rows = vec![row("s1", "chat:a", NOW - 30, false, false, None)];
        let out = format_shell_rows(&rows, None, NOW);
        assert!(out.contains("1 shell (scope \"mine\")"), "get: {out}");
        assert!(out.contains("age:30s"), "age: {out}");
    }

    /// A shell with `per_command` recorded commands, each writing `output`,
    /// plus the registry record pointing at the cast. Returns the `TmpHome`
    /// keeping the files alive, the registry (already holding the record), and
    /// the id the fixture wrote. Command output is one line per command.
    ///
    /// The whole fixture runs with `HOME` pointed at the temp dir, because
    /// `cast_path` resolves through `shell_logs_dir()` — a fixed id +
    /// `started_unix` pair always names the same file per `HOME`, so between
    /// two tests running concurrently the writes would otherwise collide.
    fn fixture(per_command: &[&str]) -> (TmpHome, ShellRegistry, String) {
        let tmp = TmpHome::new();
        let reg = ShellRegistry::default();
        with_home(&tmp, || {
            let id = "s3";
            let started_unix = 1_788_470_000;
            let cast = crate::shell::cast_path(id, started_unix);
            std::fs::create_dir_all(cast.parent().expect("cast has a parent")).unwrap();

            let mut w = crate::shell::CastWriter::create(&cast, 80, 24, started_unix).unwrap();
            let mut records = Vec::new();
            for (i, out) in per_command.iter().enumerate() {
                let t = 1.0 + i as f64;
                w.mark(Duration::from_secs_f64(t), &format!("cmd {i}"))
                    .unwrap();
                let first_byte = w.byte_len();
                w.write_output(
                    Duration::from_secs_f64(t + 0.5),
                    format!("{out}\n").as_bytes(),
                )
                .unwrap();
                let end_byte = w.byte_len();
                w.mark(Duration::from_secs_f64(t + 1.0), &format!("end {i}"))
                    .unwrap();
                records.push(crate::shell::CommandRecord {
                    index: i as u32,
                    command: format!("cmd {i}"),
                    started: t,
                    ended: t + 0.5,
                    exit_code: 0,
                    first_byte,
                    end_byte,
                });
            }
            drop(w);
            let meta = crate::shell::MetaIndex {
                shell_id: id.into(),
                cast: cast.file_name().unwrap().to_string_lossy().into_owned(),
                started_unix,
                commands: records,
            };
            let meta_path = crate::shell::meta_path_for(&cast);
            meta.save(&meta_path).unwrap();

            insert(&reg, 3, Owner::Chat("a".into()), started_unix);
        });
        (tmp, reg, "s3".to_string())
    }

    /// Register a shell whose recording holds `per_command`, one line each.
    /// `started_unix` is derived from the index so two ids never share a cast
    /// file. Must be called with `tmp` already installed as `HOME` (via
    /// `with_home`) because `cast_path` resolves through `shell_logs_dir()`.
    fn fixture_into(
        tmp: &TmpHome,
        reg: &mut ShellRegistry,
        index: u32,
        owner: Owner,
        per_command: &[&str],
    ) {
        let id = format!("s{index}");
        let started_unix = 1_788_470_000u64 + index as u64;
        let cast = crate::shell::cast_path(&id, started_unix);
        std::fs::create_dir_all(cast.parent().expect("cast has a parent")).unwrap();

        let mut w = crate::shell::CastWriter::create(&cast, 80, 24, started_unix).unwrap();
        let mut records = Vec::new();
        for (i, out) in per_command.iter().enumerate() {
            let t = 1.0 + i as f64;
            w.mark(Duration::from_secs_f64(t), &format!("cmd {i}"))
                .unwrap();
            let first_byte = w.byte_len();
            w.write_output(
                Duration::from_secs_f64(t + 0.5),
                format!("{out}\n").as_bytes(),
            )
            .unwrap();
            let end_byte = w.byte_len();
            w.mark(Duration::from_secs_f64(t + 1.0), &format!("end {i}"))
                .unwrap();
            records.push(crate::shell::CommandRecord {
                index: i as u32,
                command: format!("cmd {i}"),
                started: t,
                ended: t + 0.5,
                exit_code: 0,
                first_byte,
                end_byte,
            });
        }
        drop(w);
        let meta = crate::shell::MetaIndex {
            shell_id: id.clone(),
            cast: cast.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix,
            commands: records,
        };
        let meta_path = crate::shell::meta_path_for(&cast);
        meta.save(&meta_path).unwrap();

        insert(reg, index, owner, started_unix);
        let _ = tmp;
    }

    /// Two shells in one registry under one HOME: shell `index_a` owned by
    /// `owner_a`, shell `index_b` owned by `owner_b`.
    fn fixture_two(
        index_a: u32,
        owner_a: Owner,
        lines_a: &[&str],
        index_b: u32,
        owner_b: Owner,
        lines_b: &[&str],
    ) -> (TmpHome, ShellRegistry) {
        let tmp = TmpHome::new();
        let mut reg = ShellRegistry::default();
        with_home(&tmp, || {
            fixture_into(&tmp, &mut reg, index_a, owner_a, lines_a);
            fixture_into(&tmp, &mut reg, index_b, owner_b, lines_b);
        });
        (tmp, reg)
    }

    /// Delete the cast + meta files of the shell `id` in `reg`, leaving the
    /// registry entry live — the unreadable-recording shape.
    fn remove_cast(reg: &ShellRegistry, id: &str) {
        let started = with_shells(reg, |store| store.get(id).map(|r| r.started_unix));
        if let Some(started) = started {
            let cast = crate::shell::cast_path(id, started);
            let _ = std::fs::remove_file(&cast);
            let _ = std::fs::remove_file(crate::shell::meta_path_for(&cast));
        }
    }

    #[test]
    fn find_in_shells_reports_matches_with_context() {
        let (tmp, reg) = fixture_two(
            3,
            Owner::Chat("a".into()),
            &["one", "needle here", "three"],
            4,
            Owner::Chat("a".into()),
            &["unrelated"],
        );
        let out = with_home(&tmp, || {
            find_in_shells_in(&reg, "needle", None, &Owner::Chat("a".into()))
        });
        assert!(out.contains("s3"), "shell id in match output: {out}");
        assert!(
            out.contains("needle here"),
            "the matching line itself: {out}"
        );
        assert!(out.contains("one"), "line before is context: {out}");
        assert!(out.contains("three"), "line after is context: {out}");
        assert!(out.contains("2"), "1-indexed line number present: {out}");
    }

    #[test]
    fn find_in_shells_searches_only_scoped_shells() {
        let (tmp, reg) = fixture_two(
            3,
            Owner::Chat("a".into()),
            &["own needle line"],
            4,
            Owner::Chat("b".into()),
            &["foreign needle line"],
        );
        let out = with_home(&tmp, || {
            find_in_shells_in(&reg, "needle", None, &Owner::Chat("a".into()))
        });
        assert!(
            out.contains("s3") && out.contains("own needle line"),
            "caller's own hit appears: {out}"
        );
        assert!(
            !out.contains("foreign needle line"),
            "another owner's hit is absent under scope mine: {out}"
        );
        let all = with_home(&tmp, || {
            find_in_shells_in(&reg, "needle", Some("all"), &Owner::Chat("a".into()))
        });
        assert!(
            all.contains("own needle line") && all.contains("foreign needle line"),
            "scope all sees both owners: {all}"
        );
    }

    #[test]
    fn find_in_shells_caps_total_matches_across_shells() {
        let lines: Vec<String> = (0..30).map(|i| format!("hit needle {i}")).collect();
        let owned: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, reg) = fixture_two(
            3,
            Owner::Chat("a".into()),
            &owned,
            4,
            Owner::Chat("a".into()),
            &owned,
        );
        let out = with_home(&tmp, || {
            find_in_shells_in(&reg, "needle", None, &Owner::Chat("a".into()))
        });
        assert!(out.contains("[capped at 50 matches"), "cap reported: {out}");
        let match_lines = out.matches(": hit needle").count();
        assert!(
            match_lines == 50,
            "exactly 50 match lines across both shells, got {match_lines}: {out}"
        );
        assert!(
            out.matches("shell s3 —").count() == 1 && out.matches("shell s4 —").count() == 1,
            "both shells contributed: {out}"
        );
    }

    #[test]
    fn find_in_shells_skips_an_unreadable_shell() {
        let (tmp, reg) = fixture_two(
            3,
            Owner::Chat("a".into()),
            &["good needle one"],
            4,
            Owner::Chat("a".into()),
            &["good needle two"],
        );
        // Delete shell s4's cast file while its HOME is installed: the
        // registry entry stays live but the recording is gone — the
        // unreadable-shell shape.
        with_home(&tmp, || remove_cast(&reg, "s4"));
        let out = with_home(&tmp, || {
            find_in_shells_in(&reg, "needle", None, &Owner::Chat("a".into()))
        });
        assert!(
            out.contains("good needle one"),
            "the readable shell's match survives a sibling's bad cast: {out}"
        );
        assert!(
            out.contains("s4") && out.contains("skipped"),
            "the skipped shell is named: {out}"
        );
    }

    #[test]
    fn find_in_shells_invalid_pattern_is_an_error() {
        let (tmp, reg) = fixture_two(
            3,
            Owner::Chat("a".into()),
            &["alpha"],
            4,
            Owner::Chat("a".into()),
            &["beta"],
        );
        let out = with_home(&tmp, || {
            find_in_shells_in(&reg, "[", None, &Owner::Chat("a".into()))
        });
        assert!(out.contains("Error:"), "an error, not a result: {out}");
        assert!(
            out.contains('['),
            "the pattern is named in the error: {out}"
        );
        assert!(
            !out.contains("alpha") && !out.contains("beta"),
            "no shell content leaks on an error path: {out}"
        );
    }

    #[test]
    fn find_in_shells_no_matches_differs_from_no_shells() {
        let (tmp, reg) = fixture_two(
            3,
            Owner::Chat("a".into()),
            &["innocent"],
            4,
            Owner::Chat("a".into()),
            &["more innocent"],
        );
        let no_match = with_home(&tmp, || {
            find_in_shells_in(&reg, "zzz-no-match", None, &Owner::Chat("a".into()))
        });
        assert!(
            no_match.contains("No recorded shell output matched"),
            "searched-but-empty says so: {no_match}"
        );
        assert!(
            no_match.contains("searched 2 shells"),
            "the shell count is stated: {no_match}"
        );

        let empty = ShellRegistry::default();
        let no_shells = find_in_shells_in(&empty, "zzz-no-match", None, &Owner::Chat("a".into()));
        assert!(
            no_shells.contains("No shells to search"),
            "nothing-to-search is its own sentence: {no_shells}"
        );
        assert_ne!(
            no_match.split('\n').next(),
            no_shells.split('\n').next(),
            "the two sentences differ"
        );
    }

    #[test]
    fn read_shell_returns_recorded_output() {
        let (tmp, reg, id) = fixture(&["alpha-1", "alpha-2"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, None, None, None);
            assert!(out.contains("alpha-1"), "command 1's output present: {out}");
            assert!(out.contains("alpha-2"), "command 2's output present: {out}");
            assert!(out.contains("s3"), "the id is named in the header: {out}");
        });
    }

    #[test]
    fn read_shell_since_command_selects_from_that_index() {
        let (tmp, reg, id) = fixture(&["one", "two", "three"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, None, Some(2), None);
            assert!(
                out.contains("three"),
                "command 3 must be included (index >= 2): {out}"
            );
            assert!(
                !out.contains("two") && !out.contains("one"),
                "commands 0 and 1 must be excluded: {out}"
            );
        });
    }

    #[test]
    fn read_shell_since_command_past_the_end_says_so() {
        let (tmp, reg, id) = fixture(&["only"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, None, Some(9), None);
            assert!(
                out.starts_with("Error:"),
                "out-of-range says so as an error the model reads: {out}"
            );
            assert!(
                out.contains("9"),
                "the requested index must be named: {out}"
            );
            assert!(
                out.contains("highest recorded index is 0"),
                "the highest recorded index must be named: {out}"
            );
        });
    }

    #[test]
    fn read_shell_filter_selects_matching_lines() {
        let (tmp, reg, id) = fixture(&["alpha-noise", "beta-treasure", "alpha-noise-2"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, None, None, Some("treasure"));
            assert!(
                out.contains("beta-treasure"),
                "the matching line is present: {out}"
            );
            assert!(
                !out.contains("alpha-noise"),
                "non-matching lines are excluded: {out}"
            );
        });

        // A regex that matches nothing: a sentence, not an empty body.
        let (tmp, reg, id) = fixture(&["plenty", "of", "output"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, None, None, Some("never-present"));
            assert!(
                out.contains("no lines matched /never-present/"),
                "filtered-empty is a sentence naming the pattern: {out}"
            );
            assert!(
                out.contains("Record the pattern"),
                "the sentence must say what to do next, not just that nothing matched: {out}"
            );
            assert!(
                !out.contains("plenty"),
                "nothing matched, so no shell content is returned: {out}"
            );
        });
    }

    #[test]
    fn read_shell_invalid_filter_regex_is_an_error() {
        let (tmp, reg, id) = fixture(&["has [ in it", "clean line"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, None, None, Some("["));
            assert!(
                out.starts_with("Error:"),
                "a malformed regex is an error, not a fallback: {out}"
            );
            assert!(out.contains("["), "the offending pattern is named: {out}");
            assert!(
                out.contains("character class") || out.contains("unterminated"),
                "the parse error is named: {out}"
            );
            assert!(
                !out.contains("clean line"),
                "no shell content may accompany a parse failure: {out}"
            );
        });
    }

    #[test]
    fn read_shell_unknown_id_is_an_error_naming_live_shells() {
        let (tmp, reg, _id) = fixture(&["present"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, "s99", None, None, None);
            assert!(
                out.starts_with("Error:"),
                "an unknown id is an error, returned as a value not a Result: {out}"
            );
            assert!(out.contains("s99"), "the bad id is named: {out}");
            assert!(
                out.contains("s3"),
                "the live ids are named so the model knows what to pass: {out}"
            );
        });

        // An empty registry: different sentence, same error contract.
        let empty = ShellRegistry::default();
        with_home(&tmp, || {
            let out = read_shell_in(&empty, "s99", None, None, None);
            assert!(
                out.starts_with("Error:"),
                "even with nothing live, the answer is a sentence: {out}"
            );
            assert!(
                out.contains("s99") && out.contains("No shells"),
                "the message names both the bad id and the empty registry: {out}"
            );
        });
    }

    #[test]
    fn read_shell_lines_clamps_to_the_tail() {
        let (tmp, reg, id) = fixture(&["line-1", "line-2", "line-3", "line-4", "line-5"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, Some(2), None, None);
            assert!(
                out.contains("line-5") && out.contains("line-4"),
                "the tail is what survives a clamp: {out}"
            );
            assert!(
                !out.contains("line-1") && !out.contains("line-3"),
                "earlier lines are dropped: {out}"
            );
        });

        // Shorter than `lines`: everything is returned without padding.
        let (tmp, reg, id) = fixture(&["short-a", "short-b"]);
        with_home(&tmp, || {
            let out = read_shell_in(&reg, &id, Some(50), None, None);
            assert!(
                out.contains("short-a") && out.contains("short-b"),
                "output shorter than the limit is returned whole: {out}"
            );
            assert!(
                !out.contains("(padded") && !out.contains("pad"),
                "no padding is added: {out}"
            );
        });
    }

    // ── open_shell ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn open_shell_returns_a_new_id() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let out = open_shell_with(
            &reg,
            &Owner::Chat("open".into()),
            5,
            Duration::from_millis(3000),
            spawn_in_process_host,
        )
        .await;
        let ids = with_shells(&reg, |store| {
            let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
            ids.sort();
            ids
        });
        assert_eq!(
            ids.len(),
            1,
            "one call must create exactly one record: {out}"
        );
        let id = &ids[0];
        assert!(
            out.contains(&format!("Opened shell {id}")),
            "the sentence must name the new id: {out}"
        );
        assert!(
            id.starts_with('s') && id[1..].chars().all(|c| c.is_ascii_digit()),
            "the id must be an sN shell id: {id}"
        );
        let dedicated = with_shells(&reg, |store| {
            store
                .values()
                .find(|r| r.id.as_str() == id)
                .map(|r| r.dedicated)
        });
        assert_eq!(
            dedicated,
            Some(false),
            "open_shell must create a non-dedicated shell — a dedicated one can \
             never be selected by the reuse rule"
        );
        assert!(
            out.contains("lowest-numbered"),
            "the reuse caveat must be in the sentence: {out}"
        );
    }

    #[tokio::test]
    async fn open_shell_refuses_past_the_owner_cap() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 7, Owner::Chat("capped".into()), NOW);
        let ran: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let spawn = counting_spawn(ran, spawn_in_process_host);
        let out = open_shell_with(
            &reg,
            &Owner::Chat("capped".into()),
            1,
            Duration::from_millis(100),
            spawn,
        )
        .await;
        assert!(
            out.starts_with("Error:") && out.contains("cap"),
            "a breach is error text, not a panic and not an Err: {out}"
        );
        assert!(
            out.contains("limit is 1"),
            "the configured limit is named so the model can act: {out}"
        );
        assert_eq!(
            with_shells(&reg, |store| store.len()),
            1,
            "the pre-existing record is the only one"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the closure must not have been attempted"
        );
    }

    #[tokio::test]
    async fn open_shell_removes_the_record_when_readiness_fails() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let out = open_shell_with(
            &reg,
            &Owner::Chat("stale".into()),
            5,
            Duration::from_millis(200),
            spawn_never_binds,
        )
        .await;
        assert!(
            out.starts_with("Error:") && out.contains("did not become ready"),
            "a readiness failure returns the error text: {out}"
        );
        assert_eq!(
            with_shells(&reg, |store| store.len()),
            0,
            "a host that never came up must not leave a record behind"
        );
    }

    // ── close_shell ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn close_shell_ends_the_callers_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let opened = open_shell_with(
            &reg,
            &Owner::Chat("closer".into()),
            5,
            Duration::from_millis(3000),
            spawn_in_process_host,
        )
        .await;
        assert!(
            opened.contains("Opened shell"),
            "the precondition is a real, ready shell: {opened}"
        );
        let id = opened
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            })
            .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
            .expect("the success sentence names the id");

        // The signal is one-way and an idle bash ignores it, so the honest
        // outcome for a live host is the "still finishing" branch. Poll:
        // whichever of the two observation branches the tool reports, it must
        // be one of them and must name the id — never a bare `Ok` and never a
        // closure the host did not confirm. This measures the reporting
        // contract on the only signal this wire carries.
        let mut out = String::new();
        for _ in 0..3 {
            out = close_shell_with(
                &reg,
                &id,
                &Owner::Chat("closer".into()),
                Duration::from_millis(3000),
            )
            .await;
            if out.contains("Closed shell") || out.contains("Sent TERM") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            out.contains(&format!("shell {id}")),
            "either branch names the shell id: {out}"
        );
        if out.contains("Closed shell") {
            // Observed closure: the host must really be gone.
            assert!(
                !host_is_alive(&crate::shell::lock_path(&id)),
                "a 'Closed shell' answer must mean the lock is released: {out}"
            );
        } else {
            // The lingering branch: the report must not present the shell as
            // gone, and the host must still hold its lock — the exact
            // "possible still finishing" state the sentence describes.
            assert!(
                out.contains("Sent TERM") && out.contains("still"),
                "the lingering branch says what was sent and what remains: {out}"
            );
            assert!(
                host_is_alive(&crate::shell::lock_path(&id)),
                "the still-finishing answer must match a live host: {out}"
            );
        }
        // Either way the registry record stays: the reaper owns record
        // lifetime, close_shell never removes one.
        assert!(
            has_record(&reg, &id),
            "close_shell must not remove the record"
        );
    }

    #[tokio::test]
    async fn close_shell_refuses_another_owners_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        // Same inner string, different kind: the point of the `key()` check.
        insert(&reg, 4, Owner::Ghost("cross".into()), NOW);
        let out = close_shell_with(
            &reg,
            "s4",
            &Owner::Chat("cross".into()),
            Duration::from_millis(500),
        )
        .await;
        assert!(
            out.starts_with("Error:") && out.contains("s4"),
            "another kind's shell must be refused, the id named: {out}"
        );
        assert!(
            out.contains("not yours"),
            "the refusal sentence says whose shell it is: {out}"
        );
        assert!(
            has_record(&reg, "s4"),
            "a refused close must not have removed the record"
        );
    }

    #[tokio::test]
    async fn close_shell_unknown_id_is_an_error() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let out = close_shell_with(
            &reg,
            "s99",
            &Owner::Chat("anon".into()),
            Duration::from_millis(500),
        )
        .await;
        assert!(
            out.starts_with("Error:"),
            "an unknown id is error text: {out}"
        );
        assert!(out.contains("s99"), "the unknown id is named: {out}");
        assert!(
            out.contains("list_shells"),
            "the model is told the way to get a fresh id: {out}"
        );
    }

    // ── watch_shell (phase-04b) ──────────────────────────────────────────

    /// Hold (and leak) the per-id instance lock so `host_is_alive` reads the
    /// shell as live without a real host — what a pattern-arm test needs and
    /// what a real host would otherwise provide.
    fn hold_lock(id: &str) {
        let lock = crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id))
            .expect("acquiring the per-id lock");
        std::mem::forget(lock);
    }

    /// A registry record for `index` under `Owner::Chat("watcher")`, written
    /// inside a `lifecycle_home` scope (paths resolve through `HOME`).
    fn insert_watch_record(reg: &ShellRegistry, index: u32, started_unix: u64) {
        insert(reg, index, Owner::Chat("watcher".into()), started_unix);
    }

    /// Append one pre-encoded cast event line to `path` (with its newline),
    /// creating the logs dir the way a host's first write does.
    fn append_event(path: &std::path::Path, t: f64, payload: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("creating the shell logs dir");
        }
        let line = format!(
            "[{}, \"o\", {}]\n",
            t,
            serde_json::to_string(payload).unwrap_or_default()
        );
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap();
        f.write_all(line.as_bytes()).unwrap();
        f.flush().unwrap();
    }

    #[tokio::test]
    async fn watch_shell_returns_when_the_pattern_appears() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 5, 1_788_470_005);
        append_event(&cast_path("s5", 1_788_470_005), 1.0, "warming up\n");
        hold_lock("s5");
        let path = cast_path("s5", 1_788_470_005);
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            append_event(&path, 2.0, "READY to go\n");
        });
        let out = watch_shell_with(
            &reg,
            "s5",
            Some("READY"),
            5,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        writer.await.unwrap();
        assert!(
            out.contains("matched"),
            "a hit is reported as a match: {out}"
        );
        assert!(
            out.contains("READY to go"),
            "the matching line is shown: {out}"
        );
        assert!(
            out.contains("since this call started"),
            "the line number's meaning is named: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_ignores_output_written_before_the_call() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 6, 1_788_470_006);
        append_event(&cast_path("s6", 1_788_470_006), 1.0, "READY already\n");
        hold_lock("s6");
        let out = watch_shell_with(
            &reg,
            "s6",
            Some("READY"),
            1,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        assert!(
            out.contains("timed out"),
            "pre-call output must not match — the tool watches from now: {out}"
        );
        assert!(
            !out.contains("matched"),
            "the stale line is not what a match report shows: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_times_out_and_reports_what_it_saw() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 7, 1_788_470_007);
        hold_lock("s7");
        let path = cast_path("s7", 1_788_470_007);
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            append_event(&path, 1.0, "progress 12%\n");
        });
        let out = watch_shell_with(
            &reg,
            "s7",
            Some("never-appears"),
            1,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        writer.await.unwrap();
        assert!(
            out.contains("timed out after 1 s"),
            "the timeout names the elapsed seconds: {out}"
        );
        assert!(
            out.contains("progress 12%"),
            "the tail shows what was actually seen: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_unknown_id_is_an_error_naming_live_shells() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 8, NOW);
        let out = watch_shell_with(
            &reg,
            "s99",
            Some("x"),
            5,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        assert!(
            out.starts_with("Error:"),
            "an unknown id is error text, returned as a String: {out}"
        );
        assert!(out.contains("s99"), "the bad id is named: {out}");
        assert!(
            out.contains("s8"),
            "the live ids are named so the model knows what to pass: {out}"
        );
        assert!(
            !out.contains("timed out"),
            "the refusal returns before any waiting: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_invalid_pattern_is_an_error() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 9, NOW);
        let early = std::time::Instant::now();
        let out = watch_shell_with(
            &reg,
            "s9",
            Some("[unclosed"),
            30,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        assert!(
            early.elapsed() < Duration::from_secs(1),
            "the refusal must return before the timeout, not after waiting: {out}"
        );
        assert!(
            out.starts_with("Error:"),
            "an invalid pattern is error text: {out}"
        );
        assert!(
            out.contains("[unclosed"),
            "the offending pattern is named: {out}"
        );
        assert!(
            out.contains("regex"),
            "the error says the pattern is applied as a regex: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_finds_the_cast_when_the_records_timestamp_is_stale() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        // The record says the shell started at N, but the host stamped the
        // cast N+1 — a spawn that crossed a second boundary.
        insert_watch_record(&reg, 10, 1_788_470_010);
        let path = cast_path("s10", 1_788_470_011);
        append_event(&path, 1.0, "warm\n");
        hold_lock("s10");
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            append_event(&path, 2.0, "FOUND IT\n");
        });
        let out = watch_shell_with(
            &reg,
            "s10",
            Some("FOUND IT"),
            5,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        writer.await.unwrap();
        assert!(
            out.contains("FOUND IT"),
            "the scan by parse_cast_name must find the fresh cast, \
             not the record's stale timestamp: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_stops_when_the_host_is_gone() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 11, 1_788_470_011);
        let path = cast_path("s11", 1_788_470_011);
        // A leased rather than leaked lock: dropping it makes the shell's
        // host look dead to `host_is_alive`.
        let leased =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path("s11"))
                .expect("acquiring the per-id lock for the test host");
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            append_event(&path, 1.0, "left over\n");
            tokio::time::sleep(Duration::from_millis(150)).await;
            drop(leased);
        });
        let out = watch_shell_with(
            &reg,
            "s11",
            Some("never"),
            30,
            Duration::from_millis(50),
            Duration::from_millis(0),
        )
        .await;
        writer.await.expect("the writer task");
        assert!(
            out.contains("host is gone"),
            "the host-gone sentence is what comes back: {out}"
        );
        assert!(
            out.contains("left over"),
            "the tail of what was seen is reported: {out}"
        );
    }

    #[tokio::test]
    async fn watch_shell_reports_that_nothing_was_running() {
        let (guard, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        spawn_in_process_host(ShellId::from_index(12))
            .await
            .expect("the host fixture needs bash");
        insert_watch_record(&reg, 12, NOW);
        let out = watch_shell_with(
            &reg,
            "s12",
            None,
            5,
            Duration::from_millis(50),
            Duration::from_millis(100),
        )
        .await;
        drop(_dir);
        drop(guard);
        assert!(
            out.contains("nothing was running"),
            "an idle shell with no command is reported as such: {out}"
        );
        assert!(
            !out.contains("finished"),
            "an idle shell that never ran anything must not be called finished: {out}"
        );
        assert!(
            out.contains("start the command") || out.contains("Start the command"),
            "the sentence says what to do next: {out}"
        );
    }

    #[test]
    fn visible_text_strips_ansi_escapes() {
        // The real prompt line from the measured production cast: OSC title
        // set, SGR colour onset and reset, around the visible text.
        let raw = "\u{1b}]0;matt@scrappy:~\u{7}\u{1b}]0;matt@scrappy: ~\u{7}\u{1b}[01;32mmatt@scrappy\u{1b}[00m:\u{1b}[01;34m~\u{1b}[00m$ ";
        let out = visible_text(raw);
        assert!(!out.contains('\u{1b}'), "no escape survives: {out:?}");
        assert!(
            out.contains("matt@scrappy") && out.contains('$'),
            "the plain text is what is left: {out:?}"
        );
    }

    #[test]
    fn visible_text_keeps_the_text_after_the_last_carriage_return() {
        let out = visible_text("10%\r50%\r100% done\nnext");
        assert_eq!(
            out, "100% done\nnext",
            "the last write of a rewritten line wins: {out:?}"
        );
    }
}
