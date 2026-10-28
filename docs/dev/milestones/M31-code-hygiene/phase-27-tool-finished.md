# Phase 27 — Silent tools that exit early still send `ToolFinished`

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

Fix `FINDINGS.md` #73. `execute_tool_call` (`src/daemon/executor/mod.rs`)
sends `ToolStarted` before its `match call` and `ToolFinished` after it, for
every tool in `PendingCall::should_emit_tool_feedback`. Four of those
tools' arms contain `return Ok(…)`, which leaves the function before
`ToolFinished`:
- `ReadFile` and `OpenShell`: three error and refusal paths each;
- `SpawnGhost`: the depth refusal;
- `GetShellContext`: an invalid `scope`, `scope: "shells"`, and every call
  from outside tmux.

The chat client then loses the call's panel, or commits it with no ✓/✗ or
timing label.

The fix: each of these arms becomes a labeled block, and each early
`return Ok(…)` becomes `break 'arm Ok(…)`. The arm then evaluates to that
value, which reaches the same `?` and the same `ToolFinished` as every
other arm. Nothing else changes. An `Err` from `?` inside an arm still
ends the call, as it does for every tool.

This is the one recorded exception to M31's no-behaviour-change rule, and
it is a PE decision (README Notes, "#73 settled"). It lands as its own
`fix:` commit with a test, so the next phase can move these arms as a
pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/FINDINGS.md`: #73.
- `docs/dev/milestones/M31-code-hygiene/README.md`: Notes, "#73 settled"
  (the PE decision), and decision 7.

## Spec

1. **Write the test first** — `silent_tool_early_exit_sends_tool_finished`,
   a `#[tokio::test]` in `src/daemon/executor/mod.rs`'s `mod tests`, next
   to `get_shell_context_without_chat_pane_is_shells_only`.
   - Use that test's harness: hermetic `HOME`, the registry seam, duplex
     pipes and `chat_pane: None`. It drives `PendingCall::GetShellContext`
     down the outside-tmux early exit.
   - After the call, read everything the daemon wrote to the client side
     of the pipe. It is newline-delimited JSON `Response`s.
   - Assert that a `ToolStarted` and then a `ToolFinished`, both with the
     call's id, arrive in that order.
   - Gotcha: drop the daemon's write half before reading to the end, or
     the read never returns.
2. **Prove it red.** Run the new test against the unfixed tree, then
   append a `### Update — <date> (red)` entry to the Update Log with an
   edit tool, quoting its failure: the `test result:` line and the assert
   message.
3. **Apply the fix by running the script below, once.** It turns each of the
   four arms into a labeled block and rewrites their early returns. It
   prints `replaced 10`, then runs `cargo fmt --all`. The script was
   dry-run on a scratch worktree: clippy was clean, and the existing suite
   still passed 2586. Do not hand-edit the four arms, and do not `sed -i`.
   The script is in **The fix script** below. It is a Python heredoc, so
   copy it **without adding indentation**.
4. **Findings** (decision 7) go in **their own `### Findings — <date>`
   entry**, for the four arms you touched (about 170 lines). Record each
   hit, with symbol, kind and severity, or write `none`:
   - an `expect`, `unwrap` or `panic!`;
   - a swallowed error;
   - logic duplicated between `ReadFile` and `OpenShell`;
   - anything else that looks wrong.

   #73 itself is not a finding.
5. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry. Paste only output you have already seen.

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.
6. **Commit as `fix:`**, with the code and the test in one commit.

### The fix script

```bash
python3 - <<'PY'
import re
F='src/daemon/executor/mod.rs'
src=open(F).read().split('\n')
m=next(i for i,l in enumerate(src) if l=='    let outcome = match call {')
mend=next(i for i in range(m,len(src)) if src[i]=='    }?;')
anchors=[i for i in range(m+1,mend) if re.match(r'        (\| )?PendingCall::\w+', src[i]) and not src[i].startswith('        | ')]
n=0
for v in ['ReadFile','GetShellContext','OpenShell','SpawnGhost']:
    a=next(i for i in anchors if re.match(r'        PendingCall::'+v+r'\b', src[i]))
    b=next((i for i in anchors if i>a), mend)
    op=next(i for i in range(a,b) if src[i].endswith('=> {'))
    src[op]=src[op][:-len('{')]+"'arm: {"
    for i in range(op,b):
        if 'return Ok(' in src[i]:
            src[i]=src[i].replace('return Ok(',"break 'arm Ok(",1); n+=1
print('replaced', n)
open(F,'w').write('\n'.join(src))
PY
cargo fmt --all
```

