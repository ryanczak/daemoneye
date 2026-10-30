# Phase 08 — `edit_file` logs a failed connect; one host lookup per foreground call

**Status:** done
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Findings:** M31 `FINDINGS.md` #80 and #82 (#75, #78 and #79 are
deferred below)

## Goal

Two changes, both in `src/daemon/executor/`:
- **#80.** When the connect to a named host fails, `read_file` and
  `open_shell` write an `ssh_connect_failed` event, but `edit_file`
  (`dispatch_files.rs`, the `EditFile` arm) writes none. A failed edit
  connect leaves no record in `events.jsonl`. After this phase, the
  file tools share one `log_connect_failure` helper, and `edit_file`
  calls it too.
- **#82.** A foreground call to a named host runs
  `crate::shell::host_entry_or_bare` twice: in `run_approved`, then
  again in `run_on_named_host` (`dispatch_foreground.rs`). Only the
  second copy writes the `host-error` shell log line. The second can
  fail only if the config changes between two adjacent lines. After
  this phase, the first copy is gone, and every failure goes through
  the copy that writes the log line.

**Deferred, not fixed here:** #75 (the `Owner::Chat(session_id.unwrap_or_default())`
sites), #78 (the host-parse prologue repeated across `read_file`,
`open_shell` and the foreground arm) and #79 (`read_file` sends one
`ToolResult` frame that other silent tools do not). All three reshape
`dispatch` bodies. One of them is `dispatch_foreground.rs`'s `dispatch`,
at 145 of its 150-line cap, which a phase-01 run already broke by
extracting from it. They go to the function milestone, which splits
those bodies anyway. #79 is harmless meanwhile.

## Architecture references

- M32 README, decisions 1 and 6.
- `CLAUDE.md` § "Important Invariants": `log_event` stamps `ts` and
  `pid` itself.

## Spec

The architect dry-ran the whole diff below on `HEAD`: clippy and fmt
clean, hygiene passing, and the full suite at `passed=2605 failed=0`
twice. Apply it as written.

1. **The helper and its test** (`dispatch_files.rs`). Add
   `log_connect_failure` and the test module at the end of the file, as
   in the diff. Replace `read_file`'s inline `ssh_connect_failed`
   `log_event` with a call to the helper; this step does not change
   what is logged. Run
   `cargo test --lib log_connect_failure_records_the_tool_and_host`; it
   passes.
2. **The #80 fix.** In the `EditFile` arm, call
   `log_connect_failure("edit_file", session_id, host.as_deref().unwrap_or("local"), &msg)`
   as the first statement of the connect-failure branch, as in the diff.
3. **The #82 fix** (`dispatch_foreground.rs`). In `run_approved`,
   delete the `let host = match &host { … };` block that calls
   `host_entry_or_bare`. Shorten the comment above it to the two lines
   in the diff, and change `cmd_id,` in `run_approved`'s `ApprovedRun`
   destructure to a trailing `..`, because `cmd_id` is no longer used
   there. `run_on_named_host` and everything else stay as they are.
4. **Findings** go in their own `### Findings — <date>` entry, from the
   two edited functions only, within five tool calls, or the single line
   `none`.
5. **Commit** as `fix:`.

**Constraints.**
- `dispatch_foreground.rs`'s `dispatch` must not be edited. It is at 145
  of 150 lines.
- `foreground::refuse_foreground(` must still be called exactly once in
  `dispatch_foreground.rs`.
- Add no `#[allow(…)]`.

There is no red step here. #80 is a missing call, which the
`helper_calls=2` criterion shows (today `0`). #82 removes a duplicate.
The test pins what the shared helper records.

