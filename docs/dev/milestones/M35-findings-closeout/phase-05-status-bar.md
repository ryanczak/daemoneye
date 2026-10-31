# Phase 05 — status-bar

**Status:** done
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 04 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

M29's O6 is closed. Two defects in the chat client, both seen live:

- **The session name vanishes under a prompt panel.** While an approval,
  credential or picker panel holds the live region, the status bar reads
  `session: · <model> · up …`, with the id blank. Its source is
  `prompt_status` in `src/cli/commands/stream_ask.rs`: it builds the
  `StatusBarState` every prompt panel draws over with `session_id: ""`, and
  its doc comment calls that deliberate. After this phase `prompt_status`
  takes the turn's session id (`ask_with_session_ratatui` already has it as
  `session_id: Option<&str>`; `None` stays `""`), so the bar names the
  session, as it does between turns. Correct the doc comment.
- **A resumed chat counts turns from 1.** `run_chat_ratatui`
  (`src/cli/commands/chat_loop.rs`) starts `let mut turn: usize = 0` and
  labels each echoed turn `turn {n} · …`, so `chat --resume <id>` labels its
  first turn `turn 1` whatever the session's history. After this phase a
  resumed chat seeds the counter from the daemon's count before its first
  turn: one `Request::QueryLimits { session_id }` (the request `/limits`
  uses; it answers `Response::LimitsInfo { turn_count, .. }`). A fresh chat
  sends nothing and starts at 0, as today. If the request fails or answers
  anything else, the counter starts at 0. That is today's behaviour, and the
  chat must not fail over a label.

The suite goes from **2626 to exactly 2628**.

## Constraints

- **Test seams, as pure functions:**
  - `prompt_status` already is one. Give it the session id as a parameter.
  - Put the seeding decision in a small function in `chat_loop.rs`,
    `resumed_turn(reply: anyhow::Result<Response>) -> usize`:
    `Ok(LimitsInfo { turn_count, .. })` gives `turn_count`; anything else
    gives 0. The I/O stays at the call site.
- **Where the seed comes from:** `run_chat_inner` (`src/cli/commands/chat.rs`)
  knows whether this is a resume (`resume_id`). Carry the seeded number into
  `run_chat_ratatui` through `RatatuiCtx` (a new field), and start `turn`
  from it.
- **The allowlist:** `run_chat_ratatui` (500), `ask_with_session_ratatui`
  (673) and `read_input_line_inner_ratatui` (183) are allowlisted. If a fix
  grows one, raise its `tests/hygiene_allowlist.txt` line and say why in the
  Update Log (M35 README).
- **Use `patch` with whole blocks,** never `patch_lines`, on these long
  files.

## Spec

1. **Session id, red first** — give `prompt_status` a `session_id: &'a str`
   parameter but keep building the state with `""`. Pass the turn's id at
   every call site (`session_id.unwrap_or("")`). Write
   `prompt_status_names_the_session` in `stream_ask.rs`'s tests:
   `prompt_status(.., "0123456789abcdef", ..)` gives a state whose
   `session_id` is `"0123456789abcdef"`. Run it, keep the failing output, then
   use the parameter. The test passes.
2. **Turn seed, red first** — add `resumed_turn` returning 0 always, and write
   `a_resumed_chat_seeds_its_turn_from_the_daemon`: a `LimitsInfo` with
   `turn_count: 7` gives 7, and an `Err` and a `Response::Error` each give 0.
   Run it, keep the failing output, then implement it. The test passes.
3. **Wire the seed** — on a resume only, `run_chat_inner` sends one
   `QueryLimits` for the session id through the existing one-shot request
   helper, passes the reply through `resumed_turn`, and hands the number to
   `run_chat_ratatui` through `RatatuiCtx`.
4. **Mark M29** — append ` **Fixed in M35 phase-05.**` to the end of the O6
   item's text in `docs/dev/milestones/M29-post-2.0-fixes/README.md`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting steps 1
   and 2's `test result:` lines and assertion messages.
6. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
7. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry. This
   step is required. The reviewer checks for the entry.
8. **Commit** as `fix: the status bar names the session under a prompt; a resumed chat counts on (M29 O6)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `blank_session=0 seed_fn=1 new_tests=2 o6_fixed=1`
      (today `blank_session=1 seed_fn=0 new_tests=0 o6_fixed=0`).
- [ ] The red-runs entry shows both new tests failing before their fixes.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2628 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "blank_session=$(grep -c 'session_id: "",' src/cli/commands/stream_ask.rs) seed_fn=$(grep -c 'fn resumed_turn' src/cli/commands/chat_loop.rs) new_tests=$(( $(grep -c 'fn prompt_status_names_the_session' src/cli/commands/stream_ask.rs) + $(grep -c 'fn a_resumed_chat_seeds_its_turn_from_the_daemon' src/cli/commands/chat_loop.rs) )) o6_fixed=$(grep -c 'Fixed in M35 phase-05' docs/dev/milestones/M29-post-2.0-fixes/README.md)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/stream_ask.rs`, `chat_loop.rs`, `chat.rs` (the
  resume seed and `RatatuiCtx`), the O6 item of the M29 README, and
  `tests/hygiene_allowlist.txt` (only per the rule above).

## Out of scope

- `/session load`: it loads history into the current session id and does not
  switch sessions, so its turn label is a separate question.
- The `daemon_up` and `tools_total` fields of `prompt_status`; the status bar
  does not draw them.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 14:24 (started)

Executor: Claude (Opus 4.6). Starting phase 05: `prompt_status` takes the turn's
session id, `resumed_turn` seeds the turn counter from the daemon's
`LimitsInfo`, wired through `RatatuiCtx`. Red-first on both tests.

