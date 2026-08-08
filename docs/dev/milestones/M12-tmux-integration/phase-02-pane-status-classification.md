# Phase 02: Pane Status Classification

**Milestone:** M12 — Full-View tmux Integration
**Status:** todo
**Depends on:** phase-01
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Replace `SessionCache::summarize()`'s pattern-matching heuristics with a
derived `PaneStatus` enum (D2): a pure classification function computed from
metadata the cache already holds — no new tmux calls. Every cached pane gains
a live `status` field refreshed each 2 s cycle, and the one-line pane summary
becomes `<status> — <last meaningful line>`. Phases 03–07 render this status
on their new surfaces; this phase creates it and flows it through the existing
summary consumers (`[BACKGROUND PANE]`/`[SESSION PANE]` context lines and the
pane-select prompt).

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § D2 — the settled design this phase
  implements (status taxonomy, priority, rendering).
- `CLAUDE.md` § "Key files" rows for `src/tmux/cache.rs` and
  `src/daemon/executor/` (the `foreground.rs` shell-prompt helper moves).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All facts below were verified against the tree at drafting (2026-08-07;
baseline `cargo test --lib` = **1153 passed**). Line numbers are
current-as-of-drafting; re-derive with the greps shown if they drift.

**The heuristics being replaced.** `SessionCache::summarize()`
(`src/tmux/cache.rs:389-404`) recognizes `$`/`#` first characters, `top -`/
`htop`, and `GET /`/`POST /` substrings; everything else collapses to
`Active: <last line truncated to 50 chars>`:

```rust
// src/tmux/cache.rs:389-404 — DELETE this method in Task 4
fn summarize(&self, buffer: &str) -> String {
    let Some(last_line) = buffer.lines().rfind(|l| !l.trim().is_empty()) else {
        return "Empty pane".to_string();
    };
    let last_line = last_line.trim();

    if last_line.starts_with('$') || last_line.starts_with('#') {
        format!("Idle shell at: {}", last_line)
    } else if last_line.contains("top - ") || last_line.contains("htop") {
        "Running system monitor".to_string()
    } else if last_line.contains("GET /") || last_line.contains("POST /") {
        "Tailing web logs".to_string()
    } else {
        format!("Active: {}", last_line.chars().take(50).collect::<String>())
    }
}
```

Its only production call site is `refresh()`'s buffer-update branch
(`src/tmux/cache.rs:306-312`):

```rust
if let Some(c) = content
    && entry.buffer != c
{
    entry.buffer = c;
    entry.summary = self.summarize(&entry.buffer);
    entry.last_updated = std::time::Instant::now();
}
```

**The two summary consumers** (behavior flows through unchanged — do not edit
them): `get_labeled_context`'s pane lines (`src/tmux/cache.rs:740`,
`mask_sensitive(&state.summary)`) and `find_best_target_pane`'s
`PaneInfo.summary` (`src/daemon/executor/mod.rs:973`).

**The shell-name predicate to reuse.** `is_shell_prompt`
(`src/daemon/executor/foreground.rs:97-115`) is `pub(super)`, matching 13
shell names (`bash`, `zsh`, `fish`, `sh`, `ksh`, `csh`, `tcsh`, `dash`, `nu`,
`pwsh`, `elvish`, `xonsh`, `yash`) after `trim()`. `src/tmux/` cannot reach it
(daemon depends on tmux, not vice versa), so the function **moves** to the new
status module and `foreground.rs` re-exports it. Its callers: four sites in
`src/daemon/executor/knowledge/pane.rs` (lines 279, 288, 303, 312, all via
`super::super::foreground::is_shell_prompt`) and three tests inside
`foreground.rs` (lines 1293-1314, importing from `super`). A `pub(super) use`
re-export satisfies every one of them unchanged.

**Inputs the classifier needs, all already on `PaneState`**
(`src/tmux/cache.rs:74-118`, refreshed every cycle from `RichPaneInfo`):
`dead: bool`, `dead_status: Option<i32>`, `current_cmd: String`,
`last_activity: u64` (unix seconds, **0 = unknown**), `window_name: String`.
Bell state lives on `WindowState` (`src/tmux/window.rs:23`, `has_bell()` =
`flags.contains('!')`); the cache stores the session's windows in
`self.windows` (refreshed at `src/tmux/cache.rs:328-379`, *after* the pane
write loop — see Task 4 for the consequence).

