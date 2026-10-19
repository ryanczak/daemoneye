# Phase 03: log-fields — `wait` and `shell` on command events; 2.0 wording in daemon.log

**Milestone:** M29 — Post-2.0 fixes
**Status:** done
**Depends on:** phase-02
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

The audit log and daemon.log still use 1.x terms. A `command` event says
`mode: foreground` or `mode: background` for an approval decision and
`mode: shell` for a run, and its `pane` field holds a shell id, a host
name or nothing. daemon.log says `foreground command approved: <cmd>`.

In 2.0 the distinction is **waited vs `wait: false`**, and commands run in
**shells**. PE decision: add the 2.0 fields **alongside** the old ones, so
existing readers of `mode`/`pane` keep working, and reword daemon.log.

## Spec

**Files:** `src/daemon/utils/event_log.rs`, `src/daemon/executor/mod.rs`,
`README.md` (§ "Command audit log").

1. **`log_shell_command` takes `wait: bool`** and writes two new keys:
   `"wait": <wait>` and `"shell": <shell_id>`. `mode` (`"shell"`) and
   `pane` (the shell id) are unchanged. Update every caller. The compiler
   names them; all are in `executor/mod.rs`:
   - the waited `run_terminal_command` arms (local and named host),
     including their `host-error` refusals, pass `true`;
   - the chat **`wait: false`** start (`[Background Task Completed]`) and
     its `shell-error`, pass `false`.
2. **`log_command` writes `wait` derived from `mode`:** `true` for
   `"foreground"`, `false` for `"background"`, `null` for any other value.
   It also writes `"shell": null`, because its `pane` is empty or a host
   name, never a shell id. Its signature and `mode`/`pane` are unchanged.
3. **daemon.log in 2.0 terms.** Add a pure
   `fn approval_log_line(background: bool, decision: &str, cmd: &str)
   -> String` in `executor/mod.rs`. It returns
   `command <decision> (waited): <cmd>`, or
   `command <decision> (wait: false): <cmd>` when `background` is true.
   Use it in `prompt_and_await_approval` for both the approved line and
   the denied/timeout line, replacing the two `log::info!` formats that
   lead with `mode`. The `command_approval` events and the stats counters
   are unchanged.
4. **README.** In § "Command audit log", add `"wait"` and `"shell"` to both
   example lines, keeping the keys alphabetical, as `serde_json` writes
   them. Replace the paragraph after the examples so it says:
   - `wait` is `true` for a waited command and `false` for `wait: false`;
   - `shell` is the PTY shell id for a run and `null` for an approval
     decision;
   - `mode` and `pane` are kept for existing readers, and new readers
     should use `wait` and `shell`.
5. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m29-03.txt`) into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)` as the **first**
   fenced block in that entry, then the `PASTE MATCH` line. Do not paste
   the command block.

**Gotchas.**
- Event keys serialize alphabetically (see CLAUDE.md § Important
  Invariants), so never assert on key position.
- `foreground.rs` passes `"background"` to `log_command` on the ghost
  paths. Ghost runs are background-only, so `wait: false` is correct
  there. Do not change those calls.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

All in `event_log.rs` tests, using the fixture that
`log_shell_command_writes_the_exit_code` already uses, except the last one.

- `log_shell_command_records_wait_and_shell`: a call with `wait: false`
  and shell `s3` writes a record with `"wait": false`, `"shell": "s3"`,
  `"mode": "shell"` and `"pane": "s3"`.
- `log_command_derives_wait_from_mode`: `"foreground"` gives
  `"wait": true`, `"background"` gives `false`, and `"other"` gives `null`.
  Each record has `"shell": null` and keeps its `mode` and `pane`.
- `the_approval_log_line_speaks_2_0` (`executor/mod.rs` tests, pure):
  - `approval_log_line(false, "approved", "ls")` equals
    `command approved (waited): ls`;
  - `approval_log_line(true, "denied", "rm x")` equals
    `command denied (wait: false): rm x`;
  - neither contains `foreground` or `background`.

## Acceptance criteria

- [ ] The three new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `grep -c 'command approved: {}' src/daemon/executor/mod.rs` ⇒ `0`
      (end-to-end line `old_log_lines=0`).
- [ ] README § "Command audit log" mentions `"wait"` and `"shell"`
      (end-to-end line `readme_wait=` is at least 2).
- [ ] `cargo test` ⇒ about `passed=2418 failed=0` (2415 at `e5bb5291`
      + 3). An extra test is fine: name it and report; do not re-run the
      suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-03.txt; : > "$A"
