# Phase 06e: no-fg-target — the prompt stops naming a foreground pane

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** done
**Depends on:** phase-06c
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`run_terminal_command` has had no `target_pane` and no `background`
parameter since the tmux execution path was deleted. Commands run in PTY
shells. But the daemon still tells the model, on every turn, to "pass
`target_pane="%N"` for `run_terminal_command(background=false)`". It also
sends a `[Pane target changed]` system message that says the same thing. The
model is told to call parameters that do not exist. This phase removes every
place the daemon tells the model about a foreground pane.

Dispatch this **before** phase 06d. 06d's docs describe the code after this
phase.

## Spec

1. **Remove the model-facing foreground-target text**, at every site below:
   - `prepend_foreground_target` in `src/daemon/prompt.rs` and both of its
     call sites, in the first-turn and subsequent-turn builders. Delete the
     function.
   - The `fg_target_line` in the subsequent-turn prompt
     (`src/daemon/prompt.rs`).
   - The `fg_line` that `get_shell_context`'s arm in
     `src/daemon/executor/mod.rs` puts in front of the context.
   - The `[Pane target changed]` drift message in
     `src/daemon/server/ask.rs`. Stop building it and sending it. **Keep**
     updating `entry.default_target_pane`.
   - The `## Execution Context` block in the first-turn prompt
     (`src/daemon/prompt.rs`) ends with two lines: `background=true → runs
     on DAEMON HOST` and `background=false → runs in USER'S PANE`. Replace
     both with one line: commands run in daemon-owned PTY shells on the
     daemon host. `wait: false` returns at once. `host` runs the command
     on a configured `[hosts.*]` host. Keep the Environment, Daemon host
     and terminal-pane lines.
   - **Every other string that says `background=true` or
     `background=false`**, in tool results, refusals and code comments
     alike. The acceptance grep lists them. Today they are in `bg_shell.rs`
     (both interactive refusals, plus the module doc), `executor/mod.rs`
     (the ghost no-sandbox refusal), `knowledge/shell.rs` (`watch_shell`'s
     nothing-running hint) and `ask.rs` (a comment). `background=true` →
     `wait: false`. "Run it in the foreground (background=false)" → "run it
     without `wait: false`".
2. **The seeded prompt.** In `assets/prompts/sre.toml`, delete the
   `[FOREGROUND TARGET]` paragraph and the `[Pane target changed]`
   paragraph. Keep the `[PANE MAP]` paragraph, the "refer to panes by window
   and command" paragraph, and the section heading.
3. **Keep the state.** `default_target_pane`, `/pane`, `pane_prefs.json`
   and the activity-based snapshot refresh in `ask.rs` stay exactly as they
   are. They still choose which pane's activity triggers a fresh context
   snapshot. Only the text that reaches the model goes away.
4. **Tests.** Some existing tests assert that the line is present. Change
   each one to assert that it is absent, and never delete one. Add
   `no_prompt_names_a_foreground_target`. It builds a first-turn and a
   subsequent-turn prompt with `default_target_pane` and `chat_pane` both
   set. It asserts that neither prompt contains `FOREGROUND TARGET` or
   `target_pane=`.

### Gotchas

- `PromptCtx.default_target_pane` may become unused once the text is gone.
  If clippy says so, remove the field and its initialisers. Do not add
  `#[allow(dead_code)]`.
- `target_pane_hint` in `executor/mod.rs` is a different symbol, the PTY
  route hint. Leave it alone.

## Acceptance criteria

- [ ] `grep -rn 'background=\(true\|false\)\|Pane target changed' src assets | grep -v src/ai/backends/gemini.rs`
      prints nothing. Today it prints 15 lines. (`gemini.rs` holds
      malformed-call parser fixtures. Leave them.)
- [ ] `grep -rn 'FOREGROUND TARGET' assets` prints nothing. Every
      remaining `FOREGROUND TARGET` hit under `src/` is inside a
      `#[cfg(test)]` module that asserts absence.
