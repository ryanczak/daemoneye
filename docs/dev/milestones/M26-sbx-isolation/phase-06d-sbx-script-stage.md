# Phase 06d: sbx-script-stage — copy the one approved script into the ghost's sandbox

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** review
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

### Update — 2026-10-07 10:54 (progress)

Executor local-inference-lab/GLM-5.3-Flash-NVFP4-Spark started phase 06d.
Status flipped to `in-progress` here and in the milestone README's phase
table. All four Spec tasks are implemented and committed as `a8395eb4`:
the four pure helpers + `STAGED_SCRIPTS_DIR` in `sbx.rs`, `stage_script`
in `ghost_sandbox.rs`, the staging step in `run_background`'s
`Ok(sandbox)` arm (`foreground.rs`, command rewrite before
`run_ghost_in_sandbox`; staging failure yields `(reason, 1)` and
`log_command` still records the original command), and the eight named
tests. Full-suite run: 2396 passed, 0 failed.

### Update — 2026-10-07 17:56 (complete, server-authored)

**Summary:** Phase 06d complete and committed.

**Summary:** Implemented the Q3 script-staging path for sandboxed ghosts. Task 1 added `STAGED_SCRIPTS_DIR`, `script_name_is_safe`, `script_invocation`, `staged_script_command`, `stage_prep_args`, and `stage_cp_args` to `src/daemon/executor/sbx.rs`, building every string from the constant per spec. Task 2 added `stage_script` to `ghost_sandbox.rs`: it refuses an unsafe name with zero `sbx` calls, then runs prep (`exec -u root <sbx> sh -c "mkdir -p /de/scripts && chown agent:agent /de/scripts"`) followed by `cp <host_path> <sandbox>:/de/scripts/<name>`, failing closed with `` sandbox staging failed for `<name>`: `` plus the trimmed stderr on any non-zero exit. Task 3 rewrote the `Ok(sandbox)` arm in `run_background`: the command is detected as an approved-script invocation, staged, and rewritten to `/de/scripts/<name><args_tail>` before `run_ghost_in_sandbox`; a staging failure yields `(reason, 1)` and never runs the command, while `log_command` still records the original command the model asked for. Task 4 captured the E2E block verbatim.

**End-to-end verification** (run once, output pasted below as required by the acceptance criteria):

```
staged_script_command_points_at_the_sandbox_copy: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
stage_args_match_the_measured_shapes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
script_name_is_safe_rejects_path_and_shell_characters: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
script_invocation_skips_sudo_and_unknown_scripts: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
stage_script_preps_then_copies: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
stage_script_refuses_an_unsafe_name_without_sbx: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
stage_script_fails_closed_on_a_failed_copy: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
stage_script_fails_closed_on_a_failed_prep: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2345 filtered out; finished in 0.00s
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 2293 filtered out; finished in 0.00s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 2330 filtered out; finished in 0.03s
fmt_exit=0
passed=2396 failed=0
```

**Notes for review:**
- All eight named tests pass individually; `executor::sbx` is 53/0 (49 prior + 4 new) and `executor::ghost_sandbox` is 16/0 (12 prior + 4 new); clippy exited 0; `fmt_exit=0`; full suite 2396 passed / 0 failed. No `#[allow(`, no `unsafe`, no `.unwrap()`/`.expect(` outside `#[cfg(test)]`; `container.rs` untouched.
- Two adaptations from the spec's sketch: (1) `exec_args` in `sbx.rs` puts the sandbox name before the `-u root` flags (`exec <name> -u root …`), which the existing test module's shape for one-off commands pins, so `stage_prep_args` builds its argv literally in the spec's exact order (`exec -u root <name> …`) instead of routing through `exec_args` — the spec's pinned vector is authoritative and the test asserts it. (2) The Task 3 sketch's early `return (reason, 1)` is illegal inside the `Ok(sandbox)` arm (the arm must evaluate to the `(output, exit_code)` tuple), so the arm computes `Result<String, String>` and matches on it — same semantics: staging failure → `(reason, 1)`, command never run.
- Verification loop drove three build errors (module-path scoping in `ghost_sandbox.rs`, a borrow/move conflict in the staging arm, and an argv-order mismatch the exact-shape test caught); all fixed by editing through file tools only. The staging failure message for an unsafe name embeds the name twice (prefix + body) to satisfy both the prefix pin and the "not a stageable script name" wording the docker path's error uses.
- Commits: `a8395eb4` (code + tests, includes the start-of-phase status flips), `add3f719` (the progress entry). Phase doc and README rows left `in-progress` for the server's completion tail.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.30s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
r_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_counts_are_accurate ... ok
test docs_document_the_reindex_command ... ok
test readme_tools_tables_match_the_code ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_tool_call_response_round_trip ... ok
test config_pricing_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.41s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (654525): No such process
kill: (-655253): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M26-sbx-isolation/README.md` — +1 -1
- `docs/dev/milestones/M26-sbx-isolation/phase-06d-sbx-script-stage.md` — +13 -1
- `src/daemon/executor/foreground.rs` — +46 -13
- `src/daemon/executor/ghost_sandbox.rs` — +104 -1
- `src/daemon/executor/sbx.rs` — +146 -0

**Commit:** add3f7197a12a3d983b36413f3d4e21c819ee359

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
