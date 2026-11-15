# Phase 10 — chat-panel-polish

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** in-progress
**Drafted:** 2026-11-15
**Depends on:** phase 09 (the tree at `bff5abc2`)
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Four cosmetic defects in `daemoneye chat`, found before the M37 live check:

1. **The input box is the wrong colour.** Its top and bottom rules are gray.
   The panels committed to chat history (the tool-call panels) draw their
   borders in bold palette red. The input box must use that same style.
2. **The input box has no corners.** History panels open `╭─` and close `─╮`
   on the top rule, and use `╰` … `╯` on the bottom. The input box rules are
   plain `─` from edge to edge. It must take the same corners. The side
   columns stay blank, so a multi-line copy picks up no `│`.
3. **The terminal panel's title says nothing.** The approval panel committed
   for a foreground `run_terminal_command` is titled
   `terminal · visible to you`. It must name where the command runs, as
   `user@host:sN`: the user the shell runs as, the host the shell runs on,
   and the DaemonEye shell id, for example `matt@pinky:s3`.
4. **Two status-bar glyphs change** in the emoji dialect. The info alert
   glyph `ℹ` becomes `⚠`, drawn in palette yellow, the colour the warning
   glyph already has. The cost glyph `💸` becomes `💰`. The symbols and ascii
   dialects are unchanged.

**PE decisions (2026-11-15):**
- **Info and warning share the yellow `⚠`.** In the compact form the two
  read the same (`⚠1`); the wide form's word (`info` / `warn`) tells them
  apart. Accepted.
- **The daemon picks the shell before the prompt and pins it.** The pick
  never creates a shell and never connects to a host. It reuses the owner's
  idle shell on the requested host, by the same rule `shell_for_owner` uses
  today. If no such shell exists, the label reads `new`
  (`matt@pinky:new`). The approved run then targets exactly the shell it
  picked: the picked id, or `"new"`. The label is never a guess the run can
  contradict.
- **A configured host's user and host come from its `ssh` target.** For
  `matt@web01.lan` the label is `matt@web01.lan:s4`. For a bare `db01` it is
  `db01:s4`, with no user and no `@`.

## Architecture references

- `src/cli/render_ratatui.rs`: `input_box_block` (used by the live, prompt
  and spinner regions) and `render_live_region`.
- `src/cli/render_ratatui_draw.rs`: `commit_panel_labeled`, which builds the
  history border style and the `╭─ … ─╮` rule.
- `src/cli/commands/stream_prompts.rs`: `tool_call_panel_label` and
  `prompt_tool_call_ratatui`. The file is allowlisted at 810 production
  lines.
- `src/cli/commands/stream_ask.rs`: the `Response::ToolCallPrompt` arm.
- `src/ipc.rs`: `Response::ToolCallPrompt`.
- `src/daemon/executor/dispatch_foreground.rs`: `dispatch` calls
  `prompt_and_await_approval`, then `run_approved` with an `ApprovedRun`
  whose `shell` reaches every run arm (local, sudo, interactive, named host).
- `src/daemon/executor/mod.rs`: `prompt_and_await_approval` (allowlisted at
  210 lines) and `ApprovalRequest`.
- `src/shell/session.rs`: `shell_for_owner`. Its candidate filter and
  live-probe walk are the reuse rule.
- `src/daemon/executor/shell_exec.rs`: `resolve_shell_target`. An explicit
  id that is busy, or on another host, is already refused before anything
  is written.
- `src/daemon/utils/host.rs`: `daemon_hostname`.
- `src/cli/status_glyphs.rs`: the `EMOJI` table (`alert_info`, `cost`).
  `src/cli/status_bar.rs`: `alert_parts`, whose severity colour closure
  gives rank 1 (info) no colour today.
- `src/shell/session.rs`: `host_entry_or_bare` (a host name to its
  `HostEntry`, whose `ssh` field is the target).

## Spec

These are numbered tasks, in execution order, and the task list is seeded
from this section. Mark a task `active` when you start it and `done` when
its `cargo build` is green. **Step 1 is a write.** Make the first edit
within 15 turns.

Symbol map: input box → `input_box_block` in `render_ratatui.rs`; history
border → `commit_panel_labeled` in `render_ratatui_draw.rs`; prompt frame →
`Response::ToolCallPrompt` in `ipc.rs`; the shell pick → `shell_for_owner`
in `shell/session.rs`; the prompt send → `prompt_and_await_approval` in
`executor/mod.rs`; the panel title → `tool_call_panel_label` in
`stream_prompts.rs`.

