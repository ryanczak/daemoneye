# Phase 05: Block Copy

**Milestone:** M17 — Transcript View
**Status:** review
**Depends on:** phase-04 (search, `done`)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Press `y` in the viewer to copy the focused block into a tmux buffer, so the
output the inline panel elided can leave the transcript and be pasted anywhere.
Copy the block's **real content** — a collapsed block copies in full, and none
of the viewer's own decoration comes with it.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"Clipboard" is the design of record for
  why this is `tmux load-buffer` and not OSC 52.
- `src/cli/viewer.rs` — the viewer this extends; 1272 lines.
- `src/cli/transcript.rs` — the `Block` enum being copied.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**`ViewerAction` has 20 variants** (`src/cli/viewer.rs`), decoded by the pure
`key_action(key, searching)`. Its head establishes the shape a new arm must
follow:

```rust
pub fn key_action(key: &crate::cli::input::Key, searching: bool) -> ViewerAction {
    match (searching, key) {
        (true, crate::cli::input::Key::Char('\x1b')) => ViewerAction::SearchCancel,
        (true, crate::cli::input::Key::Enter) => ViewerAction::SearchCommit,
        (true, crate::cli::input::Key::Backspace) => ViewerAction::SearchBackspace,
        (true, crate::cli::input::Key::Char(c)) if !c.is_control() => ViewerAction::SearchType(*c),
```

The `(true, Char(c)) if !c.is_control()` arm sits **above** every command arm,
so a new `(false, Char('y'))` arm is automatically inert while searching. Keep
that ordering.

**The blocks being copied** (`src/cli/transcript.rs`):

```rust
pub enum Block {
    UserTurn { label: String, text: String },
    Assistant { text: String },
    ToolPanel { tool: String, summary: String, label: Option<String> },
    Output { tool_call_id: String, full: String, shown: usize },
    System { text: String },
}
```

**The status line** is assembled in `render_transcript` from
`format!("transcript — {shown_from}-{shown_to} of {total} lines")`, then
optionally prefixed with the eviction note and suffixed with the search counter
before the key hints are appended.

### Three gotchas, each verified against the tree

1. **`crate::tmux::bounded_output` cannot be used here.** It pipes *stdout* and
   *stderr* only (`src/tmux/mod.rs:67-75`) and never provides a stdin handle,
   so a `load-buffer -` invocation through it would hand tmux an empty buffer.
   This phase writes its own small spawner — the exact shape is given in task 2.
2. **Copy from the `Block`, not from the rendered `ViewRow`s.** The rows carry
   the viewer's decoration (the `▾ `/`▸ ` marker, the `output (N lines)` header,
   the `[collapsed, N lines]` suffix) and a collapsed block has no body rows at
   all. Deriving the text from `Block` is what makes "collapsed copies in full"
   true, and it is pinned as a test.
3. **`y` must type into the query while searching.** That falls out of the arm
   ordering in gotcha-free fashion *if* the new arm is `(false, …)` — but it is
   asserted explicitly in the criteria, because M17's record is that the
   untested half of a mode-sensitive behaviour is the half that breaks.

## Spec

### Task 1 — The pure copy text

In `src/cli/viewer.rs`, add:

```rust
/// The text a block yields when copied — its real content, with none of the
/// viewer's decoration and independent of whether it is collapsed.
pub fn copy_text(block: &crate::cli::transcript::Block) -> String
```

Pinned per variant:

- `UserTurn { text, .. }` → `text` verbatim (the `label` is not copied).
- `Assistant { text }` → `text` verbatim.
- `System { text }` → `text` verbatim, **without** the `⚙ ` prefix the viewer
  renders.
- `Output { full, .. }` → `full` verbatim — every line, never `shown`-limited.
- `ToolPanel { tool, summary, label }` → `tool`, then a newline, then `summary`;
  when `label` is `Some(l)`, the first line is `{tool} — {l}`. This is the one
  variant that is a composition rather than a field; pin it exactly so the test
  can assert a literal.

No trailing newline is added or removed — what the block holds is what comes
out.

Write the `Output` arm **exactly** as below — task 6's mutation targets this
line verbatim, so an equivalent-but-differently-written arm breaks the pair:

```rust
        crate::cli::transcript::Block::Output { full, .. } => full.clone(),
```

### Task 2 — The tmux hand-off

Also in `src/cli/viewer.rs`:

```rust
/// Load `text` into a tmux buffer, and into the system clipboard where the
/// terminal supports it (`-w` uses OSC 52; tmux >= 3.2).
fn copy_to_tmux_buffer(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("tmux")
        .args(["load-buffer", "-w", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("tmux load-buffer: no stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("tmux load-buffer exited with {status}");
    }
    Ok(())
}
```

