# Phase 07: `/panes` Inspector + Widened `PaneList` IPC

**Milestone:** M12 — Full-View tmux Integration
**Status:** in-progress
**Depends on:** phase-02, phase-05
**Estimated diff:** ~380 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

D7: replace `Response::PaneList`'s opaque 5-tuple with a named `PaneInfo`
struct, and turn the bare `/pane` listing into a window-grouped inspector
showing cwd, `PaneStatus`, activity age and a one-line preview. `/pane
<n|%id>` keeps its pinning role exactly as it is.

**No new tool.** The counts line stays at **36 tools: 27 core + 9 deferred**.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § "D7 — `/pane` / `/panes` overhaul" — the
  settled design, including the wire-protocol note (both ends ship in one
  binary, so no compat shim is needed, but the IPC change and the client render
  land in the same phase).
- `docs/design/tmux-integration.md` § "D6 — One targetable-panes filter" — read
  it to know what this phase must **not** do. D6 is phase-08's.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The wire type is a bare tuple** (`src/ipc.rs:503-507`):

```rust
    /// List of targetable panes (response to `ListPanesForSession`).
    /// Each entry is `(pane_id, current_cmd, window_name, pane_index, is_current_target)`.
    PaneList {
        panes: Vec<(String, String, String, usize, bool)>,
    },
```

**Three call sites consume it**: `src/cli/commands/slash.rs:153` (the listing)
and `:182` (the `/pane <n>` resolver), plus the producer
`src/daemon/server/handlers.rs:219`.

**`/panes` is already an alias for `/pane`** — `src/cli/commands/slash.rs:60`
reads `"/pane" | "/panes" => cmd_pane(ctx, rest).await`. So this phase does not
add a command; it upgrades what the **no-argument** branch renders. Keep the
alias.

**The renderer today** (`slash.rs:153-166`) is a flat numbered list:

```rust
                for (i, (id, cmd, window, idx, is_target)) in panes.iter().enumerate() {
                    let marker = if *is_target { "●" } else { " " };
                    body.push(format!("{marker} [{}] {id}  {window}:{idx}  {cmd}", i + 1));
                }
```

**The number is load-bearing and this is the trap of the phase.** `/pane <n>`
resolves with `panes.get(n.saturating_sub(1))` (`slash.rs:182`) — a **flat
index into the vector**. Once rows are grouped into window sections, a
per-section counter would renumber them and silently pin the wrong pane. The
displayed number must stay the pane's position in the vector, counted
**globally across all sections**. Task 3 pins this and mutation M1 proves it.

**The producer** (`handlers.rs`) already sorts by `(window_name, pane_index)`
— the grouping order this phase needs — filters to the home session, excludes
daemon windows with inline prefix literals, and liveness-probes each candidate
with `pane_exists`. All of that stays; the only change is what it puts in each
element.

**`PaneStatus`** (phase-02, `src/tmux/status.rs`) implements `Display`. Its
`summarize()` produces `<status> — <last line>`, which is the preview this
phase wants; read it before writing anything new.

**Tests today:** 1191 in the lib suite.

## Spec

### Task 1 — `PaneInfo` in `src/ipc.rs`

Replace the tuple with a named struct, declared next to the `Response` enum:

```rust
/// One row of `Response::PaneList` (M12 D7). A named struct rather than a
/// widening tuple — the tuple had already reached five positional fields.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PaneInfo {
    pub id: String,
    /// Window-relative pane index — the number `ctrl+a q` shows.
    pub idx: usize,
    pub window: String,
    /// Owning tmux session. Equal to the user's own session for every row this
    /// phase produces; carried because the cache is multi-session (D1) and the
    /// renderer must not assume otherwise.
    pub session: String,
    pub cmd: String,
    pub cwd: String,
    pub title: String,
    /// `PaneStatus` rendered through its `Display` impl.
    pub status: String,
    /// Seconds since the pane last produced output; `None` when unknown.
    pub activity_age_secs: Option<u64>,
    /// True when this pane is the session's pinned foreground target.
    pub is_target: bool,
    /// One-line preview of the pane's buffer, masked.
    pub preview: String,
}
```

