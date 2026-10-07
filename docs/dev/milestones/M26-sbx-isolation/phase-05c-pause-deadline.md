# Phase 05c: pause-deadline — time spent paused does not count against a command's deadline

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** done
**Depends on:** phase-05b
**Estimated diff:** ~120 lines (about 25 code, 95 tests)
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Q6, decided at scope sign-off: **a pause suspends the command's deadline
clock**, as prompt time already does. Today, if an operator pauses a
command with `Ctrl-p` in attached mode, the paused time still counts. A ghost
command with a 300 s limit that is paused for 5 minutes is reported as timed
out the moment it is resumed. The fix lives in the one place every command's
deadline lives, `read_to_marker` in `src/shell/client.rs`. It covers host
shells and sandbox shells alike: since 05b, a sandbox shell's `Status` reports
`Paused` from inside the VM.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Decisions on the open
  questions", Q6.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`read_to_marker` (`src/shell/client.rs`) waits for the command's end marker in
slices of at most `EXIT_POLL_INTERVAL` (500 ms). When a slice times out with no
output it checks the deadline **first**, then asks the host whether the shell
has exited:

```rust
let mut deadline = tokio::time::Instant::now() + timeout;
…
loop {
    let slice_end = match drain_until {
        Some(end) => end,
        None => deadline.min(tokio::time::Instant::now() + EXIT_POLL_INTERVAL),
    };
    let read = tokio::time::timeout_at(slice_end, reader.read_until(b'\n', &mut line)).await;
    let n = match read {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => { … }
        Err(_) => {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out after {timeout:?} waiting for command output: {cmd:?}"
                );
            }
            if let Some(end) = drain_until {
                if tokio::time::Instant::now() >= end {
                    return Err(ShellExited { cmd: cmd.to_string() }.into());
                }
            } else if let Ok(ShellResponse::Status {
                state: ShellState::Exited,
                ..
            }) = status(sock, EXIT_STATUS_TIMEOUT).await
            {
                drain_until = Some(tokio::time::Instant::now() + EXIT_DRAIN_WINDOW);
            }
            continue;
        }
    };
```

**The idiom to copy is in the same function.** Time spent waiting on a prompt
answer is already added back to the deadline:

```rust
let asked = tokio::time::Instant::now();
let reply = on_chunk(&seen).await;
deadline += asked.elapsed();
```

A paused shell prints nothing, so a paused command sits in the `Err(_)`
(slice timed out) arm, and every slice ends with the same `status` call. That
call is where the pause is visible: `ShellResponse::Status { state:
ShellState::Paused, .. }`.

### Test fixture to copy (`src/shell/client.rs` tests)

```rust
async fn spawn_host() -> Host {
    let _home = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    let cast = dir.path().join("e2e.cast");
    let backend =
        PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0).expect("bash available");
    let path = dir.path().join("shell.sock");
    let listener = host::bind(&path).await.expect("bind the shell socket");
    tokio::spawn(host::serve(listener, Arc::clone(&backend)));
    Host { path, _dir: dir }
}
```

`send_signal(&host.path, ShellSignal::Stop, Duration::from_secs(5))` and the
`Cont` form already work against this host (the tests near line 1049 use
them), and its `Status` reports `Paused` while a job is stopped.

## Spec

### Task 1 — Count a paused slice out of the deadline

In `read_to_marker`, restructure only the `Err(_) =>` arm (the slice
timed out), so that:

1. Record when the slice began. Add `let slice_started = tokio::time::Instant::now();`
   immediately **before** the `timeout_at(slice_end, …)` call.
2. In the `Err(_)` arm, do the **status check first**, then the deadline
   check:
   - **draining** (`drain_until` is `Some`): unchanged; past `end` →
     `ShellExited`.
   - **not draining**: call `status(sock, EXIT_STATUS_TIMEOUT).await` once and
     match it:
     - `Ok(Status { state: Exited, .. })` → set `drain_until` as today;
     - `Ok(Status { state: Paused, .. })` →
       `deadline += slice_started.elapsed();`, the time this slice spent
       paused, the same way prompt time is added back;
     - anything else → nothing.
   - **then** `if tokio::time::Instant::now() >= deadline { bail!(… timed out …) }`,
     with the message unchanged;
   - then `continue`.

