# Phase 09a: user-shell-open — `/shell [host]` opens a shell the user owns and attaches to it

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 08d (done)
**Estimated diff:** ~120 lines of production code across 9 files; 2 new tests (~200 lines in all)
**Tags:** language=rust, kind=feature, size=s

## Goal

After this phase, `/shell` in `daemoneye chat` opens a new PTY shell **owned
by the user** on the daemon host, and `/shell pinky` opens one on the
configured host `pinky`. Chat then attaches to it at once (07a). There the
user can, for example, pre-authenticate (type an ssh or sudo password) before
an agent needs that host.

- A new owner kind, **`Owner::User`** (key `"user"`), sits beside `Chat`,
  `Ghost` and `Adopted`. `/shells` shows it as `owner:user`.
- A new request, `Request::ShellOpen { host }`, answered by
  `Response::ShellOpened { id }`. The daemon opens the shell on **the same
  path the model's `open_shell` tool takes** (host resolution, the per-owner
  cap, readiness, rollback of a dead host), with `Owner::User` as the
  owner.
- Refusals reach the user as `  ✗ shell: <reason>`, without the model's
  `Error: ` prefix. Examples are an unknown host and the per-owner cap.
- `/help` lists `/shell`. `/shells` is unaffected.

**Row 09 was split at drafting:** 09a (this phase) is opening a user shell;
**09b** lets agents see user shells and drive them only through approval.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6: "`/shell [host]` opens a
  user-owned shell (owner `User`) agents can see but drive only through
  approval; useful to pre-authenticate to a host".
