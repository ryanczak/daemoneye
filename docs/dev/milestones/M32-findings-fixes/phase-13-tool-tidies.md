# Phase 13 — Tool tidies

**Status:** review
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-12b
**Estimated diff:** ~57 insertions, ~92 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #42, #43, #64, #70, #72, #76, #81

## Goal

Seven small repeats in the shell tools, `daemoneye top` and two executor
arms. After this phase:

- **#70:** one `shell_tool_regex` in
  `src/daemon/executor/knowledge/shell.rs` compiles every pattern the
  shell tools take, so their shared 1 MiB size limit is written once. Each
  tool keeps its own refusal sentence, because each names its own
  parameter.
- **#72:** `send_input` and `close_shell` refuse an unknown shell id with
  the shared `unknown_shell_error`, as the other shell tools already do.
  Their two drifted copies of that refusal go, and so does
  `watch_shell_unknown_error`, a one-line alias of it. The two tests of
  that refusal now assert the shared wording byte for byte.
- **#42, #43:** `top`'s `window_text` takes its lines in one pass instead
  of a `lines().nth(i)` per line, and its alias `activity_window_text`
  goes.
- **#64:** `run_started_at` calls `started_at_of` instead of repeating its
  body.
- **#76:** the three run-log arms in `dispatch_runs.rs` share one
  `with_run_id`, which parses the id or returns the shared rejection.
- **#81:** `load_tools` looks each group up once instead of twice.

The one change a user can see is #72's: an unknown shell id now gets the
same refusal from `send_input` and `close_shell` as from every other
shell tool. The suite stays at **exactly 2610**; a different count means
something else changed.

## Architecture references

- `src/daemon/executor/knowledge/shell_watch.rs` § `unknown_shell_error`,
  whose doc says it exists so the wording cannot drift.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-13-tool-tidies.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `4fcc9402`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2610 failed=0`. It asserts its own anchors and prints seven
   lines, the last `the two unknown-id tests pin the shared wording`. If
   an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `refactor:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-13-tool-tidies.py`
(sha256 begins `452777ea8631ae94`), kept in the repo as the record of the change.

### The red run is done — do not repeat it

The two strengthened tests were run against the pre-phase code at
drafting. Both failed, showing the drifted wording against the shared
one:

```
  left: "Error: no shell with id \"s99\" is in the registry (no shells are live — call list_shells to confirm)."
 right: "Error: no shell with id \"s99\" is in the registry. No shells are live; call list_shells to confirm, or run a command first — …"
  left: "Error: no shell with id \"s99\" is in the registry. Live shell ids: s5. Call list_shells to refresh."
 right: "Error: no shell with id \"s99\" is in the registry — it may have exited and aged out, or the listing you are working from is stale. Live shell ids: s5. …"
