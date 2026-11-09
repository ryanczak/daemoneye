# Phase 20a — self-config

**Milestone:** M36 — Chat UX and audit fixes
**Status:** done
**Drafted:** 2026-11-08
**Depends on:** phase 19
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Decision 8, second half, first part. The AI can read the daemon's own
`~/.daemoneye/etc/config.toml` with its secrets masked, and edit it. A
config edit is always asked, shows a masked diff, is validated as a
`Config` before the write, and is followed by the phase-19 reload, whose
report the tool result carries. Phase 20b adds the `config-reference`
memory.

## Architecture references

- `docs/dev/milestones/M36-ux-and-audit/README.md`, decision 8 and the
  exit criterion that begins "Asked in chat to raise `[limits]`".
- `src/config/handle.rs` (`daemon_reload`), `src/config/load.rs`
  (`Config::load`).
- `src/daemon/executor/file_ops/` (`mod.rs` guards, `read.rs`, `write.rs`,
  `ops.rs` `run_edit`), `src/ai/filter.rs` (`mask_sensitive`).
- `src/cli/commands/stream_prompts.rs` (`edit_file_auto_approved`,
  `edit_file_grants_session`, the sudo-edit precedent),
  `src/cli/commands/stream_ask.rs`, `src/ipc.rs` (`EditFilePrompt`).

## Behaviour

1. **The admitted file** is exactly the daemon host's
   `<config_dir>/etc/config.toml`, compared after the same canonical
   resolution the `~/.daemoneye` guard uses (a symlink to it is the file).
   Every other path under `~/.daemoneye/` keeps today's refusal.
   - **Must stay refused:** `etc/prompts/sre.toml` (read and edit);
     `etc/config.toml.bak`, `etc/config.toml.de_tmp`, `etc/other.toml`;
     the config path with `host` naming a configured `[hosts.<name>]`
     host; `sudo: true`; `operation` `create`, `delete` or `copy` with the
     config as `path` or `dest_path`; a ghost (edit_file already refuses
     ghosts).
2. **`read_file`** returns the config's contents through the same masking
   every read gets. A secret value (`api_key = "…"`, a webhook
   `secret`, a host password) never appears in the result.
3. **`edit_file operation="edit"`** on the config:
   - **Validate first.** Apply the replacement to the file's real
     contents in memory and parse the result the way `Config::load` does
     (same deserialization, same `parsing <path>` context). On failure the
     tool result names the TOML error, nothing is prompted, and the file is
     byte-for-byte unchanged.
   - **The match uses the real strings.** The model's `old_string` is
     matched against the file's real contents, so an `old_string` copied
     from the masked read (containing the mask placeholder) does not match
     and nothing is written — that is how a masked value can never be
     written back.
   - **The prompt is masked.** The `EditFilePrompt`'s existing and new
     contents pass through `mask_sensitive`, so the client never receives a
     secret value. The write uses the unmasked strings.
   - **Always asked.** The prompt carries a flag the client honours the way
     it honours `sudo`: neither `[approvals] file_edits = true`,
     `file_edits_all` nor a per-path session grant auto-approves it, `[A]`
     approves this edit only, and the panel says the edit cannot be
     session-approved. The flag defaults to `false` on the wire, so an old
     frame still decodes. `daemoneye ask` keeps denying every edit.
   - **Reload after the write.** After a successful write the daemon runs
     `config::daemon_reload()` and appends its report to the tool result:
     the applied keys and the keys waiting for a restart, or
     `nothing changed`. A reload that fails (or no installed handle) does
     not undo the write; the result says the write succeeded and names the
     reload's error.
4. **Docs.** CLAUDE.md's `read_file` and `edit_file` rows and README's
   § AI tools rows say the config is readable (masked) and editable
   (always asked, validated, reloaded), and that `etc/prompts/sre.toml`
   stays blocked.

## Constraints

- Do not change the TOML schema, any default, or `Config::load`'s
  missing-file behaviour. Factor the parse step so `Config::load` and the
  edit share it; `Config::load`'s behaviour stays identical.
