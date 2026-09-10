# Phase 05b: `background=true` as an un-awaited Run on a dedicated shell

**Milestone:** M24 — Run Manager
**Status:** todo
**Depends on:** phase-05a (`spawn_command`, `CommandHandle`,
`spawn_dedicated_for`) and phase-01/02a (`RunId`, the turn log). All **done**.
**Estimated diff:** ~500 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

**This is the milestone's payoff.** Under `[execution] backend = "pty"`, a chat
turn's `run_terminal_command(background=true)`:

1. returns **immediately** with a run id and a shell id — the turn does not
   block,
2. runs the command on a **dedicated** shell (never the owner's reused
   foreground shell — 08b's poisoning rule),
3. delivers completion later as a `[Background Task Completed]` message
   carrying the **real exit code from the marker**, and
4. does not touch tmux at all — no `de-bg-*` window.

The tmux path stays exactly as it is for every other case: `backend = "tmux"`
(the default), ghost sessions, and `retry_in_pane`. This is a **branch**, not a
deletion.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 "Run manager" — background is "an
  un-awaited Run"; § 2.5 `run_terminal_command { …, wait?: bool }`.
- `docs/dev/milestones/M20-shell-engine/README.md` § Retrospective — "foreground
  vs background is no longer a property of the *shell* but of the *call* — does
  the turn await the end marker or not."
- M24 README § exit criteria, first bullet — the criterion this phase satisfies.

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-10, commit
`4fb513b`):

```sh
git status --porcelain | wc -l                                        # 0
cargo test --lib 2>&1 | grep '^test result'                           # 1726 passed
cargo test --lib daemon::executor 2>&1 | grep '^test result'          # 226 passed
cargo test --lib daemon::executor::shell_exec 2>&1 | grep '^test result'  # 10 passed
cargo test --lib daemon::run:: 2>&1 | grep '^test result'             # 50 passed
test -f src/daemon/executor/bg_shell.rs && echo 1 || echo 0           # 0
grep -c 'spawn_dedicated_for' src/daemon/executor/mod.rs              # 0
```

## Current state

### The branch to copy — the foreground PTY arm

`src/daemon/executor/mod.rs:300-393` already does exactly the shape this phase
needs for background: gate on the config, approve, run on the substrate, send
the result. **Quote it and follow it**, changing only what background changes:

```rust
            // The PTY shell substrate, when configured. A ghost session, an
            // explicit `target_pane`, or `backend = "tmux"` (the default)
            // falls through to `run_foreground` unchanged, so the default
            // behaviour is byte-for-byte today's.
            let uses_pty = crate::config::Config::load()
                .unwrap_or_default()
                .execution
                .uses_pty();
            if uses_pty
                && !is_ghost
                && session_id.is_some()
                && target.as_deref().is_none_or(|t| t.is_empty())
            {
                match prompt_and_await_approval(
                    ApprovalRequest { id, cmd, background: false, target_pane_hint: None },
                    session_id, ghost_policy.as_ref(), tx, rx,
                ).await? {
                    Err(outcome) => return Ok(outcome),
                    Ok(cmd_id) => {
                        let owner = Owner::Chat(session_id.unwrap_or_default().to_string());
                        let max_per_owner = crate::config::Config::load()
                            .unwrap_or_default().shells.max_per_owner;
                        let outcome = match shell_exec::run_in_shell(
                            crate::shell::registry(), owner.clone(), cmd, max_per_owner,
                        ).await {
                            Ok((shell_id, result)) => {
                                crate::daemon::stats::finish_command(cmd_id, result.exit_code);
                                send_response_split(tx, Response::ToolResult {
                                    tool_call_id: id.to_string(),
                                    output: result.text.clone(),
                                }).await?;
                                crate::daemon::utils::log_shell_command(
                                    session_id, shell_id.as_str(), cmd,
                                    "approved", result.exit_code, &result.text,
                                );
                                result.text
                            }
                            Err(e) => { /* names the shell, warns the command may still run */ }
                        };
                        return Ok(ToolCallOutcome::Result(outcome));
                    }
                }
            }
```

### The background arm today

`src/daemon/executor/mod.rs:415-434` — unconditional, and the arm this phase
adds the branch to:

```rust
        PendingCall::Background { id, cmd, retry_pane, .. } => {
            foreground::run_background(
                id, cmd, retry_pane.as_deref(), ctx,
                GhostCtx { policy: ghost_policy.as_ref(), is_ghost },
                tx, rx,
            ).await
        }
```

