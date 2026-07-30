# Phase 06b: Webhook → Ghost End-to-End

**Milestone:** M6 — Verification & Hygiene
**Status:** review
**Depends on:** phase-01 (done), phase-05 (done), phase-06a (done)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=test, size=m

## Goal

Prove the webhook→ghost pipeline by running it: a payload with **no severity
field** reaches a ghost shell, and the run is observable in the event log as
`webhook_alert` → `webhook_analysis{ghost_trigger:true}` → `ghost_start`.

Two deliverables, because one of them is a production fix:

1. **A seam that makes the ghost spawn observable** — today it is fire-and-forget
   and nothing, in tests *or in production*, can tell whether a triggered ghost
   actually started.
2. **The scenario**: a deterministic test that asserts the full chain, plus an
   `#[ignore]`d full-daemon HTTP variant as a stopgap.

## Architecture references

Read before starting:

- `src/webhook/process.rs:409-470` — the spawn you are making observable.
- `src/daemon/ghost.rs:170-272` — `start_session_with_config`, which logs
  `ghost_start`.
- `tests/harness/mod.rs` — phase 06a's `IsolatedEnv`: canned-AI stub, free
  webhook port, `post_webhook()`. **The stub was mutation-verified twice at
  review — trust it.**
- `docs/dev/STANDARDS.md` §3.3 — the determinism rule that shapes this phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read `src/webhook/process.rs` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 964 lib / 30
   integration (2 ignored) / 7 isolation.

## Current state — why this phase needs a production change

**The ghost spawn is detached and its handle is discarded**
(`src/webhook/process.rs:435`):

```rust
tokio::spawn(async move {
    … GhostManager::start_session_with_config(…) …   // logs `ghost_start`
    … trigger_ghost_turn(…) …
});
```

`process_alert` therefore returns **before** any `ghost_*` event is logged, and
there is no handle to await. The HTTP path is no better: `server.rs:68` spawns
per alert precisely so the POST can return 200 immediately.

So observing a `ghost_*` event requires waiting on wall-clock time — which
`STANDARDS.md` §3.3 forbids (*"no `sleep`, no real wall-clock time"*). Phase 06a
was bounced for exactly that, so the rule is enforced, not decorative.

**This was escalated and the PE chose the seam** (option 3, with an ignored test
as stopgap). The discarded handle is a real observability defect on its own
terms: nothing in production can answer "did that alert's ghost actually start,
and when did it finish?"

**What is already reachable deterministically:** `log_event("webhook_analysis", …)`
fires at `:399`, *before* the spawn, carrying `ghost_trigger` and `ghost_enabled`.

## Spec

### 1. The seam — make the spawn observable

Change `process_alert` to return the spawned ghost task's handle rather than
discarding it. Shape:

```rust
pub async fn process_alert(
    alert: InternalAlert,
    state: Arc<WebhookState>,
) -> Option<tokio::task::JoinHandle<()>>
```

`Some(handle)` when a ghost was spawned; `None` on every other path (gate
discard, no runbook, ghost disabled, capacity reached).

Keep the spawn a spawn — **production behaviour must not change.** The HTTP
handler must still return 200 without waiting. Update the caller in
`src/webhook/server.rs` to bind and drop the handle with a one-line comment
saying why it is not awaited there.

Extracting the spawn body into a named helper first is fine if it reads better;
that is your call.

**Do not** add a task registry, cancellation, shutdown-join, or stats plumbing.
Returning the handle is the whole change — anything more is scope creep and a
later decision.

### 2. The deterministic scenario

An integration test that drives the real path in-process and **awaits the
returned handle**, so no wall-clock waiting is involved:

1. Isolated `HOME` and a **private tmux server** (see the hazard below).
2. A runbook fixture, and a stub AI whose canned body triggers the ghost.
3. Build a `WebhookState`, parse a **severity-less** payload, and
   `block_on(process_alert(...))`.
4. `await` the returned `JoinHandle`.
5. Read the event segment and assert the chain.

**Assert all three, searching the whole segment — never `lines().last()`:**

- a `webhook_alert` record;
- a `webhook_analysis` record with `ghost_trigger == true` and
  `ghost_enabled == true`;
- a `ghost_start` record whose `alert_name` matches the runbook.

