# Phase 02 — rusqlite 0.41 and syn 3

**Status:** done
**Milestone:** M33 — Dependency refresh
**Drafted:** 2026-10-30
**Depends on:** phase 01 (done)
**Tags:** language=rust, kind=chore, size=s

## Goal

The two breaking upgrades that the survey found build clean land here,
so a failure points at one of two crates (M33 decision 2):

- **rusqlite** 0.40 → **0.41.1** (normal dependency, `bundled` feature
  kept). Its only users are `src/memory/index/` (`mod.rs`, `search.rs`,
  `reconcile.rs`, `tests.rs`).
- **syn** 2.0.119 → **3.0.9** (dev dependency, `full` and `visit` features
  kept). Its only user is `tests/hygiene.rs`, which parses every `.rs` file
  and enforces the line caps. Raise `proc-macro2` too if syn 3 requires a
  newer one.

When this phase is done, `Cargo.toml` requires both new versions,
`Cargo.lock` resolves them, and the suite still passes **exactly 2616**
tests (no test is added or removed).

## Pre-flight

- The survey's trial build of both upgrades compiled with no error. The
  memory-index tests and the hygiene gate have not been run against them;
  that is this phase's real check.
- You have no web access, but the new crates' source is on disk after the
  first build, under `~/.cargo/registry/src/*/rusqlite-0.41.1/` and
  `.../syn-3.0.9/`. Open it **only** to fix a failing build, clippy or
  test. If the gates are green, there is nothing to look up: do not diff
  or audit the crates.

## Spec

1. **Raise the requirements** — edit `Cargo.toml`: rusqlite to `0.41.1`,
   syn to `3.0.9`. Run `cargo update -p rusqlite -p syn` (and
   `-p proc-macro2` if needed). Do not run a bare `cargo update`.
2. **Fix what breaks** — build, clippy and test. Fix any failure in the
   files named in the Goal, using the changed API rather than a shim.
   A hygiene test that fails because syn 3 counts or visits differently
   is a real finding: report the cause, and keep the gate's meaning
   (the same caps, the same exemptions); do not loosen it to pass.
3. **Findings** — one `### Findings — <date>` entry, from exactly two
   commands and nothing else: `git diff --stat` and
   `git diff Cargo.lock | grep -E '^[-+](name|version) '`. Write in two or
   three lines what moved in the lockfile, and any gate that failed and
   how you fixed it — or `none`. Open no crate source for this step.
4. **End-to-end** — run the block below and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry.
5. **Commit** as `chore: upgrade rusqlite to 0.41 and syn to 3`.

## Acceptance criteria

- [ ] The E2E block's first line prints `rusqlite=0.41.1 syn=3.0.9 ` (the direct dependencies, per `cargo tree --depth 1`)
      (today `rusqlite=0.40.2 syn=2.0.119`).
- [ ] `allowlist=unchanged`, `build_warnings=0`, clippy clean,
      `fmt_exit=0`.
- [ ] `tests/hygiene.rs` and `tests/doc_truth.rs` pass, unchanged in what
      they enforce.
- [ ] `cargo test` reports exactly `passed=2616 failed=0`.
- [ ] No new `#[allow]`, `#[ignore]`, `unwrap()` in production, or
      `unsafe`.

## End-to-end verification