test result: FAILED. 0 passed; 2 failed
```

With the change applied, both pass.

## Acceptance criteria

- [ ] The End-to-end block prints
      `regex_builders=1 drifted=0 aliases=0 nth=0 started_copies=0 id_parses=1 group_lookups=1 wording_pins=2`
      (today: `regex_builders=6 drifted=2 aliases=2 nth=1
      started_copies=1 id_parses=3 group_lookups=2 wording_pins=0`).
- [ ] `cargo test` reports **exactly** `passed=2610 failed=0`, the same
      count as before the phase.
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
K=src/daemon/executor/knowledge
echo "regex_builders=$(cat $K/shell.rs $K/shell_read.rs $K/shell_screen.rs $K/shell_watch.rs | grep -c 'size_limit(1 << 20)') drifted=$(cat $K/shell_input.rs $K/shell_life.rs | grep -c 'let live = live_shell_ids(reg);') aliases=$(grep -c 'fn activity_window_text\|fn watch_shell_unknown_error' src/cli/commands/top_tui_draw.rs $K/shell_watch.rs | awk -F: '{s+=$2} END {print s}') nth=$(grep -c 'lines().nth(i)' src/cli/commands/top_tui_draw.rs) started_copies=$(grep -c 'TurnLogEvent::RunStarted' src/cli/commands/top_load.rs) id_parses=$(grep -c 'RunId::parse(run_id)' src/daemon/executor/dispatch_runs.rs) group_lookups=$(grep -c 'tools_in_group(g)' src/daemon/executor/dispatch_meta.rs) wording_pins=$(cat $K/shell_observer_tests.rs $K/shell_lifecycle_tests.rs | grep -c 'words the refusal as every shell tool does')"
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
`passed=2610 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-13-tool-tidies.py`; do not edit it.
- Edit the thirteen files it names, through the script only: the five
  shell tool files and `knowledge/shell.rs`, the two shell test files,
  `top_tui_draw.rs`, `top_load.rs`, `top_detail.rs`, `dispatch_runs.rs`
  and `dispatch_meta.rs`.

## Out of scope

- `read_file`'s filter in `file_ops/read.rs`, which builds its regex with
  the same size limit but belongs to the file tools, not the shell tools.
- Each shell tool's refusal sentence for an invalid pattern, which names
  its own parameter and stays as it is.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 10:47 (started)

Executing phase 13 (tool tidies) per the spec. First step after this entry: run
`docs/dev/milestones/M32-findings-fixes/scripts/phase-13-tool-tidies.py` once,
then `cargo fmt --all`, before reading any source file.

### Update — 2026-10-30 10:47 (cuts done)

Ran `python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-13-tool-tidies.py && cargo fmt --all`
as the first command after the started entry; both exited 0. The script printed
seven lines, the last `the two unknown-id tests pin the shared wording`:

```
one shell_tool_regex for 6 pattern sites
one unknown_shell_error for send_input, close_shell and watch_shell
window_text linear; activity_window_text gone
run_started_at calls started_at_of
one with_run_id for the three run-log tools
load_tools looks each group up once
the two unknown-id tests pin the shared wording
```

`cargo fmt --all` reformatted nothing to report (exit 0, no output).

### Findings — 2026-10-30

`git diff --stat` shows 15 files changed, 83 insertions(+), 94 deletions(-):
the 13 source files the phase names plus this phase doc and the milestone
README (the start-of-phase status flip). Nothing unexpected.

### Update — 2026-10-30 10:52 (end-to-end verification)

Ran the block in § End-to-end verification verbatim, redirected to a file and
pasted below. Every line matches the expected values: the shape line
(`regex_builders=1 drifted=0 aliases=0 nth=0 started_copies=0 id_parses=1
group_lookups=1 wording_pins=2`), `allowlist=unchanged`, hygiene `16 passed`,
bug tracker `6 passed`, clippy clean, `fmt_exit=0`, and the full suite at
exactly `passed=2610 failed=0` — the same count as before the phase.

```
regex_builders=1 drifted=0 aliases=0 nth=0 started_copies=0 id_parses=1 group_lookups=1 wording_pins=2
allowlist=unchanged
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.22s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
fmt_exit=0
passed=2610 failed=0
```

### Update — 2026-10-30 17:55 (complete, server-authored)

**Summary:** </think>All tasks are done. Let me run through the completion checklist:

- [x] Phase doc's Status: line still says `in-progress` — yes, I flipped it to in-progress at start and did not flip to review.
- [x] Milestone README's phase-table row still says `in-progress` — yes.
- [x] I did NOT hand-write a `(complete)` Update Log entry — correct, I wrote (started), (cuts done), (Findings), and (end-to-end verification) entries only.
- [x] `cargo fmt --all` was run immediately before `git add` — yes, in the same command chain.
- [x] All verification commands ran clean — yes, per the E2E block.
- [x] Final message: substantive Summary + Notes for review — doing now.
- [x] `git status --short` shows nothing — the last run showed nothing after the commit lines (the tail output was just the stat continuation). Wait, let me double check — the output of `git status --short; git log -1 --stat | tail -10` showed the stat lines but no status lines, meaning status was clean. Good.
- [x] `git log -1 --stat` shows the commit includes every file touched — yes, all 15 files.

Let me verify git status is truly clean with one more check... Actually the previous command combined `git status --short; git log -1 --stat | tail -10` and the output began with ` src/daemon/executor/dispatch_runs.rs...` — those are the tail of the log stat, and there were no status lines. So clean.

Also I should double check the acceptance criterion `grep -c '^### Findings — '` prints 1 — I ran it, got 1. Good.

One thing to verify: the contract says "Grep for spec-pinned literals before reporting complete" — the spec pins the shape line output which I grepped (it's the E2E block itself). Done.

Now write the final summary message.</think>
All tasks complete. Final checklist verified: status still `in-progress` in both the phase doc and README, no hand-written `(complete)` entry, `cargo fmt --all` run immediately before `git add`, clean working tree, and the commit includes all 15 touched files.

**Summary:** Phase 13 (tool tidies) executed per spec — the authorized script `phase-13-tool-tidies.py` (sha256 prefix `452777ea8631ae94` verified) ran once as the first post-started-entry command and printed all seven cut lines ending in `the two unknown-id tests pin the shared wording`, with no assertion failures. It applied all seven cuts: one `shell_tool_regex` now holds the shell tools' 1 MiB pattern size limit (#70), `send_input` and `close_shell` use the shared `unknown_shell_error` with their two drifted copies and the `watch_shell_unknown_error` alias removed and both tests pinning the shared wording byte for byte (#72), `window_text` takes its lines in one pass with `activity_window_text` gone (#42 #43), `run_started_at` calls `started_at_of` (#64), one `with_run_id` serves the three run-log arms (#76), and `load_tools` looks each group up once (#81). Logged a `(cuts done)` entry with the script's verbatim output, a `### Findings — 2026-10-30` entry (`git diff --stat`: 15 files, 83+/94−; the 13 named source files plus the phase doc and README status flip — nothing unexpected), and an `(end-to-end verification)` entry pasting the captured block output. Committed as `df713e6a` (`refactor:`), working tree clean.

