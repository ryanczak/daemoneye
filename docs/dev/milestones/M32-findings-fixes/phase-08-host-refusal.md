# Phase 08 — `edit_file` logs a failed connect; one host lookup per foreground call

**Status:** in-progress
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
not edited. Findings from the two edited functions are in the M32 ledger as
`### Findings — 2026-10-30` (#13, #14). All five greps print the expected
values (`helper_calls=2 lookups=1 refuse=1`, `#[allow` 0/0), the helper test
passes, fmt/build/clippy are clean, and the full suite reads
`passed=2605 failed=0`.

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

