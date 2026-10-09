# Phase 10b: sbx-stage-resolved — stage a script the ghost path has already resolved to its absolute path

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** review
**Depends on:** phase-10a
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=bugfix, size=xs

## Goal

Found at the row-10 live close on pinky, 2026-10-08 (ghost run 4). A
sandboxed ghost ran its approved script `de-live-tick.sh` and got:

```
bash: /home/matt/.daemoneye/scripts/de-live-tick.sh: No such file or directory
[command exited with status 127]
```

The script was never staged into the sandbox and the command was never
rewritten to `/de/scripts/de-live-tick.sh`.

## Root cause

In `run_background` (`src/daemon/executor/foreground.rs`) a ghost's command
goes through `policy.resolve_command(cmd)` **before** the sandboxed-ghost
branch, turning `de-live-tick.sh` into
`/home/matt/.daemoneye/scripts/de-live-tick.sh`. The branch then calls
`sbx::script_invocation(cmd, …)`, which uses
`crate::scripts::parse_script_invocation`, and that returns `None` for any
first word starting with `/` (`src/scripts.rs`, the `first.starts_with('/')`
check). So staging is skipped and the host path runs inside the VM.

The 06d unit tests only used bare names. `start_background_sandboxed`
(`src/daemon/executor/bg_shell.rs`, the chat `wait: false` path) calls the
same `script_invocation`; check whether chat commands reach it resolved too,
and cover whichever paths do.

## Spec

A sandboxed command whose first word is the absolute path of a file inside
`crate::config::scripts_dir()` must be staged and rewritten exactly as the
bare name is. An absolute path **outside** the scripts directory must still
not be staged. Keep `parse_script_invocation`'s other callers (the remote
ghost path) behaving as they do now. Choose where the fix lives.

Tests, in `src/daemon/executor/sbx.rs` or wherever the logic lands:
- `script_invocation_accepts_an_absolute_path_under_scripts_dir`: the first
  word `<scripts_dir()>/restart.sh` with `exists = |n| n == "restart.sh"`
  gives `Some(("restart.sh", " --now"))` for a ` --now` tail.
- `script_invocation_rejects_an_absolute_path_outside_scripts_dir`:
  `/usr/bin/restart.sh --now` gives `None` even when `exists` returns true.

## Acceptance criteria

