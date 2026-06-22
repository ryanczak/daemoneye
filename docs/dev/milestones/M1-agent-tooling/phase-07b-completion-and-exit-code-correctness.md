# Phase 07b: Completion & Exit-Code Correctness

**Milestone:** M1 — Agent Tooling Improvements
**Status:** done
**Depends on:** phase-07a (shares the foreground execution path; 07a must be `done`)
**Estimated diff:** ~110 lines (incl. tests)
**Tags:** language=rust, kind=bugfix, size=m

> **Scope note (architect, 2026-06-22).** The original phase-07
> ("execution-robustness-and-tmux") bundled six findings plus open-ended tmux
> leverage. It was split into **07a** (done — the four `medium` mechanical/safety
> fixes) and **07b** (this doc — the two `high` completion/exit-code items). The
> open-ended **tmux-verb leverage** bullet (`wait-for`, `set-buffer`/`paste-buffer`,
> `copy-mode -X`, `if-shell`) is **deferred out of this phase** — see Out of scope
> and README § Notes. This phase touches **only the local foreground completion
> wait and the exit-code reporting**; it does not touch the sudo, remote, or
> interactive branches.

## Goal

Make local foreground command execution report completion and exit status
correctly:

1. **Reliable completion detection.** The `saw_child`/PID-return loop can
   false-early-exit on very fast commands and uses a too-short start window. Use
   the `DE_EXIT_<pane>` latch written by the shell hook as the *exact, primary*
   completion signal (cleared before send so its reappearance means *this*
   command finished), keeping PID-return as the fallback when the hook is not
   installed.
2. **Surface non-zero exit codes to the model.** Today the captured exit code is
   fabricated as `0` when the hook didn't write it (`read_pane_exit_status(...).
   unwrap_or(0)`) and is fed only to `finish_command` (stats) — it never reaches
   the AI, so the model cannot distinguish a failed command from a clean one.
   Annotate the `ToolResult` with the real non-zero exit code; never fabricate
   success.

## Architecture references

Read before starting:

- `docs/architecture.md#21-interactive-requestresponse` — the approval →
  send-keys → completion → `ToolResult` flow; this phase hardens the completion
  and exit-code reporting steps.
- `docs/architecture.md#24-remote-host-execution-model` — why exit-code surfacing
  is **local-pane only**: remote panes run the local shell hook for the *ssh
  wrapper*, not the remote command, so no meaningful per-command code exists there.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom (note §2.1 error handling — no new
   `.unwrap()`/`.expect()` in production paths; §2.2 "no premature abstraction";
   §3 test rules: hermetic, deterministic, no real network/home writes).
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. **Re-verify the cited line numbers** in `src/daemon/executor/foreground.rs`
   and `src/tmux/pane.rs` before editing — they were captured at draft time and
   the tree moves. Match on the quoted code, not the line number.
5. Confirm the repo is on a clean branch with no uncommitted changes (07a landed).

## Current state

### The `DE_EXIT_<pane>` latch (existing, load-bearing for this phase)

The shell hook the user installs via `daemoneye setup` writes the last command's
exit code to the tmux session environment under `DE_EXIT_<num>` (`src/cli/commands/
setup.rs:264-277`):

```sh
# bash (~/.bashrc):
_de_exit_trap() { tmux set-environment "DE_EXIT_${TMUX_PANE#%}" "$?" 2>/dev/null; }
PROMPT_COMMAND="_de_exit_trap${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
```

It fires from `PROMPT_COMMAND`/`precmd` — i.e. **right before each prompt is
redrawn, after the previous command finished**. The daemon reads it back via
`read_pane_exit_status` (`src/tmux/pane.rs:366-386`):

```rust
pub fn read_pane_exit_status(pane_id: &str) -> Option<i32> {
    let key = format!("DE_EXIT_{}", pane_id.trim_start_matches('%'));
    let output = Command::new("tmux")
        .args(["show-environment", &key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .split_once('=')
        .and_then(|(_, val)| val.parse::<i32>().ok())
}
```

