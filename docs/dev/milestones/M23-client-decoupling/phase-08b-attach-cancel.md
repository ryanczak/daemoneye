# Phase 08b: attach-cancel — `Ctrl-c` while attached also cancels the run that owns the shell

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 08a (done)
**Estimated diff:** ~90 lines of production code across 6 files; 4 new tests (~250 lines in all)
**Tags:** language=rust, kind=feature, size=s

## Goal

After this phase, `Ctrl-c` in attached mode does what the design says: it
interrupts the foreground job (08a) **and cooperatively cancels the run that
owns the shell**. The run stops at its next turn boundary, and its run log
records `Cancelled { stage: "user_attached", … }`.

- The shell's owner names the run. A shell owned by `chat:<session>` or
  `ghost:<session>` cancels whatever run is in flight for that session; an
  `adopted` shell, or a session with nothing in flight, cancels nothing.
- **Ghost runs become reachable.** `trigger_ghost_turn` now also registers
  the ghost's session in the session→run map (`register_active`), through a
  small helper `register_ghost_run`. Before this, only chat turns were in
  that map.
- A new cancel door, `CancelDoor::UserAttached`, with stage `"user_attached"`,
  sits beside `Chat` (`"chat_cancel"`) and `Sentinel` (`"stop_sentinel"`).
- `Response::ShellSignalled` gains `cancelled_run: Option<String>`
  (`#[serde(default)]`). The attached status note then reads
  `interrupted · cancelled run <id>`, or plain `interrupted` when no run
  owned the shell.
- Only `INT` cancels. `STOP` and `CONT` never touch a run.

**Row 08 re-cut at drafting:** 08b (this phase) is cancel. **08c** is the
pause side (park the owning run at its next turn boundary while paused, and
tell it how long), and **08d** is accountability.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 "attached mode": "**`Ctrl-c`
  cancels** — cooperative cancel of the owning run (§ 2.3) plus `SIGINT` to
  the foreground group; the run ends `Cancelled{stage: "user_attached"}`."