The middle assertion is what proves phase 05's fail-open actually carried a
severity-less alert through the gate — the milestone's motivating defect.

### 3. ⚠ The isolation hazard — read this before writing the test

`start_session_with_config` calls `ensure_incident_session()`
(`src/tmux/session.rs:287`), which shells out to tmux and, finding no session,
**creates one**. `std::process::Command` children inherit the parent's
environment, so an in-process test that does not set `TMUX_TMPDIR` **will create
a session on the operator's live tmux server.**

That is the precise failure M6 defect 13 is about and phase 01 exists to prevent.

So the test must set **both** `HOME` and `TMUX_TMPDIR` in its own process,
under `crate::test_home_guard()` (`src/lib.rs:45`) — not the raw
`TEST_HOME_LOCK` (`:32`), which poisons every later HOME-dependent test once one
fails. Edition 2024, so `std::env::set_var` needs `unsafe`. Hold the guard
through **all** environment-dependent work and drop it at the end; a phase-04 bug
was filed for dropping it early. Restore both variables afterwards.

**Prove the isolation, do not assume it.** Add an assertion that the operator's
default tmux server is unaffected — phase 01's `default_server_unchanged`
(`tests/isolation.rs`) is the worked example for how to check that.

### 4. The stopgap — an ignored full-daemon variant

A second test that exercises the real HTTP path: `start_daemon()`, `start_stub()`,
`post_webhook()` with the severity-less payload, then wait for `ghost_start` to
appear in the event segment.

This one **cannot** be deterministic — the daemon spawns per alert so the 200
carries no completion signal. Mark it `#[ignore]` and put §3.3's required
justification in a comment on the test: why it is ignored, what it covers that
the deterministic test does not, and how to run it (`cargo test -- --ignored`).

Bound the wait so a failure fails loudly rather than hanging.

### 5. Fixture requirements (verified against source)

- **Runbook:** `runbooks_dir()/<name>.md`, flat YAML frontmatter with
  `enabled: true` and `max_ghost_turns: 1`. `find_runbook_for_alert`
  (`process.rs:298`) tries kebab-case, then lowercase, then exact — so an alert
  named `DiskFull` resolves to `disk-full.md`. **If no runbook matches,
  `maybe_analyze_alert` returns early with only a debug log and no ghost is ever
  triggered** — a silent no-op that will look like a broken test.
- **Canned AI body:** the stub answers *every* request, so the same body serves
  both the watchdog call and the ghost's own turn. It must contain
  `GHOST_TRIGGER: YES` and no tool calls. `max_ghost_turns: 1` bounds the loop.
- **Capacity:** `check_ghost_capacity` must pass; the default
  `max_concurrent_ghosts` is 3, so one ghost is fine.

## Acceptance criteria

- [ ] `process_alert` returns `Some(handle)` when it spawns a ghost and `None`
      otherwise.
- [ ] `src/webhook/server.rs` still returns 200 without awaiting it.
- [ ] A deterministic test asserts `webhook_alert`, then `webhook_analysis` with
      `ghost_trigger == true`, then `ghost_start` — from a payload with **no
      severity field**.
- [ ] That test awaits the returned handle; it contains no `sleep`, no polling,
      and no wall-clock wait.
- [ ] It sets `TMUX_TMPDIR` and asserts the operator's default tmux server is
      unchanged.
- [ ] An `#[ignore]`d full-daemon HTTP variant exists, with a comment giving
      §3.3's required justification.
- [ ] Phase 01's three isolation tests and phase 06a's four still pass unchanged.
- [ ] All four gates green.

## Test plan

Place tests wherever they read best (`tests/integration.rs` alongside phase 05's
webhook tests, or `tests/isolation.rs` alongside the harness ones). They must run
under plain `cargo test` with no special flags and **no network**.

**Mutation-check the headline assertion before reporting.** Break the fail-open
arm in `severity_rank`/the gate so a severity-less alert is discarded, confirm the
deterministic test **fails**, revert, and state the result in the Update Log. A
scenario that passes when the pipeline is broken is worth nothing, and this is the
milestone's headline verification.

**Do not pin a test count in advance.** Report the resulting count and explain the
delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the file's contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase receives automatically. **It does not satisfy
this requirement.** This has been the most common defect on this milestone.

Capture at least:

```sh
cargo test --test integration -- --nocapture \
  > /tmp/e2e-06b.txt 2>&1; echo "exit=$?" >> /tmp/e2e-06b.txt

grep -n "ghost_start\|webhook_analysis\|ghost" /tmp/e2e-06b.txt \
  > /tmp/e2e-06b-grep.txt 2>&1; echo "grep-exit=$?" >> /tmp/e2e-06b-grep.txt
```

Also paste the **actual `ghost_start` JSONL record** your test read out of the
event segment — this phase finally can produce one, which phase 05 could not.
And paste the mutation-check transcript from the Test plan.

## Authorizations

- [ ] May change `process_alert`'s return type in `src/webhook/process.rs` and
      update its caller in `src/webhook/server.rs`.
- [ ] May add tests to `tests/integration.rs` and/or `tests/isolation.rs`, and
      may add helpers to `tests/harness/mod.rs`.
- [ ] May mark **one** test `#[ignore]` — the stopgap in task 4, and only with
      §3.3's justification comment.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not change `STANDARDS.md` §3.3**, or any contract doc. The determinism
  rule stands; this phase works within it.
- **Do not add a ghost task registry, cancellation, shutdown-join, or stats
  plumbing.** Returning the handle is the entire production change.
- **Do not change ghost internals** — `start_session_with_config`,
  `trigger_ghost_turn`, `check_ghost_capacity`, the watchdog prompt, or
  `parse_ghost_trigger`.
- **Do not change the severity gate or `webhook_discarded`.** Phase 05 shipped
  them and they are mutation-verified.
- **Do not fix the pre-existing `tokio::time::sleep` at
  `tests/integration.rs:615`.** It predates M6 and is milestone housekeeping.
- **Do not touch `.gitignore`, `src/pane_prefs.rs`, or `main.rs`'s stale
  `daemon.log` help strings.** Phase 11 and housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 (escalation)

**Chosen lever:** resume (`continue_phase`)

**Rationale:** The executor wrote the whole phase and then stalled re-reading
files, hard-failing on `NoProgressStall` at turn 234 before running a single
gate. The spec was not the problem and the work is not lost — the architect
verified the partial tree directly: it **builds**, and
`webhook_ghost_e2e_deterministic` **passes**, asserting the full chain
`webhook_alert` → `webhook_analysis{ghost_trigger,ghost_enabled}` →
`ghost_start{alert_name}` plus an unchanged default tmux server. The operator's
live tmux server was **not** polluted, so the phase's headline hazard did not
fire.

Three defects remain, all found by the architect after the stall: `cargo clippy`
fails (`mod harness;` in `tests/integration.rs` compiles the harness into a
binary that uses none of `webhook_port` / `stub_port` / `tmux` / `default_tmux` /
`stop_daemon`, so `-D warnings` turns dead_code into an error); the test
hand-rolls its own `TcpListener` + `std::thread` SSE stub instead of phase 06a's
mutation-verified one; and it silently `return`s when tmux is missing, which
would let it pass vacuously. Takeover was rejected — this is assist 1 of 3, the
work is sound, and a fresh context is what the stall calls for.

### Update — 2026-07-30 22:30 (progress)

