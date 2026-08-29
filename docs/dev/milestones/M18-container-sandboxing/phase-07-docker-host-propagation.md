# Phase 07: Carry `DOCKER_HOST` into the sandboxed command

**Milestone:** M18 — Container-sandboxed Agents
**Status:** review
**Depends on:** phase-04 (`run_args`, `stage_args`), phase-05 (`sandbox_window_command`)
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

**Sandboxed background execution is broken in production right now.** The
`docker` line that phase-05 puts into a `de-bg-*` window carries no
`DOCKER_HOST`, so it targets the *rootful* socket and fails — while phase-06's
preflight passes, because the daemon probes through a different path that
*does* set the variable. This phase makes the runtime endpoint travel with the
command as an argv flag, and adds a live test that runs with a scrubbed
environment so the gap cannot reopen.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "Config schema" — `docker_host`
  is `unix:///run/user/1000/docker.sock`, the rootless per-user socket. The
  rootful default (`/var/run/docker.sock`) is a *different daemon* and is not
  what this design targets.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-29, commit `a61f40b`):

- `cargo test --lib` → **1440 passed; 0 failed; 3 ignored**. Four gates green.
- **The three `#[ignore]`d live tests were run by the architect for the first
  time and all pass** — so the pieces individually work. `daemoneye sandbox
  build` was also executed for the first time: it built the image, wrote a
  well-formed `~/.daemoneye/etc/sandbox.lock`, and the recorded id matches
  `docker image inspect`. Preflight now passes through the full chain rather
  than via its `NoLock` escape.
- `grep -c '"--host"' src/daemon/executor/container.rs` → **0**.
- `run_args` currently returns a vector beginning `["run", "--rm", "--user",
  …]` (`container.rs`, pinned by
  `sandbox_exec_run_args_match_the_prototyped_vector` at ~line 897).
  `stage_args` likewise begins `["run", "--rm", "--user", "0:0", …]`.
- The three `.env("DOCKER_HOST", &cfg.docker_host)` calls in `container.rs`
  (lines ~146, ~380, ~398) are on `Command` objects the **daemon** spawns —
  the probe, the uid probe and the image inspect. They do not reach the
  window command, which is a *string* executed by tmux.

## Gotchas

Five traps. Items 1–3 are measured on this host.

1. **This is a real production break, verified end to end.** A live tmux pane
   on this machine reports `DOCKER_HOST=[UNSET]` — the tmux server inherits
   no such variable, and neither `tmux show-environment` nor
   `show-environment -g` carries it. Running the generated command without it:

   ```
   $ env -u DOCKER_HOST sh -c "docker run --rm --network none daemoneye-agent-base sh -lc 'echo x'"
   failed to connect to the docker API at unix:///var/run/docker.sock; check if
   the path is correct and if the daemon is running: dial unix
   /var/run/docker.sock: connect: no such file or directory
   ```

   Note `/var/run/docker.sock` — the **rootful** socket, a different daemon
   entirely. The preflight gate cannot catch this because it probes with
   `Command::env` set.

2. **`--host` must come BEFORE the subcommand.** Measured:

   ```
   $ docker --host unix:///run/user/1000/docker.sock run --rm … sh -lc 'echo host-flag-ok'
   host-flag-ok
   $ docker run --host unix:///run/user/1000/docker.sock --rm …
   unknown flag: --host
   ```

   So the flag is the **first** element of the argv vector, before `"run"`.

3. **Use the `--host` flag, not a `DOCKER_HOST=…` shell prefix.** Both work
   (both were measured), but the flag is ordinary argv: it flows through the
   same `sh_single_quote` path as every other element and carries no shell
   assignment semantics. A `VAR=value cmd` prefix would need separate quoting
   rules and would break if the command string is ever wrapped differently.

4. **This changes phase-04's pinned vector, deliberately.**
   `sandbox_exec_run_args_match_the_prototyped_vector` currently asserts the
   vector starts `["run", "--rm", …]`. It must be updated to the new prefix.
   That is the point of the phase — **update the expectation, do not work
   around it** by appending `--host` somewhere it does not belong.

5. **`cargo test --lib sandbox_host` passes today with zero tests.** Every
   criterion is a line count, not an exit status.

## Spec

### Task 1 — Emit `--host` from both argv builders

In `src/daemon/executor/container.rs`, make **both** `run_args` and
`stage_args` begin their vector with:

```
"--host", <cfg.docker_host>, "run", "--rm", …
```

i.e. two new elements prepended, before the existing `"run"`. Everything else
about both vectors is unchanged. Both builders must use `cfg.docker_host`
verbatim — no defaulting, no fallback.

`run_args` still returns an empty vector for an unparseable `run_as`; that
check happens first and is unchanged.

### Task 2 — Update the pinned vector

Update `sandbox_exec_run_args_match_the_prototyped_vector` so its expected
vector is the new one — `"--host"`, `"unix:///run/user/1000/docker.sock"`,
then `"run"`, `"--rm"`, and the rest exactly as before. Keep asserting the
**whole vector**, not a prefix.

