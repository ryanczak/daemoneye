# Phase 09 — No silent drops; a gate holds the lock invariant

**Status:** review
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Findings:** M31 `FINDINGS.md` #18, #61 and #77

## Goal

Three places drop a failure, or panic, where the codebase's own
conventions say to log:
- **#61.** `run_sudo_command` (`executor/shell_exec_auth.rs`) locks its
  attempt state with `state.lock().expect("sudo attempt state")`, at two
  sites. `CLAUDE.md`'s invariant is that every mutex lock site uses
  `.unwrap_or_log()`. A poisoned lock here panics, where it should log
  an ERROR and recover. These are the only two production lock sites in
  `src/` that break the invariant. Nothing checks the invariant, which
  is how they got in.
- **#77.** `execute_tool_call` (`executor/mod.rs`) sends `ToolStarted`
  and `ToolFinished` with `let _ = send_response_split(…).await`.
  Best-effort is right for UI frames, since a dead client must not fail
  the tool, but the drop is silent. `dispatch_background.rs` also drops
  `close_shell_with`'s result text with `let _ =`.
- **#18.** `apply_extraction` (`context/epoch_extract.rs`) leaves a
  memory out of its count when `add_memory` fails, and logs nothing.

After this phase:
- the two lock sites use `.unwrap_or_log()`;
- a new source gate, `tests/lock_invariant.rs`, fails on any
  production `.lock().unwrap()` or `.lock().expect(`;
- the two frame sends and the close log at `debug` on failure;
- a failed extraction `add_memory` logs a `warn` naming the memory.

## Architecture references

- `CLAUDE.md` § "Important Invariants": the `.unwrap_or_log()` rule.
- M32 README, decisions 1 and 6.

## Spec

The architect dry-ran everything below on `HEAD`: clippy and fmt
clean, hygiene passing, and the full suite at `passed=2607 failed=0`.

1. **Add the gate, and run it red.** Create `tests/lock_invariant.rs`
   exactly as below, and run `cargo test --test lock_invariant`.
   `the_gate_sees_both_forms_and_ignores_comments` passes, and
   `production_lock_sites_never_unwrap_or_expect` fails, naming
   `src/daemon/executor/shell_exec_auth.rs:301` and `…:318`. Paste that
   output into the Update Log.
2. **The fixes.** Apply the diff below to the four source files. Run
   `cargo test --test lock_invariant` again; both tests must pass.
3. **Findings** go in their own `### Findings — <date>` entry, from the
   edited lines only, within five tool calls, or the single line `none`.
4. **Commit** as `fix:`.

**Headroom.** `executor/mod.rs` has about 782 production lines under an
800 cap, and the diff adds four. Add nothing else to that file.
`execute_tool_call` stays well under 150 lines.

**Gotchas.**
- Clippy's `collapsible_if` rejects `if emit_feedback { if let Err(e)
  = … }`. The diff writes it as the let-chain
  `if emit_feedback && let Err(e) = …`, and the gate's split-form
  check is written the same way. Keep both forms.
- `close_shell_with` returns a `String`, not a `Result`, so the
  background closer logs that text and does not match on `Err`.

`tests/lock_invariant.rs`:

```rust
//! Source gate: outside test code, no mutex lock site may `unwrap()` or
//! `expect()` its guard. CLAUDE.md's invariant is that every lock site uses
//! `.unwrap_or_log()` (`crate::util::UnpoisonExt`), which logs an ERROR and
//! recovers from a poisoned lock instead of panicking.

use std::path::Path;

/// Production lines of one file: everything before its first
/// `#[cfg(test)]`, minus comment lines. A `*_tests.rs` or `tests.rs` file is
/// test code throughout and has none.
fn production_lines(path: &Path, text: &str) -> Vec<(usize, String)> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string());
    if name
        .as_deref()
        .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
    {
        return Vec::new();
    }
    let source = text.split("#[cfg(test)]").next().unwrap_or_default();
    source
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect()
}

