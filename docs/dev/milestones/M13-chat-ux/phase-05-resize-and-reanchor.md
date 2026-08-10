# Phase 05: Mid-stream resize/focus re-anchoring; legacy dead-code deletion

**Milestone:** M13 — Chat UX Polish
**Status:** in-progress
**Depends on:** phase-04
**Estimated diff:** ~270 lines (net ≈ +150 −120)
**Tags:** language=rust, kind=bugfix, size=m

## Goal

A tmux window switch or pane resize during an in-flight streamed turn leaves
the chat UI mangled — the input dialog can end up at the top of the visible
terminal with the history unreachable below. The streaming loop simply never
observes SIGWINCH or focus events; only the idle input loop does. This phase
gives the streaming select both signals and re-anchors the inline viewport,
makes the idle-loop resize arm re-anchor too, and — the milestone's closing
task — deletes the six dead legacy printers from `src/cli/render.rs`.

## Architecture references

Read before starting:

- `docs/dev/milestones/M13-chat-ux/README.md` § "Derived code facts" issue 6
  and § "Legacy-renderer delta audit" (the deletion list).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(All line numbers and grep counts verified 2026-08-10 against the
post-phase-04 tree.)

- **`select_stream`** (`src/cli/commands/stream.rs:692`) selects over exactly:
  `read_key(stdin)` (keys feed `interrupt_state.feed(&key)` — every non-
  interrupt key, including `Key::FocusGained`, is swallowed as
  `InterruptAction::Ignore`), `recv_line(rx, buf)`, an optional overall
  timeout, and the 80 ms tick. **No SIGWINCH arm, no focus handling.** The
  body is two near-identical `tokio::select!` blocks (with/without the
  timeout future) — any per-key logic added there lands in **both** branches.
- `StreamOutcome` (`stream.rs:22`) has variants `Msg`, `Tick`, `Warn`,
  `Interrupted`, `Error`. The caller's outcome match is at `:190-243`; `Tick`
  redraws the spinner and `continue`s — the shape a re-anchor outcome should
  follow.
- `select_stream` has two existing tests (`:1356`
  `select_stream_first_interrupt_press_warns`, `:1384`
  `select_stream_delivers_a_full_daemon_message`) that build a fake stdin
  from a pipe and write escape bytes into it — the pattern for a focus-event
  test (`ESC [ I` = focus gained, decoded to `Key::FocusGained` by
  `input/tty.rs`).
- **Idle-loop SIGWINCH arm** (`src/cli/commands/chat.rs:625-631`) re-queries
  `terminal_width()` and calls `renderer.draw(...)` but never `reanchor()`,
  so a resize that moved the inline viewport leaves it wherever ratatui's
  autoresize put it. `renderer.reanchor()` currently has exactly **1** call
  site in `chat.rs` (the `Key::FocusGained` arm).
- `reanchor` (`src/cli/render_ratatui.rs`, near `input_content_width`) is a
  same-size `Terminal::resize` that re-queries the backend size — it already
  handles both "same size, wrong origin" (window switch) and "new size"
  (resize), so it is the one call both signals need.
- **Dead legacy printers** in `src/cli/render.rs` (234 lines; zero callers
  outside the file — verified by grep): `print_tool_panel` (`:6`),
  `local_user_host` (`:38`), `print_tool_started` (`:53`),
  `print_tool_finished` (`:66`), `print_user_query` (`:90`),
  `terminal_height` (`:204`). **Still live and kept:** `wrap_line_hard`
  (called by `commit_panel_labeled` since phase-03, plus its test in
  `src/cli/tests.rs:76`), `visual_len` (markdown), `terminal_width`
  (markdown/ask/status/chat), `StatusBarState` (everywhere).
- tokio allows multiple listeners on one signal — a second
  `signal(SignalKind::window_change())` inside the streaming path does not
  starve the idle loop's listener in `chat.rs`; every listener sees every
  signal.

## Spec

### Task 1 — `Reanchor` outcome and a pure focus filter in `stream.rs`

1. Add a variant to `StreamOutcome` (`:22`):

   ```rust
   /// A resize or focus-gain arrived mid-stream — caller must re-anchor.
   Reanchor,
   ```