Update `sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid` if
its assertions depend on element positions.

### Task 3 — Unit tests

Add the tests named in § Test plan. Every name must contain `sandbox_host`.

### Task 4 — A live test with a scrubbed environment

Add exactly one `#[ignore]`d test,
`sandbox_host_command_runs_with_no_ambient_docker_host`, marked
`#[ignore = "requires a running rootless Docker daemon"]`. It must:

1. Build the window command with `sandbox_window_command` for
   `echo scrubbed-ok`, using an `enabled = true` config.
2. Run it via `std::process::Command::new("sh").arg("-c").arg(&line)` with
   **`.env_remove("DOCKER_HOST")`** — this is the whole point: the test must
   prove the command works when the environment does *not* supply the
   endpoint.
3. Assert stdout contains `scrubbed-ok`.

`.env_remove("DOCKER_HOST")` is what makes this test able to fail. Without it
the test would pass on a developer machine that happens to export the
variable, and the production gap would stay invisible — which is exactly how
it survived phases 05 and 06.

The `#[ignore]` count becomes **4**.

### Task 5 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '"--host"'`
      prints `2` (**before: 0**) — one in `run_args`, one in `stage_args`.
      The `sed` scoping is required: the tests also contain the literal.
- [ ] `cargo test --lib sandbox_host 2>&1 | grep -c "^test .* ok$"` prints
      `3` — one per non-ignored test in § Test plan. A count, not an exit
      status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1443 passed; 0 failed; 4 ignored` (1440 + 3 new; ignored 3 → 4).
- [ ] `grep -c "#\[ignore" src/daemon/executor/container.rs` prints `4`
      (**before: 3**).
- [ ] `grep -c "env_remove" src/daemon/executor/container.rs` prints `1`
      (**before: 0**) — the live test scrubs the variable (§ Task 4).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `7` — **unchanged**. Staging is still unwired; do not add or
      remove any `#[allow]`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Three non-ignored tests plus the one ignored live test, all in
`container.rs`. Every name contains `sandbox_host`.

- `sandbox_host_run_args_start_with_the_configured_endpoint` — with
  `SandboxConfig::default()`, `run_args` returns a vector whose first three
  elements are exactly `["--host", "unix:///run/user/1000/docker.sock",
  "run"]`. **Negative half:** the vector must **not** contain `"--host"` more
  than once, and `"run"` must not be element 0.
- `sandbox_host_stage_args_start_with_the_configured_endpoint` — the same
  three-element prefix for `stage_args`, using a non-default
  `docker_host = "unix:///tmp/alt.sock"` so the test proves the value is read
  from config rather than hardcoded.
- `sandbox_host_window_command_carries_the_endpoint` — the string from
  `sandbox_window_command` with `enabled = true` contains
  `'--host' 'unix:///run/user/1000/docker.sock'` (each element separately
  single-quoted, as `sh_single_quote` renders them) and it appears **before**
  `'run'` in the string.

**Ignored:**

- `sandbox_host_command_runs_with_no_ambient_docker_host` — per Task 4.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_host tests (expect 3 lines) =="
cargo test --lib sandbox_host 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "--host in prod (want 2): "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '"--host"'
echo -n "ignore count (want 4):   "; grep -c "#\[ignore" src/daemon/executor/container.rs
echo -n "env_remove (want 1):     "; grep -c "env_remove" src/daemon/executor/container.rs
echo -n "allow(dead_code) tot:    "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
} > /tmp/e2e-07.txt 2>&1
cat /tmp/e2e-07.txt
```

Paste the contents of `/tmp/e2e-07.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-07-docker-host-propagation.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07.txt
diff /tmp/pasted-07.txt /tmp/e2e-07.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If any label in it has gone stale
against the criteria, that is a spec defect — record a blocker naming it
rather than editing the block, so the pasted evidence stays a faithful
capture of what the spec asked for.

## Authorizations

- Edit `src/daemon/executor/container.rs` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The live test is `#[ignore]`d; the
  architect runs it.
- **Do not add or remove any `#[allow(...)]`.**
- **Append to the Update Log; never edit or delete an existing entry.** Each
  entry is the historical record of the tree that produced it. If an earlier
  entry is now wrong or superseded, say so in a **new** entry — do not rewrite
  the old one, and do not describe an entry as still present if you have
  changed it.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause — record a blocker
  Update Log entry naming the exact criterion or failing test, and stop.
  Reporting the blocker *is* the successful outcome.** Do not proceed past a
  blocker you have filed.
- **Record what you decide, not what you wish had been decided.**

## Out of scope

- **Volume and container GC** — the `de-stage-*` leak phase-05 recorded, the
  `de.ghost=1` orphan sweep, `docker rm -f`. A later phase owns all of it.
- **Staging** — `stage_args` gains the `--host` prefix here but nothing calls
  it yet; `script_name_is_safe` stays unwired and the `#[allow(dead_code)]`
  stays with it.
- Ghost-specific container lifecycle, the escape hatch, the egress proxy,
  `Request::ContainerStatus`, the `log` relay opcode.
