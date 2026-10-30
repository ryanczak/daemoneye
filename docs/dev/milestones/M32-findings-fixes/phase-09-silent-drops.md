# Phase 09 — No silent drops; a gate holds the lock invariant

**Status:** todo
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
