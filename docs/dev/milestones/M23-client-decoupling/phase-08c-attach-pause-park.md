# Phase 08c: attach-pause-park — a paused shell parks its ghost run, and the run is told how long

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 08b (done)
**Estimated diff:** ~150 lines of production code (one new file given whole, four diffs); 5 new tests (~370 lines in all)
**Tags:** language=rust, kind=feature, size=m

## Goal

After this phase, `Ctrl-p` in attached mode reaches the **run**, not just
the job:

- when an attached `STOP` really pauses the job (the shell settles
  `Paused`, as 08a measures), the run that owns the shell is **marked
  paused**. A ghost run then **parks at its next turn boundary**: it starts
  no new turn while paused, and its run log gets the note
  `paused by the user from attached mode`;
- `CONT` (a second `Ctrl-p`, or detaching while paused) **wakes** it. Before
  its next turn the ghost is told, as a user message:
  `[The user paused shell s5 for 40 s from attached mode. It is running again.]`.
  The run log notes `resumed after 40 s`;
- a cancel (08b's `Ctrl-c`) ends the park at once, and a cancelled run is
  told nothing;
- a `STOP` that paused nothing (an idle shell) marks nothing;
- when the run finishes, its pause state is swept with its other
  registrations (the `ActiveRunGuard` drop).

The mechanism is a pause registry keyed by run id, `src/daemon/run/pause.rs`
(new), beside the cancel registry, resolved from a shell's owner the same
way 08b's cancel is.

**Row 08 continues:** 08d (accountability) is next.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 "attached mode": "**`Ctrl-p`
  pauses** the shell — `SIGSTOP` … and the owning run parks at its next turn
  boundary with a `Paused` state; `Ctrl-p` again resumes (`SIGCONT`, run
  continues and is told "the user paused s5 for 40 s")."
- `docs/dev/milestones/M23-client-decoupling/README.md`: row 08c.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `195f5328`. Baseline:
`cargo test --lib` → **2204 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2209**/0/14, every other target unmoved
  (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2204p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A–F each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**;
- no `sleep 30` was left running.

### Facts measured for this phase

- **"Paused" in the run log is a `TurnLogEvent::Note`.** `RunState` is
  terminal-only apart from `Running` (`src/daemon/run/mod.rs:313`), and the
  turn log already carries free-text notes
  (`TurnLogEvent::Note { text }`, `src/daemon/run/turn_log.rs:43-44`). The
  park writes two notes; no enum changes. The record is built as
  `emit_turn_started` builds its own (`src/daemon/ghost.rs:387-393`):
  ```rust
  append_turn_event(&TurnLogRecord {
      timestamp: chrono::Utc::now().to_rfc3339(),
      run_id: run_id.clone(),
      event: TurnLogEvent::TurnStarted { turn: turn as u32 },
  });
  ```
- **The turn boundary is the top of the ghost loop.** It already checks the
  cancel signal there (`src/daemon/ghost.rs:771`). The park goes
  **before** that check, so a cancel that arrives while parked is honoured
  without running one more turn. The notice is pushed the way the wrap-up
  message is (`with_sessions` push plus `append_session_message`).
- **The park is pure over the registries**, so it is tested on tokio's
  paused clock with no ghost loop. `park_while_paused(run_id, cancel)`
  returns the line to tell the model, and the loop pushes it. A resume after
  exactly 40 s of virtual time reads `40 s`. The registry uses
  `tokio::time::Instant` so the paused clock applies.
- **Owner → run is 08b's resolution, shared.** `active.rs` gains
  `owner_run(owner_key)` (and a private `owner_session`), and
  `cancel_owner_run` now uses `owner_session` too. Its behaviour is
  unchanged, and 08b's tests still pass.
- **The handler decides on the settled state.** `record_pause` parks only on
  `STOP` → `Paused`, and wakes on `CONT`. The handler reads the owner key
  once and uses it for both 08b's cancel and the pause.
- **Chat runs do not park in this phase.** Only the ghost loop has a park
  point. A chat session's shell is attached from that same chat, which
  cannot be mid-turn while its own attach view is open.
- **A mutant that left a cancel unable to end the park would hang**, not
  fail, under the paused clock, so none is pinned. The cancel exit is
  covered by the test's last case.
- **Known flake:**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  is frequent (4 of 22 lib runs at the 08b drafting). It is pre-existing and
  in a file this phase does not touch.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes below. They are the dry run, formatted and
clippy-clean, tests included. `cargo build --tests` is the check that you
missed nothing. Copy; do not retype from memory.

### Task 1 — create `src/daemon/run/pause.rs` with exactly this content

It is a new file: create it whole with your file-writing tool.

```rust
//! Runs paused from attached mode (M23). `Ctrl-p` stops a shell's foreground
//! job; the run that owns the shell is marked paused here and parks at its
//! next turn boundary. `Ctrl-p` again resumes the job and leaves the run a
//! one-shot notice of how long it was paused, which it reads when it wakes.
//!
//! Keyed by run id, like the cancel registry. An entry is swept when the
//! run's `ActiveRunGuard` drops.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::RunId;
use crate::util::UnpoisonExt;

/// One run's pause state: paused now (the shell and since when), and the
/// notice a resume leaves for the run (the shell and how long).
#[derive(Default)]
struct PauseEntry {
    paused: Option<(String, tokio::time::Instant)>,
    notice: Option<(String, Duration)>,
}

static PAUSES: OnceLock<Mutex<HashMap<String, PauseEntry>>> = OnceLock::new();

fn pauses() -> &'static Mutex<HashMap<String, PauseEntry>> {
    PAUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mark `run_id` paused by the user through `shell`. A run already paused
/// keeps its first shell and start time.
pub fn pause_run(run_id: &RunId, shell: &str) {
    let mut map = pauses().lock().unwrap_or_log();
    let entry = map.entry(run_id.as_str().to_string()).or_default();
    if entry.paused.is_none() {
        entry.paused = Some((shell.to_string(), tokio::time::Instant::now()));
    }
}

/// Resume `run_id`: clear the pause and leave the run a notice of how long
/// it lasted. Returns that duration, or `None` when the run was not paused.
pub fn resume_run(run_id: &RunId) -> Option<Duration> {
    let mut map = pauses().lock().unwrap_or_log();
    let entry = map.get_mut(run_id.as_str())?;
    let (shell, since) = entry.paused.take()?;
    let lasted = since.elapsed();
    entry.notice = Some((shell, lasted));
    Some(lasted)
}

/// Whether `run_id` is paused now.
pub fn is_paused(run_id: &RunId) -> bool {
    pauses()
        .lock()
        .unwrap_or_log()
        .get(run_id.as_str())
        .is_some_and(|entry| entry.paused.is_some())
}

/// Take the notice a resume left for `run_id`: the shell and how long it was
/// paused. `None` once taken, or when there was no pause.
pub fn take_pause_notice(run_id: &RunId) -> Option<(String, Duration)> {
    pauses()
        .lock()
        .unwrap_or_log()
        .get_mut(run_id.as_str())
        .and_then(|entry| entry.notice.take())
}

/// Forget everything about `run_id`. Called when its `ActiveRunGuard` drops.
pub(super) fn clear_pause(run_id: &str) {
    pauses().lock().unwrap_or_log().remove(run_id);
}

/// Pause the run in flight for the owner of an attached shell (`chat:<s>` or
/// `ghost:<s>`). Returns the run paused.
pub fn pause_owner_run(owner_key: &str, shell: &str) -> Option<RunId> {
    let run_id = super::active::owner_run(owner_key)?;
    pause_run(&run_id, shell);
    Some(run_id)
}

/// Resume the run in flight for the owner of an attached shell. Returns the
/// run resumed, if it was paused.
pub fn resume_owner_run(owner_key: &str) -> Option<RunId> {
    let run_id = super::active::owner_run(owner_key)?;
    resume_run(&run_id).map(|_| run_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::cancel::test_support::fresh_run_id;

    #[tokio::test(start_paused = true)]
    async fn pause_resume_leaves_one_notice_of_how_long() {
        let run = fresh_run_id();
        assert!(!is_paused(&run));
        assert_eq!(
            resume_run(&run),
            None,
            "resuming an unpaused run is nothing"
        );

        pause_run(&run, "s5");
        assert!(is_paused(&run));
        tokio::time::advance(Duration::from_secs(10)).await;
        pause_run(&run, "s9");
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            resume_run(&run),
            Some(Duration::from_secs(40)),
            "a second pause keeps the first start"
        );
        assert!(!is_paused(&run));
        assert_eq!(
            take_pause_notice(&run),
            Some(("s5".to_string(), Duration::from_secs(40))),
            "the notice names the first shell"
        );
        assert_eq!(take_pause_notice(&run), None, "the notice is one-shot");

        pause_run(&run, "s5");
        clear_pause(run.as_str());
        assert!(!is_paused(&run), "a cleared run is not paused");
    }

    #[test]
    fn pause_owner_run_reaches_the_owners_run_in_flight() {
        let run = fresh_run_id();
        let session = "pause-owner-session-1";
        let _active_guard = crate::daemon::run::register_active(session, &run);
        assert_eq!(
            pause_owner_run(&format!("ghost:{session}"), "s5"),
            Some(run.clone())
        );
        assert!(is_paused(&run));
        assert_eq!(
            resume_owner_run(&format!("ghost:{session}")),
            Some(run.clone())
        );
        assert!(!is_paused(&run));
        assert_eq!(
            resume_owner_run(&format!("ghost:{session}")),
            None,
            "a run that is not paused is not resumed"
        );
        assert_eq!(pause_owner_run("adopted", "s5"), None);
        assert_eq!(pause_owner_run("chat:pause-owner-idle", "s5"), None);
    }

    #[test]
    fn dropping_the_active_guard_clears_the_pause() {
        let run = fresh_run_id();
        {
            let _active_guard = crate::daemon::run::register_active("pause-guard-session-1", &run);
            pause_run(&run, "s5");
            assert!(is_paused(&run));
        }
        assert!(!is_paused(&run), "a finished run leaves no pause behind");
    }
}
```

### Task 2 — register it, share owner resolution, sweep on drop (`src/daemon/run/mod.rs`, `src/daemon/run/active.rs`)

```diff
--- a/src/daemon/run/mod.rs
+++ b/src/daemon/run/mod.rs
@@ -486,6 +486,7 @@ pub(crate) mod cancel;
 mod guard;
 mod list;
 mod log_read;
+mod pause;
 mod result;
 mod resume;
 mod turn_log;
@@ -504,6 +505,9 @@ pub use log_read::{
     DEFAULT_RUN_LOG_LIMIT, MAX_RUN_LOG_LIMIT, clamp_run_log_limit, get_turn,
     run_log_input_rejected, run_log_search, run_log_tail,
 };
+pub use pause::{
+    is_paused, pause_owner_run, pause_run, resume_owner_run, resume_run, take_pause_notice,
+};
 pub use result::{CancelReason, Cancellation, RunResult, RunStatus};
 pub use resume::{
     ResumeError, ResumePlan, render_seed, resumable_runbook, resume_plan, terminal_outcome,
```

```diff
--- a/src/daemon/run/active.rs
+++ b/src/daemon/run/active.rs
@@ -106,10 +106,21 @@ pub fn cancel_session(session_id: &str, reason: CancelReason) -> bool {
 /// owning run, and neither does a session with nothing in flight. Returns the
 /// run that was cancelled.
 pub fn cancel_owner_run(owner_key: &str, reason: CancelReason) -> Option<RunId> {
-    let session = owner_key
+    cancel_through(owner_session(owner_key)?, reason, CancelDoor::UserAttached)
+}
+
+/// The run in flight for the owner of an attached shell (`chat:<session>` or
+/// `ghost:<session>`), if any.
+pub(super) fn owner_run(owner_key: &str) -> Option<RunId> {
+    run_for_session(owner_session(owner_key)?)
+}
+
+/// The session an owner key names: `chat:<session>` and `ghost:<session>`
+/// name one; `adopted` and anything else name none.
+fn owner_session(owner_key: &str) -> Option<&str> {
+    owner_key
         .strip_prefix("chat:")
-        .or_else(|| owner_key.strip_prefix("ghost:"))?;
-    cancel_through(session, reason, CancelDoor::UserAttached)
+        .or_else(|| owner_key.strip_prefix("ghost:"))
 }
 
 /// Cancel `session_id`'s run in flight through `door`: mark the door, then
@@ -139,6 +150,7 @@ impl Drop for ActiveRunGuard {
         // exit would otherwise sit in the map forever.
         if let Some(run_id) = run_id {
             doors().lock().unwrap_or_log().remove(run_id.as_str());
+            super::pause::clear_pause(run_id.as_str());
         }
     }
 }
```

### Task 3 — the handler parks and wakes the owning run (`src/daemon/server/shells.rs`)

```diff
--- a/src/daemon/server/shells.rs
+++ b/src/daemon/server/shells.rs
@@ -116,14 +116,16 @@ where
         send_response_split(tx, Response::Error(msg)).await?;
         return Ok(());
     }
+    let owner_key = with_shells(reg, |store| store.get(id).map(|record| record.owner.key()));
     // `Ctrl-c` while attached also cancels the run that owns the shell. The
     // cancel is cooperative (the run stops at its next turn boundary), so it
     // is fired first; the signal then ends what the shell is running now.
     let cancelled_run = if sig == ShellSignal::Int {
-        with_shells(reg, |store| store.get(id).map(|record| record.owner.key()))
+        owner_key
+            .as_deref()
             .and_then(|key| {
                 crate::daemon::run::cancel_owner_run(
-                    &key,
+                    key,
                     crate::daemon::run::CancelReason::UserStop,
                 )
             })
@@ -137,16 +139,42 @@ where
         return Ok(());
     }
     let response = match settled_state(&sock, sig).await {
-        Ok(state) => Response::ShellSignalled {
-            state: state_name(state).to_string(),
-            cancelled_run,
-        },
+        Ok(state) => {
+            if let Some(key) = owner_key.as_deref() {
+                record_pause(key, id, sig, state);
+            }
+            Response::ShellSignalled {
+                state: state_name(state).to_string(),
+                cancelled_run,
+            }
+        }
         Err(e) => Response::Error(format!("shell {id}: {e:#}")),
     };
     send_response_split(tx, response).await?;
     Ok(())
 }
 
+/// Tell the run that owns shell `id` (owner `owner_key`) about a pause from
+/// attached mode: a `STOP` that really stopped the job parks the run at its
+/// next turn boundary, and a `CONT` wakes it.
+fn record_pause(
+    owner_key: &str,
+    id: &str,
+    sig: crate::shell::ShellSignal,
+    state: crate::shell::ShellState,
+) {
+    use crate::shell::{ShellSignal, ShellState};
+    match sig {
+        ShellSignal::Stop if state == ShellState::Paused => {
+            crate::daemon::run::pause_owner_run(owner_key, id);
+        }
+        ShellSignal::Cont => {
+            crate::daemon::run::resume_owner_run(owner_key);
+        }
+        _ => {}
+    }
+}
+
 /// The shell's state once `sig` has had its effect: read until it is `Paused`
 /// after `STOP`, or anything but `Paused` after `CONT` or `INT`, or until
 /// `SIGNAL_SETTLE` runs out, when the last state read is the answer.
@@ -1160,6 +1188,61 @@ mod tests {
         .await;
     }
 
+    #[tokio::test]
+    async fn shell_signal_stop_parks_and_cont_wakes_the_owning_run() {
+        use crate::daemon::run::{is_paused, register_active, take_pause_notice};
+        use crate::shell::ShellState;
+        let (_guard, _dir) = shell_test_home();
+        let reg = ShellRegistry::default();
+        let session = "shells-08c-owner";
+        let owner = Owner::Ghost(session.into());
+        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
+            Box::pin(spawn_in_process_host(id))
+        })
+        .await
+        .unwrap();
+        let id = id.as_str().to_string();
+        let run = crate::daemon::run::cancel::test_support::fresh_run_id();
+        let _active_guard = register_active(session, &run);
+
+        assert_eq!(
+            signal_state(&reg, &id, "STOP").await,
+            Ok("Idle".to_string())
+        );
+        assert!(!is_paused(&run), "a STOP that paused nothing parks no run");
+
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_input(&mut tx, &reg, &id, b"sleep 30\n")
+            .await
+            .unwrap();
+        assert!(status_reaches(&id, ShellState::Running).await);
+        assert_eq!(
+            signal_state(&reg, &id, "STOP").await,
+            Ok("Paused".to_string())
+        );
+        assert!(is_paused(&run), "a STOP that paused the job parks its run");
+        assert_eq!(
+            signal_state(&reg, &id, "CONT").await,
+            Ok("Running".to_string())
+        );
+        assert!(!is_paused(&run), "CONT wakes the run");
+        assert_eq!(
+            take_pause_notice(&run).map(|(shell, _)| shell),
+            Some(id.clone()),
+            "the run is told which shell was paused"
+        );
+        let _ = signal_state(&reg, &id, "INT").await;
+        assert!(status_reaches(&id, ShellState::Idle).await);
+
+        let _ = crate::daemon::executor::knowledge::close_shell_with(
+            &reg,
+            &id,
+            &owner,
+            std::time::Duration::from_millis(3000),
+        )
+        .await;
+    }
+
     #[tokio::test]
     async fn shell_signal_refuses_unknown_signals_and_shells() {
         let (_guard, _dir) = shell_test_home();
```

### Task 4 — the ghost loop parks at its turn boundary (`src/daemon/ghost.rs`)

```diff
--- a/src/daemon/ghost.rs
+++ b/src/daemon/ghost.rs
@@ -588,6 +588,41 @@ fn register_ghost_run(
     (cancel_guard, signal, active_guard)
 }
 
+/// How often a parked ghost checks whether the user has resumed it.
+const PAUSE_POLL: Duration = Duration::from_millis(250);
+
+/// Park at a turn boundary while the user has one of this ghost's shells
+/// paused from attached mode. Returns the line to tell the model once it has
+/// been resumed, or `None` when it was not paused or was cancelled while
+/// parked. Both edges are noted in the run log.
+async fn park_while_paused(
+    run_id: &RunId,
+    cancel: &crate::daemon::run::RunCancelSignal,
+) -> Option<String> {
+    let note = |text: String| {
+        append_turn_event(&TurnLogRecord {
+            timestamp: chrono::Utc::now().to_rfc3339(),
+            run_id: run_id.clone(),
+            event: TurnLogEvent::Note { text },
+        })
+    };
+    if crate::daemon::run::is_paused(run_id) {
+        note("paused by the user from attached mode".to_string());
+        while crate::daemon::run::is_paused(run_id) && !cancel.is_cancelled() {
+            tokio::time::sleep(PAUSE_POLL).await;
+        }
+    }
+    if cancel.is_cancelled() {
+        return None;
+    }
+    let (shell, lasted) = crate::daemon::run::take_pause_notice(run_id)?;
+    let secs = lasted.as_secs();
+    note(format!("resumed after {secs} s"));
+    Some(format!(
+        "[The user paused shell {shell} for {secs} s from attached mode. It is running again.]"
+    ))
+}
+
 fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
     crate::daemon::run::take_cancel_door(run_id)
         .map(crate::daemon::run::CancelDoor::stage)
@@ -785,6 +820,26 @@ async fn do_ghost_turn(
     // exactly once before breaking the loop.
     let mut wrap_up_turn = false;
     loop {
+        if let Some(notice) = park_while_paused(run_id, cancel).await {
+            let message = crate::ai::Message {
+                role: "user".to_string(),
+                content: notice,
+                tool_calls: None,
+                tool_results: None,
+                turn: Some(turn + 1),
+            };
+            let pushed = with_sessions(sessions, |store| {
+                if let Some(entry) = store.get_mut(session_id) {
+                    entry.messages.push(message.clone());
+                    true
+                } else {
+                    false
+                }
+            });
+            if pushed {
+                crate::daemon::session::append_session_message(session_id, &message);
+            }
+        }
         if cancel.is_cancelled() {
             log::info!(
                 "Ghost Shell {session_id}: cancelled, stopping before turn {}",
@@ -1844,6 +1899,50 @@ mod tests {
         }
     }
 
+    #[tokio::test(start_paused = true)]
+    async fn park_while_paused_waits_for_the_resume_and_says_how_long() {
+        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
+        use crate::daemon::run::{CancelReason, cancel_run, pause_run, register_run, resume_run};
+        let _home = HomeGuard::new();
+        let id = fresh_run_id();
+        let (_cancel_guard, signal) = register_run(&id);
+        assert_eq!(
+            park_while_paused(&id, &signal).await,
+            None,
+            "a run that is not paused does not wait"
+        );
+
+        pause_run(&id, "s5");
+        let resumer = tokio::spawn({
+            let id = id.clone();
+            async move {
+                tokio::time::sleep(Duration::from_secs(40)).await;
+                resume_run(&id);
+            }
+        });
+        assert_eq!(
+            park_while_paused(&id, &signal).await.as_deref(),
+            Some("[The user paused shell s5 for 40 s from attached mode. It is running again.]"),
+            "it waits for the resume, then says which shell and how long"
+        );
+        resumer.await.unwrap();
+
+        pause_run(&id, "s5");
+        let canceller = tokio::spawn({
+            let id = id.clone();
+            async move {
+                tokio::time::sleep(Duration::from_secs(1)).await;
+                cancel_run(&id, CancelReason::UserStop);
+            }
+        });
+        assert_eq!(
+            park_while_paused(&id, &signal).await,
+            None,
+            "a cancel ends the wait, and a cancelled run is told nothing"
+        );
+        canceller.await.unwrap();
+    }
+
     #[test]
     fn ghost_run_is_cancellable_from_an_attached_shell() {
         use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
```

### Task 5 — tests

All five are above. Names pinned:
in `src/daemon/run/pause.rs` —
`pause_resume_leaves_one_notice_of_how_long`,
`pause_owner_run_reaches_the_owners_run_in_flight`,
`dropping_the_active_guard_clears_the_pause`;
in `src/daemon/ghost.rs` —
`park_while_paused_waits_for_the_resume_and_says_how_long`;
in `src/daemon/server/shells.rs` —
`shell_signal_stop_parks_and_cont_wakes_the_owning_run`.

**Test count:** five new, so **2209 passed / 14 ignored**.

### Task 6 — apply and record Mutations A–F (before the capture)

Do this **after** Tasks 1–5 pass and **before** Task 7. A mutation does not
affect the capture: each one is restored, and `git diff --stat` is checked,
before the next step. Follow § Mutations: for each one, apply it with the
`patch` tool, run its named test once, restore it, and confirm
`git diff --stat` matches. Then create an Update Log entry titled exactly
`### Update — <date> (mutations)` with one line per mutation: its letter,
the test, and the observed result (`red` or `green`).

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-08c.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2209 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2204/0/14). Both counts are exact. A failure
      of the known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- run::pause ghost::tests::park_while_paused shells::tests::shell_signal_stop_parks``
      → **5 passed** (current: 0). That exact filter printed 5 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'park_while_paused(run_id, cancel)' src/daemon/ghost.rs`` → 1;
      ``grep -c 'record_pause(key, id, sig, state)' src/daemon/server/shells.rs`` → 1;
      ``grep -c 'super::pause::clear_pause' src/daemon/run/active.rs`` → 1.
- [ ] The mutations entry exists and records all six:
      ``grep -c '^### Update — .* (mutations)$' <this doc>`` → **1**
      (current: 0).
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's
      contents, unchanged, and the paste check, run as written below,
      prints `PASTE MATCH`, which is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run the named test once with
`cargo test --lib -- <name>`, restore, and record the observed result in the
Task 6 entry. Each was measured red at drafting on 3 of 3 runs, and each
mutant builds clippy-clean.

- **Mutation A** (`pause.rs`, the first pause wins): change
  `    if entry.paused.is_none() {` to `    if true {`. Run
  `run::pause::tests::pause_resume_leaves_one_notice_of_how_long`. Expected
  **red**.
- **Mutation B** (`pause.rs`, the notice carries the duration): change
  `entry.notice = Some((shell, lasted));` to
  `entry.notice = Some((shell, Duration::ZERO));`. Run
  `run::pause::tests::pause_resume_leaves_one_notice_of_how_long`. Expected
  **red**.
- **Mutation C** (`active.rs`, the sweep): change
  `super::pause::clear_pause(run_id.as_str());` to
  `super::pause::clear_pause("");`. Run
  `run::pause::tests::dropping_the_active_guard_clears_the_pause`. Expected
  **red**.
- **Mutation D** (`shells.rs`, only a real pause parks): change
  `ShellSignal::Stop if state == ShellState::Paused => {` to
  `ShellSignal::Stop if state != ShellState::Exited => {`. Run
  `shells::tests::shell_signal_stop_parks_and_cont_wakes_the_owning_run`.
  Expected **red**.
- **Mutation E** (`shells.rs`, `CONT` wakes): delete the line
  `crate::daemon::run::resume_owner_run(owner_key);`. Run
  `shells::tests::shell_signal_stop_parks_and_cont_wakes_the_owning_run`.
  Expected **red**.
- **Mutation F** (`ghost.rs`, the park waits): change
  `    if crate::daemon::run::is_paused(run_id) {` to `    if false {`. Run
  `ghost::tests::park_while_paused_waits_for_the_resume_and_says_how_long`.
  Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Five new tests:
- the registry: resuming an unpaused run is nothing; a second pause keeps
  the first shell and start; resume returns the duration (40 s of virtual
  time) and leaves a one-shot notice; a cleared run is not paused;
- owner resolution: a `ghost:` owner's run in flight is paused and resumed;
  resuming an unpaused run returns nothing; `adopted` and an idle session
  pause nothing;
- dropping the run's `ActiveRunGuard` clears its pause;
- the park: no wait when not paused; with a pause, it waits for a resume 40 s
  later and returns the exact notice; a cancel ends the wait and returns
  nothing;
- **real host:** a `STOP` on an idle shell parks nothing; on a running
  `sleep 30`, `STOP` parks the owning run, `CONT` wakes it, and the notice
  names the shell.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    verdict=$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')
    failed=$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | paste -sd, -)
    printf 'lib%d=%s failed:%s\n' "$i" "$verdict" "${failed:-none}"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'phase_tests=%s\n' "$(cargo test --lib -- run::pause ghost::tests::park_while_paused shells::tests::shell_signal_stop_parks 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'park_while_paused(run_id, cancel)' src/daemon/ghost.rs)" "$(grep -c 'record_pause(key, id, sig, state)' src/daemon/server/shells.rs)" "$(grep -c 'super::pause::clear_pause' src/daemon/run/active.rs)"
} > /tmp/e2e-m23-08c.txt
cat /tmp/e2e-m23-08c.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2209p-0f-14i`;
- `phase_tests=5p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
The second is frequent right now. Neither is in a file this phase touches.
If either fails, name it and re-run the whole block. After three re-runs,
stop, paste the last capture as it is, and name every failing test. Any
other failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-08c.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-08c-attach-pause-park.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-08c.txt
diff /tmp/pasted-m23-08c.txt /tmp/e2e-m23-08c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-08c.txt /tmp/e2e-m23-08c.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/run/mod.rs`, `src/daemon/run/active.rs`,
  `src/daemon/server/shells.rs`, `src/daemon/ghost.rs`: the diffs above.
- `src/daemon/run/pause.rs`: create it with the content above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **Parking chat runs.** Only the ghost loop has a park point (see Facts).
- **Showing a parked run in `/runs` or the attached status line.** The
  record is the run log's notes.
- **Logging attached input, telling the agent what was typed, scrubbing**:
  08d.
- **README.md**: documented at the milestone close.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-10-03 21:57 (started)

**Executor:** executor (M23 phase-08c). Flipped Status to `in-progress` (and the README phase-table row), read the dry-run facts, and am applying Tasks 1–5 as given.
### Update — 2026-10-03 22:07 (mutations)

All six mutations applied with the `patch` tool, each red on its single named run, each restored, `git diff --stat` confirmed unchanged after every restore, and each mutant clippy-clean with `-D warnings`:

- **A** — `run::pause::tests::pause_resume_leaves_one_notice_of_how_long` → **red** (`a second pause keeps the first start: left Some(30s), right Some(40s)`).
- **B** — `run::pause::tests::pause_resume_leaves_one_notice_of_how_long` → **red** (`the notice names the first shell: left Some(("s5", 0ns)), right Some(("s5", 40s))`).
- **C** — `run::pause::tests::dropping_the_active_guard_clears_the_pause` → **red** (`a finished run leaves no pause behind`).
- **D** — `shells::tests::shell_signal_stop_parks_and_cont_wakes_the_owning_run` → **red** (`a STOP that paused nothing parks no run`).
- **E** — `shells::tests::shell_signal_stop_parks_and_cont_wakes_the_owning_run` → **red** (`CONT wakes the run`).
- **F** — `ghost::tests::park_while_paused_waits_for_the_resume_and_says_how_long` → **red** (`it waits for the resume, then says which shell and how long: left None, right Some(...)`).
### Update — 2026-10-03 22:12 (end-to-end verification)

The § End-to-end verification block was run verbatim on this tree ( Tasks 1–5 + mutations restored, `git diff --stat` at its pre-mutation state ); its output is `/tmp/e2e-m23-08c.txt`, whose contents follow unchanged.

```
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2209p-0f-14i
phase_tests=5p-0f
isolation=11 passed; 0 failed
wired=1/1/1
```

All Expected values matched: three clean lib runs, `lib_counts=2209p-0f-14i` (2204 + 5 new), the phase filter printed `5p-0f`, isolation 11/0, and `wired=1/1/1`. The remaining targets were also checked unmoved: bug_tracker 6 passed / 0 failed, doc_truth 14 passed / 0 failed, integration 32 passed / 0 failed — no flake appeared and no `sleep 30` was left running.

Paste check:
PASTE MATCH
