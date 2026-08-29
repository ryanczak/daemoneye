# Phase 02: Stage the invoked script into the sandbox, and retire the module `#[allow(dead_code)]`

**Milestone:** M19 — Sandbox Completion
**Status:** in-progress
**Depends on:** none (independent of phase-01)
**Estimated diff:** ~190 lines including tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`stage_args` — the argv for the root helper that copies **one approved script**
into a per-job volume — has been correct and unit-tested since M18, and
**nothing calls it**. A sandboxed background command that invokes a script
from `~/.daemoneye/scripts/` therefore fails today: the container mounts an
empty `de-stage-<job_id>` volume at `/de/scripts` and the script's host path
does not exist inside it.

This phase gives `stage_args` its caller: when a sandboxed background command
invokes a daemon-host script, the daemon stages that script into the job's
volume **before** the command runs and rewrites the command to the staged
path. It also removes the job's volume when the job completes — measured
below, every sandboxed job today leaks one — and **removes the module-level
`#[allow(dead_code)]`** from `src/daemon/executor/mod.rs`. That removal, with
clippy still green, is the phase's real acceptance test.

## Architecture references

- `docs/design/agent-container-sandboxing.md` § "D4 — Mount policy" — the
  staging design: a root helper reads the 0700 originals, copies one script,
  `chmod 0500`, `chown 1000:1000`; the agent container mounts the volume
  read-only at `/de/scripts`; **"the volume is removed with the container."**
  Steps 1–3 exist as builders; this phase implements the calls and step 4.
- `docs/design/agent-container-sandboxing.md` § "D0 — Tool disposition table"
  — *"non-sudo scripts run in-container from the ro mount; sudo scripts are
  escape-hatch (D6)"*. That sentence is why a `sudo` command is never staged
  here.
- `CLAUDE.md` § "Container sandbox" — what shipped; *"one spawn site per
  operation"* is the `container.rs` convention the two new spawn sites follow.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-29, commit `6625650`):

- `cargo test --lib` → **1458 passed; 0 failed; 4 ignored**. All four gates
  green.
- `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'` → **7**;
  `grep -c 'allow(dead_code)' src/daemon/executor/mod.rs` → **1**, at
  `src/daemon/executor/mod.rs:1-3`:

  ```rust
  // Dead until phase-06 wires the probe/preflight/staging callers.
  #[allow(dead_code)]
  pub(crate) mod container;
  ```

- **With those two lines deleted, clippy reports exactly two dead items** —
  measured by deleting them and running the gate, not by grepping:

  ```
  error: function `script_name_is_safe` is never used
     --> src/daemon/executor/container.rs:477:4
  error: function `stage_args` is never used
     --> src/daemon/executor/container.rs:490:8
  ```

  `script_name_is_safe` is called only by `stage_args`, so one production
  caller of `stage_args` retires both.

- `stage_args(cfg, job_id, script_name)` (`container.rs:490-518`) returns the
  full `docker` argv (`--host … run --rm --user 0:0 -v <scripts_dir>:/de/src:ro
  -v de-stage-<job_id>:/stage --label de.sandbox=1 <image> sh -c "cp … &&
  chmod 0500 … && chown <uid>:<gid> …"`), or an **empty** vector when the name
  fails `script_name_is_safe` or `run_as` is unparseable.
- `run_args` (`container.rs:668`) **unconditionally** mounts
  `-v de-stage-<job_id>:/de/scripts:ro`, so every sandboxed container already
  expects the volume.
- `sweep_volume_rm_args(cfg, names)` (`container.rs:560`) builds
  `--host <h> volume rm <names…>`; `stage_volume_name(job_id)` (`:473`) builds
  `de-stage-<job_id>`. Both exist; neither needs to change.
- `src/daemon/background/run.rs` builds `job_id` **inside** the sandbox block
  (`run.rs:188`, 12-space indent): `grep -c '^    let job_id' run.rs` → **0**.