and change the variant to `PaneList { panes: Vec<PaneInfo> }`, keeping its doc
comment accurate (drop the "Each entry is (…)" line — it describes the tuple).

`status` is a pre-rendered `String`, not `PaneStatus`, deliberately: it keeps
the wire type free of a dependency on an internal enum whose variants carry
data, and the client only ever displays it.

### Task 2 — Populate it in `src/daemon/server/handlers.rs`

In `handle_list_panes`, the `.map(|(id, s)| …)` that builds each candidate now
builds a `PaneInfo`. Everything else in that function — the home-session
filter, the daemon-window prefix filter, the `pane_exists` liveness probe, the
`sort_by_key` — **stays exactly as it is.**

- `status`: `s.status.to_string()`.
- `preview`: `crate::tmux::status::summarize(s.status, &s.buffer)`, then
  `crate::ai::filter::mask_sensitive` over the result. The preview crosses the
  IPC boundary to a terminal, so it is masked like every other pane content.
- `activity_age_secs`: `None` when `s.last_activity == 0`, else the difference
  from now in seconds, saturating (never a negative or wrapped value).
- `title`: `mask_sensitive(&s.pane_title)`, empty string when the title equals
  `current_cmd` (the existing convention — see `list_panes` in
  `executor/knowledge/pane.rs`).
- `cwd`: `s.current_path.clone()`.

The `sort_by_key` closure destructures the tuple today; it becomes
`panes_snapshot.sort_by_key(|p: &crate::ipc::PaneInfo| (p.window.clone(), p.idx));`.

### Task 3 — The renderer, extracted and pure

In `src/cli/commands/slash.rs`, add a **pure** function above `cmd_pane` — it
is what makes the inspector testable, and both mutation targets live in it:

```rust
/// Render the `/panes` inspector body: one section per window, rows numbered
/// globally so the number a user types into `/pane <n>` still indexes
/// `panes[n - 1]`.
///
/// Pure so the numbering and marker rules can be tested without a daemon.
fn render_pane_inspector(panes: &[crate::ipc::PaneInfo]) -> Vec<String> {
```

Rules, all of them load-bearing:

1. **Number globally.** Enumerate the slice once and use that index for every
   row, regardless of which section the row lands in:
   ```rust
       let number = i + 1; // global index — /pane <n> resolves panes[n - 1]
   ```
   Write that line exactly as given, trailing comment included — it is mutation
   M1's target. A per-section counter is the bug this rule exists to prevent.
2. **Group by window, in slice order.** Emit a `window '<name>' (<n> panes)`
   header whenever `p.window` differs from the previous row's. The producer
   already sorts by `(window, idx)`, so a single pass suffices — do **not**
   re-sort, or the numbering stops matching the resolver.
3. **Mark the pinned target** with `●`, others with a space:
   ```rust
       let marker = if p.is_target { "●" } else { " " }; // pinned target marker
   ```
   Exactly as given — mutation M2's target.
4. Each row carries, in this order: the marker, `[<number>]`, the pane id,
   `idx:<n>`, `cmd:<cmd>`, `status:<status>`, the activity age when known
   (`[idle 4m]` / `[active 12s]` — reuse the formatting `list_panes` already
   uses in `executor/knowledge/pane.rs`), and `cwd:<cwd>`.
5. The **preview** goes on its own continuation line beneath the row, indented,
   and is skipped when empty.
6. `session:<name>` appears on a row **only** when `p.session` differs from the
   first row's session — today that never fires, and it must not print a
   redundant label on every row.
7. Trailing: a blank line then `pin with: /pane <number|%id>`, as today.
8. On an empty slice return a single-element vec with the existing
   `"no targetable panes in this session"` text, so the caller's empty-case
   branch can go away.