1. **One border style.** Add a function that returns the history panel's
   border style (palette red, bold). Have `commit_panel_labeled` call it, and
   have the input box's `Block` take it in place of `Color::Gray`. All three
   regions that use `input_box_block` change with it.
2. **Corners on the input box.** The input box's top rule renders
   `╭` + `─`… + `╮` and its bottom rule renders `╰` + `─`… + `╯`, across the
   box's full width. The content rows' first and last columns stay blank.
   ratatui draws no corners when a `Block` has only `TOP | BOTTOM` borders.
   After the paragraph renders, overwrite the four corner cells in the frame
   buffer, in the border style. Do this in one place shared by the three
   regions.
3. **The frame field.** Add `shell_label: Option<String>` to
   `Response::ToolCallPrompt`, with `#[serde(default)]` so an older daemon's
   frame still deserialises. Every construction site sets it; non-terminal
   prompts set `None`.
4. **The label pieces** go in a new module,
   `src/daemon/executor/shell_label.rs`. They are pure, except for the
   username lookup:
   - `ssh_user_host(target) -> (Option<String>, String)`. It splits at the
     last `@`, strips a leading `ssh://` and a trailing `:port`.
   - `local_username()`: the passwd name for the daemon's effective uid,
     through `libc` (already a dependency; no new crate or feature). If the
     lookup fails, it falls back to `$USER`, then to the numeric uid.
   - `shell_where_label(user, host, shell) -> String`. It gives
     `user@host:sN`, `host:sN` when there is no user, and `…:new` when no
     shell was picked.
5. **The pick.** Factor the reuse rule out of `shell_for_owner` into a
   function that returns the shell it would reuse, or `None`, and never
   spawns. `shell_for_owner` calls it, so there is exactly one reuse rule.
   In `dispatch_foreground::dispatch`, before the prompt, resolve the pick
   for a chat session:
   - an explicit `shell` id names itself;
   - `"new"` gives `None`;
   - otherwise use the reuse rule on the requested host.

   Build the label from that pick:
   - **local:** `local_username()` and `daemon_hostname()`;
   - **named host:** `ssh_user_host` of its entry's `ssh`;
   - **unconfigured name:** the bare name as the host, with no user.

   Pass the label through `ApprovalRequest` into the frame.
   `prompt_and_await_approval` must not grow (its allowlist count is 210).
   Move lines out of it to make room, for example by building the frame in a
   helper.
6. **The pin.** The `ApprovedRun` carries the picked shell as its `shell`:
   the picked id, or `"new"` when the pick was `None`. Every run arm then
   lands in the shell the label named. A pinned shell that went busy while
   the prompt was open is refused, as an explicit busy id is today. That
   refusal is accepted.
7. **The client title.** `tool_call_panel_label` takes the frame's
   `shell_label`:
   - foreground with `Some(label)`: the label;
   - foreground with `None` (an older daemon): today's
     `terminal · visible to you`;
   - background: unchanged, `daemon · runs silently`.

   `stream_prompts.rs` may not grow past its allowlisted 810 lines. Move the
   panel-label helpers to `stream.rs` if you need the room.
8. **The glyphs.** In the `EMOJI` table set `alert_info` to `⚠` and `cost`
   to `💰`. In `alert_parts`, the info rank takes palette yellow, as the
   warning rank does. Update every test that pins `ℹ` or `💸`; delete none.
   `glyph_sample`'s output is allowed to repeat `⚠`; a uniqueness test over
   the sample, if one exists, is loosened only for that pair.
9. **Docs.** Add a `CLAUDE.md` key-files row for `shell_label.rs`. In
   `README.md` and the M37 README's decision 3 table and examples, replace
   `ℹ` with `⚠` and `💸` with `💰`.
10. **Evidence.** Run the mutation check and the § End-to-end block. Paste
   both into one Update Log entry.

## Constraints

- **The pick never creates a shell and never connects to a host.** A denied
  prompt leaves the shell registry as it found it. No ssh process starts
  before approval, including for an unconfigured `new_host` name.
- **Ghost runs are unchanged.** A ghost does not prompt, so nothing is
  picked or pinned. The background arm (`wait: false`) is also unchanged.