```sh
bash <<'E2E'
cargo tree --depth 1 -e normal,dev | grep -E '── (rusqlite|syn) v' | awk '{print $2"="$3}' | tr -d v | sort | tr '\n' ' '; echo
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

(The lockfile already holds a syn 3.0.9 pulled in by another crate, and
syn 2 may stay for others; the first line reads only DaemonEye's direct
dependencies.)

## Authorizations

- Edit `Cargo.toml`, `Cargo.lock` (via `cargo update -p`), the four
  `src/memory/index/` files and `tests/hygiene.rs`.

## Out of scope

- vt100 0.18 (phase 03). Any other crate's version.
- `tests/hygiene_allowlist.txt`: no entry may be added or grown.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-30

The first run (session-phase-02-6ae5287f) did steps 1 and 2 correctly, then
stalled after ~120 read-only calls diffing the rusqlite and syn sources for
Findings. The architect re-ran the gates on that tree: fmt clean,
`build_warnings=0`, clippy clean, `passed=2616 failed=0`, `cargo tree` at
rusqlite 0.41.1 and syn 3.0.9. **The upgrade is done; do not redo or audit
it.** What remains is steps 3–5 only: the Findings entry from the two
commands step 3 names (the lockfile also moved `libsqlite3-sys` to 0.39.1
and `tempfile`'s `getrandom` from 0.4.3 to 0.3.4 — say so), the E2E block
pasted, and the commit. Steps 3 and Pre-flight were tightened so that the
crate sources are read only to fix a red gate.

### Update — 2026-10-30 (escalation)

**Chosen lever:** resume
**Rationale:** the stalled tree is complete and green against every gate,
so only the bookkeeping is left; the one spec gap (an open-ended Findings
audit) is fixed in the doc that the resumed run reads.

### Update — 2026-10-30 23:37 (started)

Executor rexy (claude-opus-4-6). Status flipped `todo → in-progress` here and in
the milestone README's phase table. Next: raise the rusqlite and syn
requirements in `Cargo.toml`, `cargo update -p` them, then build, clippy and
test against the memory index and the hygiene gate.


### Findings — 2026-10-30

`git diff --stat`: only `Cargo.toml` and `Cargo.lock` carry the upgrade
(4 and 12 lines); no source file changed, so nothing in rusqlite 0.41 or
syn 3 broke the build, clippy or tests. `git diff Cargo.lock`:
rusqlite 0.40.2 → 0.41.1, with libsqlite3-sys 0.38.2 → 0.39.1 underneath,
and syn 2.0.119 → 3.0.9; tempfile's getrandom also moved 0.4.3 → 0.3.4
(resolution side-effect, not a requirement change). Gates: none failed —
build, clippy, fmt, hygiene, doc_truth and the full suite (passed=2616
failed=0) were already green on this tree.

### Update — 2026-10-30 (end-to-end verification)

```text
rusqlite=0.41.1 syn=3.0.9 
allowlist=unchanged
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.78s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2616 failed=0
```

### Update — 2026-10-31 00:01 (complete, server-authored)

**Summary:** **Summary:** Resumed phase 02 at steps 3–5, exactly as the architect's guidance scoped it — no source, `Cargo.toml`, or lockfile edits, and no crate-source audit. The Findings entry was written from only the two spec-named commands (`git diff --stat`: Cargo.toml/Cargo.lock only, no source change; `git diff Cargo.lock`: rusqlite 0.40.2→0.41.1 with libsqlite3-sys 0.38.2→0.39.1, syn 2.0.119→3.0.9, and tempfile's getrandom 0.4.3→0.3.4 as a resolution side-effect). The E2E block ran once and its verbatim output is pasted in the end-to-end verification entry. Committed as `chore: upgrade rusqlite to 0.41 and syn to 3` (781d9854); `git status --short` is clean and the commit contains all four touched files.

**Notes for review:** E2E confirms every acceptance criterion: first line `rusqlite=0.41.1 syn=3.0.9`, `allowlist=unchanged`, `build_warnings=0`, clippy clean, `fmt_exit=0`, hygiene 16 passed, doc_truth 16 passed, and the full suite `passed=2616 failed=0` — no test added or removed, no gate fix needed (Findings says `none` on failures). Phase status left at `in-progress` with no hand-written `(complete)` entry, per the lifecycle split.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST

test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.08s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g4_briefing_injection_block_format ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_entry_format ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-6252e26b6ec9889b)
kill: (1322729): No such process
kill: (-1323456): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-27a608fb4790b1c9)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-2c50982d98d70d1c)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6c982ebc971170c8)
     Running tests/hygiene.rs (target/debug/deps/hygiene-d59fc8118e8ecc0d)
     Running tests/integration.rs (target/debug/deps/integration-1cdcdd5c7e5c0780)
     Running tests/isolation.rs (target/debug/deps/isolation-d7c0f2abbac7f554)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-d0a008f93be4cbf4)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-a585ff67b6d9c31a)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M33-dependency-refresh/phase-02-rusqlite-and-syn.md` — +61 -6

**Commit:** 781d985483fbf541cd7dbe2e1e05d714d3ee8c3b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-31

- **Verdict:** approved_after_1
- **Bounces:** 1 (the first run's `NoProgressStall` hard-fail, resumed with `continue_phase`; no bug filed)
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none. `src/`, `tests/` and the allowlist are unchanged; the upgrade needed no code change.
- **Calibration:** `spec_bug`. The architect's Findings step ("anything the upgrade changed in behaviour") and its Pre-flight pointer at the crate sources had no end. The first run finished the upgrade green, then spent about 120 read-only calls diffing the two crates. Bounded to two named commands, the resume finished in 17 turns. Every Spec step must name its commands and the output that ends it.
