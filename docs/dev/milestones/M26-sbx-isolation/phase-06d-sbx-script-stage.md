# Phase 06d: sbx-script-stage — copy the one approved script into the ghost's sandbox

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-06c
**Estimated diff:** ~300 lines (about 90 code, 210 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Q3, decided at scope sign-off: a sandboxed ghost that invokes an approved
script from `~/.daemoneye/scripts/` gets **that one script** copied into its
sandbox with `sbx cp`, and the command is rewritten to run the copy. The
scripts directory is never mounted. Today a sandboxed ghost's script
invocation fails, because the script does not exist inside the VM.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Decisions on the open
  questions", Q3.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Measured on pinky, 2026-10-07 (architect probe, sandbox removed)

| Step | Result |
|---|---|
| `sbx exec <n> mkdir -p /de/scripts` as the default `agent` user | exit 1: the agent cannot create `/de` |
| `sbx exec -u root <n> sh -c 'mkdir -p /de/scripts && chown agent:agent /de/scripts'` | exit 0, and exit 0 again (idempotent) |
| `sbx cp <host>/s.sh <n>:/de/scripts/s.sh` | exit 0; `stat` → `700 agent`; running it prints `v1 a b` |
| a second `sbx cp` after editing the host file | exit 0, and the copy is **overwritten** (`v2 a`) |
| `sbx cp <missing> <n>:/de/scripts/x.sh` | exit 1, stderr `error: resolve source path: lstat …: no such file or directory` |
| the agent overwriting `/de/scripts/s.sh` from inside | succeeds |

The last row is expected, because the VM is the boundary and the agent has
full control inside it. It is the reason to **stage on every invocation**
rather than once: the ghost always runs the operator's approved copy.

### The docker path's pure helpers, to port (`src/daemon/executor/container.rs`)

`container.rs` is deleted in row 09, so sbx gets its own copies in
`src/daemon/executor/sbx.rs`. Do not call `container.rs`.

```rust
fn script_name_is_safe(script_name: &str) -> bool {
    if script_name.is_empty() || script_name.contains("..") {
        return false;
    }
    script_name.chars().all(|c| {
        !c.is_whitespace() && !matches!(c, '/' | ';' | '&' | '|' | '$' | '`' | '\'' | '"' | '\n')
    })
}

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

pub fn staged_script_command(script_name: &str, args_tail: &str) -> String {
    format!("/de/scripts/{script_name}{args_tail}")
}
```

Production passes `|n| crate::scripts::resolve_script(n).is_ok()` as
`script_exists`. The host path of a script is
`crate::config::scripts_dir().join(name)`.

### Where the sandboxed ghost runs its command (`run_background`, from row 06c)

```rust
let (output, exit_code) = match super::ghost_sandbox::ensure_ghost_sandbox(
    &run, &sandbox_cfg, &sid, profile.as_deref(),
).await {
    Ok(sandbox) => {
        let spawn_sandbox = sandbox.clone();
        super::ghost_sandbox::run_ghost_in_sandbox(
            crate::shell::registry(),
            crate::shell::Owner::Ghost(sid.clone()),
            &sandbox,
            cmd,
            id,
            move |shell_id| { … spawn_sbx_host … },
        ).await
    }
    Err(reason) => (reason, 1),
};
```

### The fake runner

Copy the one in `src/daemon/executor/ghost_sandbox.rs` tests (it records each
argv and returns canned `VmRun`s in order).

## Spec

### Task 1 — Pure helpers in `src/daemon/executor/sbx.rs`

Add, all `pub`:

```rust
/// Where staged scripts live inside a sandbox.
pub const STAGED_SCRIPTS_DIR: &str = "/de/scripts";

/// A script name safe to put on a path and a command line: no `..`, no
/// whitespace, none of `/ ; & | $ \` ' " \n`.
pub fn script_name_is_safe(script_name: &str) -> bool          // copy of the docker one

/// The approved script `cmd` invokes, as `(name, args_tail)`; never under sudo.
pub fn script_invocation(cmd: &str, script_exists: impl Fn(&str) -> bool) -> Option<(String, String)>
                                                                 // copy of sandbox_script_invocation

/// `/de/scripts/<name><args_tail>`.
pub fn staged_script_command(script_name: &str, args_tail: &str) -> String

/// `exec -u root <sandbox> sh -c "mkdir -p /de/scripts && chown agent:agent /de/scripts"`.
pub fn stage_prep_args(sandbox: &str) -> Vec<String>