- `docs/dev/milestones/M23-client-decoupling/README.md`: row 09a.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `970f6606`. Baseline:
`cargo test --lib` → **2212 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2214**/0/14, every other target unmoved
  (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2212p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0`, with `open_shell_tests=9p-0f` on
  both trees (preservation);
- Mutations A–D each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **`open_shell` returned only the model's sentence**
  (`src/daemon/executor/knowledge/shell.rs`, `pub async fn open_shell(caller:
  &Owner, host: Option<&str>) -> String`). `/shell` needs the id, so the work
  is split without changing the model's text:
  - `open_shell_id(caller, host) -> Result<(ShellId, HostId), String>` holds
    the host resolution and spawn that `open_shell` had;
  - `open_shell_with_id(…) -> Result<ShellId, String>` holds the
    spawn-and-ready core that `open_shell_with` had;
  - `opened_shell_sentence(id, host)` builds the success sentence;
  - `open_shell` and `open_shell_with` become thin wrappers that produce the
    same strings as before.

  **The 9 existing `open_shell` tests pin those strings**
  (`knowledge::shell::tests::open_shell*`, including
  `open_shell_local_result_is_unchanged`), and the end-to-end block runs
  them (`open_shell_tests=9p-0f`).
- **`open_shell_with` becomes test-only.** After the split, only its tests
  call it, so it gains `#[cfg(test)]`; without that the lib build warns
  `function open_shell_with is never used`. This is a test-only entry
  point, not a silenced lint.
- **`open_shell_id` is re-exported `pub`** from `knowledge/mod.rs`, in the
  wider group with `hosts_context_block`, so `daemon::server` can reach it.
  The `pub(super)` group cannot be widened on re-export (that module's own
  comment says so).
- **`Owner` is matched exhaustively in one place**, `Owner::key()`
  (`src/shell/registry.rs:64-70`). The new variant adds one arm.
- **`/shell` is routed before the slash dispatcher.** `chat.rs` handles it
  in the chat loop, as `/attach` and `/log` are, ahead of
  `slash::handle_slash`, where `/shells` lives (`slash.rs:62`). The parser
  compares the first word exactly, so `/shells` never reaches it.
- **An unobservable mutant was dropped.** Passing `HostId::Local` instead of
  the real host into `open_shell`'s success sentence stayed green 3 of 3.
  A named-host success through the public `open_shell` needs a real ssh
  host; that test (`open_shell_on_pinky_end_to_end`) is `#[ignore]`d. The
  other named-host tests go through `open_shell_with`.
- **Known flake:**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  failed in 3 of 10 lib runs on the dry-run tree. It is pre-existing and in
  a file this phase does not touch.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — the owner kind (`src/shell/registry.rs`)

```diff
--- a/src/shell/registry.rs
+++ b/src/shell/registry.rs
@@ -56,6 +56,8 @@ pub enum Owner {
     /// definition — so an adopted shell is owned by nobody until a later phase
     /// re-claims it.
     Adopted,
+    /// A shell the user opened with `/shell` (M23), from any chat session.
+    User,
 }
 
 impl Owner {
@@ -66,6 +68,7 @@ impl Owner {
             Owner::Chat(id) => format!("chat:{id}"),
             Owner::Ghost(id) => format!("ghost:{id}"),
             Owner::Adopted => "adopted".to_string(),
+            Owner::User => "user".to_string(),
         }
     }
 }
```

### Task 2 — the id-returning open path (`src/daemon/executor/knowledge/shell.rs`, `src/daemon/executor/knowledge/mod.rs`)

```diff
--- a/src/daemon/executor/knowledge/shell.rs
+++ b/src/daemon/executor/knowledge/shell.rs
@@ -1712,6 +1712,19 @@ fn format_err_chain(e: &anyhow::Error) -> String {
 /// through `open_shell_error`, which would wrap a host problem in per-owner
 /// shell-limit advice the model cannot act on.
 pub async fn open_shell(caller: &Owner, host: Option<&str>) -> String {
+    match open_shell_id(caller, host).await {
+        Ok((id, host)) => opened_shell_sentence(&id, &host),
+        Err(msg) => msg,
+    }
+}
+
+/// `open_shell` for a caller that needs the shell's id rather than the
+/// model's sentence: the user's `/shell` (M23). `Ok` carries the id and the
+/// host it landed on; `Err` carries the same sentence `open_shell` returns.
+pub async fn open_shell_id(
+    caller: &Owner,
+    host: Option<&str>,
+) -> Result<(ShellId, crate::host::HostId), String> {
     let max_per_owner = crate::config::Config::load()
         .unwrap_or_default()
         .shells
@@ -1719,13 +1732,13 @@ pub async fn open_shell(caller: &Owner, host: Option<&str>) -> String {
     let host = match host.map(crate::host::HostId::parse) {
         None => crate::host::HostId::Local,
         Some(Ok(parsed)) => parsed,
-        Some(Err(msg)) => return format!("Error: {msg}"),
+        Some(Err(msg)) => return Err(format!("Error: {msg}")),
     };
     // The two arms spawn different programs, so their closure types differ and
-    // each calls `open_shell_with` itself.
-    match &host {
+    // each calls `open_shell_with_id` itself.
+    let id = match &host {
         crate::host::HostId::Local => {
-            open_shell_with(
+            open_shell_with_id(
                 &open_shell_test_registry(),
                 caller,
                 crate::host::HostId::Local,
@@ -1742,9 +1755,9 @@ pub async fn open_shell(caller: &Owner, host: Option<&str>) -> String {
             // dress a host problem in per-owner shell-limit advice the model
             // cannot act on (§ Current state 5).
             if let Err(e) = crate::shell::host_entry(name) {
-                return format!("Error: {e}");
+                return Err(format!("Error: {e}"));
             }
-            open_shell_with(
+            open_shell_with_id(
                 &open_shell_test_registry(),
                 caller,
                 crate::host::HostId::Named(name.clone()),
@@ -1754,7 +1767,8 @@ pub async fn open_shell(caller: &Owner, host: Option<&str>) -> String {
             )
             .await
         }
-    }
+    }?;
+    Ok((id, host))
 }
 
 /// `open_shell` with every effect injected: the registry, the host the
@@ -1762,6 +1776,7 @@ pub async fn open_shell(caller: &Owner, host: Option<&str>) -> String {
 /// spawn step. Tests replace `spawn` with an in-process host because
 /// `spawn_detached_host` re-invokes `current_exe()`, which under a test
 /// binary is not the daemon (07b F4).
+#[cfg(test)]
 pub(super) async fn open_shell_with<F, Fut>(
     reg: &ShellRegistry,
     caller: &Owner,
@@ -1770,6 +1785,35 @@ pub(super) async fn open_shell_with<F, Fut>(
     ready_timeout: Duration,
     spawn: F,
 ) -> String
+where
+    F: FnOnce(ShellId) -> Fut,
+    Fut: std::future::Future<Output = anyhow::Result<()>>,
+{
+    match open_shell_with_id(
+        reg,
+        caller,
+        host.clone(),
+        max_per_owner,
+        ready_timeout,
+        spawn,
+    )
+    .await
+    {
+        Ok(id) => opened_shell_sentence(&id, &host),
+        Err(msg) => msg,
+    }
+}
+
+/// `open_shell_with`'s work: spawn, wait for readiness, and return the id, or
+/// the sentence the model reads when the shell could not be opened.
+async fn open_shell_with_id<F, Fut>(
+    reg: &ShellRegistry,
+    caller: &Owner,
+    host: crate::host::HostId,
+    max_per_owner: u32,
+    ready_timeout: Duration,
+    spawn: F,
+) -> Result<ShellId, String>
 where
     F: FnOnce(ShellId) -> Fut,
     Fut: std::future::Future<Output = anyhow::Result<()>>,
@@ -1792,7 +1836,7 @@ where
         // The per-owner cap lives in `spawn_for`'s `check_owner_cap` breach,
         // returned as text rather than an `Err`: every tool result is a value
         // the model reads, so the refusal must name the configured limit.
-        Err(e) => return open_shell_error(max_per_owner, &e),
+        Err(e) => return Err(open_shell_error(max_per_owner, &e)),
     };
     // `spawn_for` reserved the record and rolled back only a *spawn* failure,
     // so a host that never came up leaves a record behind that would be
@@ -1802,17 +1846,23 @@ where
         with_shells(reg, |store| {
             store.remove(id.as_str());
         });
-        return format!(
+        return Err(format!(
             "Error: shell {} did not become ready, and its empty registry record \
              was removed, so nothing can reach a dead shell: {}.",
             id.as_str(),
             format_err_chain(&e)
-        );
+        ));
     }
+    Ok(id)
+}
+
+/// The sentence the model reads after `open_shell` opened shell `id` on
+/// `host`.
+fn opened_shell_sentence(id: &ShellId, host: &crate::host::HostId) -> String {
     // The local sentence is byte-identical to the pre-host one (`HostId` has
     // no printable form); a named host is appended so the model can see where
     // the shell landed and address follow-up work there.
-    let opened_on = match &host {
+    let opened_on = match host {
         crate::host::HostId::Local => String::new(),
         crate::host::HostId::Named(name) => format!(" It is on host {name}."),
     };
```

```diff
--- a/src/daemon/executor/knowledge/mod.rs
+++ b/src/daemon/executor/knowledge/mod.rs
@@ -23,7 +23,7 @@ pub(super) use shell::{
 // Wider than the group above on purpose: the prompt builders reach it from
 // `daemon`, and a `pub(super)` binding cannot be widened on re-export.
 pub use shell::{
-    ContextBreadth, hosts_context_block, resolve_context_scope, shell_context_block,
+    ContextBreadth, hosts_context_block, open_shell_id, resolve_context_scope, shell_context_block,
     shells_only_context,
 };
 
```

### Task 3 — the request, reply and handler (`src/ipc.rs`, `src/cli/commands/ask.rs`, `src/cli/commands/stream.rs`, `src/daemon/server/mod.rs`, `src/daemon/server/shells.rs`)

```diff
--- a/src/ipc.rs
+++ b/src/ipc.rs
@@ -244,6 +244,12 @@ pub enum Request {
         id: String,
         bytes: Vec<u8>,
     },
+    /// Open a PTY shell owned by the user (`/shell [host]`, M23), on the
+    /// daemon host or on a configured `[hosts.<name>]` host. Answered with
+    /// `Response::ShellOpened`.
+    ShellOpen {
+        host: Option<String>,
+    },
     /// Signal one PTY shell's foreground job from attached mode (M23):
     /// `"STOP"` pauses it, `"CONT"` resumes it, `"INT"` interrupts it.
     /// Answered with `Response::ShellSignalled`.
@@ -599,6 +605,8 @@ pub enum Response {
     },
     /// Live output from a followed shell, after its `ShellScreen`.
     ShellChunk { bytes: Vec<u8> },
+    /// The shell `ShellOpen` opened, owned by the user.
+    ShellOpened { id: String },
     /// The shell's state once a `ShellSignal` took effect: `"Idle"`,
     /// `"Running"`, `"Paused"` or `"Exited"`. After `INT`, `cancelled_run`
     /// names the run that owned the shell and was cancelled with it.
@@ -919,6 +927,7 @@ impl Response {
             Response::ShellScreen { .. } => "ShellScreen",
             Response::ShellChunk { .. } => "ShellChunk",
             Response::ShellSignalled { .. } => "ShellSignalled",
+            Response::ShellOpened { .. } => "ShellOpened",
             Response::ShellExited => "ShellExited",
             Response::Error(_) => "Error",
             Response::SessionInfo { .. } => "SessionInfo",
```

```diff
--- a/src/cli/commands/ask.rs
+++ b/src/cli/commands/ask.rs
@@ -220,6 +220,7 @@ async fn run_ask_raw(query: String) -> Result<()> {
             | Response::ShellScreen { .. }
             | Response::ShellChunk { .. }
             | Response::ShellSignalled { .. }
+            | Response::ShellOpened { .. }
             | Response::ShellExited
             | Response::ContainerStatus { .. }
             | Response::DaemonStatus { .. }
```

```diff
--- a/src/cli/commands/stream.rs
+++ b/src/cli/commands/stream.rs
@@ -759,6 +759,7 @@ pub(super) async fn ask_with_session_ratatui(
             | Response::ShellScreen { .. }
             | Response::ShellChunk { .. }
             | Response::ShellSignalled { .. }
+            | Response::ShellOpened { .. }
             | Response::ShellExited
             | Response::ContainerStatus { .. }
             | Response::LimitsInfo { .. }
```

```diff
--- a/src/daemon/server/mod.rs
+++ b/src/daemon/server/mod.rs
@@ -179,6 +179,9 @@ pub async fn handle_client(
         Request::ShellInput { id, bytes } => {
             shells::handle_shell_input(&mut tx, crate::shell::registry(), &id, &bytes).await?;
         }
+        Request::ShellOpen { host } => {
+            shells::handle_shell_open(&mut tx, host.as_deref()).await?;
+        }
         Request::ShellSignal { id, signal } => {
             shells::handle_shell_signal(&mut tx, crate::shell::registry(), &id, &signal).await?;
         }
```

```diff
--- a/src/daemon/server/shells.rs
+++ b/src/daemon/server/shells.rs
@@ -108,6 +108,26 @@ where
     answer_relay(tx, id, result).await
 }
 
+/// Open a shell owned by the user (`/shell [host]`): the same path the
+/// model's `open_shell` takes, with `Owner::User` as the owner. Answers with
+/// the new shell's id, or the refusal (an unknown host, the per-owner cap).
+pub(super) async fn handle_shell_open<W>(tx: &mut W, host: Option<&str>) -> Result<()>
+where
+    W: tokio::io::AsyncWriteExt + Unpin,
+{
+    let response =
+        match crate::daemon::executor::knowledge::open_shell_id(&crate::shell::Owner::User, host)
+            .await
+        {
+            Ok((id, _)) => Response::ShellOpened {
+                id: id.as_str().to_string(),
+            },
+            Err(msg) => Response::Error(msg.trim_start_matches("Error: ").to_string()),
+        };
+    send_response_split(tx, response).await?;
+    Ok(())
+}
+
 /// How long a signal's effect is waited for. A stopped job reads `T` in
 /// `/proc` only once the kernel has delivered `SIGSTOP`, so the state right
 /// after the ack can still be the old one.
@@ -1327,6 +1347,41 @@ mod tests {
         .await;
     }
 
+    #[tokio::test]
+    async fn shell_open_opens_a_user_owned_shell() {
+        let (_guard, _dir) = shell_test_home();
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_open(&mut tx, None).await.unwrap();
+        let Response::ShellOpened { id } = parse_response(&tx) else {
+            panic!("expected ShellOpened, got {tx:?}");
+        };
+        let owner = with_shells(crate::shell::registry(), |store| {
+            store.get(&id).map(|record| record.owner.key())
+        });
+        assert_eq!(owner.as_deref(), Some("user"), "the user owns it");
+
+        let mut tx: Vec<u8> = Vec::new();
+        handle_shell_open(&mut tx, Some("no-such-host-08x"))
+            .await
+            .unwrap();
+        let Response::Error(msg) = parse_response(&tx) else {
+            panic!("an unknown host is refused, got {tx:?}");
+        };
+        assert!(msg.contains("no-such-host-08x"), "{msg}");
+        assert!(
+            !msg.starts_with("Error: "),
+            "the client adds its own prefix: {msg}"
+        );
+
+        let _ = crate::daemon::executor::knowledge::close_shell_with(
+            crate::shell::registry(),
+            &id,
+            &crate::shell::Owner::User,
+            std::time::Duration::from_millis(3000),
+        )
+        .await;
+    }
+
     #[tokio::test]
     async fn shell_signal_refuses_unknown_signals_and_shells() {
         let (_guard, _dir) = shell_test_home();
```

### Task 4 — `/shell` in chat (`src/cli/commands/chat.rs`)

```diff
--- a/src/cli/commands/chat.rs
+++ b/src/cli/commands/chat.rs
@@ -33,6 +33,17 @@ fn attach_command(query: &str) -> Option<Option<&str>> {
     Some(words.next())
 }
 
+/// `/shell` and its host: `Some(None)` for a bare `/shell` (the daemon
+/// host), `Some(Some(host))` for `/shell <host>`, `None` for anything else —
+/// `/shells` included.
+fn shell_command(query: &str) -> Option<Option<&str>> {
+    let mut words = query.split_whitespace();
+    if words.next() != Some("/shell") {
+        return None;
+    }
+    Some(words.next())
+}
+
 /// What to attach to: `Some(Some(id))` for `/attach <id>`, `Some(None)` for
 /// the picker (`/agents`, or a bare `/attach`), `None` for anything else.
 fn attach_target(query: &str) -> Option<Option<&str>> {
@@ -55,6 +66,7 @@ Commands:
   /log       open one recording       (/log <id>; n/p commands, space replays)
   /attach    type into a live shell   (/attach <id>; ctrl-d detach, twice for EOF)
   /agents    pick a shell to attach   (alias: a bare /attach; ↑↓ enter esc)
+  /shell     open your own shell      (/shell [host]; attaches; agents see it)
 
 Aliases: /models /panes /sessions /approval /new /quit (and help, ? open help).
 
@@ -477,6 +489,44 @@ async fn run_chat_ratatui(ctx: RatatuiCtx<'_>) -> Result<()> {
             );
             continue;
         }
+        if let Some(host) = shell_command(&query) {
+            let opened = super::ipc_client::request_once(crate::ipc::Request::ShellOpen {
+                host: host.map(str::to_string),
+            })
+            .await;
+            match opened {
+                Ok(crate::ipc::Response::ShellOpened { id }) => {
+                    if let Err(e) =
+                        crate::cli::attach::run_attach(stdin, sigwinch, &mut renderer, &id).await
+                    {
+                        let _ = renderer.commit(&format!("  ✗ attach {id}: {e}\n"));
+                    }
+                }
+                Ok(crate::ipc::Response::Error(msg)) => {
+                    let _ = renderer.commit(&format!("  ✗ shell: {msg}\n"));
+                }
+                Ok(other) => {
+                    let _ =
+                        renderer.commit(&format!("  ✗ shell: unexpected reply {}\n", other.kind()));
+                }
+                Err(e) => {
+                    let _ = renderer.commit(&format!("  ✗ shell: {e}\n"));
+                }
+            }
+            redraw(
+                &mut renderer,
+                input_state,
+                &session_id,
+                approval,
+                &model,
+                prompt_tokens,
+                context_window,
+                daemon_up,
+                cost_usd,
+                has_untracked,
+            );
+            continue;
+        }
         if let Some(id) = attach_target(&query) {
             match id {
                 Some(id) => {
@@ -1079,7 +1129,7 @@ fn turn_budget_label(turn: usize, prompt_tokens: u32, context_window: u32) -> St
 
 #[cfg(test)]
 mod tests {
-    use super::{HELP_TEXT, attach_command, attach_target, log_command};
+    use super::{HELP_TEXT, attach_command, attach_target, log_command, shell_command};
 
     #[test]
     fn help_text_lists_shells() {
@@ -1103,6 +1153,24 @@ mod tests {
         assert!(HELP_TEXT.contains("/hosts"), "HELP_TEXT must list '/hosts'");
     }
 
+    #[test]
+    fn shell_command_matches_shell_and_not_shells() {
+        assert_eq!(
+            shell_command("/shell"),
+            Some(None),
+            "bare /shell is the daemon host"
+        );
+        assert_eq!(shell_command("/shell pinky"), Some(Some("pinky")));
+        assert_eq!(shell_command("  /shell   pinky "), Some(Some("pinky")));
+        for other in ["/shells", "/shells mine", "/shellx", "shell pinky", "/sh"] {
+            assert_eq!(shell_command(other), None, "{other:?} is not /shell");
+        }
+        assert!(
+            HELP_TEXT.contains("/shell "),
+            "HELP_TEXT must list '/shell'"
+        );
+    }
+
     #[test]
     fn attach_target_opens_the_picker_for_agents_and_bare_attach() {
         assert_eq!(attach_target("/agents"), Some(None));
```

### Task 5 — tests

Both are in the diffs above. Names pinned:
`shell_open_opens_a_user_owned_shell` (`src/daemon/server/shells.rs`) and
`shell_command_matches_shell_and_not_shells` (`src/cli/commands/chat.rs`).
The 9 existing `open_shell` tests must stay green unchanged.

**Test count:** two new, so **2214 passed / 14 ignored**.

### Task 6 — apply and record Mutations A–D (before the capture)

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
**Its first fenced block is the contents of `/tmp/e2e-m23-09a.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2214 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2212/0/14). Both counts are exact. A failure
      of the known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The existing `open_shell` tests still pass, unchanged:
      ``cargo test --lib -- knowledge::shell::tests::open_shell`` →
      **9 passed / 0 failed** (*preservation*; current: the same).
- [ ] The phase's tests pass:
      ``cargo test --lib -- shells::tests::shell_open chat::tests::shell_command``
      → **2 passed** (current: 0). That exact filter printed 2 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'shells::handle_shell_open' src/daemon/server/mod.rs`` → 1;
      ``grep -c 'Owner::User => "user"' src/shell/registry.rs`` → 1;
      ``grep -c 'if let Some(host) = shell_command(&query)' src/cli/commands/chat.rs`` → 1.
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
Task 6 entry. Each was measured red at drafting on 3 of 3 runs, and each
mutant builds clippy-clean.

- **Mutation A** (`registry.rs`, the key): change
  `Owner::User => "user".to_string(),` to
  `Owner::User => "adopted".to_string(),`. Run
  `shells::tests::shell_open_opens_a_user_owned_shell`. Expected **red**.
- **Mutation B** (`shells.rs`, the user owns it): change
  `open_shell_id(&crate::shell::Owner::User, host)` to
  `open_shell_id(&crate::shell::Owner::Adopted, host)`. Run
  `shells::tests::shell_open_opens_a_user_owned_shell`. Expected **red**.
- **Mutation C** (`shells.rs`, no model prefix for the user): change
  `Err(msg) => Response::Error(msg.trim_start_matches("Error: ").to_string()),`
  to `Err(msg) => Response::Error(msg),`. Run
  `shells::tests::shell_open_opens_a_user_owned_shell`. Expected **red**.
- **Mutation D** (`chat.rs`, `/shell` is not `/shells`): change
  `if words.next() != Some("/shell") {` to
  `if !query.trim_start().starts_with("/shell") {`. Run
  `chat::tests::shell_command_matches_shell_and_not_shells`. Expected
  **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Two new tests, plus the 9 existing `open_shell` tests as the refactor's
guard:
- **real shell:** `ShellOpen` with no host opens a ready shell whose
  registry owner is `user`; with an unknown host it answers `Error` naming
  the host, without the `Error: ` prefix. The shell is closed at the end;
- `/shell` and `/shell <host>` are parsed, and `/shells`, `/shells mine`,
  `/shellx`, `shell pinky` and `/sh` are not `/shell`.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- shells::tests::shell_open chat::tests::shell_command 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'open_shell_tests=%s\n' "$(cargo test --lib -- knowledge::shell::tests::open_shell 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'shells::handle_shell_open' src/daemon/server/mod.rs)" "$(grep -c 'Owner::User => "user"' src/shell/registry.rs)" "$(grep -c 'if let Some(host) = shell_command(&query)' src/cli/commands/chat.rs)"
} > /tmp/e2e-m23-09a.txt
cat /tmp/e2e-m23-09a.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2214p-0f-14i`;
- `phase_tests=2p-0f`;
- `isolation=11 passed; 0 failed`;
- `open_shell_tests=9p-0f`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
The second is frequent (3 of 10 runs at drafting). Neither is in a file this
phase touches. If either fails, name it and re-run the whole block. After
three re-runs, stop, paste the last capture as it is, and name every failing
test. Any other failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-09a.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-09a-user-shell-open.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-09a.txt
diff /tmp/pasted-m23-09a.txt /tmp/e2e-m23-09a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-09a.txt /tmp/e2e-m23-09a.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/registry.rs`, `src/daemon/executor/knowledge/shell.rs`,
  `src/daemon/executor/knowledge/mod.rs`, `src/ipc.rs`,
  `src/cli/commands/ask.rs`, `src/cli/commands/stream.rs`,
  `src/daemon/server/mod.rs`, `src/daemon/server/shells.rs`,
  `src/cli/commands/chat.rs`: the diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **Agents seeing user shells and driving them only through approval**:
  09b.
- **Closing a user shell from chat** (a `/shell close`). `close_shell` and
  the shell's own `exit` work as for any shell.
- **README.md**: documented at the milestone close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-04 11:54 (started)

Applying the pinned diffs from the dry run: `Owner::User`, the `open_shell_id` split, `Request::ShellOpen` / `Response::ShellOpened` with the daemon handler, `/shell` in chat, and the two pinned tests; then Mutations A–D and the end-to-end capture. — Executor (rexyMCP headless run)

### Update — 2026-10-04 11:58 (mutations)

- A (`registry.rs`, `Owner::User` key → `"adopted"`): `shells::tests::shell_open_opens_a_user_owned_shell` — **red** (assertion failed: `the user owns it`, left `Some("adopted")`, right `Some("user")`); restored, `git diff --stat` matched.
- B (`shells.rs`, owner passed → `Owner::Adopted`): `shells::tests::shell_open_opens_a_user_owned_shell` — **red** (same assertion, `Some("adopted")` vs `Some("user")`); restored, `git diff --stat` matched.
- C (`shells.rs`, refusal keeps the model's `Error: ` prefix): `shells::tests::shell_open_opens_a_user_owned_shell` — **red** (`the client adds its own prefix: Error: unknown host "no-such-host-08x"…`); restored, `git diff --stat` matched.
- D (`chat.rs`, first-word compare → `starts_with("/shell")`): `chat::tests::shell_command_matches_shell_and_not_shells` — **red** (`bare /shell is the daemon host`, left `Some(Some("/shell"))`); restored, `git diff --stat` matched.

### Update — 2026-10-04 12:03 (end-to-end verification)

Ran § End-to-end verification verbatim; the capture below is `/tmp/e2e-m23-09a.txt`'s contents, unchanged.

```
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2214p-0f-14i
phase_tests=2p-0f
isolation=11 passed; 0 failed
open_shell_tests=9p-0f
wired=1/1/1
```

Paste check: `PASTE MATCH` (diff of the fenced block against `/tmp/e2e-m23-09a.txt` — empty).
