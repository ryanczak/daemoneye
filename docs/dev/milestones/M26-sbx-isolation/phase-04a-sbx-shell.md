# Phase 04a: sbx-shell — shell records that live in a sandbox, and how they are spawned

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-03
**Estimated diff:** ~200 lines (about 70 code, 130 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Teach the shell registry that a shell can run **inside an sbx sandbox**, and
give the daemon the function that spawns one. Nothing creates sandboxes or
routes commands to these shells yet; rows 06–07 do that. This row lays four
pieces:

1. a `sandbox` field on `ShellRecord`;
2. a registry spawn that stamps it;
3. a shell-host launcher whose child is `sbx exec -it <sandbox> bash`;
4. the marker-shell rule for such a shell.

It also adds one safety property now: an ordinary caller is **never** handed a
sandbox shell.

Adoption after a daemon restart (recovering the sandbox name) is row **04b**.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Measured on pinky":
  the marker protocol round-trips through `sbx exec -it <n> bash`, with real
  exit codes, `^C` and resize.
- `docs/design/daemoneye-2.0.md` § 2.9: a sandboxed command is a PTY shell
  whose shell-host child is `sbx exec -it <sandbox> bash`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### `ShellRecord` (`src/shell/registry.rs`)

```rust
pub struct ShellRecord {
    pub id: ShellId,
    pub owner: Owner,
    /// Where this shell runs. `HostId::Local` for every shell the daemon
    /// spawns today; a named host arrives with the SSH spawn.
    pub host: crate::host::HostId,
    pub started_unix: u64,
    pub interactive: bool,
    pub dedicated: bool,
    pub exited_unix: Option<u64>,
}
```

It is built with a struct literal at **exactly ten sites**. Every one must
gain the new field:

| File | Line (approx.) | What it is |
|---|---|---|
| `src/shell/registry.rs` | 253 | `spawn_for_on_host`, the production insert |
| `src/shell/registry.rs` | 377 | `adopt_at_startup` |
| `src/shell/registry.rs` | 479 | test helper `fn record(…)` |
| `src/shell/session.rs` | 513, 564, 615, 627, 771 | tests |
| `src/daemon/executor/knowledge/shell.rs` | 2309 | test |

### The registry spawn (`src/shell/registry.rs`)

`spawn_for` is a one-line delegation to `spawn_for_on_host`, which inserts the
record and then runs the caller's `spawn` closure (removing the record if it
fails):

```rust
pub async fn spawn_for_on_host<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    host: crate::host::HostId,
    dedicated: bool,
    spawn: F,
) -> anyhow::Result<ShellId>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let id = with_shells(reg, |store| -> anyhow::Result<ShellId> {
        check_owner_cap(store, &owner, max_per_owner)?;
        …
        store.insert(
            id.as_str().to_string(),
            ShellRecord {
                id: id.clone(),
                owner: owner.clone(),
                host,
                started_unix,
                interactive: false,
                dedicated,
                exited_unix: None,
            },
        );
        Ok(id)
    })?;
    if let Err(e) = spawn(id.clone()).await { … }
    Ok(id)
}
```

### The SSH launcher: the shape to copy (`src/shell/session.rs`)

```rust
pub async fn spawn_ssh_host(id: ShellId, name: &str, rows: u16, cols: u16) -> anyhow::Result<()> {
    let entry = host_entry_or_bare(name)?;
    …
    let argv = ssh_argv(&entry, &control);
    let program = which_ssh()?;
    launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await
}

pub fn which_ssh() -> anyhow::Result<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("ssh");
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err(anyhow::anyhow!(
        "ssh was not found on PATH for the daemon user — an SSH shell needs the \
         ssh(1) binary; install openssh-clients or add it to PATH"
    ))
}
```

The argv for the sandbox shell already exists and is tested. Use it; do not
re-spell the argv:

```rust
// src/daemon/executor/sbx.rs
pub fn shell_exec_args(name: &str, workdir: Option<&str>) -> Vec<String>
// shell_exec_args("de-ghost-r1", None) == ["exec", "-it", "de-ghost-r1", "bash"]
```

### Reuse (`shell_for_owner`, `src/shell/session.rs`)

The reuse filter today:

```rust
!record.interactive
    && !record.dedicated
    && record.host == host
    && record.owner.key() == owner_key
```