- `crate::scripts::parse_script_invocation(cmd)` (`src/scripts.rs:370-388`)
  is the existing pure parser: strips one leading `sudo `, returns
  `Some((basename, args_tail))` for a relative first token whose basename is a
  valid script name, `None` for an absolute path or no token. Its production
  caller is the remote-pane foreground path, `src/daemon/executor/foreground.rs:328-370`,
  which confirms existence with `read_script` and treats a miss as an ordinary
  command:

  ```rust
  let send_cmd: &str = if is_remote_pane
      && let Some((name, args)) = crate::scripts::parse_script_invocation(cmd)
  {
      match crate::scripts::read_script(&name) {
          Ok(content) => { … }
          // Basename did not resolve to a daemon-host script — a normal remote command
          // (e.g. `ls -la`). Send it verbatim.
          Err(_) => cmd,
      }
  } else {
      cmd
  };
  ```

- `crate::scripts::resolve_script(name)` (`src/scripts.rs:69-77`) validates
  the name and errors unless `~/.daemoneye/scripts/<name>` exists.
- `crate::daemon::utils::command_has_sudo(cmd)` (`src/daemon/utils/sudo.rs:30`)
  matches `sudo` in **command position anywhere** in the line
  (`(?:^|[;&|])\s*sudo\b`), not only as a leading token.
- `SandboxConfig` derives `Clone` (`src/config/types.rs:498`).
- `tmux::off_runtime` (`src/tmux/mod.rs:30`) wraps `spawn_blocking` in a
  **5 s** `TMUX_TIMEOUT`; `bounded_output_with` (`:67`) is the project's
  bounded synchronous spawn, already imported in `container.rs`.

### Live measurements (architect, rootless Docker on the daemon host)

Run against the real `daemoneye-agent-base` image with a throwaway 0700
script `de-proto-02.sh` in `~/.daemoneye/scripts/` (removed afterwards):

1. **The volume leak is real.** `docker run --rm -v de-stage-leakprobe:/x
   alpine true` then `docker volume ls` → `de-stage-leakprobe` **present**.
   `--rm` removes anonymous volumes only. Because `run_args` always mounts
   `de-stage-<job_id>`, **every sandboxed background job today leaves one
   empty named volume behind until the next daemon start's sweep** (three such
   volumes were on the host from M18's own pilot and e2e tests).
2. **The staging helper exactly as `stage_args` builds it succeeds**
   (`stage_exit=0`), and the sandboxed run then executes the staged copy as
   the sandbox uid with its argument tail intact:

   ```
   $ docker … run --rm --user 1000:1000 --network none … -v de-stage-proto-02:/de/scripts:ro … sh -lc '/de/scripts/de-proto-02.sh --flag one; echo __EXIT=$?; ls -l /de/scripts'
   STAGED_RUN_OK uid=1000 args=--flag one
   __EXIT=0
   -r-x------ 1 de de 52 Aug 29 17:38 de-proto-02.sh
   ```

3. **A missing script fails the helper loudly**: `cp: cannot stat
   '/de/src/nope.sh': No such file or directory`, `stage_missing_exit=1`.
   Non-zero exit is the signal; stderr is the operator-facing reason.
4. `docker volume rm de-stage-proto-02` → exit 0; a second `rm` of the same
   name → exit 1 (`no such volume`). Removal is not idempotent, so it is
   best-effort and logged, never surfaced.

## Gotchas

1. **Do not run staging through `tmux::off_runtime`.** Its bound is
   `TMUX_TIMEOUT` = 5 s, sized for tmux; a docker helper on a cold host can
   exceed it and a timeout there would refuse a good command. Use
   `tokio::task::spawn_blocking` directly and let `bounded_output_with`'s own
   60 s bound inside `stage_script` be the limit. Treat a `JoinError` as a
   staging failure (fail closed), never as success.

2. **Staging must happen after the window exists, because `job_id` needs the
   pane number** — and that means a staging failure must reclaim the window
   it already created. Follow the existing `send-keys` failure branch
   (`run.rs:233-243`), minus the pipe-pane stop — pipe-pane has not started
   at the staging point:

   ```rust
   let (s5, wn5) = (session.to_string(), win_name.clone());
   let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s5, &wn5))
       .await;
   return format!("Failed to send command to window: {}", e);
   ```

