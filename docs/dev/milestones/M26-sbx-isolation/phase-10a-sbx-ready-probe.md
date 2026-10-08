# Phase 10a: sbx-ready-probe — a sandbox shell host must pass the readiness probe

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** review
**Depends on:** phase-09c
**Estimated diff:** ~40 lines
**Tags:** language=rust, kind=bugfix, size=xs

## Goal

Found at the row-10 live close on pinky, 2026-10-08. A sandboxed ghost's
every command failed with:

```
Error: could not open a shell in sandbox de-ghost-…: shell s1 never became
ready at …/run/shells/s1.sock within 15s
```

while the host was healthy: its socket, lock and `agent@de-ghost-…` prompt
all appeared within 0.3 s of spawn, and the process stayed alive.

## Root cause

`crate::shell::launch::await_ready` (`src/shell/launch.rs`) sends
`ShellRequest::Status` and waits **200 ms** for the reply
(`tokio::time::timeout(Duration::from_millis(200), …)`). A plain PTY host
answers in microseconds. An sbx host's `SbxBackend::status`
(`src/shell/sbx_backend.rs`) answers only after running the in-VM FIND
script through `sbx exec`, measured at **0.24 s warm** against the live
host, and longer cold. So every probe times out and reconnects until the
15 s deadline, which `spawn_dedicated_for`, `shell_for_owner_in_sandbox`
and `start_background_in_sandbox_with` all treat as a dead host.

## Spec

Make the readiness probe tolerate a slow-but-healthy `Status` reply
without weakening the 15 s overall deadline or the "host never came up"
rollback. Choose the mechanism. Two that fit: a per-probe reply timeout
long enough for an sbx FIND (the probe already loops until the deadline),
or a cheap readiness answer that does not need the VM round trip. Do not
touch the vm-signal script or `SbxBackend`'s signal routing.

Add one test proving the fix: a `Status` reply that arrives after 500 ms
(a fake host, or a delayed responder) now passes `await_ready` within a
2 s deadline. Name it `await_ready_accepts_a_slow_status_reply`. Keep the
existing `await_ready` tests passing unchanged.

## Acceptance criteria

- [ ] `cargo test --lib await_ready_accepts_a_slow_status_reply` reports
      `1 passed`. Today it reports `0 passed`.
- [ ] `cargo test --lib shell::launch` reports `0 failed`, no existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`, and exactly one more passing test
      than today's 2297: `passed=2298`.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test --lib await_ready_accepts_a_slow_status_reply 2>&1 | grep -E '^test result'
cargo test --lib shell::launch 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines.

## Authorizations

- [x] May edit `src/shell/launch.rs`, and `src/shell/sbx_backend.rs` only
      if the chosen mechanism needs it.

## Out of scope

- The live re-run: the architect repeats the row-10 ghost check after this
  lands.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-08 12:56 (started)

Starting phase-10a. Chosen mechanism: lengthen `await_ready`'s per-probe reply
timeout — a slow reply arrives inside the existing connect loop, so the 15 s
overall deadline and the rollback on failure stay untouched, and
`SbxBackend::status` needs no change (the FIND round trip is the reply, just a
slower one). Next: implement, add
`launch_await_ready_accepts_a_slow_status_reply`, verify. — Executor (phase-10a)

### Update — 2026-10-08 13:00 (end-to-end verification)

Ran the phase's End-to-end verification block; every line as printed, each run
once:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2243 filtered out; finished in 0.53s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 2232 filtered out; finished in 1.02s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
fmt_exit=0
passed=2298 failed=0
 docs/dev/milestones/M26-sbx-isolation/README.md    |  2 +-
 .../M26-sbx-isolation/phase-10a-sbx-ready-probe.md | 11 +++-
 src/shell/launch.rs                                | 58 ++++++++++++++++++++--
 3 files changed, 66 insertions(+), 5 deletions(-)
```

PASTE MATCH: yes

Notes: the test count moved 2297 → 2298 (the one new test, no existing test
edited); `shell::launch` is 12 passed / 0 failed. — Executor (phase-10a)

### Update — 2026-10-08 20:07 (complete, server-authored)

**Summary:** </think>Everything is committed, working tree clean (git status --short printed nothing). Phase doc status is still in-progress (I did not flip to review), README row in-progress, no hand-written (complete) entry.

Final message: Summary + Notes for review.</think>
**Summary:** Fixed the sandbox readiness-probe bug in `await_ready` (`src/shell/launch.rs`): the per-probe `Status` reply timeout is now 2 s (constant `STATUS_REPLY_TIMEOUT`) instead of 200 ms, clamped to the caller's overall deadline via `tokio::time::timeout_at`, so an sbx host whose `Status` needs an in-VM FIND round trip (measured 0.24 s warm) passes while a dead host still rolls back at the original 15 s deadline. The chosen mechanism needed only `launch.rs` — `sbx_backend.rs` is untouched, as is the vm-signal script. Added the pinned test `launch_await_ready_accepts_a_slow_status_reply` (a real socket server whose reply lands 500 ms after the request, passing within a 2 s deadline); no existing test was edited.

**Notes for review:** One verifier-reported compile error during the run — `deadline.min(...)` produced a `std::time::Instant` where `timeout_at` needs a `tokio::time::Instant`, fixed with `tokio::time::Instant::from_std`; no other deviations from the spec. End-to-end verification ran once, verbatim, and is pasted into the phase doc's Update Log with `PASTE MATCH: yes`: the new test reports `1 passed`, `cargo test --lib shell::launch` reports `12 passed; 0 failed`, clippy and `fmt --check` exit 0, and the full suite moved exactly 2297 → 2298 passing / 0 failed. Status left at `in-progress` per the contract; the live row-10 ghost re-run stays with the architect.

**Progress note:** phase-10a complete.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.38s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_counts_are_accurate ... ok
test readme_tools_tables_match_the_code ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.41s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (1369738): No such process
kill: (-1370468): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M26-sbx-isolation/README.md` — +1 -1
- `docs/dev/milestones/M26-sbx-isolation/phase-10a-sbx-ready-probe.md` — +32 -1
- `src/shell/launch.rs` — +55 -3

**Commit:** 50892344cd8d2024033bcd80a8b862ac1bbbe098

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