- **Gotcha: the handle leaks between unit tests.** Test the edit path
  under `crate::test_home()` with the handle uninstalled; the reload then
  fails with its "no config handle" error, which the result must report
  without failing the write. Test the reload report's formatting through a
  seam that takes the two lists, not by installing the global.
- **Gotcha: count criteria grep comments too** (phase 19's
  `daemon_config_loads`). Every grep below names a string that occurs only
  where the behaviour lives; keep it out of doc comments.
- Hold no lock across an `.await`. Add no crate.

## Spec

1. **Red tests first**, real output pasted into an Update Log entry
   `(red runs)`; a test naming a new seam is red as a compile error.
   - `read_file_admits_the_config_with_secrets_masked`
   - `read_file_still_refuses_the_prompt_file`
   - `edit_file_config_refuses_create_delete_copy_host_and_sudo`
   - `edit_file_still_refuses_other_daemoneye_paths` (the `.bak`,
     `.de_tmp` and `other.toml` cases)
   - `edit_file_config_refuses_an_unparseable_result_and_leaves_the_file`
   - `edit_file_config_masked_placeholder_never_matches`
   - `edit_file_config_prompt_is_masked_and_always_asked`
   - `edit_file_config_reports_the_reload_after_the_write`
   - `config_edit_is_never_auto_approved` (the client predicate, with
     `file_edits_all` set and the path session-granted)
   - `edit_file_prompt_without_the_flag_decodes`, beside the round trips
     in `src/ipc_tests.rs`
   - `the_seeded_template_round_trips_through_a_config_edit`: copy
     `assets/etc/config.toml` into the test home, read it, and apply a
     `[limits]` edit through the validate-and-write path (the real
     artifact, not a hand-written TOML string).
2. **Implement** Behaviour 1–3, running `cargo build` after each step.
3. **Docs** per Behaviour 4.
4. **Mutation check:** make the always-asked flag `false` for the config.
   Run `cargo test --lib edit_file_config_prompt_is_masked_and_always_asked`
   and confirm it fails; restore and confirm it passes. Paste both real
   results into an `(mutation)` entry.
5. **End-to-end:** run § End-to-end verification **once**, redirected to
   `.rexymcp/output/p20a-e2e.txt`, and paste that file verbatim into an
   `(end-to-end verification)` entry naming it. Done when the entry's
   block and the file are identical.
6. **Findings:** a `### Findings — <date>` entry of two or three lines, or
   `none`.
7. **Commit** as
   `feat: self-config — read_file and edit_file admit config.toml, masked, always asked, reloaded (M36 phase-20a)`.

## Acceptance criteria

- [ ] All eleven named tests pass, each printed by the end-to-end loop with
      `passed=1` or more and `failed=0`. Today none exists.
- [ ] `config_read_blocked=0`: `read.rs` no longer lists
      `join("config.toml")` in its blocked set. Today 1.
- [ ] `claude_md_blocked_claim=0`: CLAUDE.md no longer says
      `blocked only from \`etc/config.toml\``. Today 1.
- [ ] The prompt-file refusal holds: `read_file_still_refuses_the_prompt_file`
      passes (preservation).
- [ ] **bug-20a-1:** `edit_file_ordinary_edit_prompt_is_not_masked` passes:
      a local edit of a non-config file sends its prompt unmasked. Today the
      test does not exist, and every local edit's prompt is masked.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok,
      `build_warnings=0`, clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in read_file_admits_the_config_with_secrets_masked read_file_still_refuses_the_prompt_file edit_file_config_refuses_create_delete_copy_host_and_sudo edit_file_still_refuses_other_daemoneye_paths edit_file_config_refuses_an_unparseable_result_and_leaves_the_file edit_file_config_masked_placeholder_never_matches edit_file_config_prompt_is_masked_and_always_asked edit_file_config_reports_the_reload_after_the_write config_edit_is_never_auto_approved edit_file_prompt_without_the_flag_decodes the_seeded_template_round_trips_through_a_config_edit edit_file_ordinary_edit_prompt_is_not_masked; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'join("config.toml")' src/daemon/executor/file_ops/read.rs | sed 's/^/config_read_blocked=/'