Write it as given. The `stdin` handle **must be dropped before `wait()`** —
taking it out of the child and letting it fall out of scope at the end of the
statement is what closes the pipe; holding it across `wait()` deadlocks.

This function shells out, so it gets **no unit test** — task 5's tests cover
`copy_text`, and the E2E block exercises the tmux call for real.

### Task 3 — Wire the action

- Add `Copy` to `ViewerAction`.
- Add `(false, crate::cli::input::Key::Char('y')) => ViewerAction::Copy` to
  `key_action`, **below** the existing `(true, Char(c))` arm.
- Handle `ViewerAction::Copy` in `viewer_loop`: take the focused block from
  `transcript.blocks()`, run `copy_text`, hand it to `copy_to_tmux_buffer`, and
  record the outcome as a status note (task 4). An empty transcript copies
  nothing and reports nothing.

### Task 4 — Report the outcome

Add a `note: Option<String>` to the viewer's state and a `note: Option<&str>`
parameter to `render_transcript` (after `search`). When present it is appended
to the status line as ` · {note}`.

- On success: `copied {n} lines to tmux buffer` where `n` is
  `copy_text(...).lines().count()`.
- On failure: `copy failed: {e}` — the error is **shown, never swallowed**. Do
  not `let _ =` the result.
- The note is cleared on the next keypress that produces any action other than
  `Ignore`, so it does not linger.

### Task 5 — Tests

Write the tests named in § Test plan. All are pure — `copy_text` and
`key_action` need no terminal and no tmux.

### Task 6 — Mutation M1: apply

Use the `patch` tool on `src/cli/viewer.rs` to make a collapsed block copy only
what is on screen.

- `old_str`: `        crate::cli::transcript::Block::Output { full, .. } => full.clone(),`
- `new_str`: `        crate::cli::transcript::Block::Output { full, shown, .. } => full.lines().take(*shown).collect::<Vec<_>>().join("\n"),`

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-05.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'take(\*shown)' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail**. A green run means the test is vacuous; stop and file a
blocker.

### Task 7 — Mutation M1: restore

`patch` the arm back, then:

```sh
A=/tmp/e2e-05.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'take(\*shown)' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 6 and `0` after task 7. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-05.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 9 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-05-block-copy.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

Every criterion asserts an observed value or count.

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] Test `copy_text_copies_full_output_not_the_elided_view` passes — a
      300-line `Output` with `shown: 9` yields **exactly 300** lines, and the
      string equals `full`.
- [ ] Test `copy_text_of_collapsed_block_is_unchanged` passes — `copy_text` is
      a function of the `Block` alone, so the same block collapsed or expanded
      yields byte-identical text (assert equality against the expanded case,
      with the block present in a collapsed set).
- [ ] Test `copy_text_omits_viewer_decoration` passes — for a `System` block
      the result does **not** start with `⚙`, and for a `UserTurn` the result
      does **not** contain the label.
- [ ] Test `copy_text_tool_panel_composes_header_and_summary` passes — asserts
      the exact literal for both the `Some(label)` and `None` cases.
- [ ] Test `key_action_y_copies_only_when_not_searching` passes —
      `key_action(&Key::Char('y'), false) == ViewerAction::Copy` **and**
      `key_action(&Key::Char('y'), true) == ViewerAction::SearchType('y')`.
- [ ] `grep -c "let _ = copy_to_tmux_buffer" src/cli/viewer.rs` prints `0` —
      the copy result is surfaced, never discarded.
- [ ] The E2E block's `== TMUX ROUND TRIP ==` section shows the exact three
      lines that were loaded coming back out of `tmux show-buffer`.
- [ ] `/tmp/e2e-05.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/viewer.rs` (`#[cfg(test)] mod tests`):

- `copy_text_copies_full_output_not_the_elided_view` — 300 lines, `shown: 9`;
  exactly 300 lines out, string equality with `full`.
- `copy_text_of_collapsed_block_is_unchanged` — same block, collapsed vs
  expanded layout; `copy_text` output byte-identical.
- `copy_text_omits_viewer_decoration` — `System` result does not start with
  `⚙`; `UserTurn` result does not contain its label.
- `copy_text_tool_panel_composes_header_and_summary` — exact literals for
  `Some("2.1s")` → `"cargo build — 2.1s\ncompiling…"` and for `None` →
  `"cargo build\ncompiling…"`.
- `key_action_y_copies_only_when_not_searching` — both modes, exact actions.

`copy_to_tmux_buffer` is deliberately untested at unit level — it spawns a
process. The E2E block covers it against the real `tmux`.

