# Phase 03b: geometry — shells spawn at `[shells] rows` × `cols`, default 40×120

**Milestone:** M28 — Interactive shells
**Status:** done
**Depends on:** phase-03a
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Every shell the daemon spawns is 24×80, which cramps every modern TUI, and
Claude Code most of all. This phase adds `rows` and `cols` to `[shells]`
(default **40×120**). Every production spawn path reads them through one
helper. The model's `open_shell(rows, cols)` is phase 03c.

## Architecture references

- `docs/design/interactive-shells.md` D10: the defaults, and that every
  spawn path reads them. The `Resize` frame and attached mode's resize are
  unchanged. Tests that pin 24×80 keep passing it explicitly.

## Spec

1. **Config.** `ShellsConfig` (`src/config/types.rs`) gains `rows: u16`
   (default 40) and `cols: u16` (default 120), each with a serde default
   function like the existing ones. Add a method
   `ShellsConfig::size(&self) -> (u16, u16)` that clamps rows to `10..=200`
   and cols to `40..=500`, so a config of `rows = 0` cannot spawn an
   unusable terminal.
2. **One helper.** Add `pub fn spawn_size() -> (u16, u16)` in
   `src/shell/session.rs`, re-exported from `src/shell/mod.rs`. It returns
   `Config::load()`'s `shells.size()`, or `ShellsConfig::default().size()`
   when the load fails. `ShellsConfig` already has a hand-written
   `impl Default`: add the two fields to it, so that `default().size()`
   is `(40, 120)`. Also correct the struct's doc comment, which still says
   nothing in the daemon reads these values.
3. **The spawn sites.** Each of these 13 production calls passes
   `24, 80` or `TEST_ROWS, TEST_COLS`. Replace the size with
   `spawn_size()`. The list comes from the grep in § Acceptance criteria
   and is complete:
   - `src/daemon/executor/mod.rs`: 2;
   - `src/daemon/scheduled.rs`: 2;
   - `src/daemon/executor/bg_shell.rs`: 2;
   - `src/daemon/executor/foreground.rs`: 3;
   - `src/daemon/executor/file_ops/read.rs`: 1;
   - `src/daemon/executor/file_ops/ops.rs`: 1;
   - `src/daemon/executor/knowledge/shell.rs`: 2. These are the
     `#[cfg(not(test))]` `spawn_in_process_host` and `spawn_on_named_host`.
     `TEST_ROWS` / `TEST_COLS` stay for the test-only in-process host.

   Also change the fallback in `src/daemon/server/shells.rs`, where a host
   that cannot answer `Status` "reads as the spawn default": it uses
   `spawn_size()` now.
4. **Docs.** Add `rows` and `cols` to the `[shells]` block of
   `assets/etc/config.toml` and to the README's `[shells]` table and
   example, next to `scrollback_lines`. The
   `seeded_config_template_documents_every_config_field` test in
   `tests/doc_truth.rs` enforces the template and must pass unmodified.
   While you are in that block, delete its stale line "Nothing here takes
   effect yet; later M20 phases consume these values."
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03b.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Do not touch:**
- `PtyShell::spawn(..., (24, 80))` in `src/daemon/executor/shell_exec.rs`.
  It is not a shell host.
- The `shell-host` CLI's `--rows` / `--cols` defaults in `src/main.rs`.
  Every spawn passes explicit sizes.
- Any test that passes 24×80 explicitly.

## Tests

- `shells_config_defaults_to_40_by_120` (`src/config/mod.rs` tests): an
  empty `[shells]` table gives rows 40 and cols 120; `rows = 50` and
  `cols = 200` parse; `ShellsConfig::default().size() == (40, 120)`.
- `shells_size_clamps_nonsense`: rows 0 and cols 0 ⇒ `(10, 40)`; rows
  9999 and cols 9999 ⇒ `(200, 500)`; `(50, 200)` is unchanged.

## Acceptance criteria

- [ ] The 2 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `grep -rnE 'spawn_(detached|ssh|sbx|ghost_ssh)_host\([^)]*(24, 80|TEST_ROWS, TEST_COLS)\)' src | wc -l`
      ⇒ `0` (13 today).
- [ ] `cargo test` ⇒ `passed=2387 failed=0`: exactly 2385 at `02bf9a12`
      plus the 2 new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03b.txt; : > "$A"
for t in shells_config_defaults_to_40_by_120 shells_size_clamps_nonsense; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "fixed_size_spawns=$(grep -rnE 'spawn_(detached|ssh|sbx|ghost_ssh)_host\([^)]*(24, 80|TEST_ROWS, TEST_COLS)\)' src | wc -l)" >> "$A"
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
D=docs/dev/milestones/M28-interactive-shells/phase-03b-geometry.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03b.txt
diff /tmp/pasted-03b.txt /tmp/e2e-03b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `README.md` (the `[shells]` table and example only).

## Out of scope

- `open_shell(rows, cols)` and the result naming the size: phase 03c.
- `run_terminal_command(interactive: true)` and the ssh auto-upgrade:
  phase 03d.

## Update Log

### Update — 2026-10-16 20:57 (progress)

Executor Claude (Opus 4.6, rexyMCP run) started phase-03b. Status flipped to
in-progress (README phase table to match). Tasks 1–4 implemented: `rows`/`cols`
on `ShellsConfig` with `size()` clamping (10..=200 rows, 40..=500 cols);
`spawn_size()` in `src/shell/session.rs`, re-exported from `src/shell/mod.rs`;
all 13 production spawn sites plus `server/shells.rs`'s unreadable-host
fallback now read it (`TEST_ROWS`/`TEST_COLS` stay test-only, `#[cfg(test)]`-gated
after they went dead in the non-test build); `[shells]` docs in the template
and README, stale "nothing takes effect yet" line deleted;
`shells_config_defaults_to_40_by_120` and `shells_size_clamps_nonsense` added.
Gates green: fmt, clippy -D warnings, full suite passed=2387 failed=0.
End-to-end capture and the started commit follow.

