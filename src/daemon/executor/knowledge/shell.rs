//! `list_shells` — the model-facing listing of the PTY shells the daemon is
//! running. Reads **only** the shell registry: no socket round-trips, no
//! screen-grid access and no asciicast reads — those are the buffer readers'
//! substrate (03b).

use crate::ai::filter::mask_sensitive;
use crate::config::shell_logs_dir;
use crate::shell::{
    Foreground, MetaIndex, Owner, ShellId, ShellRecord, ShellRegistry, await_ready, cast_path,
    host_is_alive, meta_path_for, parse_cast_name, read_command_output, socket_path,
    spawn_for_on_host, with_shells,
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
    /// The observed classification, not the timestamp: what holds the shell
    /// is what the listing shows, when it was seen is not.
    pub foreground: Option<Foreground>,
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
        foreground: record.last_foreground.as_ref().map(|(fg, _)| fg.clone()),
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
        if let Some(fg) = row
            .foreground
            .as_ref()
            .filter(|_| row.exited_unix.is_none())
            .filter(|fg| matches!(fg, Foreground::Program { .. } | Foreground::Paused { .. }))
        {
            flags.push_str(&format!(" [{fg}]"));
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
/// shells that run on `HostId::Local` and are neither busy nor `dedicated` —
/// a bare `run_terminal_command` lands on the daemon host, so a remote shell
/// is never its default. Busy is the *observed* foreground
/// (`Program`/`Paused`), the cache's classification, never a probe: this
/// runs on every prompt assembly and must do no I/O, so it is one
/// observation stale where `shell_for_owner` is not.
///
/// **Deliberate divergence:** `shell_for_owner` settles liveness with a
/// per-shell flock probe — the registry's own liveness check — while this
/// runs on every prompt assembly and must do no I/O. It reads the
/// `exited_unix` stamp `reap_exited` writes instead: a host that died since
/// the last reap is named one turn too long, a reaped record is never named.
fn default_shell_for(rows: &[ShellRow], owner_key: &str) -> Option<String> {
    rows.iter()
        .filter(|row| {
            !matches!(
                row.foreground,
                Some(Foreground::Program { .. }) | Some(Foreground::Paused { .. })
            ) && !row.dedicated
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
            if let Some(fg) = row
                .foreground
                .as_ref()
                .filter(|_| row.exited_unix.is_none())
                .filter(|fg| matches!(fg, Foreground::Program { .. } | Foreground::Paused { .. }))
            {
                flags.push_str(&format!(" [{fg}]"));
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

/// Compile a pattern a shell tool was given. The 1 MiB compiled-size limit
/// is the one policy every pattern-taking shell tool shares; each caller
/// words its own refusal, naming its parameter, on an `Err`.
fn shell_tool_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
}

#[path = "shell_read.rs"]
mod readers;
pub use readers::*;
#[path = "shell_watch.rs"]
mod watchers;
pub use watchers::*;

#[path = "shell_input.rs"]
mod input;
pub use input::*;
#[path = "shell_screen.rs"]
mod screen;
pub use screen::*;
#[path = "shell_life.rs"]
mod life;
pub use life::*;

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