### How a completion reaches the session today

`src/daemon/background/helpers.rs:146-205` (`notify_session`) is the tmux
version. The **four-phase lock discipline** is the part to reproduce; the tmux
specifics (`bg_windows`, `pane_id`, the persist note) are not:

```rust
    // Phase 1 (locked): update the registry and take what the rest needs.
    let Some(chat_pane) = with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;
        // … tmux-specific bg_windows bookkeeping …
        Some(entry.chat_pane.clone())
    }) else { return };

    // Phase 2 (unlocked): the filesystem scan, the formatting, and the file write.
    let history_content = format!(/* … */);
    let completion_msg = Message {
        role: "user".to_string(),
        content: format!("[Background Task Completed]\n{}", history_content),
        tool_calls: None, tool_results: None, turn: None,
    };
    append_session_message(session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) { entry.messages.push(completion_msg); }
    });

    // Phase 4 (unlocked): the tmux notification.
```

**`with_shells`/`with_sessions` are not reentrant** — never call one inside the
other's closure, and never `.await` inside either. Collect, return, act after.

### The test harness to reuse verbatim

`src/daemon/executor/shell_exec.rs:128-167` already builds a real in-process PTY
host. **Use this**; `spawn_detached_host` re-invokes `current_exe()`, which under
a test binary is not the daemon:

```rust
    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1).
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()); }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    async fn spawn_with_cast(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket).await.context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }

    async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
        let cast = spawn_with_cast(id.clone()).await?;
        std::mem::forget(cast);
        Ok(())
    }
```

### The 05a primitives this phase consumes

```rust
pub async fn spawn_dedicated_for<F, Fut>(          // src/shell/session.rs
    reg: &ShellRegistry, owner: Owner, max_per_owner: u32,
    ready_timeout: Duration, spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>;

pub async fn spawn_command(                        // src/shell/client.rs
    sock: &Path, cmd: &str, shell_name: &str, timeout: Duration,
) -> anyhow::Result<CommandHandle>;

pub struct CommandHandle {
    pub outcome: tokio::sync::oneshot::Receiver<anyhow::Result<CommandOutcome>>,
    pub nonce: Nonce,
}
pub struct CommandOutcome { pub output: Vec<u8>, pub exit_code: i32 }
```

## Spec

Six tasks. Everything testable lives in the new `bg_shell.rs` and takes **no**
`tx`/`rx`; the IPC and approval stay in the executor arm. That separation is
what makes this phase testable at all — it is the same split
`shell_exec::run_in_shell` uses.

### Task 1 — `src/daemon/executor/bg_shell.rs`, the start half

```rust
/// The ids a started background run hands back to the model.
pub(super) struct BackgroundStart {
    pub run_id: RunId,
    pub shell_id: ShellId,
}

/// Start `cmd` on a **dedicated** shell for `owner` and return without
/// awaiting it. Registers the run and writes `RunStarted` to its turn log.
///
/// The spawn step is injected: `spawn_detached_host` re-invokes `current_exe()`
/// and cannot run under a test binary.
pub(super) async fn start_background_with<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    spawn: F,
) -> anyhow::Result<(BackgroundStart, CommandHandle)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>;

/// The production form: `start_background_with` with the real spawn.
pub(super) async fn start_background(
    reg: &ShellRegistry, owner: Owner, cmd: &str, max_per_owner: u32,
) -> anyhow::Result<(BackgroundStart, CommandHandle)>;
```

Body order, and each step's reason:

1. `RunId::new()`.
2. `spawn_dedicated_for(reg, owner, max_per_owner, SHELL_READY_TIMEOUT, spawn)`
   — reuse `shell_exec::SHELL_READY_TIMEOUT`, do not invent a second constant.
   A cap error propagates unchanged: a background command that would exceed
   `[shells] max_per_owner` **must fail loudly**, not silently borrow a
   foreground shell.
3. `append_turn_event` a `TurnLogEvent::RunStarted { kind: RunKind::Chat {
   session } }` for the run.
4. `spawn_command(&socket, cmd, &login_shell(), BACKGROUND_CMD_TIMEOUT)`.
5. Return the ids and the handle.

Add `pub(super) const BACKGROUND_CMD_TIMEOUT: Duration = Duration::from_secs(3600);`
with a comment saying why it is not `foreground::LOCAL_CMD_TIMEOUT` (45 s): a
background command is *expected* to outlive a turn; the ceiling exists only so a
wedged command cannot leak a reader task forever.

