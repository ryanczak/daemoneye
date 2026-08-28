# Phase 05: Route background commands through the sandbox

**Milestone:** M18 — Container-sandboxed Agents
**Status:** todo
**Depends on:** phase-04 (`run_args`, `ExecSpec`), phase-01 (`SandboxConfig.enabled`)
**Estimated diff:** ~330 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Wire phase-04's argv into the background execution path: when
`[sandbox] enabled = true`, the command a `de-bg-*` window runs becomes a
`docker run …` invocation instead of the raw user command. **This is the first
phase in M18 whose code actually starts a container.** The `de-bg-*` window,
completion detection, output capture and GC are untouched — only the string
that goes into the window changes.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D3 — Container lifecycle",
  the **Background-window integration** bullet: the `de-bg-*` model is kept
  deliberately, so the user can still watch a sandboxed job in a pane.
- `docs/design/agent-container-sandboxing.md` § "D0 — Tool disposition table":
  only `run_terminal_command` **background** mode is in scope. Foreground
  (`send-keys` into the user's pane) is host-level by design and must not
  change.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-28, commit `8beca06`):

- `cargo test --lib` → **1426 passed; 0 failed; 1 ignored**. Four gates green.
- `src/daemon/executor/container.rs` provides `run_args(&SandboxConfig,
  &ExecSpec) -> Vec<String>` and `ExecSpec { job_id, network, is_ghost,
  command }` from phase-04. `run_args` returns a vector **beginning with
  `"run"`** — the runtime binary is *not* included and the caller prepends it.
- `src/daemon/executor/mod.rs:1-4` still carries `#[allow(dead_code)]` on
  `pub(crate) mod container;`. **This phase removes it** — it adds the first
  production caller. Repo-wide `allow(dead_code)` goes **7 → 6**; removing it
  was measured to leave the count at 6 with the module still compiling.
- `cargo test --lib sandbox_window` → **0** test lines (the vacuity trap).

### The wrapping seam (this is where your change goes)

`src/daemon/background/run.rs:159-172`. `cmd` is rebound once for the sudo
sentinel, then wrapped with the completion notifier:

```rust
    let sentineled_cmd;
    let cmd: &str = if command_has_sudo(cmd) && credential.is_some() {
        sentineled_cmd = format!("SUDO_PROMPT='[de-sudo-prompt]' {cmd}");
        &sentineled_cmd
    } else {
        cmd
    };

    let wrapped = if exit_var == "$status" {
        // fish: use set to capture status before running notify
        format!("{cmd}; set __de_ec $status; {notify}")
    } else {
        // bash / zsh / sh / ksh / dash / ...
        format!("{cmd}; __de_ec=$?; {notify}")
    };
```

Your sandbox rebinding goes **between those two blocks** — after the sudo
sentinel, before `wrapped`. That ordering is load-bearing: `$__de_ec` must
capture the **exit status of `docker run`**, which is the container's exit
code, so completion detection and the archived exit code keep working with no
other change.

`pane_num` and `unix_ts` are defined at `run.rs:78-81` and are still live at
the seam (`format!` borrows, it does not move), so
`format!("{pane_num}-{unix_ts}")` is available as the job id.

### The quoting helper — use `sh_single_quote`, not `shell_escape_arg`

`src/daemon/utils/shell.rs:27`, and its own doc comment says which to use:

```rust
/// Single-quote an arbitrary string so a POSIX shell parses it as one literal
/// token. Wraps in `'…'` and rewrites each embedded `'` as `'\''`. Use this
/// (NOT `shell_escape_arg`) whenever a value is placed inside single quotes —
/// e.g. building an `ssh <host> <cmd>` invocation.
pub fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
```

`shell_escape_arg` (same file, line 15) is for the tmux `send-keys` layer and
is **not** a shell-quoter. Using it here would leave `;` and `&&` live.

## Gotchas

Six traps. Items 1–3 were measured on this host; the executor has no runtime.

1. **The whole point of this phase is that the user's command must not reach
   the host shell.** Prototyped before this spec was written, using the exact
   `sh_single_quote` algorithm above and the hostile command
   `echo inside-container; touch /tmp/PWNED`:

   ```
   … 'daemoneye-agent-base' 'sh' '-lc' 'echo inside-container; touch /tmp/PWNED'
   $ sh /tmp/wincmd.sh
   inside-container
   exit=0
   did the host get touched? NO — command stayed inside the container
   ```

   Every argv element is separately single-quoted and joined with spaces. A
   test pins exactly this: the `;` must survive **inside** the final quoted
   token, not as a shell separator.

2. **Do not pre-create the staging volume.** Measured: `docker run -v
   de-stage-x:/de/scripts:ro …` auto-creates the named volume even when the
   mount is read-only, and the container sees an empty directory. This phase
   stages nothing, so an empty `/de/scripts` is correct.

