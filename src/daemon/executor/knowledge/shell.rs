//! `list_shells` — the model-facing listing of the PTY shells the daemon is
//! running. Reads **only** the shell registry: no socket round-trips, no
//! screen-grid access and no asciicast reads — those are the buffer readers'
//! substrate (03b).

use crate::ai::filter::mask_sensitive;
use crate::config::shell_logs_dir;
use crate::shell::{
    MetaIndex, Owner, ShellId, ShellRecord, ShellRegistry, await_ready, cast_path, host_is_alive,
    meta_path_for, parse_cast_name, read_command_output, socket_path, spawn_for_on_host,
    with_shells,
};
use std::time::{Duration, Instant};

/// One collected shell, shaped for the formatter. Holds the owner's `key()`
/// string rather than the `Owner` itself, so the formatter can never compare
/// inner ids across owner kinds.
pub(super) struct ShellRow {
    pub id: String,
    pub owner_key: String,
    pub host: crate::host::HostId,
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
                .filter(|record| record.owner.key() == key || matches!(record.owner, Owner::User))
                .map(collect_row)
                .collect::<Vec<_>>()
        })),
        Some("all") => Ok(with_shells(reg, |store| {
            store.values().map(collect_row).collect::<Vec<_>>()
        })),
        Some(other) => Err(format!(
            "Error: unknown {tool} scope {other:?}. Accepted values: \
             \"mine\" (this session's shells and the user's, the default) or \"all\" (every \
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
        host: record.host.clone(),
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
        let host_part = host_part(&row.host);
        out.push_str(&format!(
            "  {}  owner:{}{host_part}{age_part}{flags}\n",
            row.id, row.owner_key
        ));
    }
    out
}

/// The host token a shell's line carries. Empty for the implicit local host:
/// a shell on `HostId::Local` must render exactly as it did before hosts
/// existed, so the token only appears for a named host.
fn host_part(host: &crate::host::HostId) -> String {
    match host {
        crate::host::HostId::Local => String::new(),
        crate::host::HostId::Named(name) => format!(" host:{name}"),
    }
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
// shell_context_block (M22 phase-08a) — the ambient [SHELLS] / [DEFAULT SHELL]
// ---------------------------------------------------------------------------

/// The shell a bare `run_terminal_command` would reuse for `owner_key`, or
/// `None` when the owner has no eligible shell.
///
/// Pure mirror of `shell_for_owner`'s step-(1) reuse predicate
/// (`src/shell/session.rs`): lowest **numeric** index among the owner's
/// shells that run on `HostId::Local` and are neither `interactive` nor
/// `dedicated` — a bare `run_terminal_command` lands on the daemon host, so
/// a remote shell is never its default.
///
/// **Deliberate divergence:** `shell_for_owner` settles liveness with a
/// per-shell flock probe — the registry's own liveness check — while this
/// runs on every prompt assembly and must do no I/O. It reads the
/// `exited_unix` stamp `reap_exited` writes instead: a host that died since
/// the last reap is named one turn too long, a reaped record is never named.
fn default_shell_for(rows: &[ShellRow], owner_key: &str) -> Option<String> {
    rows.iter()
        .filter(|row| {
            !row.interactive
                && !row.dedicated
                && row.exited_unix.is_none()
                && row.owner_key == owner_key
                && row.host == crate::host::HostId::Local
        })
        .min_by_key(|row| ShellIndex::new(&row.id).0)
        .map(|row| row.id.clone())
}

/// Format the ambient shell block. Pure: takes `now_unix` rather than reading
/// the clock and never sees the registry, so tests hand-build rows.
///
/// Returns the **empty string** when `rows` is empty — every session's first
/// turn has no shells, and a "no shells" line on every one of those turns is
/// pure token cost for no information.
fn format_shell_context(rows: &[ShellRow], owner_key: &str, now_unix: u64) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&ShellRow> = rows.iter().collect();
    sorted.sort_by_key(|row| ShellIndex::new(&row.id).0);
    let mut out = String::from("[SHELLS] ");
    out.push_str(&sorted.len().to_string());
    out.push_str(" — ");
    let parts: Vec<String> = sorted
        .iter()
        .map(|row| {
            let age_part = if row.started_unix > 0 && now_unix >= row.started_unix {
                format!(" age:{}", format_age(now_unix - row.started_unix))
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
            let host_part = match &row.host {
                crate::host::HostId::Local => String::new(),
                crate::host::HostId::Named(name) => format!(" host:{name}"),
            };
            format!("{}{host_part}{age_part}{flags}", row.id)
        })
        .collect();
    out.push_str(&parts.join(", "));
    out.push('\n');
    if let Some(default) = default_shell_for(rows, owner_key) {
        out.push_str(&format!(
            "[DEFAULT SHELL] {default} — run_terminal_command with no shell argument lands here\n"
        ));
    }
    out
}

/// The block against an injected registry — the seam tests drive. `resolve_scope`
/// with `None` collects exactly `caller`'s own rows, the same rows `list_shells`
/// scope-"mine" shows, so the ambient block and the tool can never disagree on
/// what the caller owns.
///
/// `pub(crate)`: `prompt.rs`'s tests drive the same seam for the shells-only
/// prompt path (M27 phase 04) — they must see the same formatting the real
/// block does.
pub(crate) fn shell_context_block_in(reg: &ShellRegistry, caller: &Owner, now_unix: u64) -> String {
    let key = caller.key();
    let rows = match resolve_scope(reg, None, caller, "shell_context") {
        Ok(rows) => rows,
        Err(_) => return String::new(),
    };
    format_shell_context(&rows, &key, now_unix)
}

/// The ambient `[SHELLS]` / `[DEFAULT SHELL]` block for `caller`, or the
/// empty string when the caller owns no shells.
///
/// `pub` rather than `pub(super)`: `knowledge/mod.rs` re-exports it for
/// `executor` and the prompt builders the way every sibling module's functions
/// are re-exported, and a `pub(super)` item cannot be given wider visibility on
/// re-export.
pub fn shell_context_block(caller: &Owner) -> String {
    #[cfg(test)]
    let reg = open_shell_test_registry();
    #[cfg(not(test))]
    let reg = crate::shell::registry().clone();
    shell_context_block_in(&reg, caller, now_unix())
}

/// Format the ambient hosts block: the implicit `local` host, then each
/// configured `[hosts.<name>]` entry by name, with its ssh destination and how
/// many **live** shells run on it, across every owner. Pure: tests hand-build
/// the hosts and the rows.
///
/// Returns the **empty string** when no hosts are configured: a lone
/// `local` says nothing the model does not already know, and costs tokens on
/// every turn. No ssh probe here either: this runs on every prompt assembly,
/// and ControlMaster state is `/hosts`' job.
fn format_hosts_context(
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
    rows: &[ShellRow],
) -> String {
    if hosts.is_empty() {
        return String::new();
    }
    let shells_on = |host: &crate::host::HostId| match rows
        .iter()
        .filter(|row| &row.host == host && row.exited_unix.is_none())
        .count()
    {
        0 => "no shells".to_string(),
        1 => "1 shell".to_string(),
        n => format!("{n} shells"),
    };
    let mut names: Vec<&String> = hosts.keys().collect();
    names.sort();
    let mut parts = vec![format!(
        "local ({})",
        shells_on(&crate::host::HostId::Local)
    )];
    for name in names {
        let host = crate::host::HostId::Named(name.clone());
        parts.push(format!(
            "{name} (ssh {}, {})",
            hosts[name].ssh,
            shells_on(&host)
        ));
    }
    format!("[HOSTS] {}\n", parts.join(" · "))
}

/// The ambient `[HOSTS]` block for a prompt, or the empty string when
/// `config` has no `[hosts]` entries. Counts come from every shell in the
/// registry, whoever owns it.
pub fn hosts_context_block(config: &crate::config::Config) -> String {
    let rows = with_shells(crate::shell::registry(), |store| {
        store.values().map(collect_row).collect::<Vec<_>>()
    });
    format_hosts_context(&config.hosts, &rows)
}

/// The breadth a `get_shell_context` call asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextBreadth {
    /// The `[SHELLS]` / `[DEFAULT SHELL]` block only — no tmux call.
    ShellsOnly,
    /// The user's session: today's default snapshot.
    Session,
    /// The session plus foreign-session pane metadata.
    All,
}

/// Pure: the breadth a `scope` argument names, or the operator-facing error
/// for one it does not. `"window"` and `"run"` are named explicitly because
/// both were real vocabulary — `"window"` on this tool until M22 phase-08b,
/// `"run"` in the 2.0 design — and a model that learned either deserves to be
/// told what replaced it rather than silently getting a session snapshot.
pub fn resolve_context_scope(scope: Option<&str>) -> Result<ContextBreadth, String> {
    match scope {
        None | Some("session") => Ok(ContextBreadth::Session),
        Some("shells") => Ok(ContextBreadth::ShellsOnly),
        Some("all") => Ok(ContextBreadth::All),
        Some(other) => Err(format!(
            "Error: unknown get_shell_context scope {other:?}. Accepted values: \
             \"shells\" (the open shells and the default one, no terminal \
             capture), \"session\" (the default, this session's terminal \
             context) or \"all\" (adds pane metadata for other tmux sessions)."
        )),
    }
}

/// The `scope: "shells"` result: 08a's ambient block, or a sentence saying
/// there is nothing, because a tool result of `""` reads to the model as a
/// failed call.
pub fn shells_only_context(caller: &Owner) -> String {
    let block = shell_context_block(caller);
    if block.is_empty() {
        "[SHELLS] none — this session has no open shells. \
         run_terminal_command or open_shell starts one.\n"
            .to_string()
    } else {
        block
    }
}

// ---------------------------------------------------------------------------
// read_shell (M22 phase-03b)
// ---------------------------------------------------------------------------

