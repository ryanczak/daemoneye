//! The recorded-output readers: `read_shell` and the shell-log readers `list_shell_logs` / `read_shell_log`.

use super::*;

// ---------------------------------------------------------------------------
// read_shell (M22 phase-03b)
// ---------------------------------------------------------------------------

/// Default tail depth when `lines` is omitted. Same values as the retired
/// `read_pane` had, which is the analogue this tool's parameters were
/// measured against.
const READ_SHELL_DEFAULT_LINES: usize = 200;
/// Hard ceiling on a single `read_shell` capture. Same value as the retired
/// `read_pane` had.
pub(super) const READ_SHELL_MAX_LINES: usize = 2000;

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
pub(super) struct ShellOutput {
    pub(super) owner_key: String,
    /// How many commands the output was assembled from. Searchable text is the
    /// concatenation of a subset: empty output can mean either "recorded
    /// nothing yet" (0) or "the selected commands wrote nothing" (non-zero).
    pub(super) command_count: usize,
    /// Raw (lossy-UTF-8) recorded output of the selected commands, in command
    /// order. Normalization lives with each caller's rendering.
    pub(super) text: String,
}

/// The recorded-output read `read_shell` and `find_in_shells` share:
/// registry lookup → cast path → `MetaIndex` → byte-range read, decoded and
/// normalized. `since_command` selects a suffix of the commands; `None` reads
/// every one. `Err` carries the exact sentence the caller shows — five
/// distinct failure reasons end here, and a second copy of them would drift.
/// A live shell that has recorded no commands is a success with `command_count`
/// 0, not an error: a search over it finds nothing, while `read_shell` says so
/// in its own words.
pub(super) fn read_recorded_output(
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

/// Pure: the tail depth `lines` requests, with the retired `read_pane`'s
/// defaults (200, capped at 2000, `0` or absent → default). Split from
/// `build_body` so the clamp arithmetic is assertable directly.
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
pub(super) fn read_shell_in(
    reg: &ShellRegistry,
    id: &str,
    lines: Option<u64>,
    since_command: Option<u32>,
    filter: Option<&str>,
) -> String {
    let out = match read_recorded_output(reg, id, since_command) {
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
                     Record the pattern without anchors, or check the previous \
                     commands' output with list_shells."
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
// list_shell_logs / read_shell_log (M22 phase-06b) — the recorded-log readers
// ---------------------------------------------------------------------------

/// How many rows `list_shell_logs` shows when `limit` is omitted.
pub(super) const LIST_LOGS_DEFAULT_LIMIT: u64 = 50;
/// Hard ceiling on a single `list_shell_logs` listing.
pub(super) const LIST_LOGS_MAX_LIMIT: u64 = 500;

/// One recording found in the log directory, shaped for the formatter. The
/// row's id is the **log id** — `<shell id>-<started unix>` — which is what
/// `read_shell_log` takes as its `log` argument.
struct LogRow {
    log_id: String,
    started_unix: u64,
    /// `Some(n)` when the recording has a readable `.meta.json` carrying `n`
    /// commands; `None` when the sidecar is missing or unreadable. A cast
    /// written before the index's producer existed and a shell that has
    /// recorded nothing are different facts, and the listing must not equate
    /// them with 0.
    commands: Option<usize>,
    /// The first recorded command's text, when the index carries one — the
    /// hint of what the session was.
    first_command: Option<String>,
}

/// Pure: fold the requested listing limit into its window. `None` (or `0`,
/// nonsense for "show me the last N") takes the default; anything above the
/// cap is clamped to it.
pub(super) fn clamp_list_logs_limit(limit: Option<u64>) -> u64 {
    match limit {
        None | Some(0) => LIST_LOGS_DEFAULT_LIMIT,
        Some(n) => n.min(LIST_LOGS_MAX_LIMIT),
    }
}

/// Pure: whether the requested limit was reduced, phrased so the tool result
/// can quote it. The model has to be able to tell a truncated listing from a
/// complete one.
fn list_logs_clamp_note(requested: Option<u64>) -> Option<String> {
    match requested {
        Some(r) if r > LIST_LOGS_MAX_LIMIT => Some(format!(
            "requested limit {r} exceeded the maximum of {LIST_LOGS_MAX_LIMIT} and was clamped"
        )),
        _ => None,
    }
}

/// List the shell recordings in `~/.daemoneye/var/log/shells/` — live and
/// finished shells alike, newest first. Each row carries the **log id**
/// (`s3-1788470000`), not the bare shell id: `s0` after a daemon restart is a
/// different shell from today's `s0`, and the timestamp is what distinguishes
/// two recordings of one id.
///
/// `pub` rather than `pub(super)`: `knowledge/mod.rs` re-exports it for
/// `executor` the way every sibling module's functions are re-exported, and a
/// `pub(super)` item cannot be given wider visibility on re-export.
pub fn list_shell_logs(since: Option<u64>, limit: Option<u64>) -> String {
    list_shell_logs_in(&shell_logs_dir(), since, limit, now_unix())
}

/// `list_shell_logs` against an injected directory and clock — the seam the
/// tests drive. Mirrors `list_shells`'s split: I/O here, formatting below.
pub(super) fn list_shell_logs_in(
    dir: &std::path::Path,
    since: Option<u64>,
    limit: Option<u64>,
    now_unix: u64,
) -> String {
    // Scan for casts, parsing `<id>-<started_unix>` from each name. Anything
    // that does not parse — the `.meta.json` sidecars, any stray file — is not
    // a recording and is not listed.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return format!(
            "No shell recordings found: log directory {} does not exist or \
             cannot be read. A recording appears here when a command runs in \
             a PTY shell; list_shells shows the live shells.",
            dir.display()
        );
    };
    let mut rows: Vec<LogRow> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((_, started_unix)) = parse_cast_name(name) else {
            continue;
        };
        rows.push(LogRow {
            log_id: name.strip_suffix(".cast").unwrap_or(name).to_string(),
            started_unix,
            commands: None,
            first_command: None,
        });
    }
    // Best-effort sidecar read, after the scan: a row's index is loaded if it
    // is there and left `None` if it is not — an unindexed cast stays listed
    // with an unknown command count, never with 0.
    for row in &mut rows {
        let cast = dir.join(format!("{}.cast", row.log_id));
        if let Ok(meta) = MetaIndex::load(&meta_path_for(&cast)) {
            row.commands = Some(meta.commands.len());
            row.first_command = meta.commands.first().map(|c| c.command.clone());
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.started_unix));
    format_log_rows(&rows, dir, since, limit, now_unix)
}

