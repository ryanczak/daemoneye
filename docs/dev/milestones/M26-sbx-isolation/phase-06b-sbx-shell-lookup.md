# Phase 06b: sbx-shell-lookup — find or create an owner's shell in a given sandbox, and mark it POSIX

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** review
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

### Update — 2026-10-07 08:39 (started)

**Executor:** executor (Claude Opus 4.6, headless run)
Flipped the phase's Status to `in-progress` and the milestone README's row
with it. Beginning Task 1 (`shell_for_owner_in_sandbox`).

### Update — 2026-10-07 09:07 (end-to-end verification)

§ End-to-end verification, run verbatim, every line as printed (the `cargo
test --lib` lines show only the `test result:` summaries because rexyMCP's
output filter removes the individual `test … ok` lines; the tail line of
`cargo clippy … -D warnings` is its `Finished` line, i.e. exit 0):

```
$ cargo test --lib shell_for_owner_in_sandbox 2>&1 | grep -E '^test result'
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 2322 filtered out; finished in 0.01s
$ cargo test --lib a_sandbox_shell_targeted_by_id_uses_the_posix_marker 2>&1 | grep -E '^test result'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2325 filtered out; finished in 0.02s
$ cargo test --lib shell::session 2>&1 | grep -E '^test result'
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 2297 filtered out; finished in 0.23s
$ cargo test --lib executor::shell_exec 2>&1 | grep -E '^test result'
test result: ok. 40 passed; 0 failed; 2 ignored; 0 measured; 2284 filtered out; finished in 11.14s
$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.47s
$ cargo fmt --all --check; echo "fmt_exit=$?"
fmt_exit=0
$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2376 failed=0
$ git diff --stat
(no output — the working tree is clean; the phase's diff is the commit
 cacadc69: 5 files changed, 417 insertions(+), 29 deletions(-))
```

PASTE MATCH: yes

- `shell_for_owner_in_sandbox` filters: 4 passed — the three reuse/boundary
  tests plus `shell_for_owner_in_sandbox` matching the count-scan's own
  `shell_for_owner*` filter prefix (all four tests in § Test plan run).
- `a_sandbox_shell_targeted_by_id_uses_the_posix_marker`: 1 passed with the
  daemon's `$SHELL` fish and a live sandbox shell running real bash — the
  POSIX `$?` marker parsed the child's exit code (asserted `== 3`).
- No pre-existing test was edited; `shell::session` went 25 → 29 tests,
  `executor::shell_exec` 39 → 40.
- The full suite (`cargo test`, all targets incl. integration): 2376 passed,
  0 failed.

### Update — 2026-10-07 16:09 (complete, server-authored)

**Summary:** </think>Everything is complete. Let me write the Summary + Notes for review.

Wait — one thing to double-check: the completion checklist says the phase doc's Status should still be `in-progress` — yes it is. The milestone README row — `in-progress` — yes. I did not write a `(complete)` entry — correct, I wrote the started entry and the E2E verification entry (which the STANDARDS §1 requires — "The end-to-end evidence lives in its own Update Log entry, authored by whoever ran the phase — titled `### Update — <date> (end-to-end verification)`"). That's per STANDARDS, not a `(complete)` entry — good.

`cargo fmt --all` was run before `git add` — yes (ran before the commit; and after it I only wrote the doc, which doesn't need fmt).

All verification commands ran clean — yes.

git status clean — yes.

Final message: Summary + Notes for review.</think>
**Summary**

Phase 06b is implemented and committed (`cacadc69` code + `91df5c78` E2E transcript; the phase doc stays `in-progress` for the server's completion tail). Task 1 added `shell_for_owner_in_sandbox` to `src/shell/session.rs` — `shell_for_owner`'s body with the two specified deltas (reuse filter keyed on `record.sandbox.as_deref() == Some(sandbox)` with no host condition; create via `spawn_for_in_sandbox`), keeping the dead-host removal, reuse shortcut and readiness rollback verbatim — and exported it from `src/shell/mod.rs`. Task 2 extended `run_in_shell_with_prompts`'s record read to compute `crate::shell::shell_marker_name(record)` (falling back to `marker_shell_name(&HostId::Local)` only when the record is absent) and pointed both the plain and sudo paths at it; nothing else in the function changed and `record_host` is still passed to the sudo path. Tests: the four `shell_for_owner_in_sandbox_*` tests in `session.rs` (same-id reuse counted via a spawn-call counter, per-sandbox isolation, host-shell exclusion with the plain record's `sandbox == None` intact, cross-owner exclusion) and `a_sandbox_shell_targeted_by_id_uses_the_posix_marker` in `shell_exec.rs` (live in-process bash behind a `spawn_for_in_sandbox` record, `$SHELL` set to fish by a `FishShellEnv` Drop guard, `sh -c 'exit 3'` → exit code 3).

**Notes for review**

- End-to-end verification ran verbatim and is pasted into the phase doc's `### Update — 2026-10-07 09:07 (end-to-end verification)` entry with `PASTE MATCH: yes`: the three named-test greps report `4 passed; 0 failed`, `1 passed; 0 failed` and the module greps `29 passed / 40 passed`, clippy exits 0, `fmt_exit=0`, full suite `passed=2376 failed=0`, `git diff --stat` empty (clean tree).
- Two adaptations to note: (1) rustc 1.97 rejects `let (.., _marker) = …` with a later `&marker` (E0423 "built-in attribute `marker`", reproduced standalone), so the binding is `marker` — under `#[cfg(test)]`-free non-test code it is fully used by both call sites, so no allow is needed; (2) my first append of the marker test landed *after* `mod tests`' closing brace (unexpected-`}` error); it now lives at the end of the `connect_tests` module with a `use super::tests::{FishShellEnv, spawn_in_process_host}` import, which made those two test helpers `pub(super)` within the file — no production visibility changed.
- No existing test was edited; no `#[allow(`, no new `unwrap`/`expect` outside `#[cfg(test)]`, no TODOs. The `Reviewer mutation` check was not exercised (reverting Task 2 would make the marker test hang to its 10 s timeout — that is the regression guard's design). Phase 06c (ghost wiring) untouched, as out of scope.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.43s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
r_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (572552): No such process
kill: (-573273): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M26-sbx-isolation/README.md` — +1 -1
- `docs/dev/milestones/M26-sbx-isolation/phase-06b-sbx-shell-lookup.md` — +47 -1
- `src/daemon/executor/shell_exec.rs` — +123 -25
- `src/shell/mod.rs` — +3 -2
- `src/shell/session.rs` — +283 -0

**Commit:** 91df5c7822cb5a27f131d880fc55b4ad573d8b90

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
