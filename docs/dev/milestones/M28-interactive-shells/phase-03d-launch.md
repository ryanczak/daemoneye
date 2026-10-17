# Phase 03d: launch — an interactive command starts in its shell and returns the screen

**Milestone:** M28 — Interactive shells
**Status:** done
**Depends on:** phase-03c
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Since phase 02c, `run_terminal_command("ssh host")` is refused: an
interactive command never exits, so the marker protocol would wait for
ever. The tools to drive such a session now exist (`send_input` with keys,
`read_shell_screen` with waits, the busy-shell guard), so the refusal can
become a **launch**:

1. write the bare command and Enter into the shell, with no markers;
2. wait for the screen to settle;
3. return what holds the shell and the screen, and wait for nothing more.

The shell stays busy while the session runs, so later commands go to
another shell. No tool parameter changes here; `interactive: true` for any
command is phase 03e.

## Architecture references

- `docs/design/interactive-shells.md` D6: the bare command plus `\r`, no
  markers, settle, then the `Foreground` plus the masked screen. Never wait
  for an exit, never signal the foreground group. A command
  `is_interactive_command` recognises is upgraded to this path.

## Spec

**Files:** `src/daemon/executor/shell_exec.rs` and
`src/daemon/executor/knowledge/shell.rs` (visibility only). Do not change
`is_interactive_command`, the `wait: false` path (it still refuses an
interactive command) or any tool definition.

1. **`launch_interactive`** — a new
   `async fn launch_interactive(reg, id: &ShellId, sock: &Path, cmd: &str) -> anyhow::Result<ShellRunResult>`
   in `shell_exec.rs`. In order:
   1. send `cmd` followed by `\r` with `crate::shell::send_input`, with no
      marker wrapping;
   2. call `knowledge::read_shell_screen_in(reg, id, None, None, None, false, None, Some(500), Some(5), <the standard screen timeout>)`.
      That waits up to 5 s for the screen to settle, records the shell's
      `last_foreground`, and returns the masked screen with its foreground
      header. Widen `read_shell_screen_in` to `pub(crate)` (and re-export it
      from `knowledge`) for this. Do not copy its logic;
   3. return `ShellRunResult { text, exit_code: 0 }`, where `text` is one
      header line followed by that screen read. The header line:
      `` [Interactive session started] `<cmd>` holds shell <id>; nothing waits for it. Type into it with send_input(shell: "<id>") and read it with read_shell_screen(shell: "<id>"). Later commands run in another shell while it is busy. ``

   The text names no exit code. `exit_code: 0` exists only for the stats
   and the shell-command log, which have no field for "did not exit".
2. **The branch.** In `run_in_shell_core` — the branch is the one
   `if crate::daemon::utils::is_interactive_command(cmd) {` in that
   function, and the resolved shell is bound above it by
   `let (id, socket) = match target {`, so pass `&id` and `&socket` — the
   `is_interactive_command(cmd)` branch calls `launch_interactive` with the
   resolved `id` and socket, instead of `bail!`ing. It stays where it is,
   after target resolution and the named-shell busy probe, so a named busy
   shell is still refused before anything is written. The comment above
   the branch says that the command is now launched, not refused.
3. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03d.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Gotchas.**
- These two 02c tests change, and that list is complete:
  - `shell_exec_interactive_returns_without_running_the_command` is
    **renamed** `run_terminal_command_launches_an_interactive_command`.
    `ssh nowhere.invalid` now returns `Ok` within 10 s, and its text
    contains `[Interactive session started]` and the shell id. The ssh
    really runs now and fails to resolve the host; the screen shows that
    honestly, and the test must not assert what ssh printed.
  - `run_terminal_command_refuses_an_interactive_command_and_writes_nothing`
    is **renamed** `an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another`.
    Call `launch_interactive` directly with `sleep 30` on the owner's shell.
    The owner's next `echo ALIVE` then runs in a **different** shell id,
    and its output contains `ALIVE`. Send `\x03` to the first shell at the
    end. Use `sleep 30` and not ssh, because ssh to an unresolvable host
    exits at once and leaves the shell idle.
