# Phase 08: Label every sandboxed container, and sweep what leaks

**Milestone:** M18 — Container-sandboxed Agents
**Status:** done
**Depends on:** phase-04 (`run_args`), phase-07 (`--host` prefix)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Two things leak today, both measured. Sandboxed containers carry **no labels
at all**, so nothing can identify them for a sweep — and a container whose
`docker` client is killed keeps running with `--rm` never firing. Separately,
every sandboxed run leaves a `de-stage-*` volume behind. This phase labels
every sandboxed container and sweeps both leaks at daemon start.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D3 — Container lifecycle":
  ghost containers are labelled `de.ghost=1` for the orphan sweep. This phase
  generalises that — **every** sandboxed container gets a label, because a
  sweep that can only find ghosts cannot clean up the background containers
  that actually exist today.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree and the live runtime at drafting time (2026-08-29,
commit `3605e3e`):

- `cargo test --lib` → **1443 passed; 0 failed; 4 ignored**. Four gates green.
  All four `#[ignore]`d live tests pass when run by the architect.
- `run_args` emits `--label de.ghost=1` **only when `spec.is_ghost`**, and the
  one call site (`run.rs`) hardcodes `is_ghost: false`. So in practice
  **no sandboxed container is labelled**. Measured:
  `docker inspect … --format '{{json .Config.Labels}}'` → `{}`.
- `grep -c "de.sandbox" src/daemon/executor/container.rs` → **0**.
- `cargo test --lib sandbox_gc` → **0** test lines (the vacuity trap).
- `src/daemon/mod.rs:482` is `startup_config.sandbox.validate();` — the
  startup hook this phase extends.
- Two stale `de-stage-*` volumes exist on this host right now
  (`de-stage-e2e-1041632`, `de-stage-scrub-1076883`), left by the architect's
  live tests. They are deliberately **not** cleaned up: they are the fixture
  the milestone-close live check will sweep.

## Gotchas

Six traps. Items 1–4 were measured against the live runtime; the executor has
no runtime and cannot reproduce them.

1. **`docker volume ls --filter name=X` is a SUBSTRING match, not a prefix
   match.** Measured — a decoy volume named `zz-de-stage-decoy` **matched**
   `--filter name=de-stage-`. Relying on docker's filter to select what to
   delete would destroy a user volume whose name merely contains the string.
   **Do the prefix check in Rust**: list volume names, then keep only those
   where `name.starts_with("de-stage-")`. A test pins the decoy as a
   must-NOT-match.

2. **A killed `docker` client leaves the container running and `--rm` never
   fires.** Measured: `SIGKILL` to the client, then
   `docker ps --filter name=…` still reports `Up 3 seconds`. So a crashed
   daemon or a killed pane orphans a live container — which is exactly what
   the sweep is for, and why `--rm` alone is not enough.

3. **Label filtering works and is exact.** Measured:
   `docker ps -aq --filter label=de.sandbox=1` returns the labelled container
   and does **not** pick up an unlabelled one started alongside it. So the
   label is a sound sweep key — but only if every sandboxed container
   actually carries it, which is Task 1.

4. **This changes the pinned vector again.** Phase-07 already moved it once.
   `sandbox_exec_run_args_match_the_prototyped_vector` must be updated to
   include the new unconditional label. **Update the expectation; do not work
   around it.**

5. **The sweep must never run when the sandbox is disabled.** With
   `[sandbox] enabled = false` the daemon must not shell out to `docker` at
   all — that is the promise every phase has kept so far, and a startup sweep
   is the easiest place to break it.

6. **`cargo test --lib sandbox_gc` passes today with zero tests.** Every
   criterion is a line count, not an exit status.

## Spec

### Task 1 — Label every sandboxed container

In `run_args` (`src/daemon/executor/container.rs`), emit
`"--label", "de.sandbox=1"` **unconditionally**, immediately before the
existing conditional ghost label. The ghost label stays exactly as it is:
`--label de.ghost=1` only when `spec.is_ghost`. A ghost container therefore
carries **both** labels; a background container carries only `de.sandbox=1`.

Do the same in `stage_args` — the staging helper is a sandboxed container too
and must be sweepable.

### Task 2 — Update the pinned vector