- **Session approval is unchanged.** An auto-approved call still commits its
  panel, now with the label, and runs in the pinned shell.
- **One reuse rule.** A copy of `shell_for_owner`'s filter in the pick, or
  in a test, is a defect.
- **The `ssh_user_host` negatives:** `db01` has no user, so the label has no
  `@`. `ssh://matt@web01.lan:2222` gives host `web01.lan`; neither the port
  nor the scheme appears.
- **Hygiene:** no file passes 800 production lines, no function passes 150
  lines, and no allowlist count grows. `stream_prompts.rs` stays at or under
  810, and `prompt_and_await_approval` at or under 210.
- **Tests are hermetic.** No real `HOME`, no ssh, no tmux. The daemon tests
  use the in-process shell host the existing `dispatch_foreground` tests
  already spawn.

## Test plan

Renderer (`TestBackend`, in the existing `render_ratatui` test modules):

- `the_input_box_has_rounded_corners`: after `draw`, the box's top row is
  `╭`, `─`…, `╮`, and its bottom row is `╰`, `─`…, `╯`.
- `the_input_box_border_matches_the_history_panel`: every input-box border
  cell has the same style as a border cell from `commit_panel` on the same
  palette.
- `the_input_box_sides_stay_blank` (negative): each content row's first and
  last cells are spaces.
- `the_prompt_and_spinner_boxes_have_the_same_corners`: `draw_prompt` and
  `draw_spinner` render the same four corners.

Label (in `shell_label.rs`):

- `ssh_user_host_splits_the_target`: covers `matt@web01.lan`, `db01` and
  `ssh://matt@web01.lan:2222`.
- `shell_where_label_formats_each_case`: `matt@pinky:s3`, `db01:s4`,
  `matt@pinky:new`.
- `local_username_is_a_plain_name`: the result is non-empty and contains no
  `@`, `:` or whitespace.

Status bar (in `src/cli/status_bar_tests.rs`, calling `status_bar_line`):

- `an_info_alert_shows_a_yellow_warning_glyph`: one firing `info` alert in
  the emoji dialect renders `⚠`, styled palette yellow, and the wide form
  reads `⚠ info <name>`.
- `the_cost_glyph_is_a_money_bag`: a session with spend renders `💰` and no
  `💸`, in both forms.
- Negative: `the_symbols_and_ascii_alert_glyphs_are_unchanged`: `●` / `i`
  for info, `$` for cost.

