# Phase 08: One Targetable-Panes Filter + Docs True at Close

**Milestone:** M12 — Full-View tmux Integration
**Status:** in-progress
**Depends on:** phase-05, phase-06b, phase-07
**Estimated diff:** ~340 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

D6: replace the seven-way duplication of `de-*` window-prefix literals with one
shared predicate, and fix the lock-ordering inconsistency phase-01 left behind
at the same sites. Then verify the milestone's docs-true-at-close criterion.

**Pure refactor plus verification.** No new tool, no behavior change: every
site must classify exactly the windows it classifies today. The counts line
stays at **36 tools: 27 core + 9 deferred**.

This is M12's last in-scope phase.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § "D6 — One targetable-panes filter" — the
  settled design: *"A single function … answering 'is this pane daemon-owned?'
  and 'is this pane targetable (not chat, not daemon-owned)?', used by
  `pane_map_summary`, the `list_panes` tool, `handle_list_panes`, and the new
  tools."*
- `docs/dev/milestones/M12-tmux-integration/README.md` § "Carried to phase 08 —
  lock-ordering inconsistency across the filter sites" — the second half of
  this phase, and the reason it is not a pure find-and-replace.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### The duplication, enumerated

`src/daemon/mod.rs:50-66` defines six prefix constants:

```rust
pub const DAEMON_WINDOW_PREFIX: &str = "de-";
pub const BG_WINDOW_PREFIX: &str = "de-bg-";
pub const SCHED_WINDOW_PREFIX: &str = "de-sj-";
pub const GS_BG_WINDOW_PREFIX: &str = "de-gs-bg-";
pub const GS_SCHED_WINDOW_PREFIX: &str = "de-gs-sj-";
pub const INCIDENT_WINDOW_PREFIX: &str = "de-gs-ir-";
```

**Seven sites re-derive the classification.** Find them with:

```bash
grep -rn '"de-bg-"\|"de-sj-"\|"de-gs-bg-"\|"de-gs-sj-"\|"de-gs-ir-"' --include=*.rs src/
grep -rn 'starts_with(crate::daemon::' --include=*.rs src/
```

Three of them are the **byte-identical five-line conjunction** with raw string
literals — in `src/tmux/cache.rs` (the bell-window filter and
`pane_map_summary`) and in `src/daemon/server/handlers.rs`
(`handle_list_panes`):

```rust
                !s.window_name.starts_with("de-bg-")
                    && !s.window_name.starts_with("de-sj-")
                    && !s.window_name.starts_with("de-gs-bg-")
                    && !s.window_name.starts_with("de-gs-sj-")
                    && !s.window_name.starts_with("de-gs-ir-")
```

(the binding is `s`, `w` or `state` depending on the site). A fourth, in
`get_labeled_context`, is the same five prefixes in **positive** form for the
`BACKGROUND PANE` label. Two more are **ghost-only** — three prefixes, not five
— for the `[ghost]` tag: one in `get_labeled_context`, one in `list_panes`
(`src/daemon/executor/knowledge/pane.rs`). The seventh is
`is_daemon_window` in `executor/knowledge/pane.rs`, added by phase-05, which
already uses the constants correctly and is the shape to generalise.

**Line numbers have drifted across phases 05–07 — locate every site by the
greps above, not by a line number quoted here.**

### The lock-ordering hazard, and why it is real but not a bug today

Phase-01 added a home-session filter at five sites with **inconsistent lock
order**. Three clone `session_name` before acquiring `panes`; three hold the
`panes` read guard and acquire `session_name` inside it, e.g. in
`handle_list_panes`:

```rust
        let panes = cache.panes.read().unwrap_or_log();
        let home = cache.session_name.read().unwrap_or_log().clone();
```

**No deadlock is possible today** — verified at the phase-01 review: every
`session_name` guard in the codebase is a statement-temporary dropped at its
`;`, so nothing ever holds `session_name` while waiting on `panes`, and there
is no cycle. It is a latent hazard: binding one of those guards to a `let` in a
future edit closes it. This phase touches all of those sites anyway, which is
why the fix lands here.

### Docs state

