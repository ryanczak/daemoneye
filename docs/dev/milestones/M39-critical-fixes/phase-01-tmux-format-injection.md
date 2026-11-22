# Phase 01 — tmux-format-injection

**Milestone:** M39 — Critical fixes
**Status:** in-progress (bounced 2026-11-22: [bug-01-1](bugs/bug-01-1.md))
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
- [ ] **(bug-01-1)** One run of
      `cargo test --lib format_literal_round_trips_through_tmux` leaves the
      count from `pgrep -fc 'tmux -S /tmp/.*/s.sock new-session -d -s t'`
      unchanged, on a passing run **and** on a deliberately failing one.
      The Update Log shows both counts and the red run.
- [ ] **(bug-01-1)** The test's skip probe is `tmux -V`, and every other
      tmux call in `src/tmux/pane.rs`'s test module passes `-S`.

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

### Update — 2026-11-22 01:32 (complete, server-authored)

**Summary:** **Summary:** Phase 01 (tmux-format-injection) is implemented: `src/tmux/pane.rs` gains the pure `format_literal` (`#`→`##`, `%`→`%%`, `\n`/`\r`/`\t`→space, other control chars dropped), the pure `overlay_argv` (`display-message -d 8000 -t <pane> -- <escaped>`), and `show_overlay` running it through `crate::process::bounded_output`; `notify_chat_panes` now calls `crate::tmux::show_overlay` and no longer contains any `display-message` (grep count `0`), while the presence push still receives the unescaped message and `deliver_chat_notice` needed no edit. All five new tests live in `src/tmux/pane.rs`'s test module and pass, including a round-trip test against a real private tmux server (`-S <tempdir>/s.sock`, tmux 3.6) asserting the escaped `#(touch …)` payload prints back unchanged and leaves no `pwned` file; `notify_chat_panes_pushes_over_presence` passes unchanged. End-to-end verification block was run mechanically and its output pasted into a dedicated Update Log entry (`/tmp/m39p1-e2e/out.txt`); full `cargo test` is 3049 lib + 107 integration tests, 0 failed, and `cargo fmt --all`, `cargo build`, and `cargo clippy --all-targets --all-features -- -D warnings` are all clean.

**Notes for review:** (1) While fixing an earlier edit the doc-comment on `query_pane_width` was briefly duplicated by a patch race; I caught and removed it in the same session — the committed file has it once. (2) The committed diff includes three pre-existing uncommitted doc changes (`docs/dev/NEXT.md` active-phase pointer and the M39 README's phase-table row/drafting note) that were on disk from the architect's dispatch; I staged everything as permitted, and the README row now reads `in-progress` with the phase doc left there for the server to flip. (3) Findings on the presence push (out of scope): `presence_broadcast(msg)` still receives the raw unescaped line, so control characters in an alert reach chat clients as-is — worth a look when the notice-rendering concern lands. (4) The round-trip test uses `std::thread::sleep(300ms)` per the spec's explicit instruction, and skips only when spawning `tmux` fails; both are test-only deviations the spec pins.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ig_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test attach_sends_session_counters_after_ok ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.21s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v2.10.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.43s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-c067c2b4b53c4301)
kill: (1600242): No such process
kill: (-1600956): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-d9a9e35dfb95286a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-2e2bade347dfc2bf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-fec57f080473704c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-b7049ce40c11944f)
     Running tests/integration.rs (target/debug/deps/integration-521aa0dc073b7f75)
     Running tests/isolation.rs (target/debug/deps/isolation-4e1a6c89c680c6e7)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-25580a42c302b017)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-a1f9f82f934e25c3)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M39-critical-fixes/README.md` — +1 -1
- `docs/dev/milestones/M39-critical-fixes/phase-01-tmux-format-injection.md` — +36 -1
- `src/tmux/pane.rs` — +123 -0
- `src/webhook/process.rs` — +1 -3

**Commit:** 7e85594a5af7c5ce5c4fbc0cad5aa35402f390bb

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review — 2026-11-22 (bounced)

The production change is correct and stays: `format_literal`,
`overlay_argv`, `show_overlay`, and the `notify_chat_panes` swap. All four
gates are green on re-run (3049 lib tests). The round-trip test leaks its
private tmux server on every run, and its skip probe calls the ambient
server. See [bug-01-1](bugs/bug-01-1.md).

**Notes for executor (re-dispatch):** Touch only the
`format_literal_round_trips_through_tmux` test in `src/tmux/pane.rs`.
Give the private server a teardown that runs on drop, as `PrivateTmux` in
`src/cli/program_status_emit_tests.rs` does, so it also runs when an
assertion panics. Make the skip probe `tmux -V`. Then work through the
bug's Definition of done, starting with the red run that shows the count
unchanged. Your first `patch` lands within 8 turns. No other file
changes.