It returns `None` when the key is absent (hook not installed) — **never assume
`0` means success; `None` means "unknown", not "clean".** Note the addressing:
`show-environment` with **no `-t`**. The new clear helper (Task 1) must mirror
this addressing exactly — do **not** add `-t`.

### Task subject — the local completion wait

`run_foreground` (`src/daemon/executor/foreground.rs`). The command is sent once
at line ~358 inside the `Ok(())` arm of `match tmux::send_keys(target_str,
send_cmd)`. `idle_pid` (the pane's shell PID before the command) is captured
earlier at line ~293:

```rust
    let idle_pid = tmux::pane_pid(target_str).unwrap_or(0);
```

The **local** completion branch is the final `else` of the
interactive/remote/local three-way (lines ~696-737):

```rust
                let deadline = tokio::time::Instant::now() + LOCAL_CMD_TIMEOUT;

                // Wait until the child process is visible via a PID change.
                // idle_pid == 0 means the query failed; treat as child-started
                // immediately so we fall through to the hook-based completion wait.
                let saw_child = if idle_pid == 0 {
                    true
                } else {
                    tokio::time::timeout(LOCAL_CHILD_START_WINDOW, async {
                        loop {
                            tokio::time::sleep(LOCAL_CHILD_POLL).await;
                            let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                            if cur_pid != idle_pid {
                                break;
                            }
                        }
                    })
                    .await
                    .is_ok()
                };

                if saw_child {
                    loop {
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                        tokio::select! {
                            result = fg_rx.recv() => {
                                if let Ok(notified_pane) = result
                                    && notified_pane == target_str {
                                        let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                                        // idle_pid == 0: rely solely on hook signals
                                        if idle_pid != 0 && cur_pid == idle_pid { break; }
                                    }
                            }
                            _ = tokio::time::sleep(LOCAL_SLOW_POLL) => {
                                let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                                if idle_pid != 0 && cur_pid == idle_pid { break; }
                            }
                        }
                    }
                }
```

The two failure modes (both **high**):
- A command that finishes faster than the start window's first PID observation
  leaves `saw_child = false` → the whole completion wait is skipped → capture
  happens immediately, sometimes before output settles, and **with no exit code**.
- `LOCAL_CHILD_START_WINDOW = 300ms` (line ~35) is too short for a slow-to-fork
  command, so its child start is missed and the wait is skipped.

### Task subject — the exit-code report

At the capture/return point (lines ~744-793, same `Ok(())` arm), `output` is
built from `capture_pane`, then:

```rust
            let exit_code = tmux::read_pane_exit_status(target_str).unwrap_or(0);
            crate::daemon::stats::finish_command(cmd_id, exit_code);
            send_response_split(tx, Response::ToolResult(output.clone())).await?;
```

`unwrap_or(0)` fabricates `0`, and `exit_code` only reaches `finish_command`
(stats) — it is **never** added to `output`, so the model never sees it. This
line runs for **all three** branches (interactive/remote/local), so any exit
surfacing must be gated to the local pane.

`stats::finish_command(id: usize, exit_code: i32)` (`src/daemon/stats.rs:89`) is
unchanged by this phase — it stays best-effort; only what reaches the **model**
changes.

## Spec

Numbered tasks in execution order. Each names the exact file and change. **Build
after Task 1** (it adds a `tmux` function used by `foreground.rs`).

### 1. Add `tmux::clear_pane_exit_status` — `src/tmux/pane.rs`

Add a helper next to `read_pane_exit_status` (re-exported via `pub use pane::*;`
in `src/tmux/mod.rs`, so it becomes `crate::tmux::clear_pane_exit_status`). It
unsets the latch so a stale value from the previous command can't be misread as
this command's result:

```rust
/// Clear the `DE_EXIT_<pane>` latch before sending a foreground command, so its
/// later reappearance — written by the shell hook when the prompt redraws — marks
/// *this* command's completion and carries its real exit code, not a stale one.
/// Best-effort: a missing hook simply means the key never reappears, and the
/// caller falls back to PID-return completion.
pub fn clear_pane_exit_status(pane_id: &str) {
    let key = format!("DE_EXIT_{}", pane_id.trim_start_matches('%'));
    let _ = Command::new("tmux")
        .args(["set-environment", "-u", &key])
        .output();
}
```

Mirror `read_pane_exit_status`'s addressing: **no `-t`**, same `DE_EXIT_<num>` key
derivation (`trim_start_matches('%')`). Returns `()` — there is nothing for the
caller to act on (STANDARDS §2.2: no error handling for a best-effort clear).

### 2. Clear the latch before sending — `src/daemon/executor/foreground.rs`

Immediately **before** `let result = match tmux::send_keys(target_str, send_cmd)`
(line ~358), clear the latch:

```rust
    // Clear the DE_EXIT latch so its reappearance signals THIS command's
    // completion (and carries its real exit code) rather than a stale value from
    // the previous command. No-op for remote/interactive panes (they don't
    // consult it).
    tmux::clear_pane_exit_status(target_str);

    let result = match tmux::send_keys(target_str, send_cmd) {
```

### 3. Hoist a shared `exit_status` and rewrite the local completion wait — `src/daemon/executor/foreground.rs`

**3a.** Inside the `Ok(())` arm, alongside the existing `let mut
switched_to_working = false;` / `let mut is_interactive = false;` (line ~361),
add:

```rust
            let mut exit_status: Option<i32> = None;
```

This is visible both in the local branch (which sets it) and at the
capture/return point (Task 4, which reads it). It stays `None` on the
interactive/remote branches.

**3b.** Replace the local `else`-branch completion wait shown in Current state
(the `let saw_child = …` block **through** the `if saw_child { … }` loop, lines
~696-737) with the two-phase wait below. Leave everything above it in the same
`else` block (the N9 `monitor-silence` / `alert-silence` install at lines
~674-694) untouched:

```rust
                let deadline = tokio::time::Instant::now() + LOCAL_CMD_TIMEOUT;

                // Phase 1 — within the start window, detect either the child
                // appearing (PID diverges from idle) or a fast command having
                // already finished (the DE_EXIT latch reappeared). The latch is
                // exact regardless of how fast the command was; PID-divergence is
                // the fallback when the shell hook is not installed.
                let mut saw_child = idle_pid == 0;
                if idle_pid != 0 {
                    let start_deadline =
                        tokio::time::Instant::now() + LOCAL_CHILD_START_WINDOW;
                    while tokio::time::Instant::now() < start_deadline
                        && exit_status.is_none()
                        && !saw_child
                    {
                        if let Some(code) = tmux::read_pane_exit_status(target_str) {
                            exit_status = Some(code);
                            break;
                        }
                        tokio::time::sleep(LOCAL_CHILD_POLL).await;
                        if tmux::pane_pid(target_str).unwrap_or(0) != idle_pid {
                            saw_child = true;
                        }
                    }
                }

                // Phase 2 — only when a child was seen running (a non-trivial
                // command). A command that finished inside the start window is
                // already done: either its latch was read above, or — hook absent —
                // it is captured as-is below (matching the prior fast-path
                // behavior, no false hang). Completion = the DE_EXIT latch (exact,
                // primary) or the child PID returning to idle (fallback). Hook
                // signals (fg_rx) drive promptness.
                if saw_child {
                    while exit_status.is_none() {
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                        if let Some(code) = tmux::read_pane_exit_status(target_str) {
                            exit_status = Some(code);
                            break;
                        }
                        tokio::select! {
                            result = fg_rx.recv() => {
                                if let Ok(notified_pane) = result
                                    && notified_pane == target_str {
                                        let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                                        if idle_pid != 0 && cur_pid == idle_pid { break; }
                                    }
                            }
                            _ = tokio::time::sleep(LOCAL_SLOW_POLL) => {
                                let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                                if idle_pid != 0 && cur_pid == idle_pid { break; }
                            }
                        }
                    }
                }
```

Behavioral pins (must hold; do not "optimize" them away):
- **Hook installed, any speed:** `exit_status` ends `Some(code)` with the real
  code — phase 1 for fast commands, phase 2 for slower ones.
- **Hook absent, child seen:** completion still detected by PID-return;
  `exit_status` stays `None`.
- **Hook absent, command finished within the start window (no PID divergence
  observed):** phase 2 is skipped (`saw_child == false`); capture proceeds
  immediately; `exit_status` stays `None`. This preserves today's fast-path — do
  **not** make this case hang to the deadline.
- **`idle_pid == 0` (PID query failed):** `saw_child` starts `true`, phase 2 runs,
  completion relies on the latch (or the deadline); the PID-return branch never
  fires (cur_pid never equals 0). No regression vs. today.

**3c.** Widen the start window (line ~35) from 300ms — too short to observe a
slow-to-fork child — to:

```rust
const LOCAL_CHILD_START_WINDOW: Duration = Duration::from_millis(750);
```

The latch path is unaffected by this value (it is exact); the window only governs
PID-divergence detection for hook-absent users. 750ms trades a little latency on a
fast hook-absent command for reliable child-start detection on a slow one.

`LOCAL_CHILD_POLL` (25ms) and `LOCAL_SLOW_POLL` (500ms) keep their current values
and remain in use — verify no `unused const` warning after the rewrite.

### 4. Add `exit_status_annotation` and surface the code — `src/daemon/executor/foreground.rs`

**4a.** Add a pure helper (place it near the other free helpers in the file, above
the `#[cfg(test)] mod tests` block):

```rust
/// Build the trailing annotation appended to a local command's captured output so
/// the model can see a failure. Returns `None` for unknown (`None`, hook absent)
/// and clean (`Some(0)`) — neither is annotated, so a clean or
/// exit-code-unknown command reads exactly as its output. A non-zero code yields
/// a one-line note.
fn exit_status_annotation(exit_status: Option<i32>) -> Option<String> {
    match exit_status {
        Some(code) if code != 0 => Some(format!("\n[command exited with status {code}]")),
        _ => None,
    }
}
```

**4b.** At the capture/return point, change `let output = match …` to `let mut
output = match …` (so the annotation can be appended), and replace the
`read_pane_exit_status(...).unwrap_or(0)` line (~782) with the gated surfacing
below. Leave the `if switched_to_working && …` focus-restore and the
`send_response_split`/`log_command` calls in place:

```rust
            // Surface the exit status to the model — local pane only. Interactive
            // sessions never "exit"; on a remote pane the shell hook records the
            // ssh wrapper's status, not the remote command's — neither is a
            // meaningful per-command code, so both are left unannotated.
            if !is_interactive
                && !is_remote_pane
                && let Some(note) = exit_status_annotation(exit_status)
            {
                output.push_str(&note);
            }
            crate::daemon::stats::finish_command(cmd_id, exit_status.unwrap_or(0));
            send_response_split(tx, Response::ToolResult(output.clone())).await?;
```

`finish_command` keeps a concrete `i32`; `unwrap_or(0)` here feeds **stats only**
(unchanged best-effort behavior) and is *not* shown to the model — the model is
told nothing it can mistake for "succeeded". `is_remote_pane` (line ~294) and
`is_interactive` (set in the interactive branch) are already in scope.

### 5. Unit-test the pure helper — `src/daemon/executor/foreground.rs`

Extend the existing `#[cfg(test)] mod tests` (line ~1067). Add `exit_status_annotation`
to its `use super::{…}` import and add three tests (see Test plan).

## Acceptance criteria

Verifiable conditions — each checkable by running a command or reading a file.

- [ ] `tmux::clear_pane_exit_status(pane_id)` issues `tmux set-environment -u
      DE_EXIT_<num>` (no `-t`), is callable as `crate::tmux::clear_pane_exit_status`,
      and returns `()`.
- [ ] `run_foreground` calls `tmux::clear_pane_exit_status(target_str)` before
      `tmux::send_keys` for the command.
- [ ] The local completion branch reads `tmux::read_pane_exit_status(target_str)`
      as its primary completion signal and stores the result in `exit_status`;
      PID-return remains as the fallback path.
- [ ] `LOCAL_CHILD_START_WINDOW` is `Duration::from_millis(750)`.
- [ ] `exit_status_annotation(None)` and `exit_status_annotation(Some(0))` return
      `None`; `exit_status_annotation(Some(n))` for `n != 0` returns
      `Some(s)` where `s` contains the code `n`.
- [ ] The captured `output` is annotated with the exit code **only** when
      `!is_interactive && !is_remote_pane` and the code is non-zero; no
      `read_pane_exit_status(...).unwrap_or(0)` remains feeding the model.
- [ ] No new `.unwrap()`/`.expect()`/`panic!`/`unsafe` in production paths
      (existing `tmux::pane_pid(...).unwrap_or(0)` calls are retained as-is).
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Concrete tests. Per STANDARDS §3.2, the completion-wait and latch-clear changes
are tmux side effects with no hermetic seam (precedent: 07a verified Tasks 2–4 by
inspection; `FgHookGuard` ships untested for the same reason). The one genuinely
pure, newly-introduced unit is `exit_status_annotation` — test it:

- `exit_status_annotation_unknown_is_silent` in `src/daemon/executor/foreground.rs`
  — `exit_status_annotation(None)` returns `None` (unknown is never reported as a
  failure or a success).
- `exit_status_annotation_zero_is_silent` in the same module —
  `exit_status_annotation(Some(0))` returns `None` (a clean command's output is
  unannotated).
- `exit_status_annotation_nonzero_notes_code` in the same module —
  `exit_status_annotation(Some(2))` returns `Some(s)` and `assert!(s.contains("2"))`
  (pins that the code reaches the model; do not pin exact surrounding wording).

Do **not** invent tests that shell out to a real tmux server for Tasks 1–3
(non-deterministic, violates §3.3). Verify those by inspection + the End-to-end
section.

## End-to-end verification

The completion-wait and latch changes are tmux side effects, not a checked-in
artifact or CLI output; the build/clippy/test gates plus the
`exit_status_annotation` unit tests are the automatable verification. State in the
completion log:

> Tasks 1–3 are tmux-side-effect changes (latch clear, completion-wait rewrite)
> with no runtime-loadable artifact and no hermetic seam; verified by inspection.
> Task 4's pure helper is verified by the three unit tests above.

Quote the passing output of `cargo test exit_status_annotation` in the completion
Update Log.

If a live tmux session **with the shell hook installed** is available, these
manual checks confirm Tasks 1–4 (optional, record results if run):
- Run a failing local foreground command (e.g. ask the agent to run `false` or
  `ls /nonexistent`); the `ToolResult` ends with `[command exited with status N]`
  for the real non-zero `N`.
- Run a clean local command (e.g. `echo hi`); the `ToolResult` carries the output
  with **no** exit annotation.
- Run a very fast local command; it completes promptly (no spurious wait to the
  45s timeout) and its exit code is surfaced.

If no live tmux session is available, state that and rely on the unit tests +
inspection.

## Authorizations

- [ ] May add dependencies: **no.**
- [ ] May touch `docs/architecture.md`: **no.**

Adding `pub fn clear_pane_exit_status` to `src/tmux/pane.rs` and the private
`exit_status_annotation` to `foreground.rs` is in scope (new functions in
existing modules, not new files or dependencies).

## Out of scope

What the executor must **not** do, even if tempted:

- **tmux-verb leverage** (`wait-for`, `set-buffer`/`paste-buffer`, `copy-mode
  -X`, `if-shell`). This is open-ended and risks a fragile rewrite of the send
  path; it is deferred to a separately-drafted phase (originally **07c**, since
  renumbered → **phase-10** "tmux-surface-and-safe-verbs"). Do not introduce any
  of these verbs in this phase.
- **The sudo branch** (`command_has_sudo(cmd)` block, ~lines 364-601), the
  **interactive branch** (`is_interactive_command(cmd)`, ~603-646), and the
  **remote branch** (`is_remote_pane`, ~647-672). Only the local `else` branch and
  the shared capture/return point change.
- **Exit-code surfacing for interactive or remote panes.** Per § 2.4 there is no
  meaningful per-command code there; leave them unannotated (the Task 4 gate
  enforces this).
- **Changing `read_pane_exit_status` or `stats::finish_command`** signatures or
  behavior, the IPC `Response` types, or any error-message text outside the new
  exit annotation.
- **Removing or repurposing `LOCAL_CHILD_POLL` / `LOCAL_SLOW_POLL`** — both stay
  in use by the rewritten wait.
- **The N9 `monitor-silence` / `alert-silence` install** at the top of the local
  branch — leave it as-is.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 21:30 (started)

**Executor:** rexyMCP executor. Implementing Tasks 1–5: `clear_pane_exit_status` helper, latch clear before send, two-phase completion wait rewrite, `exit_status_annotation` helper with exit-code surfacing, and three unit tests.

### Update — 2026-06-22 21:30 (complete)

**Summary:** All five spec tasks implemented. The `DE_EXIT_<pane>` latch is now cleared before each foreground command send, making its reappearance the exact primary completion signal. The local completion wait was rewritten as a two-phase loop (Phase 1: start window checks for latch or PID divergence; Phase 2: waits for latch or PID return). `LOCAL_CHILD_START_WINDOW` widened from 300ms to 750ms. Non-zero exit codes are surfaced to the model via a trailing annotation on `ToolResult` output, gated to local panes only. No deviations from spec.

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
(no output — clean)

cargo build 2>&1 | tail -20
   Compiling daemoneye v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.1s

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
    Checking daemoneye v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.1s

cargo test 2>&1 | tail -30
test result: ok. 27 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

**End-to-end verification:**

Tasks 1–3 are tmux-side-effect changes (latch clear, completion-wait rewrite) with no runtime-loadable artifact and no hermetic seam; verified by inspection. Task 4's pure helper is verified by the three unit tests above.

```
cargo test exit_status_annotation 2>&1 | tail -15
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 743 filtered out
```

Grep proof of pinned literal:
```
grep -rn 'command exited with status' src/daemon/executor/foreground.rs
1104:        Some(code) if code != 0 => Some(format!("\n[command exited with status {code}]")),
```

**Files changed:**
- `src/tmux/pane.rs` — added `clear_pane_exit_status` helper (best-effort `tmux set-environment -u`)
- `src/daemon/executor/foreground.rs` — cleared latch before send, rewrote local completion wait as two-phase loop, widened `LOCAL_CHILD_START_WINDOW` to 750ms, added `exit_status_annotation` helper, surfaced non-zero exit codes to model output (local pane only), added 3 unit tests

**New tests:**
- `exit_status_annotation_unknown_is_silent` in `src/daemon/executor/foreground.rs`
- `exit_status_annotation_zero_is_silent` in `src/daemon/executor/foreground.rs`
- `exit_status_annotation_nonzero_notes_code` in `src/daemon/executor/foreground.rs`

**Commits:**
- (pending — one commit below)

**Notes for review:** None — implementation matches spec exactly.

### Review verdict — 2026-06-22

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** rexyMCP executor (Qwen/Qwen3.6-27B-FP8)
- **Scope deviations:** none — only the local completion branch and the shared
  capture/return point were touched; sudo/interactive/remote branches and the N9
  silence-monitor install left intact; `LOCAL_CHILD_POLL`/`LOCAL_SLOW_POLL`
  retained and in use.
- **Calibration:** none

Independent re-run: `cargo fmt --all -- --check`, `cargo build` (zero warnings),
`cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
(746 lib + 27 integration, 1 ignored) all pass. The three
`exit_status_annotation` unit tests pass and are non-trivial (each pins a
distinct branch of the helper). The model-facing `read_pane_exit_status(...).
unwrap_or(0)` fabrication is gone; the remaining `unwrap_or(0)` feeds stats only,
as the spec allows. No new `unwrap()`/`expect()`/`panic!`/`unsafe` in production
paths.