`CLAUDE.md` already reads `**36 tools: 27 core + 9 deferred.**` with rows for
`read_pane`, `find_in_panes` and `tmux_control`, and `assets/prompts/sre.toml`
documents all three. `tests/doc_truth.rs` is green. Task 6 **verifies** this; it
should need no edit, and if it does, that is a finding worth stating in the
Update Log rather than a licence to rewrite the table.

**Tests today:** 1195 in the lib suite.

## Spec

### Task 1 — Three predicates in `src/daemon/mod.rs`

Directly below the prefix constants. This is where D6 says they belong — beside
the constants they encode. Write the bodies exactly as given, trailing comments
included: two of these lines are mutation targets.

```rust
/// True when `window_name` is a window this daemon created and manages.
///
/// The single source of truth for that question (M12 D6). Note it deliberately
/// does **not** use `DAEMON_WINDOW_PREFIX` (`"de-"`): that would also match a
/// user's own window called `de-icing`.
pub fn is_daemon_window(window_name: &str) -> bool {
    window_name.starts_with(BG_WINDOW_PREFIX)
        || window_name.starts_with(SCHED_WINDOW_PREFIX)
        || window_name.starts_with(GS_BG_WINDOW_PREFIX)
        || window_name.starts_with(GS_SCHED_WINDOW_PREFIX)
        || window_name.starts_with(INCIDENT_WINDOW_PREFIX) // all five daemon prefixes
}

/// True when `window_name` belongs to a Ghost Shell specifically — a strict
/// subset of [`is_daemon_window`]. `de-bg-` and `de-sj-` are daemon windows but
/// not ghost windows.
pub fn is_ghost_window(window_name: &str) -> bool {
    window_name.starts_with(GS_BG_WINDOW_PREFIX)
        || window_name.starts_with(GS_SCHED_WINDOW_PREFIX)
        || window_name.starts_with(INCIDENT_WINDOW_PREFIX)
}

/// True when a pane may be offered to the user or the agent as a target: not
/// daemon-managed, and not the chat pane itself.
pub fn is_targetable_pane(window_name: &str, pane_id: &str, chat_pane: Option<&str>) -> bool {
    !is_daemon_window(window_name) && chat_pane != Some(pane_id) // never target the chat pane
}
```

### Task 2 — Rewrite the five-prefix sites

Every site found by the § Current state greps, **except** the two ghost-only
ones (Task 3):

- The two in `src/tmux/cache.rs` (the bell-window filter and
  `pane_map_summary`) and the one in `handle_list_panes`
  (`src/daemon/server/handlers.rs`) become
  `!crate::daemon::is_daemon_window(&<binding>.window_name)`.
- **`pane_map_summary` and `handle_list_panes` additionally filter the chat
  pane**, on a separate `.filter(...)` line. Collapse *both* filters into one
  `crate::daemon::is_targetable_pane(&s.window_name, id, chat_pane)` call —
  that pairing is exactly what D6's second question exists for. Leave the
  **home-session** filter as its own separate `.filter(...)`; it is a different
  question and does not belong in the predicate.
- The positive-form five-prefix test in `get_labeled_context` (the
  `BACKGROUND PANE` label) becomes
  `crate::daemon::is_daemon_window(&state.window_name)`.
- `is_daemon_window` in `src/daemon/executor/knowledge/pane.rs` keeps its name,
  signature and visibility — phase-06b's callers must not change — and its body
  becomes the one-line delegation `crate::daemon::is_daemon_window(window_name)`.
  This is the one-line change phase-06b's spec promised.

**Behavior must not change at any site.** Each rewrite classifies exactly the
same windows; if you find a site where it would not, stop and record it as a
blocker rather than "fixing" the behavior here.

### Task 3 — Rewrite the two ghost-only sites

The `[ghost]` tag in `get_labeled_context` (`src/tmux/cache.rs`) and in
`list_panes` (`src/daemon/executor/knowledge/pane.rs`) both test the three
ghost prefixes. Both become `crate::daemon::is_ghost_window(&…window_name)`.

Do **not** widen them to `is_daemon_window` — the tag means "ghost", and
`de-bg-` / `de-sj-` windows must keep *not* getting it. This is the distinction
the two predicates exist to preserve.

### Task 4 — Fix the lock ordering at every site this phase touches

