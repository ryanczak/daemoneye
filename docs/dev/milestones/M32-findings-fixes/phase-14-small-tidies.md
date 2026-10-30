# Phase 14 — Small tidies

**Status:** done
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-13
**Estimated diff:** ~45 insertions, ~26 deletions
**Tags:** language=rust, kind=bugfix, size=s
**Findings:** M32 `FINDINGS.md` #1; M31 `FINDINGS.md` #10, #17, #32, #33,
#48 (part; the rest is M32 #16)

## Goal

One defect and five tidies, the milestone's last phase. After this phase:

- **M32 #1 (bug):** a ghost refused for an unreadable config is told why.
  `ghost_foreground_refusal` in `dispatch_foreground.rs` prints the load
  error with `{e:#}`. `Config::load` wraps the TOML parse error in a
  `parsing <path>` context, and plain `{e}` printed only that outer
  layer, so the parse error itself was dropped. The remedy stops naming
  the path a second time. A new test,
  `ghost_foreground_refusal_names_the_error_under_its_context`, gives the
  gate an error with a context layer, as `Config::load` does.
- **#10:** `SessionsConfig` and `ApprovalsConfig`'s `Default` impls call
  `default_true()`, the function their serde attributes name, instead of
  writing a literal `true`.
- **#32, #33:** `ContainerInfo`'s docs describe the sbx sandbox it now
  carries: the sandbox name, its `de-ghost-` / `de-chat-` prefix, and the
  `image` field, which is always `"shell"` and kept only as wire format.
- **#48 (part):** `ArtifactHit.kind`, `ArtifactHit.score` and
  `EpochHit.score`, which nothing reads, go with their `#[allow(dead_code)]`.
- **#17:** `src/daemon/startup.rs` names its imports instead of taking
  them through `use super::*`.

The suite goes from **2610** to **exactly 2611**: one test added, none
removed.

**Not in this phase: `EventHit`'s two fields.** At drafting, removing them
broke `search_events_returns_tail_not_head_when_segment_exceeds_cap`. The
events FTS table is contentless (`content=''`), so `e.event` reads back
NULL, the `String` decode fails on every row, and `search_events` has
returned no hits at all. A probe at drafting confirmed it: the index held
the row, the `event` column read `None`, and `search_events` returned 0.
The events search has worked only through its segment-scan fallback.
Removing the "dead" field would switch the index path on and change what
the search returns. That is M32 FINDINGS #16, a decision for the PE, not a
tidy.

## Architecture references

- `src/config/load.rs` § `Config::load`: the `parsing <path>` context the
  refusal must see through.