## End-to-end verification

Unlike the earlier viewer phases, this one **does** have a headlessly
verifiable real artifact: the tmux buffer. The block below loads a known string
through the same `tmux load-buffer -w -` invocation the code uses and reads it
back with `tmux show-buffer`, so the hand-off is proven against the real binary
rather than only the unit tests. (This shell round-trip proves the *mechanism*;
that the viewer's `y` key reaches it is the live check at milestone close.)

Tasks 6 and 7 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-05.txt` here.

```sh
A=/tmp/e2e-05.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== VIEWER UNITS ==" >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -30 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== TMUX VERSION ==" >> "$A"
tmux -V >> "$A" 2>&1
echo "== TMUX ROUND TRIP ==" >> "$A"
printf 'alpha\nbeta\ngamma\n' | tmux load-buffer -w -
echo "load exit=$?" >> "$A"
tmux show-buffer >> "$A" 2>&1
echo "show exit=$?" >> "$A"
echo "== RESULT NOT DISCARDED ==" >> "$A"
grep -c "let _ = copy_to_tmux_buffer" src/cli/viewer.rs >> "$A"
echo "== PHASE-02 CONTRACT STILL HOLDS ==" >> "$A"
grep -c "disarm" src/cli/viewer.rs >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
```

## Authorizations

- [ ] May edit `src/cli/viewer.rs`.

No new dependencies — `anyhow` and `std::process` are already in use.
`docs/architecture.md` is **not** authorized, and neither is
`src/cli/input/tty.rs` (no new key parsing: `y` is already a `Key::Char`).

## Out of scope

- **Rehydration** (phase-06), **mouse** (phase-07).
- **Free-form drag selection.** Block-level copy is the shipped affordance;
  the design doc's §"Clipboard" records why.
- **OSC 52 by hand.** `-w` asks tmux to do it. Do not emit escape sequences
  directly, and do not add a fallback path for tmux < 3.2 — the target is 3.7b
  and the E2E block records the version it ran against.
- **Copying anything other than the focused block** — no "copy all", no
  multi-block selection.
- **Undoing anything phases 02–04 established.** `AltScreenGuard` keeps its
  unconditional `Drop`, `viewer.rs` gains no `disarm` and no `try_restore` /
  `disable_raw_mode` / `.restore()`, and `key_action`'s search-mode precedence
  is unchanged. The E2E block re-checks the guard contract.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-19 16:40 (started)

**Executor:** DeepSeek V4 Flash 0731

- Flipped status `todo → in-progress`; milestone README row updated to match.
- Implementing `copy_text`, `copy_to_tmux_buffer`, `ViewerAction::Copy`,
  status-line note, and the five pinned tests in `src/cli/viewer.rs`.

### Update — 2026-08-19 16:55 (end-to-end verification)

The artifact below is `/tmp/e2e-05.txt` pasted verbatim (this run re-took
the mutation pair and the full gate sequence in one clean contiguous capture —
I discarded an earlier two-piece capture taken while iterating on a clippy
diagnostic in one of the new tests). It covers M1 APPLIED (failing,
`grep -c take(*shown)` = 1) and M1 RESTORED (passing, count 0), the
fmt/clippy/test gates (all exit 0), the viewer unit tests (33 pass including
the five pinned ones), the real-tmux round trip (3.7b; the exact three lines
come back out of `tmux show-buffer`), the not-discarded copy result, and the
phase-02 guard contract re-check.

```text
== M1 APPLIED ==
1
assertion `left == right` failed
  left: 9
 right: 300

---- cli::viewer::tests::copy_text_of_collapsed_block_is_unchanged stdout ----

thread 'cli::viewer::tests::copy_text_of_collapsed_block_is_unchanged' (2343541) panicked at src/cli/viewer.rs:1375:9:
assertion `left == right` failed
  left: "alpha\nbeta\ngamma"
 right: "alpha\nbeta\ngamma\n"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    cli::viewer::tests::copy_text_copies_full_output_not_the_elided_view
    cli::viewer::tests::copy_text_of_collapsed_block_is_unchanged

test result: FAILED. 31 passed; 2 failed; 0 ignored; 0 measured; 1334 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
exit=101
== M1 RESTORED ==
0
test cli::viewer::tests::key_action_commands_apply_when_not_searching ... ok
test cli::viewer::tests::key_action_escape_cancels_search_but_quits_otherwise ... ok
test cli::viewer::tests::key_action_typing_wins_over_commands_while_searching ... ok
test cli::viewer::tests::key_action_y_copies_only_when_not_searching ... ok
test cli::viewer::tests::layout_blocks_empty_transcript_is_empty ... ok
test cli::viewer::tests::layout_blocks_separates_blocks_with_one_blank ... ok
test cli::viewer::tests::next_match_wraps ... ok
test cli::viewer::tests::prev_match_wraps ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::scroll_to_row_only_moves_when_offscreen ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::render_transcript_shows_match_counter ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok

test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 1334 filtered out; finished in 0.00s

exit=0
== GATES ==
fmt exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.12s
clippy exit=0

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s

     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)

