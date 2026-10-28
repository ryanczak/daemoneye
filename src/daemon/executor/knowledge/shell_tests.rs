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
        let p = std::env::temp_dir().join(format!("de_kshell_test_{}_{}", std::process::id(), n));
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
        out.contains("[DEFAULT SHELL] s0 — run_terminal_command with no shell argument lands here"),
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
    let err =
        resolve_context_scope(Some("banana")).expect_err("an arbitrary string is not a breadth");
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

#[path = "shell_lifecycle_tests.rs"]
mod lifecycle;
#[path = "shell_observer_tests.rs"]
mod observers;