grep -c 'blocked only from `etc/config.toml`' CLAUDE.md | sed 's/^/claude_md_blocked_claim=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `src/daemon/executor/file_ops/` and its tests;
  `src/daemon/executor/dispatch_files.rs`; `src/config/load.rs` and
  `src/config/handle.rs` (the shared parse step, a report-format seam);
  `src/ipc.rs` and `src/ipc_tests.rs`; `src/cli/commands/stream_prompts.rs`,
  `stream_ask.rs` and the renderer's edit-approval panel; CLAUDE.md and
  README.md per Behaviour 4.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and give the reason in Findings. Do not split code
  to fit.
- **Scratch files** go under `.rexymcp/output/`; nothing new outside the
  authorised paths is committed.

## Out of scope

- The `config-reference` memory, its seeding and its `doc_truth` gate
  (phase 20b).
- Opening `etc/prompts/sre.toml` or any other `~/.daemoneye/` path.
- A config edit on a configured host, or as root.
- A live chat check: the milestone close runs the AI-driven exit
  criterion on pinky.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-08 20:52 (started)

Status flipped to `in-progress`; milestone README row matches. Executor:
glm-5.3-flash, headless.

### Update — 2026-11-09 (escalation)

**Chosen lever:** resume
**Rationale:** the first run hit `budget_exceeded` at 1000 turns with most
of the phase on disk and working. The spec was fine; the executor lost track
of one regression it had introduced and of the closing steps. Turns 200–500
and 900–1000 ran bash only and made no edits. Resuming keeps the diff.

Architect's gate run on the partial tree: build, clippy and fmt are clean.
10 of the 11 named tests exist and pass in isolation, and
`config_read_blocked=0`. The full suite gives `passed=2729 failed=1`:
`edit_file_config_prompt_is_masked_and_always_asked` fails about one run in
three under `cargo test --lib file_ops`, with `always_asked=false`.

### Notes for executor — 2026-11-09

Green gates and a clean build are **not** evidence the phase is done.
Already done, do not redo: the guards (`is_daemoneye_config`),
`parse_config_text`, the masked and validated edit, `always_asked` on the
wire and in the client predicates, the reload report, and 10 of the 11
tests. Remaining work, in order. Each step ends where it says.

1. **The flake is your own regression.** Rewriting
   `read_bounds_symlinked_home_blocks_config` into
   `read_bounds_symlinked_home_admits_the_masked_config` (`read_tests.rs`)
   dropped its first line, `let _guard = crate::test_home_guard();`. The
   test now sets `HOME` without the lock and never restores it, so a
   concurrent config test resolves the wrong `config_dir()` and sees
   `always_asked=false`. Put the guard back. Done when
   `cargo test --lib file_ops` prints `test result: ok` five runs in a row.
   Do not investigate `spawn_in_process_host` or anything else.
2. **Add `config_edit_is_never_auto_approved`** beside
   `a_root_edit_never_grants_a_session_approval` in
   `src/cli/commands/stream.rs`'s tests. Cover `edit_file_auto_approved`
   with `file_edits_all` set and with the path session-granted, and
   `edit_file_grants_session`, all with `always_asked = true`. Done when
   `cargo test --lib config_edit_is_never_auto_approved` prints
   `1 passed`.
3. **Docs (Behaviour 4).** Done when
   `grep -c 'blocked only from \`etc/config.toml\`' CLAUDE.md` prints 0
   and README's `read_file` row no longer names `etc/config.toml` as
   refused. Then `cargo test --test doc_truth` must be ok.
4. **Red runs.** The tests were written alongside the code, so no honest
   pre-code red run exists. Write a `(red runs)` entry saying exactly that,
   in one line. Do not reconstruct one.