Update `sandbox_exec_run_args_match_the_prototyped_vector` to the new expected
vector (the label pair now sits where § Task 1 places it), and
`sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid` if its
position-dependent slice moves. Keep asserting the **whole** vector for the
former.

`sandbox_exec_run_args_label_ghost_jobs` must still hold: with
`is_ghost: true` the vector contains `de.ghost=1`; with `is_ghost: false` it
does **not**. Extend it so it also asserts `de.sandbox=1` is present in
**both** cases — that is the new invariant.

### Task 3 — The sweep argv builders (pure)

```rust
/// argv listing every container this daemon's sandbox created, running or not.
pub fn sweep_container_list_args(cfg: &SandboxConfig) -> Vec<String>

/// argv force-removing the given container ids.
pub fn sweep_container_rm_args(cfg: &SandboxConfig, ids: &[String]) -> Vec<String>

/// argv listing every volume name known to the runtime.
pub fn sweep_volume_list_args(cfg: &SandboxConfig) -> Vec<String>

/// argv removing the given volumes.
pub fn sweep_volume_rm_args(cfg: &SandboxConfig, names: &[String]) -> Vec<String>
```

All four begin with `--host`, `<cfg.docker_host>` exactly as phase-07
established, then:

- list containers: `ps`, `-aq`, `--filter`, `label=de.sandbox=1`
- remove containers: `rm`, `-f`, then each id
- list volumes: `volume`, `ls`, `-q` — **no `--filter`** (§ Gotchas 1)
- remove volumes: `volume`, `rm`, then each name

Both `rm` builders return an **empty vector** when given an empty slice — a
bare `docker rm -f` with no arguments is an error, and running it on every
clean startup would log noise forever.

### Task 4 — Select stale volumes (pure)

```rust
/// The subset of `names` that are sandbox staging volumes.
/// Prefix match only — `docker`'s own `--filter name=` is a substring match
/// and would select a user volume that merely contains the string.
pub fn stale_stage_volumes(names: &[String]) -> Vec<String>
```

Keep a name only when `name.starts_with("de-stage-")`. Preserve input order.

### Task 5 — The impure sweep, called at startup

```rust
/// Remove orphaned sandbox containers and staging volumes. Best-effort:
/// every failure is logged and none is fatal — a sweep that cannot run must
/// never stop the daemon from starting.
pub fn sweep_sandbox_leftovers(cfg: &SandboxConfig)
```

Return immediately when `!cfg.enabled` (§ Gotchas 5). Otherwise run the four
argv vectors through `crate::tmux::bounded_output_with(&mut cmd,
Duration::from_secs(30))`, splitting each list command's stdout on newlines
and discarding empty lines. Log a single `log::info!` naming how many
containers and how many volumes were removed; log a `log::warn!` per failed
command and continue.

Call it from `src/daemon/mod.rs` immediately after the existing
`startup_config.sandbox.validate();` at line 482.

### Task 6 — Unit tests

Add the tests named in § Test plan. Every name must contain `sandbox_gc`.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '"de.sandbox=1"'`
      prints `2` (**before: 0**) — one in `run_args`, one in `stage_args`.
      The `sed` scoping is required; the tests also contain the literal.
- [ ] `grep -c "pub fn stale_stage_volumes" src/daemon/executor/container.rs`
      prints `1` (**before: 0**).
- [ ] `grep -c "pub fn sweep_sandbox_leftovers" src/daemon/executor/container.rs`
      prints `1` (**before: 0**).
- [ ] `grep -c "sweep_sandbox_leftovers" src/daemon/mod.rs` prints `1`
      (**before: 0**) — the single startup call site.
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c 'filter name='`
      prints `0` (**before: 0**, must stay `0`) — volume selection is a Rust
      prefix check, never docker's substring filter (§ Gotchas 1).
- [ ] `cargo test --lib sandbox_gc 2>&1 | grep -c "^test .* ok$"` prints `7` —
      one per non-ignored test in § Test plan. A count, not an exit status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1450 passed; 0 failed; 4 ignored` (1443 + 7 new; ignored unchanged —
      this phase adds no `#[ignore]`).
