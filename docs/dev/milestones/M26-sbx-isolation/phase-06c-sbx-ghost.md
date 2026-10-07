# Phase 06c: sbx-ghost — a sandboxed local ghost runs in its own sandbox

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** in-progress
**Depends on:** phase-06b
**Estimated diff:** ~420 lines (about 180 code, 240 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Put the pieces together for the milestone's first real capability. With
`[sandbox] enabled = true` and `runtime = "sbx"`, a **local** ghost's commands
run inside a sandbox that belongs to that ghost run:

- created on its first command, with a fail-closed preflight;
- reached through a sandbox shell;
- torn down when the ghost exits.

The daemon also sweeps leaked daemon sandboxes at startup. A remote ghost, an
unsandboxed ghost and `runtime = "docker"` behave exactly as today.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "PE decisions" (one
  sandbox per ghost run) and § "Measured on pinky".
- Rows 06a (`sbx_lifecycle`) and 06b (`shell_for_owner_in_sandbox`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Where a local ghost's command runs today (`src/daemon/executor/foreground.rs`, `run_background`)

`session_id`, `sessions`, `ghost_policy`, `is_ghost` and `uses_pty` are all
in scope. The local-ghost branch is:

```rust
if super::ghost_remote::ghost_local_pty(
    is_ghost,
    uses_pty,
    crate::config::Config::load()
        .unwrap_or_default()
        .sandbox
        .enabled,
    ghost_policy,
) {
    let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
    let (output, exit_code) = super::ghost_remote::run_ghost_remote(
        crate::shell::registry(),
        owner,
        crate::host::HostId::Local,
        cmd,
        id,
        |shell_id| crate::shell::spawn_detached_host(shell_id, &[], 24, 80),
    )
    .await;
    crate::daemon::stats::finish_command(cmd_id, exit_code);
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: output.clone(),
        },
    )
    .await?;
    log_command(session_id, "background", "", cmd, "approved", &output);
    return Ok(ToolCallOutcome::Result(output));
}
```

`ghost_local_pty` (`src/daemon/executor/ghost_remote.rs`) is true only when
the sandbox is **off**:

```rust
pub(super) fn ghost_local_pty(is_ghost: bool, uses_pty: bool, sandbox_enabled: bool, policy: Option<&GhostPolicy>) -> bool {
    is_ghost && uses_pty && !sandbox_enabled && policy.is_some_and(|p| p.ssh_target.is_none())
}
```

### `run_ghost_remote`: the body to copy (same file)

```rust
pub(super) async fn run_ghost_remote<F, Fut>(reg: &ShellRegistry, owner: Owner, host: HostId, cmd: &str, call_id: &str, spawn: F) -> (String, i32)
where F: FnOnce(crate::shell::ShellId) -> Fut, Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let max_per_owner = crate::config::Config::load().unwrap_or_default().shells.max_per_owner;
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::BufReader::new(tokio::io::empty());
    let outcome = super::shell_exec::run_in_shell_with_prompts(
        reg, owner.clone(), cmd, max_per_owner, None, Some(GHOST_REMOTE_TIMEOUT_SECS),
        call_id, &mut sink, &mut eof, host, spawn,
    ).await;
    match outcome {
        Ok((_, result)) => (result.text, result.exit_code),
        Err(e) => {
            let shell_id = crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_else(|| "s?".to_string());
            (super::shell_run_error_text(&shell_id, &e), 1)
        }
    }
}
```

### The ghost's profile name (from `src/daemon/background/run.rs`)

```rust
// The sandbox profile this job runs under is the ghost's agent name; an
// interactive session has none and so resolves to the default profile.
let profile_name = session_id.as_deref().and_then(|id| {
    with_sessions(&sessions, |store| {
        store
            .get(id)
            .and_then(|e| e.ghost_config.as_ref().and_then(|g| g.agent.clone()))
    })
});
```

In `run_background`, `session_id` is an `Option<&str>` and `sessions` is a
reference, so the call is `with_sessions(sessions, …)` with `session_id`
directly.

### The ghost's exit path (`src/daemon/ghost.rs`, after the mailbox write)

```rust
let (sbx, sid) = (config.sandbox.clone(), session_id.to_string());
let _ = tokio::task::spawn_blocking(move || {
    crate::daemon::executor::container::teardown_ghost_containers(&sbx, &sid)
})
.await;
// 08c: the ghost's shells die with the ghost …
let owner = crate::shell::Owner::Ghost(session_id.to_string());
let closed = crate::daemon::executor::knowledge::close_owner_shells(
    crate::shell::registry(), &owner, Duration::from_secs(5),
).await;
if !closed.is_empty() {
    crate::daemon::utils::log_event("ghost_shells_closed", serde_json::json!({ … }));
}
```

### Startup (`src/daemon/mod.rs`)

Inside `if startup_config.execution.uses_pty() { … }`, the registry is
adopted:

```rust
let reg = crate::shell::registry().clone();
let adopted = crate::shell::adopt_at_startup(&reg, &crate::config::shells_dir(), &crate::config::shell_logs_dir());
if !adopted.is_empty() { … log::info!(…) }
Some(reg)
```

### Measured `sbx` fact used here

A duplicate create gives exit 1 and stderr
`error: sandbox '<n>' already exists; connect to the existing sandbox`.

## Spec

### Task 1 — `ghost_sandbox` module (`src/daemon/executor/ghost_sandbox.rs`)

Declare it in `src/daemon/executor/mod.rs` as `pub(crate) mod ghost_sandbox;`.
Everything below is `pub`. Every item is used in this phase, so there is no
`dead_code` risk.

```rust
/// True for a ghost on the PTY backend whose command runs locally (no
/// `ssh_target`) while the sandbox is enabled with runtime "sbx".
pub fn ghost_sandbox_pty(is_ghost: bool, uses_pty: bool, sandbox: &crate::config::SandboxConfig, policy: Option<&crate::daemon::policy::GhostPolicy>) -> bool
// = is_ghost && uses_pty && sandbox.enabled && sandbox.runtime == "sbx"
//   && policy.is_some_and(|p| p.ssh_target.is_none())

/// Make sure the ghost run `session` has its sandbox; returns the sandbox name.
/// The first call runs preflight and creates the sandbox; later calls in the
/// same daemon return at once. `Err` is an operator-facing reason, ready to
/// hand to the model, starting `sandbox unavailable:`.
pub async fn ensure_ghost_sandbox(run: &crate::shell::SbxRunner, cfg: &crate::config::SandboxConfig, session: &str, profile: Option<&str>) -> Result<String, String>

/// Tear the ghost run's sandbox down and forget it.
pub async fn teardown_ghost_sandbox(run: &crate::shell::SbxRunner, session: &str) -> crate::daemon::executor::sbx_lifecycle::Teardown

/// `run_ghost_remote`, in this ghost's sandbox shell.
pub async fn run_ghost_in_sandbox<F, Fut>(reg: &crate::shell::ShellRegistry, owner: crate::shell::Owner, sandbox: &str, cmd: &str, call_id: &str, spawn: F) -> (String, i32)
```

**Created-sandbox set.** Keep a process-wide set of the sandbox names this
daemon has created:

```rust
static CREATED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
fn created() -> &'static std::sync::Mutex<std::collections::HashSet<String>> { CREATED.get_or_init(Default::default) }
```

Lock it with `.unwrap_or_log()` (`use crate::util::UnpoisonExt;`), as every
mutex in this crate is (`CLAUDE.md` invariant). **Never hold the lock across
an `.await`**: check or insert, then drop.

**`ensure_ghost_sandbox`.**
1. `name = sbx::sandbox_name(SandboxKind::Ghost, session)`. If `created()`
   contains it → `Ok(name)`, with **no** `sbx` call.
2. `sbx_lifecycle::preflight(run).await`. On `Err(r)` →
   `Err(sbx::describe_unavailable(&r))`.
3. `sbx_lifecycle::create_sandbox(run, cfg, profile, SandboxKind::Ghost, session).await`:
   - `Ok(_)` → insert into `created()` and `Ok(name)`;
   - `Err(e)` whose `format!("{e:#}")` contains `already exists` → insert and
     `Ok(name)`. A previous daemon created it, and the startup sweep did not
     own it;
   - any other `Err(e)` → `Err(format!("sandbox unavailable: {e:#}"))`.

**`teardown_ghost_sandbox`.** Remove the name from `created()`, then return
`sbx_lifecycle::teardown_sandbox(run, &name).await`. A ghost that never ran a
sandboxed command gets `rm` of a missing sandbox, which is `Removed` (06a), so
it is harmless.

**`run_ghost_in_sandbox`.** The same shape as `run_ghost_remote` above,
except:
1. Obtain the shell first:
   `crate::shell::shell_for_owner_in_sandbox(reg, owner.clone(), sandbox, max_per_owner, Duration::from_secs(15), spawn).await`.
   On `Err(e)` return `(format!("Error: could not open a shell in sandbox {sandbox}: {e:#}"), 1)`.
2. Then call `super::shell_exec::run_in_shell_with_prompts` with
   `shell = Some(id.as_str())`, `host = HostId::Local`, a timeout of
   `Some(super::ghost_remote::GHOST_REMOTE_TIMEOUT_SECS)`, and a spawn closure
   `|_id| async { Ok(()) }`. It is never called, because the shell is given.
3. Map the outcome exactly as `run_ghost_remote` does, using
   `super::shell_run_error_text(id.as_str(), &e)` on error.

`GHOST_REMOTE_TIMEOUT_SECS` is `pub(super)` in `ghost_remote`. Use
`super::ghost_remote::GHOST_REMOTE_TIMEOUT_SECS`; both files are inside
`executor`.

### Task 2 — Route the sandboxed local ghost (`foreground.rs`, `run_background`)

Immediately **before** the existing `if super::ghost_remote::ghost_local_pty(…)`
block, add:

```rust
let sandbox_cfg = crate::config::Config::load().unwrap_or_default().sandbox;
if super::ghost_sandbox::ghost_sandbox_pty(is_ghost, uses_pty, &sandbox_cfg, ghost_policy) {
    let sid = session_id.unwrap_or_default().to_string();
    let profile = with_sessions(sessions, |store| {
        store.get(sid.as_str()).and_then(|e| e.ghost_config.as_ref().and_then(|g| g.agent.clone()))
    });
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    let run = crate::shell::real_runner(program);
    let (output, exit_code) =
        match super::ghost_sandbox::ensure_ghost_sandbox(&run, &sandbox_cfg, &sid, profile.as_deref()).await {
            Ok(sandbox) => {
                let spawn_sandbox = sandbox.clone();
                super::ghost_sandbox::run_ghost_in_sandbox(
                    crate::shell::registry(),
                    crate::shell::Owner::Ghost(sid.clone()),
                    &sandbox,
                    cmd,
                    id,
                    move |shell_id| async move {
                        crate::shell::spawn_sbx_host(shell_id, &spawn_sandbox, 24, 80).await
                    },
                )
                .await
            }
            Err(reason) => (reason, 1),
        };
    // … then exactly the four lines the local branch ends with:
    // finish_command, send ToolResult, log_command(session_id, "background", "", cmd, "approved", &output), return.
}
```

If the borrow checker rejects the closure shape (`spawn_sandbox` moved into an
`async move` that borrows it), use
`move |shell_id| { let s = spawn_sandbox.clone(); async move { crate::shell::spawn_sbx_host(shell_id, &s, 24, 80).await } }`.

**Fail closed:** a preflight or create failure returns the reason to the
model as the tool result with exit 1. The command is **never** run on the
host.

Leave `ghost_local_pty` and its branch unchanged. With the sandbox enabled
they still route a `runtime = "docker"` ghost to the old container path below.

### Task 3 — Tear the sandbox down when the ghost exits (`src/daemon/ghost.rs`)

Right **after** the `close_owner_shells` block (shells first, then the VM):

```rust
if config.sandbox.enabled && config.sandbox.runtime == "sbx" {
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    let outcome = crate::daemon::executor::ghost_sandbox::teardown_ghost_sandbox(
        &crate::shell::real_runner(program), session_id,
    ).await;
    crate::daemon::utils::log_event("ghost_sandbox_teardown", serde_json::json!({
        "session": session_id,
        "outcome": format!("{outcome:?}"),
    }));
}
```

`config` and `session_id` are the names already used by the neighbouring
`teardown_ghost_containers` call. `log_event` stamps `pid` itself; do not pass
one (`CLAUDE.md` invariant).

### Task 4 — Startup sweep (`src/daemon/mod.rs`)

Inside the `uses_pty()` block, right after the `adopt_at_startup` logging and
before `Some(reg)`:

```rust
if startup_config.sandbox.enabled && startup_config.sandbox.runtime == "sbx" {
    let live: Vec<String> = crate::shell::with_shells(&reg, |store| {
        store.values().filter_map(|r| r.sandbox.clone()).collect()
    });
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    tokio::spawn(async move {
        let report = crate::daemon::executor::sbx_lifecycle::sweep_daemon_sandboxes(
            &crate::shell::real_runner(program), &live,
        ).await;
        crate::daemon::utils::log_event("sbx_startup_sweep", serde_json::json!({
            "removed": report.removed, "stopped": report.stopped,
            "failed": report.failed, "list_error": report.list_error,
        }));
    });
}
```

`tokio::spawn` keeps startup from waiting on `sbx`. `with_shells` is exported
from `crate::shell` (check `src/shell/mod.rs`). If the surrounding function is
not async, `tokio::spawn` still works inside the runtime.

### Task 5 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib executor::ghost_sandbox` reports **at least 9 passed;
      0 failed**.
- [ ] `cargo test --lib executor::ghost_remote` and
      `cargo test --lib executor::sbx` report `0 failed`, with no existing
      test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code; every mutex goes through `.unwrap_or_log()`.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/daemon/executor/ghost_sandbox.rs` tests. Copy the fake runner from
`src/daemon/executor/sbx_lifecycle.rs` tests (it records every argv and
returns canned `VmRun`s in order). **Give every test its own session id**
(for example `"t-ensure-1"`): the created-set is process-wide, and tests run
in parallel.

- `ghost_sandbox_pty_only_for_a_local_sbx_ghost`: true for
  `(true, true, enabled+"sbx", local policy)`. False when any one of these
  flips: not a ghost; not PTY; sandbox disabled; `runtime = "docker"`; a policy
  with `ssh_target: Some(..)`; no policy.
- `ensure_ghost_sandbox_creates_once`: canned version, daemon status and a
  create exit 0 → `Ok("de-ghost-t-ensure-1")`. A **second** call returns the
  same name and records **no** further calls.
- `ensure_ghost_sandbox_fails_closed_on_preflight`: version run
  `VmRun { code: None, stderr: "spawning sbx: No such file", .. }` → `Err`
  starting `sandbox unavailable:`. No create call was made.
- `ensure_ghost_sandbox_adopts_an_existing_sandbox`: create exit 1 with
  stderr `error: sandbox 'de-ghost-t-ensure-3' already exists; connect to the existing sandbox`
  → `Ok(name)`.
- `ensure_ghost_sandbox_reports_a_failed_create`: create exit 1 with stderr
  `boom` → `Err` starting `sandbox unavailable:` and containing `boom`. A
  later call tries to create **again**: failures are not cached.
- `ensure_ghost_sandbox_uses_the_agent_profile`: a config with
  `profile["researcher"] = proxy, allow ["a.example"]` and
  `profile = Some("researcher")` → the recorded calls include
  `policy_allow_args(name, "a.example")`.
- `teardown_ghost_sandbox_forgets_the_sandbox`: after a successful ensure, a
  teardown (canned `rm` exit 0) gives `Removed`. A following ensure runs
  preflight and create **again**.
- `run_ghost_in_sandbox_runs_in_the_sandbox_shell`: with `test_home()`, a
  fresh `ShellRegistry`, and `spawn_in_process_host` (copy it from
  `src/daemon/executor/shell_exec.rs` tests, or call it there if it is
  `pub(super)`) as the spawn, run `sh -c 'exit 4'` → `(_, 4)`. The registry
  then holds exactly one shell for the owner, with
  `sandbox == Some("de-ghost-x")`.
- `run_ghost_in_sandbox_reuses_its_shell`: two commands for the same owner
  and sandbox use one shell. The registry count for the owner stays 1.

**Reviewer checks:** (a) a mutation that makes `ensure_ghost_sandbox` skip
preflight must fail `ensure_ghost_sandbox_fails_closed_on_preflight`; (b) a
live check on pinky at review: a sandboxed local ghost's command runs in a
`de-ghost-*` sandbox, and the sandbox is gone after the ghost exits.

## End-to-end verification

```sh
cargo test --lib executor::ghost_sandbox 2>&1 | grep -E '^test result'
cargo test --lib executor::ghost_remote 2>&1 | grep -E '^test result'
cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.

## Authorizations

- [x] May add `pub(crate) mod ghost_sandbox;` to `src/daemon/executor/mod.rs`.
- [x] May edit `run_background` in `src/daemon/executor/foreground.rs` (Task 2
      only), the ghost exit path in `src/daemon/ghost.rs` (Task 3 only), and
      the PTY startup block in `src/daemon/mod.rs` (Task 4 only).
- [x] May make `spawn_in_process_host` in `shell_exec.rs` tests
      `pub(crate)` if the new tests need to reuse it.

## Out of scope

- Chat `wait: false` in a sandbox: row 07.
- Status output and egress audit: row 08.
- Deleting the docker path or `ghost_local_pty`: row 09.
- Script staging with `sbx cp` (Q3): a later row. A ghost's script runs only
  if it is already present in the sandbox; note it in the Update Log if you
  see a test need it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-07 16:56 (started, executor dispatch 1)

**Executor:** executor (M26 phase-06c, dispatch 1)

Flipped the phase and README rows to `in-progress`. Beginning Task 1: the
`ghost_sandbox` module with its tests, then the `run_background` routing,
the ghost-exit teardown and the startup sweep, then Task 5's end-to-end
capture.