**Interactive commands are refused here, not started.** If
`crate::daemon::utils::is_interactive_command(cmd)` is true, return an `Err`
whose message says a background `ssh`/`mosh` session can never be typed into and
to run it in the foreground instead. (`run_in_shell` handles this by returning
an "[Interactive session started]" result on a *foreground* shell; backgrounding
one would strand it.)

### Task 2 — the completion message (pure)

```rust
/// The `[Background Task Completed]` body for a finished background run.
/// Pure: no locks, no IO, no clock.
pub(super) fn completion_message(
    cmd: &str,
    run_id: &RunId,
    shell_id: &ShellId,
    outcome: &Result<CommandOutcome, String>,
) -> String;
```

On success the first line **must** be exactly:

```
Background command `<cmd>` (run <run-id>, shell <shell-id>) finished with exit code <N>.
```

followed by the output inside `<output>` … `</output>`, with the same
annotate → mask → normalize order `shell_exec::format_for_model` uses (F3: the
annotation step strips control bytes *before* masking, or an escape embedded
mid-token could split a secret past its regex). Reuse
`shell_exec::format_for_model` — do not re-implement the order.

On failure (the handle resolved to `Err`, e.g. the timeout) the first line is:

```
Background command `<cmd>` (run <run-id>, shell <shell-id>) did not complete: <error>.
```

and the body must say the command **may still be running in that shell**, so the
model does not retry and double-apply it. Same warning `run_in_shell`'s error
path gives.

The whole string is prefixed by the caller with `"[Background Task Completed]\n"`
— **that literal is what `build_catchup_brief` scans for**
(`src/daemon/server/catchup.rs:46` matches `"[Background Task Completed"`), so
it must not drift.

### Task 3 — await and notify

```rust
/// Await the run's outcome, write `RunFinished` to its turn log, and inject the
/// completion into the session. Runs detached; never returns to the turn.
pub(super) async fn await_and_notify(
    sessions: SessionStore,
    session_id: String,
    cmd: String,
    start: BackgroundStart,
    handle: CommandHandle,
);
```

Reproduce `notify_session`'s **four-phase lock discipline** (quoted in § Current
state): take what you need under the lock, format and `append_session_message`
unlocked, push into `entry.messages` under the lock, notify tmux last. Drop the
tmux `bg_windows` bookkeeping — there is no window.

`RunFinished { state }` uses `RunState::Complete` when the handle delivered an
outcome (**whatever the exit code** — a command that ran and exited 1 completed;
it did not fail to run) and `RunState::HardFail { .. }` when the handle resolved
to `Err`. A session entry that has disappeared is a silent return, exactly as
`notify_session` does.

### Task 4 — the executor branch

In `PendingCall::Background` (`src/daemon/executor/mod.rs:415`), **before** the
`foreground::run_background` call, add the branch. The gate is the foreground
arm's gate plus one term:

```rust
let uses_pty = crate::config::Config::load().unwrap_or_default().execution.uses_pty();
if uses_pty
    && !is_ghost
    && session_id.is_some()
    && retry_pane.as_deref().is_none_or(|p| p.is_empty())
{ … }
```

**`retry_pane` is the extra term** and it is load-bearing: `retry_in_pane` names
a tmux pane, which the substrate has none of, so that request must keep falling
through to the tmux path unchanged.

Inside: `prompt_and_await_approval` with `background: true` and
`target_pane_hint: None`; on approval call `start_background`, then

- **Success** — send `Response::ToolResult` **immediately**, whose text names
  the run id, the shell id, and that completion arrives later as a
  `[Background Task Completed]` message. Then
  `tokio::spawn(await_and_notify(...))` and return
  `ToolCallOutcome::Result(text)`. Do **not** await the handle in the arm.
- **Failure to start** — `crate::daemon::stats::finish_command(cmd_id, 1)`, send
  the error as the tool result, and return it.

Call `crate::daemon::stats::finish_command(cmd_id, 0)` on the *start* success
path: the command was dispatched. (The real exit code is not known yet and the
stats row is about dispatch, not outcome.)

### Task 5 — `mod.rs` wiring

`src/daemon/executor/mod.rs`: add `mod bg_shell;` beside `mod shell_exec;`
(`src/daemon/executor/mod.rs:6`). Private, like its neighbour.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-05b.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `test -f src/daemon/executor/bg_shell.rs && echo 1 || echo 0` → **1**
      (now `0`), and `grep -c '^mod bg_shell;' src/daemon/executor/mod.rs` → **1**
      (now `0`).