/// The lines that unwrap or expect a lock guard, including the split form
/// where `.lock()` ends one line and `.unwrap()` or `.expect(` starts the next.
fn lock_unwrap_hits(lines: &[(usize, String)]) -> Vec<usize> {
    let mut hits = Vec::new();
    for (i, (n, line)) in lines.iter().enumerate() {
        if line.contains(".lock().unwrap()") || line.contains(".lock().expect(") {
            hits.push(*n);
        } else if line.trim_end().ends_with(".lock()")
            && let Some((_, next)) = lines.get(i + 1)
            && (next.trim_start().starts_with(".unwrap()")
                || next.trim_start().starts_with(".expect("))
        {
            hits.push(*n);
        }
    }
    hits
}

#[test]
fn production_lock_sites_never_unwrap_or_expect() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src dir") {
            let path = entry.expect("readdir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read source file");
                for n in lock_unwrap_hits(&production_lines(&path, &text)) {
                    let rel = path
                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path);
                    failures.push(format!("{}:{n}", rel.display()));
                }
            }
        }
    }
    failures.sort();
    assert!(
        failures.is_empty(),
        "lock sites that unwrap or expect instead of `.unwrap_or_log()`:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_gate_sees_both_forms_and_ignores_comments() {
    let lines: Vec<(usize, String)> = [
        "let a = m.lock().unwrap();",
        "let b = m.lock().expect(\"x\");",
        "let c = m",
        "    .lock()",
        "    .expect(\"y\");",
        "/// `.lock().unwrap()` in a doc comment",
        "let d = m.lock().unwrap_or_log();",
    ]
    .iter()
    .enumerate()
    .filter(|(_, l)| !l.trim_start().starts_with("//"))
    .map(|(i, l)| (i + 1, l.to_string()))
    .collect();
    assert_eq!(lock_unwrap_hits(&lines), vec![1, 2, 4]);
}
```

The source diff:

```diff
diff --git a/src/daemon/context/epoch_extract.rs b/src/daemon/context/epoch_extract.rs
index cf7f3b51..3aaafcc1 100644
--- a/src/daemon/context/epoch_extract.rs
+++ b/src/daemon/context/epoch_extract.rs
@@ -76,15 +76,14 @@ pub(super) fn apply_extraction(json: &str, _session_id: &str, _epoch_seq: u32) -
             chrono::Utc::now().to_rfc3339(),
             fact.content.trim(),
         );
-        if crate::memory::add_memory(
+        match crate::memory::add_memory(
             &fact.name,
             &body,
             crate::memory::MemoryCategory::Knowledge,
             "global",
-        )
-        .is_ok()
-        {
-            count += 1;
+        ) {
+            Ok(_) => count += 1,
+            Err(e) => log::warn!("epoch extraction: memory '{}' not saved: {e}", fact.name),
         }
     }
     count
