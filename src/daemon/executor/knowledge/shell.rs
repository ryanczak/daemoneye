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
        foreground: Option<Foreground>,
        dedicated: bool,
        exited_unix: Option<u64>,
    ) -> ShellRow {
        ShellRow {
            id: id.to_string(),
            owner_key: owner_key.to_string(),
            host: crate::host::HostId::Local,
            started_unix,
            foreground,
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
            foreground: None,
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
            row("s0", "chat:a", NOW, None, false, None),
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
        let rows = vec![row("s0", "chat:a", NOW, None, false, None)];
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
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
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
            row("s10", "chat:a", NOW - 100, None, false, None),
            row("s2", "chat:a", NOW - 100, None, false, None),
        ];
        let got = default_shell_for(&rows, "chat:a");
        assert_eq!(
            got,
            Some("s2".to_string()),
            "s2 must beat s10: a numeric index, not a string sort"
        );
    }

    #[test]
    fn default_shell_for_skips_a_dedicated_shell() {
        let rows = vec![
            row("s0", "chat:a", NOW - 100, None, true, None),
            row("s1", "chat:a", NOW - 100, None, false, None),
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
            row("s0", "chat:a", NOW - 100, None, false, Some(NOW)),
            row("s1", "chat:a", NOW - 100, None, false, None),
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
            row("s0", "chat:other", NOW - 100, None, false, None),
            row("s1", "chat:a", NOW - 100, None, false, None),
        ];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            Some("s1".to_string()),
            "another owner's shell is invisible to the selector"
        );
    }

    #[test]
    fn default_shell_for_returns_none_when_every_shell_is_ineligible() {
        let rows = vec![row(
            "s0",
            "chat:a",
            NOW - 100,
            Some(Foreground::Program {
                cmd: "claude".to_string(),
                tui: true,
            }),
            false,
            None,
        )];
        assert_eq!(
            default_shell_for(&rows, "chat:a"),
            None,
            "a shell last seen busy has no default"
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
            row("s2", "chat:a", NOW - 100, None, false, None),
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
            row(
                "s2",
                "chat:a",
                NOW - 12,
                Some(Foreground::Program {
                    cmd: "claude".to_string(),
                    tui: true,
                }),
                false,
                None,
            ),
            row("s0", "chat:a", NOW - 240, None, false, None),
            row("s4", "chat:b", NOW - 240, None, true, Some(NOW - 5)),
        ];
        let out = format_shell_context(&rows, "chat:a", NOW);
        assert!(
            out.starts_with("[SHELLS] 3 — s0 age:4m, s2 age:12s [running claude (TUI)], s4 age:"),
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
            row("s0", "chat:a", NOW - 240, None, false, None),
            row(
                "s2",
                "chat:a",
                NOW - 12,
                Some(Foreground::Program {
                    cmd: "claude".to_string(),
                    tui: true,
                }),
                false,
                None,
            ),
            row("s4", "chat:b", NOW - 240, None, true, Some(NOW - 5)),
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
            segments[1].contains("[running claude (TUI)]"),
            "s2's segment must carry the observed foreground: segment {:?} in {out}",
            segments[1]
        );
        assert!(
            segments[2].contains("[exited]"),
            "s4's segment must carry [exited]: segment {:?} in {out}",
            segments[2]
        );
        assert!(
            !segments[2].contains("[paused"),
            "an exited shell's segment must not restate its foreground: {:?}",
            segments[2]
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
        let rows = vec![row(
            "s0",
            "chat:a",
            NOW - 100,
            Some(Foreground::Program {
                cmd: "claude".to_string(),
                tui: true,
            }),
            false,
            None,
        )];
        let out = format_shell_context(&rows, "chat:a", NOW);
        assert!(
            out.contains("[SHELLS] 1"),
            "the single busy row is still counted: {out}"
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
            row("s3", "chat:a", NOW - 100, None, false, None),
            row("s4", "ghost:a", 0, None, true, Some(NOW - 10)),
        ];
        let out = format_shell_rows(&rows, Some("all"), NOW);
        assert!(out.contains("[dedicated]"), "flag carried: {out}");
        assert!(
            out.contains("[exited]"),
            "an exited shell is kept visible: {out}"
        );
        assert!(!out.contains("[interactive]"), "no stale flag: {out}");
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
    fn format_shell_rows_shows_the_observed_foreground() {
        let rows = vec![
            row(
                "s0",
                "chat:a",
                NOW - 10,
                Some(Foreground::Program {
                    cmd: "claude".to_string(),
                    tui: true,
                }),
                false,
                None,
            ),
            row(
                "s1",
                "chat:a",
                NOW - 10,
                Some(Foreground::Paused {
                    cmd: "vim".to_string(),
                }),
                false,
                None,
            ),
            row(
                "s2",
                "chat:a",
                NOW - 10,
                Some(Foreground::Prompt),
                false,
                None,
            ),
            row(
                "s3",
                "chat:a",
                NOW - 10,
                Some(Foreground::Unknown),
                false,
                None,
            ),
            row("s4", "chat:a", NOW - 10, None, false, None),
        ];
        let out = format_shell_rows(&rows, None, NOW);
        assert!(
            out.contains("s0  owner:chat:a  age:10s [running claude (TUI)]"),
            "a TUI program is shown with its TUI marker: {out}"
        );
        assert!(
            out.contains("s1  owner:chat:a  age:10s [paused vim]"),
            "a paused shell shows its stopped job: {out}"
        );
        assert!(
            out.contains("s2  owner:chat:a  age:10s\n"),
            "a shell at its prompt renders no bracketed foreground: {out}"
        );
        assert!(
            out.contains("s3  owner:chat:a  age:10s\n"),
            "an unknown classification renders nothing: {out}"
        );
        assert!(
            out.contains("s4  owner:chat:a  age:10s\n"),
            "an unobserved shell renders nothing: {out}"
        );
    }

    #[test]
    fn format_shell_context_shows_the_observed_foreground() {
        let rows = vec![
            row(
                "s0",
                "chat:a",
                NOW - 10,
                Some(Foreground::Program {
                    cmd: "claude".to_string(),
                    tui: true,
                }),
                false,
                None,
            ),
            row(
                "s1",
                "chat:a",
                NOW - 10,
                Some(Foreground::Paused {
                    cmd: "vim".to_string(),
                }),
                false,
                None,
            ),
            row(
                "s2",
                "chat:a",
                NOW - 10,
                Some(Foreground::Prompt),
                false,
                None,
            ),
            row(
                "s3",
                "chat:a",
                NOW - 10,
                Some(Foreground::Unknown),
                false,
                None,
            ),
            row("s4", "chat:a", NOW - 10, None, false, None),
        ];
        let out = format_shell_context(&rows, "chat:a", NOW);
        let listing = out
            .lines()
            .find(|line| line.starts_with("[SHELLS] "))
            .expect("the count line is present");
        assert!(
            listing.contains("s0 age:10s [running claude (TUI)]"),
            "the TUI program is shown in the block: {listing}"
        );
        assert!(
            listing.contains("s1 age:10s [paused vim]"),
            "the paused shell is shown in the block: {listing}"
        );
        for id in ["s2", "s3", "s4"] {
            let segment = listing
                .split(", ")
                .find(|s| s.starts_with(&format!("{id} ")))
                .unwrap_or_else(|| panic!("{id} listed: {listing}"));
            assert!(
                !segment.contains('['),
                "{id} is not busy, so no bracketed foreground: {segment:?} in {listing}"
            );
        }
        assert!(
            out.contains("[DEFAULT SHELL] s2 —"),
            "the default skips the busy shells and names the first prompt shell: {out}"
        );
    }

    #[test]
    fn default_shell_skips_a_shell_last_seen_busy() {
        let busy_first = vec![
            row(
                "s0",
                "chat:a",
                NOW - 10,
                Some(Foreground::Program {
                    cmd: "claude".to_string(),
                    tui: true,
                }),
                false,
                None,
            ),
            row(
                "s1",
                "chat:a",
                NOW - 10,
                Some(Foreground::Prompt),
                false,
                None,
            ),
        ];
        assert_eq!(
            default_shell_for(&busy_first, "chat:a"),
            Some("s1".to_string()),
            "a shell last seen running a program is skipped, s1 is the default"
        );
        let paused_first = vec![
            row(
                "s0",
                "chat:a",
                NOW - 10,
                Some(Foreground::Paused {
                    cmd: "vim".to_string(),
                }),
                false,
                None,
            ),
            row(
                "s1",
                "chat:a",
                NOW - 10,
                Some(Foreground::Prompt),
                false,
                None,
            ),
        ];
        assert_eq!(
            default_shell_for(&paused_first, "chat:a"),
            Some("s1".to_string()),
            "a shell last seen paused is skipped too"
        );
        let unobserved_first = vec![
            row("s0", "chat:a", NOW - 10, None, false, None),
            row(
                "s1",
                "chat:a",
                NOW - 10,
                Some(Foreground::Prompt),
                false,
                None,
            ),
        ];
        assert_eq!(
            default_shell_for(&unobserved_first, "chat:a"),
            Some("s0".to_string()),
            "an unobserved shell is not busy, so the lowest stays the default"
        );
    }

    #[test]
    fn formatter_reports_a_single_shell_without_plural_splitting() {
        let rows = vec![row("s1", "chat:a", NOW - 30, None, false, None)];
        let out = format_shell_rows(&rows, None, NOW);
        assert!(out.contains("1 shell (scope \"mine\")"), "get: {out}");
        assert!(out.contains("age:30s"), "age: {out}");
    }

    /// Local rendering is pinned **exactly**: the phase's rule is that a
    /// `HostId::Local` shell's lines are byte-identical to the pre-host
    /// output, so a user with no hosts configured sees nothing new.
    #[test]
    fn local_shells_render_without_a_host() {
        let rows = vec![row("s1", "chat:a", NOW - 30, None, false, None)];
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
            foreground: None,
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
        let out = open_shell(&Owner::Chat("hosted".into()), Some("nope"), None, None).await;
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
        let out = open_shell(&Owner::Chat("local".into()), None, None, None).await;
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

    /// The no-host result sentence names the size the shell opened at. With
    /// no `rows`/`cols` in the request and no `[shells]` table in
    /// `lifecycle_home`'s config, that is the 03b default 40×120; the byte
    /// contract now includes it.
    #[tokio::test]
    async fn open_shell_local_result_names_the_size() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let _swap = open_shell_test_seam::swap_in(&reg);
        let out = open_shell(&Owner::Chat("sentinel".into()), None, None, None).await;
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
                 close_shell ends it. Its terminal is 40 rows × 120 columns. Note: \
                 a command still runs in this session's lowest-numbered open shell \
                 by default, so this shell sees work only when it is that one — \
                 pass shell: \"{id}\" to run_terminal_command to target it \
                 directly, or shell: \"new\" for one command's exclusive shell."
            ),
            "the no-host sentence names the size it opened at (40×120 here: no \
             request, no [shells] table)"
        );
    }

    /// A sized request resolves through `open_shell` the same way: the
    /// sentence names the size the shell was opened at, `rows` first, with
    /// the ` × ` between.
    #[tokio::test]
    async fn open_shell_names_the_requested_size() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let _swap = open_shell_test_seam::swap_in(&reg);
        let out = open_shell(&Owner::Chat("sized".into()), None, Some(50), Some(160)).await;
        assert!(
            out.contains("Opened shell"),
            "the precondition is a ready shell: {out}"
        );
        assert!(
            out.contains("Its terminal is 50 rows × 160 columns."),
            "the sentence names the requested size: {out}"
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
        let out = open_shell(&Owner::Chat("weird".into()), Some("bad name!"), None, None).await;
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
        let out = open_shell(&Owner::Chat("no-swap".into()), None, None, None).await;
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
    fn input_target_note_names_each_foreground() {
        let prompt = input_target_note(&crate::shell::Foreground::Prompt);
        assert!(
            prompt.contains("prompt") && prompt.contains("shell command"),
            "Prompt must say the text will be run as a shell command: {prompt}"
        );
        let stdin_prog = input_target_note(&crate::shell::Foreground::Program {
            cmd: "grep".to_string(),
            tui: false,
        });
        assert!(
            stdin_prog.contains("grep")
                && stdin_prog.contains("stdin")
                && stdin_prog.contains("NOT"),
            "a non-TUI program must name the cmd and stdin: {stdin_prog}"
        );
        let tui = input_target_note(&crate::shell::Foreground::Program {
            cmd: "claude".to_string(),
            tui: true,
        });
        assert!(
            tui.contains("claude") && tui.contains("TUI"),
            "a TUI must name the cmd and the TUI: {tui}"
        );
        assert_ne!(
            prompt, stdin_prog,
            "a note that says the same thing in every state tells the approving \
             user nothing"
        );
        let paused = input_target_note(&crate::shell::Foreground::Paused {
            cmd: "sleep".to_string(),
        });
        assert!(
            paused.contains("sleep") && paused.contains("stopped"),
            "Paused names the stopped job: {paused}"
        );
        for fg in [
            crate::shell::Foreground::Exited,
            crate::shell::Foreground::Unknown,
        ] {
            let note = input_target_note(&fg);
            assert!(
                note.contains("gone"),
                "{fg} says the shell may be gone: {note}"
            );
        }
    }

    #[test]
    fn screen_tail_keeps_the_last_rows_masked() {
        let screen = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n\n\n\n";
        let out = screen_tail(screen, 5);
        assert_eq!(
            out, "four\nfive\nsix\nseven\neight",
            "8 rows + 3 blank ⇒ exactly rows 4–8, blanks dropped: {out:?}"
        );
        let secret = "AKIAIOSFODNN7EXAMPLE is on screen";
        let masked = screen_tail(&format!("row\n{secret}\nlast"), 5);
        assert!(
            !masked.contains("AKIAIOSFODNN7EXAMPLE") && masked.contains("<AWS_KEY>"),
            "the tail is masked before it reaches the panel: {masked:?}"
        );
    }

    #[test]
    fn input_panel_shows_the_foreground_and_a_tui_screen_tail() {
        let input = composed("y", true);
        let screen = "top row\npick a mode\n> claude\n  1. opt\nAKIAIOSFODNN7EXAMPLE";
        let panel = input_panel(
            "s4",
            "y",
            &input,
            &crate::shell::Foreground::Program {
                cmd: "claude".to_string(),
                tui: true,
            },
            Some(screen),
        );
        assert!(
            panel.contains("foreground: running claude (TUI)"),
            "the panel names what holds the shell: {panel}"
        );
        assert!(
            panel.contains("screen (last 5 rows):") && panel.contains("> claude"),
            "a TUI panel shows the screen tail: {panel}"
        );
        assert!(
            panel.contains("<AWS_KEY>") && !panel.contains("AKIAIOSFODNN7EXAMPLE"),
            "the tail rows are masked: {panel}"
        );

        let prompt_panel = input_panel(
            "s4",
            "y",
            &input,
            &crate::shell::Foreground::Prompt,
            Some(screen),
        );
        assert!(
            prompt_panel.contains("foreground: shell prompt"),
            "the prompt panel names the prompt: {prompt_panel}"
        );
        assert!(
            !prompt_panel.contains("screen ("),
            "a prompt panel never shows a screen tail: {prompt_panel}"
        );
    }

    /// The composed input the older `send_input_with` tests send: the text,
    /// no keys, the Enter flag, the default modes — the shape phase-02e's
    /// new tests build explicitly.
    fn composed(text: &str, enter: bool) -> ComposedInput {
        compose_input(text, None, &[], enter, crate::shell::ShellModes::default())
            .expect("plain text, no keys, a valid composition")
    }

    #[test]
    fn compose_input_orders_text_keys_then_enter() {
        let keys = vec!["Up".to_string()];
        let input = compose_input("ls", None, &keys, true, crate::shell::ShellModes::default())
            .expect("a valid composition");
        assert_eq!(
            input.bytes,
            b"ls\x1b[A\r".to_vec(),
            "the text, then the encoded keys, then the Enter \\r: {:?}",
            input.bytes
        );
        assert_eq!(input.keys, keys, "the key names are carried verbatim");
        assert!(!input.bracketed, "plain text is never bracketed");
        assert!(input.enter, "the Enter flag is carried");

        let ctrl_c = vec!["C-c".to_string()];
        let input = compose_input(
            "",
            None,
            &ctrl_c,
            false,
            crate::shell::ShellModes::default(),
        )
        .expect("a keys-only send");
        assert_eq!(
            input.bytes,
            b"\x03".to_vec(),
            "an empty text with one control key sends just that key: {:?}",
            input.bytes
        );
        assert!(input.csi_u.is_empty(), "C-c has a legacy byte");
    }

    #[test]
    fn compose_input_brackets_a_multiline_paste_when_the_shell_asked() {
        let on = crate::shell::ShellModes {
            bracketed_paste: true,
            ..crate::shell::ShellModes::default()
        };
        let input = compose_input("a\nb", None, &[], false, on).expect("the mode is on");
        assert_eq!(
            input.bytes,
            b"\x1b[200~a\nb\x1b[201~".to_vec(),
            "a newline with the mode on is wrapped in the paste brackets: {:?}",
            input.bytes
        );
        assert!(input.bracketed, "the wrap is reported to the panel/result");

        let off = crate::shell::ShellModes::default();
        let input = compose_input("a\nb", None, &[], false, off).expect("the mode is off");
        assert_eq!(
            input.bytes,
            b"a\nb".to_vec(),
            "with the mode off the raw bytes go out, never brackets: {:?}",
            input.bytes
        );
        assert!(!input.bracketed, "and it is reported as unbracketed");
    }

    #[test]
    fn compose_input_refuses_bad_input_before_anything_is_sent() {
        let bad_key = vec!["Bogus".to_string()];
        let err = compose_input(
            "x",
            None,
            &bad_key,
            false,
            crate::shell::ShellModes::default(),
        )
        .expect_err("an unknown key is refused");
        assert!(
            err.to_string().contains("Bogus"),
            "the bad name is echoed: {err}"
        );

        let err = compose_input(
            "a\nb",
            Some(true),
            &[],
            false,
            crate::shell::ShellModes::default(),
        )
        .expect_err("paste: true with the mode off is refused");
        assert!(
            err.to_string().contains("bracketed paste"),
            "the error names the mode that is off: {err}"
        );

        let err = compose_input("", None, &[], false, crate::shell::ShellModes::default())
            .expect_err("an empty call is refused");
        assert!(
            err.to_string().contains("nothing to send"),
            "the empty call names its own error: {err}"
        );
    }

    #[tokio::test]
    async fn send_input_result_names_csi_u_keys_and_the_paste() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("caller".into()), NOW);
        let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
        let keys = vec!["S-Enter".to_string()];
        let input = compose_input("", None, &keys, false, crate::shell::ShellModes::default())
            .expect("S-Enter is in the vocabulary");
        let out = send_input_with(
            &reg,
            "s3",
            &input,
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
            out.contains("S-Enter") && out.contains("CSI u"),
            "the result names the CSI u key and the encoding: {out}"
        );
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [input.bytes.as_slice()],
            "the bytes the injected send saw equal input.bytes"
        );

        captured.lock().unwrap().clear();
        let on = crate::shell::ShellModes {
            bracketed_paste: true,
            ..crate::shell::ShellModes::default()
        };
        let input = compose_input("a\nb", None, &[], false, on).expect("the mode is on");
        let out = send_input_with(
            &reg,
            "s3",
            &input,
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
            out.contains("bracketed paste"),
            "the result says the text went out as a paste: {out}"
        );
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [input.bytes.as_slice()],
            "the bracketed bytes are what the injected send saw"
        );
    }

    #[tokio::test]
    async fn send_input_enter_sends_a_carriage_return() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert(&reg, 3, Owner::Chat("caller".into()), NOW);
        let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
        let input = compose_input(
            "hello",
            None,
            &[],
            true,
            crate::shell::ShellModes::default(),
        )
        .expect("plain text, no keys");
        let out = send_input_with(
            &reg,
            "s3",
            &input,
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
        assert!(out.contains("Enter sent"), "the Enter is reported: {out}");
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [b"hello\r".to_vec()],
            "enter: true appends exactly one \\r — the byte a real Enter sends"
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
            &composed("ls", true),
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
            &composed("ls", true),
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
        let input = compose_input(
            "hello",
            None,
            &[],
            false,
            crate::shell::ShellModes::default(),
        )
        .expect("plain text, no keys");
        let out = send_input_with(
            &reg,
            "s3",
            &input,
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
            out.contains("no Enter"),
            "the absence of the Enter is reported: {out}"
        );
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            [b"hello".to_vec()],
            "enter: false sends the text alone, no terminator"
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
            &composed("whoami", true),
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
            &composed("echo hi", false),
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
            &composed("SENTINEL-NOT-IN-OUTPUT", false),
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

    // -----------------------------------------------------------------------
    // read_shell_screen (M28 phase 01c)
    // -----------------------------------------------------------------------

    // -- screen waits (M28 phase 03a) --------------------------------------

    #[test]
    fn wait_bounds_clamp_the_timeout_and_the_settle() {
        let (t, s) = wait_bounds(None, None);
        assert_eq!(t, Duration::from_secs(30), "the default timeout is 30 s");
        assert_eq!(s, None, "no settle_ms means no settle wait");

        let (t, _) = wait_bounds(Some(0), None);
        assert_eq!(t, Duration::from_secs(1), "0 s clamps up to 1 s");
        let (t, _) = wait_bounds(Some(9999), None);
        assert_eq!(t, Duration::from_secs(300), "9999 s clamps down to 300 s");

        let (t, s) = wait_bounds(Some(10), Some(1));
        assert_eq!(t, Duration::from_secs(10));
        assert_eq!(s, Some(Duration::from_millis(50)), "1 ms clamps to 50 ms");
        let (_, s) = wait_bounds(None, Some(99999));
        assert_eq!(
            s,
            Some(Duration::from_millis(5000)),
            "99999 ms clamps to 5 s"
        );
    }

    #[tokio::test]
    async fn wait_screen_change_returns_on_the_first_difference() {
        let seq: std::cell::RefCell<Vec<&'static str>> = std::cell::RefCell::new(vec![
            "$ claude", "$ claude", "UI", "UI", "UI", "UI", "UI", "UI", "UI", "UI", "UI", "UI",
        ]);
        let read = || {
            let mut q = seq.borrow_mut();
            let v = if q.is_empty() { "UI" } else { q.remove(0) };
            std::future::ready(Ok::<String, String>(v.to_string()))
        };
        assert!(
            wait_screen_change(
                read,
                "$ claude",
                Duration::from_millis(200),
                Duration::from_millis(10),
            )
            .await,
            "the third read shows the drawn UI, so the wait returns true"
        );

        let read = || std::future::ready(Ok::<String, String>("$ claude".to_string()));
        assert!(
            !wait_screen_change(
                read,
                "$ claude",
                Duration::from_millis(200),
                Duration::from_millis(10),
            )
            .await,
            "a reader that never changes is the launch's not-drawn-yet case: false"
        );
    }

    #[tokio::test]
    async fn wait_screen_returns_when_the_pattern_appears() {
        let seq: std::cell::RefCell<Vec<&'static str>> = std::cell::RefCell::new(vec![
            "loading", "loading", "done $", "done $", "done $", "done $",
        ]);
        let read = || {
            let mut q = seq.borrow_mut();
            let v = q.remove(0);
            std::future::ready(Ok::<String, String>(v.to_string()))
        };
        let re = regex::Regex::new("done").unwrap();
        let out = wait_screen(
            read,
            Some(&re),
            None,
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(out, WaitOutcome::Matched { .. }),
            "the third read matches, so the wait ends Matched: {out:?}"
        );
    }

    #[tokio::test]
    async fn wait_screen_with_both_waits_for_the_pattern_then_settles() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let counter = std::rc::Rc::clone(&calls);
        let read = move || {
            let c = counter.get();
            counter.set(c + 1);
            let v = match c {
                0 => "loading",
                1 => "done 1",
                2 => "done 2",
                _ => "done 3",
            };
            std::future::ready(Ok::<String, String>(v.to_string()))
        };
        let re = regex::Regex::new("done").unwrap();
        let out = wait_screen(
            read,
            Some(&re),
            Some(Duration::from_millis(10)),
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(out, WaitOutcome::Settled { .. }),
            "with both waits the pattern match hands over to the settle phase, \
             which needs two equal reads after `done 1`: {out:?}"
        );
        assert!(
            calls.get() > 2,
            "the reader was called past the first `done` read (called {} times)",
            calls.get()
        );
    }

    #[tokio::test]
    async fn wait_screen_settles_when_two_reads_agree() {
        let seq: std::cell::RefCell<Vec<&'static str>> = std::cell::RefCell::new(vec![
            "a", "b", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c",
            "c", "c", "c",
        ]);
        let read = || {
            let mut q = seq.borrow_mut();
            let v = q.remove(0);
            std::future::ready(Ok::<String, String>(v.to_string()))
        };
        let out = wait_screen(
            read,
            None,
            Some(Duration::from_millis(10)),
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(out, WaitOutcome::Settled { .. }),
            "reads three and four both say c, so the screen settled: {out:?}"
        );
    }

    #[tokio::test]
    async fn wait_screen_that_never_settles_is_a_result_not_an_error() {
        let n = std::cell::Cell::new(0u64);
        let read = || {
            let v = n.get();
            n.set(v + 1);
            std::future::ready(Ok::<String, String>(format!("frame {v}")))
        };
        let out = wait_screen(
            read,
            None,
            Some(Duration::from_millis(10)),
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;
        let WaitOutcome::NotSettled { timeout } = &out else {
            panic!("a counter screen never settles, so the wait is NotSettled: {out:?}")
        };
        assert_eq!(
            *timeout,
            Duration::from_millis(200),
            "the deadline is named"
        );
        let note = wait_note(&out, None).unwrap_or_default();
        assert!(
            note.contains("did not settle") && note.contains("wait_for"),
            "the note points the model at wait_for: {note}"
        );
    }

    #[tokio::test]
    async fn wait_screen_never_matching_times_out() {
        let read = || std::future::ready(Ok::<String, String>("loading".to_string()));
        let re = regex::Regex::new("done").unwrap();
        let out = wait_screen(
            read,
            Some(&re),
            None,
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(out, WaitOutcome::NotMatched { .. }),
            "a screen that never shows the pattern is NotMatched: {out:?}"
        );
        let note = wait_note(&out, Some(&re)).unwrap_or_default();
        assert!(
            note.contains("done"),
            "the note names the pattern it waited for: {note}"
        );

        let read = || std::future::ready(Err::<String, String>("gone".to_string()));
        let out = wait_screen(
            read,
            Some(&re),
            None,
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(&out, WaitOutcome::Failed(msg) if msg == "gone"),
            "the read's error is passed through: {out:?}"
        );
        assert!(
            wait_note(&out, Some(&re)).is_none(),
            "a Failed outcome shows no screen"
        );
    }

    #[tokio::test]
    async fn read_shell_screen_invalid_wait_for_is_an_error() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 5, 1_788_470_005);
        let out = read_shell_screen_in(
            &reg,
            "s5",
            None,
            None,
            None,
            false,
            Some("("),
            None,
            None,
            Duration::from_millis(200),
        )
        .await;
        assert!(
            out.starts_with("Error: invalid wait_for regex \"(\""),
            "an invalid wait_for regex is an error before any socket I/O: {out}"
        );
    }

    // -----------------------------------------------------------------------
    // read_shell_screen (M28 phase 01c) — live-shell fixtures
    // -----------------------------------------------------------------------

    /// Open a real in-process shell host and return its id. The same path
    /// `close_shell_ends_the_callers_shell` takes.
    async fn open_screen_shell(reg: &ShellRegistry) -> String {
        let opened = open_shell_with(
            reg,
            &Owner::Chat("screen-reader".into()),
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
        opened
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            })
            .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
            .expect("the success sentence names the id")
    }

    async fn type_into(id: &str, bytes: &[u8]) {
        crate::shell::send_input(&socket_path(id), bytes, Duration::from_secs(5))
            .await
            .expect("the shell accepts input");
    }

    /// Read the screen until `done` holds or 10 s pass; returns the last read.
    async fn screen_until(
        reg: &ShellRegistry,
        id: &str,
        lines: Option<u64>,
        pattern: Option<&str>,
        done: impl Fn(&str) -> bool,
    ) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = read_shell_screen_in(
                reg,
                id,
                lines,
                None,
                pattern,
                false,
                None,
                None,
                None,
                Duration::from_secs(5),
            )
            .await;
            if done(&out) || Instant::now() >= deadline {
                return out;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The rows after the header line.
    fn body_rows(out: &str) -> Vec<&str> {
        out.lines().skip(1).collect()
    }

    #[tokio::test]
    async fn read_shell_screen_returns_the_screen_with_a_foreground_header() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let id = open_screen_shell(&reg).await;
        type_into(&id, b"echo SCREEN-MARKER-01C\n").await;
        let out = screen_until(&reg, &id, None, None, |o| {
            o.lines().skip(1).any(|l| l.trim() == "SCREEN-MARKER-01C") && o.contains("shell prompt")
        })
        .await;
        let header = out.lines().next().unwrap_or_default();
        assert!(
            header.starts_with(&format!("Shell {id} — shell prompt — 24x80, cursor ")),
            "the header names the shell, its foreground and its size: {out}"
        );
        assert!(
            body_rows(&out)
                .iter()
                .any(|l| l.trim() == "SCREEN-MARKER-01C"),
            "the body is the rendered screen: {out}"
        );
    }

    #[tokio::test]
    async fn read_shell_screen_names_a_running_program() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let id = open_screen_shell(&reg).await;
        type_into(&id, b"sleep 30\n").await;
        let out = screen_until(&reg, &id, None, None, |o| o.contains("running sleep")).await;
        type_into(&id, b"\x03").await;
        assert!(
            out.lines()
                .next()
                .is_some_and(|h| h.contains("— running sleep —")),
            "a running job is named in the header: {out}"
        );
    }

    #[tokio::test]
    async fn read_shell_screen_lines_tails_the_visible_rows() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let id = open_screen_shell(&reg).await;
        type_into(&id, b"printf 'row-a\\nrow-b\\nrow-c\\n'\n").await;
        // Wait for the output (a row that is exactly `row-c`, not the echoed
        let full = screen_until(&reg, &id, None, None, |o| {
            o.lines().any(|l| l.trim() == "row-c") && o.contains("shell prompt")
        })
        .await;
        let rows = body_rows(&full);
        let after_c = rows
            .iter()
            .rposition(|l| l.trim() == "row-c")
            .expect("row-c is on screen");
        let tail_len = rows.len() - after_c;
        let out = read_shell_screen_in(
            &reg,
            &id,
            Some(tail_len as u64),
            None,
            None,
            false,
            None,
            None,
            None,
            Duration::from_secs(5),
        )
        .await;
        let tailed = body_rows(&out);
        assert_eq!(tailed.len(), tail_len,);
        assert_eq!(tailed[0].trim(), "row-c", "the tail starts at row-c: {out}");
        assert!(
            !tailed.iter().any(|l| l.trim() == "row-a"),
            "rows above the tail are dropped: {out}"
        );
    }
    #[tokio::test]
    async fn read_shell_screen_pattern_filters_rows() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let id = open_screen_shell(&reg).await;
        type_into(&id, b"printf 'row-a\\nrow-b\\nrow-c\\n'\n").await;
        let out = screen_until(&reg, &id, None, Some("^row-b$"), |o| {
            body_rows(o).contains(&"row-b")
        })
        .await;
        assert_eq!(
            body_rows(&out),
            vec!["row-b"],
            "only the row matching the pattern is returned, not the echoed command: {out}"
        );
        assert!(
            out.lines().next().is_some_and(|h| h.contains("/^row-b$/")),
            "{out}"
        );
    }

    #[tokio::test]
    async fn read_shell_screen_invalid_pattern_is_an_error() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 5, 1_788_470_005);
        let out = read_shell_screen_in(
            &reg,
            "s5",
            None,
            None,
            Some("("),
            false,
            None,
            None,
            None,
            Duration::from_millis(200),
        )
        .await;
        assert!(
            out.starts_with("Error: invalid pattern regex \"(\""),
            "an invalid regex is an error before any socket I/O: {out}"
        );
    }

    #[tokio::test]
    async fn read_shell_screen_unknown_shell_is_an_error_naming_live_shells() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        insert_watch_record(&reg, 5, 1_788_470_005);
        let out = read_shell_screen_in(
            &reg,
            "s9",
            None,
            None,
            None,
            false,
            None,
            None,
            None,
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(
            out,
            unknown_shell_error(&reg, "s9"),
            "the same sentence read_shell gives"
        );
        assert!(out.contains("s5"), "the live ids are named: {out}");
        assert!(out.contains("s5"), "the live ids are named: {out}");
    }

    #[tokio::test]
    async fn read_shell_screen_records_last_foreground() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let id = open_screen_shell(&reg).await;
        let before = with_shells(&reg, |store| {
            store.get(&id).and_then(|r| r.last_foreground.clone())
        });
        assert_eq!(before, None, "nothing is observed before the first read");
        let out = screen_until(&reg, &id, None, None, |o| o.contains("shell prompt")).await;
        let after = with_shells(&reg, |store| {
            store.get(&id).and_then(|r| r.last_foreground.clone())
        });
        let (fg, at) = after.expect("the read stores what it observed");
        assert_eq!(fg, crate::shell::Foreground::Prompt, "{out}");
        assert!(at > 0, "the observation carries its time");
    }

    #[test]
    fn scrollback_part_strips_the_visible_suffix() {
        assert_eq!(
            scrollback_part("old-1\nold-2\nnow-1\nnow-2", "now-1\nnow-2"),
            "old-1\nold-2"
        );
        assert_eq!(
            scrollback_part("old-1\nchanged", "now-1"),
            "old-1\nchanged",
            "a screen that changed between reads keeps the whole later read"
        );
        assert_eq!(
            scrollback_part("old-1", ""),
            "old-1",
            "an empty screen strips nothing"
        );
    }
}