At each site that acquires both `session_name` and `panes`, read
`session_name` into a local **before** acquiring the `panes` guard:

```rust
        // Read session_name before the panes guard (M12 lock ordering).
        let home = cache.session_name.read().unwrap_or_log().clone();
        let panes = cache.panes.read().unwrap_or_log();
```

Apply it to `pane_map_summary` and `get_labeled_context_scoped` in
`src/tmux/cache.rs` and to `handle_list_panes` in
`src/daemon/server/handlers.rs` — the three the milestone README names as
inconsistent. The other two already do it and need no change.

**Never bind a `session_name` guard to a `let` that outlives the statement.**
The whole reason no deadlock exists today is that every one of those guards is
a statement-temporary; `.clone()` on the same line is what keeps it that way.

### Task 5 — Tests

Write the five tests named in § Test plan, in a test module in
`src/daemon/mod.rs`. All are pure calls to the three predicates — **no test in
this phase may reach tmux or a cache.**

### Task 6 — Verify the milestone's docs-true-at-close criterion

This task is a **check, not an edit**. Run:

```bash
grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md
grep -c '| `read_pane` | core |' CLAUDE.md
grep -c '| `find_in_panes` | core |' CLAUDE.md
grep -c '| `tmux_control` | core |' CLAUDE.md
grep -c 'read_pane\|find_in_panes\|tmux_control' assets/prompts/sre.toml
cargo test --test doc_truth
```

Each of the first four must print `1`; the fifth `3` or more; `doc_truth` must
pass. They are expected to be true already. **If any is not, fix that specific
thing and say so in the Update Log** — do not restructure the table.

One edit is expected: `CLAUDE.md` § "Key files" should gain a line for the new
`src/daemon/mod.rs` predicates, since that file's row does not mention them.

### Task 7 — Apply mutation M1 and capture both directions

Per `docs/dev/WORKFLOW.md` § "End-to-end verification", the edit is a **`patch`
tool call, not `sed -i`** — in-place shell edits are banned by your contract
and `bash` refuses them.

1. `patch` `src/daemon/mod.rs`:
   - `old_str`: `        || window_name.starts_with(INCIDENT_WINDOW_PREFIX) // all five daemon prefixes`
   - `new_str`: `        || window_name.starts_with(GS_SCHED_WINDOW_PREFIX) // all five daemon prefixes`
   That drops the incident prefix by duplicating another, so it still compiles
   and every constant stays used.
2. Append the marker, the applied-check and the mutated run:
   ```bash
   echo "== M1 APPLIED ==" >> /tmp/e2e-08.txt
   echo -n "M1 mutated-lines-present=" >> /tmp/e2e-08.txt
   grep -c 'starts_with(GS_SCHED_WINDOW_PREFIX) // all five daemon prefixes' src/daemon/mod.rs >> /tmp/e2e-08.txt
   cargo test is_daemon_window 2>&1 | grep -E '^test .*(ok|FAILED)$|^test result:|panicked at' | head -10 >> /tmp/e2e-08.txt
   echo "M1 exit=${PIPESTATUS[0]}" >> /tmp/e2e-08.txt
   ```
   The `grep -c` must print `1`; a `0` means the `patch` hit the wrong line and
   the pair proves nothing.
3. `patch` it back, append `== M1 RESTORED ==`, the same `grep -c` (now `0`),
   the restored run, and `M1 restored exit=`.

### Task 8 — Apply mutation M2 and capture both directions

Same procedure on the chat-pane clause:

- `old_str`: `    !is_daemon_window(window_name) && chat_pane != Some(pane_id) // never target the chat pane`
- `new_str`: `    !is_daemon_window(window_name) // never target the chat pane`

That leaves `pane_id` and `chat_pane` unused, which is a warning, not an error —
`cargo test` still runs. Use `grep -c '!is_daemon_window(window_name) // never target'`,
the test filter `is_targetable_pane`, and the `M2` labels.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim — it **appends** to the
same `/tmp/e2e-08.txt` Tasks 7 and 8 wrote, so run it **after** them.

Then paste the entire file into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`, inside one fenced block, and
run the paste-fidelity check in that section. It must print **`PASTE MATCH`**;
record that line in a second entry headed `### Update — <date> (paste check)`.