- [ ] `grep -c "#\[ignore" src/daemon/executor/container.rs` prints `4`
      (**unchanged**).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `7` — **unchanged**. Staging is still unwired; do not add or
      remove any `#[allow]`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Seven tests, all in `container.rs`. Every name contains `sandbox_gc`.

**Labelling:**

- `sandbox_gc_every_container_carries_the_sandbox_label` — with
  `is_ghost: false`, `run_args` contains `"de.sandbox=1"` and does **not**
  contain `"de.ghost=1"`; with `is_ghost: true` it contains **both**.
- `sandbox_gc_stage_args_carry_the_sandbox_label` — `stage_args` contains
  `"de.sandbox=1"`.

**Volume selection — the § Gotchas 1 guard:**

- `sandbox_gc_selects_only_stage_prefixed_volumes` — given
  `["de-stage-a", "zz-de-stage-decoy", "de-stage-b", "unrelated",
  "de-stagex", "de-stage-"]`, the result is exactly
  `["de-stage-a", "de-stage-b", "de-stage-"]`, in that order. **Every
  negative here is load-bearing**: `zz-de-stage-decoy` is the name docker's
  own filter wrongly matched (measured), `de-stagex` is a near-miss without
  the trailing hyphen, and `unrelated` is the control. `de-stage-` itself is
  a legitimate (degenerate) match.
- `sandbox_gc_selects_nothing_from_an_empty_list` — an empty slice yields an
  empty vector.

**Argv builders:**

- `sandbox_gc_container_list_args_filter_by_label` — the vector is exactly
  `["--host", "unix:///run/user/1000/docker.sock", "ps", "-aq", "--filter",
  "label=de.sandbox=1"]`.
- `sandbox_gc_volume_list_args_do_not_filter` — the vector is exactly
  `["--host", "unix:///run/user/1000/docker.sock", "volume", "ls", "-q"]`.
  **Assert the vector contains no element starting with `--filter`** — the
  selection happens in Rust (§ Gotchas 1).
- `sandbox_gc_rm_args_are_empty_for_an_empty_slice` — both
  `sweep_container_rm_args` and `sweep_volume_rm_args` return an empty vector
  for `&[]`, and a non-empty one for a single id/name (so the test cannot
  pass by always returning empty).

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_gc tests (expect 7 lines) =="
cargo test --lib sandbox_gc 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "de.sandbox=1 in prod (2): "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '"de.sandbox=1"'
echo -n "stale_stage_volumes (1):  "; grep -c "pub fn stale_stage_volumes" src/daemon/executor/container.rs
echo -n "sweep fn (1):             "; grep -c "pub fn sweep_sandbox_leftovers" src/daemon/executor/container.rs
echo -n "startup call site (1):    "; grep -c "sweep_sandbox_leftovers" src/daemon/mod.rs
echo -n "no docker name filter (0):"; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c 'filter name='
echo -n "ignore count (4):         "; grep -c "#\[ignore" src/daemon/executor/container.rs
echo -n "allow(dead_code) tot (7): "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
} > /tmp/e2e-08.txt 2>&1
cat /tmp/e2e-08.txt
```

Paste the contents of `/tmp/e2e-08.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-08-sandbox-gc.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08.txt
diff /tmp/pasted-08.txt /tmp/e2e-08.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block, so the pasted evidence stays a faithful capture.

## Authorizations

- Edit `src/daemon/executor/container.rs` and `src/daemon/mod.rs`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. Nothing in this phase needs a
  runtime: every argv is returned as data, and the impure sweep is exercised
  only by the architect at milestone close.
- **Do not add or remove any `#[allow(...)]`**, and **do not add any
  `#[ignore]`** — the count stays at 4.
- **Append to the Update Log; never edit or delete an existing entry.** If an
  earlier entry is wrong or superseded, say so in a **new** entry.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause — record a blocker
  Update Log entry naming the exact criterion or failing test, and stop.
  Reporting the blocker *is* the successful outcome.** Do not proceed past a
  blocker you have filed.
- **Record what you decide, not what you wish had been decided.**

## Out of scope

- **Staging.** `stage_args` gains the label here, but nothing calls it;
  `script_name_is_safe` stays unwired and the `#[allow(dead_code)]` stays
  with it.
