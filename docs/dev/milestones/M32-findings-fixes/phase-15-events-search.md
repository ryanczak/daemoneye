# Phase 15 — Events search

**Status:** review
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-14
**Estimated diff:** ~131 insertions, ~41 deletions
**Tags:** language=rust, kind=bugfix, size=s
**Findings:** M32 `FINDINGS.md` #16, #17; the rest of M31 `FINDINGS.md` #48

## Goal

Two defects in `search_repository`'s events search, added by the PE on
2026-10-30. After this phase:

- **M32 #16 (bug):** the events search reads its FTS index.
  `search_events` in `src/memory/index/search.rs` selected `e.event` from
  a contentless table. That column reads back NULL, so the `String`
  decode failed on every row and `filter_map` dropped them all. The
  search has worked only through its tail scan, which reads the last
  `EVENTS_TAIL_LINES` lines. Now it selects only `events_map` columns,
  and `EventHit` loses its two dead fields with their
  `#[allow(dead_code)]`.
- **What the events search returns.** It searches the whole history.
  The index is queried first, with the query's words as one FTS5 phrase
  from a new `build_phrase_expr`. The OR form `build_match_expr` builds
  would return every line holding `idx` or `0` for the query `idx=0`.
  The tail scan then adds matches the index did not return. Both paths
  keep only lines that contain the query text, as the scan always has.
  Both name a hit by its segment file, so a line both find is returned
  once.
- **M32 #17 (bug):** every events result is masked with
  `mask_sensitive`. The index masks the body it stores, but both paths
  returned the unmasked line from disk, so a key in a logged command
  reached the model.

The suite goes from **2611** to **exactly 2612**. One test is renamed and
rewritten, one is added, and one is strengthened.

## Architecture references

- `src/memory/index/mod.rs` § the `events` FTS5 table (`content=''`) and
  `build_match_expr`.
- `src/search.rs` § `search_events_fts`, `search_events_in_segments`.
- M32 README, decisions 1 and 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-15-events-search.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `5a330f6e`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2612 failed=0`. It asserts its own anchors and prints four
   lines, the last `the events tests pin the whole history, one copy and
   masking`. If an assert fires, stop and report it in the Update Log.
   Do not hand-edit anything it touches, and do not edit the script.
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
`docs/dev/milestones/M32-findings-fixes/scripts/phase-15-events-search.py`
(sha256 begins `7019bfaf531d8e90`), kept in the repo as the record of the
change.

### The red run and the mutations are done — do not repeat them

The new tests were run against the pre-phase search at drafting. Two
failed: the index found no head line, and the key came back unmasked.

```
search_events_covers_the_whole_history_and_returns_each_line_once ... FAILED
  assertion `left == right` failed: the index finds a head line
events_search_masks_what_it_returns ... FAILED
  the key is masked: command=aws configure set aws_access_key_id AKIAIOSFODNN7EXAMPLE event=command ts=2026-01-01T00:00:00Z
```

Each of these four mutations of the post-phase code failed at least one
search test:

| Mutation | Tests failed |
|---|---|
| The scan ignores lines the index already returned | 3 |
| No `mask_sensitive` on `matched_line` | 1 |
| No query-text filter on index hits | 1 |
| `search_events` uses `build_match_expr` | 1 |

## Acceptance criteria

- [ ] The End-to-end block prints
      `phrase_fn=1 event_col=0 dead_allows=0 masked=2 old_test=0 mask_test=1`
      (today: `phrase_fn=0 event_col=1 dead_allows=2 masked=0 old_test=1
      mask_test=0`).
- [ ] `cargo test` reports **exactly** `passed=2612 failed=0` (today
      2611).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "phrase_fn=$(grep -c 'fn build_phrase_expr' src/memory/index/mod.rs) event_col=$(grep -c 'e\.event' src/memory/index/search.rs) dead_allows=$(grep -c 'allow(dead_code)' src/memory/index/search.rs) masked=$(grep -c 'crate::ai::mask_sensitive' src/search.rs) old_test=$(grep -c 'fn search_events_returns_tail_not_head' src/search_tests.rs) mask_test=$(grep -c 'fn events_search_masks_what_it_returns' src/search_tests.rs)"
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
`passed=2612 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-15-events-search.py`; do not edit it.
- Edit the four files it names, through the script only:
  `src/memory/index/mod.rs`, `src/memory/index/search.rs`,
  `src/search.rs`, `src/search_tests.rs`.

## Out of scope

- The `legacy` segment. Its events index under the label `legacy` but
  live in `events.jsonl`, so an index hit on one resolves to a missing
  `legacy.jsonl` and is skipped with a warning, as it was before. The
  tail scan still reads the file.
- `build_match_expr` and every other index search: memory recall wants
  the OR form.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 19:36 (cuts done)

Started the phase, flipped the status, then ran the phase script as the first
command after the started entry, exactly as Spec task 1 requires:

```sh
python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-15-events-search.py && cargo fmt --all
```

Its complete output, pasted as-is:

```
build_phrase_expr quotes the query as one phrase
search_events reads only the map columns
the index hits come first, each line once, all masked
the events tests pin the whole history, one copy and masking
```

Exit 0; no assert fired. The script's sha256 is
`7019bfaf531d8e907ce28c675b1fae36e1fb16156156945209bb162c104745fe`, matching
the `7019bfaf531d8e90` prefix the phase doc records. `cargo fmt --all` followed
it in the same command and reflowed nothing new. No source file was opened
before the script ran.

### Findings — 2026-10-30 19:36

`git diff --stat` shows 6 files: the two doc-status flips I made at start
(README, phase doc) plus the four files the script edits —
`src/memory/index/mod.rs` (15 insertions), `src/memory/index/search.rs` (13
changed), `src/search.rs` (57 changed), `src/search_tests.rs` (87 changed) —
133 insertions, 43 deletions total, close to the estimated ~131/~41.

### Update — 2026-10-30 19:39 (end-to-end verification)

Ran the phase doc's End-to-end verification block verbatim, unmodified. Its
complete output, pasted as-is:

```
phrase_fn=1 event_col=0 dead_allows=0 masked=2 old_test=0 mask_test=1
allowlist=unchanged
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.98s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.13s
fmt_exit=0
passed=2612 failed=0
```

Every line matches the expected shape: the grep line is
`phrase_fn=1 event_col=0 dead_allows=0 masked=2 old_test=0 mask_test=1`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`,
clippy clean, `fmt_exit=0`, and the full suite reports `passed=2612 failed=0` —
exactly the 2612 the acceptance criteria pin (up from 2611). The block ran once,
with no early stop and no re-run.