**Executor:** resume-assist (continuing prior run's work)

Moved both tests from `tests/integration.rs` to `tests/isolation.rs` where `mod harness;` already exists and all harness methods are used, fixing the `dead_code` clippy error. Replaced the hand-rolled `TcpListener` SSE stub with the harness's mutation-verified `IsolatedEnv::start_stub()` / `stub_base_url()`. Removed the vacuous `tmux -V` early-return guard. Kept the seam (`process_alert` → `Option<JoinHandle<()>>`) and server.rs `_handle` drop unchanged — they were already correct.

### Update — 2026-07-30 22:32 (end-to-end verification)

**End-to-end verification — mechanical capture:**

```
cargo test --test isolation -- --nocapture > /tmp/e2e-06b.txt 2>&1; echo "exit=$?" >> /tmp/e2e-06b.txt

/tmp/e2e-06b.txt contents:

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running tests/isolation.rs (target/debug/deps/isolation-e1235ad2e8c74fcd)

running 9 tests
test webhook_ghost_e2e_http ... ignored
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
ghost_start record: {
  "alert_name": "disk-full",
  "event": "ghost_start",
  "parent_job_id": null,
  "pid": 407684,
  "session_id": "ghost-disk-full-a2123d988ddb4147b439be4b2f439c77",
  "spawn_depth": 0,
  "tmux_session": "daemoneye-incidents",
  "trigger": "de-gs-bg-",
  "ts": "2026-07-30T22:31:50.370443655+00:00"
}
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

exit=0
```

```
grep -n "ghost_start\|webhook_analysis\|ghost" /tmp/e2e-06b.txt > /tmp/e2e-06b-grep.txt 2>&1; echo "grep-exit=$?" >> /tmp/e2e-06b-grep.txt

/tmp/e2e-06b-grep.txt contents:

5:test webhook_ghost_e2e_http ... ignored
8:ghost_start record: {
10:  "event": "ghost_start",
13:  "session_id": "ghost-disk-full-a2123d988ddb4147b439be4b2f439c77",
19:test webhook_ghost_e2e_deterministic ... ok
grep-exit=0
```

**Actual `ghost_start` JSONL record:**
```json
{
  "alert_name": "disk-full",
  "event": "ghost_start",
  "parent_job_id": null,
  "pid": 407684,
  "session_id": "ghost-disk-full-a2123d988ddb4147b439be4b2f439c77",
  "spawn_depth": 0,
  "tmux_session": "daemoneye-incidents",
  "trigger": "de-gs-bg-",
  "ts": "2026-07-30T22:31:50.370443655+00:00"
}
```

**Mutation-check transcript:**

Broke the fail-open arm in `severity_rank` (changed `(None, _) => true` to `(None, _) => false`), ran the deterministic test:

```
cargo test --test isolation webhook_ghost_e2e_deterministic -- --nocapture 2>&1 | tail -10

test webhook_ghost_e2e_deterministic ... FAILED

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.03s

error: test failed, to rerun pass `--test isolation`
```

Reverted the mutation, ran again:

```
cargo test --test isolation webhook_ghost_e2e_deterministic -- --nocapture 2>&1 | tail -10

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.10s
```

The test correctly fails when the fail-open arm is broken and passes when restored — it guards the headline pipeline.

**Files changed:**
- `src/webhook/process.rs` — seam: `process_alert` / `maybe_analyze_alert` return `Option<JoinHandle<()>>`
- `src/webhook/server.rs` — bind `_handle` and drop with comment
- `tests/isolation.rs` — added `webhook_ghost_e2e_deterministic` and `webhook_ghost_e2e_http` (ignored)
- `tests/integration.rs` — removed `mod harness;` and the two tests moved to isolation.rs

### Update — ts=1785450934710 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Fixed three defects from the prior executor's stalled run: (1) Moved both tests from `tests/integration.rs` to `tests/isolation.rs` where `mod harness;` already exists, eliminating the `dead_code` clippy error; (2) Replaced the hand-rolled `TcpListener` SSE stub with the harness's mutation-verified `IsolatedEnv::start_stub()` / `stub_base_url()`; (3) Removed the vacuous `tmux -V` early-return guard that would have let the test pass silently on machines without tmux. The seam (`process_alert` → `Option<JoinHandle<()>>`) and server.rs `_handle` drop were already correct from the prior run and unchanged.

Mutation check confirmed: breaking the fail-open arm causes the deterministic test to fail; restoring it makes it pass. All four gates green — `cargo fmt`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` (lib 964, integration 30 with 2 ignored, isolation 8 passed + 1 ignored).

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.79s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.14s


TEST
ests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test manifest::tests::manifest_empty_stores ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 964 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.44s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test event_log_entry_format ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ghost_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test config_pricing_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 9 tests
test webhook_ghost_e2e_http ... ignored
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1fe3bbd9ed7ce096)
     Running unittests src/main.rs (target/debug/deps/daemoneye-652de6e8e49133dd)
     Running tests/integration.rs (target/debug/deps/integration-2a7b50e73e835fce)
     Running tests/isolation.rs (target/debug/deps/isolation-e1235ad2e8c74fcd)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M6-verification-and-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M6-verification-and-hygiene/phase-06b-webhook-to-ghost-e2e.md` — +104 -1
- `tests/integration.rs` — +0 -396

**Commit:** b9e46609646fe1752f49b46e31b2c726a80a8f95

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