3. **Check `sudo` with `command_has_sudo` *before* parsing.**
   `parse_script_invocation` silently strips a leading `sudo `, so `sudo
   myscript.sh` parses as `myscript.sh` and would be staged and rewritten to
   `/de/scripts/myscript.sh` — dropping the `sudo` and running a script the
   operator marked privileged as the sandbox uid. D0 says sudo scripts are the
   escape hatch's business (phase-09); this phase must leave them exactly as
   they are today. Mutation M2 pins this.

4. **The existence predicate is injected, not called.** `resolve_script` hits
   the filesystem; `sandbox_script_invocation` takes an
   `impl Fn(&str) -> bool` so its tests are hermetic (STANDARDS § 3.3). The
   production closure is `|n| crate::scripts::resolve_script(n).is_ok()`.
   `ls -la` **parses** as a candidate (`ls` is a valid script name) — only the
   predicate makes it an ordinary command. Mutation M1 pins this.

5. **An absolute path is not a script invocation.** `parse_script_invocation`
   returns `None` for `/home/op/.daemoneye/scripts/x.sh`, and the foreground
   remote path inherits that. Keep parity; do not add a special case for
   absolute paths under `scripts_dir()` — that is a contract change to a
   shared parser and belongs to no phase in this milestone.

6. **Clippy `-D warnings` rejects `let mut cfg = SandboxConfig::default();
   cfg.runtime = …`** (`field_reassign_with_default`). Measured on the
   prototype. Build the test config with struct-update syntax:

   ```rust
   let cfg = SandboxConfig {
       runtime: "/nonexistent/de-runtime".to_string(),
       ..Default::default()
   };
   ```

7. **`stage_args` itself does not change.** Three M18 phases each moved its
   pinned slice; this phase calls it and leaves its vector alone. If an
   existing `stage_args` test needs editing, stop and record a blocker.

8. **The volume is removed on both completion paths.** `run.rs` has two
   `job_complete` sites — the ≤ 3 s inline path and the spawned slow path.
   Both must call `remove_stage_volume`; the slow path's `tokio::spawn` needs
   its own clones of `config.sandbox` and `job_id` moved in.

## Spec

### Task 1 — Add the four staging functions to `src/daemon/executor/container.rs`

Insert them directly after `stage_args` (before the doc comment of
`sweep_container_list_args`). They are the exact prototype, and they are all
the new production code this file gets:

```rust
/// The daemon-host script a sandboxed background command invokes, if any,
/// as `(name, args_tail)` from [`crate::scripts::parse_script_invocation`].
///
/// Pure: `script_exists` answers whether `~/.daemoneye/scripts/<name>` is a
/// real script (production passes `resolve_script(..).is_ok()`; tests pass a
/// closure). `ls -la` parses as a candidate but does not exist, so it is an
/// ordinary command. A command under `sudo` is never staged — sudo inside the
/// sandbox is the escape hatch's business, not staging's.
pub fn sandbox_script_invocation(
    cmd: &str,
    script_exists: impl Fn(&str) -> bool,
) -> Option<(String, String)> {
    if crate::daemon::utils::command_has_sudo(cmd) {
        return None;
    }
    let (name, args_tail) = crate::scripts::parse_script_invocation(cmd)?;
    if !script_exists(&name) {
        return None;
    }
    Some((name, args_tail))
}

/// The in-container command for a staged script: its path under the
/// `/de/scripts` mount `run_args` provides, then the verbatim argument tail.
pub fn staged_script_command(script_name: &str, args_tail: &str) -> String {
    format!("/de/scripts/{script_name}{args_tail}")
}

/// Stage one script into this job's volume by spawning the helper
/// [`stage_args`] describes. Blocking — call it off the async runtime.
/// Fails closed: every error is an operator-facing reason and the caller
/// must not run the command.
pub fn stage_script(cfg: &SandboxConfig, job_id: &str, script_name: &str) -> Result<(), String> {
    let args = stage_args(cfg, job_id, script_name);
    if args.is_empty() {
        return Err(format!(
            "sandbox staging refused: `{script_name}` is not a stageable script name"
        ));
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(args);
    match bounded_output_with(&mut cmd, Duration::from_secs(60)) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "sandbox staging failed for `{script_name}`: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("sandbox staging failed for `{script_name}`: {e}")),
    }
}

/// Remove this job's staging volume once the job is over. Best-effort: a
/// failure is logged, never surfaced — the startup sweep reclaims leftovers.
pub fn remove_stage_volume(cfg: &SandboxConfig, job_id: &str) {
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_volume_rm_args(cfg, &[stage_volume_name(job_id)]));
    if let Err(e) = bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        log::warn!("sandbox stage volume remove failed for job {job_id}: {e}");
    }
}
```

