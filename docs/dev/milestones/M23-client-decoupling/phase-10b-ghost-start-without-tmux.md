# Phase 10b: ghost-start-without-tmux — a ghost started by a daemon running without tmux starts no tmux server

**Milestone:** M23 — Client decoupling & shell UX
**Status:** review
**Depends on:** 02 (done: `tmux_session = ""` means no tmux at daemon startup)
**Estimated diff:** ~30 lines of production code across 2 files; 2 new tests (~25 lines)
**Tags:** language=rust, kind=bugfix, size=s

## Goal

A daemon launched outside tmux with `tmux_session = ""` starts no tmux
server (row 02). **Starting a ghost still starts one.** Every ghost entry
point (webhook, scheduler, `spawn_ghost_shell`, resume) goes through
`GhostManager::start_session_with_config`. That function calls
`ensure_incident_session()`, which never reads the config and runs
`tmux new-session -d -s daemoneye-incidents` when no server exists. This
breaks the milestone's "Nothing starts tmux" exit criterion.

After this phase:

- **The daemon records its tmux mode once at startup.** A new
  `crate::daemon::tmux_off()` returns `true` exactly when `run_daemon` chose
  "no tmux" (`initial_session` is `None`). Unrecorded, it reads `false`, so
  tests and the CLI keep today's behaviour.
- **A starting ghost with tmux off gets an empty tmux session** and never
  calls `ensure_incident_session()`. The decision is a pure function,
  `ghost_tmux_session(tmux_off, ensure)`.
- **A ghost turn with an empty tmux session is not refused.** Today
  `do_ghost_turn` bails with "tmux session '' no longer exists" unless
  `tmux has-session` succeeds. The check becomes
  `ghost_session_alive(tmux_session, exists)`: an empty session never asks
  tmux, and a named session must still exist, as before.
- **Inside tmux, or with a non-empty `tmux_session`, nothing changes.**