- **Foreground execution** — unchanged, host-level by design.
- Editing `run.rs`, `gc.rs`, `CLAUDE.md`, `README.md`,
  `assets/etc/config.toml`, or `containers/Dockerfile`.

## Update Log

### Update — 2026-08-29 02:42 (progress)

Started phase-07. Flipped Status: todo → in-progress; README phase-table row 07 updated to match. Implemented Task 1 (`--host` prepended in `run_args` and `stage_args`), Task 2 (pinned vector in `sandbox_exec_run_args_match_the_prototyped_vector` updated to the new prefix; `sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid`'s position-dependent `&args[..6]` slice widened to `[..8]`), and Tasks 3–4 (three `sandbox_host` unit tests + one `#[ignore]`d live scrubbed-env test) in `container.rs`. Also updated `sandbox_window_enabled_starts_with_the_quoted_runtime`, which asserted the old vector prefix and would otherwise have failed — the same deliberate phase-04 expectation change, applied to the window-line test that pinned the pre-fix prefix. Full lib suite: 1443 passed; 0 failed; 4 ignored. Now running the gates and the § End-to-end block.

### Update — 2026-08-29 02:45 (end-to-end verification)

```text
== A. sandbox_host tests (expect 3 lines) ==
test daemon::executor::container::tests::sandbox_host_stage_args_start_with_the_configured_endpoint ... ok
test daemon::executor::container::tests::sandbox_host_run_args_start_with_the_configured_endpoint ... ok
test daemon::executor::container::tests::sandbox_host_window_command_carries_the_endpoint ... ok
cargo_exit=0
== B. lib suite totals ==
test result: ok. 1443 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 3.90s
cargo_exit=0
== C. structural greps ==
--host in prod (want 2): 2
ignore count (want 4):   4
env_remove (want 1):     1
allow(dead_code) tot:    7
```

PASTE MATCH

### Update — ts=1787971565293 (complete, server-authored)

**Summary:** </think>Working tree is clean. Let me run through the completion checklist once more:

- [x] Phase doc Status: in-progress ✓ (I flipped it, didn't touch review)
- [x] Milestone README row: in-progress ✓
- [x] No hand-written (complete) entry ✓
- [x] cargo fmt --all run right before git add ✓ (actually run)
- [x] All verification commands clean: fmt, build, clippy, cargo test ✓
- [x] Final message will be substantive
- [x] git status --short shows nothing ✓
- [x] git log -1 --stat shows every file touched ✓

Now the final summary. Let me include the E2E verdict, grep proof, deviation notes.

Key notes for review:
- Extra test updated: `sandbox_window_enabled_starts_with_the_quoted_runtime` — phase-05 window-line test that pinned the pre-fix prefix would have failed; changed its assertion to the new prefix.
- PASTE MATCH confirmed.
- Live test not run (per authorization — architect runs it).</think>
**Summary + Notes for review**

Phase-07 complete: `--host <docker_host>` now heads the argv of both `run_args` and `stage_args`, so the tmux-executed window string carries the runtime endpoint and can no longer fall back to the rootful socket. The pinned vector in `sandbox_exec_run_args_match_the_prototyped_vector` was updated to the new prefix as the spec requires, and `sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid`'s position-dependent slice widened `[..6]` → `[..8]`. The three `sandbox_host` unit tests and the `#[ignore]`d scrubbed-env live test were added in `container.rs`; the live test is not run (per authorization — the architect runs it), and no `#[allow]`s were added or removed (`allow(dead_code)` total stays 7).

One deviation to flag: `sandbox_window_enabled_starts_with_the_quoted_runtime` — a phase-05 test that pinned the pre-fix window prefix (got `'docker' 'run' '--rm'`, want `'docker' '--host' … 'run' '--rm'`) — was also updated to the new prefix. The phase doc's Task list doesn't name this test, but its assertion is exactly the phase-04-pinned-vector-style expectation the phase changes deliberately, and leaving it red would violate "all gates green." Everything else was per-spec.

End-to-end verification (the § block, run verbatim) is pasted in its own `### Update — 2026-08-29 02:45 (end-to-end verification)` entry with the literal `PASTE MATCH` verdict; the block's own self-check diff printed `PASTE MATCH`. Criterion counts: `--host` in prod = 2, `sandbox_host` ok-lines = 3, lib suite = 1443 passed / 0 failed / 4 ignored, `#[ignore` count = 4, `env_remove` = 1, dead_code total = 7 — acceptance-criteria grep for `'--host' 'unix:///run/user/1000/docker.sock'` before `'run'` in the window string is exercised by `sandbox_host_window_command_carries_the_endpoint`. All four gates (`cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`) pass; commit `1001b46`; working tree clean.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nored; 0 measured; 0 filtered out; finished in 3.93s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
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
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M18-container-sandboxing/README.md` — +1 -1
- `docs/dev/milestones/M18-container-sandboxing/phase-07-docker-host-propagation.md` — +25 -1
- `src/daemon/executor/container.rs` — +128 -3

**Commit:** 1001b463f9c5e02503d0fe7a70b24b2032beb761

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