- M32 README, decisions 1 and 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-14-small-tidies.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `0fbb19f3`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2611 failed=0`. It asserts its own anchors and prints five
   lines, the last `startup.rs imports by name`. If an assert fires, stop
   and report it in the Update Log. Do not hand-edit anything it touches,
   and do not edit the script.
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
`docs/dev/milestones/M32-findings-fixes/scripts/phase-14-small-tidies.py`
(sha256 begins `0a6c5c98fb0f5b6d`), kept in the repo as the record of the change.

### The red run is done — do not repeat it

The new test was run against the pre-phase refusal at drafting. It
failed on its first assertion, because the refusal carried only the
context layer:

```
the refusal must carry the parse error itself: Error: foreground execution is refused because the configuration could not be read: parsing /home/op/.daemoneye/etc/config.toml. Fix or remove /home/…/config.toml, then retry; nothing was run.
test result: FAILED. 0 passed; 1 failed
```

With the change, it passes.

## Acceptance criteria

- [ ] The End-to-end block prints
      `chain_fmt=1 chain_test=1 literal_true=0 docker_docs=0 dead_allows=2 glob_import=0`
      (today: `chain_fmt=0 chain_test=0 literal_true=2 docker_docs=3
      dead_allows=5 glob_import=1`). The two `dead_code` allows left are
      `EventHit`'s.
- [ ] `cargo test` reports **exactly** `passed=2611 failed=0` (today
      2610).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "chain_fmt=$(grep -c 'could not be read: {e:#}' src/daemon/executor/dispatch_foreground.rs) chain_test=$(grep -c 'fn ghost_foreground_refusal_names_the_error_under_its_context' src/daemon/executor/dispatch_foreground.rs) literal_true=$(grep -cE '^ +(auto_name_enabled|commands): true,' src/config/types.rs) docker_docs=$(grep -cE 'de\.session|de\.ghost=1|as docker displays it' src/ipc_types.rs) dead_allows=$(grep -c 'allow(dead_code)' src/memory/index/search.rs) glob_import=$(grep -c '^use super::\*;' src/daemon/startup.rs)"
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
`passed=2611 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-14-small-tidies.py`; do not edit it.
- Edit the five files it names, through the script only:
  `src/daemon/executor/dispatch_foreground.rs`, `src/config/types.rs`,
  `src/ipc_types.rs`, `src/memory/index/search.rs`,
  `src/daemon/startup.rs`.

## Out of scope

- `EventHit` and the events FTS search (M32 FINDINGS #16).
- Removing `ContainerInfo.image`: it is wire format.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 18:30 (started)

Status flipped to `in-progress` and the milestone README row matches. Next:
run `phase-14-small-tidies.py` once, before opening any source file.

### Update — 2026-10-30 18:32 (cuts done)

`python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-14-small-tidies.py && cargo fmt --all`
(sha256 `0a6c5c98fb0f5b6d…`) printed, then exited 0:

```
the ghost refusal prints the error chain, and a test pins it
SessionsConfig and ApprovalsConfig defaults call default_true
ContainerInfo docs describe the sbx sandbox
the three unread ArtifactHit and EpochHit fields are gone
startup.rs imports by name
```

All five anchors asserted; `cargo fmt --all` followed clean.

### Findings — 2026-10-30

`git diff --stat`: 7 files, +67/−28 — the script's five source edits
(`dispatch_foreground.rs`, `config/types.rs`, `ipc_types.rs`,
`memory/index/search.rs`, `daemon/startup.rs`) plus the status flip in
this doc and the README phase-table row.

### Update — 2026-10-30 18:36 (end-to-end verification)

Ran the phase doc's End-to-end block verbatim (`bash <<'E2E' … E2E`,
output tee'd to `/tmp/phase14-e2e.txt`, pasted below unedited):

```
chain_fmt=1 chain_test=1 literal_true=0 docker_docs=0 dead_allows=2 glob_import=0
allowlist=unchanged
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.96s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.77s
fmt_exit=0
passed=2611 failed=0
```

Shape line matches the acceptance criteria; hygiene and bug tracker pass;
the allowlist is untouched; clippy and fmt are clean; the suite is
exactly `passed=2611 failed=0` — one test added, none removed.

### Update — 2026-10-30 18:36 (complete, server-authored)

**Summary:** **Summary:** Phase 14 executed per spec — the `phase-14-small-tidies.py` script (sha256 `0a6c5c98fb0f5b6d…`, verified before running) applied all five tidies as its first post-started-entry command, printed its five anchor lines clean, and `cargo fmt --all` followed. The ghost foreground refusal now prints the error chain via `{e:#}` with the new `ghost_foreground_refusal_names_the_error_under_its_context` test; config defaults call `default_true`; `ContainerInfo` docs describe the sbx sandbox; the three unread `ArtifactHit`/`EpochHit` fields are gone; `startup.rs` imports by name. End-to-end block (pasted verbatim into the phase doc's `### Update — … (end-to-end verification)` entry) gave exactly `chain_fmt=1 chain_test=1 literal_true=0 docker_docs=0 dead_allows=2 glob_import=0`, `allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`, and `passed=2611 failed=0` — one test added, none removed, matching the acceptance criteria.

**Notes for review:** No deviations; the script's output matched its five expected anchor lines and every assert held. The two remaining `dead_code` allows in `search.rs` are `EventHit`'s, as the spec requires (M32 #16, out of scope). Status left at `in-progress` for the server's completion tail; commit `304acad9` contains all seven touched files and the working tree is clean.

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
ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.29s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.07s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
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

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (817607): No such process
kill: (-818290): No such process
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
- `docs/dev/milestones/M32-findings-fixes/phase-14-small-tidies.md` — +47 -1
- `src/config/types.rs` — +2 -2
- `src/daemon/executor/dispatch_foreground.rs` — +25 -3
- `src/daemon/startup.rs` — +6 -1
- `src/ipc_types.rs` — +11 -6
- `src/memory/index/search.rs` — +1 -14

**Commit:** 304acad9b20aa63bea66e32227264a4ddabe318b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-30

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none. The source diff is byte-identical to a fresh run of the repo script on the pre-phase tree.
- **Calibration:** 27 turns, the script-as-repo-file pattern again. Gates rerun at review: fmt, build and clippy clean, `passed=2611 failed=0`. The new test's red run is the draft dry run on pre-phase code; the executor's log carries none, which the script pattern makes redundant.