- [ ] Declared exactly once each in `src/daemon/executor/bg_shell.rs`:
      `async fn start_background_with` → **1**, `async fn start_background` → **2**
      (the `_with` name contains the shorter one — that is why this is 2, not 1),
      `fn completion_message` → **1**, `async fn await_and_notify` → **1**
      (each **0** now).
- [ ] **The dedicated shell is used, and reuse is not:**
      `grep -c 'spawn_dedicated_for' src/daemon/executor/bg_shell.rs` → **1 or
      more**, and `grep -c 'shell_for_owner' src/daemon/executor/bg_shell.rs` →
      **0**.
- [ ] **The turn does not await the command:**
      `grep -c 'handle.outcome' src/daemon/executor/mod.rs` → **0** (the await
      belongs to `await_and_notify`, never the arm), and
      `grep -c 'tokio::spawn' src/daemon/executor/mod.rs` → **2 or more** (now
      `1`). *(The existing 1 is prose — a doc comment at
      `src/daemon/executor/mod.rs:53` saying the caller must `tokio::spawn`.
      Yours is the first real call site in this file.)*
- [ ] **`retry_in_pane` still reaches the tmux path:**
      `grep -c 'retry_pane' src/daemon/executor/mod.rs` → **3 or more** (now
      `2`) — today's destructure (`:418`) and pass-through (`:424`), plus the
      new gate term.
- [ ] **The catch-up literal did not drift:**
      `grep -c '\[Background Task Completed\]' src/daemon/executor/bg_shell.rs`
      → **1 or more**, and `cargo test --lib daemon::server::catchup` → **19**,
      unchanged (now **19**). *(Preservation: the literal this phase emits is
      what `build_catchup_brief` scans for.)*
- [ ] **One timeout constant, and it is not the foreground one:**
      `grep -c 'BACKGROUND_CMD_TIMEOUT' src/daemon/executor/bg_shell.rs` → **2 or
      more** (definition + use), and
      `grep -c 'LOCAL_CMD_TIMEOUT' src/daemon/executor/bg_shell.rs` → **0**.
- [ ] **Masking is not re-implemented:**
      `grep -c 'format_for_model' src/daemon/executor/bg_shell.rs` → **1 or
      more**, and `grep -c 'mask_sensitive' src/daemon/executor/bg_shell.rs` →
      **0**.
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/executor/bg_shell.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0**.
- [ ] **No real sleeps on the Rust side of the new tests** (a `sleep` *inside a
      shell command* is the subject under test and is fine):
      `grep -cE 'thread::sleep' src/daemon/executor/bg_shell.rs` → **0**.
- [ ] `cargo test --lib daemon::executor::bg_shell` → **9 or more** passing,
      `0 failed` (now **0** — no such module). *Arithmetic: the 9 tests in the
      Test plan.*
- [ ] `cargo test --lib daemon::executor` → **235 or more** passing, `0 failed`
      (now **226**). *226 + 9.*
- [ ] **Preservation, each must not move:** `daemon::executor::shell_exec` **10**,
      `daemon::run::` **50**, `shell::client` **15**, `shell::session` **10**,
      `shell::registry` **28**, `daemon::background` **12**. *The tmux background path is branched around,
      never modified.*
- [ ] **`background/` is untouched:**
      `git diff --name-only 4fb513b -- src/daemon/background/ | wc -l` → **0**.
- [ ] **The real HOME's shell litter does not grow** — same before and after a
      full suite run (**5** now: pre-existing M20 `shell::launch` litter, an open
      milestone item). Every new test uses a tempdir HOME.
- [ ] Full lib suite **1735 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1726**).
- [ ] All four gates pass.

## Test plan

All in `src/daemon/executor/bg_shell.rs`, using the harness quoted in § Current
state.

**`completion_message` — pure, no host needed (4):**

- `bg_completion_names_the_run_the_shell_and_the_exit_code` — a success outcome
  with `exit_code: 7` produces a first line containing the run id, the shell id
  and `exit code 7`.
- `bg_completion_wraps_the_output_in_output_tags` — the command's bytes appear
  between `<output>` and `</output>`.
- `bg_completion_masks_a_secret_in_the_output` — an output containing something
  the masking filter catches comes back masked. **This is the negative case**:
  assert the raw secret is **not** present, not merely that something changed.
- `bg_completion_on_failure_warns_the_command_may_still_run` — an `Err` outcome
  produces the "did not complete" line **and** the may-still-be-running warning,
  and does **not** claim an exit code.

