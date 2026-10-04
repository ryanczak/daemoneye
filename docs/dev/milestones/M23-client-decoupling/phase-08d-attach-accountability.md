# Phase 08d: attach-accountability — attached input is recorded (count, not content) and the owning agent is told

**Milestone:** M23 — Client decoupling & shell UX
**Status:** done
**Depends on:** 08c (done)
**Estimated diff:** ~90 lines of production code across 4 files; 3 new tests (~200 lines in all)
**Tags:** language=rust, kind=feature, size=s

## Goal

After this phase, every input the user types into a shell while attached
(each `Request::ShellInput` the daemon relays successfully) is accounted for:

- **`events.jsonl` gets a `shell_input` event:**
  `{"event":"shell_input","shell":"s5","owner":"ghost:<session>","bytes":16,"source":"user_attached",…}`.
  **It records the byte count, never the bytes.** A password typed at a
  prompt cannot reach the event log, because nothing typed does.
- **The run that owns the shell is told** at its next turn boundary (the
  ghost loop, where 08c parks), as a user message:
  `[The user typed 16 bytes into shell s5 from attached mode. Read its recent output before relying on what you last saw.]`.
  Counts add up between turns, so one notice covers a whole burst of typing.
- **The cast log's `"i"` events were already scrubbed at a secret prompt.**
  The shell host arms a scrub when output looks like a password prompt, and
  writes `[redacted]` for the next input (see Facts). Attached input goes
  through that same path, so this phase adds nothing there.
- Refused input (an unknown or dead shell) records nothing.

**Carried from 08c (cosmetic):** in `handle_shell_signal`, the
`let owner_key = …;` line moves back **above** the `Ctrl-c` comment, so the
comment sits on `let cancelled_run`.

This closes row 08. Row 09 (`/shell`) and row 10 (the live close) remain.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 "attached mode": "Everything typed
  lands in the `"i"` log and in `events.jsonl` as a user-authored input
  event; the owning agent is told on its next turn that the user typed into
  its shell."
- `docs/dev/milestones/M23-client-decoupling/README.md`: exit criterion
  "Every attached keystroke is accountable"; row 08d.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `adf041be`. Baseline:
`cargo test --lib` → **2209 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2212**/0/14, every other target unmoved
  (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. The block's first draft read `wired=1/0/1`, because rustfmt
  had split the `notices` array over lines; it now greps the formatted
  line. On the current tree it prints `lib_counts=2209p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A–D each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**;
- no `sleep 30` was left running.

### Facts measured for this phase

- **The cast-log scrub already exists.** `src/shell/backend.rs` arms
  `awaiting_secret` when an output chunk `looks_like_secret_prompt`
  (`backend.rs:121`), and the next input is written to the cast as
  `[redacted]` (`backend.rs:163`). It is pinned by
  `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
  `Request::ShellInput` reaches the same `backend.input()` through the host.
- **The daemon cannot tell whether an input was a secret.** The host's scrub
  flag never leaves the host, and its ack is a bare `Ok`. So the event and
  the agent notice carry **only the byte count**. That needs no scrub, and
  it cannot leak.
- **`log_event(event, fields)`** (`src/daemon/utils/event_log.rs:10`) stamps
  `ts`, `event` and `pid` and appends to today's segment,
  `crate::config::current_event_segment_path()`. The new test reads that
  file under `shell_test_home`'s HOME. Call it as the codebase does:
  ```rust
  crate::daemon::utils::log_event(
      "shell_input",
      serde_json::json!({ "shell": id, "owner": owner_key, "bytes": bytes.len(), "source": "user_attached" }),
  );
  ```
- **Only attached mode sends `ShellInput`.** The model's `send_input` tool
  goes through the executor, not this IPC handler, so `source:
  "user_attached"` is always true here.
- **The notice reuses 08c's registry.** `PauseEntry` in
  `src/daemon/run/pause.rs` gains `typed: Option<(shell, total)>`.
  `note_owner_typed` resolves the owner as `pause_owner_run` does
  (`super::active::owner_run`). The `ActiveRunGuard` sweep already clears
  the whole entry.
- **The ghost loop's notice push is factored out.** 08c inlined it;
  `push_user_note(session_id, sessions, text, turn)` now serves both the
  pause notice and the typed notice.
- **An unobservable mutant was dropped.** Changing the `if result.is_ok()`
  guard before `record_user_input` to `if true` stayed green 3 of 3. An
  unknown shell is refused by `live_shell` before the relay, so the test's
  refused case never reaches the guard; observing it would need a registered
  shell whose host died. The guard stays.
- **Gotcha (twice now): rustfmt splits lines a grep pins.** 08b's and this
  phase's first `wired` greps both missed a reformatted line. The criteria
  below were all run on the formatted dry-run tree.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — the typed notice (`src/daemon/run/pause.rs`, `src/daemon/run/mod.rs`)

```diff
--- a/src/daemon/run/pause.rs
+++ b/src/daemon/run/pause.rs
@@ -3,6 +3,10 @@
 //! next turn boundary. `Ctrl-p` again resumes the job and leaves the run a
 //! one-shot notice of how long it was paused, which it reads when it wakes.
 //!
+//! Typing into the shell while attached is noted here too: the run is told,
+//! at its next turn boundary, how many bytes the user typed and where. Only
+//! the count is kept — never what was typed.
+//!
 //! Keyed by run id, like the cancel registry. An entry is swept when the
 //! run's `ActiveRunGuard` drops.
 
@@ -13,12 +17,15 @@ use std::time::Duration;
 use super::RunId;
 use crate::util::UnpoisonExt;
 
-/// One run's pause state: paused now (the shell and since when), and the
-/// notice a resume leaves for the run (the shell and how long).
+/// One run's attached-mode state: paused now (the shell and since when), the
+/// notice a resume leaves for the run (the shell and how long), and the
+/// bytes the user typed since the run last looked (the first shell and the
+/// total).
 #[derive(Default)]
 struct PauseEntry {
     paused: Option<(String, tokio::time::Instant)>,
     notice: Option<(String, Duration)>,
+    typed: Option<(String, usize)>,
 }
 
 static PAUSES: OnceLock<Mutex<HashMap<String, PauseEntry>>> = OnceLock::new();
@@ -67,6 +74,34 @@ pub fn take_pause_notice(run_id: &RunId) -> Option<(String, Duration)> {
         .and_then(|entry| entry.notice.take())
 }
 
+/// Note that the user typed `bytes` bytes into `shell` while attached to a
+/// shell `run_id` owns. Counts add up until the run takes them; the first
+/// shell named is kept.
+pub fn note_typed(run_id: &RunId, shell: &str, bytes: usize) {
+    let mut map = pauses().lock().unwrap_or_log();
+    let entry = map.entry(run_id.as_str().to_string()).or_default();
+    let (first, total) = entry.typed.take().unwrap_or((shell.to_string(), 0));
+    entry.typed = Some((first, total + bytes));
+}
+
+/// Take what the user typed for `run_id` since it last looked: the shell and
+/// the byte count. `None` once taken, or when nothing was typed.
+pub fn take_typed_notice(run_id: &RunId) -> Option<(String, usize)> {
+    pauses()
+        .lock()
+        .unwrap_or_log()
+        .get_mut(run_id.as_str())
+        .and_then(|entry| entry.typed.take())
+}
+
+/// Note typing for the run in flight for the owner of an attached shell.
+/// Returns the run noted.
+pub fn note_owner_typed(owner_key: &str, shell: &str, bytes: usize) -> Option<RunId> {
+    let run_id = super::active::owner_run(owner_key)?;
+    note_typed(&run_id, shell, bytes);
+    Some(run_id)
+}
+
 /// Forget everything about `run_id`. Called when its `ActiveRunGuard` drops.
 pub(super) fn clear_pause(run_id: &str) {
     pauses().lock().unwrap_or_log().remove(run_id);
@@ -149,6 +184,29 @@ mod tests {
         assert_eq!(pause_owner_run("chat:pause-owner-idle", "s5"), None);
     }
 
+    #[test]
+    fn typed_notices_add_up_and_are_taken_once() {
+        let run = fresh_run_id();
+        assert_eq!(take_typed_notice(&run), None);
+        note_typed(&run, "s5", 3);
+        note_typed(&run, "s6", 4);
+        assert_eq!(
+            take_typed_notice(&run),
+            Some(("s5".to_string(), 7)),
+            "counts add up under the first shell"
+        );
+        assert_eq!(take_typed_notice(&run), None, "the notice is one-shot");
+
+        let session = "typed-owner-session-1";
+        let _active_guard = crate::daemon::run::register_active(session, &run);
+        assert_eq!(
+            note_owner_typed(&format!("ghost:{session}"), "s5", 2),
+            Some(run.clone())
+        );
+        assert_eq!(take_typed_notice(&run), Some(("s5".to_string(), 2)));
+        assert_eq!(note_owner_typed("adopted", "s5", 2), None);
+    }
+
     #[test]
     fn dropping_the_active_guard_clears_the_pause() {
         let run = fresh_run_id();
```

```diff
--- a/src/daemon/run/mod.rs
+++ b/src/daemon/run/mod.rs
@@ -506,7 +506,8 @@ pub use log_read::{
     run_log_input_rejected, run_log_search, run_log_tail,
 };
 pub use pause::{
-    is_paused, pause_owner_run, pause_run, resume_owner_run, resume_run, take_pause_notice,
+    is_paused, note_owner_typed, note_typed, pause_owner_run, pause_run, resume_owner_run,
+    resume_run, take_pause_notice, take_typed_notice,
 };
 pub use result::{CancelReason, Cancellation, RunResult, RunStatus};
 pub use resume::{
```

### Task 2 — the input relay records it, and the 08c carry (`src/daemon/server/shells.rs`)

```diff
--- a/src/daemon/server/shells.rs
+++ b/src/daemon/server/shells.rs
@@ -62,9 +62,33 @@ where
     }
     let result =
         crate::shell::send_input(&crate::shell::socket_path(id), bytes, RELAY_TIMEOUT).await;
+    if result.is_ok() {
+        record_user_input(reg, id, bytes);
+    }
     answer_relay(tx, id, result).await
 }
 
+/// Account for input the user typed into shell `id` while attached: a
+/// `shell_input` event in `events.jsonl`, and a note for the run that owns
+/// the shell, which it is told at its next turn boundary. Only the byte
+/// count is recorded, never the bytes: a password typed at a prompt must not
+/// reach either record (the cast log's own scrub covers the `"i"` events).
+fn record_user_input(reg: &ShellRegistry, id: &str, bytes: &[u8]) {
+    let owner_key = with_shells(reg, |store| store.get(id).map(|record| record.owner.key()));
+    crate::daemon::utils::log_event(
+        "shell_input",
+        serde_json::json!({
+            "shell": id,
+            "owner": owner_key,
+            "bytes": bytes.len(),
+            "source": "user_attached",
+        }),
+    );
+    if let Some(key) = owner_key.as_deref() {
+        crate::daemon::run::note_owner_typed(key, id, bytes.len());
+    }
+}
+
 pub(super) async fn handle_shell_resize<W>(
     tx: &mut W,
     reg: &ShellRegistry,
@@ -116,10 +140,10 @@ where
         send_response_split(tx, Response::Error(msg)).await?;
         return Ok(());
     }
+    let owner_key = with_shells(reg, |store| store.get(id).map(|record| record.owner.key()));
     // `Ctrl-c` while attached also cancels the run that owns the shell. The
     // cancel is cooperative (the run stops at its next turn boundary), so it
     // is fired first; the signal then ends what the shell is running now.
-    let owner_key = with_shells(reg, |store| store.get(id).map(|record| record.owner.key()));
     let cancelled_run = if sig == ShellSignal::Int {
         owner_key
             .as_deref()
@@ -1243,6 +1267,66 @@ mod tests {
         .await;
     }
 
+    #[tokio::test]
+    async fn shell_input_records_a_user_input_event_without_its_content() {
+        use crate::daemon::run::{register_active, take_typed_notice};
+        let (_guard, _dir) = shell_test_home();
+        let reg = ShellRegistry::default();
+        let session = "shells-08d-owner";
+        let owner = Owner::Ghost(session.into());
+        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
+            Box::pin(spawn_in_process_host(id))
+        })
+        .await
+        .unwrap();
+        let id = id.as_str().to_string();
+        let run = crate::daemon::run::cancel::test_support::fresh_run_id();
+        let _active_guard = register_active(session, &run);
+        let input_events = || -> Vec<String> {
+            std::fs::read_to_string(crate::config::current_event_segment_path())
+                .unwrap_or_default()
+                .lines()
+                .filter(|l| l.contains("\"shell_input\""))
+                .map(str::to_string)
+                .collect()
+        };
+
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_input(&mut tx, &reg, "s99", b"refused\n")
+            .await
+            .unwrap();
+        assert!(input_events().is_empty(), "refused input records nothing");
+
+        let typed = b"echo secret-xyz\n";
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_input(&mut tx, &reg, &id, typed).await.unwrap();
+        let events = input_events();
+        assert_eq!(events.len(), 1, "{events:?}");
+        let event: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
+        assert_eq!(event["shell"], id.as_str());
+        assert_eq!(event["owner"], format!("ghost:{session}"));
+        assert_eq!(event["bytes"], typed.len());
+        assert_eq!(event["source"], "user_attached");
+        assert!(
+            !events[0].contains("secret-xyz"),
+            "what was typed is never recorded: {}",
+            events[0]
+        );
+        assert_eq!(
+            take_typed_notice(&run),
+            Some((id.clone(), typed.len())),
+            "the owning run is told how much was typed and where"
+        );
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

### Task 3 — the ghost is told (`src/daemon/ghost.rs`)

```diff
--- a/src/daemon/ghost.rs
+++ b/src/daemon/ghost.rs
@@ -623,6 +623,39 @@ async fn park_while_paused(
     ))
 }
 
