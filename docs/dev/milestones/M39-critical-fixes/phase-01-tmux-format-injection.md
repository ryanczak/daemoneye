# Phase 01 — tmux-format-injection

**Milestone:** M39 — Critical fixes
**Status:** in-progress
**Drafted:** 2026-11-21
**Depends on:** none
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

A chat-pane overlay shows its text literally. Today `notify_chat_panes`
(`src/webhook/process.rs`) passes the notice line to `tmux
display-message` as a FORMAT. The line carries a webhook's `alert_name` and
`summary`, and the default webhook listener is unauthenticated on
`127.0.0.1:9393`. An alert named `#(touch /tmp/x)` therefore runs a shell
command as the user whenever a chat session has a chat pane. Ledger row S1.

## Architecture references

- `src/webhook/process.rs` `notify_chat_panes`, and its one production
  caller, `deliver_chat_notice` in `src/daemon/notice_sinks.rs`.
- `src/tmux/pane.rs`: where the overlay call moves, beside the other pane
  operations (CLAUDE.md: `src/tmux/` holds the tmux subprocess calls).
- The analogue for a test on a private tmux server is `PrivateTmux` in
  `src/cli/program_status_emit_tests.rs` (a `-S <tempdir>/s.sock` server,
  killed on drop).

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Facts the executor cannot get from the repo

These were verified on pinky's tmux 3.6 against a private server at
drafting:

- Without `-l`, `display-message` expands the message twice. First as a
  tmux FORMAT, where `#(cmd)` runs `cmd` through `/bin/sh` without waiting
  and inserts its output, `#{…}` and `#S` are variables, and `##` is a
  literal `#`. Then through strftime, where `%H` prints the hour and `%%`
  is a literal `%`.
  `display-message -p -- '#(echo PWNED) %H ## #{pane_id}'` printed
  ` 17 # %0`, and `-p -- '##(echo PWNED) %%H'` printed `#(echo PWNED) %H`.
- A message starting with `-` is read as a flag: `display-message -p
  '-x hello'` fails with `unknown flag -x`. With `--` before the message,
  it prints `-x hello`.
- `-l` (print the message unchanged) was added in **tmux 3.4**. Ubuntu
  22.04 ships 3.2a and Debian 12 ships 3.3a. **Do not use `-l`**: escaping
  works on every version, and with `-l` an escaped `##` would print as
  two `#`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 10 turns.**
Read **only** these files: `src/tmux/pane.rs`, `src/webhook/process.rs`
(`notify_chat_panes` and its test `notify_chat_panes_pushes_over_presence`),
`src/daemon/notice_sinks.rs` (`deliver_chat_notice`), and
`src/cli/program_status_emit_tests.rs` (`PrivateTmux` only).

## Spec

1. **The escape** (in `src/tmux/pane.rs`). Add
   `pub fn format_literal(msg: &str) -> String`, a pure function. It
   returns text that tmux's format and strftime expansion turn back into
   `msg`, with these rules:
   - `#` becomes `##` and `%` becomes `%%`.
   - `\n`, `\r` and `\t` each become one space.
   - Every other `char::is_control()` character is dropped. That covers
     ESC, NUL, DEL and C1.
   - Everything else passes through unchanged, non-ASCII included.

   Write the four `format_literal_*` tests from the Test plan first. Run
   them red against a stub that returns `msg` unchanged, then implement.
   Ends at a green `cargo test format_literal`.

2. **The argv** (in `src/tmux/pane.rs`). Add
   `pub fn overlay_argv(pane: &str, msg: &str) -> Vec<String>`, a pure
   function returning `display-message -d 8000 -t <pane> --
   <format_literal(msg)>` without the `tmux` binary. `--` is always the
   second-to-last element and the escaped message the last. Then add
   `pub fn show_overlay(pane: &str, msg: &str)`, which runs `tmux` with
   that argv through `crate::process::bounded_output` and ignores the
   result, as the call it replaces does. Ends at a green
   `cargo test overlay_argv`.

3. **The caller** (in `src/webhook/process.rs`). `notify_chat_panes` calls
   `crate::tmux::show_overlay(pane, msg)` for each pane in place of its
   inline `Command::new("tmux")`. Its unlocked-phase structure and the
   `presence_broadcast(msg)` call are unchanged, and the presence push
   still gets the **unescaped** `msg`. Ends at a green
   `cargo test notify_chat_panes`.

4. **Capture the end-to-end evidence.** Run the block in § End-to-end
   verification and paste its output into a new Update Log entry.

**Must stay:**
- `notify_chat_panes`'s signature, and `deliver_chat_notice`, which
  needs no edit.
- No `display-message` call in `src/webhook/` after this phase.
- No other `display-message` site changes: every other one passes a fixed
  `-p` format string, not untrusted text.