**Notes for review:** The End-to-end block was run twice — once verbatim to inspect, once redirected to a file for the mechanical capture — both producing identical shape lines. All expected values hit exactly: `regex_builders=1 drifted=0 aliases=0 nth=0 started_copies=0 id_parses=1 group_lookups=1 wording_pins=2`, `allowlist=unchanged`, hygiene `16 passed`, bug tracker `6 passed`, clippy clean, `fmt_exit=0`, and the full suite at exactly `passed=2610 failed=0` — the same count as before the phase, so nothing else changed. `grep -c '^### Findings — '` on the doc prints `1`. No deviations from the spec; the script's anchors all held, so no hand edits were needed anywhere.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.4.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.25s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.30s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.32s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (748822): No such process
kill: (-749547): No such process
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
- `docs/dev/milestones/M32-findings-fixes/phase-13-tool-tidies.md` — +50 -1
- `src/cli/commands/top_detail.rs` — +3 -1
- `src/cli/commands/top_load.rs` — +1 -8
- `src/cli/commands/top_tui_draw.rs` — +5 -10
- `src/daemon/executor/dispatch_meta.rs` — +3 -10
- `src/daemon/executor/dispatch_runs.rs` — +17 -21
- `src/daemon/executor/knowledge/shell.rs` — +9 -0
- `src/daemon/executor/knowledge/shell_input.rs` — +1 -13
- `src/daemon/executor/knowledge/shell_life.rs` — +1 -13
- `src/daemon/executor/knowledge/shell_lifecycle_tests.rs` — +5 -0
- `src/daemon/executor/knowledge/shell_observer_tests.rs` — +5 -0
- `src/daemon/executor/knowledge/shell_read.rs` — +2 -2
- `src/daemon/executor/knowledge/shell_screen.rs` — +2 -2
- `src/daemon/executor/knowledge/shell_watch.rs` — +3 -12

**Commit:** df713e6ad33f8dcf715b3b90641bcc2120751aa2

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