- [ ] Both named tests report `1 passed` alone. Today each reports `0 passed`.
- [ ] `cargo test --lib executor::sbx` reports `0 failed`; no existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed` and exactly `passed=2300` (today 2298,
      plus the two new tests).
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
for t in script_invocation_accepts_an_absolute_path_under_scripts_dir script_invocation_rejects_an_absolute_path_outside_scripts_dir; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines.

## Authorizations

- [x] May edit `src/daemon/executor/sbx.rs`, `src/scripts.rs`,
      `src/daemon/executor/foreground.rs` and
      `src/daemon/executor/bg_shell.rs` as the chosen fix needs.

## Out of scope

- The live re-run (the architect repeats the ghost check after this lands).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-08 15:01 (started, executor: claude)

Executor run started. Flipped Status todo → in-progress; README row mirrored.
Plan: re-read the resolve/staging call path (`policy.resolve_command` in
`run_background`, then `sbx::script_invocation` → `parse_script_invocation`),
choose where the absolute-path fix lives, add the two named tests, keep the
remote ghost path and all existing tests untouched, then run the § End-to-end
verification block verbatim and paste it as this phase's end-to-end entry.

### Notes for executor — 2026-10-08 (refined re-dispatch after a NoProgressStall)

The previous run made 120 read-only calls exploring `policy.rs`,
`executor/mod.rs` and `session.rs`, and edited nothing. The spec left the
fix's location open, and that was the gap. **It is now decided**:

- **Fix only `script_invocation` in `src/daemon/executor/sbx.rs`** (line
  ~362). Before it calls `parse_script_invocation`, if the command's first
  word is an absolute path whose **parent directory equals
  `crate::config::scripts_dir()`**, replace that first word with its file
  name. Any other absolute path is left as it is, and
  `parse_script_invocation` rejects it as today.
- Keep the signature unchanged. Both callers (`foreground.rs` ~1246, the
  ghost path; `bg_shell.rs` ~116, chat `wait: false`) then get the fix with
  no edit.
- Do **not** touch `src/scripts.rs`, `policy.rs` or `executor/mod.rs`.
  There is nothing to investigate there.
- Add the two named tests next to `script_invocation_skips_sudo_and_unknown_scripts`.
  Build the absolute path from `crate::config::scripts_dir()`.

Finish condition: `passed=2300`, exactly two more than today.

### Update — 2026-10-08 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** no code was written, so there is nothing to resume. The stall
came from the spec leaving the fix's location open, and pinning that one
decision removes it.

### Update — 2026-10-08 (end-to-end verification)

Fix landed in `src/daemon/executor/sbx.rs` only, per the re-dispatch note:
`script_invocation` now runs its first word through
`canonicalize_scripts_dir_path`, which rewrites
`<crate::config::scripts_dir()>/<name>` to the bare `name` (keeping the
verbatim argument tail) before `parse_script_invocation` sees it; any other
absolute path passes through untouched and stays `None` there. Both callers
(`foreground.rs` ghost path, `bg_shell.rs` chat `wait: false`) pick the fix up
unmodified. `src/scripts.rs`, `policy.rs`, `executor/mod.rs` untouched; no
existing test edited.

```
$ for t in script_invocation_accepts_an_absolute_path_under_scripts_dir script_invocation_rejects_an_absolute_path_outside_scripts_dir; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
script_invocation_accepts_an_absolute_path_under_scripts_dir: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2245 filtered out; finished in 0.00s
script_invocation_rejects_an_absolute_path_outside_scripts_dir: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2245 filtered out; finished in 0.00s
$ cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
test result: ok. 66 passed; 0 failed; 0 ignored; 0 measured; 2180 filtered out; finished in 0.00s
$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.10s
$ cargo fmt --all --check; echo "fmt_exit=$?"
fmt_exit=0
$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2300 failed=0
$ git diff --stat
 src/daemon/executor/sbx.rs | 46 +++++++++++++++++++++++++++++++++++++++++++++-
 1 file changed, 45 insertions(+), 1 deletion(-)
```

PASTE MATCH: yes

### Update — 2026-10-09 00:05 (complete, server-authored)

**Summary:** **Summary + Notes for review:** Phase 10b is implemented in `src/daemon/executor/sbx.rs` only, per the re-dispatch note's pinned fix location: a new `canonicalize_scripts_dir_path` helper rewrites a first word of `<crate::config::scripts_dir()>/<name>` back to the bare `name` (argument tail preserved verbatim) before `script_invocation` hands the command to `parse_script_invocation`, so a ghost-resolved absolute path now stages and rewrites to `/de/scripts/<name>` exactly like a bare name; any absolute path outside the scripts dir passes through untouched and stays `None`, so it is still never staged. The signature is unchanged, both callers (`foreground.rs` ghost path and `bg_shell.rs` chat `wait:false`) pick the fix up with no edits, and `src/scripts.rs`, `policy.rs`, `executor/mod.rs` were untouched with no existing test edited. The § End-to-end verification block was run verbatim, once, and pasted into the phase's Update Log with `PASTE MATCH: yes`: both named tests report `1 passed` alone, `executor::sbx` reports `66 passed; 0 failed`, clippy and `cargo fmt --all --check` exit 0, and full `cargo test` gives exactly `passed=2300 failed=0`. No deviations from the refined spec; one transient compile error (helper referenced before definition) was caught by the verifier and fixed within this run. Committed as `db5b20f7` (`fix:`) including the started-entry status flip already present; working tree is clean, status left at `in-progress` for the server's completion tail.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test config_pricing_round_trip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.41s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (1637659): No such process
kill: (-1638403): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `src/daemon/executor/sbx.rs` — +45 -1

**Commit:** db5b20f77e5539c38c50429afa8503811adb5d33

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