2. Add a pure helper near `tool_runtime_label` — pinned exactly (mutation
   M1's target lives on its match arm):

   ```rust
   /// Map a key event to a stream outcome the interrupt filter must not
   /// swallow. FocusGained (ESC [ I) means the user switched back to this
   /// pane and the viewport may need re-pinning.
   fn focus_outcome(key: &Key) -> Option<StreamOutcome> {
       match key {
           Key::FocusGained => Some(StreamOutcome::Reanchor),
           _ => None,
       }
   }
   ```

   (Adjust the `Key` import to however `read_key`'s type is already named in
   this file.)

3. In **both** `tokio::select!` branches of `select_stream`, at the top of
   the `read_key` arm's `if let Some(key) = key` body — before
   `interrupt_state.feed(&key)` — insert:

   ```rust
   if let Some(outcome) = focus_outcome(&key) {
       return outcome;
   }
   ```

### Task 2 — SIGWINCH arm in `select_stream`

1. Change `select_stream`'s signature to take the listener:

   ```rust
   sigwinch: &mut tokio::signal::unix::Signal,
   ```

2. In **both** `tokio::select!` blocks add the arm:

   ```rust
   _ = sigwinch.recv() => {
       return StreamOutcome::Reanchor;
   }
   ```

3. In the function that owns the streaming loop (the `loop` at `:168`),
   create the listener once before the loop:

   ```rust
   let mut sigwinch = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
   ```

   and pass `&mut sigwinch` to every `select_stream` call. Update the two
   existing `select_stream` tests to construct and pass their own listener
   the same way.

### Task 3 — Handle `Reanchor` in the outcome match

In the caller's match (`stream.rs:190-243`), add an arm alongside `Tick`:

```rust
StreamOutcome::Reanchor => {
    renderer.reanchor();
    continue;
}
```

(The next 80 ms tick redraws the spinner into the re-pinned viewport; no
explicit redraw needed here.)

### Task 4 — Idle-loop resize also re-anchors

In `src/cli/commands/chat.rs`'s SIGWINCH arm (`:625-631`), add
`renderer.reanchor();` immediately before the existing `renderer.draw(...)`
call. After this, `grep -c 'renderer.reanchor()' src/cli/commands/chat.rs`
is 2.

### Task 5 — Delete the dead legacy printers

In `src/cli/render.rs`, delete these six items **and nothing else**:
`print_tool_panel`, `local_user_host`, `print_tool_started`,
`print_tool_finished`, `print_user_query`, `terminal_height`.

`wrap_line_hard`, `visual_len`, `terminal_width`, and `StatusBarState` stay
(all have live callers). The `wrap_line_hard_with_newlines` test in
`src/cli/tests.rs` stays. If the deletion orphans an import or doc reference
inside `render.rs`, clean that up within the file; the deny-warnings gate is
the check.

### Task 6 — Tests

Write the tests named in § Test plan. The two select-stream tests follow the
existing fixture shape at `stream.rs:1356/:1384` (pipe-backed fake stdin;
write raw bytes; assert on the returned outcome). For the SIGWINCH test,
raise the signal to the current process with `unsafe { libc::raise(libc::SIGWINCH) }`
after registering the listener (the test harness ignores SIGWINCH itself, and
no other test selects on it; `libc` is already a dependency).

### Task 7 — Mutation M1 apply + restore (focus filter)

Apply a `patch` on `src/cli/commands/stream.rs` changing
`Key::FocusGained => Some(StreamOutcome::Reanchor),` to
`Key::FocusGained => None,`, then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-05.txt
cargo test --lib focus 2>&1 | tail -5 >> /tmp/e2e-m13-05.txt
```

`focus_outcome_maps_focus_gained_to_reanchor` and/or
`select_stream_focus_gained_returns_reanchor` must show **FAILED**. If all
stay green, report a blocker — do not adjust a test to make it fail. Restore
with the inverse `patch`, then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-05.txt
grep -c 'Key::FocusGained => None' src/cli/commands/stream.rs >> /tmp/e2e-m13-05.txt
cargo test --lib focus 2>&1 | tail -5 >> /tmp/e2e-m13-05.txt
```

The grep count must be `0` and the tests green.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
append a new Update Log entry headed
`### Update — <date> (end-to-end verification)` whose fenced block is the
contents of `/tmp/e2e-m13-05.txt`, **inserted by command (`cat >> <this
phase doc>`), never retyped**. Then run this self-check and paste its output
as the entry's last line, outside the fence:

```sh
awk '/^### Update — .*\(end-to-end verification\)/{f=1} f' docs/dev/milestones/M13-chat-ux/phase-05-resize-and-reanchor.md | sed -n '/^```$/,/^```$/p' | sed '1d;$d' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-m13-05.txt > /dev/null && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The run is finished only when this prints `PASTE MATCH`. The server-authored
`(complete)` entry does not satisfy Task 8.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `grep -c 'StreamOutcome::Reanchor' src/cli/commands/stream.rs` prints
      at least `4` (variant, helper, two select arms, caller arm).
      (Currently: 0.)
- [ ] `grep -c 'renderer.reanchor()' src/cli/commands/chat.rs` prints `2`.
      (Currently: 1.)
- [ ] `grep -cE 'fn (print_tool_panel|print_user_query|print_tool_started|print_tool_finished|local_user_host|terminal_height)' src/cli/render.rs`
      prints `0`. (Currently: 6.)
- [ ] Tests `focus_outcome_maps_focus_gained_to_reanchor`,
      `select_stream_focus_gained_returns_reanchor`,
      `select_stream_sigwinch_returns_reanchor` pass. (Currently: none
      exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] `select_stream_first_interrupt_press_warns` and
      `select_stream_delivers_a_full_daemon_message` still pass (updated only
      for the new `sigwinch` parameter).
- [ ] `wrap_line_hard_with_newlines` (`src/cli/tests.rs`) still passes —
      `wrap_line_hard` must survive the deletion.
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`
      — the deny-warnings gate doubles as the no-orphaned-dead-code check
      after Task 5.

## Test plan

In `src/cli/commands/stream.rs` `mod tests`:

- `focus_outcome_maps_focus_gained_to_reanchor` — pure:
  `focus_outcome(&Key::FocusGained)` matches
  `Some(StreamOutcome::Reanchor)`; `focus_outcome(&Key::Char('x'))` is
  `None`. (Mutation M1 target.)
- `select_stream_focus_gained_returns_reanchor` — pipe-backed stdin fixture
  (copy the `:1384` shape); write the focus-gained escape `b"\x1b[I"`; assert
  the returned outcome is `StreamOutcome::Reanchor`, not
  `Warn`/`Interrupted`.
- `select_stream_sigwinch_returns_reanchor` — register the listener, spawn
  the select with a quiet stdin/socket, `libc::raise(libc::SIGWINCH)`, assert
  `StreamOutcome::Reanchor`.

## End-to-end verification

```sh
: > /tmp/e2e-m13-05.txt
echo "== GATES ==" >> /tmp/e2e-m13-05.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-05.txt; echo "fmt exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-05.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-05.txt; echo "build exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-05.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-05.txt; echo "clippy exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-05.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-05.txt; echo "test exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-05.txt
echo "== SURFACES ==" >> /tmp/e2e-m13-05.txt
echo "reanchor outcomes: $(grep -c 'StreamOutcome::Reanchor' src/cli/commands/stream.rs)" >> /tmp/e2e-m13-05.txt
echo "chat reanchors: $(grep -c 'renderer.reanchor()' src/cli/commands/chat.rs)" >> /tmp/e2e-m13-05.txt
echo "dead printers: $(grep -cE 'fn (print_tool_panel|print_user_query|print_tool_started|print_tool_finished|local_user_host|terminal_height)' src/cli/render.rs)" >> /tmp/e2e-m13-05.txt
echo "render.rs lines: $(wc -l < src/cli/render.rs)" >> /tmp/e2e-m13-05.txt
wc -l /tmp/e2e-m13-05.txt >> /tmp/e2e-m13-05.txt
```

(The Task 7 mutation runs append into the same file in task order.)

The milestone's live checks — window switch mid-turn re-anchoring, remote
colors on pinky, the cursor feel-check — happen at milestone close, not in
this phase's executor block.

## Authorizations

None.

## Out of scope

- Re-wrapping already-committed scrollback on resize — declared a non-goal in
  the milestone README (insert_before content is immutable).
- `render_prompt_region` / approval-prompt flows.
- Deleting `wrap_line_hard`, `visual_len`, `terminal_width`, or
  `StatusBarState` — all live.
- Anything under `src/daemon/`, `src/cli/markdown/`, `src/cli/input/editor.rs`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-10 01:25 (started)

**Executor:** rexyMCP executor

Implementing Tasks 1–8: Reanchor outcome, focus filter, SIGWINCH in select_stream, reanchor handling, idle-loop resize re-anchor, dead legacy printer deletion, tests, mutation M1, and end-to-end evidence capture.


### Update — 2026-08-10 01:25 (end-to-end verification)

```
== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.34s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.05s
clippy exit=0
test result: ok. 1228 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.98s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
== SURFACES ==
reanchor outcomes: 7
chat reanchors: 2
dead printers: 0
render.rs lines: 87
21 /tmp/e2e-m13-05.txt
== M1 APPLIED ==
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.39s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 2 tests
test cli::commands::stream::stream_seam_tests::focus_outcome_maps_focus_gained_to_reanchor ... FAILED
== M1 RESTORED ==
0
test cli::commands::stream::stream_seam_tests::focus_outcome_maps_focus_gained_to_reanchor ... ok
test cli::commands::stream::stream_seam_tests::select_stream_focus_gained_returns_reanchor ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1226 filtered out; finished in 0.00s

35 /tmp/e2e-m13-05.txt
```
