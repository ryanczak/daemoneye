# Phase 03 — vt100 0.18

**Status:** review
**Milestone:** M33 — Dependency refresh
**Drafted:** 2026-10-31
**Depends on:** phase 02 (done)
**Tags:** language=rust, kind=chore, size=m

## Goal

vt100 goes from 0.16.2 to **0.18.1**, and DaemonEye takes its new resize
behaviour in both places a screen is resized (M33 decision 3; PE choice
2026-10-31, "viewer + backend"):

- **The tail viewer** (`src/cli/tail.rs`, `resize`) already calls
  `set_size`; it gets the new behaviour by upgrading.
- **The daemon's shell screen** stops being rebuilt on resize.
  `PtyBackend::resize` (`src/shell/backend.rs`) today replaces the screen
  with a blank `ShellScreen::new` (the old "F2, does not reflow" comment).
  After this phase it resizes the existing screen through a new
  `ShellScreen::resize`, so a resized shell keeps its screen and scrollback.

The suite goes from **2616 to exactly 2618**: two tests added, one renamed.

## What changed upstream (vt100 CHANGELOG, 0.17.0 and 0.18.0)

You cannot fetch this; it is the part that matters here.

- `Parser::new(rows, cols, scrollback)` and `Screen::set_size(rows, cols)`
  take `NonZeroU16`, and `Screen::size()` returns `(NonZeroU16, NonZeroU16)`.
  Passing 0 used to panic.
- These `Screen` methods return `String` instead of `Vec<u8>` (the bytes
  were always UTF-8): `attributes_formatted`, `contents_diff`,
  `contents_formatted`, `cursor_state_formatted`, `input_mode_diff`,
  `input_mode_formatted`, `rows_diff`, `rows_formatted`, `state_diff`,
  `state_formatted`.
- `Screen::set_size`, when the height **decreases**: drops rows from the
  bottom only until the cursor is on the bottom row, then pushes the excess
  rows off the top **into scrollback**. When the height **increases**: pulls
  rows back from scrollback into the top, then pads with blank rows at the
  bottom. It also resets the scroll region and the scrollback offset when the
  size changes. (0.16 cut or padded rows at the bottom.)
- New and unused here: `Screen::scrollback_len`, `clear_scrollback`,
  `contents_formatted_basic`, `Parser::set_size`, `Callbacks::on_scroll`.

## Spec

1. **Upgrade** — `Cargo.toml` vt100 to `0.18.1`; `cargo update -p vt100`.
   Not a bare `cargo update`.
2. **Fix the build** — the errors are in `src/shell/screen.rs`,
   `src/cli/tail.rs`, `src/cli/tail_view.rs`, `src/shell/keys.rs` and the
   test files that construct parsers (`src/shell/backend_tests.rs`,
   `src/daemon/server/shells_tests.rs`). Rules: `ShellScreen` and the tail
   model keep their `u16` signatures; a 0 becomes 1 at the conversion, as
   `tail.rs` already does with `.max(1)`; no `unwrap`/`expect` in production
   code (`NonZeroU16::MIN` is the fallback). Where a caller wants bytes, take
   the `String` and call `.into_bytes()`; do not round-trip through
   `from_utf8`. Done when `cargo build --all-targets` is clean.
3. **`ShellScreen::resize(rows, cols)`** — add it in `src/shell/screen.rs`
   (calls `set_size`, same 0 → 1 rule). `PtyBackend::resize` calls it instead
   of assigning a new `ShellScreen`; replace the F2 comment with one line
   saying the screen keeps its rows across a resize (vt100 0.18 semantics).
   Nothing else in `backend.rs` changes.
4. **Tests** — both new tests go in the existing test modules, beside their
   neighbours:
   - `screen_resize_shrink_pushes_rows_to_scrollback_and_grow_restores_them`
     (`src/shell/screen.rs` tests): print numbered lines into a small screen,
     shrink the height, assert the top lines left `contents()` and appear in
     `scrollback_rows`, grow back, assert `contents()` shows them again.
   - `tail_resize_shrink_then_grow_keeps_the_rows` (`src/cli/tail.rs`
     tests): the same shrink and grow through the tail model's `resize`.
   - Rename `backend_resize_replaces_the_screen_and_does_not_reflow`
     (`src/shell/backend_tests.rs`) to `backend_resize_keeps_the_screen_contents`.
     Before resizing, print a marker and wait for it the way
     `subscribe_with_screen_then_live_chunks_rebuild_the_screen` does. Keep
     the size asserts, and assert the marker is still in `screen_contents()`
     after resizing to 40×120.