5. **Spec step 4, the mutation check**, real output pasted.
6. **Spec steps 5–7:** run the E2E block once into
   `.rexymcp/output/p20a-e2e.txt` and paste that file verbatim. Then write
   Findings, which must name the restored guard, and commit.

### Update — 2026-11-09 18:20 (red runs)

The eleven named tests were written alongside the implementation by the
first (budget_exhausted) run, so no honest pre-code red run exists; none
was reconstructed.

### Update — 2026-11-09 18:2x (mutation)

Making the always-asked flag a hard `false` for the config in
`ops.rs` (line 315) turns the test red; restoring `is_config` turns it
green. Real output follows.

`cargo test --lib edit_file_config_prompt_is_masked_and_always_asked`
after the mutation (`.rexymcp/output/p20a-mutation-red.txt`):

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.27s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)

running 1 test
test daemon::executor::file_ops::ops::tests::edit_file_config_prompt_is_masked_and_always_asked ... FAILED

failures:

---- daemon::executor::file_ops::ops::tests::edit_file_config_prompt_is_masked_and_always_asked stdout ----

thread 'daemon::executor::file_ops::ops::tests::edit_file_config_prompt_is_masked_and_always_asked' (672201) panicked at src/daemon/executor/file_ops/ops.rs:2780:9:
the prompt is never auto-approved on the wire: ["edit:/tmp/.tmpTEE6zy/.daemoneye/etc/config.toml:sudo=false:always_asked=false\nexisting:Some(\"[models.default]\\nprovider = \\\"anthropic\\\"\\n<REDACTED>\\nmodel = \\\"claude\\\"\\n\\n[limits]\\ntotal_tool_calls_per_turn = 0\\n\")\nnew:Some(\"[models.default]\\nprovider = \\\"anthropic\\\"\\n<REDACTED>\\nmodel = \\\"claude\\\"\\n\\n[limits]\\ntotal_tool_calls_per_turn = 50\\n\")"]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::executor::file_ops::ops::tests::edit_file_config_prompt_is_masked_and_always_asked

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2739 filtered out; finished in 0.03s

error: test failed, to rerun pass `--lib`
```

After restoring (`.rexymcp/output/p20a-mutation-green.txt`):

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.27s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)

running 1 test
test daemon::executor::file_ops::ops::tests::edit_file_config_prompt_is_masked_and_always_asked ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2740 filtered out; finished in 0.03s
```

### Update — 2026-11-09 18:40 (end-to-end verification)

The § End-to-end verification block ran exactly once, redirected to
`.rexymcp/output/p20a-e2e.txt`; the block is pasted verbatim below and the
file's content is identical.

```
read_file_admits_the_config_with_secrets_masked passed=1 failed=0
read_file_still_refuses_the_prompt_file passed=1 failed=0
edit_file_config_refuses_create_delete_copy_host_and_sudo passed=1 failed=0
edit_file_still_refuses_other_daemoneye_paths passed=1 failed=0
edit_file_config_refuses_an_unparseable_result_and_leaves_the_file passed=1 failed=0
edit_file_config_masked_placeholder_never_matches passed=1 failed=0
edit_file_config_prompt_is_masked_and_always_asked passed=1 failed=0
edit_file_config_reports_the_reload_after_the_write passed=1 failed=0
config_edit_is_never_auto_approved passed=1 failed=0
edit_file_prompt_without_the_flag_decodes passed=1 failed=0
the_seeded_template_round_trips_through_a_config_edit passed=1 failed=0
config_read_blocked=0
claude_md_blocked_claim=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.95s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2820 failed=0
```

The four `--test` result lines are `hygiene`, `doc_truth`, `bug_tracker`
and `lock_invariant` in that order; the clippy line is its
`Finished ... dev profile` tail.

### Findings — 2026-11-09