```diff
diff --git a/src/daemon/executor/dispatch_files.rs b/src/daemon/executor/dispatch_files.rs
index 82a731b7..5532a8c2 100644
--- a/src/daemon/executor/dispatch_files.rs
+++ b/src/daemon/executor/dispatch_files.rs
@@ -50,15 +50,7 @@ where
             }
             if let Err(msg) = connect_named_host(host.as_deref(), is_ghost, call.id(), tx, rx).await
             {
-                crate::daemon::utils::log_event(
-                    "ssh_connect_failed",
-                    serde_json::json!({
-                        "session": session_id.unwrap_or("-"),
-                        "tool": "read_file",
-                        "requested_host": host_id.as_str(),
-                        "error": msg,
-                    }),
-                );
+                log_connect_failure("read_file", session_id, host_id.as_str(), &msg);
                 let outcome = format!("Error: {msg}");
                 send_response_split(
                     tx,
@@ -96,6 +88,12 @@ where
                 && let Err(msg) =
                     connect_named_host(host.as_deref(), false, id.as_str(), tx, rx).await
             {
+                log_connect_failure(
+                    "edit_file",
+                    session_id,
+                    host.as_deref().unwrap_or("local"),
+                    &msg,
+                );
                 let outcome = format!("Error: {msg}");
                 send_response_split(
                     tx,
@@ -133,3 +131,40 @@ where
         other => anyhow::bail!("{} is not one of the file tools", other.tool_name()),
     }
 }
+
+/// Record a failed connect to a named host, for a file tool's call.
+fn log_connect_failure(tool: &str, session_id: Option<&str>, requested_host: &str, msg: &str) {
+    crate::daemon::utils::log_event(
+        "ssh_connect_failed",
+        serde_json::json!({
+            "session": session_id.unwrap_or("-"),
+            "tool": tool,
+            "requested_host": requested_host,
+            "error": msg,
+        }),
+    );
+}
+
+#[cfg(test)]
+mod tests {
+    use super::log_connect_failure;
+
+    #[test]
+    fn log_connect_failure_records_the_tool_and_host() {
+        let _home = crate::daemon::run::cancel::test_support::HomeGuard::new();
+        log_connect_failure("edit_file", Some("sess-1"), "web01", "connection refused");
+
+        let seg = crate::config::current_event_segment_path();
+        let text = std::fs::read_to_string(&seg).unwrap_or_default();
+        let events: Vec<serde_json::Value> = text
+            .lines()
+            .filter_map(|l| serde_json::from_str(l).ok())
+            .filter(|v: &serde_json::Value| v["event"] == "ssh_connect_failed")
+            .collect();
+        assert_eq!(events.len(), 1, "one call, one event: {events:?}");
+        assert_eq!(events[0]["tool"], "edit_file");
+        assert_eq!(events[0]["requested_host"], "web01");
+        assert_eq!(events[0]["session"], "sess-1");
+        assert_eq!(events[0]["error"], "connection refused");
+    }
+}
diff --git a/src/daemon/executor/dispatch_foreground.rs b/src/daemon/executor/dispatch_foreground.rs
index 84bad698..c4fc0765 100644
--- a/src/daemon/executor/dispatch_foreground.rs
+++ b/src/daemon/executor/dispatch_foreground.rs
@@ -211,37 +211,13 @@ where
         shell,
         timeout_secs,
         interactive,
-        cmd_id,
         session_id,
         max_per_owner,
+        ..
     } = a;
     let owner = Owner::Chat(session_id.unwrap_or_default().to_string());
-    // `host` is already parsed above, before the prompt.
-    // The permissive lookup runs **after** approval: an
-    // unconfigured host resolves to its own ssh
-    // destination there, and only an empty-ssh half-written
-    // entry refuses — never routed through the shell-error
-    // wording, which would dress a host problem in
-    // shell-limit or timeout advice the model cannot act on.
-    let host = match &host {
-        crate::host::HostId::Local => crate::host::HostId::Local,
-        crate::host::HostId::Named(name) => {
-            if let Err(e) = crate::shell::host_entry_or_bare(name) {
-                let msg = format!("Error: {e}");
-                send_response_split(
-                    tx,
-                    Response::ToolResult {
-                        tool_call_id: id.to_string(),
-                        output: msg.clone(),
-                    },
-                )
-                .await?;
-                crate::daemon::stats::finish_command(cmd_id, 1);
-                return Ok(msg);
-            }
-            host.clone()
-        }
-    };
+    // `host` is already parsed above, before the prompt. A named host's
+    // permissive lookup runs once, after approval, in `run_on_named_host`.
     // The two arms spawn different programs, so their closure
     // types differ and each calls `run_in_shell_with_prompts`
     // itself (the same split `open_shell` makes). An
```