Then `cmd_pane`'s no-argument branch becomes: call
`render_pane_inspector(&panes)` and `commit_panel("panes", &body, false)`.
**Leave the `/pane <n|%id>` resolver at `slash.rs:182` alone** except for the
tuple-to-struct change (`Some(p) => p.id.clone()`).

### Task 4 — Fix the remaining consumers

`src/cli/commands/stream.rs:616` and `src/cli/commands/ask.rs:214` name
`Response::PaneList { .. }` in match arms; they ignore the payload and need no
change beyond compiling. Check them, do not restructure them.

### Task 5 — Documentation

- `CLAUDE.md` § "Key files": the `src/cli/` row should mention the `/panes`
  inspector. **The tool-counts line must NOT change** — this phase adds no
  tool.
- If `CLAUDE.md` documents `Response::PaneList`'s shape anywhere, update it to
  say `Vec<PaneInfo>`. If it does not, add nothing — do not invent a new
  section.

### Task 6 — Tests

Write the four tests named in § Test plan, all pure calls to
`render_pane_inspector` with hand-built `PaneInfo` values. **No test in this
phase may reach tmux or a daemon.**

### Task 7 — Apply mutation M1 and capture both directions

Per `docs/dev/WORKFLOW.md` § "End-to-end verification", the edit is a **`patch`
tool call, not `sed -i`** — in-place shell edits are banned by your contract
and `bash` refuses them.

1. `patch` `src/cli/commands/slash.rs`:
   - `old_str`: `        let number = i + 1; // global index — /pane <n> resolves panes[n - 1]`
   - `new_str`: `        let number = 1; // global index — /pane <n> resolves panes[n - 1]`
2. Append the marker, the applied-check and the mutated run:
   ```bash
   echo "== M1 APPLIED ==" >> /tmp/e2e-07.txt
   echo -n "M1 mutated-lines-present=" >> /tmp/e2e-07.txt
   grep -c 'let number = 1; // global index' src/cli/commands/slash.rs >> /tmp/e2e-07.txt
   cargo test render_pane_inspector 2>&1 | grep -E '^test .*(ok|FAILED)$|^test result:|panicked at' | head -10 >> /tmp/e2e-07.txt
   echo "M1 exit=${PIPESTATUS[0]}" >> /tmp/e2e-07.txt
   ```
   The `grep -c` must print `1`; a `0` means the `patch` hit the wrong line and
   the pair proves nothing.
3. `patch` it back (swap `old_str`/`new_str`), append `== M1 RESTORED ==`, the
   same `grep -c` (now `0`), the restored run, and `M1 restored exit=`.

Adjust the indentation in `old_str` to match what you actually wrote — the
`patch` tool matches exactly, and the line's leading whitespace depends on how
deeply nested your loop is.

### Task 8 — Apply mutation M2 and capture both directions

Same procedure on the target marker:

- `old_str`: `        let marker = if p.is_target { "●" } else { " " }; // pinned target marker`
- `new_str`: `        let marker = if p.is_target { " " } else { "●" }; // pinned target marker`

with `grep -c 'if p.is_target { " " } else'`, the test filter
`render_pane_inspector`, and the `M2` labels.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim — it **appends** to the
same `/tmp/e2e-07.txt` Tasks 7 and 8 wrote, so run it **after** them.

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
- [ ] `grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md` prints `1`
      — unchanged. `cargo test --test doc_truth` passes.
- [ ] `grep -c 'Vec<(String, String, String, usize, bool)>' src/ipc.rs` prints
      `0` — the tuple is gone, not merely wrapped.
- [ ] `grep -c 'pub struct PaneInfo' src/ipc.rs` prints `1`.
- [ ] All four tests named in § Test plan pass.
- [ ] Mutation M1: `M1 mutated-lines-present=1`, the mutated
      `cargo test render_pane_inspector` reports `FAILED`, the restored count
      is `0` and the tests pass. Both directions in the transcript.
- [ ] Mutation M2: the same shape.
- [ ] The Update Log holds a new `### Update — <date> (end-to-end
      verification)` entry containing `/tmp/e2e-07.txt` byte for byte, and a
      `### Update — <date> (paste check)` entry reading `PASTE MATCH`.