Daemon (in `dispatch_foreground.rs`'s tests):

- `the_prompt_names_the_shell_the_run_lands_in`: the session already has
  an idle shell. The `ToolCallPrompt` label ends in `:<that id>`, and the
  approved run's result names the same shell.
- `a_session_without_a_shell_is_labelled_new`: the label ends in `:new`.
  The call is denied, and the registry holds no shell for the session
  afterwards.
- `an_explicit_shell_id_labels_itself`.

Client (in `stream.rs`'s `tool_call_panel_tests`): update
`the_local_command_panel_is_unchanged` and add
`the_terminal_panel_names_user_host_and_shell`. It covers the `Some`, the
`None` fallback and the background cases from task 7.

## Acceptance criteria

- [ ] Every test named in the test plan exists and passes:
      `grep -rlw "fn <name>" src` prints a file for each name. Today none
      of them do.
- [ ] `grep -n 'Color::Gray' src/cli/render_ratatui.rs` no longer matches
      inside `input_box_block`.
- [ ] `grep -rn 'ℹ\|💸' src README.md CLAUDE.md` prints nothing.
- [ ] `grep -rn '"terminal · visible to you"' src` matches only the
      older-daemon fallback and its test.
- [ ] Mutation check, pasted with its `test result:` lines. Make the pick
      always return `None`. Confirm `the_prompt_names_the_shell_the_run_lands_in`
      fails, then revert. Separately, drop the corner overwrite. Confirm
      `the_input_box_has_rounded_corners` fails, then revert.
- [ ] fmt, build (0 warnings) and clippy are green. `cargo test` (under
      `timeout 600`) reports `failed=0` and passed greater than 3008. The
      hygiene, `doc_truth` and `bug_tracker` tests pass.

## Pre-flight

The baseline at drafting (2026-11-15, `bff5abc2`) is `cargo test`
passed=3008, failed=0, from phase 09's review. Run every `cargo test` under
`timeout 600`.

## End-to-end verification

```sh
timeout 600 cargo test --lib input_box 2>&1 | grep -E '^test result'
timeout 600 cargo test --lib shell_label 2>&1 | grep -E '^test result'
timeout 600 cargo test --lib dispatch_foreground 2>&1 | grep -E '^test result'
timeout 600 cargo test --lib tool_call_panel 2>&1 | grep -E '^test result'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- New file `src/daemon/executor/shell_label.rs`, and its `mod` line in
  `src/daemon/executor/mod.rs`.
- Edits to `src/cli/render_ratatui.rs`, `render_ratatui_draw.rs` and their
  test modules.
- Edits to `src/cli/commands/stream_prompts.rs`, `stream.rs`,
  `stream_ask.rs` and `slash.rs`.
- Edits to `src/ipc.rs`, `src/daemon/executor/mod.rs`,
  `dispatch_foreground.rs` and `src/shell/session.rs`.
- Edits to `src/cli/status_glyphs.rs`, `status_bar.rs`, `status_bar_tests.rs`
  and any test that pins `ℹ` or `💸`.
- Edits to any test that constructs `ToolCallPrompt` (including
  `tests/integration.rs`), to `CLAUDE.md`, `README.md` and the M37 README.

## Out of scope

- The background panel's `daemon · runs silently` title, and the sandbox
  route.
- The approval, credential and picker dialogs (`alert_panel`, already red
  and rounded).
- `/shell`, `top`, the rest of the status bar, and showing the user as `root` for a
  `sudo` command (the body already shows `sudo`).
- Probing a remote host's real user or hostname. That would need a
  connection before approval.

## Update Log

### Update — 2026-11-15 13:00 (started)

Executor (headless run) starting phase 10. Status flipped `todo → in-progress` here and in the milestone README. Beginning with task 1 (one border style); edits to follow in spec order.

### Update — 2026-11-15 14:37 (end-to-end verification)

Mutation checks and the § End-to-end block, pasted from the runs.

**Mutation check 1 — the pick always returns `None`.** `pick_shell_for_run`
replaced with `let _ = (host, shell_arg, session_id); None`. Then
`cargo test --lib the_prompt_names_the_shell_the_run_lands_in`:

```
test daemon::executor::dispatch_foreground::approved_path_tests::the_prompt_names_the_shell_the_run_lands_in ... FAILED

thread 'daemon::executor::dispatch_foreground::approved_path_tests::the_prompt_names_the_shell_the_run_lands_in' panicked at src/daemon/executor/dispatch_foreground.rs:949:9:
the label ends in the shell the run lands in: "matt@pinky:new" vs s0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2934 filtered out; finished in 0.04s
```

Reverted; the test is green again (`test result: ok. 1 passed`).

**Mutation check 2 — the corner overwrite dropped.** `draw_input_box_corners`
made to paint `─` at the four corner cells instead of `╭`/`╮`/`╰`/`╯`. Then
`cargo test --lib the_input_box_has_rounded_corners`:

```
test cli::render_ratatui::layout_tests::the_input_box_has_rounded_corners ... FAILED

thread 'cli::render_ratatui::layout_tests::the_input_box_has_rounded_corners' panicked at src/cli/render_ratatui_layout_tests.rs:1018:5:
the box's top rule opens ╭ and closes ╮: "────────────────────────────────────────────────────────────"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2934 filtered out; finished in 0.00s
```

Reverted; the test is green again (`test result: ok. 1 passed`).

**§ End-to-end block:**

```
$ timeout 600 cargo test --lib input_box 2>&1 | grep -E '^test result'
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2929 filtered out; finished in 0.00s

$ timeout 600 cargo test --lib shell_label 2>&1 | grep -E '^test result'
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2929 filtered out; finished in 0.00s

$ timeout 600 cargo test --lib dispatch_foreground 2>&1 | grep -E '^test result'
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 2924 filtered out; finished in 16.10s

$ timeout 600 cargo test --lib tool_call_panel 2>&1 | grep -E '^test result'
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2930 filtered out; finished in 0.00s

$ cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
build_warnings=0

$ timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=3025 failed=0
```

Hygiene, `doc_truth` and `bug_tracker` all green (`--test hygiene` 16 passed,
`--test doc_truth` 19 passed, `--test bug_tracker` 6 passed). The two mutation
checks above ran against the real tests and were reverted; the tree that is
committed is the reverted one.