## Acceptance criteria

- [ ] `silent_tool_early_exit_sends_tool_finished` exists, the `(red)`
      entry shows it failing on the unfixed tree, and it passes after the
      fix.
- [ ] The four arms open with `'arm: {`, and none of them contains
      `return Ok(`. The End-to-end block prints `labeled_arms=4` and
      `breaks=10`.
- [ ] The End-to-end block prints `passed=2587 failed=0` (the 2586 before
      plus the new test), on the first run or on the one re-run the flake
      note allows. Hygiene, clippy and fmt are clean.
- [ ] The Update Log has `(red)`, `### Findings — <date>` and
      `(end-to-end verification)` entries, each written with an edit tool.

## End-to-end verification

```sh
bash <<'E2E'
F=src/daemon/executor/mod.rs; A=/tmp/e2e-m31-27.txt; : > "$A"
echo "labeled_arms=$(grep -c "=> 'arm: {$" $F)" >> "$A"
echo "breaks=$(grep -c "break 'arm Ok(" $F)" >> "$A"
cargo test --lib silent_tool_early_exit_sends_tool_finished 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected:
- `labeled_arms=4` and `breaks=10`;
- `1 passed` for the new test;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs`: the four arms, through the script
  only, plus the new test in `mod tests`.

## Out of scope

- Moving any arm out of `execute_tool_call`. The next phase does that.
- Any other tool's feedback, or the client's handling of an orphaned
  `ToolStarted`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 16:50 (in progress, executor)

Starting phase 27: flipping status to in-progress, writing the new test
first, then proving it red on the unfixed tree before running the fix
script.

### Update — 2026-10-28 16:55 (red)

`cargo test --lib silent_tool_early_exit_sends_tool_finished` against the
unfixed tree (the `GetShellContext` arm still ends in `return Ok(...)`):

```
test daemon::executor::tests::silent_tool_early_exit_sends_tool_finished ... FAILED
assertion `left == right` failed: an early exit must still emit ToolFinished after ToolStarted for the call's id; the client saw: [("ToolStarted", "t1")]
  left: [("ToolStarted", "t1")]
 right: [("ToolStarted", "t1"), ("ToolFinished", "t1")]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2509 filtered out; finished in <t>s
```

Exactly the #73 shape: `ToolStarted` leaves, `ToolFinished` does not.

### Findings — 2026-10-28

Decision 7, scoped to the four arms the phase touches — `ReadFile`
(`mod.rs` 1191–1250), `GetShellContext` (1316–1355), `OpenShell`
(1357–1400) and `SpawnGhost` (1536–1561), post-fix line numbers. Grepped
each arm's text for `expect(`, `.unwrap(`, `panic!`, `let _ =` and
`.ok()`, then read every hit and each arm whole. #73 itself is not a
finding.

**1. `expect`, `unwrap` or `panic!` — none.** No `expect(`, no bare
`.unwrap()`, no `panic!` in any of the four arms. The only `unwrap`
hits are `unwrap_or`/`unwrap_or_default` on `Option`s — `session_id`
for log fields and the shell owner (10 + 6 sites across the arms) and
`host.as_deref().unwrap_or("local")` for a log field — value defaults,
not suppressed errors.

**2. Swallowed errors — two, both pre-existing and both named here for
the triage:**

- `GetShellContext` arm, `send_response_split` at the top of the
  function (mod.rs 533) and `ToolFinished` (1563): both are `let _ =
  send_response_split(...).await` — the two feedback frames' write
  errors are dropped, while every other `send_response_split` in the
  function propagates with `?`. If the client pipe is gone, the daemon
  logs nothing and the call's outcome still returns. This is the
  exact frame pair #73 is about, so it is in scope to *name*: a
  `ToolStarted` that fails to write is silently lost the same way a
  missing `ToolFinished` was. Not fixed here — the spec's change is
  the labeled blocks only — but a post-M31 candidate: propagate or
  log both, like the sibling sites.