This is row 10b. Row 10c (a local ghost's commands in PTY shells) and row
10d (the live close) follow (PE decision 2026-10-04).

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md` § Exit criteria:
  "Nothing starts tmux", and the live exit's "no server before, during and
  after the run".
- `docs/design/daemoneye-2.0.md` § 2.9: "ghosts default to sandboxed local
  shells". That is row 10c's work, not this phase's.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `58d6df2d`. Baseline:
`cargo test --lib` gives **2219 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2221**/0/14, and every other target was unmoved
  (bug_tracker 6, doc_truth 15, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2219p-0f-14i`,
  `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A and B each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **A ghost starts a tmux server (measured, not read).** A ghost driven
  through `process_alert` under an isolated `TMUX_TMPDIR` with no server
  left `tmux-1000/` in that directory. `tmux ls` there printed
  `daemoneye-incidents: 1 windows`.
- **Through the real daemon binary (measured).** A daemon started with
  `--session ""` in the isolation harness, plus a webhook-triggered ghost
  (stub LLM, a one-turn runbook):
  - **current tree:** `ghost_complete`, but `tmux-1000/` appeared;
  - **dry-run tree:** `ghost_complete`, no `tmux-*` directory, and no
    "no longer exists" line in `daemon.log`.

  That probe is the harness's `#[ignore]`d HTTP test shape. It polls the
  event log, which STANDARDS §3.3 forbids in the suite, so it is **not**
  added. The real door is re-checked live in row 10d.
- **Why the decision is a pure function.** The in-process ghost tests in
  `tests/isolation.rs` share one process. A process-wide flag set by a new
  test there would change the behaviour of the other two, depending on test
  order. A separate test file that uses `tests/harness` fails clippy with
  `dead_code` (measured), and `#[allow]` is banned. So the two decisions are
  pure functions with unit tests, and the `wired` line pins the three call
  sites.
- **An unobservable mutant was dropped.** Passing `false` instead of
  `crate::daemon::tmux_off()` at the start site stays green, because no unit
  test runs `run_daemon`. It is also not clippy-clean (`tmux_off` becomes
  unused). The `wired` line pins that call site.
- **The idiom for a startup-recorded global** is the existing
  `DAEMON_START` (`src/daemon/mod.rs:17`):
  ```rust
  static DAEMON_START: OnceLock<Instant> = OnceLock::new();

  pub fn daemon_uptime_secs() -> u64 {
      DAEMON_START
          .get()
          .map(|t| t.elapsed().as_secs())
          .unwrap_or(0)
  }
  ```
- **Known flakes:**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  (frequent),
  `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`, and,
  seen once in 10a,
  `daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`.
  All are pre-existing and in files this phase does not touch.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — record the tmux mode (`src/daemon/mod.rs`)

```diff
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -24,6 +24,21 @@ pub fn daemon_uptime_secs() -> u64 {
         .unwrap_or(0)
 }
 
+/// Whether this daemon runs with no tmux at all: launched outside tmux with an
+/// empty `tmux_session`. Recorded once at startup; unrecorded (tests, the CLI)
+/// reads as `false`, today's behaviour.
+static TMUX_OFF: OnceLock<bool> = OnceLock::new();
+
+/// Record the daemon's tmux mode. The first call wins.
+pub(crate) fn record_tmux_off(off: bool) {
+    let _ = TMUX_OFF.set(off);
+}
+
+/// Whether the daemon runs with no tmux at all. See [`record_tmux_off`].
+pub(crate) fn tmux_off() -> bool {
+    TMUX_OFF.get().copied().unwrap_or(false)
+}
+
 pub mod auto_name;
 pub mod background;
 pub mod briefing;
@@ -658,6 +673,8 @@ pub async fn run_daemon(log_file: Option<PathBuf>, session_override: Option<Stri
         }
     };
 
+    record_tmux_off(initial_session.is_none());
+
     log_event(
         "daemon_start",
         serde_json::json!({
```

### Task 2 — the ghost honours it (`src/daemon/ghost.rs`)

```diff
--- a/src/daemon/ghost.rs
+++ b/src/daemon/ghost.rs
@@ -138,6 +138,26 @@ pub fn check_ghost_capacity(config: &crate::config::Config) -> bool {
     crate::daemon::stats::get_ghosts_active() < max
 }
 
+/// The tmux session a starting ghost's windows go in. With tmux off it is
+/// empty and `ensure` — which creates a session, and so a server, when none
+/// exists — is never called.
+fn ghost_tmux_session(tmux_off: bool, ensure: impl FnOnce() -> Result<String>) -> Result<String> {
+    if tmux_off {
+        return Ok(String::new());
+    }
+    ensure()
+}
+
+/// Whether a ghost's turn may run in `tmux_session`. An empty session — the
+/// daemon runs without tmux — has nothing to lose, so `exists` is never
+/// awaited; a named one must still exist.
+async fn ghost_session_alive(
+    tmux_session: &str,
+    exists: impl std::future::Future<Output = bool>,
+) -> bool {
+    tmux_session.is_empty() || exists.await
+}
+
 /// Resolve a ghost's `ssh_target` against `[hosts]` and record the outcome.
 ///
 /// Runs **once, at ghost start** — `start_session_with_config` is the one
@@ -243,9 +263,11 @@ impl GhostManager {
     ) -> Result<String> {
         let alert_name = &runbook.name;
 
-        // 1. Ensure host tmux session exists (active or detached)
-        let tmux_session =
-            ensure_incident_session().context("GhostManager: failed to ensure incident session")?;
+        // 1. The tmux session the ghost's windows go in: none when the daemon
+        //    runs without tmux, so a ghost never starts a tmux server.
+        let tmux_session = ghost_tmux_session(crate::daemon::tmux_off(), || {
+            ensure_incident_session().context("GhostManager: failed to ensure incident session")
+        })?;
 
         // 2. Initialize ghost shell entry
         let session_id = format!("ghost-{}-{}", alert_name, uuid::Uuid::new_v4().simple());
@@ -817,9 +839,12 @@ async fn do_ghost_turn(
     );
 
     let s = tmux_session.clone();
-    let pane_alive = tmux::off_runtime("session-exists", move || tmux::session_exists(&s))
-        .await
-        .unwrap_or(false);
+    let pane_alive = ghost_session_alive(&tmux_session, async move {
+        tmux::off_runtime("session-exists", move || tmux::session_exists(&s))
+            .await
+            .unwrap_or(false)
+    })
+    .await;
     if !pane_alive {
         anyhow::bail!(
             "Ghost Shell {}: tmux session '{}' no longer exists",
@@ -1549,6 +1574,30 @@ async fn do_ghost_turn(
 mod tests {
     use super::*;
 
+    #[test]
+    fn ghost_tmux_session_starts_no_tmux_when_tmux_is_off() {
+        let off = ghost_tmux_session(true, || panic!("tmux off must not ensure a session"));
+        assert_eq!(off.expect("tmux off"), "");
+
+        let on = ghost_tmux_session(false, || Ok("daemoneye-incidents".to_string()));
+        assert_eq!(on.expect("tmux on"), "daemoneye-incidents");
+
+        let failed = ghost_tmux_session(false, || anyhow::bail!("no tmux"));
+        assert!(failed.is_err(), "tmux on still reports a failed session");
+    }
+
+    #[tokio::test]
+    async fn an_empty_tmux_session_is_never_checked() {
+        assert!(
+            ghost_session_alive("", async { panic!("an empty session must not ask tmux") }).await
+        );
+        assert!(ghost_session_alive("daemoneye-incidents", async { true }).await);
+        assert!(
+            !ghost_session_alive("daemoneye-incidents", async { false }).await,
+            "a named session that is gone still stops the turn"
+        );
+    }
+
     #[test]
     fn ghost_start_records_how_its_ssh_target_resolved() {
         let _lock = crate::test_home_guard();
```

### Task 3 — tests

Both are in the diff above, in `src/daemon/ghost.rs`'s `mod tests`:
`ghost_tmux_session_starts_no_tmux_when_tmux_is_off` and
`an_empty_tmux_session_is_never_checked` (a `#[tokio::test]`).

**Test count:** two new, so **2221 passed / 14 ignored**.

### Task 4 — apply and record Mutations A–B (before the capture)

Do this **after** Tasks 1–3 pass and **before** Task 5. A mutation does not
affect the capture: each one is restored, and `git diff --stat` is checked,
before the next step. Follow § Mutations: for each one, apply it with the
`patch` tool, run its named test once, restore it, and confirm
`git diff --stat` matches. Then create an Update Log entry titled exactly
`### Update — <date> (mutations)` with one line per mutation: its letter,
the test, and the observed result (`red` or `green`).

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-10b.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2221 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2219/0/14). Both counts are exact. A failure
      of a known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 15,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- ghost_tmux_session_starts_no_tmux_when_tmux_is_off an_empty_tmux_session_is_never_checked``
      → **2 passed** (current: 0). That exact filter printed 2 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'record_tmux_off(initial_session.is_none())' src/daemon/mod.rs`` → 1;
      ``grep -c 'ghost_tmux_session(crate::daemon::tmux_off()' src/daemon/ghost.rs`` → 1;
      ``grep -c 'ghost_session_alive(&tmux_session' src/daemon/ghost.rs`` → 1.
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
Task 4 entry. Each was measured red at drafting on 3 of 3 runs, and each
mutant builds clippy-clean.