- **Per-ghost container lifecycle** — setting `is_ghost: true` at a call
  site, ghost-scoped teardown. Nothing sets it yet; this phase only makes
  ghosts *sweepable* when they arrive.
- **Sweeping on any schedule other than daemon start** — no timer, no
  post-run hook.
- The escape hatch, the egress proxy, `Request::ContainerStatus`, the `log`
  relay opcode.
- Editing `run.rs`, `gc.rs`, `CLAUDE.md`, `README.md`,
  `assets/etc/config.toml`, or `containers/Dockerfile`.

## Update Log

### Update — 2026-08-29 02:50 (started)

Started phase-08. Flipped Status: todo → in-progress; README phase-table row 08 updated to match. Working through Spec Task 1 (unconditional `de.sandbox=1` label in `run_args` + `stage_args`), Task 2 (update pinned vectors + ghost-label test), Tasks 3–5 (four sweep argv builders, `stale_stage_volumes`, `sweep_sandbox_leftovers` + startup call site in `src/daemon/mod.rs`), Task 6 (seven `sandbox_gc` tests), Task 7 (E2E capture). Full lib suite currently 1443 passed; 0 failed; 4 ignored.

### Update — 2026-08-29 03:10 (end-to-end verification)

```text
== A. sandbox_gc tests (expect 7 lines) ==
test daemon::executor::container::tests::sandbox_gc_selects_nothing_from_an_empty_list ... ok
test daemon::executor::container::tests::sandbox_gc_container_list_args_filter_by_label ... ok
test daemon::executor::container::tests::sandbox_gc_rm_args_are_empty_for_an_empty_slice ... ok
test daemon::executor::container::tests::sandbox_gc_every_container_carries_the_sandbox_label ... ok
test daemon::executor::container::tests::sandbox_gc_selects_only_stage_prefixed_volumes ... ok
test daemon::executor::container::tests::sandbox_gc_volume_list_args_do_not_filter ... ok
test daemon::executor::container::tests::sandbox_gc_stage_args_carry_the_sandbox_label ... ok
cargo_exit=0
== B. lib suite totals ==
test result: ok. 1450 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 4.02s
cargo_exit=0
== C. structural greps ==
de.sandbox=1 in prod (2): 2
stale_stage_volumes (1):  1
sweep fn (1):             1
startup call site (1):    1
no docker name filter (0):0
ignore count (4):         4
allow(dead_code) tot (7): 7
```

PASTE MATCH

### Update — ts=1787973102202 (complete, server-authored)

**Summary:** </think>That `eprintln!` is pre-existing code from before my phase (it's in mod.rs but not mine — it's part of `init_logging`). That's fine, not a debug call I added. My changes don't include debug calls or TODO comments. Let me verify my own additions don't contain `println!` — they don't.

Now run the completion checklist:
- [x] Phase doc Status: in-progress (I flipped it)
- [x] Milestone README row: in-progress
- [x] No hand-written (complete) entry — I wrote started + e2e entries only
- [x] cargo fmt --all run immediately before git add (actually run)
- [x] All verification commands clean
- [x] Final message substantive
- [x] git status --short shows nothing
- [x] git log -1 --stat shows every file touched (3 files: container.rs, mod.rs, phase doc)

Now write the final summary.</think>
All tasks complete. Final checklist confirmation:

- Phase doc Status: `in-progress` (I flipped it; did not touch `review`).
- README phase-table row 08: `in-progress` (started 2026-08-29).
- No hand-written `(complete)` entry — the Update Log holds only my started entry and the E2E entry with the `PASTE MATCH` verdict (self-check diff printed `PASTE MATCH`).
- `cargo fmt --all` run twice immediately before `git add` (fence: after E2E block too); all four gates green.
- Working tree clean; `git log -1 --stat` shows the commit touching `container.rs`, `mod.rs`, phase doc.

**Summary + Notes for review**

Phase-08 complete: every sandboxed container now carries `--label de.sandbox=1` (run_args + stage_args), the ghost label stays conditional behind it, and a best-effort `sweep_sandbox_leftovers` runs once at daemon start after sandbox validation. The four sweep argv builders are pure data, volume selection is a Rust prefix check (`stale_stage_volumes`), and the `--filter name=` substring trap is absent from production code (`filter name=` count = 0 in the pre-test region). `#[ignore]` stays 4, `allow(dead_code)` total stays 7 — no allow/ignore added or removed.