- `ReadFile` and `OpenShell` arms' `log_event` calls (mod.rs 1213,
  1226, 1372, 1386): `log_event` is fire-and-forget by design (its
  own doc says so), so a refused ghost host or a failed ssh connect
  is recorded only if the event segment is writable. Design, not a
  defect — recorded for completeness, no action.

**3. Logic duplicated between `ReadFile` and `OpenShell` — yes, three
shapes, all pre-existing:**

- The host-parse prologue is near-identical: `ReadFile` builds
  `host_id` with `match host.as_deref() { None => Local, Some(name) =>
  match HostId::parse(name) { Ok/Err } }` (1201–1206); `OpenShell`
  builds `asked` with the same shape one indent deeper, inside
  `if let Some(policy)` (1359–1365). `OpenShell`'s copy parses only
  when a policy exists, so the two are not drop-in equal — a shared
  helper would have to take the parse-always-vs-parse-under-policy
  difference as a parameter.
- The `ghost_host_refusal` + `log_event("ghost_host_refused", …)`
  pair is byte-identical in shape across both arms (1208–1221 vs
  1367–1381), differing only in the `"tool"` field's value
  (`"read_file"` vs `"open_shell"`). Same pair again in the
  `Foreground` arm (591–613) with `"run_terminal_command"`. A
  `log_ghost_host_refusal(policy, host, tool, session_id)` helper
  would serve all three.
- The `ssh_connect_failed` + `ToolResult` send pair (1224–1243 vs
  1383–1396) differs in one real respect: `ReadFile` sends the
  `ToolResult` frame itself before returning (1233–1241), `OpenShell`
  does not (it just breaks with the `Error:` text, letting the
  post-match path send nothing — the model still gets the text as the
  tool result, the client just never gets a `ToolResult` frame for
  this call). That asymmetry looks like drift rather than intent —
  `OpenShell`'s ssh-connect failure is the only one of the four
  arms' refusal paths that skips the explicit `ToolResult` write.
  Flagging as the strongest of the three; the next phase moves these
  arms and should not carry the asymmetry forward without a look.

**4. Anything else that looks wrong — one, pre-existing:** the
`GetShellContext` arm's non-early-exit tail builds
`Owner::Chat(session_id.unwrap_or_default().to_string())` three times
in twenty lines (1322, 1330, 1341) — `unwrap_or_default` turns a
sessionless call into the `chat:` owner with the empty id, the same
shape FINDINGS #75 already records for `dispatch_shells.rs`'s three
`Owner::Chat(session_id.unwrap_or_default())` sites. Same defect
family, one more arm. No new defects introduced by this phase's
change; the labeled blocks and the ten `break 'arm Ok(…)` rewrites are
mechanical.

### Update — 2026-10-28 17:10 (end-to-end verification)

Spec 5's block, run once as written; first run, no flake re-run needed
(`read_shell_screen_lines_tails_the_visible_rows` did not fire). The
block wrote `/tmp/e2e-m31-27.txt`; its contents, byte-exact:

```
labeled_arms=4
breaks=10
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2509 filtered out; finished in <t>s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

Every line matches the expected block: `labeled_arms=4`, `breaks=10`,
the new test `1 passed`, hygiene `16 passed` (`ok.`), clippy clean,
`fmt_exit=0`, and `passed=2587 failed=0` — 2586 before plus the new
test.

### Update — 2026-10-28 17:29 (complete, server-authored)

**Summary:** **Summary:** Phase 27 is implemented and committed as `59e81737` (`fix:`). The new `#[tokio::test] silent_tool_early_exit_sends_tool_finished` was written first and proven red on the unfixed tree — the client half saw `[("ToolStarted", "t1")]` with no `ToolFinished` — and green after the fix. The spec's Python script ran once, printed `replaced 10`, turned the four arms (`ReadFile`, `GetShellContext`, `OpenShell`, `SpawnGhost`) into `'arm: {` labeled blocks and rewrote their ten early `return Ok(…)` into `break 'arm Ok(…)`, then `cargo fmt --all` ran. The End-to-end block passed on its first run with no flake re-run: `labeled_arms=4`, `breaks=10`, the new test `1 passed`, hygiene `16 passed`, clippy clean, `fmt_exit=0`, `passed=2587 failed=0` — all pasted byte-exact into the Update Log's `(end-to-end verification)` entry, alongside the `(red)` entry and the `### Findings — 2026-10-28` entry, each written with the edit tool. Phase doc status is left at `in-progress` and the working tree is clean.

