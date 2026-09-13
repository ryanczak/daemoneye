//! `list_shells` — the model-facing listing of the PTY shells the daemon is
//! running. Reads **only** the shell registry: no socket round-trips, no
//! screen-grid access and no asciicast reads — those are the buffer readers'
//! substrate (03b).

use crate::ai::filter::mask_sensitive;
use crate::daemon::utils::normalize_output;
use crate::shell::{
    Owner, ShellId, ShellRecord, ShellRegistry, cast_path, meta_path_for, with_shells,
};

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
    match scope {
        None | Some("mine") => {
            let key = caller.key();
            let rows = with_shells(reg, |store| {
                store
                    .values()
                    .filter(|record| record.owner.key() == key)
                    .map(collect_row)
                    .collect::<Vec<_>>()
            });
            format_shell_rows(&rows, scope, now_unix)
        }
        Some("all") => {
            let rows = with_shells(reg, |store| {
                store.values().map(collect_row).collect::<Vec<_>>()
            });
            format_shell_rows(&rows, scope, now_unix)
        }
        Some(other) => format!(
            "Error: unknown list_shells scope {other:?}. Accepted values: \
             \"mine\" (this session's shells, the default) or \"all\" (every \
             shell the daemon is running)."
        ),
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

/// Pure: the command indices `read_shell` should read. `since_command` selects
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

/// Pure: turn the concatenated command output + the optional `lines` clamp and
/// `filter` into the body text. Everything above this function is I/O; this is
/// the part tests can hand-build. `Ok(None)` is the filtered-empty case.
fn build_body(
    bytes: Vec<u8>,
    lines: Option<u64>,
    filter: Option<&regex::Regex>,
) -> Result<Option<String>, String> {
    let decoded = String::from_utf8_lossy(&bytes);
    let normalized = normalize_output(&decoded);
    let matched: Vec<&str> = if let Some(re) = filter {
        normalized.lines().filter(|l| re.is_match(l)).collect()
    } else {
        normalized.lines().collect()
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

/// The registry + cast-file half of `read_shell`. `registry` and `now` are
/// parameters rather than module reads: `list_shells`'s `list_shells_in` seam,
/// so the lookup can be driven from a hand-built registry without touching the
/// process-global one.
fn read_shell_in(
    reg: &ShellRegistry,
    id: &str,
    lines: Option<u64>,
    since_command: Option<u32>,
    filter: Option<&str>,
) -> String {
    let resolved = with_shells(reg, |store| {
        store
            .get(id)
            .map(|record| (record.started_unix, record.owner.key()))
    });
    let Some((started_unix, owner_key)) = resolved else {
        let live = with_shells(reg, |store| {
            let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
            ids.sort_by_key(|i| ShellId::parse(i).map(|s| s.index()).unwrap_or(u32::MAX));
            ids
        });
        return if live.is_empty() {
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
        };
    };
    let cast = cast_path(id, started_unix);
    let meta_path = meta_path_for(&cast);
    let Some(meta) = crate::shell::MetaIndex::load(&meta_path).ok() else {
        if std::fs::metadata(&cast).is_err() {
            return format!(
                "Error: shell {id} ({owner_key}) has no recorded cast log at {} yet — \
                 nothing this shell ran has been recorded. Run a command in it, or \
                 check the shell survived restart.",
                meta_path.display()
            );
        }
        return format!(
            "Error: shell {id} ({owner_key}) has a cast log but its command index \
             {} is missing or unreadable. The command index is rebuilt by the \
             shell host; retry, or list_shells and use an id that reports commands.",
            meta_path.display()
        );
    };
    if meta.commands.is_empty() {
        return format!(
            "Shell {id} ({owner_key}) is live but has recorded no commands yet — \
             the recording starts with the first command run in it."
        );
    }
    let indices = match select_command_indices(&meta, since_command) {
        Some(i) => i,
        None => {
            let highest = meta.commands.iter().map(|c| c.index).max().unwrap_or(0);
            return format!(
                "Error: since_command {since:?} is beyond every recorded command \
                 of shell {id} — the highest recorded index is {highest}. Call \
                 with a smaller index (0 is always valid: it is the shell's \
                 first recorded command).",
                since = since_command.unwrap_or_default(),
            );
        }
    };
    let mut bytes = Vec::new();
    for index in &indices {
        match crate::shell::read_command_output(&cast, &meta, *index) {
            Ok(mut out) => bytes.append(&mut out),
            Err(e) => {
                return format!(
                    "Error: failed reading shell {id}'s recorded output at command \
                     index {index}: {e}. The cast file may have been rotated; \
                     list_shells and re-read."
                );
            }
        }
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
    let body = match build_body(bytes, lines, filter_re.as_ref()) {
        Ok(body) => body,
        Err(msg) => return msg,
    };
    let header = format!(
        "Shell {id} ({owner_key}) — {} command{}{}{}:",
        indices.len(),
        if indices.len() == 1 { "" } else { "s" },
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

#[cfg(test)]
mod tests {
    use super::*;
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

    fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::test_home_guard();
        let old = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp.0);
        }
        f();
        match old {
            Some(v) => unsafe {
                std::env::set_var("HOME", v);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
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
}