- **How a test gets a shell to launch into** (this is what the first run
  could not find). In the `shell_exec.rs` test module:
  1. `let (_home, _dir) = test_home(); let reg = ShellRegistry::default();`
  2. `let (id, _) = run_in_shell_for_test(&reg, owner.clone(), "true", 5, None, None, spawn_in_process_host).await?;`
     — this creates the owner's shell and returns its id;
  3. `launch_interactive(&reg, &id, &socket_path(id.as_str()), "sleep 30").await`;
  4. for the next-command test, `run_in_shell_for_test(&reg, owner, "echo ALIVE", 5, None, None, spawn_in_process_host)`
     returns `(id2, result)` with `id2 != id`;
  5. clean up with `crate::shell::send_input(&socket_path(id.as_str()), b"\x03", Duration::from_secs(5))`.
  `run_in_shell_for_test` is the helper at the top of the test module;
  you do not need to read the helpers below it.

## Tests

- `launch_interactive_returns_the_settled_screen` (`shell_exec.rs` tests):
  `launch_interactive` with `sleep 30` on an in-process shell ⇒ `Ok`; the
  text starts with `[Interactive session started]`, names the shell id, and
  its screen part contains `sleep 30`. Afterwards the record's
  `last_foreground` is `Program { cmd: "sleep", .. }`. Send `\x03` at the
  end.
- The two renames in § Gotchas.

## Acceptance criteria

- [ ] The new test and the two renamed ones ⇒ `test result: ok. 1 passed`
      each.
- [ ] `grep -n 'cannot hold' src/daemon/executor/shell_exec.rs | wc -l` ⇒
      `0` (the refusal text is gone).
- [ ] `cargo test` ⇒ `passed=2391 failed=0`: exactly 2390 at `07558aa7`
      plus the 1 new test. The renames add nothing.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03d.txt; : > "$A"
for t in launch_interactive_returns_the_settled_screen run_terminal_command_launches_an_interactive_command an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another shell_exec_non_interactive_still_runs_on_the_same_shell; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "refusal_text=$(grep -n 'cannot hold' src/daemon/executor/shell_exec.rs | wc -l)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then run the self-check, and put the line it prints into the entry after
the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-03d-launch.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03d.txt
diff /tmp/pasted-03d.txt /tmp/e2e-03d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- `run_terminal_command(interactive: true)` for any command, its approval
  panel line, and the `wait: false` + `interactive` refusal: phase 03e.
- An interactive command run inside a named-host (transport) shell. The
  local terminal cannot see the remote foreground (design § 9). Named-host
  shells keep today's transport classification.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-17 (refined re-dispatch)

The first run wrote no code: it hard-failed at turn 147 on a
`NoProgressStall` after reading `shell_exec.rs` in full and then **one or two
lines at a time** through lines 2040–2154 (the test helpers), looking for how
a test obtains a shell to call `launch_interactive` on. That recipe is now in
§ Gotchas, and the production edit's two anchors (the `is_interactive_command`
branch and the `let (id, socket) = match target {` binding) are named in
Spec Task 2. **Do not read files a line at a time**: find an anchor with one
`grep -n`, then read one 60–120-line window around it. Write
`launch_interactive` first, then the branch, then the three tests. Record the
executor as the model that actually runs (the first run's log entry named a
different model and has been removed).


### Update — 2026-10-17 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** first failure, wrote no code, and the stall traces to a spec gap (no recipe for a test to obtain the shell `launch_interactive` needs, and no anchor for the edit in a 3,700-line file), which a re-dispatch with those named fixes; there is no partial work for a resume to keep.

### Update — 2026-10-17 18:37 (progress)

**Executor:** phase-03d run 2 (headless).

`launch_interactive` and the branch are in; the two 02c tests are renamed and
rewritten to the launch shape and `launch_interactive_returns_the_settled_screen`
passes. `read_shell_screen_in` is `pub(crate)` and re-exported from
`knowledge`, with `SCREEN_TIMEOUT` widened for the same caller. Capturing the
end-to-end evidence next.

### Update — 2026-10-17 18:52 (end-to-end verification)

Run of § End-to-end verification, verbatim, captured to `/tmp/e2e-03d.txt`:

```
launch_interactive_returns_the_settled_screen: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
run_terminal_command_launches_an_interactive_command: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shell_exec_non_interactive_still_runs_on_the_same_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
refusal_text=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2391 failed=0
```

PASTE MATCH

### Update — 2026-10-17 19:42 (complete, server-authored)

**Summary:** Phase 03d complete.

**Summary:** `run_terminal_command`'s interactive-command branch now **launches** instead of refusing: `launch_interactive(reg, &id, &socket, cmd)` writes the bare command plus `\r` via `crate::shell::send_input` with no marker wrapping, then reads the settled screen through `read_shell_screen_in(..., settle_ms: Some(500), timeout: Some(5), SCREEN_TIMEOUT)` — the injected-registry form widened to `pub(crate)` and re-exported from `knowledge` (with `SCREEN_TIMEOUT`, also widened) rather than copying its logic — and returns `ShellRunResult { exit_code: 0 }` with the `[Interactive session started]` header (no exit-code wording) followed by the masked screen; the branch still sits after target resolution and the named-shell busy probe, and the `wait: false` path is untouched. The two 02c tests are renamed per § Gotchas and rewritten to the launch shape — `run_terminal_command_launches_an_interactive_command` (ssh really runs, `Ok` within 10 s, header + shell id asserted, nothing about what ssh printed) and `an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another` (launch `sleep 30` directly; `echo ALIVE` then runs in a different shell id; the held shell is refused by name; `\x03` ends the session and the shell is reusable) — plus the new `launch_interactive_returns_the_settled_screen`, which also pins `last_foreground == Program { cmd: "sleep", tui: false }` after the settle read.

**End-to-end verification:** § E2E ran verbatim, once, captured to `/tmp/e2e-03d.txt` and pasted into the `end-to-end verification` Update Log entry with the self-check printing **PASTE MATCH** — all four named tests `1 passed` each, `refusal_text=0`, `doc_truth` 16 passed, clippy (`-D warnings`) clean, `fmt_exit=0`, full suite `passed=2391 failed=0` (2390 at `07558aa7` + the 1 new test, renames adding nothing). Committed as `a4f57c1d feat: an interactive command launches in its shell and returns the settled screen`; `git status --short` empty.

**Notes for review:** (1) `run_terminal_command_launches_an_interactive_command` no longer asserts the record's `last_foreground` is `Program` — `ssh nowhere.invalid` fails to resolve at once, so the settle read may honestly see `Prompt`; the busy read is pinned by the `sleep 30` test, and the comment in the test says so. (2) An earlier verifier-corrected paste had a stray blank line before the closing fence; the self-check now passes byte-for-byte. (3) `sre.toml` § "Interactive commands" (line 100) still describes the pre-02c "return on remote-prompt appearance" behaviour, and `docs/architecture.md` line 494 still says the shell is "marked interactive = true" — both predate M28 and neither file is mine to edit; flagging for the architect.

**Gates:** format=run, build=run, lint=run, test=run

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.18s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (2909867): No such process
kill: (-2910585): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `src/daemon/executor/knowledge/mod.rs` — +5 -0
- `src/daemon/executor/knowledge/shell.rs` — +8 -3
- `src/daemon/executor/shell_exec.rs` — +239 -82

**Commit:** a4f57c1d0923ecedbb2f92d676010558a24ab59b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-17

- **Verdict:** approved_first_try
- **Bounces:** none (one escalation: the first run hard-failed on a
  `NoProgressStall` with no code written; refined re-dispatch, 247 turns)
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** none
- **Calibration:** the stall was architect-caused. The spec told a test to
  "call `launch_interactive` on the owner's shell" without saying how a
  test obtains that shell, and gave no anchor for the edit in a
  3,700-line file. The executor crawled the test helpers one line per
  read. Adding a five-step recipe and two anchors turned it into a clean
  run. Lesson: when a spec asks a test to call a new function directly,
  give the fixture recipe that produces its arguments. Independent re-run:
  2391 passed / 0 failed, fmt and clippy clean, refusal text 1 ⇒ 0. The
  new test pins `last_foreground` `None` before and `Program { sleep }`
  after the launch.