### Update — 2026-10-31 14:41 (red runs)

Both new tests failed on the pre-fix code, each captured with the fix
mechanically reverted and the test kept:

```
thread 'cli::commands::stream::ask::tests::prompt_status_names_the_session' panicked at src/cli/commands/stream_ask.rs:716:9:
assertion `left == right` failed
  left: ""
  right: "0123456789abcdef"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2548 filtered out; finished in 0.00s
```

```
thread 'cli::commands::chat::chat_loop::tests::a_resumed_chat_seeds_its_turn_from_the_daemon' panicked at src/cli/commands/chat_loop.rs:738:9:
  left: 0
  right: 7
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2548 filtered out; finished in 0.00s
```

Both pass after the fix (`test result: ok. 1 passed; …` each).

### Findings — 2026-10-31

From `git diff --stat` alone: no file moved and no allowlist line grew; the
phase touched only the three named sources and three docs. none.

### Update — 2026-10-31 14:52 (end-to-end verification)

The phase's E2E block, run once against the working tree before commit; output
redirected to a file and pasted verbatim:

```
blank_session=0 seed_fn=1 new_tests=2 o6_fixed=1
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.79s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2628 failed=0
```

First line matches the acceptance criterion (`blank_session=0 seed_fn=1
new_tests=2 o6_fixed=1`); hygiene, doc_truth and bug_tracker pass; clippy clean
(no diagnostics, `Finished` only); `fmt_exit=0`; `cargo test` reports exactly
`passed=2628 failed=0`.

### Update — 2026-10-31 21:47 (complete, server-authored)

**Summary:** Both M29 O6 defects are fixed. `prompt_status` in `stream_ask.rs` now takes the turn's session id (`session_id.unwrap_or("")` at all nine call sites, `None` still renders `""`), so the status bar names the session under an approval, credential or picker panel instead of reading `session: · <model>`. `run_chat_ratatui` seeds its turn counter from a new `RatatuiCtx.turn_seed` field: on a resume, `run_chat_inner` sends one `QueryLimits` through the existing `request_once` helper and feeds the reply through the new pure `resumed_turn` in `chat_loop.rs` (`LimitsInfo` → `turn_count`; `Err` or any other reply → 0, the chat never fails over a label); a fresh chat sends nothing and starts at 0. Both tests were written red-first and captured failing before their fixes; M29's O6 deferred item is marked `**Fixed in M35 phase-05.**`

**End-to-end verification:** the phase's E2E block ran once against the working tree and is pasted verbatim in the phase doc's `### Update — 2026-10-31 14:52 (end-to-end verification)` entry — first line `blank_session=0 seed_fn=1 new_tests=2 o6_fixed=1`, `build_warnings=0`, hygiene/doc_truth/bug_tracker all `ok`, clippy clean, `fmt_exit=0`, `cargo test` exactly `passed=2628 failed=0` (2626 → 2628).

**Notes for review:**
- One deviation: my first wiring grew `run_chat_ratatui` to 501 lines against its allowlisted 500. Rather than raise the allowlist line (the M35 rule permits growth with a logged why), I removed a duplicated comment line (`// Wait for tmux client attachment.` appeared twice in a row) that the function had carried, bringing it back to 500 — no allowlist change needed, `tests/hygiene_allowlist.txt` untouched.
- Mid-phase the verifier caught two transient breakages I introduced (missing imports/signature drift after editing `chat.rs`, and a botched `patch` that briefly dropped an `if should_echo(&query) {` line); both were restored and the final tree builds clean with zero warnings, so nothing of those remains.
- `resumed_turn` is `pub(super)` and matches the spec's signature `resumed_turn(reply: anyhow::Result<Response>) -> usize`, spelled `crate::ipc::Response` because `chat_loop.rs` reaches `ipc` through the full path (no `use crate::ipc` in that file's scope).
- Status left at `in-progress` as required; the server authors the completion flip and entry. Working tree is clean, commit `0f6c3c8f` contains all six touched files.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST

test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.90s


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
test config_pricing_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
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
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
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
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-6e634504ff19257b)
kill: (2635736): No such process
kill: (-2636444): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-d575d0b2c2ae3264)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-b278ad2979a68876)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-9c63a2decda31dcd)
     Running tests/hygiene.rs (target/debug/deps/hygiene-aab856d6777ceb37)
     Running tests/integration.rs (target/debug/deps/integration-d3f77f8c074697c0)
     Running tests/isolation.rs (target/debug/deps/isolation-9e7c6a22e4706ca4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-0968df9ce6900eb4)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-01205396fa4dcbd5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M29-post-2.0-fixes/README.md` — +1 -1
- `docs/dev/milestones/M35-findings-closeout/README.md` — +1 -1
- `docs/dev/milestones/M35-findings-closeout/phase-05-status-bar.md` — +55 -1
- `src/cli/commands/chat.rs` — +22 -1
- `src/cli/commands/chat_loop.rs` — +32 -2
- `src/cli/commands/stream_ask.rs` — +33 -4

**Commit:** 0f6c3c8fbef3bf263fb95e962fd2af6f75055f88

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-31

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none. A duplicated `// Wait for tmux client attachment.` comment line in `chat_loop.rs` was dropped; the full comment below it stays.
- **Calibration:** all entries are present, including the end-to-end one that phase 04 skipped. Marking that step "required" in the spec worked. Both red runs are real: the session id was `""` against `"0123456789abcdef"`, and the seed was `0` against `7`. The live behaviour (the session name under an approval panel, and the turn number after `chat --resume`) is checked at the milestone close. The reviewer stripped executor reasoning that had leaked into the completion summary.