## Acceptance criteria

- [ ] `grep -c 'log_connect_failure("' src/daemon/executor/dispatch_files.rs`
      prints `2` (today: `0`).
- [ ] `grep -c 'host_entry_or_bare' src/daemon/executor/dispatch_foreground.rs`
      prints `1` (today: `2`).
- [ ] Preservation: `grep -c 'refuse_foreground(' src/daemon/executor/dispatch_foreground.rs`
      prints `1`, and `grep -c '#\[allow' src/daemon/executor/dispatch_foreground.rs src/daemon/executor/dispatch_files.rs`
      prints `0` for both.
- [ ] `log_connect_failure_records_the_tool_and_host` passes.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2605 failed=0` (2604 + 1).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
E=src/daemon/executor
echo "helper_calls=$(grep -c 'log_connect_failure("' $E/dispatch_files.rs) lookups=$(grep -c 'host_entry_or_bare' $E/dispatch_foreground.rs) refuse=$(grep -c 'refuse_foreground(' $E/dispatch_foreground.rs)"
cargo test --lib log_connect_failure_records_the_tool_and_host 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `helper_calls=2 lookups=1 refuse=1`,
`test result: ok. 1 passed`, hygiene `ok. 16 passed`, bug tracker `ok.
6 passed`, clippy clean, `passed=2605 failed=0`. Paste the output into
an `### Update — <date> (end-to-end verification)` entry. **Known
flakes:** `read_shell_screen_lines_tails_the_visible_rows` and
`for_each_skips_unparseable_lines`. If the block shows
`passed=2604 failed=1`, or stops short with one of those failing, run
it once more and paste both outputs.

## Authorizations

- Edit `src/daemon/executor/dispatch_files.rs` and
  `src/daemon/executor/dispatch_foreground.rs`, outside the
  foreground `dispatch` function. No other source file.

## Out of scope

- `open_shell`'s own `ssh_connect_failed` call in
  `dispatch_open_shell.rs`.
- #75, #78, #79 (above).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 00:45 (progress)

Started and applied the full spec diff: `log_connect_failure` and its test
landed in `dispatch_files.rs` (the `read_file` arm now calls the helper,
logging unchanged); the `EditFile` arm calls it as the first statement of its
connect-failure branch; `run_approved` no longer destructures `cmd_id` and no
longer runs `host_entry_or_bare` — the comment above the old lookup shrank to
the two lines the diff pins, `cmd_id,` became a trailing `..`, and
`run_on_named_host` is untouched. `dispatch` in `dispatch_foreground.rs` was
not edited. All five greps print the expected values (`helper_calls=2
lookups=1 refuse=1`, `#[allow` 0/0), the helper test passes, fmt/build/clippy
are clean, and the full suite reads `passed=2605 failed=0`.

### Findings — 2026-10-30