**`start_background_with` — real PTY host (3):**

- `bg_start_returns_before_the_command_finishes` — `sleep 1; printf 'LATE\n'`;
  the call returns and the handle has **not** resolved (`try_recv` is `Empty`).
  Awaiting it afterwards still delivers `LATE`.
- `bg_start_uses_a_dedicated_shell_not_the_owners_existing_one` — give the owner
  a live reusable shell first (via `shell_for_owner`), then start a background
  command: the returned `shell_id` differs, and its registry record has
  `dedicated == true`.
- `bg_start_refuses_an_interactive_command` — `ssh host` returns `Err` naming
  the foreground alternative, and **no** shell is allocated for it (the registry
  gains no record).

**`await_and_notify` — real PTY host + a real `SessionStore` (2):**

- `bg_notify_pushes_a_completion_message_carrying_the_real_exit_code` — run
  `printf 'DONE\n'; sh -c 'exit 4'`; after `await_and_notify`, the session
  entry's last message has `role == "user"`, starts with
  `[Background Task Completed]`, and contains `exit code 4` and `DONE`.
- `bg_notify_on_a_vanished_session_is_a_silent_return` — remove the session
  entry before awaiting; the call returns without panicking and the store stays
  empty. *(The negative case for the four-phase lock discipline.)*

## Mutation checks

Three. Use the `patch` tool to break and to revert — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.**
Record the red-then-green transcript for each in the Update Log.

Each names **the assertion message expected in the failure output**, not just
the test. A mutation that reddens a multi-assertion test proves only that *some*
assertion fired; quote the line that actually failed, so the check is evidence
about the property you meant to guard.

1. **The turn must not block.** In `start_background_with`, await
   `handle.outcome` before returning (send the result into a fresh oneshot so
   the types still fit) → `bg_start_returns_before_the_command_finishes` must go
   **red** on its "must not have resolved yet" assertion, **not** on the `LATE`
   one.
2. **The shell must be dedicated.** Swap `spawn_dedicated_for` for
   `shell_for_owner` in `start_background_with` →
   `bg_start_uses_a_dedicated_shell_not_the_owners_existing_one` must go **red**
   on the "different shell id" assertion.
3. **The real exit code must reach the session.** In `completion_message`, render
   a hardcoded `0` instead of `outcome.exit_code` →
   `bg_completion_names_the_run_the_shell_and_the_exit_code` must go **red**
   naming the expected `7`, and
   `bg_notify_pushes_a_completion_message_carrying_the_real_exit_code` must
   redden too — note whether it did.

## End-to-end verification

