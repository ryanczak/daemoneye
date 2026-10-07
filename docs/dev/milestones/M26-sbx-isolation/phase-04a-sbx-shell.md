# Phase 04a: sbx-shell — shell records that live in a sandbox, and how they are spawned

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** done
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

### Update — 2026-10-06 16:43 (progress)

Started phase-04a: flipped the status to in-progress and updated the README row.
Read the current `ShellRecord` and its ten literal sites, the SSH launcher shape
in `session.rs`, and the `shell_exec_args` builder in `sbx.rs`. Now adding the
`sandbox` field, the `spawn_for_in_sandbox` registry spawn, the sbx launcher,
the reuse-filter guard, and `shell_marker_name`.

### Update — 2026-10-06 16:58 (end-to-end verification)

Ran the § End-to-end verification block verbatim, once, redirecting to
`.rexymcp/output/phase-04a-e2e.log`; the paste below is that file.

```
test shell::session::tests::shell_marker_name_is_posix_inside_a_sandbox ... ok
test shell::registry::tests::spawn_for_on_host_records_the_named_host ... ok
test shell::registry::tests::spawn_for_in_sandbox_records_the_sandbox ... ok
test shell::registry::tests::spawn_for_on_host_records_no_sandbox ... ok
test shell::session::tests::shell_for_owner_never_reuses_a_sandbox_shell ... ok
test shell::session::tests::shell_for_owner_does_not_reuse_a_shell_from_another_host ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2269 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.92s
fmt_exit=0
passed=2325 failed=0
 docs/dev/milestones/M26-sbx-isolation/README.md    |   2 +-
 .../M26-sbx-isolation/phase-04a-sbx-shell.md       |  10 +-
 src/daemon/executor/knowledge/shell.rs             |   1 +
 src/shell/mod.rs                                   |   6 +-
 src/shell/registry.rs                              | 100 ++++++++++++++
 src/shell/session.rs                               | 153 ++++++++++++++++++++-
 6 files changed, 265 insertions(+), 7 deletions(-)
```

PASTE MATCH: yes — six tests ok and `6 passed; 0 failed`; clippy `Finished …`;
`fmt_exit=0`; `passed=2325 failed=0`.

### Update — 2026-10-06 23:59 (complete, server-authored)

**Summary:** </think>All tasks complete. Working tree clean, commit made, phase status stays in-progress (server flips to review). Final message: Summary + Notes for review.</think>
**Summary:** Phase-04a is implemented and committed (`cedb2d5`, working tree clean). `ShellRecord` gained `sandbox: Option<String>` (set `None` at all ten literal sites, compiler-enforced); the registry spawn body moved into private `spawn_for_placed` with `spawn_for_on_host` now a one-line delegation and new `spawn_for_in_sandbox` stamping `HostId::Local` + `Some(sandbox)`; `session.rs` gained `which_sbx` (PATH probe, remedy-naming error) and `spawn_sbx_host` (child argv reuses the tested `sbx::shell_exec_args`, so it spells `exec -it <sandbox> bash` exactly); `shell_for_owner`'s reuse filter gained `&& record.sandbox.is_none()` with the one-sentence comment extension; and `shell_marker_name(&ShellRecord)` returns `"sh"` for a sandbox shell and otherwise defers to the existing `marker_shell_name(&record.host)` — no existing caller of `marker_shell_name` changed. Both new exports are re-exported from `src/shell/mod.rs`; adoption still stamps `None` (row 04b).

**Notes for review:** One deviation from the spec's sketch: `spawn_for_in_sandbox`'s `sandbox` parameter sits after `max_per_owner` (before `dedicated`) rather than after `host`, because the function takes no `host` at all — the spec's own signature shows the same ordering, so only the prose ordering ("placed right after `host`") was adapted to the real signature. The end-to-end block ran once, captured to `.rexymcp/output/phase-04a-e2e.log`, and is pasted in the phase's `(end-to-end verification)` Update Log entry with `PASTE MATCH: yes`: all six named tests `ok` / `6 passed; 0 failed`, clippy `Finished`, `fmt_exit=0`, full suite `passed=2325 failed=0`. `git diff --stat` in that capture predates the commit (it shows the pre-commit working tree), which is what the block as specified produces. The mutation check (`delete && record.sandbox.is_none()` → the reuse test must fail) is left for the reviewer per the phase doc. Phase doc status remains `in-progress` with no `(complete)` entry written by me, per the contract.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
r_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.08s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (165484): No such process
kill: (-166381): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M26-sbx-isolation/README.md` — +1 -1
- `docs/dev/milestones/M26-sbx-isolation/phase-04a-sbx-shell.md` — +37 -1
- `src/daemon/executor/knowledge/shell.rs` — +1 -0
- `src/shell/mod.rs` — +3 -3
- `src/shell/registry.rs` — +100 -0
- `src/shell/session.rs` — +146 -2

**Commit:** cedb2d5260fd90921f1b814f472b0a6d2e1b9c7a

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-06

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (128 turns,
  `cedb2d52`)
- **Scope deviations:** none. The "deviation" the executor flags (parameter
  order) is not one: `spawn_for_placed` takes `sandbox` right after `host` as
  specified, and `spawn_for_in_sandbox` has the specified signature. The
  marker test wraps its assertions in the existing `with_fish_shell` helper,
  which makes the `"sh"`-vs-`login_shell()` distinction real. That is better
  than the spec asked for.
- **Reviewer re-run:** fmt, build and clippy clean; the six named tests
  `6 passed`; full suite `passed=2325 failed=0` on the first run; no
  `unsafe` or `#[allow]` added.
- **Reviewer mutation checks** (each turned its test red, then restored):
  dropping `&& record.sandbox.is_none()` from the reuse filter
  (`shell_for_owner_never_reuses_a_sandbox_shell`); deleting the sandbox
  branch of `shell_marker_name` (`shell_marker_name_is_posix_inside_a_sandbox`).
- **Calibration:** none.