running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

test exit=0
== VIEWER UNITS ==
test cli::viewer::tests::find_matches_empty_query_matches_nothing ... ok
test cli::viewer::tests::copy_text_of_collapsed_block_is_unchanged ... ok
test cli::viewer::tests::copy_text_copies_full_output_not_the_elided_view ... ok
test cli::viewer::tests::expanded_layout_is_unchanged_by_the_new_path ... ok
test cli::viewer::tests::collapse_toggle_is_involutive ... ok
test cli::viewer::tests::copy_text_tool_panel_composes_header_and_summary ... ok
test cli::viewer::tests::focus_next_wraps_at_last_block ... ok
test cli::viewer::tests::focus_prev_wraps_at_first ... ok
test cli::viewer::tests::key_action_commands_apply_when_not_searching ... ok
test cli::viewer::tests::key_action_escape_cancels_search_but_quits_otherwise ... ok
test cli::viewer::tests::find_matches_is_case_insensitive ... ok
test cli::viewer::tests::key_action_typing_wins_over_commands_while_searching ... ok
test cli::viewer::tests::key_action_y_copies_only_when_not_searching ... ok
test cli::viewer::tests::find_matches_skips_collapsed_block_bodies ... ok
test cli::viewer::tests::layout_blocks_empty_transcript_is_empty ... ok
test cli::viewer::tests::next_match_wraps ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::layout_blocks_separates_blocks_with_one_blank ... ok
test cli::viewer::tests::prev_match_wraps ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::scroll_to_row_only_moves_when_offscreen ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_shows_match_counter ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok

test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 1334 filtered out; finished in 0.00s

units exit=0
== TMUX VERSION ==
tmux 3.7b
== TMUX ROUND TRIP ==
load exit=0
alpha
beta
gamma
show exit=0
== RESULT NOT DISCARDED ==
0
== PHASE-02 CONTRACT STILL HOLDS ==
0
teardown grep exit=1  (1 = none found, which is the pass)
```

PASTE MATCH

### Update — ts=1787183410892 (complete, server-authored)

**Summary:** All tasks are complete, all gates green, the tree is clean, and both commits are in.

## Summary

Implemented block copy for the transcript viewer in `src/cli/viewer.rs`: a pure `copy_text(&Block)` that yields the block's real content (collapsed copies in full, no viewer decoration — `System`'s `⚙` prefix and `UserTurn`'s label excluded), a `copy_to_tmux_buffer` spawner using `tmux load-buffer -w -` with the stdin handle dropped before `wait()` (exactly as spec'd, since `bounded_output` can't provide stdin), a `ViewerAction::Copy` bound to `y` only when not searching, and a status-line note (`copied N lines to tmux buffer` / `copy failed: {e}`) that's surfaced — never `let _ =`-discarded — and cleared on the next non-`Ignore` keypress. The five pinned tests pass; the earlier capture contaminated and then repaired, and the mutation pair (M1) plus full gates were re-taken in one clean contiguous run: M1 APPLIED failed with `grep -c take(*shown)` = 1, M1 RESTORED passed with count 0, and the real-tmux round trip (tmux 3.7b) returned the exact three loaded lines from `show-buffer`.

**Deviations / notes for review:** none from the spec — every pinned literal (the `Output` arm, the `copy_to_tmux_buffer` body, the five test names) landed verbatim, PASTE MATCH confirmed. One small adaptation: my new test triggered a pre-existing-style clippy diagnostic (`cloned_ref_to_slice_refs`), fixed with `std::slice::from_ref(&block)`; the E2E artifact records the fix and the re-verified gates (fmt/clippy/test all exit 0). `git status` is clean; phase status is left `in-progress` for the server's completion bookkeeping.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.93s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nored; 0 measured; 0 filtered out; finished in 3.89s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M17-transcript-view/README.md` — +1 -1
- `docs/dev/milestones/M17-transcript-view/phase-05-block-copy.md` — +148 -1
- `src/cli/viewer.rs` — +149 -1

**Commit:** 71c5aaec2e6e56e46c9ebd0b6d1e2c0b3ab44f0e

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