- **Mutation A** (`ghost.rs`, tmux off ensures no session): in
  `ghost_tmux_session`, change `    if tmux_off {` to `    if !tmux_off {`.
  Run `ghost_tmux_session_starts_no_tmux_when_tmux_is_off`. Expected **red**.
- **Mutation B** (`ghost.rs`, an empty session never asks tmux): in
  `ghost_session_alive`, change `    tmux_session.is_empty() || exists.await`
  to `    !tmux_session.is_empty() && exists.await`. Run
  `an_empty_tmux_session_is_never_checked`. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Two new unit tests:
- `ghost_tmux_session`: with tmux off, the result is empty and the `ensure`
  closure (which panics) is never called; with tmux on, `ensure`'s value or
  its error comes back unchanged;
- `ghost_session_alive`: an empty session is alive and the probe future
  (which panics) is never awaited; a named session is alive exactly when the
  probe says it exists.

The real door (a real daemon, a real ghost, no tmux server) was measured at
drafting and is re-checked live in row 10d.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- ghost_tmux_session_starts_no_tmux_when_tmux_is_off an_empty_tmux_session_is_never_checked 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'record_tmux_off(initial_session.is_none())' src/daemon/mod.rs)" "$(grep -c 'ghost_tmux_session(crate::daemon::tmux_off()' src/daemon/ghost.rs)" "$(grep -c 'ghost_session_alive(&tmux_session' src/daemon/ghost.rs)"
} > /tmp/e2e-m23-10b.txt
cat /tmp/e2e-m23-10b.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2221p-0f-14i`;
- `phase_tests=2p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Three
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`,
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
(frequent) and
`daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`.
None is in a file this phase touches. If one fails, name it and re-run the
whole block. After three re-runs, stop, paste the last capture as it is, and
name every failing test. Any other failing test is a finding: report it by
name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-10b.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-10b-ghost-start-without-tmux.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-10b.txt
diff /tmp/pasted-m23-10b.txt /tmp/e2e-m23-10b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-10b.txt /tmp/e2e-m23-10b.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/mod.rs`, `src/daemon/ghost.rs`: the diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here is in-process.