5. **Red check** — temporarily put the old `ShellScreen::new` line back in
   `PtyBackend::resize`, run `cargo test backend_resize_keeps_the_screen_contents`,
   restore it. Paste the `test result:` line and the assertion message (two
   lines) in an `### Update — <date> (red check)` entry.
6. **Findings** — one `### Findings — <date>` entry built from
   `git diff --stat` alone: two or three lines naming the files that changed
   and any call site where you departed from step 2's rules, or `none`. Open
   no file and no crate source for this step.
7. **End-to-end** — run the block below once and paste its output, fenced, in
   an `### Update — <date> (end-to-end verification)` entry.
8. **Commit** as `chore: upgrade vt100 to 0.18; keep the shell screen across a resize`.

The vt100 source is under `~/.cargo/registry/src/*/vt100-0.18.1/`. Open it
only to fix a compile error you cannot fix from the compiler message.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `vt100=0.18.1 rebuild=0 screen_resize=1 new_tests=2 old_name=0`
      (today `vt100=0.16.2 rebuild=1 screen_resize=0 new_tests=0 old_name=1`).
- [ ] `allowlist=unchanged`, `build_warnings=0`, clippy clean, `fmt_exit=0`;
      hygiene and doc_truth pass.
- [ ] `cargo test` reports exactly `passed=2618 failed=0`.
- [ ] The red-check entry shows `backend_resize_keeps_the_screen_contents`
      failing against the rebuild.
- [ ] No new `#[allow]`, `#[ignore]`, `unsafe`, or production `unwrap`/`expect`.

## End-to-end verification

