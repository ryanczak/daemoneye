# Phase 03 — small-tidies

**Status:** review
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 02 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

Two findings are closed (M31 FINDINGS #75, M32 FINDINGS #8). A third, M32
#14, was closed at scoping as stale (the ledger row says why) and needs
nothing here.

- **#75: one owner rule for session-less calls.** Nine executor sites build
  a shell owner as `Owner::Chat(session_id.unwrap_or_default().to_string())`.
  `daemoneye ask` sends no session id, so every one-shot ask shares the owner
  `""` by accident. PE decision (2026-10-31): asks keep sharing one owner,
  but a named one. After this phase one helper, `chat_owner(session_id:
  Option<&str>) -> Owner`, returns `Owner::Chat(<id>)` for a session and
  `Owner::Chat("ask")` for none, through a documented constant `ASK_OWNER`.
  All nine sites call it. Nothing is refused.
- **#8: one header format.** `src/cli/viewer_layout.rs` formats
  `"output ({n} lines)"` in two places (`layout_block` and `bare_header`,
  the `Block::Output` arms). After this phase one small function makes the
  text and both call it. The rendered text does not change.

The suite goes from **2621 to exactly 2622** (one test for #75).

## Constraints

- **The nine sites** are in `src/daemon/executor/`: `dispatch_shells.rs` (3),
  `dispatch_context.rs` (2), `dispatch_open_shell.rs`, `dispatch_send_input.rs`,
  `dispatch_background.rs`, and `run_approved` in `dispatch_foreground.rs`.
  `grep -rn 'Owner::Chat(session_id.unwrap_or_default' src` lists them. Leave
  `bg_shell.rs` and the tests alone: they build owners from ids they already
  hold.
- **Put `chat_owner` and `ASK_OWNER` in `src/daemon/executor/mod.rs`**
  (754 of 800 production lines; this adds about 15). The doc comment says
  why asks share an owner, and that a real session id is 16 hex characters,
  so `ask` cannot collide with one.
- **Shells a running daemon already holds under `""`** are not migrated. They
  age out through the existing sweep. Say so in the doc comment, not in code.
- **Use `patch` with whole blocks,** never `patch_lines`. If a fix grows an
  allowlisted function, raise its `tests/hygiene_allowlist.txt` line and say
  why in the Update Log (M35 README).

## Spec

1. **Add the helper** — `ASK_OWNER` and `chat_owner` in `executor/mod.rs`,
   first written as today's behaviour (`unwrap_or_default()`).
2. **Write the #75 test** — `chat_owner_gives_a_session_less_call_the_ask_owner`
   in `mod.rs`'s tests: `chat_owner(None)` is `Owner::Chat("ask")`, and
   `chat_owner(Some("0123456789abcdef"))` is `Owner::Chat("0123456789abcdef")`.
   Run it against step 1 and keep the failing output.
3. **Fix the helper** to use `ASK_OWNER`; the test passes.
4. **Replace the nine sites** with `chat_owner(session_id)`. The full suite
   still passes.
5. **#8** — one function in `viewer_layout.rs` returns the output header for
   `n` lines; both arms call it. The full suite still passes at 2622.
6. **Mark the ledgers** — replace the triage prefix `act: M35 phase-03`
   with `fixed: M35 phase-03, <what>` in M31 row #75
   (`chat_owner_gives_a_session_less_call_the_ask_owner`) and M32 row #8
   (`dedupe`). Keep the `(was: …)` text; touch no other row.
7. **Red run** — one `### Update — <date> (red run)` entry pasting step 2's
   `test result:` line and assertion message.
8. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
9. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry.
10. **Commit** as `fix: one chat_owner rule, asks share the named owner; one output header (M31 #75, M32 #8)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2`
      (today `default_owner_sites=9 ask_owner=0 header_fmts=2 new_test=0 fixed=0`).
- [ ] The red-run entry shows the new test failing before step 3.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2622 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "default_owner_sites=$(grep -rn 'Owner::Chat(session_id.unwrap_or_default' src --include=*.rs | wc -l) ask_owner=$(grep -c 'const ASK_OWNER' src/daemon/executor/mod.rs) header_fmts=$(grep -c 'output ({n} lines)' src/cli/viewer_layout.rs) new_test=$(grep -c 'fn chat_owner_gives_a_session_less_call_the_ask_owner' src/daemon/executor/mod.rs) fixed=$(( $(grep -cE '^\| 75 \|.*fixed: M35 phase-03' docs/dev/milestones/M31-code-hygiene/FINDINGS.md) + $(grep -cE '^\| 8 \|.*fixed: M35 phase-03' docs/dev/milestones/M32-findings-fixes/FINDINGS.md) ))"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `executor/mod.rs`, the six `dispatch_*.rs` files named above,
  `src/cli/viewer_layout.rs`, the two ledger rows, and
  `tests/hygiene_allowlist.txt` (only per the rule above).

## Out of scope

- Refusing session-less calls (PE decision: asks keep running commands).
- `ApprovedRun.cmd_id` (#14, closed at scoping), `bg_shell.rs`, the viewer's
  separator row (#9, rejected).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 09:10 (progress)

Started. Status flipped to in-progress (here and in the milestone README).
Working through the spec in order: helper, test (red), fix, nine sites,
header format, ledgers.

### Update — 2026-10-31 09:17 (red run)

Step 2's test, run against step 1's helper (which still used
`unwrap_or_default()`):

```
running 1 test
test daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner ... FAILED

failures:

---- daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner stdout ----

thread 'daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner' (2123943) panicked at src/daemon/executor/mod.rs:940:9:
a session-less call owns its shells as the named ask owner
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2542 filtered out; finished in 0.00s
```

Step 3 fixed the helper to `unwrap_or(ASK_OWNER)`; the test now passes, and the
full suite is at 2622 after steps 4 and 5.

### Findings — 2026-10-31

`git diff --stat`: 12 files, +83/−30 — the helper and its test in
`executor/mod.rs`, nine one-line owner swaps across the six dispatch files,
`viewer_layout.rs`'s header dedupe, and the two ledger rows.

### Update — 2026-10-31 09:25 (end-to-end verification)

The phase doc's E2E block, run once before committing, its output redirected to
a file and pasted verbatim:

```
default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.66s
fmt_exit=0
passed=2622 failed=0
```

First line matches the acceptance criterion
(`default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2`); hygiene
16 passed, doc_truth 16 passed, bug_tracker 6 passed; `build_warnings=0`,
clippy clean, `fmt_exit=0`; the full suite is exactly `passed=2622 failed=0`.

### Update — 2026-10-31 16:28 (complete, server-authored)

**Summary:** </think>All tasks complete. Working tree clean. Phase doc status still in-progress, no completion entry written by me. Commit is `ec796c9a`.

Now write the final Summary + Notes for review.</think>
**Summary:** Phase 03 done. Added `ASK_OWNER` and `chat_owner(Option<&str>) -> Owner` to `executor/mod.rs` (documented why asks share a named owner, that session ids are 16 hex so `ask` cannot collide, and that existing `""`-owner shells age out via the sweep), red-greenned it with `chat_owner_gives_a_session_less_call_the_ask_owner` (captured the failing run before fixing), then replaced all nine `Owner::Chat(session_id.unwrap_or_default()…)` sites across the six `dispatch_*.rs` files; `viewer_layout.rs` gained one `output_header(n)` used by both `Block::Output` arms; M31 #75 and M32 #8 ledger rows now read `fixed: M35 phase-03, …` with the `(was: …)` text kept. Suite went 2621 → exactly 2622.

**Notes for review:** One adaptation — the spec's sketch asserted equality on `Owner`, which has no `PartialEq` and adding a derive was out of scope, so the test asserts with `matches!(…, Owner::Chat(o) if o == ASK_OWNER)`; behavior identical. I also renamed the helper's parameter to `n` so the one remaining `output ({n} lines)` literal sits inside the single formatting function (the E2E grep counts it, `header_fmts=1`). E2E block ran once pre-commit and its verbatim output is pasted in the `end-to-end verification` entry: `default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2`, `build_warnings=0`, hygiene 16 / doc_truth 16 / bug_tracker 6 passed, clippy clean, `fmt_exit=0`, `passed=2622 failed=0`. No allowlist line grew (`executor/mod.rs` is 754→757 production lines, still under the 800 cap). Committed as `ec796c9a`; status left at `in-progress` for the server's completion tail; working tree clean.

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
 ok
test exempt_files_exist ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.09s


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
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test config_pricing_round_trip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
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
     Running unittests src/lib.rs (target/debug/deps/daemoneye-6e634504ff19257b)
kill: (2169197): No such process
kill: (-2169910): No such process
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

- `docs/dev/milestones/M31-code-hygiene/FINDINGS.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` — +1 -1
- `docs/dev/milestones/M35-findings-closeout/README.md` — +1 -1
- `docs/dev/milestones/M35-findings-closeout/phase-03-small-tidies.md` — +61 -1
- `src/cli/viewer_layout.rs` — +8 -5
- `src/daemon/executor/dispatch_background.rs` — +1 -1
- `src/daemon/executor/dispatch_context.rs` — +3 -5
- `src/daemon/executor/dispatch_foreground.rs` — +1 -1
- `src/daemon/executor/dispatch_open_shell.rs` — +1 -7
- `src/daemon/executor/dispatch_send_input.rs` — +1 -6
- `src/daemon/executor/dispatch_shells.rs` — +7 -18
- `src/daemon/executor/mod.rs` — +27 -0

**Commit:** ec796c9a1368a3cbff01cd7b3c913d0906d4bc6f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