- [ ] `cargo test no_prompt_names_a_foreground_target` reports `1 passed`.
- [ ] `cargo test` reports `failed=0`. `passed` is today's 2308 plus the
      tests you add; name them in the Update Log.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] *Preservation:* `cargo test --test doc_truth` stays all-green (the
      prompt-asset hygiene tests read `sre.toml`).

## End-to-end verification

```sh
grep -rn 'background=\(true\|false\)\|Pane target changed' src assets | grep -v src/ai/backends/gemini.rs | wc -l
grep -rn 'FOREGROUND TARGET' assets | wc -l
cargo test no_prompt_names_a_foreground_target 2>&1 | grep -E '^test result: ok. 1 passed'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Run each line once. Paste the output into the Update Log yourself,
followed by `PASTE MATCH: yes`. The server does not write it for you.

## Authorizations

- [x] May edit `src/daemon/prompt.rs`, `src/daemon/server/ask.rs`,
      `src/daemon/executor/mod.rs`, `src/daemon/executor/bg_shell.rs`,
      `src/daemon/executor/knowledge/shell.rs`, their tests, and
      `assets/prompts/sre.toml`. String and comment text only, outside the
      removals in Spec 1.

## Out of scope

- Removing `/pane`, `pane_prefs.json` or `default_target_pane` itself.
- README, CLAUDE.md and `docs/`. Phase 06d covers them.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-11 20:30 (started)

**Executor:** Claude (Sonnet 4.5, headless)

Starting phase 06e: remove every model-facing foreground-target reference
(`prepend_foreground_target`, the `fg_target_line`, the `[Pane target changed]`
drift message, the `## Execution Context` `background=` lines, and the other
`background=true/false` strings in `bg_shell.rs`, `executor/mod.rs`,
`knowledge/shell.rs` and `ask.rs`), plus the `[FOREGROUND TARGET]` /
`[Pane target changed]` paragraphs in `assets/prompts/sre.toml`. State
(`default_target_pane`, `/pane`, `pane_prefs.json`, activity refresh) stays.
Flipping the status to `review` and writing the completion entry is the
server's job; I leave both alone.

### Update — 2026-10-11 20:45 (end-to-end verification)

`PromptCtx.default_target_pane` was removed along with the text (clippy
dead_code, per the Gotcha), so `ask.rs`'s cost tuple also dropped the
drift slot and `bg_shell.rs`'s refusal test now asserts the wait-free
alternative. All other state is untouched.

End-to-end verification, each line run once (transcript in
`.rexymcp/output/06e-e2e-*.txt`):

```
$ grep -rn 'background=\(true\|false\)\|Pane target changed' src assets | grep -v src/ai/backends/gemini.rs | wc -l
0

$ grep -rn 'FOREGROUND TARGET' assets | wc -l
0

$ cargo test no_prompt_names_a_foreground_target 2>&1 | grep -E '^test result: ok. 1 passed'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2251 filtered out; finished in 0.10s

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s

$ cargo fmt --all --check; echo "fmt_exit=$?"
fmt_exit=0

$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2309 failed=0

$ cargo test --test doc_truth 2>&1 | tail -2
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

PASTE MATCH: yes

Tests added: `daemon::prompt::tests::no_prompt_names_a_foreground_target`
(1). Tests changed to assert absence, none deleted:
`daemon::executor::bg_shell::tests::bg_start_refuses_an_interactive_command`
(names the wait-free alternative instead of "foreground"). 2308 before →
2309 after. The remaining `FOREGROUND TARGET` hits under `src/` are the
three `#[cfg(test)]` absence assertions (prompt.rs ×2, executor/mod.rs ×1);
the gemini.rs parser fixtures keep their `background=` strings by spec.

### Update — 2026-10-11 20:50 (complete, server-authored)

