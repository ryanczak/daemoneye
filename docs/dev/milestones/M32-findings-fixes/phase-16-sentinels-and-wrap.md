# Phase 16 — Sentinels and wrap

**Status:** review
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-15
**Estimated diff:** ~93 insertions, ~25 deletions
**Tags:** language=rust, kind=bugfix, size=s
**Findings:** M32 `FINDINGS.md` #10, #7

## Goal

Two defects, added by the PE on 2026-10-30 after the M32 findings review.
After this phase:

- **M32 #10 (bug):** the remote `edit`, the sudo `edit` and the remote
  `create` read their result through `remote_write_outcome`
  (`src/daemon/executor/file_ops/mod.rs`), as `delete` and `copy` already
  do. They scanned the output for any line *containing* `DE_OK:`, checked
  before `DE_ERROR:`. A failure line that names a path holding `DE_OK:`
  therefore read as success, so a create onto an existing
  `DE_OK:exists.txt` replied `Created …` and wrote nothing.
  `remote_write_outcome` counts only lines before `__DE_DONE__` that
  *start* with a sentinel. Each site keeps its own replies, and the sudo
  edit keeps its loud "did not run" error when neither sentinel appears.
- **M32 #7 (bug):** `wrap_words` (`src/cli/viewer_layout.rs`) budgets
  the first row by subtracting the leading whitespace's **byte** length
  from a width counted in chars. A full-width space (U+3000, 3 bytes)
  wrapped the first row two chars early. The byte length stays for
  `split_at`, and the budget counts chars.

The suite goes from **2612** to **exactly 2615**: three tests added, none
removed.

## Architecture references

- `src/daemon/executor/file_ops/mod.rs` § `remote_write_outcome` and its
  tests: the sentinel contract.
- M32 README, decisions 1 and 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-16-sentinels-and-wrap.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of the commit that
   drafted this phase: fmt and clippy clean, hygiene passing, the full
   suite at `passed=2615 failed=0`. It asserts its own anchors and prints
   three lines, the last `wrap_words counts leading whitespace in chars`.
   If an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `fix:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-16-sentinels-and-wrap.py`
(sha256 begins `a4bc15c28f5ebad1`), kept in the repo as the record of the
change.

### The red run is done — do not repeat it

The three new tests were run against the pre-phase code at drafting. Each
failed on the defect it names:

```
a_failed_create_on_a_sentinel_path_is_an_error ... FAILED
  the failed create is reported as a failure: "Created /tmp/.tmpng9nMM/DE_OK:exists.txt on host web01."
a_failed_edit_on_a_sentinel_path_is_an_error ... FAILED
  the failed edit is reported as a failure: "Edited /tmp/.tmpxCe1ZA/DE_OK:probe.txt on host web01."
wrap_words_counts_leading_whitespace_in_chars ... FAILED
  left: ["\u{3000}abcd", "efgh"]
 right: ["\u{3000}abcd efgh"]
```

The sudo edit has no red test of its own: its swap is the same parser,
and the existing root-edit tests, which cover the refused-password path
the loud error serves, pass with it.

## Acceptance criteria

- [ ] The End-to-end block prints
      `ok_scans=0 outcome_calls=3 sentinel_tests=2 wrap_chars=1 wrap_test=1`
      (today: `ok_scans=3 outcome_calls=0 sentinel_tests=0 wrap_chars=0
      wrap_test=0`).
- [ ] `cargo test` reports **exactly** `passed=2615 failed=0` (today
      2612).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "ok_scans=$(grep -c 'line.contains("DE_OK:")' src/daemon/executor/file_ops/ops.rs) outcome_calls=$(grep -c 'super::remote_write_outcome(&' src/daemon/executor/file_ops/ops.rs) sentinel_tests=$(grep -cE 'fn a_failed_(create|edit)_on_a_sentinel_path_is_an_error' src/daemon/executor/file_ops/ops.rs) wrap_chars=$(grep -c 'leading.chars().count()' src/cli/viewer_layout.rs) wrap_test=$(grep -c 'fn wrap_words_counts_leading_whitespace_in_chars' src/cli/viewer.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: the shape line above, `allowlist=unchanged`, hygiene
`ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`,
`passed=2615 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-16-sentinels-and-wrap.py`; do not edit it.
- Edit the three files it names, through the script only:
  `src/daemon/executor/file_ops/ops.rs`, `src/cli/viewer_layout.rs`,
  `src/cli/viewer.rs`.

## Out of scope

- Splitting `run_edit_with_spawn` or `run_create_with_spawn`, both
  allowlisted: the function milestone.
- The perl branch's `DE_ERROR` lines, which do not name the path.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 21:09 (started)

**Started by:** executor (phase 16 run)
### Update — 2026-10-30 21:09 (cuts done)

`python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-16-sentinels-and-wrap.py && cargo fmt --all`:

```
the remote edit, sudo edit and create read remote_write_outcome
two tests pin a failure on a sentinel path
wrap_words counts leading whitespace in chars
```
### Findings — 2026-10-30

`git diff --stat`: 5 files changed, 108 insertions(+), 27 deletions(-) — the script's edits to `src/daemon/executor/file_ops/ops.rs`, `src/cli/viewer_layout.rs`, `src/cli/viewer.rs`, plus the phase-doc status flip, the started and cuts-done entries and the README phase-table row.
### Update — 2026-10-30 (end-to-end verification)

```
ok_scans=0 outcome_calls=3 sentinel_tests=2 wrap_chars=1 wrap_test=1
allowlist=unchanged
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.98s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.55s
fmt_exit=0
passed=2615 failed=0
```

### Update — 2026-10-30 21:15 (complete, server-authored)

**Summary:** **Summary:** Phase 16 executed via the phase script (`phase-16-sentinels-and-wrap.py`, three printed lines, last `wrap_words counts leading whitespace in chars`), which routed the remote edit, sudo edit and remote create through `remote_write_outcome` with per-site replies (M32 #10) and switched `wrap_words` to a char-counted leading-whitespace budget (M32 #7); `cargo fmt --all` ran immediately after it. All four acceptance criteria verified against real artifacts: shape line `ok_scans=0 outcome_calls=3 sentinel_tests=2 wrap_chars=1 wrap_test=1`, `allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`, and the full suite at exactly `passed=2615 failed=0` (2612 → 2615). The started, cuts-done, Findings and end-to-end entries are in the phase doc (Findings count `1`), the status flip and README phase-table row are `in-progress`, and everything is committed as `fix:` (`c52211b3`) with a clean working tree.

**Notes for review:** No deviations — the script ran clean on the first invocation and I hand-edited none of the three source files it names. Per the spec, the sudo-edit swap has no red test of its own (same parser as the root edit, whose refused-password tests still pass). Status is left at `in-progress` for the server to finalize.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.4.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.23s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
owlist_has_no_stale_entries ... ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.28s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
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
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

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
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (1112936): No such process
kill: (-1113645): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-084f36955f09af75)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/phase-16-sentinels-and-wrap.md` — +28 -1
- `src/cli/viewer.rs` — +8 -0
- `src/cli/viewer_layout.rs` — +2 -1
- `src/daemon/executor/file_ops/ops.rs` — +83 -24

**Commit:** c52211b3d26004cc9707e82f97ce36a98085ced9

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