/// Pure: filter, clamp and render already-collected rows. Takes `now_unix`
/// rather than reading the clock so tests can pin the ages.
fn format_log_rows(
    rows: &[LogRow],
    dir: &std::path::Path,
    since: Option<u64>,
    limit: Option<u64>,
    now_unix: u64,
) -> String {
    if rows.is_empty() {
        return format!(
            "No shell recordings found in {} — nothing has been recorded yet. \
             A recording appears when a command runs in a PTY shell; \
             list_shells shows the live shells.",
            dir.display()
        );
    }
    let kept: Vec<&LogRow> = rows
        .iter()
        .filter(|row| since.is_none_or(|s| row.started_unix >= s))
        .collect();
    if kept.is_empty() {
        return format!(
            "No shell recordings at or after unix timestamp {} in {} — \
             {} recording(s) found, all older. Omit `since` or lower it.",
            since
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0".to_string()),
            dir.display(),
            rows.len()
        );
    }
    let applied = clamp_list_logs_limit(limit) as usize;
    let note = list_logs_clamp_note(limit);
    let total = kept.len();
    let shown = &kept[..total.min(applied)];
    let mut out = format!(
        "{} shell recording{} at or after unix {s} (newest first), showing {} of them{}:",
        total,
        if total == 1 { "" } else { "s" },
        shown.len(),
        match &note {
            Some(n) => format!(" (note: {n})"),
            None => String::new(),
        },
        s = since.map(|v| v.to_string()).unwrap_or_default(),
    );
    for row in shown {
        let count_part = match row.commands {
            Some(n) => format!("{n} command{}", if n == 1 { "" } else { "s" }),
            None => "commands: unknown (no readable command index)".to_string(),
        };
        let hint = match (&row.first_command, row.commands) {
            (Some(first), Some(_)) => format!("  first: {}", mask_sensitive(first)),
            _ => String::new(),
        };
        let age_part = if now_unix >= row.started_unix {
            format_age(now_unix - row.started_unix)
        } else {
            "future".to_string()
        };
        out.push_str(&format!(
            "\n  {}  age:{age_part}  {count_part}{hint}",
            row.log_id
        ));
    }
    if total > shown.len() {
        out.push_str(&format!(
            "\n[{} more recording(s) past the limit — raise `limit` or narrow `since`]",
            total - shown.len()
        ));
    }
    out
}

/// Read one shell recording by its **log id** and render its recorded
/// commands. `log` is the `s3-1788470000` shape `list_shell_logs` prints; the
/// timestamp is the point of the id — a bare shell id would be ambiguous
/// across daemon restarts, so this tool refuses the bare form rather than
/// picking the newest recording that happens to match.
///
/// `pub` rather than `pub(super)`: `knowledge/mod.rs` re-exports it for
/// `executor` the way every sibling module's functions are re-exported, and a
/// `pub(super)` item cannot be given wider visibility on re-export.
pub fn read_shell_log(
    log: &str,
    command_index: Option<u32>,
    lines: Option<u64>,
    filter: Option<&str>,
) -> String {
    read_log_in(&shell_logs_dir(), log, command_index, lines, filter)
}