The real artifact is the substrate: a real `bash` behind a real PTY, a real
socket, the real marker protocol, and a real `SessionStore`. Sections B and C
exercise exactly that. Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-05b.txt
: > "$A"
SHELLS_BEFORE=$(ls ~/.daemoneye/var/run/shells/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME shells BEFORE the suite: $SHELLS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the new background tests, against a real bash PTY =="
cargo test --lib daemon::executor::bg_shell -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::executor::bg_shell|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. module counts (bg_shell >=9, executor >=235) =="
for m in daemon::executor::bg_shell daemon::executor; do
  printf '%-30s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== D. preservation: shell_exec 10, run 50, client 15, session 10, registry 28, background 12, catchup 19 =="
for m in daemon::executor::shell_exec daemon::run:: shell::client shell::session shell::registry daemon::background daemon::server::catchup; do
  printf '%-32s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== E. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== F. structural greps (each must print the stated number) =="
printf 'bg_shell.rs exists         (1): %s\n' "$(test -f src/daemon/executor/bg_shell.rs && echo 1 || echo 0)"
printf 'mod bg_shell; private      (1): %s\n' "$(grep -c '^mod bg_shell;' src/daemon/executor/mod.rs)"
printf 'start_background_with      (1): %s\n' "$(grep -c 'async fn start_background_with' src/daemon/executor/bg_shell.rs)"
printf 'start_background           (2): %s\n' "$(grep -c 'async fn start_background' src/daemon/executor/bg_shell.rs)"
printf 'completion_message         (1): %s\n' "$(grep -c 'fn completion_message' src/daemon/executor/bg_shell.rs)"
printf 'await_and_notify           (1): %s\n' "$(grep -c 'async fn await_and_notify' src/daemon/executor/bg_shell.rs)"
printf 'uses spawn_dedicated_for (>=1): %s\n' "$(grep -c 'spawn_dedicated_for' src/daemon/executor/bg_shell.rs)"
printf 'never shell_for_owner      (0): %s\n' "$(grep -c 'shell_for_owner' src/daemon/executor/bg_shell.rs)"
printf 'arm never awaits outcome   (0): %s\n' "$(grep -c 'handle.outcome' src/daemon/executor/mod.rs)"
printf 'arm detaches             (>=2): %s\n' "$(grep -c 'tokio::spawn' src/daemon/executor/mod.rs)"
printf 'retry_pane still gated   (>=3): %s\n' "$(grep -c 'retry_pane' src/daemon/executor/mod.rs)"
printf 'catchup literal present  (>=1): %s\n' "$(grep -c '\[Background Task Completed\]' src/daemon/executor/bg_shell.rs)"
printf 'BACKGROUND_CMD_TIMEOUT   (>=2): %s\n' "$(grep -c 'BACKGROUND_CMD_TIMEOUT' src/daemon/executor/bg_shell.rs)"
printf 'not the fg timeout         (0): %s\n' "$(grep -c 'LOCAL_CMD_TIMEOUT' src/daemon/executor/bg_shell.rs)"
printf 'reuses format_for_model  (>=1): %s\n' "$(grep -c 'format_for_model' src/daemon/executor/bg_shell.rs)"
printf 'no re-implemented masking  (0): %s\n' "$(grep -c 'mask_sensitive' src/daemon/executor/bg_shell.rs)"
printf 'no rust-side sleep         (0): %s\n' "$(grep -cE 'thread::sleep' src/daemon/executor/bg_shell.rs)"
printf 'no unwrap/panic pre-test   (0): %s\n' "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/executor/bg_shell.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"

echo "== G. the tmux background path is untouched, no HOME litter =="
printf 'background/ diff lines     (0): %s\n' "$(git diff --name-only 4fb513b -- src/daemon/background/ | wc -l)"
printf 'real HOME shells AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/run/shells/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
sed -i 's/\x1b\[[0-9;]*m//g' "$A" 2>/dev/null || true
cat "$A"
```

*(The trailing `sed -i` is the **architect's** transcript-cleanup line and is
part of this block, not an edit to a source file; if `bash` refuses it, the
`|| true` lets the block finish and section B has already stripped colour.)*

Then, as the last task, the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-05b-background.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-05b.txt
diff /tmp/pasted-m24-05b.txt /tmp/e2e-m24-05b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/executor/bg_shell.rs` — new file, tasks 1–3, 6.
- `src/daemon/executor/mod.rs` — tasks 4, 5 **only**: the `mod bg_shell;` line
  and the branch inside the `PendingCall::Background` arm. Do not restructure
  anything else in this 1814-line file.
- `docs/dev/milestones/M24-run-manager/phase-05b-background.md` — the Update Log.

**If a visibility change is needed** to reach an existing item from `bg_shell.rs`
— `shell_exec::SHELL_READY_TIMEOUT`, `shell_exec::format_for_model`,
`foreground::exit_status_annotation` — widening it from `pub(super)` is
authorized **within `src/daemon/executor/`** and nowhere else. Say which in the
Update Log. Do **not** copy a function to avoid a visibility change.

**Do not touch** `src/daemon/background/` (§ G asserts a zero diff),
`src/shell/` (05a is approved and closed), or anything under `src/daemon/run/`.

## Out of scope

- **Cancelling a running background command.** `daemoneye stop <run>` writes the
  sentinel (phase-04) but nothing watches it for a background run yet; wiring
  `watch_stop_sentinel` to the handle is phase-06's, when a chat turn is a `Run`.
- **`daemoneye runs` listing background runs** — phase-10.
- **Removing the tmux `de-bg-*` code.** It still serves `backend = "tmux"`,
  ghosts and `retry_in_pane`. Deleting it is a later milestone's call, not this
  phase's.
- **`close_background_window` / `retry_in_pane` on the substrate.** Both are
  tmux-pane concepts; § 2.5 removes the tools outright in the tool-surface work.
- **The `wait` parameter rename** (`background=true` → `wait:false`). Tool-schema
  work, and it would drag `CLAUDE.md`/`README.md`'s tool tables and the
  `doc_truth` gates in.
- **Sandboxing the substrate background path.** `container.rs` wraps the tmux
  `de-bg-*` window; the substrate equivalent is its own phase. Ghosts are
  excluded from this branch, which is what keeps the sandbox gate honest.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
