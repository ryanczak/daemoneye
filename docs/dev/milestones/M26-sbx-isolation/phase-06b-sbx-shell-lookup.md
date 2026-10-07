# Phase 06b: sbx-shell-lookup — find or create an owner's shell in a given sandbox, and mark it POSIX

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-06a
**Estimated diff:** ~260 lines (about 60 code, 200 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Row 06c will run a sandboxed ghost's commands in **its sandbox's** shell.
Two pieces are missing for that:

1. **A lookup that finds or creates an owner's shell *in a named
   sandbox*.** `shell_for_owner` deliberately never returns a sandbox shell
   (row 04a), so a ghost asking it would spawn a fresh host shell every time.
   `shell_for_owner_in_sandbox` is its sandbox twin: it reuses the owner's
   shell in that sandbox and creates one with `spawn_for_in_sandbox` when
   there is none.
2. **The right marker name when a command targets a sandbox shell by id.**
   `run_in_shell_with_prompts` picks the marker from the record's **host**
   (`marker_shell_name(&record_host)`). For a sandbox shell the host is
   `Local`, so it gets the daemon's `$SHELL`. If that is fish, the marker
   asks for `$status`, which the sandbox's bash expands to nothing, and no
   exit code ever parses. It must use `shell_marker_name(record)` (row 04a),
   which gives `"sh"` for a sandbox shell.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/phase-04a-sbx-shell.md`: the
  `ShellRecord.sandbox` field, `spawn_for_in_sandbox`, `shell_marker_name` and
  the reuse rule.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### `shell_for_owner` (`src/shell/session.rs`): the shape to copy

```rust
pub async fn shell_for_owner<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    host: crate::host::HostId,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let owner_key = owner.key();
    let reuse: Option<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                // … (comments) …
                !record.interactive
                    && !record.dedicated
                    && record.host == host
                    && record.owner.key() == owner_key
                    && record.sandbox.is_none()
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        let lowest = matches.into_iter().next()?;
        if host_is_alive(&launch::lock_path(lowest.as_str())) {
            Some(lowest)
        } else {
            store.remove(lowest.as_str());
            None
        }
    });

    if let Some(id) = reuse {
        let socket = launch::socket_path(id.as_str());
        return Ok((id, socket));
    }

    let id = spawn_for_on_host(reg, owner, max_per_owner, host, false, spawn).await?;
    let socket = launch::socket_path(id.as_str());
    if let Err(e) = launch::await_ready(&socket, Instant::now() + ready_timeout).await {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(e).with_context(|| {
            format!(
                "shell {} never became ready at {} within {ready_timeout:?}",
                id.as_str(),
                socket.display()
            )
        });
    }
    Ok((id, socket))
}
```

### The marker choice in `run_in_shell_with_prompts` (`src/daemon/executor/shell_exec.rs`)

```rust
let (started_unix, record_host) = with_shells(reg, |store| {
    store
        .get(id.as_str())
        .map(|record| (record.started_unix, record.host.clone()))
        .unwrap_or((0, crate::host::HostId::Local))
});
…
        &marker_shell_name(&record_host),     // plain path, ~line 352
…
        &marker_shell_name(&record_host),     // sudo path, ~line 376