/// `read_shell_log` against an injected directory — the seam the tests drive.
pub(super) fn read_log_in(
    dir: &std::path::Path,
    log: &str,
    command_index: Option<u32>,
    lines: Option<u64>,
    filter: Option<&str>,
) -> String {
    // 1. The log id's shape first, before touching the directory: a bare
    //    shell id ("s3") is a refusal, not a lookup that happens to resolve.
    if parse_cast_name(&format!("{log}.cast")).is_none() {
        return format!(
            "Error: {log:?} is not a log id. A log id is the shell id plus the \
             session's start timestamp, like \"s3-1788470000\" — shell ids are \
             reused across daemon restarts, so \"s3\" alone does not name one \
             recording. Call list_shell_logs and use a full id from its output."
        );
    }
    // 2. The regex next, before any file read — an invalid pattern costs
    //    nothing and reads nothing.
    let re = match filter {
        Some(p) => match regex::RegexBuilder::new(p).size_limit(1 << 20).build() {
            Ok(re) => Some(re),
            Err(e) => {
                return format!(
                    "Error: invalid filter regex {p:?}: {e}. Fix the expression and \
                     call again; the filter is applied as a regex over each line, \
                     not a literal substring match."
                );
            }
        },
        None => None,
    };
    let cast = dir.join(format!("{log}.cast"));
    let meta_path = meta_path_for(&cast);
    // 3. The sidecar is what makes a read possible. Its absence is not an
    //    error response shape change: a recording from before the index's
    //    producer, or one that has recorded nothing, simply has nothing
    //    indexed to read.
    let Ok(meta) = MetaIndex::load(&meta_path) else {
        return if std::fs::metadata(&cast).is_ok() {
            format!(
                "Log {log} has a recording but no readable command index ({}): \
                 the recording predates the command index or recorded nothing. \
                 It can still be read **live** with read_shell if its shell is \
                 still running — call list_shells to check.",
                meta_path.display()
            )
        } else {
            format!(
                "Error: no recording named {log:?} in {} — no .cast file carries \
                 that id. Available log ids: {}. Call list_shell_logs to refresh.",
                dir.display(),
                available_log_ids(dir)
            )
        };
    };
    // 4. The index exists: resolve the selected commands.
    let highest = meta.commands.len();
    if highest == 0 {
        return format!(
            "Log {log} has a readable command index with 0 commands — the shell \
             has not run a command since the index was created. Read it with \
             read_shell if its shell is still running."
        );
    }
    let indices: Vec<u32> = match command_index {
        Some(i) => {
            if meta.commands.iter().any(|c| c.index == i) {
                vec![i]
            } else {
                let max_index = meta.commands.iter().map(|c| c.index).max().unwrap_or(0);
                return format!(
                    "Error: command_index {i} is beyond every recorded command of log \
                     {log} — the highest recorded index is {max_index}. Call with a \
                     smaller index (0 is always valid: it is the first recorded \
                     command), or omit it to read every recorded command."
                );
            }
        }
        None => meta.commands.iter().map(|c| c.index).collect(),
    };
    // 5. Read each selected command through the shared byte-range reader — no
    //    new slicing here.
    let mut bytes = Vec::new();
    for index in &indices {
        match read_command_output(&cast, &meta, *index) {
            Ok(mut out) => bytes.append(&mut out),
            Err(e) => {
                return format!(
                    "Error: failed reading log {log}'s recorded output at command \
                     index {index}: {e}. The cast file may have been rotated; \
                     call list_shell_logs and re-read with the fresh id."
                );
            }
        }
    }
    let body = match build_body(&String::from_utf8_lossy(&bytes), lines, re.as_ref()) {
        Ok(body) => body,
        Err(msg) => return msg,
    };
    let exit_suffix = match command_index.map(|i| exit_code_of(&meta, i)) {
        Some(Some(0)) => ", exit code 0".to_string(),
        Some(Some(code)) => format!(", exit code {code} (non-zero)"),
        Some(None) | None => String::new(),
    };
    let header = format!(
        "Log {log} — {highest} command{} recorded; showing {}{exit_suffix}{}:",
        if highest == 1 { "" } else { "s" },
        match command_index {
            Some(i) => format!("command {i}"),
            None => format!("all {highest}"),
        },
        match filter {
            Some(p) => format!(" matching /{p}/"),
            None => String::new(),
        }
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
                     Record the pattern without anchors, or widen the line clamp."
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

/// Pure: the exit code of command `index` in `meta`, for the single-command
/// header. `None` when the index does not name one — the caller then cites
/// nothing.
fn exit_code_of(meta: &MetaIndex, index: u32) -> Option<i32> {
    meta.commands
        .iter()
        .find(|c| c.index == index)
        .map(|c| c.exit_code)
}

/// Pure: the log ids the directory actually holds, newest first, for the
/// not-found sentence.
fn available_log_ids(dir: &std::path::Path) -> String {
    let mut ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if parse_cast_name(&name).is_some() {
                ids.push(name.strip_suffix(".cast").unwrap_or(&name).to_string());
            }
        }
    }
    ids.sort();
    ids.reverse();
    if ids.is_empty() {
        "(none)".to_string()
    } else {
        ids.join(", ")
    }
}
