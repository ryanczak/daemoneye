# Phase 03c: open-shell-size — `open_shell(rows, cols)`

**Milestone:** M28 — Interactive shells
**Status:** done
**Depends on:** phase-03b
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Since 03b, every shell spawns at `[shells] rows` × `cols`. A wide program
such as `htop` on a big host, or a two-pane TUI, can want more. This
phase gives the model `open_shell(rows?, cols?)`, so it can open a shell
sized for the program it is about to run. The result names the size the
shell was opened at. Attached mode's resize and the `Resize` frame are
unchanged.

## Architecture references

- `docs/design/interactive-shells.md` D10: `open_shell` gains optional
  `rows` and `cols`, and the result names the size.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
`src/ai/types/pending.rs`, `src/daemon/stream.rs`, `src/ai/tools/defs.rs`,
`src/daemon/executor/mod.rs` (the `OpenShell` arm),
`src/daemon/executor/knowledge/shell.rs`, `src/daemon/server/shells.rs`
(one call), `src/ai/tools/dispatch.rs` (one test), `assets/prompts/sre.toml`,
and the `open_shell` rows of `CLAUDE.md` and `README.md`.

1. **The parameters.** `open_shell` gains `rows` and `cols`, both optional.
   Deserialize them as `Option<u64>`, so an absurd value is clamped rather
   than failing argument parsing. Carry them through args → `AiEvent` →
   `PendingCall` → `to_tool_call` (which writes them back into the
   arguments JSON) → the executor arm. `defs.rs` gets two `ParamDef`s,
   saying the default is the `[shells]` size and giving the clamp range.
2. **Resolving the size.** Add a pure
   `resolve_size(rows: Option<u64>, cols: Option<u64>, default: (u16, u16)) -> (u16, u16)`
   in `knowledge/shell.rs`. A missing value comes from `default`. The
   result is clamped by **the same rule as** `ShellsConfig::size`
   (rows `10..=200`, cols `40..=500`). Do not write a second copy of
   those bounds: route the values through `ShellsConfig::size`, or move
   the clamp into one shared function that both call. The production
   default is `crate::shell::spawn_size()`.
3. **Spawning at that size.** `open_shell(caller, host, rows, cols)` and
   `open_shell_id(caller, host, rows, cols)` resolve the size once and
   spawn at it. The production local spawn is
   `spawn_detached_host(id, &[], rows, cols)`, and the named-host spawn is
   `spawn_ssh_host(id, name, rows, cols)`. Give the two non-test spawn
   helpers a size parameter, or add sized variants. The `#[cfg(test)]`
   in-process host keeps its fixed size and ignores the request. The
   user's `/shell` path (`src/daemon/server/shells.rs`) passes
   `None, None`. Every other `open_shell(…)` / `open_shell_id(…)` call in
   tests gains `None, None`, and the compiler names them.
4. **The result names the size.** `opened_shell_sentence` takes the
   resolved size and adds ` Its terminal is <rows> rows × <cols> columns.`
   after the host clause (when there is one), before ` Note:`.
5. **Docs.** Add the two parameters to the `open_shell` bullet in
   `sre.toml`, with one example (`open_shell(cols: 200)` for a wide
   program). Add one sentence to the `open_shell` rows of `CLAUDE.md` and
   `README.md`. `tests/doc_truth.rs` must pass unmodified.
6. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03c.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Gotchas.**
- One existing test's assertion changes, and that list is complete:
  `open_shell_local_result_is_unchanged` pins the exact local sentence. It
  is **renamed** `open_shell_local_result_names_the_size`, and its expected
  string gains the size sentence at the 03b default, `40 rows × 120 columns`.
  `lifecycle_home` writes no `[shells]` table, so `spawn_size()` gives
  40×120 there. If it does write one, use the size it sets.
- Do not change the `Resize` frame, attached mode or the CLI `shell-host`
  defaults.

## Tests

- `resolve_size_fills_and_clamps`: `(None, None, (40, 120))` ⇒ `(40, 120)`;
  `(Some(50), None, (40, 120))` ⇒ `(50, 120)`;
  `(Some(0), Some(99999), (40, 120))` ⇒ `(10, 500)`.
- `open_shell_names_the_requested_size`: through `open_shell_test_seam`,
  as the rename's test does. `open_shell(caller, None, Some(50), Some(160))`
  ⇒ the sentence contains `50 rows × 160 columns`.
- `open_shell_args_carry_rows_and_cols` (`dispatch.rs` tests, through
  `dispatch_tool_event`): `{rows: 50, cols: 160}` ⇒ `AiEvent::OpenShell`
  carries `Some(50)` and `Some(160)`; `{}` ⇒ both `None`.
- The rename in § Gotchas.

## Acceptance criteria

- [ ] The 3 new tests and the renamed one ⇒ `test result: ok. 1 passed`
      each.
- [ ] `cargo test` ⇒ `passed=2390 failed=0`: exactly 2387 at `e1ae6c3b`
      plus the 3 new tests. The rename adds nothing.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03c.txt; : > "$A"
for t in resolve_size_fills_and_clamps open_shell_names_the_requested_size open_shell_args_carry_rows_and_cols open_shell_local_result_names_the_size; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then run the self-check, and put the line it prints into the entry after
the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-03c-open-shell-size.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03c.txt
diff /tmp/pasted-03c.txt /tmp/e2e-03c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `open_shell` rows only).

