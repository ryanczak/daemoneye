# Phase 09b: user-shell-agents — agents see the user's shells and drive them only through an approval that is always asked

**Milestone:** M23 — Client decoupling & shell UX
**Status:** done
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

### Update — 2026-10-04 21:03 (started)

**Started by:** Executor (rexyMCP headless run)

Status flipped `todo` → `in-progress` here and in the milestone README's
phase-table row. Applying the dry-run diffs from the Spec file by file, then
running the five named tests, Mutations A–E, and the end-to-end capture.

### Update — 2026-10-04 (mutations)

- **A** `shell::registry::tests::may_drive_reaches_the_users_shells_from_chat_only`
  → **red** (restored, `git diff --stat` back to 9 files / 199 insertions).
- **B** `user_shell_is_always_asked` → **red** (restored).
- **C** `the_users_shell_is_visible_and_drivable_from_chat_only` → **red**
  (restored).
- **D** `prompt_classification_marks_the_users_shell` → **red** (restored).
- **E** `the_users_shell_is_visible_and_drivable_from_chat_only` → **red**
  (restored).

Each mutant was run once with `cargo test --lib -- <name>` as the Spec names
it, restored with the `patch` tool, and `git diff --stat` confirmed to match
its pre-mutation state (9 files, 199 insertions, 11 deletions) after each
restore. Each restored tree passes `cargo fmt --all -- --check` and
`cargo clippy --all-targets --all-features -- -D warnings`.

### Update — 2026-10-04 (end-to-end verification)

Captured mechanically with the Spec's § End-to-end verification block,
redirected to `/tmp/e2e-m23-09b.txt` on a tree whose only uncommitted changes
are this phase's nine source files plus this Update Log (confirmed with
`git status --short` immediately before the run — no mutant was in the tree;
every mutation had been restored and re-checked before the capture). The
fenced block below is the contents of that file, copied unchanged:

```
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2219p-0f-14i
phase_tests=5p-0f
isolation=11 passed; 0 failed
wired=1/1/2
```

PASTE MATCH

Other targets from the same tree, one `cargo test` run (not part of the
capture above): lib 2219/0/14, bug_tracker 6, doc_truth 14, integration 32
(2 ignored), isolation 11 (1 ignored) — unmoved, as the preservation line
requires. `cargo build` and `cargo clippy --all-targets --all-features
-- -D warnings` printed no warnings; `cargo fmt --all -- --check` clean.

Known flakes named in the Spec
(`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`,
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`)
did not appear in the three lib runs above; no re-run was spent.

### Update — 2026-10-04 21:23 (complete, server-authored)

**Summary:** **Summary:** Phase 09b is implemented exactly per the dry-run diffs — `Owner::may_drive` + `is_user_shell` in the registry (Task 1), the new `Classification::UserShell` that no session grant covers (`auto_approves` → false, `record_session_approval` → None) plus its panel/label arms (Task 2), `ApprovalRequest.user_shell` plumbed from the `run_terminal_command` and `send_input` doors through a pure `prompt_classification` (Task 3), the two ownership checks and the `"mine"` listing widened via `resolve_scope` (Task 4), and the five pinned tests (Task 5). Mutations A–E each ran once red and were restored with `git diff --stat` verified after each restore (Task 6); the end-to-end block was captured mechanically to `/tmp/e2e-m23-09b.txt` and pasted unchanged (Task 7).

**Notes for review:** No deviation from the spec — every diff hunk landed as written; the only edits outside the diffs were mechanical rustfmt reflows of two `ApprovalRequest` literal lines the verifier forced by the new field, and the comment line in `auto_approves` was extended to mention the user's own shell so the never-arm comment stays truthful (the spec's diff showed the old two-line comment around the widened arm). End-to-end verification printed `lib_counts=2219p-0f-14i`, `phase_tests=5p-0f`, `isolation=11 passed; 0 failed`, `wired=1/1/2` — all four acceptance-capture values match Expected; the paste check printed `PASTE MATCH` on the first run; neither named flake appeared in the three lib runs, so no re-run budget was spent. Full gate set clean: `cargo fmt --all` (run immediately before `git add`), `cargo build` 0 warnings, `cargo clippy -D warnings` clean, and one full `cargo test` run with lib 2219/0/14 and other targets unmoved (bug_tracker 6, doc_truth 14, integration 32, isolation 11). Status left at `in-progress` with no hand-written `(complete)` entry, per the lifecycle split; tree is clean at `42757a28`.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
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
test webhook_alert_unrankable_severity_passes_gate ... ok
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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
kill: sending signal to 612963 failed: No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M23-client-decoupling/phase-09b-user-shell-agents.md` — +58 -1
- `src/cli/commands/approval.rs` — +22 -4
- `src/cli/commands/stream.rs` — +22 -0
- `src/daemon/executor/foreground.rs` — +2 -0
- `src/daemon/executor/knowledge/shell.rs` — +58 -3
- `src/daemon/executor/mod.rs` — +52 -1
- `src/daemon/executor/shell_exec.rs` — +1 -1
- `src/ipc.rs` — +3 -0
- `src/shell/mod.rs` — +2 -2
- `src/shell/registry.rs` — +37 -0

**Commit:** 42757a2837459888b5ac18ffc46d328b6f4c1bc7

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-04

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (157 turns, per
  `executor_health` and the server-authored entry). Its entry names itself
  "Executor (rexyMCP headless run)": fold C, 20th occurrence.
- **Source diff vs the drafting dry run:** identical **except one comment**.
  The comment above the never-auto-approve arm in `auto_approves` now names
  the user's own shell. **The executor disclosed this accurately**, so its
  account of its own deviations was right this time; that counts against the
  held "misreports its deviations" item (08c, 09a).
- **Reviewer re-run:**
  - fmt, build and clippy `-D warnings` were clean, each run separately;
  - the end-to-end block printed the Expected values exactly
    (`lib_counts=2219p-0f-14i`, `phase_tests=5p-0f`, `wired=1/1/2`);
  - the paste check, run verbatim, printed `PASTE MATCH`;
  - the mutations and end-to-end entries exist (both counts 1);
  - mutations A–E, re-applied exactly as pinned, each went red on 3 of 3 runs
    with zero clippy diagnostics, and the tree restored clean.
- **Known flake, the other one:** one full `cargo test` run failed
  `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
  (2218/1/14). That is the first of the two named flakes, the first time it
  has fired this milestone. It lives in `src/shell/backend.rs`, which this
  phase does not touch, and it passed 5 of 5 runs in isolation. The four lib
  runs in the end-to-end block were all green. Not a finding.
- **Scope deviations:** the one comment, above (an improvement, disclosed).
- **Calibration:** none new. Row 09 closes, both phases first-try.
