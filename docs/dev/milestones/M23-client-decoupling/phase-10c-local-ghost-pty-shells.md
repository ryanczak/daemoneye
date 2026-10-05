# Phase 10c: local-ghost-pty-shells — a local ghost's commands run in a PTY shell it owns, so `/agents` can attach to it

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 10b (done: a ghost starts without tmux)
**Estimated diff:** ~35 lines of production code and ~10 lines of prompt text across 3 files; 2 new tests and one test repair (~85 lines)
**Tags:** language=rust, kind=feature, size=s

## Goal

A ghost runs every command through `run_background` (its prompt tells it to
pass `wait: false`). There, a ghost whose `ssh_target` resolved to a
configured host already runs the command in a PTY shell **on that host**,
owned by the ghost (`Owner::Ghost(<session>)`). A **local** ghost, with no
`ssh_target`, still falls through to `run_background_in_window`, a tmux
window. So with no tmux server a local ghost cannot run a command at all,
and `/agents` has no ghost shell to attach to.

After this phase:

- **A local ghost's approved command runs in a PTY shell on the daemon
  host**, owned by the ghost and reused for its later commands. This is
  exactly the remote branch's call, `run_ghost_remote`, with
  `HostId::Local` and the chat spawn step (`spawn_detached_host`). It runs
  when:
  - `[execution] backend` is `"pty"` (the default);
  - `[sandbox] enabled` is false;
  - the ghost has no `ssh_target`.

  The decision is a pure function, `ghost_local_pty`.
- **Unchanged:** the sandboxed local command (it keeps its container in a
  tmux window; row 10d moves it), `backend = "tmux"`, an unresolved
  `ssh_target` (the 1.x `ssh <target>` wrap), and every gate before the
  branch: the `hosts:` fence, `resolve_command`, the approval and the
  remote-script build.
- **The ghost prompt stops saying its commands run in tmux windows.** Rule 7
  of `assets/prompts/ghost-shell.txt` now says a command in a shell the
  ghost owns returns its output and exit code as the result, and only a
  `de-gs-*` window's command reports later as `[Background Task Completed]`.
- **A regression test keeps pinning the tmux window path.**
  `run_background_resolves_ghost_script_paths` drives a local ghost into a
  private tmux server. With the PTY default it would now take the new
  branch, so it writes `[execution] backend = "tmux"` into its test
  config.
- The ghost's shell already closes when the ghost exits:
  `close_owner_shells` for `Owner::Ghost(session)` runs on every host
  (`src/daemon/ghost.rs`, `ghost_shells_closed`).

This is row 10c. Row 10d (a sandboxed local ghost: docker inside its PTY
shell, PE decision 2026-10-04) and row 10e (the live close) follow.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.9: "ghosts default to sandboxed local
  shells". This phase gives local ghosts their shell; 10d adds the sandbox.
- `docs/dev/milestones/M23-client-decoupling/README.md` § "PE decision at
  row 10 (2026-10-04)".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `08c353e0`. Baseline:
`cargo test --lib` gives **2221 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2223**/0/14, and every other target was unmoved
  (bug_tracker 6, doc_truth 15, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2221p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0/1`;
- Mutations A and B each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **The real door, measured.** A throwaway daemon ran in its own `HOME` and
  `TMUX_TMPDIR`, with `tmux_session = ""`, the local GLM model and a
  webhook. A one-step runbook told the ghost to run `echo GHOST_PTY_OK; tty`.
  - **Current tree:** the ghost's call reached `run_background_in_window`,
    which failed with `Failed to create job window 'de-gs-bg-…': error
    connecting to …/tmux-1000/default (No such file or directory)`. No cast
    log was written. The command never ran.
  - **Dry-run tree:** the command ran in PTY shell `s0` (`/dev/pts/12`), and
    the tool result carried `GHOST_PTY_OK`. The ghost completed,
    `ghost_shells_closed` named `["s0"]`, and no `tmux-*` directory
    appeared.

  The model passed `background: true`, the legacy alias that also reaches
  `run_background`.
- **The regression the dry run caught.**
  `run_background_resolves_ghost_script_paths` failed 3 runs in 3 on the
  first dry run: the local ghost took the new branch, and
  `spawn_detached_host` cannot run under the test binary (it re-invokes
  `current_exe()`). The test exists to pin the tmux window path, so it now
  selects that path through config. The idiom for writing a test config is
  already in this file (`foreground.rs`, the `ghost-command-host-unlisted`
  test):
  ```rust
  std::fs::write(
      crate::config::etc_dir().join("config.toml"),
      "[hosts.db01]\nssh = \"db01\"\n\n[hosts.db02]\nssh = \"db02\"\n",
  )
  .expect("write config");
  ```
- **Why the call site is not unit-tested.** `run_background` spawns through
  `crate::shell::spawn_detached_host`, which cannot run under a test binary.
  The pure predicate (`ghost_local_pty`) and the shell step
  (`run_ghost_remote` with `HostId::Local`, through an in-process host) are
  tested. The `wired` line pins the call site, and row 10e re-checks the
  real door live. An unobservable call-site mutant was not pinned.
- **The new shell test closes its shell** (`close_owner_shells`), because a
  leaked in-process shell host flakes the suite.
- **Known flakes:**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  (frequent),
  `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
  `daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`.
  All are pre-existing and in files this phase does not touch.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — the predicate and its tests (`src/daemon/executor/ghost_remote.rs`)

```diff
--- a/src/daemon/executor/ghost_remote.rs
+++ b/src/daemon/executor/ghost_remote.rs
@@ -10,6 +10,10 @@
 //! exit detection reports it, 08f's wording names the unanswerable prompt).
 //! An `ssh_target` that did not resolve keeps the 1.x tmux wrap, and so does
 //! `[execution] backend = "tmux"`.
+//!
+//! A **local** ghost (no `ssh_target`) runs the same way on the daemon host
+//! when the sandbox is off ([`ghost_local_pty`]), so its shell is one
+//! `/agents` can attach to.
 
 use crate::daemon::policy::GhostPolicy;
 use crate::host::HostId;
@@ -36,6 +40,21 @@ pub(super) fn ghost_remote_host(
     policy.and_then(|p| p.remote_host.clone())
 }
 
+/// Whether a ghost's command runs in a PTY shell on the **daemon host**.
+///
+/// Pure. True for a ghost with the PTY backend, the sandbox off, and no
+/// `ssh_target` (a resolved one took [`ghost_remote_host`]'s branch already;
+/// an unresolved one keeps the 1.x `ssh <target>` wrap in a tmux window).
+/// The sandboxed local command keeps its container in a tmux window.
+pub(super) fn ghost_local_pty(
+    is_ghost: bool,
+    uses_pty: bool,
+    sandbox_enabled: bool,
+    policy: Option<&GhostPolicy>,
+) -> bool {
+    is_ghost && uses_pty && !sandbox_enabled && policy.is_some_and(|p| p.ssh_target.is_none())
+}
+
 /// Run one approved ghost command in a PTY shell on `host`, synchronously.
 ///
 /// The shell is looked up (or spawned) under `owner` and **reused** for the
@@ -217,6 +236,78 @@ mod tests {
         );
     }
 
+    #[test]
+    fn a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend() {
+        let local = GhostPolicy::from_config(&crate::ipc::GhostConfig::default());
+        assert!(
+            ghost_local_pty(true, true, false, Some(&local)),
+            "a local ghost with the PTY backend and no sandbox takes the branch"
+        );
+        assert!(
+            !ghost_local_pty(true, false, false, Some(&local)),
+            "backend = \"tmux\" keeps the tmux window"
+        );
+        assert!(
+            !ghost_local_pty(true, true, true, Some(&local)),
+            "a sandboxed command keeps its container in a tmux window"
+        );
+        assert!(
+            !ghost_local_pty(false, true, false, Some(&local)),
+            "chat never routes through this module"
+        );
+        assert!(
+            !ghost_local_pty(true, true, false, None),
+            "no policy, no shell"
+        );
+        let unresolved = GhostPolicy {
+            ssh_target: Some("matt@db01.lan".to_string()),
+            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
+        };
+        assert!(
+            !ghost_local_pty(true, true, false, Some(&unresolved)),
+            "an unresolved ssh_target keeps the ssh wrap"
+        );
+    }
+
+    #[tokio::test]
+    async fn a_local_ghost_command_runs_in_a_ghost_owned_shell_on_the_daemon_host() {
+        let (_home, _dir) = test_home();
+        let reg = ShellRegistry::default();
+        let owner = Owner::Ghost("g-local".into());
+        let (text, exit_code) = run_ghost_remote(
+            &reg,
+            owner.clone(),
+            HostId::Local,
+            "echo LOCAL_OK",
+            "c1",
+            spawn_in_process_host,
+        )
+        .await;
+        let (owner_key, host) = crate::shell::with_shells(&reg, |store| {
+            (
+                store.values().next().map(|r| r.owner.key()),
+                store.values().next().map(|r| r.host.clone()),
+            )
+        });
+        crate::daemon::executor::knowledge::close_owner_shells(
+            &reg,
+            &owner,
+            std::time::Duration::from_secs(5),
+        )
+        .await;
+        assert_eq!(exit_code, 0, "the command ran cleanly: {text:?}");
+        assert!(
+            text.contains("LOCAL_OK"),
+            "the command's output must come back: {text:?}"
+        );
+        assert_eq!(
+            owner_key.as_deref(),
+            Some("ghost:g-local"),
+            "the shell is the ghost's: {owner_key:?}"
+        );
+        assert_eq!(host, Some(HostId::Local), "the shell is on the daemon host");
+    }
+
     #[tokio::test]
     async fn a_ghost_reuses_its_shell_for_later_commands_on_that_host() {
         let (_home, _dir) = test_home();
```

### Task 2 — the branch and the test repair (`src/daemon/executor/foreground.rs`)

```diff
--- a/src/daemon/executor/foreground.rs
+++ b/src/daemon/executor/foreground.rs
@@ -1151,8 +1151,8 @@ where
     // chat uses — synchronously, capped at the ghost turn budget, instead of
     // wrapping it in `ssh <target> <cmd>` in a tmux window. Everything above
     // (the two 08a gates, resolve_command, the approval, the remote-script
-    // build) already ran; everything below serves an unresolved ssh_target,
-    // a local ghost, and `backend = "tmux"`.
+    // build) already ran; everything below serves a local ghost, an
+    // unresolved ssh_target, and `backend = "tmux"`.
     let uses_pty = crate::config::Config::load()
         .unwrap_or_default()
         .execution
@@ -1215,6 +1215,42 @@ where
         return Ok(ToolCallOutcome::Result(output));
     }
 
+    // A local ghost runs the approved command in a PTY shell on the daemon
+    // host, owned by the ghost and reused for its later commands, as the
+    // remote branch above does on its host. The sandbox keeps the tmux
+    // window path below: its container is launched there.
+    if super::ghost_remote::ghost_local_pty(
+        is_ghost,
+        uses_pty,
+        crate::config::Config::load()
+            .unwrap_or_default()
+            .sandbox
+            .enabled,
+        ghost_policy,
+    ) {
+        let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
+        let (output, exit_code) = super::ghost_remote::run_ghost_remote(
+            crate::shell::registry(),
+            owner,
+            crate::host::HostId::Local,
+            cmd,
+            id,
+            |shell_id| crate::shell::spawn_detached_host(shell_id, &[], 24, 80),
+        )
+        .await;
+        crate::daemon::stats::finish_command(cmd_id, exit_code);
+        send_response_split(
+            tx,
+            Response::ToolResult {
+                tool_call_id: id.to_string(),
+                output: output.clone(),
+            },
+        )
+        .await?;
+        log_command(session_id, "background", "", cmd, "approved", &output);
+        return Ok(ToolCallOutcome::Result(output));
+    }
+
     // Ghost shells: wrap the approved command in `ssh <target> <cmd>` when configured.
     let ssh_wrapped_cmd;
     let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
@@ -1469,6 +1505,13 @@ fn run_background_resolves_ghost_script_paths() {
         std::env::remove_var("TMUX_PANE");
     }
     crate::config::Config::ensure_dirs().expect("ensure dirs");
+    // The tmux window path is the one this test pins: with the default PTY
+    // backend a local ghost runs in a PTY shell instead (`ghost_local_pty`).
+    std::fs::write(
+        crate::config::etc_dir().join("config.toml"),
+        "[execution]\nbackend = \"tmux\"\n",
+    )
+    .expect("write config");
 
     let marker = tmp.path().join("resolved-marker.txt");
     let script_name = "de-ghost-resolve-check.sh";
```

### Task 3 — the ghost prompt (`assets/prompts/ghost-shell.txt`)

```diff
--- a/assets/prompts/ghost-shell.txt
+++ b/assets/prompts/ghost-shell.txt
@@ -1,7 +1,7 @@
 ## Ghost Shell Operating Rules
 
 You are an autonomous Ghost Shell — an unattended AI agent. No human user is present.
-All terminal commands MUST pass `wait: false` (they run in de-gs-bg-* or de-gs-sj-* windows).
+All terminal commands MUST pass `wait: false`.
 
 **Rules:**
 1. **Remediate autonomously.** Investigate and fix the alert using the provided runbook
@@ -22,12 +22,13 @@ All terminal commands MUST pass `wait: false` (they run in de-gs-bg-* or de-gs-s
 6. **One action at a time.** Issue exactly one `run_terminal_command` per turn.
    Wait for each result before deciding the next step. Do not repeat a command
    you already ran — check the previous output instead.
-7. **Wait for background commands.** A background command returns as soon as it
-   is dispatched, not when it finishes. Its outcome arrives as a
-   `[Background Task Completed]` context message — wait for that message before
-   issuing further tool calls, and never assume a background command succeeded
-   without observing its output. Do not call a watch tool: your commands run in
-   a tmux window, not a PTY shell, so there is no shell id to watch.
+7. **Read each command's outcome.** A command that runs in a shell you own
+   returns its output and exit code as its result. One that runs in a de-gs-*
+   window returns as soon as it is dispatched; its outcome arrives as a
+   `[Background Task Completed]` context message — wait for that message
+   before issuing further tool calls. Never assume a command succeeded without
+   observing its output, and do not call a watch tool: the outcome comes back
+   to you either way.
 8. **Completion.** Your final response must be a concise summary: what alert fired,
    what you did, the outcome, and any follow-up the user should perform.
    This summary appears in the catch-up brief when the user re-attaches.
```

### Task 4 — tests

All are in the diffs above:
- `a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend`
  and `a_local_ghost_command_runs_in_a_ghost_owned_shell_on_the_daemon_host`
  (`ghost_remote.rs`), both new;
- `run_background_resolves_ghost_script_paths` (`foreground.rs`), which keeps
  its name and now writes `backend = "tmux"`.

**Test count:** two new, so **2223 passed / 14 ignored**.

### Task 5 — apply and record Mutations A–B (before the capture)

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
**Its first fenced block is the contents of `/tmp/e2e-m23-10c.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2223 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2221/0/14). Both counts are exact. A failure
      of a known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 15,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass: ``cargo test --lib -- a_local_ghost_`` →
      **2 passed** (current: 0). That exact filter printed 2 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it →
      **`1/1/1/0`** (current: `0/0/0/1`):
      ``grep -c 'ghost_local_pty(' src/daemon/executor/foreground.rs`` → 1;
      ``grep -c 'crate::host::HostId::Local,' src/daemon/executor/foreground.rs`` → 1;
      ``grep -c 'returns its output and exit code as its result' assets/prompts/ghost-shell.txt`` → 1;
      ``grep -c 'no shell id to watch' assets/prompts/ghost-shell.txt`` → 0
      (the false sentence is gone).
- [ ] The mutations entry exists and records both:
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

- **Mutation A** (`ghost_remote.rs`, the sandbox keeps its window): in
  `ghost_local_pty`, change
  `is_ghost && uses_pty && !sandbox_enabled && policy` to
  `is_ghost && uses_pty && sandbox_enabled && policy`. Run
  `a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend`.
  Expected **red**.
- **Mutation B** (`ghost_remote.rs`, an `ssh_target` keeps its wrap): in
  `ghost_local_pty`, change `policy.is_some_and(|p| p.ssh_target.is_none())`
  to `policy.is_some()`. Run the same test. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

- `ghost_local_pty`: true only for a ghost with the PTY backend, the sandbox
  off, a policy and no `ssh_target`. Each of the five other combinations is
  false.
- A local ghost command through `run_ghost_remote` with `HostId::Local` and
  an in-process host: exit 0, the output comes back, and the one shell
  record is owned by `ghost:g-local` on `HostId::Local`. The shell is closed
  before the assertions.
- `run_background_resolves_ghost_script_paths` keeps pinning script
  resolution on the tmux window path, now selected through config.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- a_local_ghost_ 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'doc_truth=%s\n' "$(cargo test --test doc_truth 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s/%s\n' "$(grep -c 'ghost_local_pty(' src/daemon/executor/foreground.rs)" "$(grep -c 'crate::host::HostId::Local,' src/daemon/executor/foreground.rs)" "$(grep -c 'returns its output and exit code as its result' assets/prompts/ghost-shell.txt)" "$(grep -c 'no shell id to watch' assets/prompts/ghost-shell.txt)"
} > /tmp/e2e-m23-10c.txt
cat /tmp/e2e-m23-10c.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2223p-0f-14i`;
- `phase_tests=2p-0f`;
- `doc_truth=15 passed; 0 failed`;
- `wired=1/1/1/0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Three
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`,
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
(frequent) and
`daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`.
None is in a file this phase touches. If one fails, name it and re-run the
whole block. After three re-runs, stop, paste the last capture as it is, and
name every failing test. Any other failing test is a finding: report it by
name. **`run_background_resolves_ghost_script_paths` failing means Task 2's
test repair is missing.** It is not a flake.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-10c.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-10c-local-ghost-pty-shells.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-10c.txt
diff /tmp/pasted-m23-10c.txt /tmp/e2e-m23-10c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-10c.txt /tmp/e2e-m23-10c.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/executor/ghost_remote.rs`,
  `src/daemon/executor/foreground.rs`, `assets/prompts/ghost-shell.txt`: the
  diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **The sandboxed local ghost.** It keeps its container in a tmux window.
  Row 10d runs the docker command inside the ghost's PTY shell (PE decision
  2026-10-04).
- **A ghost's foreground call** (`wait: true`). The ghost prompt requires
  `wait: false`, and a foreground call goes to `run_foreground` unchanged.
- **The seeded `ghost-shell-guide` knowledge memory.** It describes
  `de-gs-*` windows, which are still true for the sandbox and
  `backend = "tmux"`. M26 rewrites the tmux knowledge memories.
- **README.md.** No user-visible command changes. The live close (10e) and
  the milestone close update the README.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-05 00:42 (started)

**Executor:** Claude (this run).

Applying Tasks 1–6: `ghost_local_pty` in `src/daemon/executor/ghost_remote.rs` (plus its two tests), the local-ghost PTY branch and the `run_background_resolves_ghost_script_paths` config repair in `src/daemon/executor/foreground.rs`, rule 7's rewrite in `assets/prompts/ghost-shell.txt`; then Mutations A–B and the end-to-end capture. Status flipped to in-progress here and in the README phase table.

### Update — 2026-10-05 00:42 (mutations)

- Mutation A (ghost_remote.rs, the sandbox keeps its window): `cargo test --lib -- a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend` → red. Restored; `git diff --stat` matches the pre-mutation state (5 files, 152 insertions, 11 deletions).
- Mutation B (ghost_remote.rs, an `ssh_target` keeps its wrap): `cargo test --lib -- a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend` → red. Restored; `git diff --stat` matches the pre-mutation state (5 files, 152 insertions, 11 deletions).

### Update — 2026-10-05 00:42 (end-to-end verification)

```sh
lib1=FAILED failed:shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2223p-0f-14i
phase_tests=2p-0f
doc_truth=15 passed; 0 failed
wired=1/1/1/0
```

The known flake `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
fired twice in the first block (runs 1 and 3, in a file this phase does not
touch) and once in run 2 of the second block (the `lib_counts` run, whose
output line names no test because the `sed` expression matches `ok\.` only);
it passes 5/5 in isolation. Runs 2 and 3 of the first block are clean
`ok failed:none`, and the second block's runs 1 and 3 are clean too, so the
count 2223 passed / 0 failed / 14 ignored is confirmed three times across the
two blocks. `phase_tests`, `doc_truth` and `wired` are as expected.

PASTE MATCH