- `docs/dev/milestones/M23-client-decoupling/README.md`: row 08b.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `1c1d0fdc`. Baseline:
`cargo test --lib` → **2200 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2204**/0/14, every other target unmoved
  (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2200p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A–F each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**;
- no `sleep 30` was left running.

### Facts measured for this phase

- **The cancel doors** (`src/daemon/run/active.rs`). A door marks the run,
  then fires its signal, and the turn reads the mark back as its stage.
  `cancel_session` is the chat door:
  ```rust
  pub fn cancel_session(session_id: &str, reason: CancelReason) -> bool {
      let run_id = match run_for_session(session_id) {
          Some(id) => id,
          None => return false,
      };
      mark_cancel_door(&run_id, CancelDoor::Chat);
      if super::cancel::cancel_run(&run_id, reason) {
          true
      } else {
          take_cancel_door(&run_id);
          false
      }
  }
  ```
  The new door shares this body. It moves into a private `cancel_through`
  that both `cancel_session` and the new `cancel_owner_run` call, so
  `cancel_session`'s behaviour does not change.
- **Ghost runs were not in the session→run map.** `trigger_ghost_turn`
  (`src/daemon/ghost.rs:499-500`) registered the run by id only, which is
  what the stop sentinel needs. The shell owner of a ghost's commands is
  `Owner::Ghost(<the ghost's session id>)` (`src/daemon/ghost.rs:555`), so
  registering that session id makes the owner key resolve. Ghost session ids
  are `ghost-<alert>-<uuid>` (`ghost.rs:251`), so they cannot collide with a
  chat session's id in the shared map.
- **The ghost loop honours the signal at turn boundaries.** It checks
  `cancel.is_cancelled()` before each turn (`src/daemon/ghost.rs:771`) and
  ends with `ghost_terminal_state(…, cancel_stage(run_id), …)`, which reads
  the door mark. So an attached `Ctrl-c` while a ghost runs a command
  interrupts the command (`SIGINT`), and the ghost then stops before its
  next turn as `Cancelled { stage: "user_attached" }`. The handler cancels
  **before** it signals, so the cancel is in place when the interrupted tool
  call returns.
- **`ActiveRunGuard` removes the door mark when it drops**
  (`active.rs`, `impl Drop for ActiveRunGuard`). In `trigger_ghost_turn` the
  stage is read (`cancel_stage`) while the guards are still alive, so the
  mark is consumed before that sweep.
- **`register_run` is in-memory only**
  (`src/daemon/run/cancel.rs:39-51`). The new shells test needs no run
  `HomeGuard` beside `shell_test_home`. Taking both would deadlock: both
  take `crate::test_home_guard()`.
- **A known flake is frequent right now.**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  failed in 4 of 22 full lib runs on the dry-run tree and 1 of 8 at HEAD.
  It is pre-existing, in a file this phase does not touch, and A/B shows no
  increase. Expect the end-to-end block to need a re-run.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`. The tests drive the
  real handler against an in-process shell.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — the attached door and `cancel_owner_run` (`src/daemon/run/active.rs`, `src/daemon/run/mod.rs`)

```diff
--- a/src/daemon/run/active.rs
+++ b/src/daemon/run/active.rs
@@ -39,6 +39,9 @@ pub enum CancelDoor {
     Chat,
     /// The `var/run/stop-<run>` filesystem sentinel (`daemoneye stop <run>`).
     Sentinel,
+    /// The user's `Ctrl-c` while attached to a shell the run owns
+    /// (`Request::ShellSignal` with `INT`), resolved through the shell's owner.
+    UserAttached,
 }
 
 impl CancelDoor {
@@ -48,6 +51,7 @@ impl CancelDoor {
         match self {
             CancelDoor::Chat => "chat_cancel",
             CancelDoor::Sentinel => "stop_sentinel",
+            CancelDoor::UserAttached => "user_attached",
         }
     }
 }
@@ -93,18 +97,33 @@ pub fn run_for_session(session_id: &str) -> Option<RunId> {
 /// line keeps its meaning. Marks the run's door `Chat` before flipping the
 /// signal, so the turn records the chat door's stage.
 pub fn cancel_session(session_id: &str, reason: CancelReason) -> bool {
-    let run_id = match run_for_session(session_id) {
-        Some(id) => id,
-        None => return false,
-    };
-    mark_cancel_door(&run_id, CancelDoor::Chat);
+    cancel_through(session_id, reason, CancelDoor::Chat).is_some()
+}
+
+/// Cancel the run in flight for the owner of an attached shell, through the
+/// `UserAttached` door. `owner_key` is the shell owner's `Owner::key()`:
+/// `chat:<session>` or `ghost:<session>`. An adopted shell (`adopted`) has no
+/// owning run, and neither does a session with nothing in flight. Returns the
+/// run that was cancelled.
+pub fn cancel_owner_run(owner_key: &str, reason: CancelReason) -> Option<RunId> {
+    let session = owner_key
+        .strip_prefix("chat:")
+        .or_else(|| owner_key.strip_prefix("ghost:"))?;
+    cancel_through(session, reason, CancelDoor::UserAttached)
+}
+
+/// Cancel `session_id`'s run in flight through `door`: mark the door, then
+/// fire the signal. Returns the run cancelled, if one was found.
+fn cancel_through(session_id: &str, reason: CancelReason, door: CancelDoor) -> Option<RunId> {
+    let run_id = run_for_session(session_id)?;
+    mark_cancel_door(&run_id, door);
     if super::cancel::cancel_run(&run_id, reason) {
-        true
+        Some(run_id)
     } else {
         // The run deregistered between the lookup and the fire; nobody will
         // consume the mark, so remove it.
         take_cancel_door(&run_id);
-        false
+        None
     }
 }
 
@@ -150,6 +169,41 @@ mod tests {
         format!("active-session-{n}")
     }
 
+    #[test]
+    fn cancel_owner_run_cancels_through_the_attached_door() {
+        let _home = HomeGuard::new();
+        for kind in ["chat", "ghost"] {
+            let session = fresh_session_id();
+            let run_id = fresh_run_id();
+            let (_cancel_guard, signal) = register_run(&run_id);
+            let _active_guard = register_active(&session, &run_id);
+            assert_eq!(
+                cancel_owner_run(&format!("{kind}:{session}"), CancelReason::UserStop),
+                Some(run_id.clone()),
+                "a {kind} owner's run in flight is cancelled"
+            );
+            assert!(signal.is_cancelled());
+            assert_eq!(
+                take_cancel_door(&run_id).map(CancelDoor::stage),
+                Some("user_attached"),
+                "the turn records the attached door"
+            );
+        }
+        let idle = fresh_session_id();
+        for key in [
+            "adopted".to_string(),
+            format!("chat:{idle}"),
+            idle.clone(),
+            format!("user:{idle}"),
+        ] {
+            assert_eq!(
+                cancel_owner_run(&key, CancelReason::UserStop),
+                None,
+                "{key:?} owns no run in flight"
+            );
+        }
+    }
+
     #[test]
     fn active_register_then_run_for_session_returns_that_run() {
         let _home = HomeGuard::new();
```

```diff
--- a/src/daemon/run/mod.rs
+++ b/src/daemon/run/mod.rs
@@ -490,8 +490,8 @@ mod result;
 mod resume;
 mod turn_log;
 pub use active::{
-    ActiveRunGuard, CancelDoor, cancel_session, mark_cancel_door, register_active, run_for_session,
-    take_cancel_door,
+    ActiveRunGuard, CancelDoor, cancel_owner_run, cancel_session, mark_cancel_door,
+    register_active, run_for_session, take_cancel_door,
 };
 pub use cancel::{
     RunCancelGuard, RunCancelHandle, RunCancelSignal, STOP_POLL_INTERVAL, STOP_SENTINEL_PREFIX,
```

### Task 2 — ghost runs register their session (`src/daemon/ghost.rs`)

```diff
--- a/src/daemon/ghost.rs
+++ b/src/daemon/ghost.rs
@@ -497,7 +497,8 @@ pub async fn trigger_ghost_turn(
     })
     .unwrap_or_else(|| ghost_run_kind(None, None, None, ""));
     let mut run_log = RunLogGuard::start(RunId::new(), kind.clone());
-    let (_cancel_guard, cancel_signal) = crate::daemon::run::register_run(run_log.run_id());
+    let (_cancel_guard, cancel_signal, _active_guard) =
+        register_ghost_run(session_id, run_log.run_id());
     tokio::spawn(crate::daemon::run::watch_stop_sentinel(
         run_log.run_id().clone(),
         crate::daemon::run::STOP_POLL_INTERVAL,
@@ -571,6 +572,22 @@ pub async fn trigger_ghost_turn(
     result
 }
 
+/// Register a ghost run for cancellation: by run id, which the stop sentinel
+/// fires, and by the ghost's session id, so `Ctrl-c` while attached to one of
+/// its shells (owner `ghost:<session>`) reaches it too.
+fn register_ghost_run(
+    session_id: &str,
+    run_id: &RunId,
+) -> (
+    crate::daemon::run::RunCancelGuard,
+    crate::daemon::run::RunCancelSignal,
+    crate::daemon::run::ActiveRunGuard,
+) {
+    let (cancel_guard, signal) = crate::daemon::run::register_run(run_id);
+    let active_guard = crate::daemon::run::register_active(session_id, run_id);
+    (cancel_guard, signal, active_guard)
+}
+
 fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
     crate::daemon::run::take_cancel_door(run_id)
         .map(crate::daemon::run::CancelDoor::stage)
@@ -1827,6 +1844,25 @@ mod tests {
         }
     }
 
+    #[test]
+    fn ghost_run_is_cancellable_from_an_attached_shell() {
+        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
+        let _home = HomeGuard::new();
+        let id = fresh_run_id();
+        let session = "gs-attach-cancel-1";
+        let (_cancel_guard, signal, _active_guard) = register_ghost_run(session, &id);
+        assert_eq!(
+            crate::daemon::run::cancel_owner_run(
+                &format!("ghost:{session}"),
+                crate::daemon::run::CancelReason::UserStop
+            ),
+            Some(id.clone()),
+            "the ghost's shell owner key reaches the ghost's run"
+        );
+        assert!(signal.is_cancelled());
+        assert_eq!(cancel_stage(&id), "user_attached");
+    }
+
     #[tokio::test(start_paused = true)]
     async fn ghost_cancel_signal_stops_the_run_and_names_the_sentinel_door() {
         use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
```

### Task 3 — the reply names the cancelled run (`src/ipc.rs`), and `INT` cancels it (`src/daemon/server/shells.rs`)

```diff
--- a/src/ipc.rs
+++ b/src/ipc.rs
@@ -600,8 +600,13 @@ pub enum Response {
     /// Live output from a followed shell, after its `ShellScreen`.
     ShellChunk { bytes: Vec<u8> },
     /// The shell's state once a `ShellSignal` took effect: `"Idle"`,
-    /// `"Running"`, `"Paused"` or `"Exited"`.
-    ShellSignalled { state: String },
+    /// `"Running"`, `"Paused"` or `"Exited"`. After `INT`, `cancelled_run`
+    /// names the run that owned the shell and was cancelled with it.
+    ShellSignalled {
+        state: String,
+        #[serde(default)]
+        cancelled_run: Option<String>,
+    },
     /// The followed shell has exited; no more frames follow.
     ShellExited,
     /// An error occurred on the daemon side.
```

```diff
--- a/src/daemon/server/shells.rs
+++ b/src/daemon/server/shells.rs
@@ -116,6 +116,21 @@ where
         send_response_split(tx, Response::Error(msg)).await?;
         return Ok(());
     }
+    // `Ctrl-c` while attached also cancels the run that owns the shell. The
+    // cancel is cooperative (the run stops at its next turn boundary), so it
+    // is fired first; the signal then ends what the shell is running now.
+    let cancelled_run = if sig == ShellSignal::Int {
+        with_shells(reg, |store| store.get(id).map(|record| record.owner.key()))
+            .and_then(|key| {
+                crate::daemon::run::cancel_owner_run(
+                    &key,
+                    crate::daemon::run::CancelReason::UserStop,
+                )
+            })
+            .map(|run| run.as_str().to_string())
+    } else {
+        None
+    };
     let sock = crate::shell::socket_path(id);
     if let Err(e) = crate::shell::send_signal(&sock, sig, RELAY_TIMEOUT).await {
         send_response_split(tx, Response::Error(format!("shell {id}: {e:#}"))).await?;
@@ -124,6 +139,7 @@ where
     let response = match settled_state(&sock, sig).await {
         Ok(state) => Response::ShellSignalled {
             state: state_name(state).to_string(),
+            cancelled_run,
         },
         Err(e) => Response::Error(format!("shell {id}: {e:#}")),
     };
@@ -1017,7 +1033,7 @@ mod tests {
         let mut tx: Vec<u8> = Vec::new();
         handle_shell_signal(&mut tx, reg, id, signal).await.unwrap();
         match parse_response(&tx) {
-            Response::ShellSignalled { state } => Ok(state),
+            Response::ShellSignalled { state, .. } => Ok(state),
             Response::Error(msg) => Err(msg),
             other => panic!("unexpected reply {other:?}"),
         }
@@ -1092,6 +1108,58 @@ mod tests {
         .await;
     }
 
+    #[tokio::test]
+    async fn shell_signal_int_cancels_the_run_that_owns_the_shell() {
+        use crate::daemon::run::{CancelDoor, register_active, register_run, take_cancel_door};
+        let (_guard, _dir) = shell_test_home();
+        let reg = ShellRegistry::default();
+        let session = "shells-08b-owner";
+        let owner = Owner::Chat(session.into());
+        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
+            Box::pin(spawn_in_process_host(id))
+        })
+        .await
+        .unwrap();
+        let run = crate::daemon::run::cancel::test_support::fresh_run_id();
+        let (_cancel_guard, signal) = register_run(&run);
+        let _active_guard = register_active(session, &run);
+
+        let cancelled = |tx: &[u8]| match parse_response(tx) {
+            Response::ShellSignalled { cancelled_run, .. } => cancelled_run,
+            other => panic!("unexpected reply {other:?}"),
+        };
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_signal(&mut tx, &reg, id.as_str(), "STOP")
+            .await
+            .unwrap();
+        assert_eq!(cancelled(&tx), None, "STOP cancels no run");
+        assert!(!signal.is_cancelled());
+
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_signal(&mut tx, &reg, id.as_str(), "INT")
+            .await
+            .unwrap();
+        assert_eq!(
+            cancelled(&tx).as_deref(),
+            Some(run.as_str()),
+            "INT names the owning run it cancelled"
+        );
+        assert!(signal.is_cancelled());
+        assert_eq!(
+            take_cancel_door(&run),
+            Some(CancelDoor::UserAttached),
+            "the run records the attached door"
+        );
+
+        let _ = crate::daemon::executor::knowledge::close_shell_with(
+            &reg,
+            id.as_str(),
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

### Task 4 — the client note (`src/cli/attach.rs`)

```diff
--- a/src/cli/attach.rs
+++ b/src/cli/attach.rs
@@ -38,8 +38,12 @@ pub enum AttachOut {
 /// The daemon's answer to an `AttachOut::Signal`, fed back into the loop.
 #[derive(Debug, Clone, PartialEq, Eq)]
 pub enum AttachReply {
-    /// The state the shell settled in (`Response::ShellSignalled`).
-    Signalled(String),
+    /// The state the shell settled in (`Response::ShellSignalled`), and the
+    /// run an `INT` cancelled with it, if any.
+    Signalled {
+        state: String,
+        cancelled_run: Option<String>,
+    },
     /// The signal was refused or failed.
     Failed(String),
 }
@@ -160,6 +164,20 @@ pub fn attach_keys(chunk: &[u8], pending: bool, modes: ShellModes) -> AttachKeys
     out
 }
 
+/// The status note for a signal's answer: `sent` is the signal it answers,
+/// `paused` whether the shell is now paused, `cancelled_run` the run an `INT`
+/// cancelled.
+pub fn reply_note(sent: Option<&str>, paused: bool, cancelled_run: Option<&str>) -> Option<String> {
+    match sent {
+        Some("STOP") if !paused => Some("nothing running to pause".to_string()),
+        Some("INT") => Some(match cancelled_run {
+            Some(run) => format!("interrupted · cancelled run {run}"),
+            None => "interrupted".to_string(),
+        }),
+        _ => None,
+    }
+}
+
 /// The grid size for a terminal: every row but the status line.
 fn body_size(rows: u16, cols: u16) -> (u16, u16) {
     (rows.saturating_sub(1).max(1), cols.max(1))
@@ -207,15 +225,9 @@ where
                 let Some(reply) = reply else { continue };
                 let sent = asked.pop_front();
                 match reply {
-                    AttachReply::Signalled(state) => {
+                    AttachReply::Signalled { state, cancelled_run } => {
                         model.paused = state == "Paused";
-                        model.note = match sent {
-                            Some("STOP") if !model.paused => {
-                                Some("nothing running to pause".to_string())
-                            }
-                            Some("INT") => Some("interrupted".to_string()),
-                            _ => None,
-                        };
+                        model.note = reply_note(sent, model.paused, cancelled_run.as_deref());
                     }
                     AttachReply::Failed(msg) => model.note = Some(msg),
                 }
@@ -364,7 +376,13 @@ pub async fn run_attach(
             let reply = crate::cli::commands::ipc_client::request_once(req).await;
             if is_signal {
                 let reply = match reply {
-                    Ok(Response::ShellSignalled { state }) => AttachReply::Signalled(state),
+                    Ok(Response::ShellSignalled {
+                        state,
+                        cancelled_run,
+                    }) => AttachReply::Signalled {
+                        state,
+                        cancelled_run,
+                    },
                     Ok(Response::Error(msg)) => AttachReply::Failed(msg),
                     Ok(other) => AttachReply::Failed(format!("unexpected reply {}", other.kind())),
                     Err(e) => AttachReply::Failed(e.to_string()),
@@ -561,7 +579,10 @@ mod tests {
                         "CONT" => "Running",
                         _ => "Idle",
                     };
-                    let _ = reply_tx.send(AttachReply::Signalled(state.to_string()));
+                    let _ = reply_tx.send(AttachReply::Signalled {
+                        state: state.to_string(),
+                        cancelled_run: None,
+                    });
                 }
                 sent.push(item);
             }
@@ -791,6 +812,25 @@ mod tests {
         assert_eq!(model.note.as_deref(), Some("interrupted"));
     }
 
+    #[test]
+    fn reply_note_names_what_a_signal_did() {
+        assert_eq!(
+            reply_note(Some("STOP"), false, None).as_deref(),
+            Some("nothing running to pause")
+        );
+        assert_eq!(reply_note(Some("STOP"), true, None), None, "PAUSED says it");
+        assert_eq!(reply_note(Some("CONT"), false, None), None);
+        assert_eq!(
+            reply_note(Some("INT"), false, None).as_deref(),
+            Some("interrupted")
+        );
+        assert_eq!(
+            reply_note(Some("INT"), false, Some("0123456789abcdef")).as_deref(),
+            Some("interrupted · cancelled run 0123456789abcdef"),
+            "an INT that cancelled the owning run names it"
+        );
+    }
+
     #[test]
     fn render_tail_shows_the_paused_status_line() {
         use ratatui::Terminal;
```

### Task 5 — tests

All four are in the diffs above. Names pinned:
`cancel_owner_run_cancels_through_the_attached_door` (`active.rs`),
`ghost_run_is_cancellable_from_an_attached_shell` (`ghost.rs`),
`shell_signal_int_cancels_the_run_that_owns_the_shell` (`shells.rs`),
`reply_note_names_what_a_signal_did` (`attach.rs`).

**Test count:** four new, so **2204 passed / 14 ignored**.

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
**Its first fenced block is the contents of `/tmp/e2e-m23-08b.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2204 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2200/0/14). Both counts are exact. A failure
      of the known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- active::tests::cancel_owner_run ghost::tests::ghost_run_is_cancellable shells::tests::shell_signal_int cli::attach::tests::reply_note``
      → **4 passed** (current: 0). That exact filter printed 4 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'register_ghost_run(session_id' src/daemon/ghost.rs`` → 1;
      ``grep -c 'crate::daemon::run::cancel_owner_run' src/daemon/server/shells.rs`` → 1;
      ``grep -c 'CancelDoor::UserAttached => "user_attached"' src/daemon/run/active.rs`` → 1.
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

- **Mutation A** (`active.rs`, the stage): change
  `CancelDoor::UserAttached => "user_attached",` to
  `CancelDoor::UserAttached => "chat_cancel",`. Run
  `active::tests::cancel_owner_run_cancels_through_the_attached_door`.
  Expected **red**.
- **Mutation B** (`active.rs`, ghost owners): change
  `.or_else(|| owner_key.strip_prefix("ghost:"))?;` to
  `.or_else(|| owner_key.strip_prefix("chat:"))?;`. Run
  `active::tests::cancel_owner_run_cancels_through_the_attached_door`.
  Expected **red**.
- **Mutation C** (`active.rs`, the door marked): change
  `cancel_through(session, reason, CancelDoor::UserAttached)` to
  `cancel_through(session, reason, CancelDoor::Chat)`. Run
  `active::tests::cancel_owner_run_cancels_through_the_attached_door`.
  Expected **red**.
- **Mutation D** (`ghost.rs`, the session registered): change
  `let active_guard = crate::daemon::run::register_active(session_id, run_id);`
  to
  `let active_guard = crate::daemon::run::register_active(&format!("{session_id}-x"), run_id);`.
  Run `ghost::tests::ghost_run_is_cancellable_from_an_attached_shell`.
  Expected **red**.
- **Mutation E** (`shells.rs`, only `INT` cancels): change
  `let cancelled_run = if sig == ShellSignal::Int {` to
  `let cancelled_run = if sig == ShellSignal::Stop {`. Run
  `shells::tests::shell_signal_int_cancels_the_run_that_owns_the_shell`.
  Expected **red**.
- **Mutation F** (`attach.rs`, the note names the run): change
  `Some(run) => format!("interrupted · cancelled run {run}"),` to
  `Some(_) => "interrupted".to_string(),`. Run
  `cli::attach::tests::reply_note_names_what_a_signal_did`. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Four new tests:
- `cancel_owner_run`: a `chat:` and a `ghost:` owner each cancel their
  session's run in flight, flip its signal and mark `user_attached`;
  `adopted`, an idle session, a bare id and an unknown kind cancel nothing;
- a ghost run registered through `register_ghost_run` is cancelled through
  its shell's owner key, and its stage reads `user_attached`;
- **real host:** with a run registered for the shell's owner, `STOP` cancels
  nothing; `INT` names that run in `cancelled_run`, flips its signal and
  marks the attached door;
- the status note for each signal's answer, including
  `interrupted · cancelled run <id>`.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- active::tests::cancel_owner_run ghost::tests::ghost_run_is_cancellable shells::tests::shell_signal_int cli::attach::tests::reply_note 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'register_ghost_run(session_id' src/daemon/ghost.rs)" "$(grep -c 'crate::daemon::run::cancel_owner_run' src/daemon/server/shells.rs)" "$(grep -c 'CancelDoor::UserAttached => "user_attached"' src/daemon/run/active.rs)"
} > /tmp/e2e-m23-08b.txt
cat /tmp/e2e-m23-08b.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2204p-0f-14i`;
- `phase_tests=4p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
The second is frequent right now (4 of 22 runs at drafting). Neither is in a
file this phase touches. If either fails, name it and re-run the whole block.
After three re-runs, stop, paste the last capture as it is, and name every
failing test. Any other failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-08b.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-08b-attach-cancel.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-08b.txt
diff /tmp/pasted-m23-08b.txt /tmp/e2e-m23-08b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-08b.txt /tmp/e2e-m23-08b.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/run/active.rs`, `src/daemon/run/mod.rs`, `src/daemon/ghost.rs`,
  `src/ipc.rs`, `src/daemon/server/shells.rs`, `src/cli/attach.rs`: the
  diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **Parking the owning run while paused, and telling it how long**: 08c.
- **Logging attached input, telling the agent, scrubbing**: 08d.
- **Interrupting an in-flight LLM call mid-stream** for a ghost. The ghost
  loop is cooperative at turn boundaries, as it already is for the stop
  sentinel.
- **README.md**: documented at the milestone close.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-10-03 21:06 (started)

**Executor:** Claude (Opus 4.6, headless run)

Status flipped `todo` → `in-progress` here and in the README's phase table.
Starting on the spec's Task 1.
### Update — 2026-10-03 21:20 (mutations)

Tasks 1–5 applied as the spec's diffs; `cargo build --tests` clean; the
phase filter printed **4 passed / 0 failed**; full lib run 2204/0/14.
Mutations A–F applied with the `patch` tool, each run once with
`cargo test --lib -- <name>`, then restored; `git diff --stat` matched its
pre-mutation state (`8 files changed, 236 insertions(+), 27 deletions(-)`)
after every restore.

- **A** (`active.rs`, the stage):
  `active::tests::cancel_owner_run_cancels_through_the_attached_door` → **red**
  (`FAILED. 0 passed; 1 failed`).
- **B** (`active.rs`, ghost owners):
  `active::tests::cancel_owner_run_cancels_through_the_attached_door` → **red**
  (`FAILED. 0 passed; 1 failed`).
- **C** (`active.rs`, the door marked):
  `active::tests::cancel_owner_run_cancels_through_the_attached_door` → **red**
  (`FAILED. 0 passed; 1 failed`).
- **D** (`ghost.rs`, the session registered):
  `ghost::tests::ghost_run_is_cancellable_from_an_attached_shell` → **red**
  (`FAILED. 0 passed; 1 failed`).
- **E** (`shells.rs`, only `INT` cancels):
  `shells::tests::shell_signal_int_cancels_the_run_that_owns_the_shell` →
  **red** (`FAILED. 0 passed; 1 failed`).
- **F** (`attach.rs`, the note names the run):
  `cli::attach::tests::reply_note_names_what_a_signal_did` → **red**
  (`FAILED. 0 passed; 1 failed`).
### Update — 2026-10-03 21:45 (end-to-end verification)

Run per the phase doc's § End-to-end verification block, verbatim, with
nothing else running `cargo test`. The first fenced block below is the
contents of `/tmp/e2e-m23-08b.txt`, copied by a tool, unchanged.

```
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2204p-0f-14i
phase_tests=4p-0f
isolation=11 passed; 0 failed
wired=1/1/1
```
PASTE MATCH
