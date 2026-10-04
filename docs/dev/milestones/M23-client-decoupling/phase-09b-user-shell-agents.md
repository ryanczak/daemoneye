# Phase 09b: user-shell-agents — agents see the user's shells and drive them only through an approval that is always asked

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** 09a (done)
**Estimated diff:** ~70 lines of production code across 9 files; 5 new tests (~200 lines in all)
**Tags:** language=rust, kind=feature, size=s

## Goal

After this phase, the shells the user opens with `/shell` (owner `user`,
09a) are visible to agents and reachable by chat agents, **always with an
approval prompt**:

- **Seen:** `list_shells` with its default scope (`"mine"`), and so the
  `[SHELLS]` ambient block that shares its filter, lists the caller's own
  shells **and the user's**. Another session's shells stay out.
- **Driven only through approval:** a chat session may run
  `run_terminal_command { shell: "s7" }` in, or `send_input` into, a
  user-owned shell. Each such call's prompt carries a new classification,
  **`Classification::UserShell { shell }`**, which **no session grant
  covers**. `[A]pprove for session` widens nothing for it, and the client
  asks every time even when the session is approved for commands. The panel
  reads `→ your shell s7 (opened with /shell)`, and the session label reads
  `your shell s7 — always asked`.
- **A ghost never drives the user's shells.** The ownership checks refuse a
  ghost caller as before.
- **`close_shell` is unchanged.** An agent still cannot close a shell it
  does not own, the user's included.

This closes row 09. Row 10 (the live close) remains.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6: "`/shell [host]` opens a
  user-owned shell (owner `User`) agents can see but drive only through
  approval".