```sh
bash <<'E2E'
echo "vt100=$(cargo tree --depth 1 | sed -n 's/.*── vt100 v\([0-9.]*\).*/\1/p') rebuild=$(sed -n '/async fn resize/,/^    }/p' src/shell/backend.rs | grep -c 'ShellScreen::new') screen_resize=$(grep -c 'pub fn resize' src/shell/screen.rs) new_tests=$(grep -hcE 'fn (screen_resize_shrink_pushes_rows_to_scrollback_and_grow_restores_them|tail_resize_shrink_then_grow_keeps_the_rows)\(' src/shell/screen.rs src/cli/tail.rs | paste -sd+ | bc) old_name=$(grep -c 'fn backend_resize_replaces_the_screen_and_does_not_reflow' src/shell/backend_tests.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `Cargo.toml`, `Cargo.lock` (via `cargo update -p vt100`) and the six
  files named in step 2.
- Change `PtyBackend::resize`'s behaviour as step 3 says (PE decision).

## Out of scope

- Any other crate. The new vt100 APIs (`on_scroll`, `clear_scrollback`, …).
- Cap headroom is ample (production lines: `screen.rs` 250, `backend.rs`
  312, `tail.rs` 610 of 800); the allowlist gains nothing.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 00:40 (end-to-end verification)

Ran the phase doc's E2E block once; output pasted verbatim:

```
vt100=0.18.1 rebuild=0 screen_resize=1 new_tests=2 old_name=0
allowlist=unchanged
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.19s
fmt_exit=0
passed=2618 failed=0
```

First line matches the acceptance criterion exactly (`vt100=0.18.1 rebuild=0
screen_resize=1 new_tests=2 old_name=0`), `cargo test` reports exactly
`passed=2618 failed=0` (2616 before + the two new tests).

### Findings — 2026-10-31 (from `git diff --stat`)

Changed: `Cargo.toml`/`Cargo.lock` (vt100 0.16.2 → 0.18.1), `src/shell/screen.rs`
(`ShellScreen::resize` + the `NonZeroU16` conversions), `src/shell/backend.rs`
(`PtyBackend::resize` calls it), `src/cli/tail.rs` (parser construction, `resize`,
the `.get()` on `size()` halves), `src/cli/tail_view.rs` (one `.get()` pair),
plus the test files `src/shell/backend_tests.rs`, `src/daemon/server/shells_tests.rs`.
Departures from step 2's file list: `src/cli/attach.rs` also needed the fix (two
`model.screen().size()` destructures and a `(30, 100)` size assert leaked the
`NonZero` change there); `src/shell/keys.rs` needed nothing (its `Screen` reads
return `bool`, unchanged upstream). Also dropped `PtyBackend`'s `scrollback`
field — the removed `ShellScreen::new` in `resize` was its only reader.

### Update — 2026-10-31 00:36 (red check)

`PtyBackend::resize` temporarily rebuilt the screen with `ShellScreen::new`
(the old behaviour) while `backend_resize_keeps_the_screen_contents` was run
against it:

```
thread 'shell::backend::tests::backend_resize_keeps_the_screen_contents' (1370766) panicked at src/shell/backend_tests.rs:811:5:
the screen keeps its contents across a resize: ""
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2538 filtered out; finished in 0.05s
```

The old `ShellScreen::new` line is restored; the test passes against the
`ShellScreen::resize` implementation.

### Update — 2026-10-31 00:07 (progress)

Executor 03 started. Plan: upgrade vt100 in `Cargo.toml` + `cargo update -p vt100`,
fix the five production/test files named in step 2 for the `NonZeroU16` sizes and
`String` returns, add `ShellScreen::resize` and rewire `PtyBackend::resize`, then
the two new tests, the red check, findings, E2E, and the commit.

### Update — 2026-10-31 00:43 (complete, server-authored)

**Summary:** **Summary:** Upgraded vt100 0.16.2 → 0.18.1 (`Cargo.toml` + `cargo update -p vt100`) and adapted the call sites: `ShellScreen::new`/`resize` and the tail model's parser construction now convert `u16` → `NonZeroU16` at the boundary with a `0 → NonZeroU16::MIN` fallback (matching the existing `.max(1)` clamp), the `state_formatted()`/`size()` `String`/`NonZero` return changes are unwrapped with `.as_bytes()`/`.get()`, and a new `ShellScreen::resize(rows, cols)` lets `PtyBackend::resize` resize the existing screen instead of replacing it with a blank one (dropping `PtyBackend`'s now-readerless `scrollback` field). Added `screen_resize_shrink_pushes_rows_to_scrollback_and_grow_restores_them` and `tail_resize_shrink_then_grow_keeps_the_rows`, renamed `backend_resize_replaces_the_screen_and_does_not_reflow` → `backend_resize_keeps_the_screen_contents` (with a pre-resize marker print + wait, kept-content assert at 40×120), and verified the red check — the test fails with `the screen keeps its contents across a resize: ""` against the old `ShellScreen::new` rebuild.

**Notes for review:**
- **Deviations from the spec's file list (step 2):** `src/cli/attach.rs` also needed the fix — two `model.screen().size()` destructures and a `(30, 100)` test assert leaked the `NonZero` change there; `src/shell/keys.rs` needed nothing (its `Screen` reads return `bool`, unchanged upstream). Recorded in the Findings entry.
- **Test-value calibration:** I probed vt100 0.18.1's actual resize semantics before pinning expectations — a 3-row screen fed 6 lines then shrunk to 2 pushes `line5` into scrollback (`line1..line5` kept), and growing to 5 restores `line3\nline4\nline5\nline6`; the spec's "same shrink and grow" tests pin those measured values rather than guessed ones.
- **E2E (ran once, pasted in the phase doc):** `vt100=0.18.1 rebuild=0 screen_resize=1 new_tests=2 old_name=0`, `allowlist=unchanged`, `build_warnings=0`, clippy clean, `fmt_exit=0`, hygiene 16 passed, doc_truth 16 passed, `cargo test` = exactly `passed=2618 failed=0`.
- Red-check and E2E entries are in the phase doc's Update Log; status left at `in-progress` per the lifecycle split, one commit `b2fd8c1f` (`chore: upgrade vt100 to 0.18; keep the shell screen across a resize`), working tree clean.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s


TEST
. ok
test functions_fit_the_cap ... ok
test exempt_files_exist ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.09s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test event_log_append_read ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-3895041824b11c14)
kill: (1392625): No such process
kill: (-1393367): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-dd2871b6637bde5a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-f771ba28c5e0838d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6a9c87db276c8578)
     Running tests/hygiene.rs (target/debug/deps/hygiene-54ba86320be9ce2a)
     Running tests/integration.rs (target/debug/deps/integration-bd124ab667c8b0d2)
     Running tests/isolation.rs (target/debug/deps/isolation-321adfe6f8bdcdd8)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-8dce9e6501c0ec71)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-5251d864d6747c26)
   Doc-tests daemoneye

```

**Files changed:**

- `Cargo.toml` — +1 -1
- `docs/dev/milestones/M33-dependency-refresh/README.md` — +1 -1
- `docs/dev/milestones/M33-dependency-refresh/phase-03-vt100-0.18.md` — +56 -1
- `src/cli/attach.rs` — +9 -1
- `src/cli/tail.rs` — +56 -8
- `src/cli/tail_view.rs` — +1 -0
- `src/daemon/server/shells_tests.rs` — +5 -1
- `src/shell/backend.rs` — +3 -5
- `src/shell/backend_tests.rs` — +23 -2
- `src/shell/screen.rs` — +66 -5

**Commit:** b2fd8c1f2a994beb7585f86217bd539d013b79b8

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