### Update — 2026-10-30 19:42 (complete, server-authored)

**Summary:** </think>All tasks done. Completion checklist:

- [x] Phase doc's Status: line still says `in-progress` — yes, I flipped to in-progress and never touched it again.
- [x] Milestone README's phase-table row still says `in-progress` — yes.
- [x] No hand-written `(complete)` entry — correct, my entries are (cuts done), Findings, (end-to-end verification).
- [x] `cargo fmt --all` run immediately before git add — yes (ran again with fmt in the same command before staging).
- [x] All verification commands ran clean — yes.
- [x] Final message substantive Summary + Notes for review.
- [x] `git status --short` shows nothing.
- [x] `git log -1 --stat` shows the commit includes every file touched.

Acceptance criteria:
- E2E prints `phrase_fn=1 event_col=0 dead_allows=0 masked=2 old_test=0 mask_test=1` ✓
- `passed=2612 failed=0` ✓ exactly
- hygiene + bug_tracker pass, allowlist unchanged, fmt/build/clippy clean ✓
- `grep -c '^### Findings — '` prints 1 — the Findings entry. ✓ (one entry)

Now the final Summary.</think>
All five Spec tasks are done and the tree is clean. Final report:

**Summary:** Executed M32 phase-15 (events search) exactly per its Spec: flipped the phase doc and README row to `in-progress`, then ran `phase-15-events-search.py && cargo fmt --all` as the first command — it exited 0 with its four anchor lines (`build_phrase_expr quotes the query as one phrase` … `the events tests pin the whole history, one copy and masking`) and sha256 `7019bfaf531d8e90…` matching the doc's record, so no assert fired and no hand-edit was needed. The script's change: `search_events` now selects only the `events_map` columns (the contentless table's `e.event` reads NULL, which had silently dropped every index hit), queries the index with the query's words as one FTS5 phrase via the new `build_phrase_expr` instead of the OR-form `build_match_expr`, merges index hits with the tail scan keeping each line once and only lines containing the query text, and masks every returned line with `mask_sensitive`; `EventHit`'s two dead fields and their `#[allow(dead_code)]` are gone. Logged the `(cuts done)` entry, the `### Findings — 2026-10-30` entry (diff stat: 4 source files, 133+/43−, close to the ~131/~41 estimate), and the end-to-end entry, then committed as `fix:` (`f0e5b49b`).

**End-to-end verification:** The phase's block, run verbatim once with no early stop, printed `phrase_fn=1 event_col=0 dead_allows=0 masked=2 old_test=0 mask_test=1`, `allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`, and the full suite at exactly `passed=2612 failed=0` — matching every acceptance criterion, including the 2612 count (up from 2611).

**Notes for review:** Status is left at `in-progress` per the contract; no `(complete)` entry was written. `grep -c '^### Findings — '` on the phase doc is 1, and the end-to-end entry holds the fenced block output. Nothing was adapted or improvised — the script's anchors held, so there were zero deviations from the spec. Out-of-scope items (the `legacy` segment resolution warning, `build_match_expr`'s OR form for memory recall) were left untouched.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.4.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ok
test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_child_inherits_depth_and_parent ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test session_index_persistence ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

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
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (907061): No such process
kill: (-907788): No such process
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
- `docs/dev/milestones/M32-findings-fixes/phase-15-events-search.md` — +56 -1
- `src/memory/index/mod.rs` — +15 -0
- `src/memory/index/search.rs` — +5 -8
- `src/search.rs` — +40 -17
- `src/search_tests.rs` — +71 -16

**Commit:** f0e5b49bfb9fe0de665a15388882bd027e8f94f5

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