for t in log_shell_command_records_wait_and_shell log_command_derives_wait_from_mode the_approval_log_line_speaks_2_0 log_shell_command_writes_the_exit_code; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "old_log_lines=$(grep -c 'command approved: {}' src/daemon/executor/mod.rs)" >> "$A"
echo "readme_wait=$(awk '/^## Command audit log/{f=1} /^---/{f=0} f' README.md | grep -c '"wait"')" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-03-log-fields.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-03.txt
diff /tmp/pasted-m29-03.txt /tmp/e2e-m29-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Removing or renaming `mode` / `pane`, which the PE decided to keep.
- The `command_approval` event, the stats counters (`commands_fg_*` /
  `commands_bg_*`) and `foreground.rs`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-19 08:05 (started, executor=rexy executor)

**Status:** in-progress.

Beginning phase-03. Plan: extend `log_shell_command` with a `wait: bool`
parameter writing `"wait"`/`"shell"`, derive `wait`/`shell` in `log_command`
from `mode`/`pane`, add the pure `approval_log_line` and use it for both
daemon.log lines in `prompt_and_await_approval`, update README § Command audit
log, add the three spec tests, then run the § End-to-end verification script
once and paste its output.

### Update — 2026-10-19 08:15 (end-to-end verification)

Output of the § End-to-end verification script, run once after
`cargo fmt --all`:

```
log_shell_command_records_wait_and_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
log_command_derives_wait_from_mode: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_approval_log_line_speaks_2_0: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
log_shell_command_writes_the_exit_code: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
old_log_lines=0
readme_wait=2
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2418 failed=0
```

PASTE MATCH

### Update — 2026-10-19 15:17 (complete, server-authored)

**Summary:** **Summary:** All five Spec items are implemented and committed (`a419aa31`). `log_shell_command` now takes `wait: bool` and writes `"wait"`/`"shell"` alongside the unchanged `mode: "shell"`/`pane`; all eight call sites in `executor/mod.rs` pass `true` for the waited `run_terminal_command` arms (including their host-error refusals) and `false` for the chat `wait: false` start and its `shell-error`. `log_command` derives `wait` from `mode` (`foreground`→true, `background`→false, other→null) and writes `"shell": null`. The new pure `approval_log_line` replaces both 1.x `log::info!` formats in `prompt_and_await_approval`, so daemon.log now says `command approved (waited): …` / `command denied (wait: false): …`; the `command_approval` events and stats counters are untouched, as are `foreground.rs`'s `"background"` ghost calls. README § Command audit log gained `"wait"`/`"shell"` in both example lines (alphabetical, as `serde_json` writes them) and the rewritten key paragraph.

**Notes for review:** E2E verification ran once, verbatim, and its output is pasted as the first fence of the `### Update — 2026-10-19 08:15 (end-to-end verification)` entry with `PASTE MATCH` confirmed by the self-check script. Results: all three new tests pass individually plus the updated `log_shell_command_writes_the_exit_code` fixture; `old_log_lines=0`; `readme_wait=2`; `doc_truth` 16 passed/0 failed; clippy `-D warnings` clean; `fmt_exit=0`; full suite `passed=2418 failed=0` — exactly the spec's expected 2415+3. No new `#[allow]`, no TODOs, no scope creep; `git status` is clean and the phase doc status remains `in-progress` for the server to finalize.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.1.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
roval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
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
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-d9203e33e9de0e0e)
kill: (1100422): No such process
kill: (-1101050): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-6a738a64918f799f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9feb568c477cffdf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-05bab2f96380bc67)
     Running tests/integration.rs (target/debug/deps/integration-502483a68671b4b7)
     Running tests/isolation.rs (target/debug/deps/isolation-66ea1e45aff405d5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-0960811ac376916b)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +3 -3
- `docs/dev/milestones/M29-post-2.0-fixes/README.md` — +1 -1
- `docs/dev/milestones/M29-post-2.0-fixes/phase-03-log-fields.md` — +32 -1
- `src/daemon/executor/mod.rs` — +44 -2
- `src/daemon/utils/event_log.rs` — +56 -2

**Commit:** a419aa3134bffa747982382e2420f502876fe9c0

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-19

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (one dispatch,
  79 turns)
- **Scope deviations:** none.
- **Calibration:**
  - **The consumer check worked.** The spec named the two `wait: false`
    `log_shell_command` callers, which the todo list did not mention, and
    the executor passed `false` at exactly those two calls and `true` at
    the six waited ones. It is the first clean first dispatch of M29.
  - **Nit, mine:** the spec's "replace the paragraph" dropped the README
    sentence saying `exit` is the real exit code. Restore it at the close.
    This is not a bounce, since the executor did what the spec said.
  - Independent re-run: fmt, build and clippy exit 0; 2418 passed, 0
    failed; self-check MATCH on the first fence (the output-first paste
    instruction worked); standards grep clean; no production
    `unwrap`/`expect`; `foreground.rs` untouched.