**`PaneState { ... }` struct literals that must gain the new field** (the
struct has no `Default`; the compiler will enumerate any missed one as a
missing-field error): the `or_insert_with` in `refresh()`
(`src/tmux/cache.rs:267`), the `pane(...)` fixture fn in
`src/daemon/executor/knowledge/pane.rs:428-449`, and 15 literals in
`src/tmux/cache_tests.rs` (14 standalone plus one inside the `test_pane`
helper at line 609). Re-derive the list with
`grep -rn 'PaneState {' src --include='*.rs'`.

**The 8 tests being deleted** (they call the removed method):
`summarize_empty_buffer`, `summarize_only_blank_lines`,
`summarize_dollar_prompt`, `summarize_hash_prompt`, `summarize_top_output`,
`summarize_web_log_get`, `summarize_web_log_post`,
`summarize_generic_truncates_to_50_chars` — `src/tmux/cache_tests.rs:9-63`.
`cargo test --lib summarize` runs exactly these 8 today.

## Spec

The exact code in Tasks 1 and 4 was **prototyped, compiled, and test-run
against this tree at drafting** (including both mutation pairs, executed in
both directions), then reverted. It is evidence, not a sketch.

### Task 1 — New module `src/tmux/status.rs`

Create `src/tmux/status.rs` with the following content (verbatim — this
compiled clean under `-D warnings` and its behavior is pinned by the Test
plan), and register it in `src/tmux/mod.rs` by adding `pub mod status;` after
the existing `pub mod pane;` line. Do **not** add a glob re-export
(`pub use status::*;`) — callers use explicit `crate::tmux::status::` paths.

```rust
//! Pane status classification (M12 D2).

/// Output within this many seconds counts as "recent" for a shell pane.
const ACTIVE_WINDOW_SECS: u64 = 30;
/// A non-shell command with no output for at least this long is awaiting input.
const AWAITING_THRESHOLD_SECS: u64 = 60;

/// Live status of a tmux pane, derived from cached metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneStatus {
    /// Foreground process has exited (remain-on-exit); carries the exit code.
    Dead(Option<i32>),
    /// The pane's window holds an uncleared tmux bell flag.
    Bell,
    /// Non-shell foreground command with no output for ≥ 60 s.
    AwaitingInput,
    /// Non-shell foreground command with recent (or unknown) output.
    Running,
    /// Shell prompt with output within the last 30 s.
    Active,
    /// Shell prompt with no recent output; carries the age in seconds
    /// (0 = age unknown).
    Idle(u64),
}

/// Return true when `cmd` is a shell name, meaning the pane is at a prompt.
pub fn is_shell_prompt(cmd: &str) -> bool {
    matches!(
        cmd.trim(),
        "bash"
            | "zsh"
            | "fish"
            | "sh"
            | "ksh"
            | "csh"
            | "tcsh"
            | "dash"
            | "nu"
            | "pwsh"
            | "elvish"
            | "xonsh"
            | "yash"
    )
}

/// Classify a pane from cached metadata. Pure — no tmux calls.
///
/// Priority: Dead > Bell > (shell? Active/Idle : Running/AwaitingInput).
/// `last_activity == 0` means "unknown": a shell classifies as `Idle(0)`,
/// a non-shell command as `Running` — never `AwaitingInput` without evidence.
pub fn classify(
    dead: bool,
    dead_status: Option<i32>,
    has_bell: bool,
    current_cmd: &str,
    last_activity: u64,
    now: u64,
) -> PaneStatus {
    if dead {
        return PaneStatus::Dead(dead_status);
    }
    if has_bell {
        return PaneStatus::Bell;
    }
    let age = (last_activity > 0).then(|| now.saturating_sub(last_activity));
    if is_shell_prompt(current_cmd) {
        match age {
            Some(a) if a < ACTIVE_WINDOW_SECS => PaneStatus::Active,
            Some(a) => PaneStatus::Idle(a),
            None => PaneStatus::Idle(0),
        }
    } else {
        match age {
            Some(a) if a >= AWAITING_THRESHOLD_SECS => PaneStatus::AwaitingInput,
            _ => PaneStatus::Running,
        }
    }
}

/// Format an age in seconds as `45s`, `3m`, or `2h5m`.
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

impl std::fmt::Display for PaneStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaneStatus::Dead(Some(code)) => write!(f, "dead({})", code),
            PaneStatus::Dead(None) => write!(f, "dead(?)"),
            PaneStatus::Bell => write!(f, "bell"),
            PaneStatus::AwaitingInput => write!(f, "awaiting-input"),
            PaneStatus::Running => write!(f, "running"),
            PaneStatus::Active => write!(f, "active"),
            PaneStatus::Idle(0) => write!(f, "idle"),
            PaneStatus::Idle(age) => write!(f, "idle({})", format_age(*age)),
        }
    }
}

/// One-line pane summary: `<status> — <last meaningful line>` (line truncated
/// to 50 chars), or the status alone when the buffer has no non-empty line.
pub fn summarize(status: PaneStatus, buffer: &str) -> String {
    match buffer.lines().rfind(|l| !l.trim().is_empty()) {
        Some(line) => format!(
            "{} — {}",
            status,
            line.trim().chars().take(50).collect::<String>()
        ),
        None => status.to_string(),
    }
}
```