diff --git a/src/daemon/executor/dispatch_background.rs b/src/daemon/executor/dispatch_background.rs
index 1fa7a4c3..9dd196b9 100644
--- a/src/daemon/executor/dispatch_background.rs
+++ b/src/daemon/executor/dispatch_background.rs
@@ -153,13 +153,14 @@ where
                     start,
                     handle,
                     move |id: crate::shell::ShellId| async move {
-                        let _ = crate::daemon::executor::knowledge::close_shell_with(
+                        let closed = crate::daemon::executor::knowledge::close_shell_with(
                             crate::shell::registry(),
                             id.as_str(),
                             &close_owner,
                             std::time::Duration::from_secs(5),
                         )
                         .await;
+                        log::debug!("background shell {id} close: {closed}");
                     },
                 ));
             } else {
diff --git a/src/daemon/executor/mod.rs b/src/daemon/executor/mod.rs
index 97c3156f..38bc16f9 100644
--- a/src/daemon/executor/mod.rs
+++ b/src/daemon/executor/mod.rs
@@ -483,8 +483,8 @@ where
     // and animate an elapsed timer.  Approval-gated and panel tools are excluded
     // because they have their own richer UI (ToolCallPrompt, EditFilePrompt, …).
     let emit_feedback = call.should_emit_tool_feedback();
-    if emit_feedback {
-        let _ = send_response_split(
+    if emit_feedback
+        && let Err(e) = send_response_split(
             tx,
             Response::ToolStarted {
                 id: call.id().to_string(),
@@ -492,7 +492,9 @@ where
                 summary: call.summary(),
             },
         )
-        .await;
+        .await
+    {
+        log::debug!("ToolStarted frame not sent: {e}");
     }
     let tool_start = Instant::now();
 
@@ -500,7 +502,7 @@ where
 
     if emit_feedback {
         let (ok, detail) = tool_outcome_detail(&outcome);
-        let _ = send_response_split(
+        if let Err(e) = send_response_split(
             tx,
             Response::ToolFinished {
                 id: call.id().to_string(),
@@ -509,7 +511,10 @@ where
                 detail,
             },
         )
-        .await;
+        .await
+        {
+            log::debug!("ToolFinished frame not sent: {e}");
+        }
     }
 
     Ok(outcome)
diff --git a/src/daemon/executor/shell_exec_auth.rs b/src/daemon/executor/shell_exec_auth.rs
index b185ae63..fa4a2b36 100644
--- a/src/daemon/executor/shell_exec_auth.rs
+++ b/src/daemon/executor/shell_exec_auth.rs
@@ -3,6 +3,7 @@
 //! `ensure_connected`) and `run_sudo_command`.
 
 use super::*;
+use crate::util::UnpoisonExt;
 
 /// Prompt round-trip bookkeeping for one sudo run: how many credentials have
 /// been sent. sudo re-prompts with the same sentinel on a wrong password, so
@@ -298,7 +299,7 @@ where
             let host_name = host_name.clone();
             async move {
                 let attempts_used = {
-                    let s = state.lock().expect("sudo attempt state");
+                    let s = state.lock().unwrap_or_log();
                     s.attempts
                 };
                 match classify_prompt(&out, &sentinel) {
@@ -315,7 +316,7 @@ where
                             .await
                         };
                         if matches!(answer, PromptReply::Send(_)) {
-                            state.lock().expect("sudo attempt state").attempts += 1;
+                            state.lock().unwrap_or_log().attempts += 1;
                         }
                         answer
                     }
```

## Acceptance criteria

- [ ] `cargo test --test lock_invariant` passes, and the Update Log shows
      `production_lock_sites_never_unwrap_or_expect` failing at step 1
      with the two `shell_exec_auth.rs` lines.
- [ ] `grep -c 'let _ = send_response_split' src/daemon/executor/mod.rs`
      prints `1` (today: `3`). The one left is the `SystemMsg` notice,
      which is out of scope.
- [ ] `grep -c 'let _ = crate::daemon::executor::knowledge::close_shell_with' src/daemon/executor/dispatch_background.rs`
      prints `0` (today: `1`), and
      `grep -c 'not saved: {e}' src/daemon/context/epoch_extract.rs`
      prints `1` (today: `0`).
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2607 failed=0` (2605 + the
      gate's 2). `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass. fmt, build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
echo "silent_sends=$(grep -c 'let _ = send_response_split' src/daemon/executor/mod.rs) silent_close=$(grep -c 'let _ = crate::daemon::executor::knowledge::close_shell_with' src/daemon/executor/dispatch_background.rs) extract_warn=$(grep -c 'not saved: {e}' src/daemon/context/epoch_extract.rs)"
cargo test --test lock_invariant 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `silent_sends=1 silent_close=0 extract_warn=1`, the lock gate
`ok. 2 passed`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`,
clippy clean, `passed=2607 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry.

**Known flakes.** `read_shell_screen_lines_tails_the_visible_rows` is
M31 #65. The event-log tests (`for_each_skips_unparseable_lines`,
`for_each_streams_across_segments_in_order`) are M32 #11. Both flakes
appeared in this phase's dry run. If one fails, the suite stops early,
for example at `passed=2517 failed=1`. Run the block once more and
paste both outputs.

## Authorizations

- Create `tests/lock_invariant.rs`.
- Edit `src/daemon/executor/shell_exec_auth.rs`,
  `src/daemon/executor/mod.rs`,
  `src/daemon/executor/dispatch_background.rs` and
  `src/daemon/context/epoch_extract.rs`, as in the diff.

## Out of scope

- The `SystemMsg` notice send's `let _ =` in `executor/mod.rs`.
- `RwLock` `read()` and `write()` sites; the gate covers `lock()`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 01:38 (started)

Started phase 09. Plan: write `tests/lock_invariant.rs` first and run it red
against the two `shell_exec_auth.rs` sites, apply the four-file diff, re-run
the gate green, then the greps, findings, and the E2E block.

### Update — 2026-10-30 01:52 (progress)

Wrote `tests/lock_invariant.rs` verbatim from the spec and ran it red:
`the_gate_sees_both_forms_and_ignores_comments` passed and
`production_lock_sites_never_unwrap_or_expect` failed naming
`src/daemon/executor/shell_exec_auth.rs:301` and `…:318`. Applied the
four-file diff as written; the only addition the verifier forced was the
`use crate::util::UnpoisonExt;` line in `shell_exec_auth.rs`, which the
spec's diff also carries but which my first pass at that file dropped —
`shell_exec_auth.rs` sees only `super::*`, and `executor/mod.rs` does not
re-export the trait. The gate then passed 2/2, and the greps read
`silent_sends=1 silent_close=0 extract_warn=1`. fmt and build clean.

### Findings — 2026-10-30

- `executor/mod.rs` · `execute_tool_call`'s two feedback blocks now
  differ in shape: the started send folds `emit_feedback` into a let-chain
  while the finished send keeps the `if emit_feedback { if let Err(e) = … }`
  nest, because the finished arm also binds `tool_outcome_detail` first.
  Noted only; both compile and clippy-clean as written.

### Update — 2026-10-30 02:05 (end-to-end verification)

Ran the phase doc's E2E block verbatim, redirected to
`/tmp/phase09-e2e.txt`; the file's contents follow, unedited.

```
silent_sends=1 silent_close=0 extract_warn=1
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.09s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
passed=2607 failed=0
```

Matches the expected values exactly: `silent_sends=1 silent_close=0
extract_warn=1`, the lock gate `2 passed`, hygiene `16 passed`, bug
tracker `6 passed`, clippy clean, and the full suite `passed=2607
failed=0` (2605 + the gate's 2). No known flake failed in this run —
`read_shell_screen_lines_tails_the_visible_rows` and the two event-log
tests all passed on the first pass.

### Update — 2026-10-30 01:50 (complete, server-authored)

**Summary:** </think>All checks pass:
- Findings entry count = 1 ✓
- Greps = 1/0/1 ✓
- Status still in-progress ✓
- Working tree clean ✓
- No (complete) entry written by me ✓

Final message: Summary + Notes for review.</think>
**Summary:** Phase 09 is implemented and committed. `tests/lock_invariant.rs` landed exactly as the spec pins it, and I captured it failing red first (`shell_exec_auth.rs:301` and `…:318` named, the split-form/ignored-comments test green) before applying the four-file diff: both sudo attempt-state lock sites now use `.unwrap_or_log()` (#61), `ToolStarted`/`ToolFinished` sends and the background `close_shell_with` log at `debug` on failure (#77), and a failed epoch-extraction `add_memory` warns with the memory name (#18). The E2E block printed `silent_sends=1 silent_close=0 extract_warn=1`, the gate `2 passed`, hygiene `16 passed`, bug tracker `6 passed`, clippy clean, and the full suite `passed=2607 failed=0`; findings and the unedited E2E transcript are in the phase doc's Update Log, the commit is `fix:` 71957d30 (plus a `docs:` commit for the findings/E2E entries), `git status` is clean, and the phase doc's Status remains `in-progress` for your completion tail.

**Notes for review:** One deviation, forced by the verifier — my first pass at `shell_exec_auth.rs` omitted the diff's `use crate::util::UnpoisonExt;` line, so `unwrap_or_log` didn't resolve (`executor/mod.rs` does not re-export the trait; only `super::*` is in scope there). The line is in the spec's diff, so the applied result matches the spec byte-for-byte; noted here so the architect knows the verifier caught it rather than the spec being wrong. One finding recorded on the phase doc: the two feedback blocks in `execute_tool_call` now differ in shape (let-chain vs. nest) because the finished arm binds `tool_outcome_detail` first — noted only, no action taken. No known flake failed; neither named flake appeared in the run.

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
owlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.25s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test ipc_session_info_round_trip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
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
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


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
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (4071538): No such process
kill: (-4072278): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-084f36955f09af75)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/phase-09-silent-drops.md` — +50 -1
- `src/daemon/context/epoch_extract.rs` — +4 -5
- `src/daemon/executor/dispatch_background.rs` — +2 -1
- `src/daemon/executor/mod.rs` — +10 -5
- `src/daemon/executor/shell_exec_auth.rs` — +3 -2
- `tests/lock_invariant.rs` — +92 -0

**Commit:** 1d384d8991018e89ce2b21d9c4482afa8642238a

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