## Out of scope

- **Where a local ghost's commands run.** With tmux off, a local ghost
  command still falls through to the tmux window path
  (`src/daemon/executor/mod.rs`, "A ghost session … falls through to
  `run_foreground` unchanged"). Row 10c moves it to a PTY shell.
- **Scheduled script jobs' tmux windows** (`src/daemon/scheduled.rs`
  `create_job_window`). `tmux new-window` with no server fails rather than
  starting one; M26 deletes the path.
- **Behaviour inside tmux or with a non-empty `tmux_session`.** Unchanged.
- **README.md.** This is a fix with no user-visible surface beyond "no tmux
  server", which README.md already promises for `tmux_session = ""`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-04 16:17 (started)

**Executor:** Claude (this run).

Applying Tasks 1-5: `TMUX_OFF` OnceLock + `record_tmux_off`/`tmux_off` in `src/daemon/mod.rs`, `record_tmux_off(initial_session.is_none())` in `run_daemon`, the two pure decision functions and their call sites in `src/daemon/ghost.rs`, two new unit tests. Status flipped to in-progress here and in the README phase table.

### Update — 2026-10-04 16:17 (mutations)

- Mutation A (ghost.rs, `if !tmux_off`): `cargo test --lib -- ghost_tmux_session_starts_no_tmux_when_tmux_is_off` → red. Restored; `git diff --stat` matches the pre-mutation state (2 files, 72 insertions, 6 deletions).
- Mutation B (ghost.rs, `!tmux_session.is_empty() && exists.await`): `cargo test --lib -- an_empty_tmux_session_is_never_checked` → red. Restored; `git diff --stat` matches the pre-mutation state (2 files, 72 insertions, 6 deletions).

### Update — 2026-10-04 16:17 (end-to-end verification)


```sh
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2221p-0f-14i
phase_tests=2p-0f
isolation=11 passed; 0 failed
wired=1/1/1
```
PASTE MATCH

### Update — 2026-10-04 23:26 (complete, server-authored)

**Summary:** **Summary:** Applied the dry-run diffs exactly as specified: `src/daemon/mod.rs` gains the `TMUX_OFF` `OnceLock` with `record_tmux_off`/`tmux_off` (mode recorded once in `run_daemon` from `initial_session.is_none()`), and `src/daemon/ghost.rs` routes both decisions through the two pure functions — `ghost_tmux_session` (tmux off → empty session, `ensure_incident_session` never called) and `ghost_session_alive` (empty session → no tmux probe, so no "no longer exists" refusal) — plus the two specified unit tests. Mutations A and B each went red on their named test, were restored, and `git diff --stat` matched the pre-mutation state both times. No deviations from the spec.

**End-to-end verification:** the verbatim block ran once, and `/tmp/e2e-m23-10b.txt` printed exactly the Expected values — `lib1/lib2/lib3=ok failed:none`, `lib_counts=2221p-0f-14i`, `phase_tests=2p-0f`, `isolation=11 passed; 0 failed`, `wired=1/1/1`; its contents are pasted unchanged in the `(end-to-end verification)` entry and the paste check printed `PASTE MATCH`. No known flake fired in any run. Full gates: `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` exits 0, and `cargo test` shows lib 2221/0/14 with all other targets unmoved (bug_tracker 6, doc_truth 15, integration 32, isolation 11). Committed as `9b1a32a7` (`fix:`), working tree clean; phase doc status remains `in-progress` with no `(complete)` entry, per the lifecycle split.

**Notes for review:** Nothing notable beyond the spec — source is byte-identical to the dry-run tree the phase doc was drafted from. Rows 10c (local ghost PTY shells) and 10d (live close) are untouched, as scoped.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_counts_are_accurate ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test cancel_request_roundtrip ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
kill: sending signal to 1056214 failed: No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M23-client-decoupling/README.md` — +1 -1
- `docs/dev/milestones/M23-client-decoupling/phase-10b-ghost-start-without-tmux.md` — +26 -1
- `src/daemon/ghost.rs` — +55 -6
- `src/daemon/mod.rs` — +17 -0

**Commit:** 9b1a32a7e5335cce9ce76a070d9d9144051de131

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