## Test plan

All in `src/cli/commands/slash.rs`'s test module (add one if the file has
none), all pure:

- `render_pane_inspector_numbers_panes_globally_not_per_window` — three panes
  across **two** windows (say `main`/`main`/`edit`). Assert the rendered body
  contains `[1]`, `[2]` **and** `[3]`, and that `[3]` appears on the row whose
  pane id is the third element's. The point is that the third pane — the first
  in its window — is `[3]` and not `[1]`. **Mutation M1's target, and the
  reason the phase has a trap section.**
- `render_pane_inspector_marks_the_pinned_target` — two panes, the second with
  `is_target: true`. Assert the `●` appears on the second pane's row and not on
  the first's. **Mutation M2's target.**
- `render_pane_inspector_groups_by_window` — the same three-pane fixture
  produces exactly two `window '` header lines, `main` before `edit` (slice
  order), and the two `main` rows sit between the `main` header and the `edit`
  header.
- `render_pane_inspector_omits_empty_preview_and_unknown_age` — a pane with
  `preview: ""` and `activity_age_secs: None` renders neither a continuation
  line nor an age tag, while a sibling with both renders both.

## End-to-end verification

Run **verbatim** from the repo root, in `bash`, **without** `set -e`, and
**after** Tasks 7 and 8 — it appends to the file they created. Each command is
piped through `tail`/`grep` so the artifact stays small enough to paste whole.
`${PIPESTATUS[0]}` is read on the line immediately after each pipeline; do not
move those lines apart.

```bash
OUT=/tmp/e2e-07.txt

echo "== SURFACES ==" >> $OUT
echo -n "tuple gone from ipc.rs (want 0)=" >> $OUT
grep -c 'Vec<(String, String, String, usize, bool)>' src/ipc.rs >> $OUT 2>&1
echo -n "PaneInfo declared (want 1)=" >> $OUT
grep -c 'pub struct PaneInfo' src/ipc.rs >> $OUT 2>&1
echo -n "tool counts UNCHANGED at 36 (want 1)=" >> $OUT
grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md >> $OUT 2>&1
echo -n "/panes still aliased to /pane (want 1)=" >> $OUT
grep -c '"/pane" | "/panes"' src/cli/commands/slash.rs >> $OUT 2>&1

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
D=docs/dev/milestones/M12-tmux-integration/phase-07-pane-inspector-cli.md
START=$(grep -n '^### Update .*(end-to-end verification)' $D | tail -1 | cut -d: -f1)
tail -n +$START $D | awk '/^```/{n++; next} n==1' > /tmp/pasted-07.txt
diff /tmp/pasted-07.txt /tmp/e2e-07.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Expected readings: every `want` as stated; all four gate exits `0`; both
`M* mutated-lines-present=1` with a `FAILED` line for `render_pane_inspector`,
and both `M* restored` counts `0` with the tests passing; nothing between
`== TREE ==` and `porcelain exit=0`; and `PASTE MATCH`.

## Authorizations

None. No new dependencies; no `docs/architecture.md` changes.

## Out of scope

- **The D6 shared targetable-panes filter** — phase-08. `handle_list_panes`
  keeps its inline `de-*` prefix literals and its home-session filter exactly
  as they are; phase-08 rewrites all five sites at once and must pin
  session-before-panes lock ordering while doing it.
- **Showing foreign-session panes in `/panes`.** Not a D7 requirement, and it
  would be actively wrong here: this same response backs `/pane <n>` pinning,
  and a foreign-session pane is not a valid foreground target. `PaneInfo`
  carries `session` so the renderer never has to assume, not so this phase can
  widen the filter.
- **Changing `/pane <n|%id>`'s pinning behavior**, the `SetPane` request, or
  the resolver's flat-index semantics. D7 says pinning is unchanged, and Task 3
  rule 1 exists to keep it that way.
- **`tmux_control`** (phases 06a/06b) and anything in `src/daemon/executor/`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-08 17:00 (started)