### Update — 2026-10-16 20:57 (end-to-end verification)

`§ End-to-end verification` run verbatim, output redirected to `/tmp/e2e-03b.txt`
and pasted below unedited; the `PASTE MATCH` line follows the fence.

```
shells_config_defaults_to_40_by_120: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shells_size_clamps_nonsense: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fixed_size_spawns=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2387 failed=0
```

PASTE MATCH

<!-- entries appended below this line -->

### Update — 2026-10-16 20:18 (complete, server-authored)

**Summary:** </think>Everything is complete. Let me do the final checklist:

- [x] Phase doc Status: in-progress (not flipped to review) ✓
- [x] Milestone README phase-table row says in-progress ✓
- [x] No hand-written (complete) entry ✓ (I wrote a progress entry and the end-to-end verification entry, which the phase spec Task 5 requires — that's the E2E entry, not a completion entry)
- [x] cargo fmt --all run before git add ✓
- [x] All verification commands ran clean ✓ (fmt_exit=0, clippy finished clean, doc_truth 16 passed, full suite passed=2387 failed=0)
- [x] Final message = Summary + Notes for review — this message
- [x] git status --short shows nothing ✓
- [x] git log -1 --stat shows all touched files ✓ (16 files)

One check: the acceptance criterion "cargo test ⇒ passed=2387 failed=0: exactly 2385 at 02bf9a12 plus the 2 new tests" — captured passed=2387 failed=0. ✓

Grep for the pinned literal — the spec pins `spawn_size` as the helper and the acceptance grep pins the `24, 80`/`TEST_ROWS, TEST_COLS` patterns at the spawn sites: `fixed_size_spawns=0` ✓, and `spawn_size` appears at all 14 sites (13 spawn sites + shells.rs fallback) as the grep above shows.

Now write the final Summary + Notes for review message.</think>
**Summary + Notes for review**

Phase-03b is implemented and committed (`57281817 feat: shells spawn at [shells] rows x cols, default 40x120`, working tree clean). `ShellsConfig` gains `rows`/`cols` (serde defaults 40/120) with a `size()` that clamps to `10..=200`/`40..=500`; `spawn_size()` lives in `src/shell/session.rs` and is re-exported from `src/shell/mod.rs`; all 13 production spawn sites plus the `server/shells.rs` unreadable-host fallback now read it, and the template/README document both knobs with the stale "nothing here takes effect yet" line deleted. The two new tests pass individually and the full suite reports `passed=2387 failed=0` (2385 baseline + 2); the acceptance grep for pinned 24×80/TEST_ROWS spawn literals returns `fixed_size_spawns=0`; `doc_truth` (16 tests, file unmodified), clippy `-D warnings`, `cargo fmt --all --check` (exit 0) all green; end-to-end output was captured to `/tmp/e2e-03b.txt` per the spec's block and pasted into the phase doc's `### Update — 2026-10-16 20:57 (end-to-end verification)` entry with its `PASTE MATCH` line confirmed by the self-check.

**Notes for review:** two small adaptations beyond the letter of the spec — (1) after the spawn sites stopped using `TEST_ROWS`/`TEST_COLS`, the constants went dead in the non-test build (two `dead_code` warnings), so I gated them `#[cfg(test)]` and noted in their doc comment that production reads `spawn_size()`; they keep their exact value (24×80) for the test-only in-process host as required. (2) `spawn_size`'s `Config::load()` failure branch returns `ShellsConfig::default().size()` via `unwrap_or_else`, matching the spec's "or the default when load fails". Also note the spec's "Do not touch" list is respected: `PtyShell::spawn(..., (24, 80))` in `shell_exec.rs`, the `shell-host` CLI's `--rows`/`--cols` defaults, and every explicitly-24×80 test are unchanged. One adjacent observation, not acted on: `src/shell/session.rs:597` and other PTY-backend test spawns still pass 24×80 explicitly — consistent with D10's "tests pin the size explicitly", no action taken.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
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
test g4_briefing_injects_on_next_run ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.07s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


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
kill: (1834945): No such process
kill: (-1835677): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +4 -0
- `assets/etc/config.toml` — +5 -2
- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-03b-geometry.md` — +31 -2
- `src/config/mod.rs` — +24 -0
- `src/config/types.rs` — +30 -2
- `src/daemon/executor/bg_shell.rs` — +6 -3
- `src/daemon/executor/file_ops/ops.rs` — +2 -1
- `src/daemon/executor/file_ops/read.rs` — +4 -1
- `src/daemon/executor/foreground.rs` — +10 -3
- `src/daemon/executor/knowledge/shell.rs` — +8 -3
- `src/daemon/executor/mod.rs` — +8 -2
- `src/daemon/scheduled.rs` — +4 -2
- `src/daemon/server/shells.rs` — +1 -1
- `src/shell/mod.rs` — +1 -1
- `src/shell/session.rs` — +11 -1

**Commit:** 572818175968a6b749189bfb68e53f30a74c4fec

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-16

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (one dispatch, 120 turns)
- **Scope deviations:** none
- **Calibration:** none. Independent re-run: 2387 passed / 0 failed, fmt and
  clippy clean, fixed-size spawn grep 13 ⇒ 0. One helper (`spawn_size`)
  at every site plus the `server/shells.rs` fallback; clamps live in
  `ShellsConfig::size`; the hand-written `Default` carries 40×120; README
  table + example, config template and struct doc comment all updated.