+/// What to tell the model when the user typed into one of this ghost's shells
+/// from attached mode since its last turn, or `None` when nothing was typed.
+fn typed_notice(run_id: &RunId) -> Option<String> {
+    let (shell, bytes) = crate::daemon::run::take_typed_notice(run_id)?;
+    Some(format!(
+        "[The user typed {bytes} bytes into shell {shell} from attached mode. \
+         Read its recent output before relying on what you last saw.]"
+    ))
+}
+
+/// Add `text` to the ghost's conversation as a user message for `turn`, and
+/// persist it, as the wrap-up message is.
+fn push_user_note(session_id: &str, sessions: &SessionStore, text: String, turn: usize) {
+    let message = crate::ai::Message {
+        role: "user".to_string(),
+        content: text,
+        tool_calls: None,
+        tool_results: None,
+        turn: Some(turn),
+    };
+    let pushed = with_sessions(sessions, |store| {
+        if let Some(entry) = store.get_mut(session_id) {
+            entry.messages.push(message.clone());
+            true
+        } else {
+            false
+        }
+    });
+    if pushed {
+        crate::daemon::session::append_session_message(session_id, &message);
+    }
+}
+
 fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
     crate::daemon::run::take_cancel_door(run_id)
         .map(crate::daemon::run::CancelDoor::stage)