**Executor:** headless executor

Implementing D7: replaced `Response::PaneList`'s opaque 5-tuple with named `PaneInfo` struct, populated it in `handle_list_panes`, extracted `render_pane_inspector` as a pure function with global numbering, updated all consumers (stream.rs, executor/mod.rs, ipc_tests.rs), and added 4 pure unit tests.

### Update — 2026-08-08 17:00 (end-to-end verification)

```
== M1 APPLIED ==
M1 mutated-lines-present=1
test cli::commands::slash::tests::render_pane_inspector_groups_by_window ... ok
test cli::commands::slash::tests::render_pane_inspector_marks_the_pinned_target ... FAILED
test cli::commands::slash::tests::render_pane_inspector_omits_empty_preview_and_unknown_age ... FAILED
test cli::commands::slash::tests::render_pane_inspector_numbers_panes_globally_not_per_window ... FAILED
thread 'cli::commands::slash::tests::render_pane_inspector_marks_the_pinned_target' (1807711) panicked at src/cli/commands/slash.rs:740:14:
thread 'cli::commands::slash::tests::render_pane_inspector_omits_empty_preview_and_unknown_age' (1807713) panicked at src/cli/commands/slash.rs:865:14:
thread 'cli::commands::slash::tests::render_pane_inspector_numbers_panes_globally_not_per_window' (1807712) panicked at src/cli/commands/slash.rs:688:9:
test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 1191 filtered out; finished in 0.00s
M1 exit=101
== M1 RESTORED ==
M1 restored-lines-present=0
test cli::commands::slash::tests::render_pane_inspector_omits_empty_preview_and_unknown_age ... ok
test cli::commands::slash::tests::render_pane_inspector_marks_the_pinned_target ... ok
test cli::commands::slash::tests::render_pane_inspector_numbers_panes_globally_not_per_window ... ok
test cli::commands::slash::tests::render_pane_inspector_groups_by_window ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1191 filtered out; finished in 0.00s
M1 restored exit=0
== M2 APPLIED ==
M2 mutated-lines-present=1
test cli::commands::slash::tests::render_pane_inspector_omits_empty_preview_and_unknown_age ... ok
test cli::commands::slash::tests::render_pane_inspector_groups_by_window ... ok
test cli::commands::slash::tests::render_pane_inspector_numbers_panes_globally_not_per_window ... ok
test cli::commands::slash::tests::render_pane_inspector_marks_the_pinned_target ... FAILED
thread 'cli::commands::slash::tests::render_pane_inspector_marks_the_pinned_target' (1809665) panicked at src/cli/commands/slash.rs:741:9:
test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 1191 filtered out; finished in 0.00s
M2 exit=101
== M2 RESTORED ==
M2 restored-lines-present=0
test cli::commands::slash::tests::render_pane_inspector_marks_the_pinned_target ... ok
test cli::commands::slash::tests::render_pane_inspector_omits_empty_preview_and_unknown_age ... ok
test cli::commands::slash::tests::render_pane_inspector_numbers_panes_globally_not_per_window ... ok
test cli::commands::slash::tests::render_pane_inspector_groups_by_window ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1191 filtered out; finished in 0.00s
M2 restored exit=0
== SURFACES ==
tuple gone from ipc.rs (want 0)=0
PaneInfo declared (want 1)=1
tool counts UNCHANGED at 36 (want 1)=1
/panes still aliased to /pane (want 1)=1
== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.34s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.06s
clippy exit=0
test result: ok. 1195 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.20s
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
 M docs/dev/milestones/M12-tmux-integration/phase-07-pane-inspector-cli.md
 M src/cli/commands/slash.rs
 M src/cli/commands/stream.rs
 M src/daemon/executor/mod.rs
 M src/daemon/server/handlers.rs
 M src/ipc.rs
 M src/ipc_tests.rs
porcelain exit=0
transcript line count=69
```

### Update — 2026-08-08 17:00 (paste check)

PASTE MATCH