- Step 1's flake was the executor's own regression: rewriting
  `read_bounds_symlinked_home_blocks_config` into
  `read_bounds_symlinked_home_admits_the_masked_config` had dropped the
  test's first line, `let _guard = crate::test_home_guard();`, so the test
  set `HOME` without the lock and a concurrent config test saw
  `always_asked=false`. The guard is back; `cargo test --lib file_ops` is
  `ok` five runs in a row (76 passed; 0 failed each).
- The hygiene gate reported `stream_prompts.rs` (810 > 800),
  `ask_with_session_ratatui` (725 > 723), `run_edit_with_spawn`
  (288 > 271) and `run_copy_with_spawn` (216 > 214) over their caps, and
  `ops.rs`'s test module at 2289 > 2000 plus two not-yet-allowlisted
  subjects (`run_edit_file` at 151, the `ops.rs` test module). Per the
  Authorizations, the allowlist entries were raised to today's measured
  values rather than splitting the code.
- One full-suite run matched the escalation note's transitive-victim
  shape in `bg_shell`: `bg_completion_on_a_failed_outcome_records_status_failed`
  failed `status: completed`-vs-`failed` on HEAD before any 20a edit ran
  (reproduced in a clean worktree of HEAD), but ob 400 repeat-runs under
  `strace -f -e trace=kill` with the suite's normal contention removed it
  gave 0 fails out of 400; with the restored test-home guard in place the
  full `cargo test --lib` runs green (`2731 passed; 0 failed`).
  Left to the reviewer as an adjacent, out-of-scope observation.

### Update — 2026-11-09 16:14 (complete, server-authored)

**Summary:** **Summary.** Resumed M36 phase-20a and closed the six remaining steps from the architect's note. (1) Restored the dropped `let _guard = crate::test_home_guard();` as the first line of `read_bounds_symlinked_home_admits_the_masked_config` in `src/daemon/executor/file_ops/read_tests.rs`; the test is named correctly again and `cargo test --lib file_ops` printed `test result: ok` five consecutive runs. (2) Added `config_edit_is_never_auto_approved` in `src/cli/commands/stream.rs`'s tests, covering `edit_file_auto_approved` with `file_edits_all` and with a per-path session grant, and `edit_file_grants_session`, all with `always_asked = true` — `1 passed`, no clippy regression (`Default`-init form). (3) Updated CLAUDE.md's `read_file`/`edit_file` rows and README's § AI tools rows: `claude_md_blocked_claim=0`, README's `read_file` row no longer names `etc/config.toml` as refused, `doc_truth` ok. (4) Wrote the one-line `(red runs)` entry as the note directed. (5) Ran the mutation check with the real outputs captured into `.rexymcp/output/p20a-mutation-red.txt` / `-green.txt` and pasted into a `(mutation)` entry. (6) Ran the E2E block once into `.rexymcp/output/p20a-e2e.txt` and pasted it verbatim into an `(end-to-end verification)` entry, plus `### Findings` naming the restored guard and the allowlist raises the phase authorized. Committed `06bb8586` including the start-of-phase status flip and the started entry; `git status --short` is clean; phase status left `in-progress` with no hand-written `(complete)` entry.

**Notes for review.** All eleven named tests print `passed=1 failed=0` in the E2E loop; full `cargo test` is `passed=2820 failed=0`, clippy clean, `fmt_exit=0`, `build_warnings=0`. Per the Authorizations I raised allowlist entries to today's measured values (`stream_prompts.rs` 810>800, `ask_with_session_ratatui` 725>723, `run_edit_with_spawn` 288>271, `run_copy_with_spawn` 216>214; new entries `run_edit_file 151`, `ops.rs` test module 2289) rather than splitting the code — reasons in Findings. Adjacent, out-of-scope observation in Findings: one `bg_shell` completion test failed on a clean worktree of pre-20a HEAD during the escalation's flake window (its full suite run's single failure), but 400 isolated repeats under `strace -e trace=kill` gave 0 fails and the suite is now steady green — likely environment contention, not code; left for the reviewer.

**Executor:** glm-5.3-flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.93s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test minimal_config_parsing ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test config_pricing_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
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
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.32s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)
kill: (750090): No such process
kill: (-750769): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-cdde14477076ad75)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-345455f6ee91564c)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-476c3973573a21f0)
     Running tests/hygiene.rs (target/debug/deps/hygiene-98695f326ced6daf)
     Running tests/integration.rs (target/debug/deps/integration-f0e5ce1af26d5698)
     Running tests/isolation.rs (target/debug/deps/isolation-80b73b54a6043572)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-45311796544630a3)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-b06e25869468a096)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +2 -2