- `docs/dev/milestones/M23-client-decoupling/README.md`: row 09b.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `2b94bad6`. Baseline:
`cargo test --lib` → **2214 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2219**/0/14, every other target unmoved
  (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2214p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A–E each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **"Approve for session" is decided in the client.**
  `SessionApproval::auto_approves(class, is_sudo)`
  (`src/cli/commands/approval.rs:87`) answers from the prompt's
  `Classification`. `NewHost` and `EscapeHatch` already never auto-approve,
  and `record_session_approval` records nothing for them. A new variant in
  that same never-arm is the whole "always asked" mechanism:
  ```rust
  crate::ipc::Classification::NewHost { .. }
  | crate::ipc::Classification::EscapeHatch
  | crate::ipc::Classification::UserShell { .. } => false,
  ```
  `stream.rs`'s `approval_session_label` and `tool_call_panel_body` match
  the enum exhaustively too, so each gets an arm.
- **`send_input` was session-approvable.** It prompts through the same
  `ToolCallPrompt` (`executor/mod.rs`, the `PendingCall::SendInput` arm)
  with the default `Command` class. So both doors (the PTY foreground
  `run_terminal_command` and `send_input`) pass the target to the prompt
  through a new `ApprovalRequest { user_shell, .. }` field. The other three
  `ApprovalRequest` sites (tmux foreground ×2, background) pass `None`.
- **`prompt_classification(host, is_ghost, known_hosts, user_shell)`** is a
  new pure function, so the rule is unit-tested without driving a prompt.
- **The three ownership checks** compare owner keys:
  - `shell_exec.rs` (`ShellTarget::Existing`, run a command);
  - `knowledge/shell.rs` `send_input_with` (type);
  - `knowledge/shell.rs` close (unchanged).

  The first two now ask `Owner::may_drive(shell_owner)`: own shells, plus
  `user` shells for a `Chat` caller. The rule sits in one place
  (`src/shell/registry.rs`), next to `Owner::key()`.
- **`resolve_scope`** (`knowledge/shell.rs`) is the single filter behind
  `list_shells` and the `[SHELLS]` block, so widening `"mine"` there covers
  both. The scope error's wording says so: `"mine" (this session's shells
  and the user's, the default)`.
- **An unobservable mutant was dropped.** Reverting `shell_exec.rs`'s check
  to the old key comparison stayed green 3 of 3: no test drives a live
  command into a user shell. The rule it calls (`may_drive`) is pinned by
  Mutations A and E, and the `wired` line pins that `shell_exec` calls it.
- **Known flake:**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
  It is pre-existing and in a file this phase does not touch.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — the rule (`src/shell/registry.rs`, `src/shell/mod.rs`)

```diff
--- a/src/shell/registry.rs
+++ b/src/shell/registry.rs
@@ -71,6 +71,24 @@ impl Owner {
             Owner::User => "user".to_string(),
         }
     }
+
+    /// Whether this caller may run commands in, or type into, a shell whose
+    /// owner key is `shell_owner`: its own shells, and — for a chat session —
+    /// the user's (`/shell`), which every such action reaches only through an
+    /// approval that is always asked. A ghost never drives the user's shells.
+    pub fn may_drive(&self, shell_owner: &str) -> bool {
+        shell_owner == self.key()
+            || (shell_owner == Owner::User.key() && matches!(self, Owner::Chat(_)))
+    }
+}
+
+/// Whether `id` names a shell the user owns (opened with `/shell`).
+pub fn is_user_shell(reg: &ShellRegistry, id: &str) -> bool {
+    with_shells(reg, |store| {
+        store
+            .get(id)
+            .is_some_and(|record| matches!(record.owner, Owner::User))
+    })
 }
 
 /// One entry in the registry, keyed in the underlying map by the id string.
@@ -434,6 +452,25 @@ pub fn reap_exited(reg: &ShellRegistry, retention_secs: u64, now_unix: u64) -> V
 
 #[cfg(test)]
 mod tests {
+    #[test]
+    fn may_drive_reaches_the_users_shells_from_chat_only() {
+        let chat = Owner::Chat("a".into());
+        assert!(chat.may_drive("chat:a"), "its own shell");
+        assert!(chat.may_drive("user"), "the user's shell, through approval");
+        assert!(!chat.may_drive("chat:b"), "another session's shell");
+        assert!(
+            !chat.may_drive("ghost:a"),
+            "a ghost's shell, even with the same id"
+        );
+        let ghost = Owner::Ghost("g".into());
+        assert!(ghost.may_drive("ghost:g"));
+        assert!(
+            !ghost.may_drive("user"),
+            "a ghost never drives the user's shell"
+        );
+        assert!(!chat.may_drive("adopted"));
+    }
+
     use super::*;
     use crate::config::Config;
     use crate::test_home_guard;
```

```diff
--- a/src/shell/mod.rs
+++ b/src/shell/mod.rs
@@ -39,8 +39,8 @@ pub use pty::{
 pub(crate) use pty::{test_begin_marker, test_end_marker};
 pub use registry::{
     Owner, ShellId, ShellRecord, ShellRegistry, adopt_at_startup, allocate_index, check_owner_cap,
-    host_is_alive, mark_interactive, parse_cast_name, reap_exited, scan_shell_ids, spawn_for,
-    spawn_for_on_host, sweep_stale, with_shells,
+    host_is_alive, is_user_shell, mark_interactive, parse_cast_name, reap_exited, scan_shell_ids,
+    spawn_for, spawn_for_on_host, sweep_stale, with_shells,
 };
 pub use screen::ShellScreen;
 pub use session::{
```

### Task 2 — the always-asked class (`src/ipc.rs`, `src/cli/commands/approval.rs`, `src/cli/commands/stream.rs`)

```diff
--- a/src/ipc.rs
+++ b/src/ipc.rs
@@ -537,6 +537,9 @@ pub enum Classification {
     NewHost { host: String },
     /// A ghost shell reaching outside its sandbox.
     EscapeHatch,
+    /// A command or input aimed at a shell the user opened with `/shell`.
+    /// Always asked: no session grant covers the user's own shell.
+    UserShell { shell: String },
 }
 
 /// Classify a tool-call approval from its destination and origin — pure, so
```

```diff
--- a/src/cli/commands/approval.rs
+++ b/src/cli/commands/approval.rs
@@ -103,7 +103,8 @@ impl SessionApproval {
             // An unconfigured destination, and a ghost shell reaching outside
             // its sandbox, are always asked.
             crate::ipc::Classification::NewHost { .. }
-            | crate::ipc::Classification::EscapeHatch => false,
+            | crate::ipc::Classification::EscapeHatch
+            | crate::ipc::Classification::UserShell { .. } => false,
         }
     }
 
@@ -141,7 +142,8 @@ impl SessionApproval {
                 ))
             }
             crate::ipc::Classification::NewHost { .. }
-            | crate::ipc::Classification::EscapeHatch => None,
+            | crate::ipc::Classification::EscapeHatch
+            | crate::ipc::Classification::UserShell { .. } => None,
         }
     }
 
@@ -255,6 +257,22 @@ mod tests {
         assert!(a.file_edits_all);
     }
 
+    #[test]
+    fn user_shell_is_always_asked() {
+        let mut a = SessionApproval::from_config(&approvals_all());
+        let class = crate::ipc::Classification::UserShell { shell: "s7".into() };
+        assert!(
+            !a.auto_approves(&class, false) && !a.auto_approves(&class, true),
+            "every grant on: the user's shell is still asked"
+        );
+        assert_eq!(
+            a.record_session_approval(&class, false),
+            None,
+            "[A] widens nothing for the user's shell"
+        );
+        assert!(!a.auto_approves(&class, false));
+    }
+
     #[test]
     fn from_config_all_false_clears_all_flags() {
         let a = SessionApproval::from_config(&approvals_none());
```

```diff
--- a/src/cli/commands/stream.rs
+++ b/src/cli/commands/stream.rs
@@ -1181,6 +1181,9 @@ pub(super) fn approval_session_label(
             format!("{host} (unconfigured) — always asked")
         }
         crate::ipc::Classification::EscapeHatch => "escape hatch — always asked".to_string(),
+        crate::ipc::Classification::UserShell { shell } => {
+            format!("your shell {shell} — always asked")
+        }
     }
 }
 
@@ -1210,6 +1213,9 @@ pub(super) fn tool_call_panel_body(
         crate::ipc::Classification::EscapeHatch => {
             body.push("→ ghost shell reaching outside its sandbox".to_string());
         }
+        crate::ipc::Classification::UserShell { shell } => {
+            body.push(format!("→ your shell {shell} (opened with /shell)"));
+        }
     }
     body
 }
@@ -1896,6 +1902,22 @@ mod tool_call_panel_tests {
     use super::*;
     use crate::daemon::utils::command_has_sudo;
 
+    #[test]
+    fn the_user_shell_panel_names_the_shell_and_is_always_asked() {
+        let class = crate::ipc::Classification::UserShell { shell: "s7".into() };
+        assert_eq!(
+            tool_call_panel_body("uptime", false, None, &class),
+            vec![
+                "$ uptime".to_string(),
+                "→ your shell s7 (opened with /shell)".to_string()
+            ]
+        );
+        assert_eq!(
+            approval_session_label(&class, false),
+            "your shell s7 — always asked"
+        );
+    }
+
     #[test]
     fn the_local_command_panel_is_unchanged() {
         let body = tool_call_panel_body(
```

### Task 3 — the executor classifies the prompt (`src/daemon/executor/mod.rs`, `src/daemon/executor/foreground.rs`)

```diff
--- a/src/daemon/executor/mod.rs
+++ b/src/daemon/executor/mod.rs
@@ -44,6 +44,25 @@ struct ApprovalRequest<'a> {
     /// host. Carried only so the prompt can classify and name the
     /// destination; the call's own host resolution is untouched.
     host: Option<&'a str>,
+    /// The user-owned shell (`/shell`) the call is aimed at, if any. Such a
+    /// prompt is classified `UserShell`, which no session grant covers.
+    user_shell: Option<&'a str>,
+}
+
+/// The classification an approval prompt carries: `UserShell` when the call
+/// is aimed at a shell the user owns, otherwise `ipc::classify`'s answer.
+fn prompt_classification(
+    host: Option<&str>,
+    is_ghost: bool,
+    known_hosts: &[String],
+    user_shell: Option<&str>,
+) -> crate::ipc::Classification {
+    match user_shell {
+        Some(shell) => crate::ipc::Classification::UserShell {
+            shell: shell.to_string(),
+        },
+        None => crate::ipc::classify(host, is_ghost, known_hosts),
+    }
 }
 
 /// The outcome of a single tool call execution.
@@ -490,6 +509,9 @@ where
                         background: false,
                         target_pane_hint: None,
                         host: Some(host.as_str()),
+                        user_shell: shell
+                            .as_deref()
+                            .filter(|s| crate::shell::is_user_shell(crate::shell::registry(), s)),
                     },
                     session_id,
                     ghost_policy.as_ref(),
@@ -757,6 +779,7 @@ where
                         background: true,
                         target_pane_hint: None,
                         host: None,
+                        user_shell: None,
                     },
                     session_id,
                     ghost_policy.as_ref(),
@@ -1364,6 +1387,8 @@ where
                     background: false,
                     target_pane_hint: None,
                     host: None,
+                    user_shell: crate::shell::is_user_shell(crate::shell::registry(), shell)
+                        .then_some(shell.as_str()),
                 },
                 session_id,
                 None,
@@ -1586,6 +1611,7 @@ where
         background,
         target_pane_hint,
         host,
+        user_shell,
     } = req;
     let mode = if background {
         "background"
@@ -1663,7 +1689,12 @@ where
             command: cmd.to_string(),
             background,
             target_pane: target_pane_hint.map(|s| s.to_string()),
-            classification: crate::ipc::classify(host, ghost_policy.is_some(), &known_hosts),
+            classification: prompt_classification(
+                host,
+                ghost_policy.is_some(),
+                &known_hosts,
+                user_shell,
+            ),
         },
     )
     .await?;
@@ -2161,6 +2192,26 @@ mod ghost_host_gate_tests {
 mod tests {
     use super::*;
 
+    #[test]
+    fn prompt_classification_marks_the_users_shell() {
+        assert_eq!(
+            prompt_classification(None, false, &[], Some("s7")),
+            crate::ipc::Classification::UserShell { shell: "s7".into() },
+            "a call aimed at the user's shell is always asked"
+        );
+        assert_eq!(
+            prompt_classification(None, false, &[], None),
+            crate::ipc::Classification::Command,
+            "anything else is classified as before"
+        );
+        assert_eq!(
+            prompt_classification(Some("pinky"), false, &["pinky".to_string()], None),
+            crate::ipc::Classification::RemoteShell {
+                host: "pinky".into()
+            }
+        );
+    }
+
     #[test]
     fn namespaces_non_ghost_is_global_only() {
         let store: SessionStore = SessionStore::new();
```

```diff
--- a/src/daemon/executor/foreground.rs
+++ b/src/daemon/executor/foreground.rs
@@ -168,6 +168,7 @@ where
             background: false,
             target_pane_hint: target_hint.as_deref(),
             host: None,
+            user_shell: None,
         },
         session_id,
         ghost_policy,
@@ -1091,6 +1092,7 @@ where
             background: true,
             target_pane_hint: None,
             host: None,
+            user_shell: None,
         },
         session_id,
         ghost_policy,
```

### Task 4 — the ownership checks and the listing (`src/daemon/executor/shell_exec.rs`, `src/daemon/executor/knowledge/shell.rs`)

```diff
--- a/src/daemon/executor/shell_exec.rs
+++ b/src/daemon/executor/shell_exec.rs
@@ -238,7 +238,7 @@ where
                     live.join(", ")
                 );
             };
-            if record_owner != owner.key() {
+            if !owner.may_drive(&record_owner) {
                 anyhow::bail!(
                     "Error: shell {} is not yours (it belongs to {record_owner}), so \
                      this session cannot run a command in it. Run the command in one \
```

```diff
--- a/src/daemon/executor/knowledge/shell.rs
+++ b/src/daemon/executor/knowledge/shell.rs
@@ -85,7 +85,7 @@ fn resolve_scope(
         None | Some("mine") => Ok(with_shells(reg, |store| {
             store
                 .values()
-                .filter(|record| record.owner.key() == key)
+                .filter(|record| record.owner.key() == key || matches!(record.owner, Owner::User))
                 .map(collect_row)
                 .collect::<Vec<_>>()
         })),
@@ -94,7 +94,7 @@ fn resolve_scope(
         })),
         Some(other) => Err(format!(
             "Error: unknown {tool} scope {other:?}. Accepted values: \
-             \"mine\" (this session's shells, the default) or \"all\" (every \
+             \"mine\" (this session's shells and the user's, the default) or \"all\" (every \
              shell the daemon is running)."
         )),
     }
@@ -1633,7 +1633,7 @@ where
             )
         };
     };
-    if owner_key != caller.key() {
+    if !caller.may_drive(&owner_key) {
         return format!(
             "Error: shell {shell} is not yours (it belongs to {owner_key}), so this \
              session cannot type into it. If it needs input, the session that \
@@ -4492,6 +4492,61 @@ mod tests {
         );
     }
 
+    #[tokio::test]
+    async fn the_users_shell_is_visible_and_drivable_from_chat_only() {
+        let (_home, _dir) = lifecycle_home();
+        let reg = ShellRegistry::default();
+        insert(&reg, 3, Owner::Chat("a".into()), NOW);
+        insert(&reg, 7, Owner::User, NOW);
+        insert(&reg, 8, Owner::Chat("b".into()), NOW);
+        let mine = list_shells_in(&reg, None, &Owner::Chat("a".into()), NOW);
+        assert!(mine.contains("s3") && mine.contains("s7"), "{mine}");
+        assert!(
+            !mine.contains("s8"),
+            "another session's shell stays out: {mine}"
+        );
+        assert!(crate::shell::is_user_shell(&reg, "s7"));
+        assert!(!crate::shell::is_user_shell(&reg, "s3"));
+
+        let sent: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
+        let send = |bytes: Vec<u8>| {
+            let sent = &sent;
+            async move {
+                sent.lock().unwrap().push(bytes);
+                Ok(())
+            }
+        };
+        let out = send_input_with(
+            &reg,
+            "s7",
+            "ls",
+            true,
+            &Owner::Chat("a".into()),
+            Duration::from_secs(5),
+            send,
+        )
+        .await;
+        assert!(!out.contains("not yours"), "{out}");
+        assert_eq!(
+            sent.lock().unwrap().len(),
+            1,
+            "a chat session types into it"
+        );
+
+        let out = send_input_with(
+            &reg,
+            "s7",
+            "ls",
+            true,
+            &Owner::Ghost("g".into()),
+            Duration::from_secs(5),
+            send,
+        )
+        .await;
+        assert!(out.contains("not yours"), "a ghost is refused: {out}");
+        assert_eq!(sent.lock().unwrap().len(), 1, "nothing reached the shell");
+    }
+
     #[tokio::test]
     async fn send_input_without_enter_sends_no_newline() {
         let (_home, _dir) = lifecycle_home();
```

### Task 5 — tests

All five are in the diffs above. Names pinned:
`may_drive_reaches_the_users_shells_from_chat_only` (`registry.rs`),
`user_shell_is_always_asked` (`approval.rs`),
`the_user_shell_panel_names_the_shell_and_is_always_asked` (`stream.rs`),
`prompt_classification_marks_the_users_shell` (`executor/mod.rs`),
`the_users_shell_is_visible_and_drivable_from_chat_only`
(`knowledge/shell.rs`).

**Test count:** five new, so **2219 passed / 14 ignored**.

### Task 6 — apply and record Mutations A–E (before the capture)

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
**Its first fenced block is the contents of `/tmp/e2e-m23-09b.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2219 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2214/0/14). Both counts are exact. A failure
      of the known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- may_drive_reaches user_shell_is_always_asked the_user_shell_panel prompt_classification_marks the_users_shell_is_visible``
      → **5 passed** (current: 0). That exact filter printed 5 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/2`**
      (current: `0/0/0`):
      ``grep -c 'owner.may_drive(&record_owner)' src/daemon/executor/shell_exec.rs`` → 1;
      ``grep -c 'caller.may_drive(&owner_key)' src/daemon/executor/knowledge/shell.rs`` → 1
      (`send_input` only; `close_shell` keeps its own check);
      ``grep -c 'is_user_shell(crate::shell::registry()' src/daemon/executor/mod.rs`` → 2
      (the foreground and `send_input` prompts).
- [ ] The mutations entry exists and records all five:
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

- **Mutation A** (`registry.rs`, only chat reaches the user's shells):
  change `|| (shell_owner == Owner::User.key() && matches!(self, Owner::Chat(_)))`
  to `|| (shell_owner == Owner::User.key())`. Run
  `shell::registry::tests::may_drive_reaches_the_users_shells_from_chat_only`.
  Expected **red**.
- **Mutation B** (`approval.rs`, never auto-approved): in `auto_approves`,
  replace the two lines
  `            | crate::ipc::Classification::EscapeHatch` /
  `            | crate::ipc::Classification::UserShell { .. } => false,` with
  `            | crate::ipc::Classification::EscapeHatch => false,` /
  `            crate::ipc::Classification::UserShell { .. } => self.regular,`.
  Run `user_shell_is_always_asked`. Expected **red**.
- **Mutation C** (`knowledge/shell.rs`, the user's shells are listed):
  change
  `.filter(|record| record.owner.key() == key || matches!(record.owner, Owner::User))`
  to `.filter(|record| record.owner.key() == key)`. Run
  `the_users_shell_is_visible_and_drivable_from_chat_only`. Expected **red**.
- **Mutation D** (`executor/mod.rs`, the class reaches the prompt): in
  `prompt_classification`, replace the three lines
  `        Some(shell) => crate::ipc::Classification::UserShell {` /
  `            shell: shell.to_string(),` / `        },` with the one line
  `        Some(_) => crate::ipc::classify(host, is_ghost, known_hosts),`. Run
  `prompt_classification_marks_the_users_shell`. Expected **red**.
- **Mutation E** (`knowledge/shell.rs`, `send_input` uses the rule): change
  `    if !caller.may_drive(&owner_key) {` to
  `    if owner_key != caller.key() {`. Run
  `the_users_shell_is_visible_and_drivable_from_chat_only`. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Five new tests:
- `may_drive`: a chat session drives its own shells and the user's, but not
  another session's, a ghost's or an adopted one; a ghost drives its own
  shells, never the user's;
- with every approval grant on, `UserShell` is still not auto-approved
  (regular or sudo), and `[A]` records nothing;
- the panel body and the session label for `UserShell`;
- `prompt_classification`: a user-shell target gives `UserShell`; otherwise
  the old `classify` answers (`Command`, `RemoteShell`);
- a chat session's default listing shows its own shell and the user's but
  not another session's; `is_user_shell` tells them apart; `send_input` from
  a chat session reaches the user's shell, and from a ghost it is refused
  with nothing sent.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- may_drive_reaches user_shell_is_always_asked the_user_shell_panel prompt_classification_marks the_users_shell_is_visible 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'owner.may_drive(&record_owner)' src/daemon/executor/shell_exec.rs)" "$(grep -c 'caller.may_drive(&owner_key)' src/daemon/executor/knowledge/shell.rs)" "$(grep -c 'is_user_shell(crate::shell::registry()' src/daemon/executor/mod.rs)"
} > /tmp/e2e-m23-09b.txt
cat /tmp/e2e-m23-09b.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2219p-0f-14i`;
- `phase_tests=5p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1/2`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
The second is frequent. Neither is in a file this phase touches. If either
fails, name it and re-run the whole block. After three re-runs, stop, paste
the last capture as it is, and name every failing test. Any other failing
test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-09b.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-09b-user-shell-agents.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-09b.txt
diff /tmp/pasted-m23-09b.txt /tmp/e2e-m23-09b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-09b.txt /tmp/e2e-m23-09b.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/registry.rs`, `src/shell/mod.rs`, `src/ipc.rs`,
  `src/cli/commands/approval.rs`, `src/cli/commands/stream.rs`,
  `src/daemon/executor/mod.rs`, `src/daemon/executor/foreground.rs`,
  `src/daemon/executor/shell_exec.rs`,
  `src/daemon/executor/knowledge/shell.rs`: the diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **Closing the user's shell from an agent.** `close_shell` keeps refusing
  shells the caller does not own.
- **Ghost access to user shells.** A ghost has no user present to approve.
- **The model's tool descriptions and `sre.toml`.** The listing and the
  approval change are self-describing (`owner: user`, the always-asked
  panel). The 42-tool count does not move.
- **README.md**: documented at the milestone close.

## Update Log

<!-- entries appended below this line -->