**Read the file and copy its bytes** — do not reconstruct the transcript from
what you remember the commands printing. The server-authored `(complete)` entry
does not satisfy this.

## Acceptance criteria

- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings`, `cargo test` all exit 0.
- [ ] **The milestone's D6 exit criterion:**
      `grep -rn '"de-bg-"\|"de-sj-"\|"de-gs-bg-"\|"de-gs-sj-"\|"de-gs-ir-"' --include=*.rs src/ | grep -v '^src/daemon/mod.rs' | wc -l`
      prints `0` — no raw prefix literal survives anywhere outside the constant
      definitions.
- [ ] `grep -c 'pub fn is_daemon_window\|pub fn is_ghost_window\|pub fn is_targetable_pane' src/daemon/mod.rs`
      prints `3`.
- [ ] `grep -c 'crate::daemon::is_daemon_window\|crate::daemon::is_ghost_window\|crate::daemon::is_targetable_pane' src/tmux/cache.rs src/daemon/server/handlers.rs src/daemon/executor/knowledge/pane.rs`
      shows a non-zero count in **all three** files.
- [ ] In `src/tmux/cache.rs` and `src/daemon/server/handlers.rs`, every
      `session_name.read()` this phase touched appears on an **earlier line**
      than the `panes.read()` in the same block.
- [ ] `grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md` prints `1`
      and `cargo test --test doc_truth` passes.
- [ ] All five tests named in § Test plan pass.
- [ ] Mutation M1: `M1 mutated-lines-present=1`, the mutated
      `cargo test is_daemon_window` reports `FAILED`, the restored count is `0`
      and the tests pass. Both directions in the transcript.
- [ ] Mutation M2: the same shape for `is_targetable_pane`.
- [ ] The Update Log holds a new `### Update — <date> (end-to-end
      verification)` entry containing `/tmp/e2e-08.txt` byte for byte, and a
      `### Update — <date> (paste check)` entry reading `PASTE MATCH`.

## Test plan

All in a test module in `src/daemon/mod.rs`, all pure:

- `is_daemon_window_matches_all_five_prefixes` — one window name per prefix
  (`de-bg-1-…`, `de-sj-1-…`, `de-gs-bg-1-…`, `de-gs-sj-1-…`, `de-gs-ir-1-…`),
  each asserted `true`, **each with its own assertion message naming the
  prefix** so a failure says which one was dropped. **Mutation M1's target.**
- `is_daemon_window_rejects_user_windows` — the negative cases that matter:
  `"main"`, `"editor"`, and **`"de-icing"`**, which starts with
  `DAEMON_WINDOW_PREFIX` but is a user window. All `false`.
- `is_ghost_window_matches_only_ghost_prefixes` — the three ghost prefixes are
  `true`; `de-bg-…` and `de-sj-…` are **`false`** despite being daemon windows.
  This asymmetry is the whole reason there are two predicates.
- `is_targetable_pane_excludes_daemon_and_chat` — a user window with a
  non-chat pane id is `true`; the same pane id when it *is* the chat pane is
  `false`; a daemon window is `false` even when it is not the chat pane.
  **Mutation M2's target.**
- `is_targetable_pane_with_no_chat_pane` — `chat_pane: None` on a user window
  is `true`; the boundary case where there is no chat pane at all must not
  exclude everything.

## End-to-end verification

Run **verbatim** from the repo root, in `bash`, **without** `set -e`, and
**after** Tasks 7 and 8 — it appends to the file they created. Each command is
piped through `tail`/`grep` so the artifact stays small enough to paste whole.
`${PIPESTATUS[0]}` is read on the line immediately after each pipeline; do not
move those lines apart.