### Marker shell names (`src/shell/session.rs`)

```rust
pub fn marker_shell_name(host: &crate::host::HostId) -> String {
    match host {
        crate::host::HostId::Local => login_shell(),
        crate::host::HostId::Named(_) => "sh".to_string(),
    }
}
```

`login_shell()` is the daemon's `$SHELL`, which may be `fish`, where the exit
variable is `$status`. The shell inside a sandbox is always `bash` (the argv
above ends in `bash`), so a sandbox shell must use the POSIX name `"sh"`
(`$?`), the same as a remote shell.

## Spec

### Task 1 — `ShellRecord.sandbox`

Add to `ShellRecord`, after `host`:

```rust
/// The sbx sandbox this shell's child runs in (`sbx exec -it <sandbox> bash`),
/// or `None` for a shell on the host itself. A sandbox shell is only ever
/// handed to a caller that asks for that sandbox.
pub sandbox: Option<String>,
```

Set `sandbox: None` at **all ten** literal sites in the table. Do not use
`..Default::default()`: `ShellRecord` has no `Default`, and must not gain
one. The compiler enforces the count: a missed
site is a build error, so there is no count criterion. Do not grep-count
`sandbox: None` either, because the new tests add literals of their own.

### Task 2 — `spawn_for_in_sandbox`

In `src/shell/registry.rs`:

- Move the body of `spawn_for_on_host` into a new **private** function
  `spawn_for_placed` with one extra parameter `sandbox: Option<String>`,
  placed right after `host`. The inserted record uses it (`sandbox,` in the
  literal).
- `spawn_for_on_host` keeps its exact signature and becomes a one-line
  delegation: `spawn_for_placed(reg, owner, max_per_owner, host, None, dedicated, spawn).await`.
- Add:

  ```rust
  /// `spawn_for` for a shell whose child runs inside the sbx sandbox
  /// `sandbox`. The shell process itself is local; its commands run in the VM.
  pub async fn spawn_for_in_sandbox<F, Fut>(
      reg: &ShellRegistry,
      owner: Owner,
      max_per_owner: u32,
      sandbox: String,
      dedicated: bool,
      spawn: F,
  ) -> anyhow::Result<ShellId>
  ```

  delegating with `crate::host::HostId::Local` and `Some(sandbox)`.
- Export it next to the others in `src/shell/mod.rs`, where today's line reads
  `spawn_for, spawn_for_on_host, sweep_stale, with_shells,`.

### Task 3 — `which_sbx` and `spawn_sbx_host`

In `src/shell/session.rs`, next to `which_ssh` / `spawn_ssh_host`:

- `pub fn which_sbx() -> anyhow::Result<String>`: the same shape as
  `which_ssh`, looking for `sbx`. Its error says that `sbx` was not found on
  PATH for the daemon user, and that a sandbox shell needs Docker Sandboxes
  (`sbx`) installed.
- `pub async fn spawn_sbx_host(id: ShellId, sandbox: &str, rows: u16, cols: u16) -> anyhow::Result<()>`:

  ```rust
  let argv = crate::daemon::executor::sbx::shell_exec_args(sandbox, None);
  let program = which_sbx()?;
  launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await
  ```

  It resolves `current_exe()` through `spawn_detached`, so it cannot run
  under a test, like `spawn_ssh_host`. It gets no test of its own; its argv
  is `shell_exec_args`, which is tested.

### Task 4 — Never reuse a sandbox shell for an ordinary caller

In `shell_for_owner`, add `&& record.sandbox.is_none()` to the reuse filter.
Extend the comment above the filter with one sentence: a sandbox shell runs
its commands in a VM, so it is handed only to a caller that asks for that
sandbox, never to `shell_for_owner`.

### Task 5 — `shell_marker_name`

In `src/shell/session.rs`, next to `marker_shell_name`:

```rust
/// The marker-protocol shell name for an existing shell: `"sh"` for a shell
/// inside a sandbox (its child is always `bash`), otherwise
/// `marker_shell_name(&record.host)`.
pub fn shell_marker_name(record: &ShellRecord) -> String
```