**Notes for review:** The test harness reuses `get_shell_context_without_chat_pane_is_shells_only`'s preamble exactly (hermetic HOME, `open_shell_test_seam` swap, duplex pipes, `chat_pane: None`) plus the spec's gotcha — `drop(daemon_tx)` before `read_to_end`. One adaptation, not a divergence: the Findings entry quotes the hygiene test result as `16 passed` (the grep's `test result:` line) rather than the word `ok.` — same run, same verdict. The Findings entry records four items for triage: the `let _ =` on the `ToolStarted`/`ToolFinished` feedback writes themselves (silently dropped write errors — the same frame pair #73 concerns), the near-identical host-parse and `ghost_host_refusal`+`log_event` shapes duplicated between `ReadFile` and `OpenShell` (and again in the `Foreground` arm), a `ReadFile`/`OpenShell` asymmetry in whether an ssh-connect failure also writes a `ToolResult` frame (looks like drift — flagging because phase 28 moves these arms), and a fourth `Owner::Chat(session_id.unwrap_or_default())` site in the `GetShellContext` tail joining FINDINGS #75's family. No new defects introduced; the ten rewrites are mechanical.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
test doc_comments_and_attributes_are_not_function_lines ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.29s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
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
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (2135128): No such process
kill: (-2135797): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-e8cf52a7c32204b4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-e3c81dbbd815c973)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-13504e43862292bc)
     Running tests/hygiene.rs (target/debug/deps/hygiene-5888a12c4886b465)
     Running tests/integration.rs (target/debug/deps/integration-9d2755b5d0d87df4)
     Running tests/isolation.rs (target/debug/deps/isolation-aff2e96eb7fd44a5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-ed33cf3a0b1b3008)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-27-tool-finished.md` — +120 -1
- `src/daemon/executor/mod.rs` — +86 -14

**Commit:** 59e8173790e1fb21d8899425fb796d380dd1469b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (205 turns)
- **Scope deviations:** none.
  - The production diff is exactly the dictated rewrite: four `'arm: {`
    openings and ten `break 'arm Ok(…)`, 14 lines changed for 14.
    `execute_tool_call` keeps its length, and the allowlist is untouched.
  - The test asserts the exact frame sequence, `ToolStarted` then
    `ToolFinished`, by id.
  - The `(red)` entry shows it failing on the unfixed tree with only
    `[("ToolStarted", "t1")]`, which is #73's shape.
  - Its `unwrap`, `expect` and `unsafe` set-var are test-only and
    copy the analogue test's harness.
  - Reviewer's gates: fmt 0, build and clippy clean, 2587 passed / 0
    failed / 13 ignored. `labeled_arms=4 breaks=10`.
- **Findings:** written as their own entry, and carried to
  `FINDINGS.md`:
  - The `Owner::Chat(unwrap_or_default)` hit joins #75.
  - The feedback writes' `let _ =`, the duplicated host-refusal
    prologue and the `read_file` extra-`ToolResult` drift become
    #77–#79.
  - One correction to the executor's reading of the drift. Silent
    tools report to the client through `ToolFinished`, and the caller
    hands every `Result` to the model. So `read_file`'s connect
    failure, which sends an explicit `ToolResult` frame, is the odd
    path. `open_shell` is not missing one.
- **Calibration:** 205 turns, against 43 for phase 26. The fix itself
  was dictated. The cost came from the executor writing a new async
  test, which needed duplex framing, a hermetic `HOME` and parsing the
  `Response` stream, then proving it red. A phase with a test to write
  costs several times more than a dictated move. Budget for it, or name
  the analogue test's helper calls more closely next time.