Deviations (all signal, none blocking): (1) two syntax fixes in `sweep_sandbox_leftovers` — `Vec<u8>` has no `.lines()`, switched to `String::from_utf8_lossy(...).lines()`; (2) reworded `stale_stage_volumes`'s doc comment to avoid the literal `filter name=` — the acceptance criterion greps production-only and the spec's own doc text is pre-`#[cfg(test)]`; a doc comment is "code the phase ships", so keeping it literal-free holds the `0` count honestly since the selection genuinely happens in Rust.

End-to-end evidence captured in its own Update Log entry (`### Update — 2026-08-29 03:10 (end-to-end verification)`), including the literal `PASTE MATCH` verdict line: all seven `sandbox_gc` ok-tests, lib suite 1450 passed / 0 failed / 4 ignored, `de.sandbox=1` in prod = 2, `stale_stage_volumes` = 1, sweep fn = 1, startup call site = 1, no docker name filter in prod = 0, ignore count = 4, dead_code total = 7. All four gates (`cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`) pass; commit `0d5009d`; working tree clean. Live sweep behavior is not exercised here per authorization — the architect runs it at milestone close against the two stale volumes.

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
nored; 0 measured; 0 filtered out; finished in 4.06s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_tools_tables_match_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
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
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

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
- `docs/dev/milestones/M18-container-sandboxing/phase-08-sandbox-gc.md` — +33 -1
- `src/daemon/executor/container.rs` — +293 -6
- `src/daemon/mod.rs` — +4 -0

**Commit:** 0d5009d7d94778558654eb572d7dd08fd73a05cd

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-29

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** deepseek-v4-flash-0731
- **Scope deviations:** two, both declared, both accepted. (1) Two syntax
  fixes in `sweep_sandbox_leftovers` — `Vec<u8>` has no `.lines()`, so
  `String::from_utf8_lossy(..).lines()`. Ordinary implementation work.
  (2) The doc comment on `stale_stage_volumes` was reworded to avoid the
  literal `filter name=`. **That was forced by an architect defect** — see
  below.
- **Calibration:** one, architect-side, and it is the **fourth occurrence** of
  a family already on the fold queue.

**The data-destroying slip is guarded.** The mutation that matters here is one
character wide, and it bites:

| Mutation | Result |
|---|---|
| `starts_with("de-stage-")` → `contains("de-stage-")` | **FAILED** `sandbox_gc_selects_only_stage_prefixed_volumes` |
| empty-slice guard removed from `sweep_container_rm_args` | **FAILED** `sandbox_gc_rm_args_are_empty_for_an_empty_slice` |

The first is the one that would delete a user's volume — `zz-de-stage-decoy`
is a real name docker's own `--filter name=` matched when measured, so the
negative case is not hypothetical. The disabled short-circuit is present at
the top of `sweep_sandbox_leftovers`, so a disabled sandbox still spawns
nothing.

Also confirmed: four gates green; **1450 passed / 0 failed / 4 ignored**;
`de.sandbox=1` twice in production; the sweep wired at exactly one startup
site; `filter name=` absent from production code; `#[ignore]` still 4;
`allow(dead_code)` still 7; no `unwrap`/`expect` in the new production code;
Update Log **appended**, not rewritten; E2E artifact re-extracts identical
apart from the elapsed-time line.

**Calibration — architect-side, 4th of its family.** § Task 4 dictated this
doc comment verbatim:

```
/// Prefix match only — `docker`'s own `--filter name=` is a substring match
```

while § Acceptance criteria required `filter name=` to appear **zero** times
in production code. The spec therefore instructed the executor to write the
exact string the spec forbids. It reworded the comment to
*"docker's own name filter is a substring match"* — same meaning, criterion
satisfied — and disclosed it, which is the right handling of a spec that
contradicts itself in a way that is still satisfiable.

This is the same family as the phase-02 bounce criterion that quoted its own
sentence and the phase-04 criteria that counted the phase's own test vector:
**a criterion must not forbid a string that the phase itself is told to
write.** Recorded in the milestone README, where it is a standing fold
candidate for PE at close.