`Command`, `Duration` and `bounded_output_with` are already imported at the
top of the file.

### Task 2 — Hoist `job_id` in `src/daemon/background/run.rs`

Immediately after `let pane_num = pane_id.trim_start_matches('%');`
(`run.rs:97`) add:

```rust
let job_id = format!("{pane_num}-{unix_ts}");
```

and **delete** the identical `let job_id = …` line inside the
`if config.sandbox.enabled` block (`run.rs:188`), so the `ExecSpec` there
borrows the hoisted binding. The value is unchanged — `run_args` must mount
the same `de-stage-<job_id>` the helper staged into.

### Task 3 — Stage and rewrite, in `src/daemon/background/run.rs`

Insert this block immediately **before** `let sandboxed_cmd;` (`run.rs:185`),
i.e. after the sudo-sentinel rebinding of `cmd` and before the sandbox
wrapping:

```rust
// Stage the daemon-host script this command invokes, if any, into the
// job's volume and point the command at the staged copy. Fails closed: a
// command whose script cannot be staged is refused and its window reclaimed.
let staged_cmd;
let cmd: &str = if config.sandbox.enabled
    && let Some((name, args_tail)) =
        crate::daemon::executor::container::sandbox_script_invocation(cmd, |n| {
            crate::scripts::resolve_script(n).is_ok()
        }) {
    let (cfg_s, job_s, name_s) = (config.sandbox.clone(), job_id.clone(), name.clone());
    let staged = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::stage_script(&cfg_s, &job_s, &name_s)
    })
    .await
    .unwrap_or_else(|e| Err(format!("sandbox staging task failed: {e}")));
    if let Err(message) = staged {
        log::warn!("refusing sandboxed background command: {message}");
        let (s5, wn5) = (session.to_string(), win_name.clone());
        let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s5, &wn5))
            .await;
        return message;
    }
    staged_cmd = crate::daemon::executor::container::staged_script_command(&name, &args_tail);
    &staged_cmd
} else {
    cmd
};
```

With the sandbox disabled this block is a no-op and `cmd` is untouched —
byte-for-byte today's behaviour, per the M18 invariant.

### Task 4 — Remove the volume at both completion sites, in `src/daemon/background/run.rs`

**Inline (fast) path** — directly after the `capture_and_archive` call's
`.unwrap_or_default();` and before the `job_complete` `log_event`:

```rust
if config.sandbox.enabled {
    let (cfg_v, job_v) = (config.sandbox.clone(), job_id.clone());
    tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::remove_stage_volume(&cfg_v, &job_v)
    });
}
```

**Slow path** — add two clones beside the existing ones that feed the
`tokio::spawn`:

```rust
let sessions_bg = sessions.clone();
let sandbox_bg = config.sandbox.clone();
let job_id_bg = job_id.clone();
```

and, inside the spawned task, directly after its `capture_and_archive`
call's `.unwrap_or_default();`:

```rust
if sandbox_bg.enabled {
    tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::remove_stage_volume(
            &sandbox_bg,
            &job_id_bg,
        )
    });
}
```