```bash
OUT=/tmp/e2e-08.txt

echo "== D6 EXIT CRITERION ==" >> $OUT
echo -n "raw prefix literals outside daemon/mod.rs (want 0)=" >> $OUT
grep -rn '"de-bg-"\|"de-sj-"\|"de-gs-bg-"\|"de-gs-sj-"\|"de-gs-ir-"' --include=*.rs src/ | grep -v '^src/daemon/mod.rs' | wc -l >> $OUT
echo -n "predicates declared (want 3)=" >> $OUT
grep -c 'pub fn is_daemon_window\|pub fn is_ghost_window\|pub fn is_targetable_pane' src/daemon/mod.rs >> $OUT 2>&1
echo "-- call sites per file --" >> $OUT
grep -c 'crate::daemon::is_daemon_window\|crate::daemon::is_ghost_window\|crate::daemon::is_targetable_pane' src/tmux/cache.rs src/daemon/server/handlers.rs src/daemon/executor/knowledge/pane.rs >> $OUT 2>&1

echo "== LOCK ORDERING ==" >> $OUT
grep -n 'session_name.read()\|panes.read()' src/tmux/cache.rs >> $OUT 2>&1
grep -n 'session_name.read()\|panes.read()' src/daemon/server/handlers.rs >> $OUT 2>&1

echo "== DOCS TRUE AT CLOSE ==" >> $OUT
echo -n "tool counts (want 1)=" >> $OUT
grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md >> $OUT 2>&1
echo -n "read_pane row (want 1)=" >> $OUT
grep -c '| `read_pane` | core |' CLAUDE.md >> $OUT 2>&1
echo -n "find_in_panes row (want 1)=" >> $OUT
grep -c '| `find_in_panes` | core |' CLAUDE.md >> $OUT 2>&1
echo -n "tmux_control row (want 1)=" >> $OUT
grep -c '| `tmux_control` | core |' CLAUDE.md >> $OUT 2>&1
echo -n "sre.toml documents all three (want >=3)=" >> $OUT
grep -c 'read_pane\|find_in_panes\|tmux_control' assets/prompts/sre.toml >> $OUT 2>&1

echo "== GATES ==" >> $OUT
cargo fmt --all 2>&1 | tail -3 >> $OUT
echo "fmt exit=${PIPESTATUS[0]}" >> $OUT
cargo build 2>&1 | tail -3 >> $OUT
echo "build exit=${PIPESTATUS[0]}" >> $OUT
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3 >> $OUT
echo "clippy exit=${PIPESTATUS[0]}" >> $OUT
cargo test 2>&1 | grep -E '^test result:|^failures:|panicked at' | head -20 >> $OUT
echo "test exit=${PIPESTATUS[0]}" >> $OUT
cargo test --test doc_truth 2>&1 | grep -E '^test result:' | head -3 >> $OUT
echo "doc_truth exit=${PIPESTATUS[0]}" >> $OUT

echo "== TREE ==" >> $OUT
git status --porcelain >> $OUT 2>&1
echo "porcelain exit=$?" >> $OUT
echo -n "transcript line count=" >> $OUT
wc -l < $OUT >> $OUT
```

Then, **after** pasting the file into the Update Log, run the paste-fidelity
check and record its final line:

```bash
D=docs/dev/milestones/M12-tmux-integration/phase-08-filter-unification-and-docs.md
START=$(grep -n '^### Update .*(end-to-end verification)' $D | tail -1 | cut -d: -f1)
tail -n +$START $D | awk '/^```/{n++; next} n==1' > /tmp/pasted-08.txt
diff /tmp/pasted-08.txt /tmp/e2e-08.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Expected readings: the D6 literal count `0`; three predicates; a non-zero call
count in each of the three files; in the lock-ordering dump, each
`session_name.read()` line number **below** the `panes.read()` of its block;
every docs `want` as stated; all four gate exits `0`; both
`M* mutated-lines-present=1` with a `FAILED` line, and both `M* restored`
counts `0` with the tests passing; nothing between `== TREE ==` and
`porcelain exit=0`; and `PASTE MATCH`.

## Authorizations

None. No new dependencies; no `docs/architecture.md` changes.

## Out of scope

- **Any behavior change.** This is a refactor: every site must classify exactly
  the windows it classifies today. If a rewrite would change what a site
  accepts, that is a blocker to record, not a bug to fix here.
- **`DAEMON_WINDOW_PREFIX` (`"de-"`) and its one caller** in
  `src/cli/local_cmds.rs`. That call site is a deliberate broad sweep over
  everything the daemon owns, not the targetable-panes question; leave it.
- **Widening the `[ghost]` tag** to all daemon windows, or narrowing
  `is_daemon_window` to the ghost prefixes. The two predicates are different
  questions and both are load-bearing.