/// `cp <host_path> <sandbox>:/de/scripts/<name>`.
pub fn stage_cp_args(host_path: &str, sandbox: &str, script_name: &str) -> Vec<String>
```

The exact argv:
- `stage_prep_args("de-ghost-r1")` ==
  `["exec", "-u", "root", "de-ghost-r1", "sh", "-c", "mkdir -p /de/scripts && chown agent:agent /de/scripts"]`;
- `stage_cp_args("/h/s.sh", "de-ghost-r1", "s.sh")` ==
  `["cp", "/h/s.sh", "de-ghost-r1:/de/scripts/s.sh"]`.

Build both strings from `STAGED_SCRIPTS_DIR`.

### Task 2 — `stage_script` in `src/daemon/executor/ghost_sandbox.rs`

```rust
/// Copy the approved script `name` into `sandbox`, every time it is invoked
/// (the agent can overwrite its copy, so the operator's version is re-staged).
/// Fails closed: `Err` is an operator-facing reason, and the caller must not
/// run the command.
pub async fn stage_script(run: &crate::shell::SbxRunner, sandbox: &str, name: &str, host_path: &str) -> Result<(), String>
```

1. If `!sbx::script_name_is_safe(name)` → `Err` saying the script name is not
   stageable. **No `sbx` call.**
2. Run `sbx::stage_prep_args(sandbox)`. Exit not `Some(0)` → `Err`
   containing the trimmed stderr.
3. Run `sbx::stage_cp_args(host_path, sandbox, name)`. Exit not `Some(0)` →
   `Err` containing the trimmed stderr.
4. `Ok(())`.

Start every `Err` message with `sandbox staging failed for `{name}`: `.

### Task 3 — Stage in `run_background` before running (`foreground.rs`)

Inside the `Ok(sandbox) =>` arm from 06c, before `run_ghost_in_sandbox`:

```rust
let staged_cmd;
let cmd_to_run: &str = match crate::daemon::executor::sbx::script_invocation(cmd, |n| {
    crate::scripts::resolve_script(n).is_ok()
}) {
    Some((name, args_tail)) => {
        let host_path = crate::config::scripts_dir().join(&name);
        if let Err(reason) = super::ghost_sandbox::stage_script(
            &run, &sandbox, &name, &host_path.to_string_lossy(),
        ).await {
            // fail closed: the command is not run
            … the arm evaluates to (reason, 1) …
        }
        staged_cmd = crate::daemon::executor::sbx::staged_script_command(&name, &args_tail);
        &staged_cmd
    }
    None => cmd,
};
```

Then pass `cmd_to_run` (not `cmd`) to `run_ghost_in_sandbox`. Shape it so a
staging failure makes the whole arm yield `(reason, 1)`; an `if let … else`
or a small helper both work. Keep `log_command(…, cmd, …)` logging the
**original** command, so the operator sees what the model asked for.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] Each of these eight tests reports `1 passed; 0 failed` when run alone
      (`cargo test --lib <name>`):
      `staged_script_command_points_at_the_sandbox_copy`,
      `stage_args_match_the_measured_shapes`,
      `script_name_is_safe_rejects_path_and_shell_characters`,
      `script_invocation_skips_sudo_and_unknown_scripts`,
      `stage_script_preps_then_copies`,
      `stage_script_refuses_an_unsafe_name_without_sbx`,
      `stage_script_fails_closed_on_a_failed_copy`,
      `stage_script_fails_closed_on_a_failed_prep`.
- [ ] `cargo test --lib executor::sbx` and `cargo test --lib executor::ghost_sandbox`
      report `0 failed`, with no existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code. `container.rs` is unchanged.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/daemon/executor/sbx.rs` tests:

- `staged_script_command_points_at_the_sandbox_copy`:
  `("restart.sh", " --now")` → `/de/scripts/restart.sh --now`; `("a.sh", "")`
  → `/de/scripts/a.sh`.
- `stage_args_match_the_measured_shapes`: the two exact vectors in Task 1.
- `script_name_is_safe_rejects_path_and_shell_characters`: `"ok.sh"` and
  `"backup-db_2.sh"` are safe; `""`, `"../x"`, `"a/b"`, `"a b"`, `"a;b"`,
  `"$(x)"`, ``"a`b"``, `"a'b"` and `"a\"b"` are not.
- `script_invocation_skips_sudo_and_unknown_scripts`: with
  `exists = |n| n == "restart.sh"`, an invocation of `restart.sh` with args →
  `Some(("restart.sh", " --now"))` for the command `"restart.sh --now"`. The
  accepted shape is a bare script name followed by its arguments, and the tail
  keeps its leading space; `parse_script_invocation`'s own test shows it as
  `parse_script_invocation("foo.sh --flag arg")` ==
  `Some(("foo.sh", " --flag arg"))`. `sudo restart.sh --now`
  → `None`; an unknown script → `None`; `ls -la` → `None`.

In `src/daemon/executor/ghost_sandbox.rs` tests (fake runner):

- `stage_script_preps_then_copies`: both canned exit 0 → `Ok`. The calls are
  exactly `stage_prep_args("de-ghost-t")` then
  `stage_cp_args("/h/s.sh", "de-ghost-t", "s.sh")`.
- `stage_script_refuses_an_unsafe_name_without_sbx`: `"../evil"` → `Err`,
  with **zero** calls.
- `stage_script_fails_closed_on_a_failed_copy`: prep exit 0, cp exit 1 with
  stderr `error: resolve source path: lstat /h/s.sh: no such file or directory`
  → `Err` starting with ``sandbox staging failed for `s.sh`: `` and containing
  `no such file`.
- `stage_script_fails_closed_on_a_failed_prep`: prep exit 1 → `Err`, and
  **no** cp call.

**Reviewer mutation:** make `stage_script` skip the prep call, so
`stage_script_preps_then_copies` fails.

## End-to-end verification

```sh
for t in staged_script_command_points_at_the_sandbox_copy stage_args_match_the_measured_shapes script_name_is_safe_rejects_path_and_shell_characters script_invocation_skips_sudo_and_unknown_scripts stage_script_preps_then_copies stage_script_refuses_an_unsafe_name_without_sbx stage_script_fails_closed_on_a_failed_copy stage_script_fails_closed_on_a_failed_prep; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
cargo test --lib executor::ghost_sandbox 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.

## Authorizations

- [x] May edit the `Ok(sandbox) =>` arm of the 06c branch in `run_background`
      (`src/daemon/executor/foreground.rs`), **only** as Task 3 says.

## Out of scope

- Scripts under `sudo` inside the sandbox (the escape hatch's business).
- Removing a staged script after the command (the sandbox dies with the run).
- Chat `wait: false`: row 07.
- `container.rs` (deleted in row 09).

## Update Log

<!-- entries appended below this line -->