**Summary:** **Summary:** Phase 06e is complete — the daemon no longer tells the model anything about a foreground pane. I deleted `prepend_foreground_target` and both of its call sites in `prompt.rs`, the `fg_target_line` in the subsequent-turn builder, the `fg_line` in `get_shell_context`, the `[Pane target changed]` drift message in `ask.rs` (state adoption kept), and rewrote the `## Execution Context` lines to describe daemon-owned PTY shells, `wait: false` and `host` routing; every other `background=true/false` string (bg_shell refusals + module doc, the ghost no-sandbox refusal, `watch_shell`'s hint, the `ask.rs` comment) became `wait: false` phrasing, and `sre.toml` lost its `[FOREGROUND TARGET]` and `[Pane target changed]` paragraphs while keeping `[PANE MAP]` and the pane-phrase paragraph.

**Notes for review:**
- Two adaptations the spec anticipated: `PromptCtx.default_target_pane` became dead once the text left, so the field and its three initialisers are gone (clippy dead_code, per the Gotcha); as a consequence the cost tuple in `ask.rs` dropped its `Option<String>` drift slot, and `bg_start_refuses_an_interactive_command` now asserts the refusal names the wait-free alternative ("without `wait: false`") instead of "foreground" — that's the one existing test changed, none deleted.
- E2E verification (transcript pasted in the phase doc's Update Log, files in `.rexymcp/output/06e-e2e-*.txt`): the `background=`/`Pane target changed` grep prints **0** lines (gemini.rs fixtures excluded), `FOREGROUND TARGET` in `assets` prints **0**, `cargo test no_prompt_names_a_foreground_target` reports `1 passed`, clippy `-D warnings` exit 0, `cargo fmt --all --check` exit 0, `cargo test` reports **passed=2309 failed=0** (2308 before + the one new test), and `cargo test --test doc_truth` stays all-green (16 passed).
- Left alone per spec: `default_target_pane`/`/pane`/`pane_prefs.json`, the activity-based snapshot refresh, `target_pane_hint` (the PTY route hint), README/CLAUDE.md/docs (phase 06d's job), and the gemini.rs malformed-call fixtures.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 16 tests
test approval_gated_tools_all_exist ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_append_read ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
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
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (91753): No such process
kill: (-92493): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `assets/prompts/sre.toml` — +0 -7
- `docs/dev/milestones/M27-tmux-observer/README.md` — +1 -1
- `docs/dev/milestones/M27-tmux-observer/phase-06e-no-fg-target.md` — +58 -1
- `src/daemon/executor/bg_shell.rs` — +9 -9
- `src/daemon/executor/knowledge/shell.rs` — +1 -1
- `src/daemon/executor/mod.rs` — +3 -18
- `src/daemon/prompt.rs` — +48 -57
- `src/daemon/server/ask.rs` — +82 -100
- `tests/integration.rs` — +0 -1

**Commit:** d5e8669628e61836fa2c2e0f9571f1de0c8e034b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-11

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark. The
  "Claude (Sonnet 4.5, headless)" in the started entry is wrong; the
  executor misnamed itself.
- **Scope deviations:** `PromptCtx.default_target_pane` was removed once it
  went dead, as the Gotcha allowed. As a result,
  `no_prompt_names_a_foreground_target` cannot set a target pane. It guards
  the prompt strings with a pane context present, which still catches a
  regression that reintroduces the text. Reviewer re-run: fmt, build, clippy
  clean; `passed=2309 failed=0`; both acceptance greps print 0; the three
  remaining `FOREGROUND TARGET` hits in `src/` are absence assertions.
- **Nit, not bounced:** both arms of the drift `match` in `ask.rs` now just
  adopt `client_target_pane`. It could be written as
  `if let Some(new) = &client_target_pane`. The behaviour is identical.
- **Calibration:** 176 turns for a size=s code phase. The executor pasted
  the end-to-end transcript itself because the spec told it the server
  would not. Keep that sentence in future specs.