/// Default tail depth when `lines` is omitted. Same values as the retired
/// `read_pane` had, which is the analogue this tool's parameters were
/// measured against.
const READ_SHELL_DEFAULT_LINES: usize = 200;
/// Hard ceiling on a single `read_shell` capture. Same value as the retired
/// `read_pane` had.
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
fn read_recorded_output(
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
fn read_shell_in(
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

/// The shell ids currently in `reg`, numerically sorted. `read_recorded_output`,
/// `close_shell_with` and the new `watch_shell` all need this; extracted here
/// at the third copy rather than a second round of three.
fn live_shell_ids(reg: &ShellRegistry) -> Vec<String> {
    with_shells(reg, |store| {
        let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
        ids.sort_by_key(|i| ShellId::parse(i).map(|s| s.index()).unwrap_or(u32::MAX));
        ids
    })
}
// ---------------------------------------------------------------------------
// send_input (M22 phase-05)
// ---------------------------------------------------------------------------

/// How long `send_input` may wait for the host to acknowledge the write.
/// Same reasoning as `SIGNAL_TIMEOUT`, and the same value: the ack is the
/// socket round-trip, not the program reacting.
const INPUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The words the approval panel shows about what the shell is doing — the
/// `input_target_note` sentence for the `send_input` panel's `cmd` line.
///
/// The `Idle` wording is the honest hazard, not a refusal: a program waiting
/// on stdin reports `Running` through this wire, so `Idle` really does mean
/// the shell is at its prompt and whatever arrives will be run as a command —
/// without `run_terminal_command`'s approval text, capture or exit code. The
/// approving user is told that; refusing would block a legitimate use
/// (`send_input` to a prompt-less shell is exactly how you drive the shell
/// itself).
pub fn input_target_note(state: Option<crate::shell::ShellState>) -> String {
    match state {
        Some(crate::shell::ShellState::Running) => {
            "shell is running a program, so the input goes to that program (e.g. \
             answering its stdin prompt); it is NOT executed as a shell command"
                .to_string()
        }
        Some(crate::shell::ShellState::Idle) => {
            "shell is at its prompt, so the input WILL be run as a shell command, \
             without run_terminal_command's approval, capture or exit code"
                .to_string()
        }
        Some(crate::shell::ShellState::Paused) => {
            "shell's job is stopped; the input will sit unread until the job \
             resumes"
                .to_string()
        }
        Some(crate::shell::ShellState::Exited) => {
            "shell reports exited, so it may be gone and nothing will read the \
             input"
                .to_string()
        }
        None => "shell state could not be probed, so it may be gone and nothing will \
             read the input"
            .to_string(),
    }
}

/// Write `text` (plus a newline when `enter`) into one of `caller`'s live PTY
/// shells and return the sentence the model reads.
///
/// The public form uses the daemon's registry and the standard input timeout;
/// the `_with` form takes the registry, the timeout and the send step,
/// mirroring the `open_shell` / `open_shell_with` split. Injecting `send` is
/// what makes the newline and the absolute-wrong-target assertions exact
/// checks on the bytes rather than inferences from a message.
pub async fn send_input(shell: &str, text: &str, enter: bool, caller: &Owner) -> String {
    send_input_with(
        crate::shell::registry(),
        shell,
        text,
        enter,
        caller,
        INPUT_TIMEOUT,
        |bytes| async move {
            crate::shell::send_input(&socket_path(shell), &bytes, INPUT_TIMEOUT).await
        },
    )
    .await
}

/// `send_input` with the registry, timeout and send step injected.
pub(super) async fn send_input_with<S, Fut>(
    reg: &ShellRegistry,
    shell: &str,
    text: &str,
    enter: bool,
    caller: &Owner,
    input_timeout: Duration,
    send: S,
) -> String
where
    S: Fn(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let _ = input_timeout;
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
    if !caller.may_drive(&owner_key) {
        return format!(
            "Error: shell {shell} is not yours (it belongs to {owner_key}), so this \
             session cannot type into it. If it needs input, the session that \
             owns it does that; list_shells shows which shells are whose."
        );
    }
    let mut bytes = text.as_bytes().to_vec();
    if enter {
        // `\n`, not `\r` or `\r\n`: the same terminator `wrap_command` sends,
        // which is how every command in this codebase reaches a shell.
        bytes.push(b'\n');
    }
    if let Err(e) = send(bytes).await {
        return format!(
            "Error: shell {shell} did not accept the input, so nothing was typed: \
             {}. The write is not half-delivered (the host acks the whole frame \
             or rejects it); check the shell with list_shells and retry.",
            format_err_chain(&e)
        );
    }
    format!(
        "Sent {} byte{} to shell {}{}{}.",
        text.len() + usize::from(enter),
        if text.len() + usize::from(enter) == 1 {
            ""
        } else {
            "s"
        },
        shell,
        if enter {
            ", newline appended"
        } else {
            ", no newline"
        },
        ". Read the effect with read_shell or watch_shell — this tool returns no \
         output and does not wait for the program to react."
    )
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
///
/// `host` is resolved **before** any spawn: an unknown or malformed name is
/// refused with `host_entry`'s / `HostId::parse`'s own message — never routed
/// through `open_shell_error`, which would wrap a host problem in per-owner
/// shell-limit advice the model cannot act on.
pub async fn open_shell(caller: &Owner, host: Option<&str>) -> String {
    match open_shell_id(caller, host).await {
        Ok((id, host)) => opened_shell_sentence(&id, &host),
        Err(msg) => msg,
    }
}

/// `open_shell` for a caller that needs the shell's id rather than the
/// model's sentence: the user's `/shell` (M23). `Ok` carries the id and the
/// host it landed on; `Err` carries the same sentence `open_shell` returns.
pub async fn open_shell_id(
    caller: &Owner,
    host: Option<&str>,
) -> Result<(ShellId, crate::host::HostId), String> {
    let max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    let host = match host.map(crate::host::HostId::parse) {
        None => crate::host::HostId::Local,
        Some(Ok(parsed)) => parsed,
        Some(Err(msg)) => return Err(format!("Error: {msg}")),
    };
    // The two arms spawn different programs, so their closure types differ and
    // each calls `open_shell_with_id` itself.
    let id = match &host {
        crate::host::HostId::Local => {
            open_shell_with_id(
                &open_shell_test_registry(),
                caller,
                crate::host::HostId::Local,
                max_per_owner,
                SHELL_READY_TIMEOUT,
                spawn_in_process_host,
            )
            .await
        }
        crate::host::HostId::Named(name) => {
            // `host_entry` resolves before the spawn on purpose: its refusals
            // (unknown host, empty destination) must reach the model as their
            // own sentences. Routing them through `open_shell_error` would
            // dress a host problem in per-owner shell-limit advice the model
            // cannot act on (§ Current state 5).
            if let Err(e) = crate::shell::host_entry(name) {
                return Err(format!("Error: {e}"));
            }
            open_shell_with_id(
                &open_shell_test_registry(),
                caller,
                crate::host::HostId::Named(name.clone()),
                max_per_owner,
                SHELL_READY_TIMEOUT,
                |id| spawn_on_named_host(id, name),
            )
            .await
        }
    }?;
    Ok((id, host))
}

/// `open_shell` with every effect injected: the registry, the host the
/// record is stamped with, the per-owner cap, the readiness timeout and the
/// spawn step. Tests replace `spawn` with an in-process host because
/// `spawn_detached_host` re-invokes `current_exe()`, which under a test
/// binary is not the daemon (07b F4).
#[cfg(test)]
pub(super) async fn open_shell_with<F, Fut>(
    reg: &ShellRegistry,
    caller: &Owner,
    host: crate::host::HostId,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> String
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    match open_shell_with_id(
        reg,
        caller,
        host.clone(),
        max_per_owner,
        ready_timeout,
        spawn,
    )
    .await
    {
        Ok(id) => opened_shell_sentence(&id, &host),
        Err(msg) => msg,
    }
}

/// `open_shell_with`'s work: spawn, wait for readiness, and return the id, or
/// the sentence the model reads when the shell could not be opened.
async fn open_shell_with_id<F, Fut>(
    reg: &ShellRegistry,
    caller: &Owner,
    host: crate::host::HostId,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> Result<ShellId, String>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // Neither reuse helper is used: the reuse scan would hand the same id
    // back twice, and the dedicated path sets `dedicated: true`, which
    // excludes a shell from that scan forever. The `false` below is what
    // keeps a freshly opened shell reachable by the reuse rule.
    let id = match spawn_for_on_host(
        reg,
        caller.clone(),
        max_per_owner,
        host.clone(),
        false,
        spawn,
    )
    .await
    {
        Ok(id) => id,
        // The per-owner cap lives in `spawn_for`'s `check_owner_cap` breach,
        // returned as text rather than an `Err`: every tool result is a value
        // the model reads, so the refusal must name the configured limit.
        Err(e) => return Err(open_shell_error(max_per_owner, &e)),
    };
    // `spawn_for` reserved the record and rolled back only a *spawn* failure,
    // so a host that never came up leaves a record behind that would be
    // handed to the next caller as a live shell. Undo the reservation here.
    let socket = socket_path(id.as_str());
    if let Err(e) = await_ready(&socket, Instant::now() + ready_timeout).await {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(format!(
            "Error: shell {} did not become ready, and its empty registry record \
             was removed, so nothing can reach a dead shell: {}.",
            id.as_str(),
            format_err_chain(&e)
        ));
    }
    Ok(id)
}

/// The sentence the model reads after `open_shell` opened shell `id` on
/// `host`.
fn opened_shell_sentence(id: &ShellId, host: &crate::host::HostId) -> String {
    // The local sentence is byte-identical to the pre-host one (`HostId` has
    // no printable form); a named host is appended so the model can see where
    // the shell landed and address follow-up work there.
    let opened_on = match host {
        crate::host::HostId::Local => String::new(),
        crate::host::HostId::Named(name) => format!(" It is on host {name}."),
    };
    format!(
        "Opened shell {}. It is now registered and readable — list_shells lists \
         it, read_shell and find_in_shells read its recorded output, close_shell \
         ends it.{opened_on} Note: a command still runs in this session's \
         lowest-numbered open shell by default, so this shell sees work only \
         when it is that one — pass shell: {:?} to run_terminal_command to \
         target it directly, or shell: \"new\" for one command's exclusive \
         shell.",
        id.as_str(),
        id.as_str()
    )
}

/// The registry `open_shell`'s tests drive. The daemon-wide singleton is a
/// `OnceLock` that survives across tests, so a test that goes through the
/// public form needs a registry it can assert on afterwards — swap in a fresh
/// one and restore the old when the returned guard drops.
#[cfg(test)]
pub(crate) mod open_shell_test_seam {
    use std::sync::Mutex;

    static REPLACE: Mutex<Option<crate::shell::ShellRegistry>> = Mutex::new(None);

    /// Install `fresh` as the registry `open_shell` resolves, for as long as
    /// the returned guard is alive: dropping it puts back whatever `REPLACE`
    /// held before the swap, so nested swaps unwind in order and an outer
    /// test's registry survives an inner test's.
    pub(crate) fn swap_in(fresh: &crate::shell::ShellRegistry) -> SwapGuard {
        let previous = REPLACE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(fresh.clone());
        SwapGuard { previous }
    }

    /// The swapped-in registry, if any test currently holds one.
    pub(crate) fn swapped() -> Option<crate::shell::ShellRegistry> {
        REPLACE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Restores the pre-swap contents of `REPLACE` when dropped. Holds the
    /// *previous* value rather than clearing, so nesting restores in order.
    pub(crate) struct SwapGuard {
        previous: Option<crate::shell::ShellRegistry>,
    }

    impl Drop for SwapGuard {
        fn drop(&mut self) {
            *REPLACE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = self.previous.take();
        }
    }
}

#[cfg(test)]
fn open_shell_test_registry() -> crate::shell::ShellRegistry {
    match open_shell_test_seam::swapped() {
        Some(swapped) => swapped,
        None => crate::shell::registry().clone(),
    }
}

#[cfg(test)]
async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
    tests::spawn_in_process_host(id).await
}

/// The ignored ssh variant of the in-process host: the real `ssh` argv for
/// `pinky`, spawned by `PtyBackend` right here (not detached), so the process
/// bound to the shell's socket is an `ssh` talking to the real remote host
/// while the registry sees a normal in-process shell host.
#[cfg(test)]
pub(crate) async fn spawn_pinky_ssh(id: ShellId) -> anyhow::Result<()> {
    let control = crate::config::ssh_dir();
    use anyhow::Context as _;
    std::fs::create_dir_all(&control).with_context(|| format!("creating {}", control.display()))?;
    let entry = crate::shell::host_entry("pinky")?;
    let argv = crate::host::ssh_argv(&entry, &control);
    let program = crate::shell::which_ssh()?;
    let dir = tempfile::tempdir().context("tempdir for cast")?;
    let cast = dir.path().join(format!("{}.cast", id.as_str()));
    let backend =
        crate::shell::PtyBackend::spawn(&program, &argv, TEST_ROWS, TEST_COLS, 100, &cast, 0)
            .context("spawning ssh to pinky")?;
    let lock =
        crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
    let socket = crate::shell::socket_path(id.as_str());
    let listener = crate::shell::bind(&socket)
        .await
        .context("binding the in-process host's socket")?;
    tokio::spawn(crate::shell::serve(
        listener,
        std::sync::Arc::clone(&backend),
    ));
    std::mem::forget(lock);
    std::mem::forget(dir);
    Ok(())
}

#[cfg(not(test))]
fn open_shell_test_registry() -> crate::shell::ShellRegistry {
    crate::shell::registry().clone()
}

#[cfg(not(test))]
async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
    crate::shell::spawn_detached_host(id, &[], TEST_ROWS, TEST_COLS).await
}

async fn spawn_on_named_host(id: ShellId, name: &str) -> anyhow::Result<()> {
    crate::shell::spawn_ssh_host(id, name, TEST_ROWS, TEST_COLS).await
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

/// Close every shell `owner` holds and return `(id, sentence)` per shell.
///
/// 08c: a ghost's shells are closed when the ghost ends, so a scheduled
/// remote ghost does not leave one idle ssh shell host per run. Only
/// records whose `owner.key()` matches are touched — a chat session's
/// shells are never closed by a ghost's teardown.
pub(crate) async fn close_owner_shells(
    reg: &ShellRegistry,
    owner: &Owner,
    signal_timeout: Duration,
) -> Vec<(String, String)> {
    let key = owner.key();
    let ids = with_shells(reg, |store| {
        let mut ids: Vec<String> = store
            .values()
            .filter(|record| record.owner.key() == key)
            .map(|record| record.id.as_str().to_string())
            .collect();
        ids.sort_by_key(|id| ShellId::parse(id).map(|s| s.index()).unwrap_or(u32::MAX));
        ids
    });
    let mut closed = Vec::new();
    for id in ids {
        let sentence = close_shell_with(reg, &id, owner, signal_timeout).await;
        closed.push((id, sentence));
    }
    closed
}

/// `close_shell` with the registry and the signal timeout injected.
pub(crate) async fn close_shell_with(
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
    // `Close` hangs the shell up (SIGHUP to its pid), which also ends a job
    // running in it; a host too old to know `Close` gets TERM instead. Either
    // is sent exactly once. Whether the shell actually leaves is observed
    // afterwards, not assumed; a host that lingers still holds the lock, so
    // the tool says so instead of claiming success.
    let socket = socket_path(shell);
    if let Err(e) = crate::shell::close_or_term(&socket, signal_timeout).await {
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
    closed_by_observed_end(shell, &lock).await
}

/// The observable outcome of one close: poll the lock up to 1.5s from now
/// and report what is actually seen. A host releases its lock when its shell
/// exits (`serve_until_exit`), so a hung-up shell normally reads as gone well
/// inside the window; one that has not, is reported as still finishing, and
/// the tool's sentence never claims a closure it did not see.
async fn closed_by_observed_end(shell: &str, lock: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        if !host_is_alive(lock) {
            return format!(
                "Closed shell {shell}: its host is gone and the shell is no longer live."
            );
        }
        if std::time::Instant::now() >= deadline {
            return format!(
                "Asked shell {shell} to close, but its host is still holding its \
                 lock — the shell may still be finishing. list_shells still shows \
                 it; check again before assuming it is closed."
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
        let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        // Like a real host: serve until the shell exits, then release the lock.
        tokio::spawn(async move {
            let _ = crate::shell::serve_until_exit(
                listener,
                backend,
                std::time::Duration::from_millis(100),
            )
            .await;
            drop(lock);
        });
        std::mem::forget(dir);
        Ok(cast)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{
        ShellResponse, ShellState, bind, marker_shell_name, run_command, serve, socket_path, status,
    };
    use anyhow::Context as _;
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
            host: crate::host::HostId::Local,
            started_unix,
            interactive,
            dedicated,
            exited_unix,
        }
    }

    fn row_on(id: &str, owner_key: &str, host: crate::host::HostId, started_unix: u64) -> ShellRow {
        ShellRow {
            id: id.to_string(),
            owner_key: owner_key.to_string(),
            host,
            started_unix,
            interactive: false,
            dedicated: false,
            exited_unix: None,
        }
    }

    fn host_entry(ssh: &str) -> crate::config::HostEntry {
        crate::config::HostEntry {
            ssh: ssh.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn hosts_context_lists_local_then_each_host_with_live_shells() {
        let mut hosts = std::collections::HashMap::new();
        hosts.insert("web01".to_string(), host_entry("matt@web01.lan"));
        hosts.insert("db01".to_string(), host_entry("admin@db01"));
        let web = crate::host::HostId::Named("web01".to_string());
        let mut gone = row_on("s4", "chat:a", web.clone(), NOW);
        gone.exited_unix = Some(NOW);
        let rows = vec![
            row("s0", "chat:a", NOW, false, false, None),
            row_on("s2", "chat:a", web.clone(), NOW),
            row_on("s3", "ghost:job", web, NOW),
            gone,
        ];
        assert_eq!(
            format_hosts_context(&hosts, &rows),
            "[HOSTS] local (1 shell) · db01 (ssh admin@db01, no shells) · web01 (ssh matt@web01.lan, 2 shells)\n"
        );
    }

    #[test]
    fn hosts_context_is_empty_without_configured_hosts() {
        let rows = vec![row("s0", "chat:a", NOW, false, false, None)];
        assert_eq!(
            format_hosts_context(&std::collections::HashMap::new(), &rows),
            ""
        );
    }

    #[test]
    fn a_named_host_row_carries_the_host_through_the_formatter() {
        let rows = vec![row_on(
            "s9",
            "chat:a",
            crate::host::HostId::Named("pinky".to_string()),
            NOW - 30,
        )];
        let out = format_shell_rows(&rows, None, NOW);
        assert!(
            out.contains("s9  owner:chat:a host:pinky"),
            "the helper-built named-host row reaches the formatter: {out}"
        );
    }

    fn insert(reg: &ShellRegistry, index: u32, owner: Owner, started_unix: u64) {
        with_shells(reg, |store| {
            store.insert(
                format!("s{index}"),
                ShellRecord {
                    id: ShellId::from_index(index),
                    owner,
                    host: crate::host::HostId::Local,
                    sandbox: None,
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
    pub(super) async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
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
    pub(crate) fn lifecycle_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        (guard, dir)
    }

    // -- default_shell_for (phase-08a) --------------------------------------

    #[test]
    fn default_shell_for_picks_the_lowest_index_not_the_lowest_string() {
        let rows = vec![
            row("s10", "chat:a", NOW - 100, false, false, None),
            row("s2", "chat:a", NOW - 100, false, false, None),
        ];
        let got = default_shell_for(&rows, "chat:a");
        assert_eq!(
            got,
            Some("s2".to_string()),
            "s2 must beat s10: a numeric index, not a string sort"
        );
    }

    #[test]
    fn default_shell_for_skips_an_interactive_shell() {
        let rows = vec![
            row("s0", "chat:a", NOW - 100, true, false, None),
            row("s1", "chat:a", NOW - 100, false, false, None),
        ];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            Some("s1".to_string()),
            "an interactive shell must not be named as the default, s1 is"
        );
    }

    #[test]
    fn default_shell_for_skips_a_dedicated_shell() {
        let rows = vec![
            row("s0", "chat:a", NOW - 100, false, true, None),
            row("s1", "chat:a", NOW - 100, false, false, None),
        ];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            Some("s1".to_string()),
            "a dedicated shell belongs to one command, s1 is the default"
        );
    }

    #[test]
    fn default_shell_for_skips_an_exited_shell() {
        let rows = vec![
            row("s0", "chat:a", NOW - 100, false, false, Some(NOW)),
            row("s1", "chat:a", NOW - 100, false, false, None),
        ];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            Some("s1".to_string()),
            "a stamped-exited shell must not be named, s1 is"
        );
    }

    #[test]
    fn default_shell_for_ignores_another_owners_shell() {
        let rows = vec![
            row("s0", "chat:other", NOW - 100, false, false, None),
            row("s1", "chat:a", NOW - 100, false, false, None),
        ];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            Some("s1".to_string()),
            "another owner's shell is invisible to the selector"
        );
    }

    #[test]
    fn default_shell_for_returns_none_when_every_shell_is_ineligible() {
        let rows = vec![row("s0", "chat:a", NOW - 100, true, false, None)];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            None,
            "an interactive-only shell set has no default"
        );
    }

    /// Task 4: the default is the lowest-indexed **local** shell. A remote
    /// shell is never named — a bare `run_terminal_command` would not land
    /// there, so naming it in the prompt is the defect this phase fixes.
    #[test]
    fn default_shell_for_skips_a_remote_shell() {
        let rows = vec![row_on(
            "s0",
            "chat:a",
            crate::host::HostId::Named("pinky".to_string()),
            NOW - 100,
        )];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            None,
            "a session whose only shells are remote has no default"
        );
    }

    /// The negative-ordering case: index order alone would answer `s1`; only
    /// a host term the ordering cannot overrule answers `s2`.
    #[test]
    fn default_shell_for_prefers_the_local_shell_over_a_lower_indexed_remote_one() {
        let rows = vec![
            row_on(
                "s1",
                "chat:a",
                crate::host::HostId::Named("pinky".to_string()),
                NOW - 100,
            ),
            row("s2", "chat:a", NOW - 100, false, false, None),
        ];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            Some("s2".to_string()),
            "the local s2 is the default even though the remote s1 has the lower index"
        );
    }

    // -- format_shell_context (phase-08a) -----------------------------------

    #[test]
    fn format_shell_context_is_empty_when_there_are_no_rows() {
        let out = format_shell_context(&[], "chat:a", NOW);
        assert_eq!(
            out, "",
            "no rows must give the empty string, no header line"
        );
    }

    #[test]
    fn format_shell_context_lists_every_shell_with_its_flags() {
        let rows = vec![
            row("s2", "chat:a", NOW - 12, true, false, None),
            row("s0", "chat:a", NOW - 240, false, false, None),
            row("s4", "chat:b", NOW - 240, false, true, Some(NOW - 5)),
        ];
        let out = format_shell_context(&rows, "chat:a", NOW);
        assert!(
            out.starts_with("[SHELLS] 3 — s0 age:4m, s2 age:12s [interactive], s4 age:"),
            "numeric order (s0, s2, s4) with flags on the right rows: {out}"
        );
        assert!(
            out.contains(
                "[DEFAULT SHELL] s0 — run_terminal_command with no shell argument lands here"
            ),
            "the eligible lowest shell is named as the default: {out}"
        );
    }

    #[test]
    fn format_shell_context_keeps_each_flag_on_its_own_row() {
        let rows = vec![
            row("s0", "chat:a", NOW - 240, false, false, None),
            row("s2", "chat:a", NOW - 12, true, false, None),
            row("s4", "chat:b", NOW - 240, false, true, Some(NOW - 5)),
        ];
        let out = format_shell_context(&rows, "chat:a", NOW);
        let count_line = out
            .lines()
            .find(|line| line.starts_with("[SHELLS] "))
            .unwrap_or_else(|| panic!("no [SHELLS] line in output: {out}"));
        let listing = count_line
            .split_once(" — ")
            .map(|(_, rest)| rest)
            .expect("count line must contain the ' — ' separator");
        let segments: Vec<&str> = listing.split(", ").collect();
        assert_eq!(
            segments.len(),
            3,
            "the count line carries one segment per shell: {out}"
        );
        assert!(
            !segments[0].contains('['),
            "s0's segment must carry no flag: segment {:?} in {out}",
            segments[0]
        );
        assert!(
            segments[1].contains("[interactive]"),
            "s2's segment must carry [interactive]: segment {:?} in {out}",
            segments[1]
        );
        assert!(
            segments[2].contains("[dedicated]"),
            "s4's segment must carry [dedicated]: segment {:?} in {out}",
            segments[2]
        );
        assert!(
            !segments[0].contains("[dedicated]") && !segments[1].contains("[dedicated]"),
            "[dedicated] must ride only s4's segment: {out}"
        );
    }

    #[test]
    fn format_shell_context_omits_the_default_line_when_none_is_eligible() {
        let rows = vec![row("s0", "chat:a", NOW - 100, true, false, None)];
        let out = format_shell_context(&rows, "chat:a", NOW);
        assert!(
            out.contains("[SHELLS] 1"),
            "the single interactive row is still counted: {out}"
        );
        assert!(
            !out.contains("[DEFAULT SHELL]"),
            "the default line must be omitted entirely, not named 'none': {out}"
        );
    }

    /// Task 4's prompt-facing half: the remote shell still appears in the
    /// `[SHELLS]` listing with its host, but the `[DEFAULT SHELL]` line is
    /// gone entirely — its own text promises where a bare command lands, and
    /// for a remote shell that promise is false.
    #[test]
    fn shell_context_omits_the_default_line_when_every_shell_is_remote() {
        let rows = vec![row_on(
            "s1",
            "chat:a",
            crate::host::HostId::Named("pinky".to_string()),
            NOW - 30,
        )];
        let out = format_shell_context(&rows, "chat:a", NOW);
        assert!(
            out.contains("[SHELLS] 1") && out.contains("s1") && out.contains("pinky"),
            "the remote shell is still listed with its host: {out}"
        );
        assert!(
            !out.contains("[DEFAULT SHELL]"),
            "a remote-only session has no default; the line's promise would be false: {out}"
        );
    }

    // -- the injected-registry seam (phase-08a) -----------------------------

    #[tokio::test]
    async fn e2e_shell_context_block_names_a_real_hosts_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("e2e".into());
        let out = open_shell_with(
            &reg,
            &owner,
            crate::host::HostId::Local,
            5,
            Duration::from_millis(3000),
            spawn_in_process_host,
        )
        .await;
        assert!(out.contains("Opened shell"), "setup must succeed: {out}");
        let block = shell_context_block_in(&reg, &owner, NOW);
        println!("PASTE MATCH >>>\n{block}<<<");
        assert!(
            block.starts_with("[SHELLS] 1 — s"),
            "the block must count and name the real shell: {block}"
        );
        assert!(
            block.contains("[DEFAULT SHELL] s"),
            "a freshly opened non-dedicated shell is the default: {block}"
        );
    }

    // -- resolve_context_scope / shells_only_context (phase-08b) ------------

    #[test]
    fn context_scope_defaults_to_session() {
        assert_eq!(
            resolve_context_scope(None),
            Ok(ContextBreadth::Session),
            "an absent scope is the default breadth, session"
        );
    }

    #[test]
    fn context_scope_accepts_the_three_values() {
        assert_eq!(
            resolve_context_scope(Some("shells")),
            Ok(ContextBreadth::ShellsOnly)
        );
        assert_eq!(
            resolve_context_scope(Some("session")),
            Ok(ContextBreadth::Session)
        );
        assert_eq!(resolve_context_scope(Some("all")), Ok(ContextBreadth::All));
    }

    #[test]
    fn context_scope_rejects_retired_window_and_run() {
        for retired in ["window", "run"] {
            let err = resolve_context_scope(Some(retired))
                .expect_err("a retired vocabulary must not resolve to a breadth");
            assert!(
                err.starts_with("Error:"),
                "the refusal must read as a tool-result error: {err}"
            );
            assert!(
                err.contains(retired),
                "the refusal must name the rejected value: {err}"
            );
            for accepted in ["shells", "session", "all"] {
                assert!(
                    err.contains(accepted),
                    "the refusal must name the accepted value {accepted:?}: {err}"
                );
            }
        }
    }

    #[test]
    fn context_scope_rejects_unknown_value() {
        let err = resolve_context_scope(Some("banana"))
            .expect_err("an arbitrary string is not a breadth");
        assert!(
            err.contains("banana"),
            "the error must echo the input: {err}"
        );
    }

    #[test]
    fn shells_only_context_without_shells_is_not_empty() {
        let (_home, _dir) = lifecycle_home();
        let out = shells_only_context(&Owner::Chat("empty".into()));
        assert!(
            !out.is_empty(),
            "an empty tool result reads as a failed call; got: {out:?}"
        );
        assert!(
            out.contains("open_shell"),
            "the nothing-here sentence must point at how to get one: {out}"
        );
        assert!(
            out.ends_with('\n'),
            "the non-empty branch ends in a newline; this one must too: {out:?}"
        );
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

    /// Local rendering is pinned **exactly**: the phase's rule is that a
    /// `HostId::Local` shell's lines are byte-identical to the pre-host
    /// output, so a user with no hosts configured sees nothing new.
    #[test]
    fn local_shells_render_without_a_host() {
        let rows = vec![row("s1", "chat:a", NOW - 30, false, false, None)];
        let exact = format_shell_rows(&rows, None, NOW);
        assert_eq!(
            exact, "1 shell (scope \"mine\"):\n  s1  owner:chat:a  age:30s\n",
            "the local line must be today's bytes, no host token: {exact:?}"
        );
        let block = format_shell_context(&rows, "chat:a", NOW);
        assert_eq!(
            block,
            "[SHELLS] 1 — s1 age:30s\n[DEFAULT SHELL] s1 — run_terminal_command \
             with no shell argument lands here\n",
            "the ambient block must be today's bytes too: {block:?}"
        );
    }

    /// A named host's token appears on both surfaces. Nothing spawns an SSH
    /// shell in this phase — the row is constructed directly.
    #[test]
    fn a_named_host_is_shown() {
        let host = crate::host::HostId::Named("pinky".to_string());
        let rows = vec![ShellRow {
            id: "s1".to_string(),
            owner_key: "chat:a".to_string(),
            host: host.clone(),
            started_unix: NOW - 30,
            interactive: false,
            dedicated: false,
            exited_unix: None,
        }];
        let out = format_shell_rows(&rows, None, NOW);
        assert!(
            out.contains("host:pinky"),
            "list_shells names the host: {out}"
        );
        let line = out.lines().find(|l| l.contains("s1")).expect("the line");
        assert!(
            line.contains("owner:chat:a host:pinky"),
            "the token sits after the owner: {line}"
        );
        let block = format_shell_context(&rows, "chat:a", NOW);
        assert!(
            block.contains("s1 host:pinky"),
            "the ambient block names the host: {block}"
        );
        assert!(
            !block.contains("host:local"),
            "`local` is implicit and must never render: {block}"
        );
        let _ = host;
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
                owner: None,
                host: None,
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
            owner: None,
            host: None,
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
            crate::host::HostId::Local,
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
            crate::host::HostId::Local,
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
            crate::host::HostId::Local,
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

    // ── open_shell's host argument (M21 phase-04b) ───────────────────────────

    /// § Current state 5 pinned as behaviour: a host problem is reported as a
    /// host problem, never dressed up in per-owner shell-limit advice the
    /// model cannot act on. `host_entry`'s refusal names the unknown host and
    /// lists the configured ones; the cap sentence names neither.
    #[tokio::test]
    async fn open_shell_on_an_unknown_host_names_the_host_not_the_shell_limit() {
        let (_home, dir) = lifecycle_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n\n[hosts.db01]\nssh = \"db01\"\n",
        )
        .expect("write config");
        let reg = ShellRegistry::default();
        let _swap = open_shell_test_seam::swap_in(&reg);
        let out = open_shell(&Owner::Chat("hosted".into()), Some("nope")).await;
        assert!(
            out.contains("nope"),
            "the refusal must name the host the model asked for: {out}"
        );
        assert!(
            out.contains("web01") && out.contains("db01"),
            "the refusal must list the configured hosts so the model can correct itself: {out}"
        );
        assert!(
            !out.contains("per-owner shell limit"),
            "a host problem must never arrive as shell-limit advice: {out}"
        );
        assert_eq!(
            with_shells(&reg, |store| store.len()),
            0,
            "a refused host must not leave a record behind"
        );
    }

    /// The host named at open time rides the record and reaches the model
    /// again through `list_shells` — the plumbing's whole point.
    #[tokio::test]
    async fn open_shell_on_a_named_host_records_the_host() {
        let (_home, dir) = lifecycle_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .expect("write config");
        let reg = ShellRegistry::default();
        let out = open_shell_with(
            &reg,
            &Owner::Chat("web".into()),
            crate::host::HostId::Named("web01".to_string()),
            5,
            Duration::from_millis(3000),
            spawn_in_process_host,
        )
        .await;
        assert!(
            out.contains("Opened shell") && out.contains("web01"),
            "the precondition is a ready shell on web01: {out}"
        );
        let record_host = with_shells(&reg, |store| store.values().next().map(|r| r.host.clone()));
        assert_eq!(
            record_host,
            Some(crate::host::HostId::Named("web01".to_string())),
            "the record must carry the host open_shell_with was given"
        );
        let listed = list_shells_in(&reg, None, &Owner::Chat("web".into()), NOW);
        assert!(
            listed.contains("host:web01"),
            "list_shells must render the host so the model sees where the shell is: {listed}"
        );
    }

    /// No `host` naming is today's behaviour: the record stays stamped
    /// `HostId::Local`, and because the renderer keeps the local token empty,
    /// `list_shells` prints exactly as it did before hosts existed.
    #[tokio::test]
    async fn open_shell_without_a_host_records_the_local_host() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let _swap = open_shell_test_seam::swap_in(&reg);
        let out = open_shell(&Owner::Chat("local".into()), None).await;
        assert!(
            out.contains("Opened shell"),
            "the precondition is a ready local shell: {out}"
        );
        let record_host = with_shells(&reg, |store| store.values().next().map(|r| r.host.clone()));
        assert_eq!(
            record_host,
            Some(crate::host::HostId::Local),
            "the no-host form must keep the local stamp"
        );
        let listed = list_shells_in(&reg, None, &Owner::Chat("local".into()), NOW);
        assert!(
            !listed.contains("host:"),
            "an unhosted shell must render exactly as before hosts existed: {listed}"
        );
    }

    /// The no-host result sentence is the contract every existing caller and
    /// test was written against — it must not shift by a byte.
    #[tokio::test]
    async fn open_shell_local_result_is_unchanged() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let _swap = open_shell_test_seam::swap_in(&reg);
        let out = open_shell(&Owner::Chat("sentinel".into()), None).await;
        let id = out
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            })
            .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
            .expect("the success sentence names the id");
        assert_eq!(
            out,
            format!(
                "Opened shell {id}. It is now registered and readable — list_shells \
                 lists it, read_shell and find_in_shells read its recorded output, \
                 close_shell ends it. Note: a command still runs in this session's \
                 lowest-numbered open shell by default, so this shell sees work only \
                 when it is that one — pass shell: \"{id}\" to run_terminal_command to \
                 target it directly, or shell: \"new\" for one command's exclusive shell."
            ),
            "the no-host sentence must be byte-identical to the pre-host contract"
        );
    }

    /// A name outside `[A-Za-z0-9._-]` (or over 64 chars) is refused before
    /// any spawn, with `HostId::parse`'s own message — not with shell-limit
    /// advice.
    #[tokio::test]
    async fn open_shell_rejects_a_malformed_host_name_before_any_spawn() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let _swap = open_shell_test_seam::swap_in(&reg);
        let out = open_shell(&Owner::Chat("weird".into()), Some("bad name!")).await;
        assert!(
            out.starts_with("Error:"),
            "a malformed name is refused: {out}"
        );
        assert!(
            !out.contains("per-owner shell limit"),
            "a malformed name is a name problem, not a shell-limit problem: {out}"
        );
        assert_eq!(
            with_shells(&reg, |store| store.len()),
            0,
            "a refused name must not leave a record behind"
        );
    }

    /// Task 1's contract: the guard restores what `REPLACE` held *before* the
    /// swap, so a nested swap's drop puts the outer swap back rather than
    /// clearing to `None`.
    ///
    /// `test_home_guard()` is here for its **lock**, not its `HOME` swap: the
    /// `REPLACE` global below is shared with the five `open_shell` seam tests
    /// (`#[test]`s run in parallel threads), and this test both reads it as a
    /// precondition and mutates it twice. The other five serialize on
    /// `lifecycle_home()`, which bundles the same lock; this test needs no
    /// `$HOME`, so it takes the lock by itself.
    #[test]
    fn swap_in_restores_the_previous_registry_on_drop() {
        let _home = crate::test_home_guard();
        let outer = ShellRegistry::default();
        let inner = ShellRegistry::default();
        assert!(
            open_shell_test_seam::swapped().is_none(),
            "precondition: no swap is in scope (this test must not depend on another)"
        );
        let _outer = open_shell_test_seam::swap_in(&outer);
        {
            let _inner = open_shell_test_seam::swap_in(&inner);
            assert!(
                open_shell_test_seam::swapped().is_some(),
                "the inner swap is in force while its guard lives"
            );
        }
        assert!(
            open_shell_test_seam::swapped().is_some(),
            "the inner drop restores the outer swap, it does not clear to none"
        );
        drop(_outer);
        assert!(
            open_shell_test_seam::swapped().is_none(),
            "the outer drop restores the empty state the test found"
        );
    }

    /// The case 04b's review named as the seam's latent flake: a caller that
    /// goes through the public `open_shell` with **no** swap in scope must
    /// resolve the daemon singleton — a fresh, empty registry — not whatever
    /// an earlier test left in the seam. An unresolved (leaked or stale)
    /// seam is what would make this test order-dependent.
    #[tokio::test]
    async fn open_shell_without_a_swap_uses_the_daemon_registry() {
        let (_home, _dir) = lifecycle_home();
        assert!(
            open_shell_test_seam::swapped().is_none(),
            "precondition: no swap is in scope (this test must not depend on another)"
        );
        let out = open_shell(&Owner::Chat("no-swap".into()), None).await;
        assert!(
            out.contains("Opened shell"),
            "the precondition is a ready local shell: {out}"
        );
        // The record landed in the daemon singleton, and the seam is still
        // empty — nothing was installed.
        let singleton_ids = with_shells(crate::shell::registry(), |store| store.len());
        assert!(
            singleton_ids >= 1,
            "the record must land in the daemon registry, not a swapped one: {singleton_ids}"
        );
        assert!(
            open_shell_test_seam::swapped().is_none(),
            "the path must not install a registry of its own"
        );
    }

    // ── live host (ignored; run by hand) ─────────────────────────────────────

    /// M21 phase-04b's end-to-end check: `open_shell { host: "pinky" }` really
    /// runs on the remote host. requiring the live host `pinky` over ssh with
    /// key auth (no prompt). It goes through `open_shell_with` — the tool
    /// path, not a direct `ssh` — with the spawn step bound in-process
    /// (the real `ssh` argv; `spawn_ssh_host` cannot run under a test binary
    /// because `launch::spawn_detached` re-invokes `current_exe()`), then
    /// drives the shell through the ordinary `run_terminal_command` protocol
    /// and prints the four facts on one line.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn open_shell_on_pinky_end_to_end() {
        let (_home, dir) = lifecycle_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();

        let reg = ShellRegistry::default();
        let owner = Owner::Chat("e2e-host".into());
        let out = open_shell_with(
            &reg,
            &owner,
            crate::host::HostId::Named("pinky".to_string()),
            5,
            Duration::from_secs(120),
            |id| async move { spawn_pinky_ssh(id).await },
        )
        .await;
        assert!(
            out.contains("Opened shell"),
            "the shell must open on pinky: {out}"
        );
        let id = out
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            })
            .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
            .expect("the success sentence names the id");

        let record_host = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.host.clone()));
        let record_host = match record_host {
            Some(crate::host::HostId::Named(name)) => name,
            other => panic!("the record must name pinky, got {other:?}"),
        };

        let listed = list_shells_in(&reg, None, &owner, NOW);
        assert!(
            listed.contains(&format!("host:{record_host}")),
            "list_shells must render the host: {listed}"
        );

        let hostname = run_command(
            &crate::shell::socket_path(&id),
            "hostname",
            &marker_shell_name(&crate::host::HostId::Named(record_host.clone())),
            Duration::from_secs(60),
        )
        .await
        .expect("the remote shell answers `hostname`");
        let hostname_out = String::from_utf8_lossy(&hostname.output).to_string();
        assert!(
            hostname_out.contains("pinky"),
            "`hostname` must say pinky: {hostname_out:?}"
        );

        let exit42 = run_command(
            &crate::shell::socket_path(&id),
            "(exit 42)",
            &marker_shell_name(&crate::host::HostId::Named(record_host.clone())),
            Duration::from_secs(60),
        )
        .await
        .expect("the remote shell answers `(exit 42)`");

        println!(
            "record_host={record_host} listed_host={record_host} remote_hostname={} remote_exit_code={}",
            hostname_out.trim().lines().last().unwrap_or(""),
            exit42.exit_code,
        );

        close_shell_with(&reg, &id, &owner, Duration::from_millis(3000)).await;
        let cp = crate::config::ssh_dir();
        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", cp.display()),
                "pinky",
            ])
            .status();
    }

    /// `run_terminal_command`'s run path with the transport's write/read halves
    /// bound to sink/EOF (no sudo) and the spawn step in-process over the real
    /// `ssh` argv — the live-test shape [`open_shell_on_pinky_end_to_end`]
    /// mirrors, but through the reuse-or-create step instead of `open_shell`.
    async fn run_pinky_via_run_path(
        reg: &ShellRegistry,
        owner: Owner,
        cmd: &str,
    ) -> anyhow::Result<(ShellId, crate::daemon::executor::shell_exec::ShellRunResult)> {
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::empty();
        crate::daemon::executor::shell_exec::run_in_shell_with_prompts(
            reg,
            owner,
            cmd,
            5,
            None,
            Some(120),
            "",
            &mut sink,
            &mut eof,
            crate::host::HostId::Named("pinky".to_string()),
            |id| async move { spawn_pinky_ssh(id).await },
        )
        .await
    }

    /// M21 phase-04d's end-to-end check: `run_terminal_command { host: "pinky" }`
    /// really runs on the remote host. Same preconditions as
    /// [`open_shell_on_pinky_end_to_end`]: the live host `pinky` over ssh with
    /// key auth (no prompt). The run path is [`run_in_shell_core`]'s host
    /// parameter — the reuse-or-create step the tool drives — with the spawn
    /// step bound in-process (the real `ssh` argv; `spawn_ssh_host` cannot run
    /// under a test binary because `launch::spawn_detached` re-invokes
    /// `current_exe()`), then the command's output and exit code are asserted
    /// against the real remote and printed on one line.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn run_terminal_command_on_pinky_end_to_end() {
        let (_home, dir) = lifecycle_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();

        let reg = ShellRegistry::default();
        let owner = Owner::Chat("e2e-rc-host".into());
        let (id, first) = run_pinky_via_run_path(&reg, owner.clone(), "hostname")
            .await
            .expect("the pinky run resolves");
        assert_eq!(first.exit_code, 0, "`hostname` ran cleanly on pinky");
        let hostname_out = first.text.trim().lines().last().unwrap_or("").to_string();
        assert!(
            hostname_out.contains("pinky"),
            "`hostname` must say pinky: {hostname_out:?}"
        );

        let record_host = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.host.clone()));
        assert_eq!(
            record_host,
            Some(crate::host::HostId::Named("pinky".to_string())),
            "the record must carry the host the command was asked for"
        );

        let (id2, second) = run_pinky_via_run_path(&reg, owner.clone(), "(exit 42)")
            .await
            .expect("the exit-42 run resolves");
        assert_eq!(id2.as_str(), id.as_str(), "the second command reused s*");
        assert_eq!(second.exit_code, 42, "the remote non-zero exit propagates");

        println!(
            "record_host=pinky reused={reused} remote_hostname={hostname_out} remote_exit_code={}",
            second.exit_code,
            reused = id2.as_str(),
        );

        close_shell_with(&reg, id.as_str(), &owner, Duration::from_millis(3000)).await;
        let cp = crate::config::ssh_dir();
        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", cp.display()),
                "pinky",
            ])
            .status();
    }

    // ── close_shell ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn close_shell_ends_the_callers_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let opened = open_shell_with(
            &reg,
            &Owner::Chat("closer".into()),
            crate::host::HostId::Local,
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

    /// 08c Task 4.1: closing an owner's shells ends **only** that owner's
    /// shells. The hosts run `sh -c 'sleep 1000'`, not bash — an interactive
    /// bash ignores TERM (measured at drafting, § Current state 3), so a
    /// bash host would not report `Exited` on either side and the test would
    /// pin nothing.
    #[tokio::test]
    async fn closing_an_owners_shells_ends_only_that_owners_shells() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let spawn_sleeping_host = |id: ShellId| async move {
            let dir = tempfile::tempdir().context("tempdir for cast")?;
            let cast = dir.path().join(format!("{}.cast", id.as_str()));
            let backend = crate::shell::PtyBackend::spawn(
                "sh",
                &["-c".to_string(), "sleep 1000".to_string()],
                TEST_ROWS,
                TEST_COLS,
                100,
                &cast,
                0,
            )
            .context("spawning sh -c 'sleep 1000'")?;
            let lock = crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(
                id.as_str(),
            ))
            .context("holding the per-id instance lock")?;
            let socket = socket_path(id.as_str());
            let listener = bind(&socket).await.context("binding the host's socket")?;
            tokio::spawn(serve(listener, std::sync::Arc::clone(&backend)));
            std::mem::forget(lock);
            std::mem::forget(dir);
            Ok(())
        };
        let owner_a = Owner::Ghost("a".into());
        let owner_b = Owner::Ghost("b".into());
        let id_a = spawn_for_on_host(
            &reg,
            owner_a.clone(),
            5,
            crate::host::HostId::Local,
            false,
            spawn_sleeping_host,
        )
        .await
        .expect("a's shell spawns");
        let id_b = spawn_for_on_host(
            &reg,
            owner_b.clone(),
            5,
            crate::host::HostId::Local,
            false,
            spawn_sleeping_host,
        )
        .await
        .expect("b's shell spawns");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        await_ready(&socket_path(id_a.as_str()), deadline)
            .await
            .expect("a's host is ready");
        await_ready(&socket_path(id_b.as_str()), deadline)
            .await
            .expect("b's host is ready");

        let closed = close_owner_shells(&reg, &owner_a, Duration::from_secs(3)).await;
        assert_eq!(
            closed.len(),
            1,
            "exactly one shell belongs to ghost:a: {closed:?}"
        );
        assert_eq!(
            closed[0].0,
            id_a.as_str(),
            "the closed shell is a's, not b's: {closed:?}"
        );

        let mut a_exited = false;
        let mut b_exited = false;
        for _ in 0..40 {
            let state_a = status(&socket_path(id_a.as_str()), Duration::from_millis(100)).await;
            let state_b = status(&socket_path(id_b.as_str()), Duration::from_millis(100)).await;
            a_exited = a_exited
                || matches!(state_a, Ok(ShellResponse::Exited { .. }))
                || matches!(&state_a,
                    Ok(ShellResponse::Status { state, .. }) if *state == ShellState::Exited);
            b_exited = b_exited
                || matches!(state_b, Ok(ShellResponse::Exited { .. }))
                || matches!(&state_b,
                    Ok(ShellResponse::Status { state, .. }) if *state == ShellState::Exited);
            if a_exited {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(a_exited, "a's shell reports Exited within 2 s of the close");
        assert!(!b_exited, "b's shell must not be touched by a's teardown");
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

    #[test]
    fn input_target_note_names_the_prompt_hazard_when_idle() {
        let idle = input_target_note(Some(crate::shell::ShellState::Idle));
        assert!(
            idle.contains("prompt") && idle.contains("shell command"),
            " Idle must say the text will be run as a shell command: {idle}"
        );
        let running = input_target_note(Some(crate::shell::ShellState::Running));
        assert!(
            running.contains("program") && running.contains("NOT"),
            "Running must say the input goes to the running program: {running}"
        );
        assert_ne!(
            idle, running,
            "a note that says the same thing in every state tells the approving \
             user nothing"
        );
        assert!(
            input_target_note(Some(crate::shell::ShellState::Paused)).contains("stopped"),
            "Paused names the stopped job: {}",
            input_target_note(Some(crate::shell::ShellState::Paused))
        );
        assert!(
            input_target_note(Some(crate::shell::ShellState::Exited)).contains("gone"),
            "Exited says the shell may be gone: {}",
            input_target_note(Some(crate::shell::ShellState::Exited))
        );
        assert!(
            input_target_note(None).contains("could not be probed"),
            "a failed probe says so: {}",
            input_target_note(None)
        );
    }

    #[tokio::test]
    async fn send_input_appends_a_newline_by_default() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("caller".into()), NOW);
        let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
        let out = send_input_with(
            &reg,
            "s3",
            "hello",
            true,
            &Owner::Chat("caller".into()),
            Duration::from_secs(5),
            |bytes| {
                let cap = &captured;
                async move {
                    cap.lock().unwrap().push(bytes);
                    Ok(())
                }
            },
        )
        .await;
        assert!(
            out.starts_with("Sent") && out.contains("s3"),
            "the report names the shell and what was sent: {out}"
        );
        assert!(
            out.contains("newline appended"),
            "the newline is reported: {out}"
        );
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [b"hello\n".to_vec()],
            "exit: true appends exactly one \\n — a length cannot tell it from \\r, \
             so the bytes are asserted whole"
        );
    }

    #[tokio::test]
    async fn the_users_shell_is_visible_and_drivable_from_chat_only() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("a".into()), NOW);
        insert(&reg, 7, Owner::User, NOW);
        insert(&reg, 8, Owner::Chat("b".into()), NOW);
        let mine = list_shells_in(&reg, None, &Owner::Chat("a".into()), NOW);
        assert!(mine.contains("s3") && mine.contains("s7"), "{mine}");
        assert!(
            !mine.contains("s8"),
            "another session's shell stays out: {mine}"
        );
        assert!(crate::shell::is_user_shell(&reg, "s7"));
        assert!(!crate::shell::is_user_shell(&reg, "s3"));

        let sent: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
        let send = |bytes: Vec<u8>| {
            let sent = &sent;
            async move {
                sent.lock().unwrap().push(bytes);
                Ok(())
            }
        };
        let out = send_input_with(
            &reg,
            "s7",
            "ls",
            true,
            &Owner::Chat("a".into()),
            Duration::from_secs(5),
            send,
        )
        .await;
        assert!(!out.contains("not yours"), "{out}");
        assert_eq!(
            sent.lock().unwrap().len(),
            1,
            "a chat session types into it"
        );

        let out = send_input_with(
            &reg,
            "s7",
            "ls",
            true,
            &Owner::Ghost("g".into()),
            Duration::from_secs(5),
            send,
        )
        .await;
        assert!(out.contains("not yours"), "a ghost is refused: {out}");
        assert_eq!(sent.lock().unwrap().len(), 1, "nothing reached the shell");
    }

    #[tokio::test]
    async fn send_input_without_enter_sends_no_newline() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("caller".into()), NOW);
        let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
        let out = send_input_with(
            &reg,
            "s3",
            "hello",
            false,
            &Owner::Chat("caller".into()),
            Duration::from_secs(5),
            |bytes| {
                let cap = &captured;
                async move {
                    cap.lock().unwrap().push(bytes);
                    Ok(())
                }
            },
        )
        .await;
        assert!(
            out.contains("no newline"),
            "the absence of the newline is reported: {out}"
        );
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [b"hello".to_vec()],
            "exit: false sends the text alone, no terminator"
        );
    }

    #[tokio::test]
    async fn send_input_refuses_another_owners_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        // Same inner string "a", different owner kind — the shape that breaks
        // a comparison on the inner string instead of `Owner::key()`.
        insert(&reg, 4, Owner::Ghost("a".into()), NOW);
        let sent: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let out = send_input_with(
            &reg,
            "s4",
            "whoami",
            true,
            &Owner::Chat("a".into()),
            Duration::from_secs(5),
            |_bytes| {
                sent.store(true, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        assert!(
            out.starts_with("Error:"),
            "another owner's shell is refused, as text not an Err: {out}"
        );
        assert!(out.contains("s4"), "the refused id is named: {out}");
        assert!(
            has_record(&reg, "s4"),
            "a refusal must not remove the shell from the registry"
        );
        assert!(
            !sent.load(Ordering::SeqCst),
            "the send must never have been attempted across owners"
        );
    }

    #[tokio::test]
    async fn send_input_unknown_id_is_an_error_naming_live_shells() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 5, Owner::Chat("caller".into()), NOW);
        let sent: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let out = send_input_with(
            &reg,
            "s99",
            "echo hi",
            false,
            &Owner::Chat("caller".into()),
            Duration::from_secs(5),
            |_bytes| {
                sent.store(true, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        assert!(
            out.starts_with("Error:"),
            "an unknown id is error text, not a panic and not an Err: {out}"
        );
        assert!(
            out.contains("s99") && out.contains("s5"),
            "the unknown id and the live ids are both named: {out}"
        );
        assert!(out.contains("list_shells"));
        assert!(!sent.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn send_input_text_never_reaches_read_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        spawn_in_process_host(ShellId::from_index(3))
            .await
            .expect("host binds the socket");
        insert(&reg, 3, Owner::Chat("caller".into()), NOW);
        let out = send_input_with(
            &reg,
            "s3",
            "SENTINEL-NOT-IN-OUTPUT",
            false,
            &Owner::Chat("caller".into()),
            Duration::from_secs(5),
            |bytes| async move {
                crate::shell::send_input(&socket_path("s3"), &bytes, Duration::from_secs(5)).await
            },
        )
        .await;
        assert!(
            !out.starts_with("Error"),
            "the real path accepted the write: {out}"
        );
        // The host writes its `.cast` under the `HOME` it inherited at spawn
        // (see spawn_with_cast, whose tempdir homes both the PTY and the log);
        // the test's `lifecycle_home()` dir holds none of it by construction.
        // Production-path half of the safety property: the typed text really
        // reached a real host's cast as an `"i"` event. The read-side half —
        // that no reader surfaces it — is `…_in_a_reachable_cast` below,
        // which holds the cast the reader opens under the test's own `HOME`.
        let stray = find_stray_cast("SENTINEL-NOT-IN-OUTPUT");
        let recorded = std::fs::read_to_string(std::path::PathBuf::from(&stray))
            .unwrap_or_else(|_| panic!("reading the cast the input landed in: {stray}"));
        assert!(
            recorded.contains("SENTINEL-NOT-IN-OUTPUT"),
            "the \"i\" event is really written to the cast"
        );
    }

    /// The read-side half of the safety property, over a cast the read tools
    /// can actually open. The production half above proves a real host writes
    /// the `"i"` event; this proves a reader that stopped filtering on `"o"`
    /// would surface the typed text through `read_shell`. The fixture builds
    /// the cast directly (`CastWriter::create` + `write_output`/`write_input`)
    /// in the module's own `fixture` shape — `.meta.json` included, the same
    /// index the host screenshots through `mark` — so the reader really opens
    /// the file, and a widened filter in `src/shell/log.rs` turns this red.
    #[test]
    fn send_input_text_never_reaches_read_shell_in_a_reachable_cast() {
        let (tmp, reg) = unreachable_cast_fixture();
        with_home(&tmp, || {
            let out = read_shell_in(&reg, "s12", None, None, None);
            assert!(
                out.contains("READY to go"),
                "the \"o\" output the fixture wrote is what the read tool returns: {out}"
            );
            assert!(
                !out.contains("SENTINEL-TYPED-BY-SEND-INPUT"),
                "typed input must never surface through the read tools: {out}"
            );
        });
    }

    /// A one-shell fixture whose cast holds an `"o"` event and an `"i"` event
    /// and a `.meta.json` that names command 0 over a range covering both —
    /// a future host that begins recording input inside a command's slice is
    /// the shape this test defends against. Registry record `s12` under
    /// `Owner::Chat("caller")`, `started_unix` 1_788_470_012 (index-derived,
    /// so no other test's cast collides). Runs with `HOME` pointed at its own
    /// tmp dir, the way `fixture` does, because `cast_path` resolves through
    /// `HOME`.
    fn unreachable_cast_fixture() -> (TmpHome, ShellRegistry) {
        let tmp = TmpHome::new();
        let reg = ShellRegistry::default();
        with_home(&tmp, || {
            let (id, started_unix) = ("s12", 1_788_470_012u64);
            let cast = cast_path(id, started_unix);
            std::fs::create_dir_all(cast.parent().unwrap()).unwrap();
            let mut w = crate::shell::CastWriter::create(&cast, 80, 24, started_unix).unwrap();
            w.mark(Duration::from_secs_f64(1.0), "probe --yes").unwrap();
            let first_byte = w.byte_len();
            w.write_output(Duration::from_secs_f64(1.5), b"READY to go\n")
                .unwrap();
            w.write_input(
                Duration::from_secs_f64(2.0),
                b"SENTINEL-TYPED-BY-SEND-INPUT\n",
            )
            .unwrap();
            let end_byte = w.byte_len();
            w.mark(Duration::from_secs_f64(2.5), "end 0").unwrap();
            drop(w);
            let meta = crate::shell::MetaIndex {
                owner: None,
                host: None,
                shell_id: id.into(),
                cast: cast.file_name().unwrap().to_string_lossy().into_owned(),
                started_unix,
                commands: vec![crate::shell::CommandRecord {
                    index: 0,
                    command: "probe --yes".into(),
                    started: 1.0,
                    ended: 2.5,
                    exit_code: 0,
                    first_byte,
                    end_byte,
                }],
            };
            meta.save(&crate::shell::meta_path_for(&cast)).unwrap();
            insert(&reg, 12, Owner::Chat("caller".into()), started_unix);
        });
        (tmp, reg)
    }

    /// The most recent temp dir holding a `.cast` that records `sentinel`.
    fn find_stray_cast(sentinel: &str) -> String {
        let mut candidates: Vec<(std::time::SystemTime, String)> = Vec::new();
        let _ = std::fs::read_dir("/tmp").map(|rd| {
            for entry in rd.flatten() {
                let p = entry.path().join("s3.cast");
                if let Ok(s) = std::fs::read_to_string(&p)
                    && s.contains(sentinel)
                {
                    let t = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    candidates.push((t, p.to_string_lossy().into_owned()));
                }
            }
        });
        // The assertion above guarantees at least one; pick the newest.
        candidates
            .into_iter()
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, p)| p)
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // list_shell_logs / read_shell_log (M22 phase-06b)
    // -----------------------------------------------------------------------

    /// A log directory under the temp `HOME` (where `shell_logs_dir()` points)
    /// plus the dir path, for driving the `_in` seams directly. The whole
    /// body must call the seams inside `with_home(&tmp, …)` so
    /// `shell_logs_dir()` and the writing `meta_path_for` agree on the
    /// directory.
    fn logs_dir_fixture() -> (TmpHome, std::path::PathBuf) {
        let tmp = TmpHome::new();
        let dir = with_home(&tmp, || {
            let dir = crate::config::shell_logs_dir();
            std::fs::create_dir_all(&dir).unwrap();
            dir
        });
        (tmp, dir)
    }

    /// Write one recording named `log_id` into `dir`: a `.cast` holding
    /// `per_command` (one line of output per command) and a `.meta.json`
    /// sidecar indexing those commands with the given exit codes. The
    /// commands' text is `per_command` joined with " && " — so a row's hint
    /// is that row's own commanded output, not a label shared across rows —
    /// and the artifact is built with the real producer paths so the test
    /// cannot disagree with the sweep or the id grammar.
    fn write_recording(
        dir: &std::path::Path,
        log_id: &str,
        per_command: &[&str],
        exit_codes: &[i32],
    ) {
        let cast = dir.join(format!("{log_id}.cast"));
        let started_unix: u64 = log_id
            .rsplit_once('-')
            .and_then(|(_, ts)| ts.parse().ok())
            .unwrap_or(0);
        let mut w = crate::shell::CastWriter::create(&cast, 80, 24, started_unix).unwrap();
        let mut records = Vec::new();
        let shared_command = per_command.join(" && ");
        for (i, out) in per_command.iter().enumerate() {
            let t = 1.0 + i as f64;
            let command = shared_command.clone();
            w.mark(Duration::from_secs_f64(t), &command).unwrap();
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
                command,
                started: t,
                ended: t + 0.5,
                exit_code: exit_codes.get(i).copied().unwrap_or(0),
                first_byte,
                end_byte,
            });
        }
        drop(w);
        let meta = crate::shell::MetaIndex {
            owner: None,
            host: None,
            shell_id: log_id
                .rsplit_once('-')
                .map(|(id, _)| id)
                .unwrap_or("")
                .into(),
            cast: cast.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix,
            commands: records,
        };
        meta.save(&meta_path_for(&cast)).unwrap();
    }

    /// The row's hint is its own first command's text. The slices guard on
    /// `min`/`max` because a row id can also appear in the "past the limit"
    /// suffix; the assertion needs the listed rows only.
    fn row_hint(out: &str, id: &str, next: Option<&str>) -> String {
        let start = out.find(id).expect("row listed");
        let end = match next {
            Some(n) => out[start + id.len()..]
                .find(n)
                .map(|i| start + id.len() + i)
                .unwrap_or(out.len()),
            None => out.len(),
        };
        out[start..end.min(out.len())].to_string()
    }

    #[test]
    fn list_shell_logs_is_newest_first() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(&dir, "s3-1788470000", &["older output"], &[0]);
            write_recording(&dir, "s9-1788480000", &["newer output"], &[0]);
            write_recording(&dir, "s1-1788460000", &["oldest output"], &[0]);
            let out = list_shell_logs_in(&dir, None, None, 1_788_480_100);
            let s9 = out.find("s9-1788480000").expect("s9 listed");
            let s3 = out.find("s3-1788470000").expect("s3 listed");
            let s1 = out.find("s1-1788460000").expect("s1 listed");
            assert!(
                s9 < s3 && s3 < s1,
                "rows are newest first by started_unix: {out}"
            );
            let s1_hint = row_hint(&out, "s1-1788460000", None);
            assert!(
                s1_hint.contains("oldest output"),
                "the oldest row's hint is its own first command: {out}"
            );
            let s9_hint = row_hint(&out, "s9-1788480000", Some("s3-1788470000"));
            assert!(
                s9_hint.contains("newer output"),
                "each row's hint is that row's own command: {out}"
            );
        });
    }

    #[test]
    fn list_shell_logs_includes_a_cast_with_no_index() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(&dir, "s4-1788470400", &["indexed output"], &[0]);
            let cast = dir.join("s5-1788470500.cast");
            std::fs::write(&cast, b"{\"version\": 2, \"width\": 80}").unwrap();
            let out = list_shell_logs_in(&dir, None, None, 1_788_480_100);
            let s5 = out
                .find("s5-1788470500")
                .expect("the unindexed cast is listed");
            assert!(
                out[s5..].contains("unknown"),
                "an unindexed recording shows an unknown command count, not 0: {out}"
            );
            assert!(
                !out[s5..].contains("0 command"),
                "unknown must never be rendered as 0 commands: {out}"
            );
            // The indexed sibling keeps its real count.
            let s4 = out
                .find("s4-1788470400")
                .expect("the indexed cast is listed");
            assert!(
                out[s4..].contains("1 command"),
                "a readable index shows its real count: {out}"
            );
        });
    }

    #[test]
    fn list_shell_logs_since_and_limit_filter() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(&dir, "s2-1788460000", &["old output"], &[0]);
            write_recording(&dir, "s4-1788470000", &["boundary output"], &[0]);
            write_recording(&dir, "s6-1788480000", &["new output"], &[0]);
            // `since` is inclusive: the boundary recording itself survives.
            let out = list_shell_logs_in(&dir, Some(1_788_470_000), None, 1_788_480_100);
            assert!(
                !out.contains("s2-1788460000"),
                "a recording older than `since` is excluded: {out}"
            );
            assert!(
                out.contains("s4-1788470000") && out.contains("s6-1788480000"),
                "recordings at or after `since` are included: {out}"
            );
            // `limit` caps the rows and the output says it clamped.
            let out = list_shell_logs_in(&dir, None, Some(900), 1_788_480_100);
            assert!(
                out.contains("clamped"),
                "a limit above the cap is reported as clamped: {out}"
            );
            let out = list_shell_logs_in(&dir, None, Some(2), 1_788_480_100);
            assert!(
                out.contains("1 more recording"),
                "the truncated suffix is named, not silently dropped: {out}"
            );
            // The listed rows are the newest pair; the id string asserts on
            // the rows only, so a listed id must not appear twice (once as a
            // row, once in the suffix). Split the output at the suffix and
            // assert on the rows.
            let rows_part = out.split("[1 more recording").next().unwrap_or(&out);
            assert!(
                !rows_part.contains("s2-1788460000"),
                "not every row survived the limit: {rows_part}"
            );
            assert!(
                rows_part.contains("s6-1788480000"),
                "the newest row survives a limit of 2: {rows_part}"
            );
            let without = list_shell_logs_in(&dir, Some(1_788_481_000), None, 1_788_480_100);
            assert!(
                without.contains("No shell recordings at or after"),
                "a `since` past every recording says so plainly: {without}"
            );
        });
    }

    #[test]
    fn read_shell_log_refuses_a_bare_shell_id() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(&dir, "s3-1788470000", &["the only s3 recording"], &[0]);
            let out = read_log_in(&dir, "s3", None, None, None);
            assert!(
                out.starts_with("Error:"),
                "a bare shell id is a refusal, not a read: {out}"
            );
            assert!(
                out.contains("s3"),
                "the refusal names the offending value: {out}"
            );
            assert!(
                out.contains("list_shell_logs"),
                "the refusal points at the listing tool: {out}"
            );
            assert!(
                !out.contains("the only s3 recording"),
                "no recording is read even though exactly one s3-* exists: {out}"
            );
        });
    }

    #[test]
    fn read_shell_log_reads_one_command_with_its_exit_code() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(
                &dir,
                "s7-1788477000",
                &["first output", "second output"],
                &[3, 0],
            );
            let out = read_log_in(&dir, "s7-1788477000", Some(1), None, None);
            assert!(
                out.contains("second output") && !out.contains("first output"),
                "command_index 1 returns the second command's output only: {out}"
            );
            assert!(
                out.contains("command 1"),
                "the header names the selected index: {out}"
            );
            assert!(
                out.contains("exit code 0"),
                "command 1's real exit code is cited, not a default: {out}"
            );
            let failing = read_log_in(&dir, "s7-1788477000", Some(0), None, None);
            assert!(
                failing.contains("exit code 3") && failing.contains("non-zero"),
                "command 0's real non-zero exit code is in the header, not a default: {failing}"
            );
            assert!(
                failing.contains("first output") && !failing.contains("second output"),
                "command_index 0 returns the first command's output only: {failing}"
            );
        });
    }

    #[test]
    fn read_shell_log_unknown_log_is_an_error_naming_what_exists() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(&dir, "s2-1788465000", &["real output"], &[0]);
            let out = read_log_in(&dir, "s9-1788499999", None, None, None);
            assert!(
                out.starts_with("Error:"),
                "an absent log id is an error, returned as a String: {out}"
            );
            assert!(
                out.contains("s9-1788499999"),
                "the error names what could not be found: {out}"
            );
            assert!(
                out.contains("s2-1788465000"),
                "the error lists the available log ids: {out}"
            );
        });
    }

    #[test]
    fn read_shell_log_invalid_filter_is_an_error() {
        let (tmp, dir) = logs_dir_fixture();
        with_home(&tmp, || {
            write_recording(&dir, "s6-1788476000", &["real output"], &[0]);
            let out = read_log_in(&dir, "s6-1788476000", None, None, Some("[unclosed"));
            assert!(
                out.starts_with("Error:"),
                "an invalid filter regex is an error, not a literal match: {out}"
            );
            assert!(
                out.contains("[unclosed"),
                "the error names the offending pattern: {out}"
            );
            assert!(
                out.contains("regex"),
                "the error says the filter is applied as a regex: {out}"
            );
            assert!(
                !out.contains("real output"),
                "the pattern is rejected before any file is read: {out}"
            );
        });
    }
}