The removal is fire-and-forget by design: the job's result must not wait on
docker, and `remove_stage_volume` already logs its own failure.

### Task 5 — Remove the module `#[allow(dead_code)]` from `src/daemon/executor/mod.rs`

Delete lines 1–2 — the `// Dead until phase-06 …` comment **and** the
`#[allow(dead_code)]` attribute — leaving `pub(crate) mod container;` as the
first line. Then run
`cargo clippy --all-targets --all-features -- -D warnings`. It must be green:
Task 1's `stage_script` is the caller that retires both dead items named in
§ Current state. If clippy names any *other* dead item, that is a spec defect
— record a blocker naming it; do not add an `#[allow]` anywhere.

### Task 6 — Tests in `container.rs`'s existing `mod tests`

Six tests, named exactly as below, appended at the end of the module. Every
name contains `sandbox_staging`. Each is given in full; they are the
prototype's tests and pass on the prototype tree.

```rust
#[test]
fn sandbox_staging_detects_a_script_the_predicate_knows() {
    let known = |n: &str| n == "myscript.sh";
    assert_eq!(
        sandbox_script_invocation("myscript.sh --flag one", known),
        Some(("myscript.sh".to_string(), " --flag one".to_string()))
    );
    assert_eq!(
        sandbox_script_invocation("~/.daemoneye/scripts/myscript.sh", known),
        Some(("myscript.sh".to_string(), String::new()))
    );
}

#[test]
fn sandbox_staging_ignores_commands_that_are_not_scripts() {
    let known = |n: &str| n == "myscript.sh";
    assert_eq!(
        sandbox_script_invocation("ls -la", known),
        None,
        "a basename that is not a script is an ordinary command"
    );
    assert_eq!(
        sandbox_script_invocation("myscript.sh", |_| false),
        None,
        "the predicate is the authority, not the name shape"
    );
    assert_eq!(
        sandbox_script_invocation("/home/op/.daemoneye/scripts/myscript.sh", |_| true),
        None,
        "an absolute path is never a script invocation (foreground parity)"
    );
    assert_eq!(
        sandbox_script_invocation("", |_| true),
        None,
        "empty command"
    );
}

#[test]
fn sandbox_staging_never_stages_under_sudo() {
    assert_eq!(
        sandbox_script_invocation("sudo myscript.sh", |_| true),
        None,
        "leading sudo"
    );
    assert_eq!(
        sandbox_script_invocation("myscript.sh && sudo reboot", |_| true),
        None,
        "sudo later in the line"
    );
}

#[test]
fn sandbox_staging_rewrites_to_the_staged_path() {
    assert_eq!(
        staged_script_command("myscript.sh", " --flag one"),
        "/de/scripts/myscript.sh --flag one"
    );
    assert_eq!(
        staged_script_command("myscript.sh", ""),
        "/de/scripts/myscript.sh"
    );
}

#[test]
fn sandbox_staging_refuses_unstageable_names_without_spawning() {
    let cfg = SandboxConfig {
        runtime: "/nonexistent/de-runtime".to_string(),
        ..Default::default()
    };
    let err = stage_script(&cfg, "j1", "../etc/passwd").expect_err("refused");
    assert!(err.contains("not a stageable script name"), "got: {err}");
}

#[test]
fn sandbox_staging_reports_a_helper_that_cannot_run() {
    let cfg = SandboxConfig {
        runtime: "/nonexistent/de-runtime".to_string(),
        ..Default::default()
    };
    let err = stage_script(&cfg, "j1", "myscript.sh").expect_err("spawn fails");
    assert!(
        err.starts_with("sandbox staging failed for `myscript.sh`"),
        "got: {err}"
    );
}
```

The last two are hermetic by construction: the first returns before any
spawn, the second spawns a path that does not exist and so exercises the
`Err(e)` arm without a runtime. Neither touches docker.