- `dispatch_files.rs` · the `ReadFile` and `EditFile` connect-failure arms:
  the two build the same failure reply from different id sources (`read_file`
  uses `call.id()`, `edit_file` the destructure's own `id`) and exit
  differently (`break 'arm` vs `return`). Behaviour matches today; noted as
  shape drift for #78's shared prologue at the function milestone, not fixed
  here.
- `dispatch_foreground.rs` · `ApprovedRun.cmd_id`: with `run_approved`'s
  lookup gone, its only remaining reader is `run_on_named_host`'s
  `finish_command` — the field is now named-host-specific but still sits in
  the struct shared with the local arm. Moving it reshapes `ApprovedRun`, so
  it is deferred to the function milestone.

### Update — 2026-10-30 00:47 (end-to-end verification)

Ran the phase doc's E2E block verbatim, redirected to
`/tmp/phase08-e2e.txt`; the file's contents follow, unedited.

```
helper_calls=2 lookups=1 refuse=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2527 filtered out; finished in 0.04s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.09s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
passed=2605 failed=0
```

Matches the expected values exactly: `helper_calls=2 lookups=1 refuse=1`,
the helper test green, hygiene `16 passed`, bug tracker `6 passed`, clippy
clean, and the full suite `passed=2605 failed=0` (2604 + 1). No flake
appeared in this run; neither named known flake failed.

### Update — 2026-10-30 00:52 (complete, server-authored)

**Summary:** **Summary:** Implemented M32 phase 08 exactly as the spec's diff pins it: `log_connect_failure` (plus its `log_connect_failure_records_the_tool_and_host` test) now lives at the end of `dispatch_files.rs`, the `read_file` arm calls the helper with logging unchanged, and the `EditFile` arm calls it as the first statement of its connect-failure branch — closing #80's silent gap. In `dispatch_foreground.rs`, `run_approved` no longer destructures `cmd_id` or runs `shell::host_entry_or_bare` (the comment shrank to the two pinned lines, `cmd_id,` became a trailing `..`), so a named-host foreground call performs its permissive lookup once, in `run_on_named_host`, which is the copy that writes the host-error log line — closing #82. `dispatch` itself was not edited, `refuse_foreground(` still appears exactly once, and no `#[allow]` was added.

**E2E result:** the phase doc's E2E block, run verbatim and redirected to `/tmp/phase08-e2e.txt`, printed `helper_calls=2 lookups=1 refuse=1`, the helper test `ok. 1 passed`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, and the full suite `passed=2605 failed=0` (2604 + 1) — matching expectations exactly, with neither named flake appearing; the transcript is pasted unedited in the phase doc's `### Update — 2026-10-30 00:47 (end-to-end verification)` entry.

**Notes for review:** Two deviations, both documentation-only and self-corrected within the run — I first appended the phase's findings to the milestone `FINDINGS.md` ledger, then realized the acceptance criterion counts `^### Findings — ` on the *phase doc* (as phases 01–07 did) and moved the entries there, reverting the ledger. The two findings, from the edited functions only: (13) the `read_file` and `edit_file` connect-failure arms build the same reply from different id sources and exit differently (`break 'arm` vs `return`) — shape drift to fold into #78's shared prologue at the function milestone; (14) `ApprovedRun.cmd_id` is now named-host-specific but still sits in the shared struct — deferred, since moving it reshapes `ApprovedRun`. Two commits landed: `fix:` d3a57c15 (code + test) and `docs:` 260590d5/b4105770 (status flip, started entry, findings, E2E evidence); `git status --short` is clean, and the phase doc's Status remains `in-progress` for the server's completion tail.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
 ok
test functions_in_a_test_module_are_not_measured ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test production_lines_exclude_an_inline_test_module ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.25s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_injects_on_next_run ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (3970403): No such process
kill: (-3971120): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` — +0 -1
- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/phase-08-host-refusal.md` — +47 -1
- `src/daemon/executor/dispatch_files.rs` — +44 -9
- `src/daemon/executor/dispatch_foreground.rs` — +3 -27

**Commit:** b41057703a194842aebc2eadfc3e8778b7453932

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-30

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none.
  - The production diff is line for line the dictated one. The
    reviewer compared the `+`/`-` lines of both.
  - The only ledger edit was a trailing blank line.
- **Reviewer's checks:**
  - fmt 0, build and clippy clean, 2605 passed / 0 failed.
  - `bug_tracker` passes, with 6 tests.
  - `helper_calls=2 lookups=1 refuse=1 allow=0`, and one Findings heading.
  - The foreground `dispatch` is still 145 lines, untouched.
- **Tests:** `log_connect_failure_records_the_tool_and_host` reads the
  event back and checks all four fields. It would fail if the helper
  logged under another event name or dropped a field.
- **Findings:** two, logged as M32 #13 and #14 and both deferred to the
  function milestone.
- **Calibration:** the sixth dictated phase first try. Dictating the
  whole diff worked for a change spread over two files.