## Acceptance criteria

- [ ] `grep -c 'display-message' src/webhook/process.rs` prints `0`. The
      module doc comment in `src/webhook/mod.rs` may keep naming it.
- [ ] `grep -c 'pub fn format_literal\|pub fn overlay_argv\|pub fn show_overlay' src/tmux/pane.rs`
      prints `3`.
- [ ] Every test in the Test plan exists and passes.
      `notify_chat_panes_pushes_over_presence` passes unchanged.
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0,
      and `tests/tmux_read_only.rs` and `tests/hygiene.rs` pass.

## Test plan

All new tests live in a `#[cfg(test)] mod tests` at the bottom of
`src/tmux/pane.rs`. Each one calls `format_literal` or `overlay_argv`. **No
test restates the escape rules in its own helper.**

- `format_literal_escapes_hash_and_percent`: `#(touch /tmp/x) %H #{pane_id} ##`
  becomes `##(touch /tmp/x) %%H ##{pane_id} ####`.
- `format_literal_strips_control_characters`: `a\nb\rc\td` becomes
  `a b c d`. `x\x1b[31my\x00z\x7f` becomes `x[31myz`.
- `format_literal_leaves_plain_text_alone` (negative): `WEBHOOK ALERT:
  disk full on db-1 — ünïcødé` comes back unchanged.
- `format_literal_round_trips_through_tmux`. Start a private tmux server
  on a tempdir socket with `new-session -d`, as `PrivateTmux` does. Run
  `display-message -p -- <format_literal(input)>` on it, with `input` =
  `#(touch <tempdir>/pwned) %H #{pane_id} ## 100%`. Then:
  - assert the printed line, trimmed, equals `input`;
  - sleep 300 ms;
  - assert `<tempdir>/pwned` does **not** exist.

  If no `tmux` binary is on `PATH`, print a skip note and return. **The
  test must use `-S` with its own socket, never the ambient server**: the
  suite runs inside the PE's tmux.
- `overlay_argv_ends_with_double_dash_then_the_escaped_message`. For pane
  `%3` and message `-x #(id)`, the argv is exactly
  `["display-message", "-d", "8000", "-t", "%3", "--", "-x ##(id)"]`.

## End-to-end verification

```sh
cargo test --lib tmux::pane 2>&1 | grep 'test result:'
cargo test --lib notify_chat_panes 2>&1 | grep 'test result:'
cargo test --test tmux_read_only 2>&1 | grep 'test result:'
grep -c 'display-message' src/webhook/process.rs
```

Paste each line of output. Do not paste per-test `… ok` lines.

## Authorizations

- May touch exactly `src/tmux/pane.rs` and `src/webhook/process.rs`.

## Out of scope

- The presence push to chat clients (`presence_broadcast`) and how the
  client renders control characters in a notice. Record anything you see
  there as a Findings note in the Update Log, not as a fix.
- Every other tmux call site, including the unbounded
  `get_pane_remote_host` (M42).
- The webhook listener's authentication (M40 phase 09a).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-22 01:18 (started)

Executor: executor (headless run). Status flipped `todo → in-progress` and the
milestone README row updated. Plan: red tests for `format_literal` against an
identity stub, then the implementation, `overlay_argv` + `show_overlay`, the
caller swap in `notify_chat_panes`, and the end-to-end evidence run.

### Update — 2026-11-22 01:35 (end-to-end verification)

Captured mechanically from a single run of the phase's § End-to-end
verification block (`/tmp/m39p1-e2e/out.txt`), plus the acceptance greps:

```
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 3054 filtered out; finished in 0.32s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3058 filtered out; finished in 5.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
0
```

Line 1: `cargo test --lib tmux::pane` (the 4 `format_literal_*` tests plus
`overlay_argv_*`). Line 2: `cargo test --lib notify_chat_panes`
(`notify_chat_panes_pushes_over_presence`, unchanged, passes through the new
`crate::tmux::show_overlay` seam). Line 3: `cargo test --test tmux_read_only`.
Line 4: `grep -c 'display-message' src/webhook/process.rs` → `0`.
`grep -c 'pub fn format_literal\|pub fn overlay_argv\|pub fn show_overlay'
src/tmux/pane.rs` → `3`. `cargo test --test hygiene` → `test result: ok. 16
passed; 0 failed`. Full `cargo test` → 3049 lib tests + 107 integration, 0
failed. `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
--all-features -- -D warnings` all clean.

The `format_literal_round_trips_through_tmux` test ran against a real private
tmux server (`-S <tempdir>/s.sock`, `new-session -d`) and asserts the escaped
`#(touch …)` payload prints back unchanged and leaves no `pwned` file.