## Out of scope

- Resizing a live shell from a tool. The only resize is attached mode's.
- `run_terminal_command(interactive: true)` and the ssh auto-upgrade:
  phase 03d.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-16 13:29 (started)

**Executor:** phase-03c run (headless).

Flipped the status to `in-progress` and begun the spec: params through the
dispatch chain, `resolve_size`, the sized spawn path, the size sentence,
docs.

### Update — 2026-10-16 13:52 (end-to-end verification)

Run of § End-to-end verification, verbatim, captured to `/tmp/e2e-03c.txt`:

```
resolve_size_fills_and_clamps: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
open_shell_names_the_requested_size: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
open_shell_args_carry_rows_and_cols: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
open_shell_local_result_names_the_size: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2390 failed=0
```

PASTE MATCH

### Update — 2026-10-16 21:37 (complete, server-authored)

**Summary:** </think>The literal check passes. The phase doc status is still `in-progress`, the README row says `in-progress`, no `(complete)` entry was hand-written, the E2E entry exists with the fence and PASTE MATCH, everything is committed, tree clean.

Final Summary + Notes for review.</think>
Phase 03c complete.

**Summary:** `open_shell` now takes optional `rows`/`cols`, carried as `Option<u64>` through `OpenShellArgs` → `AiEvent::OpenShell` → `PendingCall::OpenShell` → `to_tool_call` (written back into the arguments JSON, so history shows what the model asked) → the executor arm, with new `ParamDef`s in `defs.rs` naming the default and the clamp range. A pure `resolve_size(rows, cols, default)` in `knowledge/shell.rs` fills missing halves from the default and routes the resolved pair through `ShellsConfig::size` — one subtlety worth flagging: an out-of-`u16` request (99999) *saturates* to `u16::MAX` before the clamp rather than falling back to the default, since the spec's `(Some(0), Some(99999)) ⇒ (10, 500)` case requires the clamp to see the model's number, not the 120 default it didn't ask for. The local spawn is `spawn_detached_host(id, &[], rows, cols)` and the named-host spawn `spawn_ssh_host(id, name, rows, cols)` (sized variants; the `#[cfg(test)]` in-process host keeps its fixed 24×80 and ignores the request); `/shell` passes `None, None`. `opened_shell_sentence` gained ` Its terminal is <rows> rows × <cols> columns.` after the host clause; the pinned test is renamed `open_shell_local_result_names_the_size` with the size in its expected string. `sre.toml` documents the parameters with the `open_shell(cols: 200)` example, and the `open_shell` rows of `CLAUDE.md` and `README.md` name them.

**End-to-end verification:** § E2E ran verbatim, once, captured to `/tmp/e2e-03c.txt` and pasted into the `end-to-end verification` Update Log entry with the `PASTE MATCH` self-check confirmed — all four named tests `1 passed` each, `doc_truth` 16 passed, clippy clean, `fmt_exit=0`, full suite `passed=2390 failed=0` (2387 + the 3 new tests, rename adding nothing). Committed as `feat: open_shell(rows, cols) opens a shell sized for the program`; `git status` clean.

**Notes for review:** (1) `open_shell_with` (the injected-spawn test path) resolves its sentence size as `resolve_size(None, None, spawn_size())` and therefore names the config default, not the fixture's 24×80 — under `cargo test` with no `[shells]` table that is 40×120, which no existing `open_shell_with` assertion pins, so nothing needed renaming there. (2) The `PendingCall::OpenShell` panel-detail arm (`ui_detail`) gained a variant that shows `Some("host") rows=50 cols=160` when both sizes are present — approval panels now show a sized open; no test pinned the old shape and no assertion changed besides the one the spec listed.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.41s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
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
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
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
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (1980406): No such process
kill: (-1981142): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +1 -1
- `assets/prompts/sre.toml` — +7 -1
- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-03c-open-shell-size.md` — +26 -1
- `src/ai/tools/args.rs` — +6 -0
- `src/ai/tools/defs.rs` — +27 -9
- `src/ai/tools/dispatch.rs` — +27 -0
- `src/ai/types/events.rs` — +5 -1
- `src/ai/types/pending.rs` — +19 -2
- `src/daemon/executor/knowledge/shell.rs` — +130 -52
- `src/daemon/executor/mod.rs` — +5 -5
- `src/daemon/server/shells.rs` — +13 -9
- `src/daemon/stream.rs` — +4 -0

**Commit:** e918fbbb9a9a88d57104633c21588e2c447dc0b5

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-17

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (one dispatch, 523 turns)
- **Scope deviations:** none
- **Calibration:** turn cost, held for the retrospective. Every phase that
  threads a new tool parameter through the six layers (args → event →
  pending → stream → defs → executor arm) ran long: 02e 263, 03a 587,
  03c 523, against 92 for 02g and 120 for 03b, which touched no tool
  surface. Correctness was not the problem: all three passed, and 03a's
  bounce was the spec's fault. Independent re-run: 2390 passed / 0
  failed, fmt and clippy clean. `resolve_size` routes through
  `ShellsConfig::size` (one clamp source, `u16` saturating before the
  clamp); the production sized spawns pass the size through and the
  test host ignores it, as specified.