### Task 2 — Re-export `is_shell_prompt` from `foreground.rs`

In `src/daemon/executor/foreground.rs`, delete the `is_shell_prompt` function
body (lines 97-115 — the doc comment and the whole `matches!` fn) and replace
it with a re-export in the same spot:

```rust
/// Shell-name predicate — moved to `crate::tmux::status` (M12 D2), re-exported
/// so the `knowledge::` call sites and the tests below keep their paths.
pub(super) use crate::tmux::status::is_shell_prompt;
```

Do NOT touch the four call sites in `src/daemon/executor/knowledge/pane.rs`,
the three `is_shell_prompt_*` tests in `foreground.rs`, or
`looks_like_shell_prompt` — the re-export keeps all of them compiling
unchanged. The section comment above the function ("Shell prompt detection
helpers…") may stay.

### Task 3 — `PaneState.status` field

In `src/tmux/cache.rs`, add to `PaneState` (after `shell_pid`):

```rust
/// Live status classification, re-derived on every 2 s refresh (D2).
pub status: crate::tmux::status::PaneStatus,
```

Initialize it to `crate::tmux::status::PaneStatus::Idle(0)` in every
`PaneState { ... }` struct literal the compiler flags: the `or_insert_with`
in `refresh()`, the `pane(...)` fixture in
`src/daemon/executor/knowledge/pane.rs`, and the 15 literals in
`src/tmux/cache_tests.rs` (see Current state). Use the compiler's
missing-field errors to find them — do not hunt by re-reading files.

### Task 4 — Stamp status in `refresh()`; replace the summary; delete the method

All in `src/tmux/cache.rs`, inside `refresh()`:

**4a.** Before the `self.panes.write()` acquisition (currently line 265),
compute the clock and the belled-window set. Both guards below are
statement-temporaries — never hold the `windows` (or `session_name`) lock
while taking `panes`:

```rust
let now_secs = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0);
// Windows are refreshed later in this same cycle, so bell state lags one
// 2 s poll — acceptable: every cache field is at most one poll stale.
// Home-session windows only, so foreign panes never classify as Bell.
let belled: std::collections::HashSet<String> = self
    .windows
    .read()
    .unwrap_or_log()
    .iter()
    .filter(|w| w.has_bell())
    .map(|w| w.window_name.clone())
    .collect();
```

**4b.** In the write loop, after the existing field-copy block
(`entry.shell_pid = info.pane_pid;` is its last line), replace the
buffer/summary update quoted in Current state with:

```rust
entry.status = crate::tmux::status::classify(
    entry.dead,
    entry.dead_status,
    belled.contains(&entry.window_name),
    &entry.current_cmd,
    entry.last_activity,
    now_secs,
);

if let Some(c) = content
    && entry.buffer != c
{
    entry.buffer = c;
    entry.last_updated = std::time::Instant::now();
}
entry.summary = crate::tmux::status::summarize(entry.status, &entry.buffer);
```

Two deliberate behavior changes, both intended: the summary is now recomputed
**every** cycle (so its embedded status can never go stale when the buffer is
quiet), and foreign panes — whose buffer stays empty per D1 — now get a
status-only summary (e.g. `idle(3m)`) instead of an empty string. Phase 01's
"do NOT synthesize a summary for foreign panes" restriction is hereby lifted:
this is the phase that owns summaries. `last_updated` keeps its
buffer-changed-only semantics.

**4c.** Delete the `SessionCache::summarize` method entirely.

### Task 5 — Tests

Delete the 8 `summarize_*` tests at `src/tmux/cache_tests.rs:9-63` (they call
the deleted method), including the `// ── summarize heuristics ──` section
comment. Then write the tests named in the Test plan as a
`#[cfg(test)] mod tests` inside `src/tmux/status.rs`. Everything under test
is a pure function — no tmux, no HOME mutation, no `test_home_guard` needed.

### Task 6 — Docs

In `CLAUDE.md` § "Key files", add a row directly below the `src/tmux/mod.rs`
row:

```
| `src/tmux/status.rs` | `PaneStatus` classification (M12 D2): `classify()` pure function, `is_shell_prompt()`, `summarize()` — `<status> — <last line>` pane summaries |
```

This phase adds no AI tool — do NOT touch the tool table, the tool counts
line, or `sre.toml`.

## Acceptance criteria

Split per WORKFLOW.md: the first group are progress markers, each **run and
confirmed to fail against the current tree at drafting** (values shown); the
second group are no-regression guards that already pass and are NOT evidence
of work.

Must currently fail → must pass when done:

- [ ] `grep -c 'pub enum PaneStatus' src/tmux/status.rs` prints `1`
      (drafting: file does not exist).
- [ ] `grep -c 'pub mod status;' src/tmux/mod.rs` prints `1` (drafting: `0`).
- [ ] `grep -c 'pub status: crate::tmux::status::PaneStatus' src/tmux/cache.rs`
      prints `1` (drafting: `0`).
- [ ] `grep -c 'fn summarize' src/tmux/cache.rs` prints `0` (drafting: `1` —
      the method must be deleted, not kept alongside the new path).
- [ ] `grep -c 'status::classify' src/tmux/cache.rs` prints `1`
      (drafting: `0`).
- [ ] `grep -c 'pub(super) use crate::tmux::status::is_shell_prompt;' src/daemon/executor/foreground.rs`
      prints `1` (drafting: `0`).
- [ ] `cargo test --lib tmux::status` runs ≥ 13 passing tests (drafting: `0`
      run under this filter).
- [ ] Negative case: with Mutation pair 1 applied,
      `classify_idle_shell_never_awaiting_input` FAILS. Restored, it passes.
- [ ] Negative case: with Mutation pair 2 applied,
      `classify_dead_wins_over_bell_and_command` FAILS. Restored, it passes.

Already pass today (no-regression guards):

- [ ] `cargo test --lib` green — baseline 1153, minus the 8 deleted
      `summarize_*` tests, plus the new ones. With exactly the Test plan's 13
      tests added this is **1158**; more tests are fine, a lower total than
      1158 is not.
- [ ] `cargo test --lib is_shell_prompt` — 3 passing (the `foreground.rs`
      tests survive the move via the re-export).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all` produces no diff.

## Test plan

All in the new `mod tests` in `src/tmux/status.rs`. Fixture note: `classify`
is pure, so every case pins its inputs inline — use small unix timestamps
(e.g. `last_activity: 1000, now: 1120` for a 120 s age) rather than real
clocks. The pinned negative cases exist because they are the boundaries the
milestone exit criteria name: an idle shell must never read as awaiting
input, and absence of evidence (`last_activity == 0`) must never read as
awaiting input.

- `classify_dead_wins_over_bell_and_command` — `dead=true, dead_status=Some(2),
  has_bell=true, cmd="vim"` → `Dead(Some(2))`. Dead outranks everything.
- `classify_bell_beats_running_command` — alive, `has_bell=true, cmd="vim"`,
  recent activity → `Bell`.
- `classify_running_for_nonshell_with_recent_output` — `cmd="vim"`, age 5 s →
  `Running`.
- `classify_awaiting_input_for_nonshell_stale_output` — `cmd="vim"`, age
  120 s → `AwaitingInput`.
- `classify_idle_shell_never_awaiting_input` — **pinned negative**:
  `cmd="bash"`, age 3600 s → asserts **equals** `Idle(3600)` **and**
  `assert_ne!` `AwaitingInput`.
- `classify_active_shell_with_recent_output` — `cmd="zsh"`, age 5 s →
  `Active`.
- `classify_unknown_activity_shell_is_idle_zero` — `cmd="bash"`,
  `last_activity=0` → `Idle(0)`.
- `classify_unknown_activity_nonshell_is_running` — **pinned negative**:
  `cmd="vim"`, `last_activity=0` → `Running`, NOT `AwaitingInput`.
- `classify_boundary_ages` — age exactly 30 s on a shell → `Idle(30)` (not
  `Active`); age exactly 60 s on a non-shell → `AwaitingInput` (not
  `Running`). The `<` / `>=` boundaries, pinned.
- `status_display_exact_forms` — exact strings:
  `Dead(Some(2))` → `"dead(2)"`, `Dead(None)` → `"dead(?)"`, `Bell` →
  `"bell"`, `AwaitingInput` → `"awaiting-input"`, `Running` → `"running"`,
  `Active` → `"active"`, `Idle(0)` → `"idle"`, `Idle(45)` → `"idle(45s)"`,
  `Idle(180)` → `"idle(3m)"`, `Idle(3600)` → `"idle(1h0m)"`.
- `summarize_empty_buffer_is_status_alone` — `summarize(Running, "")` ==
  `"running"`, and same for a whitespace-only buffer.
- `summarize_appends_last_meaningful_line` —
  `summarize(Active, "out\n$ ")` == `"active — $"` (last non-empty line,
  trimmed, after the em-dash).
- `summarize_truncates_line_to_50_chars` — a 100-char last line yields a
  suffix of exactly 50 chars after the `" — "` separator.

**Mutation pairs — the executor runs BOTH directions and restores, and the
architect re-runs both at review** (self-reported mutation checks alone are
not accepted). Both pairs were executed at drafting against the prototype:
each mutation made exactly the named test fail; restoring made it pass.

1. In `classify`, change `if is_shell_prompt(current_cmd) {` to `if false {`
   → `classify_idle_shell_never_awaiting_input` must FAIL (the 3600 s-old
   bash pane falls into the non-shell arm and classifies `AwaitingInput`).
   Restore → it must pass again.
2. In `classify`, delete the three lines
   `if dead { return PaneStatus::Dead(dead_status); }` →
   `classify_dead_wins_over_bell_and_command` must FAIL (the fixture's
   `has_bell=true` makes it return `Bell`). Restore → pass.

If either mutation leaves the named test green, the fixture is inert —
**report a blocker in the Update Log rather than adjusting the test until it
fails**.

## End-to-end verification

The status classification's live surfaces (`status:` in `list_panes`,
`/panes`) land in phases 05 and 07; the milestone exit criterion's
two-live-panes check is performed there and at milestone close. What this
phase ships at runtime is the new summary shape flowing through
`get_labeled_context`, which spawns tmux subprocesses and is not hermetically
drivable by the executor. The real-artifact check for this phase is therefore
the full gate run, the mutation pairs, and the wiring greps — captured
mechanically:

```sh
cargo test --lib 2>&1 | tail -5 > /tmp/e2e-02.txt; echo "exit=$?" >> /tmp/e2e-02.txt
cargo test --lib tmux::status 2>&1 | grep '^test ' >> /tmp/e2e-02.txt
grep -n 'status::classify\|status::summarize' src/tmux/cache.rs >> /tmp/e2e-02.txt
grep -c 'fn summarize' src/tmux/cache.rs >> /tmp/e2e-02.txt
# Mutation pair 1 — apply, run, restore, run:
sed -i 's/if is_shell_prompt(current_cmd) {/if false {/' src/tmux/status.rs
cargo test --lib classify_idle_shell_never_awaiting_input 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-02.txt
sed -i 's/if false {/if is_shell_prompt(current_cmd) {/' src/tmux/status.rs
cargo test --lib classify_idle_shell_never_awaiting_input 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-02.txt
# Mutation pair 2 — apply (delete the dead early-return), run, restore, run.
# Make the edit manually (delete the 3 lines), then:
cargo test --lib classify_dead_wins_over_bell_and_command 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-02.txt
# restore the 3 lines, then:
cargo test --lib classify_dead_wins_over_bell_and_command 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-02.txt
git diff --stat >> /tmp/e2e-02.txt   # must show NO src/ changes remaining from the mutations
cat /tmp/e2e-02.txt
```

Paste `/tmp/e2e-02.txt`'s contents verbatim into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this — see WORKFLOW.md § "End-to-end
verification".

## Authorizations

None. (No new dependencies; `docs/architecture.md` untouched; the `CLAUDE.md`
edit in Task 6 is authorized by this phase doc.)

## Out of scope

- Rendering `status:<name>` in the `list_panes` tool output, `handle_list_panes`,
  or `/panes` — phases 05 and 07. Do not edit
  `src/daemon/executor/knowledge/pane.rs` beyond the fixture field, and do not
  edit `src/daemon/server/handlers.rs` or `src/ipc.rs` at all.
- Refactoring the duplicated age-formatting blocks in
  `get_labeled_context`/`list_panes` onto `format_age` — later phases touch
  those sites; keep this diff focused.
- Any change to capture behavior, foreign-pane content, or the daemon-window
  prefix filters (phase 08 owns filter unification).
- Making `AWAITING_THRESHOLD_SECS`/`ACTIVE_WINDOW_SECS` configurable.
- No new tmux subprocess calls anywhere in this phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