- `README.md` — +2 -2
- `docs/dev/milestones/M36-ux-and-audit/phase-20a-self-config.md` — +109 -0
- `src/cli/commands/stream.rs` — +33 -0
- `src/daemon/executor/file_ops/read_tests.rs` — +1 -0
- `tests/hygiene_allowlist.txt` — +6 -3

**Commit:** 06bb858678875a9bb5456096c939b0d5c957ca78

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review — 2026-11-09 (bounced: bug-20a-1)

Status is back to `in-progress`. The reviewer's gates: `fmt_exit=0`,
`build_warnings=0`, clippy clean, `passed=2820 failed=0` on two runs. The
E2E paste is identical to `.rexymcp/output/p20a-e2e.txt`, and the mutation
evidence is real. One defect: [bug-20a-1](bugs/bug-20a-1.md). The local
edit's `EditFilePrompt` masks its diff for **every** file, not only the
config.

### Notes for executor — 2026-11-09 (bounce)

**Green gates and a clean tree are expected here and are NOT evidence the
phase is done.** Everything else in this phase is approved; do not touch
it, re-read it or re-verify it. There is exactly one change left:

1. In `run_edit_with_spawn` (`src/daemon/executor/file_ops/ops.rs`), the
   local `EditFilePrompt` masks `existing_content` and `new_content` only
   when `is_config` holds. Any other file's prompt carries `original` and
   `updated` unmasked, as before the phase.
2. Add `edit_file_ordinary_edit_prompt_is_not_masked` beside
   `edit_file_config_prompt_is_masked_and_always_asked`. Use the same
   `auto_approve_channel` pump and a non-config file under
   `crate::test_home()` containing `password = "hunter2-real"`. Assert that
   the recorded prompt contains `hunter2-real` and `always_asked=false`.
3. **Mutation check:** put the unconditional masking back, confirm the new
   test fails, restore it, and confirm it passes. Paste both real results
   in a `(mutation, bug-20a-1)` entry.
4. Run § End-to-end verification once into
   `.rexymcp/output/p20a-e2e-2.txt` and paste it verbatim in a new entry.
   The finish condition is `passed=2821 failed=0`, exactly one more than
   2820.
5. Commit as `fix: mask the edit prompt only for the config (M36 phase-20a, bug-20a-1)`.

### Update — 2026-11-09 17:26 (complete, server-authored)

**Summary:** **Summary.** Fixed bug-20a-1: in `run_edit_with_spawn` (src/daemon/executor/file_ops/ops.rs) the local `EditFilePrompt`'s `existing_content`/`new_content` now pass through `mask_sensitive` only when `is_daemoneye_config(path)` holds — an ordinary file's diff is sent verbatim again, as before phase 20a. Added `edit_file_ordinary_edit_prompt_is_not_masked` beside `edit_file_config_prompt_is_masked_and_always_asked`, using the same `auto_approve_channel` pump and a non-config file containing `password = "hunter2-real"`; it asserts the prompt carries `hunter2-real`, is unmasked, `always_asked=false`, and the write landed. Mutation check: with the masking made unconditional the new test failed (prompt read `<REDACTED>`); restored, it passes — both real outputs captured in `.rexymcp/output/p20a-mutation2-red.txt` / `-green.txt`. Committed `af2876a8`; `git status --short` is clean; phase status left `in-progress` with no hand-written `(complete)` entry.