3. **The volume outlives `--rm` — and cleaning it up is NOT this phase's
   job.** Measured: after the container exits, `docker volume ls` still lists
   the volume. Phase-06 owns container and volume GC (it already owns
   `docker rm -f` and the orphan sweep). Do **not** add volume cleanup here,
   and do **not** touch `src/daemon/background/gc.rs`.

4. **`run_args` does not include the runtime binary.** It begins with
   `"run"`. Prepend `cfg.runtime` when building the command line, or you will
   emit `run --rm …` with no `docker`.

5. **Sandbox off must be byte-identical to today.** With
   `enabled = false` the function returns the input command unchanged — not
   "equivalent", identical. A negative test pins this, and it is what keeps
   the default path shippable.

6. **`cargo test --lib sandbox_window` passes today with zero tests.** Every
   criterion is a line count, not an exit status.

## Spec

### Task 1 — The pure wrapper

In `src/daemon/executor/container.rs`:

```rust
/// The command string a `de-bg-*` window should run for `raw_cmd`.
///
/// With the sandbox disabled this is `raw_cmd` unchanged. With it enabled the
/// result is a fully shell-quoted `docker run …` line that carries `raw_cmd`
/// as a single literal argument to the container's shell, so nothing in it is
/// interpreted by the host shell.
pub fn sandbox_window_command(cfg: &SandboxConfig, spec: &ExecSpec, raw_cmd: &str) -> String
```

Behaviour, in order:

1. `!cfg.enabled` → return `raw_cmd.to_string()`.
2. Build `argv` = `cfg.runtime` followed by `run_args(cfg, spec)`.
3. If `run_args` returned an empty vector (phase-04 returns empty for an
   unparseable `run_as`), return `raw_cmd.to_string()` — **and log a
   `log::warn!` naming `cfg.run_as`.** Falling back to the host command is the
   conservative choice for a flag that is off by default; silently doing so is
   not.
4. Otherwise join every element of `argv` with a single space, each passed
   through `crate::daemon::utils::sh_single_quote`.

`spec.command` must be `raw_cmd`; the caller sets that.

### Task 2 — Wire it into the background path

In `src/daemon/background/run.rs`, at the seam quoted in § Current state —
**after** the sudo-sentinel rebinding, **before** `let wrapped = …`:

- Load the config the same way the surrounding daemon code does. If a
  `&Config` or `&SandboxConfig` is already threaded into
  `run_background_in_window`, use it; if not, load it with the crate's
  existing config accessor rather than inventing a new one. **If neither is
  available without changing the function's signature in a way that ripples
  beyond this file, record a blocker and stop** — do not thread a new
  parameter through unrelated call sites.
- Build `ExecSpec { job_id: &format!("{pane_num}-{unix_ts}"), network: "none",
  is_ghost: false, command: cmd }`.
- Rebind `cmd` to the result of `sandbox_window_command(...)`.

`network: "none"` and `is_ghost: false` are correct for this phase: background
commands are not ghosts, and profile-driven networking arrives with the proxy
in a later phase.

### Task 3 — Remove the dead-code allow

`src/daemon/executor/mod.rs` now has a real production caller, so delete the
`#[allow(dead_code)]` **and its two-line comment** above
`pub(crate) mod container;`. Repo-wide count goes 7 → 6. If anything in the
module is still unreachable and the lint fires, **record a blocker naming the
unreachable items** — do not re-add the attribute and do not add a new one.

### Task 4 — Unit tests

Add the tests named in § Test plan to `container.rs`'s existing `mod tests`.
Every name must contain `sandbox_window`.

### Task 5 — One `#[ignore]`d live test

Add exactly one, `sandbox_window_command_line_runs_in_a_real_container`,
marked `#[ignore = "requires a running rootless Docker daemon"]`. It builds
the command line for `echo sandbox-ok`, runs it through
`std::process::Command::new("sh").arg("-c").arg(&line)`, and asserts the
captured stdout contains `sandbox-ok`. It must not run under the default
`cargo test`; the architect runs the ignored set at milestone close. This is
the milestone's **second** `#[ignore]`, so the pinned count becomes 2.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `grep -c "pub fn sandbox_window_command" src/daemon/executor/container.rs`
      prints `1` (**before: 0**).
- [ ] `grep -c "sandbox_window_command" src/daemon/background/run.rs` prints
      `1` (**before: 0**) — the single wiring point.
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**before: 7**) — the module's attribute is gone and no new
      one replaced it.
- [ ] `grep -c "#\[ignore" src/daemon/executor/container.rs` prints `2`
      (**before: 1**) — phase-02's live probe plus this phase's, and no more.
- [ ] `grep -c "shell_escape_arg" src/daemon/executor/container.rs` prints `0`
      (**before: 0**) — the wrong quoter is never used here (§ Current state).