Moving the deadline check after the status check matters. A command paused
across its deadline must have the paused slice added back **before** it is
judged.

Add a one-sentence comment on the `Paused` branch, in the voice of the
prompt-time comment: paused time is not the command's run time, so it is
added back to the deadline.

Change nothing else in the function: not the `Chunk` arm, not the prompt
handling, not `EXIT_POLL_INTERVAL`.

### Task 2 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib client_run_command_pause_does_not_count_against_the_deadline`
      reports `1 passed; 0 failed`.
- [ ] `cargo test --lib client_run_command_still_times_out_when_not_paused`
      reports `1 passed; 0 failed`.
- [ ] `cargo test --lib shell::client` reports `0 failed`, and no existing
      test there is edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/shell/client.rs` tests, next to the shell-exit tests:

- `client_run_command_pause_does_not_count_against_the_deadline`:
  1. `let host = spawn_host().await;`
  2. Start `run_command(&host.path, "sleep 1; echo PAUSED_OK", "bash", Duration::from_secs(2))`
     in a `tokio::spawn`, so it runs concurrently. Clone `host.path` into the
     task.
  3. `tokio::time::sleep(Duration::from_millis(300)).await;` then
     `send_signal(&host.path, ShellSignal::Stop, Duration::from_secs(5)).await.expect(…)`.
  4. Sleep **3 s**: longer than the 2 s limit, all of it paused.
  5. `send_signal(&host.path, ShellSignal::Cont, …)`.
  6. Await the task. The command **succeeds** and its output contains
     `PAUSED_OK`. Without the fix, it times out.
- `client_run_command_still_times_out_when_not_paused` (the negative case):
  `run_command(&host.path, "sleep 5", "bash", Duration::from_secs(1))` with no
  pause → `Err` whose message contains `timed out`, returned in **under 3 s**.
  The extension must apply only while paused.

Use real time (`#[tokio::test]`, not `start_paused`): the host is a real PTY.

**Reviewer mutation:** delete the `deadline += slice_started.elapsed();` line,
and the pause test must fail with `timed out`.

## End-to-end verification

```sh
cargo test --lib client_run_command_pause_does_not_count_against_the_deadline 2>&1 | grep -E '^test result'
cargo test --lib client_run_command_still_times_out_when_not_paused 2>&1 | grep -E '^test result'
cargo test --lib shell::client 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.

## Authorizations

None. The change is inside `read_to_marker` and its tests.

## Out of scope

- The run-level pause bookkeeping (`src/daemon/run/pause.rs`): unchanged.
- How often a quiet command polls `Status`. For a sandbox shell each poll is
  an in-VM `FIND`; that cost predates this phase and stays as it is.
- Any other timeout (prompt timeouts, ghost turn budgets).

## Update Log

### Update — 2026-10-07 02:50 (started)

**Executor:** Claude (Opus 4.6)
**Status:** in-progress.

Task 1: restructure `read_to_marker`'s timed-out-slice arm — status check
first, `Paused` adds the slice's span back to the deadline, deadline check
last. Task 2: run the phase's end-to-end block and paste its output.

### Update — 2026-10-07 02:56 (end-to-end verification)

The phase's § End-to-end verification block, run verbatim, redirected to
`.rexymcp/output/phase-05c-e2e.txt`; its contents, every line as printed:

```
$ cargo test --lib client_run_command_pause_does_not_count_against_the_deadline 2>&1 | grep -E '^test result'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2302 filtered out; finished in 3.31s
$ cargo test --lib client_run_command_still_times_out_when_not_paused 2>&1 | grep -E '^test result'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2302 filtered out; finished in 1.00s
$ cargo test --lib shell::client 2>&1 | grep -E '^test result'
test result: ok. 29 passed; 0 failed; 1 ignored; 0 measured; 2273 filtered out; finished in 3.35s
$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
$ cargo fmt --all --check; echo "fmt_exit=$?"
fmt_exit=0
$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2353 failed=0
$ git diff --stat
 docs/dev/milestones/M26-sbx-isolation/README.md    |  2 +-
 .../M26-sbx-isolation/phase-05c-pause-deadline.md  | 12 ++-
 src/shell/client.rs                                | 88 +++++++++++++++++++---
 3 files changed, 88 insertions(+), 14 deletions(-)