- **The milestone retrospective and closing `NEXT.md`.** This phase being
  approved is what makes M12's last in-scope phase `done`; writing the
  retrospective, folding calibration lessons and setting `NEXT.md` to "none"
  are a separate, human-gated step
  (`docs/dev/WORKFLOW.md` § "Milestone boundaries are always a human gate").
  Do not do any of it here.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-08 17:36 (started)

**Executor:** rexyMCP executor

Implemented D6 predicates (`is_daemon_window`, `is_ghost_window`, `is_targetable_pane`) in `src/daemon/mod.rs`, rewrote all seven prefix-literal sites to use them, fixed lock ordering at `pane_map_summary`, `get_labeled_context_scoped`, and `handle_list_panes`, added five pure tests, verified docs-true-at-close criterion, applied mutations M1 and M2 in both directions, and added `src/daemon/mod.rs` row to CLAUDE.md Key files table.

### Update — 2026-08-08 17:36 (end-to-end verification)

```
== M1 APPLIED ==
M1 mutated-lines-present=1
test daemon::tests::is_daemon_window_rejects_user_windows ... ok
test daemon::tests::is_daemon_window_matches_all_five_prefixes ... FAILED
thread 'daemon::tests::is_daemon_window_matches_all_five_prefixes' (1882820) panicked at src/daemon/mod.rs:1059:9:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 1198 filtered out; finished in 0.00s
M1 exit=101
== M1 RESTORED ==
M1 restored count (want 0)=0
test daemon::tests::is_daemon_window_matches_all_five_prefixes ... ok
test daemon::tests::is_daemon_window_rejects_user_windows ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1198 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s
M1 restored exit=0
== M2 APPLIED ==
M2 mutated-lines-present=1
test daemon::tests::is_targetable_pane_with_no_chat_pane ... ok
test daemon::tests::is_targetable_pane_excludes_daemon_and_chat ... FAILED
thread 'daemon::tests::is_targetable_pane_excludes_daemon_and_chat' (1886633) panicked at src/daemon/mod.rs:1086:9:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 1198 filtered out; finished in 0.00s
M2 exit=101
== M2 RESTORED ==
M2 restored count (want 0)=0
test daemon::tests::is_targetable_pane_excludes_daemon_and_chat ... ok
test daemon::tests::is_targetable_pane_with_no_chat_pane ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1198 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s
M2 restored exit=0
== D6 EXIT CRITERION ==
raw prefix literals outside daemon/mod.rs (want 0)=0
predicates declared (want 3)=3
-- call sites per file --
src/tmux/cache.rs:4
src/daemon/server/handlers.rs:1
src/daemon/executor/knowledge/pane.rs:2
== LOCK ORDERING ==
219:        let session = self.session_name.read().unwrap_or_log().clone();
246:        let home = self.session_name.read().unwrap_or_log().clone();
260:        let session = self.session_name.read().unwrap_or_log().clone();
453:        let home = self.session_name.read().unwrap_or_log().clone();
454:        let panes = self.panes.read().unwrap_or_log();
569:                let panes = self.panes.read().unwrap_or_log();
668:        let home = self.session_name.read().unwrap_or_log().clone();
669:        let panes = self.panes.read().unwrap_or_log();
822:            let session_name = self.session_name.read().unwrap_or_log().clone();
136:        let panes = cache.panes.read().unwrap_or_log();
179:        let home = cache.session_name.read().unwrap_or_log().clone();
180:        let panes = cache.panes.read().unwrap_or_log();
== DOCS TRUE AT CLOSE ==
tool counts (want 1)=1
read_pane row (want 1)=1
find_in_panes row (want 1)=1
tmux_control row (want 1)=1
sre.toml documents all three (want >=3)=4
== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.10s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.76s
clippy exit=0
test result: ok. 1200 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.29s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
doc_truth exit=0
== TREE ==
 M CLAUDE.md
 M src/daemon/executor/knowledge/pane.rs
 M src/daemon/mod.rs
 M src/daemon/server/handlers.rs
 M src/tmux/cache.rs
porcelain exit=0
transcript line count=87
```

### Update — 2026-08-08 17:36 (paste check)

PASTE MATCH