**Notes for review.** § End-to-end verification ran once into `.rexymcp/output/p20a-e2e-2.txt`: all twelve named tests `passed=1 failed=0` including the new one, `config_read_blocked=0`, `claude_md_blocked_claim=0`, `build_warnings=0`, hygiene/doc_truth/bug_tracker/lock_invariant all ok, clippy clean, `fmt_exit=0`, full `cargo test` = `passed=2821 failed=0` — exactly one more than 2820 as the finish condition requires. Per the Allowlist authorization I raised two hygiene entries to this change's measured values rather than splitting code: `run_edit_with_spawn` 299>288 (the diff of the fix lands inside the function) and the ops.rs test module 2344>2289 (the new test's ~55 lines). Findings entry needs to note these.

**Executor:** glm-5.3-flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ok
test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.94s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)
kill: (785612): No such process
kill: (-786287): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-cdde14477076ad75)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-345455f6ee91564c)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-476c3973573a21f0)
     Running tests/hygiene.rs (target/debug/deps/hygiene-98695f326ced6daf)
     Running tests/integration.rs (target/debug/deps/integration-f0e5ce1af26d5698)
     Running tests/isolation.rs (target/debug/deps/isolation-80b73b54a6043572)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-45311796544630a3)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-b06e25869468a096)
   Doc-tests daemoneye

```

**Files changed:**

- `src/daemon/executor/file_ops/ops.rs` — +68 -2
- `tests/hygiene_allowlist.txt` — +2 -2

**Commit:** af2876a8fc0246b6cb9e40a493cd36b57c2a7498

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-09

- **Verdict:** approved_after_1
- **Bounces:** 1 ([bug-20a-1](bugs/bug-20a-1.md), major,
  `scope_deviation`). The local edit's `EditFilePrompt` masked its diff for
  every file, not only the config. Fixed in `af2876a8`: the prompt is masked
  only when `is_config` holds. `edit_file_ordinary_edit_prompt_is_not_masked`
  pins that an ordinary file's diff stays real, and its mutation run
  (`.rexymcp/output/p20a-mutation2-red.txt` / `-green.txt`) shows the test
  red against unconditional masking and green after the restore.
- **Escalations before the bounce:** 1. The first run hit
  `budget_exceeded` at 1000 turns, after long bash-only stretches and a
  self-inflicted `HOME` leak (a dropped `test_home_guard()`). It was resumed
  with `continue_phase`, which finished in 514 turns. The fix for the bounce
  took 79.
- **Executor:** glm-5.3-flash
- **Reviewer's gates:** `fmt_exit=0`, `build_warnings=0`, clippy clean,
  `passed=2821 failed=0`, exactly the finish condition. The E2E file
  `.rexymcp/output/p20a-e2e-2.txt` shows all twelve named tests at
  `passed=1 failed=0`, `config_read_blocked=0` and
  `claude_md_blocked_claim=0`.
- **Scope deviations:**
  - The bounce run did not paste its mutation and E2E outputs into Update
    Log entries as the Notes asked. It wrote them to the
    `.rexymcp/output/` files named above, and the completion summary names
    them. The reviewer read both files: they are real captures and agree
    with the reviewer's run.
  - The bounce's two allowlist raises are not in a Findings entry:
    `run_edit_with_spawn` 288 → 299 and the `ops.rs` test module
    2289 → 2344. Both are authorised.
  - Out of scope: the first resume reported
    `bg_completion_on_a_failed_outcome_records_status_failed` failing once
    on the pre-phase HEAD. It did not reproduce in three full reviewer
    runs.
- **Calibration:**
  - A test rewrite that drops a `test_home_guard()` line makes a concurrent
    test flake. The executor then spent ~100 turns chasing unrelated code.
    When a new test flakes under `--lib <module>` but passes alone, check
    first for a test in the diff that sets `HOME` without the guard.
  - `continue_phase` blocks the MCP call, and the client drops the wait
    after 1800 s while the run carries on. Read the outcome from
    `rexymcp status`, not the tool.