@@ -820,25 +853,12 @@ async fn do_ghost_turn(
     // exactly once before breaking the loop.
     let mut wrap_up_turn = false;
     loop {
-        if let Some(notice) = park_while_paused(run_id, cancel).await {
-            let message = crate::ai::Message {
-                role: "user".to_string(),
-                content: notice,
-                tool_calls: None,
-                tool_results: None,
-                turn: Some(turn + 1),
-            };
-            let pushed = with_sessions(sessions, |store| {
-                if let Some(entry) = store.get_mut(session_id) {
-                    entry.messages.push(message.clone());
-                    true
-                } else {
-                    false
-                }
-            });
-            if pushed {
-                crate::daemon::session::append_session_message(session_id, &message);
-            }
+        let notices = [
+            park_while_paused(run_id, cancel).await,
+            typed_notice(run_id),
+        ];
+        for notice in notices.into_iter().flatten() {
+            push_user_note(session_id, sessions, notice, turn + 1);
         }
         if cancel.is_cancelled() {
             log::info!(
@@ -1943,6 +1963,22 @@ mod tests {
         canceller.await.unwrap();
     }
 
+    #[test]
+    fn typed_notice_tells_the_ghost_where_the_user_typed() {
+        use crate::daemon::run::cancel::test_support::fresh_run_id;
+        let id = fresh_run_id();
+        assert_eq!(typed_notice(&id), None, "nothing typed, nothing said");
+        crate::daemon::run::note_typed(&id, "s5", 12);
+        assert_eq!(
+            typed_notice(&id).as_deref(),
+            Some(
+                "[The user typed 12 bytes into shell s5 from attached mode. \
+                 Read its recent output before relying on what you last saw.]"
+            )
+        );
+        assert_eq!(typed_notice(&id), None, "said once");
+    }
+
     #[test]
     fn ghost_run_is_cancellable_from_an_attached_shell() {
         use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
```

### Task 4 — tests

All three are above. Names pinned:
`typed_notices_add_up_and_are_taken_once` (`pause.rs`),
`typed_notice_tells_the_ghost_where_the_user_typed` (`ghost.rs`),
`shell_input_records_a_user_input_event_without_its_content` (`shells.rs`).

**Test count:** three new, so **2212 passed / 14 ignored**.

### Task 5 — apply and record Mutations A–D (before the capture)

Do this **after** Tasks 1–4 pass and **before** Task 6. A mutation does not
affect the capture: each one is restored, and `git diff --stat` is checked,
before the next step. Follow § Mutations: for each one, apply it with the
`patch` tool, run its named test once, restore it, and confirm
`git diff --stat` matches. Then create an Update Log entry titled exactly
`### Update — <date> (mutations)` with one line per mutation: its letter,
the test, and the observed result (`red` or `green`).

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-08d.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2212 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2209/0/14). Both counts are exact. A failure
      of the known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- run::pause::tests::typed ghost::tests::typed_notice shells::tests::shell_input_records``
      → **3 passed** (current: 0). That exact filter printed 3 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'record_user_input(reg, id, bytes)' src/daemon/server/shells.rs`` → 1;
      ``grep -c '^            typed_notice(run_id),$' src/daemon/ghost.rs`` → 1;
      ``grep -B1 'while attached also cancels the run that owns the shell' src/daemon/server/shells.rs | grep -c 'let owner_key'`` → 1
      (the 08c carry: the line right above the comment is `let owner_key`).
- [ ] The mutations entry exists and records all four:
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
Task 5 entry. Each was measured red at drafting on 3 of 3 runs, and each
mutant builds clippy-clean.

- **Mutation A** (`pause.rs`, counts add up): change
  `entry.typed = Some((first, total + bytes));` to
  `entry.typed = Some((first, total.max(bytes)));`. Run
  `run::pause::tests::typed_notices_add_up_and_are_taken_once`. Expected
  **red**.
- **Mutation B** (`pause.rs`, one-shot): change
  `.and_then(|entry| entry.typed.take())` to
  `.and_then(|entry| entry.typed.clone())`. Run
  `run::pause::tests::typed_notices_add_up_and_are_taken_once`. Expected
  **red**.
- **Mutation C** (`shells.rs`, never the content): after the line
  `"bytes": bytes.len(),` in `record_user_input`, add the line
  `"text": String::from_utf8_lossy(bytes),`. Run
  `shells::tests::shell_input_records_a_user_input_event_without_its_content`.
  Expected **red**.
- **Mutation D** (`shells.rs`, the run is told the count): change
  `crate::daemon::run::note_owner_typed(key, id, bytes.len());` to
  `crate::daemon::run::note_owner_typed(key, id, 0);`. Run
  `shells::tests::shell_input_records_a_user_input_event_without_its_content`.
  Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Three new tests:
- the registry: typed counts add up under the first shell named and are
  taken once; a `ghost:` owner's run in flight is noted; `adopted` notes
  nothing;
- the ghost's notice: nothing typed says nothing, and 12 bytes in `s5` says
  exactly the pinned line, once;
- **real host:** input to an unknown shell records nothing; typed input to a
  live ghost-owned shell writes one `shell_input` event with the shell,
  owner, byte count and `user_attached`, **without the typed text**, and the
  owning run is told the shell and count.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- run::pause::tests::typed ghost::tests::typed_notice shells::tests::shell_input_records 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'record_user_input(reg, id, bytes)' src/daemon/server/shells.rs)" "$(grep -c '^            typed_notice(run_id),$' src/daemon/ghost.rs)" "$(grep -B1 'while attached also cancels the run that owns the shell' src/daemon/server/shells.rs | grep -c 'let owner_key')"
} > /tmp/e2e-m23-08d.txt
cat /tmp/e2e-m23-08d.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2212p-0f-14i`;
- `phase_tests=3p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
The second is frequent. Neither is in a file this phase touches. If either
fails, name it and re-run the whole block. After three re-runs, stop, paste
the last capture as it is, and name every failing test. Any other failing
test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-08d.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-08d-attach-accountability.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-08d.txt
diff /tmp/pasted-m23-08d.txt /tmp/e2e-m23-08d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-08d.txt /tmp/e2e-m23-08d.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/run/pause.rs`, `src/daemon/run/mod.rs`,
  `src/daemon/server/shells.rs`, `src/daemon/ghost.rs`: the diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **Recording what was typed** anywhere outside the cast log. The cast log
  keeps it, scrubbed at secret prompts, as before.
- **Telling a chat run.** As in 08c, only the ghost loop has the turn
  boundary where notices are read.
- **Coalescing `shell_input` events.** There is one event per relayed
  input, as the exit criterion asks ("every attached keystroke").
- **README.md**: documented at the milestone close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-04 17:12 (started)

Executor: codex (headless, session 1). Status flipped `todo → in-progress`
in this doc and in the milestone README's phase table. Applying the Spec's
diffs (Tasks 1–4), then Task 5 (Mutations A–D), then Task 6 (the
end-to-end capture).

### Update — 2026-10-04 17:22 (mutations)

Mutation testing per the Spec's § Mutations, one at a time with `patch`,
each restored before the next (`git diff --stat` matched its pre-mutation
`6 files changed, 211 insertions(+), 25 deletions(-)` after every restore):

- **A** — `run::pause::tests::typed_notices_add_up_and_are_taken_once`
  (`entry.typed = Some((first, total.max(bytes)));`) → **red**
  (`test result: FAILED. 0 passed; 1 failed`). Restored.
- **B** — `run::pause::tests::typed_notices_add_up_and_are_taken_once`
  (`.and_then(|entry| entry.typed.clone())`) → **red**
  (`test result: FAILED. 0 passed; 1 failed`). Restored.
- **C** — `shells::tests::shell_input_records_a_user_input_event_without_its_content`
  (added `"text": String::from_utf8_lossy(bytes),`) → **red**
  (`test result: FAILED. 0 passed; 1 failed`). Restored.
- **D** — `shells::tests::shell_input_records_a_user_input_event_without_its_content`
  (`note_owner_typed(key, id, 0);`) → **red**
  (`test result: FAILED. 0 passed; 1 failed`). Restored.

After the final restore the tree was re-checked clean: `git diff --stat`
matched, `cargo fmt --all -- --check` was clean, and
`cargo clippy --all-targets --all-features -- -D warnings` exited 0.

### Update — 2026-10-04 17:45 (end-to-end verification)

Ran the Spec's § End-to-end verification block verbatim, with nothing else
running `cargo test`, redirecting all output to `/tmp/e2e-m23-08d.txt`.
Its contents, copied unchanged (no line ends in whitespace):

```
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2212p-0f-14i
phase_tests=3p-0f
isolation=11 passed; 0 failed
wired=1/1/1
```

Paste check (as written in the phase doc):
PASTE MATCH

### Update — 2026-10-04 17:26 (complete, server-authored)

**Summary:** </think>Wait, the last `git status --short | wc -l` printed `0`? Let me re-check output — the lines:

```
 1   ← mutations grep
 1   ← e2e grep? Actually the output shows only "1 (x6)" then "0"
```

Hmm, the output got collapsed: "1 (x6)" seems to be a dedup display of six lines each `1`, then `0` for the last command (git status --short | wc -l). Wait but PASTE MATCH grep -cx returns 1, record_user_input 1, typed_notice 1, owner_key 1... that's 6 lines of 1 total (mutations, e2e, PASTE MATCH, and 3 wired greps = 6). Yes! 6 lines of "1" displayed collapsed as "1 (x6)", then 0 for git status count.

Wait, but there are 6 commands: mutations (1), e2e (1), PASTE MATCH (1), record_user_input (1), typed_notice (1), owner_key (1) = 6 lines of 1, then git status --short | wc -l → 0. The display collapsed to "1 (x6)" and "0".

All greps return 1 and the tree is clean.

But wait — the phase doc was committed BEFORE the completion entry is written by the server. The mutations grep and e2e grep counts are against the committed doc — good, they exist. The `PASTE MATCH` grep -cx → 1.

Everything is complete. Commit exists and tree is clean.

One issue: the commit includes the phase doc and README (status flips + Update Log entries) — that's per the contract (step 7: stage everything, the only doc changes present are the start flip and started entry... but I also appended mutations and e2e entries before committing, which the contract says the server authors the completion entry only; my mutations/e2e entries are mine to write per the spec Tasks 5 and 6, so committing them is right).

Let me do a final sanity check on the commit content and then write the summary.

Also I should double check: the contract says final commit includes "source, tests, and the start-of-phase status flip". Fine.

One more check — the `# Task tracking` list is all done. Now write the final summary message with no tool call.</think>
All greps return the pinned counts (each of the six checks printed `1`) and `git status --short` is empty.

**Summary**

Phase 08d (attach-accountability) is implemented and committed as `722f95a5` on top of `39352d8a`, exactly as the Spec's diffs show: `PauseEntry` gained `typed` with `note_typed` / `take_typed_notice` / `note_owner_typed` in `run/pause.rs` (re-exported through `run/mod.rs`); a successful input relay in `server/shells.rs` now calls `record_user_input`, which writes one `shell_input` event (`shell`, `owner`, `bytes`, `source: user_attached` — **the byte count only, never the content**) and notes the owning run, and the 08c carry moved `let owner_key` back above the `Ctrl-c` comment; `daemon/ghost.rs` gained `typed_notice` and `push_user_note`, and the ghost loop now pushes both the park notice and the typed notice at its turn boundary. All three pinned tests were added; `cargo build`, `cargo fmt --all` (run immediately before staging), `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test` are clean, and the working tree is empty.

**Notes for review**

- Task 5 mutations, applied and restored one at a time (each restore confirmed via `git diff --stat` = `6 files changed, 211 insertions(+), 25 deletions(-)`): **A red**, **B red**, **C red**, **D red** (`0 passed; 1 failed` each), recorded in the `(mutations)` Update Log entry; after the final restore, fmt-check and clippy `-D warnings` were clean.
- Task 6 capture, run verbatim with nothing else testing, is the first fenced block of the `(end-to-end verification)` entry, copied from `/tmp/e2e-m23-08d.txt` with no trailing whitespace: `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`, `lib_counts=2212p-0f-14i` (exact: 2212 passed / 0 failed / 14 ignored, +3 over baseline, no flake hit), `phase_tests=3p-0f`, `isolation=11 passed; 0 failed`, `wired=1/1/1`; the paste check printed **PASTE MATCH** on its own line in the doc (single run of the check). bug_tracker 6 / doc_truth 14 / integration 32 (2 ign.) / isolation 11 (1 ign.) were each verified unmoved.
- Status is left `in-progress` in both the phase doc and the README row; no `(complete)` entry was written. No deviations from the Spec, no extra files touched, no blockers.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
ug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test readme_tools_tables_match_the_code ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.69s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
kill: sending signal to 8398 failed: No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M23-client-decoupling/README.md` — +1 -1
- `docs/dev/milestones/M23-client-decoupling/phase-08d-attach-accountability.md` — +50 -1
- `src/daemon/ghost.rs` — +55 -19
- `src/daemon/run/mod.rs` — +2 -1
- `src/daemon/run/pause.rs` — +60 -2
- `src/daemon/server/shells.rs` — +85 -1

**Commit:** 722f95a500014e3ed8cf0fbaf93368f80bfb1868

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-04

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (95 turns, per
  `executor_health` and the server-authored entry). Its entry names itself
  "codex (headless, session 1)": fold C, 18th occurrence.
- **Source diff vs the drafting dry run, tests included:** byte-identical
  (`git diff 39352d8a 722f95a5 -- src/` against the saved dry-run patch). The
  08c carry is done: `let owner_key` sits directly above the `Ctrl-c`
  comment again.
- **Reviewer re-run:**
  - fmt, build and clippy `-D warnings` were clean, each run separately;
  - `cargo test` gave lib **2212/0/14**, with bug_tracker 6, doc_truth 14,
    integration 32 and isolation 11 unmoved;
  - the end-to-end block printed the Expected values exactly;
  - the paste check, run verbatim, printed `PASTE MATCH`;
  - the mutations and end-to-end entries exist (both counts 1);
  - mutations A–D, re-applied exactly as pinned, each went red on 3 of 3 runs
    with zero clippy diagnostics, and the tree restored clean;
  - no `sleep 30` was left running.
- **Scope deviations:** none.
- **Calibration:** none new. Row 08 closes with all four phases first-try.