Export it from `src/shell/mod.rs` the same way `marker_shell_name` is. **Do
not** change any existing caller of `marker_shell_name`. Rows 06–07 switch
the call sites that can see a sandbox shell.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib spawn_for_in_sandbox_records_the_sandbox` reports
      `1 passed`.
- [ ] `cargo test --lib spawn_for_on_host_records_no_sandbox` reports
      `1 passed`.
- [ ] `cargo test --lib shell_for_owner_never_reuses_a_sandbox_shell` reports
      `1 passed`.
- [ ] `cargo test --lib shell_marker_name_is_posix_inside_a_sandbox` reports
      `1 passed`.
- [ ] The existing `spawn_for_on_host_records_the_named_host` and
      `shell_for_owner_does_not_reuse_a_shell_from_another_host` pass,
      unedited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code.
- [ ] The Update Log entry contains the § End-to-end verification output and
      a line `PASTE MATCH: yes`, meaning the pasted block equals the terminal
      output. Paste every line as printed, including clippy's `Finished …`
      line.

## Test plan

- `spawn_for_in_sandbox_records_the_sandbox` in `src/shell/registry.rs`.
  Copy `spawn_for_on_host_records_the_named_host`: call
  `spawn_for_in_sandbox(&reg, Owner::Chat("a".into()), 5, "de-ghost-r1".into(), false, |_id| async { Ok(()) })`,
  then assert the saved record's `sandbox == Some("de-ghost-r1".to_string())`
  **and** `host == HostId::Local`.
- `spawn_for_on_host_records_no_sandbox` in `src/shell/registry.rs`: the
  named-host spawn gives `sandbox == None`.
- `shell_for_owner_never_reuses_a_sandbox_shell` in `src/shell/session.rs`.
  Copy `shell_for_owner_does_not_reuse_a_shell_from_another_host`:
  1. `test_home()`; a registry; `owner = Owner::Chat("boxed".into())`.
  2. `spawn_for_in_sandbox(&reg, owner.clone(), 5, "de-chat-t".into(), false, spawn_in_process_host)`,
     then `launch::await_ready(&launch::socket_path(id.as_str()), Instant::now() + Duration::from_secs(15))`.
     The host must really be alive, or a broken filter would not be caught:
     `shell_for_owner` drops a dead match instead of reusing it.
  3. `shell_for_owner(&reg, owner, HostId::Local, 5, Duration::from_secs(15), spawn_in_process_host)`.
  4. Assert the returned id **differs** from the sandbox shell's id, and the
     sandbox record is still in the registry.
- `shell_marker_name_is_posix_inside_a_sandbox` in `src/shell/session.rs`.
  Build two records (copy the struct literal in `registry.rs` `fn record`,
  which this row extends):
  - one with `host: Local, sandbox: Some("de-ghost-r1")` → `"sh"`;
  - one with `host: Local, sandbox: None` → `login_shell()`.

  Do not set `$SHELL` in the test; compare against `login_shell()`.

**Reviewer mutation:** delete `&& record.sandbox.is_none()` from the reuse
filter. `shell_for_owner_never_reuses_a_sandbox_shell` must fail.

## End-to-end verification

```sh
cargo test --lib -- spawn_for_in_sandbox_records_the_sandbox spawn_for_on_host_records_no_sandbox shell_for_owner_never_reuses_a_sandbox_shell shell_marker_name_is_posix_inside_a_sandbox spawn_for_on_host_records_the_named_host shell_for_owner_does_not_reuse_a_shell_from_another_host 2>&1 | grep -E '^test |^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expected: six tests `ok` and `6 passed; 0 failed`;
clippy `Finished …`; `fmt_exit=0`; `failed=0`.

## Authorizations

- [x] May edit the ten `ShellRecord` literal sites listed in § Current state,
      including the test sites in `src/shell/session.rs` and
      `src/daemon/executor/knowledge/shell.rs`, **only** to add
      `sandbox: None`.
- [x] May add the `spawn_for_in_sandbox` and `shell_marker_name` exports to
      `src/shell/mod.rs`.

## Out of scope

- Creating, removing or listing sandboxes; preflight. That is rows 06–07,
  using `src/daemon/executor/sbx.rs`.
- Changing any caller of `marker_shell_name`, `shell_for_owner` or
  `spawn_for` beyond Task 4's filter.
- Adoption (`adopt_at_startup` keeps `sandbox: None`): row 04b.
- Pause/resume/cancel inside the VM: row 05.
- The `[SHELLS]` context block and `list_shells` output.

## Update Log

<!-- entries appended below this line -->