- [ ] `cargo test --lib sandbox_window 2>&1 | grep -c "^test .* ok$"` prints
      `6` — one per non-ignored test in § Test plan. A count, not an exit
      status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1432 passed; 0 failed; 2 ignored` (1426 + 6 new, and the ignored count
      rises 1 → 2).
- [ ] `git diff --stat src/daemon/background/gc.rs` is empty — volume cleanup
      is phase-06's (§ Gotchas 3).
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Six non-ignored tests plus the one ignored live test, all in `container.rs`.
Every name contains `sandbox_window`.

- `sandbox_window_disabled_returns_the_command_unchanged` — with
  `SandboxConfig::default()` (`enabled` is false), the result is **exactly**
  `"echo hi"` — same string, not merely equivalent (§ Gotchas 5).
- `sandbox_window_enabled_starts_with_the_quoted_runtime` — with
  `enabled = true`, the result starts with `'docker' 'run' '--rm'`.
- `sandbox_window_keeps_a_hostile_command_in_one_token` — the **load-bearing
  test**. With `enabled = true` and
  `raw_cmd = "echo inside-container; touch /tmp/PWNED"`, the result **ends
  with** the single quoted token
  `'echo inside-container; touch /tmp/PWNED'`, and the substring
  `; touch` never appears unquoted — i.e. the result does **not** contain
  `` `; touch /tmp/PWNED'` `` preceded by an unmatched quote. Assert
  concretely: `result.ends_with("'echo inside-container; touch /tmp/PWNED'")`.
- `sandbox_window_quotes_embedded_single_quotes` — `raw_cmd = "echo 'a'"`
  produces a result ending with `'echo '\''a'\'''`, the `sh_single_quote`
  rendering. **Negative half:** the result must not contain the bare
  two-character sequence `''` produced by naive quoting of `'`.
- `sandbox_window_carries_the_job_id_into_the_volume_mount` — with
  `job_id = "42-1712937600"` the result contains
  `'de-stage-42-1712937600:/de/scripts:ro'`.
- `sandbox_window_falls_back_when_run_as_is_unparseable` — with
  `enabled = true` and `run_as = "nope"`, the result is `raw_cmd` unchanged
  (the Task 1 step-3 fallback).

**Ignored:**

- `sandbox_window_command_line_runs_in_a_real_container` — per Task 5.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_window tests (expect 6 lines) =="
cargo test --lib sandbox_window 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "wrapper defined:      "; grep -c "pub fn sandbox_window_command" src/daemon/executor/container.rs
echo -n "single wiring point:  "; grep -c "sandbox_window_command" src/daemon/background/run.rs
echo -n "allow(dead_code) tot: "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "ignore count:         "; grep -c "#\[ignore" src/daemon/executor/container.rs
echo -n "wrong quoter absent:  "; grep -c "shell_escape_arg" src/daemon/executor/container.rs
echo -n "gc.rs untouched:      "; git diff --stat src/daemon/background/gc.rs | wc -l
} > /tmp/e2e-05.txt 2>&1
cat /tmp/e2e-05.txt
```

Paste the contents of `/tmp/e2e-05.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-05-background-window-integration.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section A is the one that can lie** — on the current tree it prints zero
test lines and still reports `cargo_exit=0`. Six lines is the pass condition.

## Authorizations

- Edit `src/daemon/executor/container.rs`, `src/daemon/executor/mod.rs`, and
  `src/daemon/background/run.rs`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The one live test is `#[ignore]`d.
- **Do not add any `#[allow(...)]`** — including re-adding the one Task 3
  deletes.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause, *or* Task 2 cannot
  reach the config without rippling beyond `run.rs` — record a blocker Update
  Log entry naming the exact criterion, failing test, or missing binding, and
  stop.** Do not repair unrelated code to turn a gate green, and do not edit
  this phase doc's criteria. Reporting the blocker *is* the successful
  outcome in that case.
- **Record what you decide, not what you wish had been decided.** Update Log
  entries are the audit trail: never attribute a choice to guidance you did
  not receive.

## Out of scope

- **Volume and container GC** — phase-06 (§ Gotchas 3). Do not touch
  `src/daemon/background/gc.rs`.
- **Foreground execution.** `send-keys` into the user's pane stays host-level
  by design (design § D0). Do not touch `src/daemon/executor/foreground.rs`.
- Ghost lifecycle and the `de.ghost=1` label (phase-06), the egress proxy and
  `network = "proxy"` (later), `Request::ContainerStatus` and the
  `daemoneye status` surface (later), the `log` relay opcode (later).
- Script staging — nothing populates `/de/scripts` yet; an empty volume is
  correct here (§ Gotchas 2).
- Changing phase-01/02/03/04 behaviour, `containers/Dockerfile`, `CLAUDE.md`,
  `README.md`, or `assets/etc/config.toml`.

## Update Log