### Task 7 — Mutation pair M1: the existence guard is real

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them.** Append each marker and run to `/tmp/e2e-02.txt`. Run the gates
(Task 9's block) only **after** both pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `    if !script_exists(&name) {`
   - `new_str`: `    if false {`

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-02.txt
   cargo test --lib sandbox_staging 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-02.txt
   grep -c "^    if false {$" src/daemon/executor/container.rs >> /tmp/e2e-02.txt
   ```
   The result must show **1 failed** and name
   `sandbox_staging_ignores_commands_that_are_not_scripts`. A mutation that
   leaves the suite green means the guard is vacuous — record a blocker.

2. **Restore.** The inverse `patch` (`old_str: "    if false {"` →
   `new_str: "    if !script_exists(&name) {"`), then:
   ```sh
   echo "== M1 RESTORED ==" >> /tmp/e2e-02.txt
   cargo test --lib sandbox_staging 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-02.txt
   grep -c "^    if !script_exists(&name) {$" src/daemon/executor/container.rs >> /tmp/e2e-02.txt
   ```
   Now the tests pass and the `grep -c` prints `1`.

### Task 8 — Mutation pair M2: the sudo guard is real

Only after M1 is restored (the `    if false {` line must be unique).

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `    if crate::daemon::utils::command_has_sudo(cmd) {`
   - `new_str`: `    if false {`

   Then:
   ```sh
   echo "== M2 APPLIED ==" >> /tmp/e2e-02.txt
   cargo test --lib sandbox_staging 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-02.txt
   grep -c "^    if false {$" src/daemon/executor/container.rs >> /tmp/e2e-02.txt
   ```
   The result must show **1 failed** and name
   `sandbox_staging_never_stages_under_sudo`.

2. **Restore.** The inverse `patch` (`old_str: "    if false {"` →
   `new_str: "    if crate::daemon::utils::command_has_sudo(cmd) {"`), then:
   ```sh
   echo "== M2 RESTORED ==" >> /tmp/e2e-02.txt
   cargo test --lib sandbox_staging 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-02.txt
   grep -c "^    if crate::daemon::utils::command_has_sudo(cmd) {$" src/daemon/executor/container.rs >> /tmp/e2e-02.txt
   ```

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the *wrong* line fails silently, and a mutation that never
applied certifies a vacuous guard.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-02.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Every count below was measured against the prototype tree while drafting —
the tree this phase produces, not the one in front of you.

- [ ] `grep -c 'allow(dead_code)' src/daemon/executor/mod.rs` prints `0`
      (**before: 1**), and
      `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**before: 7**) — the milestone exit criterion.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is green
      **with** that attribute gone.
- [ ] `grep -c 'fn sandbox_script_invocation(' src/daemon/executor/container.rs`,
      `grep -c 'fn staged_script_command(' …`, `grep -c 'fn stage_script(' …`
      and `grep -c 'fn remove_stage_volume(' …` each print `1` (**before: 0**).
      The `(` is load-bearing — without it the test names match too.
- [ ] `grep -c 'container::sandbox_script_invocation(' src/daemon/background/run.rs`
      prints `1` and `grep -c 'container::stage_script(' …` prints `1`
      (**before: 0, 0**).
- [ ] `grep -c 'container::remove_stage_volume(' src/daemon/background/run.rs`
      prints `2` (**before: 0**) — one per completion path.
- [ ] `grep -c '^    let job_id' src/daemon/background/run.rs` prints `1`
      (**before: 0**) — hoisted to function scope, 4-space indent; and
      `grep -c 'let job_id = format!' …` prints `1` (**unchanged**) — the
      inner copy is gone, not duplicated. (A bare `let job_id` would count
      `let job_id_bg` too and read `2` on a correct tree.)
- [ ] `grep -c 'off_runtime("sandbox' src/daemon/background/run.rs` prints
      `0` — staging does not go through the 5 s tmux bound (§ Gotchas 1).
- [ ] `cargo test --lib sandbox_staging 2>&1 | grep -c "^test .* ok$"` prints
      `6`. A count, not an exit status.
- [ ] `cargo test --lib` reports **at least 1464** passing and `0 failed`
      (**before: 1458**), with `4 ignored` unchanged.
- [ ] `grep -c "#\[ignore" src/daemon/executor/container.rs` prints `4`
      (**unchanged**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0` — no new panicking idiom in production code (**before: 0**).
- [ ] The § End-to-end entry shows `== M1 APPLIED ==` and `== M2 APPLIED ==`
      each **failing exactly one** named test, both `RESTORED` runs passing,
      with a `grep -c` line after each direction.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks).

## Test plan

Six unit tests in `container.rs`'s existing `mod tests`, given in full in
Task 6. No new test file.

**The negative cases are the point.** `sandbox_staging_ignores_commands_that_are_not_scripts`
pins that `ls -la` — a *valid script name* by shape — is an ordinary command
unless the predicate says otherwise, and that the predicate wins even over a
script-looking name; M1 proves that guard is live.
`sandbox_staging_never_stages_under_sudo` pins § Gotchas 3 in **both**
positions `command_has_sudo` covers; M2 proves it. The absolute-path case pins
§ Gotchas 5 so nobody "improves" the shared parser from this side.

`stage_script`'s two tests cover its two non-docker arms — refused name, and a
runtime that cannot spawn — without a container runtime. The success arm and
`remove_stage_volume` spawn `docker` and are verified live by the architect
(§ Current state) and again at milestone close; they are not unit-tested,
matching how `sweep_sandbox_leftovers` is treated.

Behaviour is unchanged with the sandbox disabled, so no existing test should
need editing. **If an existing test requires a change to pass, stop and
record a blocker** — in particular any `stage_args` test (§ Gotchas 7).

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 7 and 8 have
appended their mutation markers to `/tmp/e2e-02.txt` and both pairs are
restored.

```sh
{
echo "== A. named tests (expect 6 ok) =="
cargo test --lib sandbox_staging 2>&1 | grep -E "^test |^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. full lib suite =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== D. structural greps =="
echo -n "mod.rs allow (0):            "; grep -c 'allow(dead_code)' src/daemon/executor/mod.rs
echo -n "allow total (6):             "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "sandbox_script_invocation (1): "; grep -c 'fn sandbox_script_invocation(' src/daemon/executor/container.rs
echo -n "staged_script_command (1):   "; grep -c 'fn staged_script_command(' src/daemon/executor/container.rs
echo -n "stage_script (1):            "; grep -c 'fn stage_script(' src/daemon/executor/container.rs
echo -n "remove_stage_volume (1):     "; grep -c 'fn remove_stage_volume(' src/daemon/executor/container.rs
echo -n "run.rs invocation call (1):  "; grep -c 'container::sandbox_script_invocation(' src/daemon/background/run.rs
echo -n "run.rs stage call (1):       "; grep -c 'container::stage_script(' src/daemon/background/run.rs
echo -n "run.rs remove calls (2):     "; grep -c 'container::remove_stage_volume(' src/daemon/background/run.rs
echo -n "job_id hoisted (1):          "; grep -c '^    let job_id' src/daemon/background/run.rs
echo -n "job_id = format! (1):        "; grep -c 'let job_id = format!' src/daemon/background/run.rs
echo -n "no off_runtime staging (0):  "; grep -c 'off_runtime("sandbox' src/daemon/background/run.rs
echo -n "ignore count (4):            "; grep -c "#\[ignore" src/daemon/executor/container.rs
echo -n "prod unwrap/expect (0):      "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('
} >> /tmp/e2e-02.txt 2>&1
cat /tmp/e2e-02.txt
```

Paste the whole of `/tmp/e2e-02.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-02-staging-integration.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02.txt
diff /tmp/pasted-02.txt /tmp/e2e-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit `src/daemon/executor/container.rs`, `src/daemon/background/run.rs`
  and `src/daemon/executor/mod.rs` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Removing the module `#[allow(dead_code)]` in `executor/mod.rs` is
  required** (Task 5). No other `#[allow(...)]` may be added or removed, and
  no `#[ignore]` may be added or removed.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The staging chain was measured by
  the architect (§ Current state) and is re-verified at milestone close.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it would discard this round's own uncommitted work.
- **Do not edit any other source file, and do not edit any doc other than this
  phase doc's Update Log.**
- **Append to the Update Log; never edit or delete an existing entry.**
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  clippy names a dead item this phase did not retire, a mutation leaves the
  suite green, *or* a gate is red for a reason this phase did not cause —
  record a blocker Update Log entry naming the exact criterion, and stop.
  Reporting the blocker *is* the successful outcome.** Do not proceed past a
  blocker you have filed.
- **Record what you decide, not what you wish had been decided.**

## Out of scope

- **Scheduled script jobs** (`ActionOn::Script`, `de-sj-*` windows in
  `src/daemon/scheduled.rs`) do not go through `run_background_in_window` and
  are **not** sandboxed or staged by this phase — they run the script's host
  path in a host shell exactly as today. Recorded in the milestone README as
  a gap against D0's "scheduled commands execute sandboxed when they fire".
- **Ghost shells** — phase 03 routes them through the sandbox; this phase's
  staging applies to whatever reaches `run_background_in_window`.
- **`sudo` scripts inside the sandbox** — the escape hatch, phase 09.
- Changing `stage_args`, `run_args`, `parse_script_invocation` or
  `command_has_sudo`. All four are called, none is edited.
- Accepting absolute paths under `scripts_dir()` as script invocations
  (§ Gotchas 5).
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep. The
  `README.md` line saying the approved script *"is staged in, read-only"*
  becomes true with this phase rather than false, so nothing there needs
  correcting now.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-08-29 18:05 (started)

Executing phase-02 per the spec: four staging functions in `container.rs`,
`job_id` hoist + stage/rewrite + remove-volume wiring in `run.rs`,
`#[allow(dead_code)]` removal in `executor/mod.rs`, six unit tests, mutation
pairs M1/M2, and the end-to-end evidence capture.

**Executor:** rexyMCP executor (session 2026-08-29)

### Update — 2026-08-29 18:25 (end-to-end verification)

The full transcript, mutation markers included. Section A through D ran
verbatim from the repo root after both mutation pairs were restored.

```text
== M1 APPLIED ==
test daemon::executor::container::tests::sandbox_staging_ignores_commands_that_are_not_scripts ... FAILED
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 1462 filtered out; finished in 0.00s
1
== M1 RESTORED ==
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1462 filtered out; finished in 0.00s
1
== M2 APPLIED ==
test daemon::executor::container::tests::sandbox_staging_never_stages_under_sudo ... FAILED
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 1462 filtered out; finished in 0.00s
1
== M2 RESTORED ==
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1462 filtered out; finished in 0.00s
1
== A. named tests (expect 6 ok) ==
test daemon::executor::container::tests::sandbox_staging_rewrites_to_the_staged_path ... ok
test daemon::executor::container::tests::sandbox_staging_refuses_unstageable_names_without_spawning ... ok
test daemon::executor::container::tests::sandbox_staging_reports_a_helper_that_cannot_run ... ok
test daemon::executor::container::tests::sandbox_staging_ignores_commands_that_are_not_scripts ... ok
test daemon::executor::container::tests::sandbox_staging_detects_a_script_the_predicate_knows ... ok
test daemon::executor::container::tests::sandbox_staging_never_stages_under_sudo ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1462 filtered out; finished in 0.00s
cargo_exit=0
== B. full lib suite ==
test result: ok. 1464 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 4.07s
cargo_exit=0
== C. gates ==
fmt_exit=0
clippy_exit=0
== D. structural greps ==
mod.rs allow (0):            0
allow total (6):             6
sandbox_script_invocation (1): 1
staged_script_command (1):   1
stage_script (1):            1
remove_stage_volume (1):     1
run.rs invocation call (1):  1
run.rs stage call (1):       1
run.rs remove calls (2):     2
job_id hoisted (1):          1
job_id = format! (1):        1
no off_runtime staging (0):  0
ignore count (4):            4
prod unwrap/expect (0):      0
```