```

`record_host` is also used **elsewhere** in that function (for example
`record_host.as_str()` is passed to the sudo path). Keep it. Only the marker
name changes.

`crate::shell::shell_marker_name(&ShellRecord) -> String` exists and is
exported (row 04a).

### Test fixtures to copy

In `src/shell/session.rs` tests, `shell_for_owner_never_reuses_a_sandbox_shell`
(row 04a) already builds a live sandbox shell:

```rust
let boxed_id = spawn_for_in_sandbox(
    &reg, owner.clone(), 5, "de-chat-t".into(), false, spawn_in_process_host,
)
.await
.expect("the sandbox shell's host comes up");
launch::await_ready(
    &launch::socket_path(boxed_id.as_str()),
    std::time::Instant::now() + Duration::from_secs(15),
)
.await
.expect("the sandbox shell's host must be alive, or the filter is not under test");
```

`spawn_in_process_host` spawns a real local `bash` behind the socket, which
stands in for the VM's bash: the marker protocol is the same.

In `src/daemon/executor/shell_exec.rs` tests, `run_in_shell_with_prompts` is
called with an explicit shell id like this (from
`a_hang_after_an_answer_still_times_out`):

```rust
let outcome = run_in_shell_with_prompts(
    &reg,
    owner,
    "sudo true; sleep 30",
    5,
    Some(id.as_str()),
    Some(2),
    "p",
    &mut writer,
    &mut reader,
    crate::host::HostId::Local,
    move |_id: ShellId| async move { Ok(()) },
)
.await;
```

That module also has `test_home()` and `spawn_in_process_host(id)`.

## Spec

### Task 1 — `shell_for_owner_in_sandbox` (`src/shell/session.rs`)

Add, next to `shell_for_owner`:

```rust
/// `shell_for_owner` for shells inside the sbx sandbox `sandbox`: reuse the
/// owner's lowest-numbered live shell in **that** sandbox, or create one with
/// `spawn_for_in_sandbox`. A host shell, and a shell in any other sandbox, is
/// never returned.
pub async fn shell_for_owner_in_sandbox<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    sandbox: &str,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>
```

It is the same body as `shell_for_owner`, with exactly two differences:

- the reuse filter is
  `!record.interactive && !record.dedicated && record.owner.key() == owner_key && record.sandbox.as_deref() == Some(sandbox)`;
  there is no `host` condition, because a sandbox shell's host is always
  `Local`;
- the create step is
  `crate::shell::registry::spawn_for_in_sandbox(reg, owner, max_per_owner, sandbox.to_string(), false, spawn).await?`.

Keep the dead-host removal, the readiness wait and its rollback exactly as in
`shell_for_owner`. Export it from `src/shell/mod.rs` next to
`shell_for_owner`.

### Task 2 — The marker name comes from the record

In `run_in_shell_with_prompts` (`src/daemon/executor/shell_exec.rs`), extend
the record read to also compute the marker name:

```rust
let (started_unix, record_host, marker) = with_shells(reg, |store| {
    store
        .get(id.as_str())
        .map(|record| {
            (
                record.started_unix,
                record.host.clone(),
                crate::shell::shell_marker_name(record),
            )
        })
        .unwrap_or_else(|| {
            (0, crate::host::HostId::Local, marker_shell_name(&crate::host::HostId::Local))
        })
});
```

Then replace **both** `&marker_shell_name(&record_host)` arguments with
`&marker`. Do not change anything else in that function.

### Task 3 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] The five tests in § Test plan each report `1 passed; 0 failed` when run
      by name.
- [ ] `cargo test --lib shell::session` and
      `cargo test --lib executor::shell_exec` report `0 failed`, with no
      existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe` (outside the tests' existing `set_var` pattern),
      `.unwrap()` or `.expect(` added outside `#[cfg(test)]` code.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/shell/session.rs` tests (each uses `test_home()`, a fresh
`ShellRegistry`, and `spawn_in_process_host` so the shells are really alive):

- `shell_for_owner_in_sandbox_creates_then_reuses`: two calls with the same
  owner and sandbox `"de-ghost-a"` return the **same** id. The record has
  `sandbox == Some("de-ghost-a")`.
- `shell_for_owner_in_sandbox_keeps_sandboxes_apart`: the same owner, with
  sandbox `"de-ghost-a"` then `"de-ghost-b"`, gets **different** ids, each
  recorded with its own sandbox.
- `shell_for_owner_in_sandbox_never_returns_a_host_shell`: first
  `shell_for_owner(.., HostId::Local, ..)` gives a plain shell. Then
  `shell_for_owner_in_sandbox(.., "de-ghost-a", ..)` returns a **different**
  id, and the plain shell's record still has `sandbox == None`.
- `shell_for_owner_in_sandbox_ignores_another_owners_shell`: owner A's shell
  in `"de-ghost-a"` is not returned to owner B asking for the same sandbox.

In `src/daemon/executor/shell_exec.rs` tests:

- `a_sandbox_shell_targeted_by_id_uses_the_posix_marker`:
  1. `test_home()`; create a live sandbox shell with
     `crate::shell::spawn_for_in_sandbox(&reg, owner.clone(), 5, "de-ghost-t".into(), false, spawn_in_process_host)`,
     then `crate::shell::await_ready(&socket_path(id.as_str()), …15 s)`.
  2. Set `SHELL=/usr/bin/fish` for the duration of the call, using
     `unsafe { std::env::set_var(…) }` exactly like the existing
     `with_fish_shell` helper in `src/shell/session.rs`. Restore it with
     `remove_var` afterwards, **even on failure**: put the restore in a small
     guard struct's `Drop`.
  3. `run_in_shell_with_prompts(&reg, owner, "sh -c 'exit 3'", 5, Some(id.as_str()), Some(10), "", &mut tokio::io::sink(), &mut tokio::io::empty(), HostId::Local, |_id| async { Ok(()) })`.
  4. Assert `Ok` with `exit_code == 3`. Without Task 2 the marker is fish's
     `$status`, bash expands it to nothing, and the call fails or times out.

**Reviewer mutation:** revert Task 2's marker (use
`marker_shell_name(&record_host)` again), and
`a_sandbox_shell_targeted_by_id_uses_the_posix_marker` must fail.

## End-to-end verification

```sh
cargo test --lib shell_for_owner_in_sandbox 2>&1 | grep -E '^test result'
cargo test --lib a_sandbox_shell_targeted_by_id_uses_the_posix_marker 2>&1 | grep -E '^test result'
cargo test --lib shell::session 2>&1 | grep -E '^test result'
cargo test --lib executor::shell_exec 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.
The first line should report `4 passed`.

## Authorizations

- [x] May edit `run_in_shell_with_prompts` in
      `src/daemon/executor/shell_exec.rs` **only** as Task 2 says.
- [x] May add the `shell_for_owner_in_sandbox` export to `src/shell/mod.rs`.

## Out of scope

- Creating sandboxes, routing ghost commands, teardown and the startup sweep:
  row 06c.
- Changing `shell_for_owner`, `spawn_for_in_sandbox` or `shell_marker_name`.
- Any other caller of `marker_shell_name`.

## Update Log

<!-- entries appended below this line -->