```

PASTE MATCH: yes

The two new tests behave as the spec's Test plan describes: the pause test
finishes in ~3.3 s — the command survives a pause longer than its whole 2 s
deadline and reports `PAUSED_OK` — and the unpaused `sleep 5` against a 1 s
deadline fails with `timed out` in ~1.0 s, under the 3 s bound. No existing
test in `shell::client` was edited (the full `shell::client` run is 29
passed, the same set plus the two new tests).

### Update — 2026-10-07 03:00 (complete, server-authored)

**Summary:** </think>Working tree is clean. All completion checklist items:

- [x] Phase doc's Status: `in-progress` (server flips to review)
- [x] Milestone README phase-table row: `in-progress`
- [x] No hand-written `(complete)` entry
- [x] cargo fmt --all run before git add
- [x] All verification commands ran clean
- [x] Final message substantive Summary + Notes for review
- [x] git status clean
- [x] git log -1 --stat shows all touched files

Now the final Summary + Notes for review message.</think>
**Summary + Notes for review**

Implemented phase-05c (Q6): `read_to_marker`'s timed-out-slice arm in `src/shell/client.rs` now polls the host's `Status` **before** judging the deadline, and on `ShellState::Paused` adds the slice's elapsed span back to the deadline (`deadline += slice_started.elapsed()`), exactly the idiom the prompt-answer wait already used; the drain/`ShellExited` handling, the bail message, `EXIT_POLL_INTERVAL`, and everything else in the function are untouched. Added the two spec-pinned tests next to the shell-exit tests — `client_run_command_pause_does_not_count_against_the_deadline` (a `sleep 1; echo PAUSED_OK` command with a 2 s limit, paused for 3 s, completes with its output) and `client_run_command_still_times_out_when_not_paused` (`sleep 5` vs a 1 s limit → `timed out` in ~1.0 s). No existing test was edited.

End-to-end verification ran verbatim as specified and is pasted in the phase's Update Log with `PASTE MATCH: yes`: pause test `1 passed; 0 failed` (3.31 s), negative test `1 passed; 0 failed` (1.00 s), `shell::client` `29 passed; 0 failed` (was 27 before the two additions), clippy exit 0, `fmt_exit=0`, full suite `passed=2353 failed=0`. Committed as `b9c5993` (`fix: pause time no longer counts against a command's deadline`); working tree is clean.

**Notes for review:** (1) The commit needed `git -c user.name/-c user.email` overrides because the repo has no configured identity — I used the author identity from the prior three commits (`Matt Ryanczak <matt@ryanczak.org>`); the host's git config may be worth fixing outside this phase. (2) One interpreter-side hiccup during the session (a stale scratch line `_slice_started` before the real one) triggered a compile error mid-run; it was fixed in the same session and the final committed code carries only the real binding — the end-to-end block above was run entirely after that fix, against the committed state. (3) The mutation check (deleting the `deadline +=` line makes the pause test fail with `timed out`) is logically implied by the negative test but was not itself run, as the phase doc's evidence block does not include it.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.40s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
r_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (314738): No such process
kill: (-315464): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M26-sbx-isolation/README.md` — +1 -1
- `docs/dev/milestones/M26-sbx-isolation/phase-05c-pause-deadline.md` — +44 -2
- `src/shell/client.rs` — +77 -11

**Commit:** b9c5993abf5848cf9bd416b04c4a04797b5ca5cb

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-07

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (62 turns,
  `b9c5993a`). No evidence-step loop.
- **Scope deviations:** none. Only `read_to_marker`'s timed-out-slice arm
  changed: the status check now comes first, a `Paused` status adds the
  slice back to the deadline, and the deadline is judged after. The
  `Chunk`/prompt handling and the constants are untouched, and no existing
  test was edited.
- **Reviewer re-run:** fmt, build and clippy clean; the pause test passes
  (3.31 s, longer than its 2 s limit, as intended); the negative test times
  out at 1.00 s; `shell::client` 29 passed; full suite `passed=2353 failed=0`
  on the first run.
- **Reviewer mutation check:** deleting `deadline += slice_started.elapsed();`
  makes the pause test fail with `timed out after 2s waiting for command
  output: "sleep 1; echo PAUSED_OK"`.
- **Note:** the status flip and server-authored completion entry were
  committed by the PE as `8b6e3370` (no code).
- **Calibration:** none.
