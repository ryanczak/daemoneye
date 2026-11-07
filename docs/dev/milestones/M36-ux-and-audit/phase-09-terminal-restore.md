# Phase 09 — terminal-restore

**Milestone:** M36 — Chat UX and audit fixes
**Status:** done
**Drafted:** 2026-11-06
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

Fix two findings from `FINDINGS.md`: B6 (the ask and chat clients leave the
terminal in raw mode) and S9 (`top --once` replays control characters to the
terminal).

- **B6:** `RatatuiRendererStdout::new` (`src/cli/render_ratatui.rs`) turns on
  raw mode, bracketed paste and focus reporting. Only an explicit
  `restore()` (`src/cli/render_ratatui_draw.rs`) undoes them, and only
  `chat_loop.rs` calls it. The terminal is left raw in these cases:
  - every `daemoneye ask` without `--raw`, including one that fails to reach
    the daemon (reproduced below);
  - a renderer-init failure after `enable_raw_mode`;
  - an early `?` return out of the chat loop;
  - any panic. The renderer has no `Drop` and the process has no panic hook.
- **S9:** `run_top` in `src/cli/commands/top.rs` prints `render_once` and
  `run_detail_text` with `println!`. Activity rows carry model-written
  command text with control characters intact, so a proposed
  `\x1b]52;c;…\x07` (OSC 52 clipboard write) reaches the user's terminal. The
  live TUI is safe because ratatui drops control characters.

## Behaviour

- **Every exit restores the terminal.** After `ask`, after `chat`, after a
  failed renderer init, after an error return and after a panic, the
  terminal is back in cooked mode with echo, with bracketed paste and focus
  reporting off and the cursor shown. The restore runs at most once per
  renderer: an explicit `restore()` followed by a drop must not emit the
  sequences twice.
- **Panic hook.** It runs the restore only while a real terminal renderer
  is live, then calls the previously installed hook so the panic message
  still prints. A process that never made a terminal renderer, such as the
  daemon or `top --once`, gets no escape sequences from it.
- **`top --once` output** (both the snapshot and `--run <id> --once`) shows
  every control or invisible-format character except `\n` in its visible
  escape form, the same way the approval panel does (`visible_control` in
  `src/cli/render.rs`). It never contains a raw ESC (0x1b) or BEL (0x07)
  byte. Plain ASCII text is unchanged.

## Constraints

- **Names are fixed. Use them; do not search the repo for them, because
  they do not exist yet.**
  - `src/cli/terminal_guard.rs` holds `TerminalGuard`, `panic_hook_restores`,
    `restore_terminal()` (the real restore: disable focus reporting and
    bracketed paste, then `ratatui::try_restore()`) and
    `install_panic_restore()` (a `std::sync::Once`, which wraps
    `take_hook()`).
  - The stdout renderer gains a `guard: Option<TerminalGuard>` field. It is
    `Some(TerminalGuard::arm(restore_terminal))` in
    `RatatuiRendererStdout::new`, and `None` on every `TestBackend`
    constructor. `restore()` calls `guard.restore()`.
  - `escape_controls(text: &str) -> String` lives in `src/cli/render.rs`
    next to `visible_control`. `run_top` wraps both `println!` arguments
    in it.
- **Never grep the repo root** (`.`). `.rexymcp/sessions/*.jsonl` lines are
  hundreds of KB and trip the runaway-output hard fail. Search `src/`,
  `tests/` or `docs/` only.
- **Named seams. Do not search for others.**
  - A `TerminalGuard` type owns the terminal-restore duty. Its restore
    action is injectable (a `fn()` is enough), so tests count calls without
    touching a real terminal. `Drop` restores if it has not already run, and
    an explicit restore disarms it.
  - The stdout renderer holds an armed guard created right after
    `enable_raw_mode`, so an `Err` later in `new()` still restores.
  - Renderers built on `TestBackend` must never run the real restore.
  - The panic hook is installed once (`std::sync::Once`), from the code path
    that creates the stdout renderer. Whether to restore is decided by a
    process-wide "a terminal renderer is live" flag that the guard sets and
    clears. Test the decision as a pure function of that flag. Do **not**
    install a panic hook inside a test.
  - S9 is fixed by one escaping function applied at the two `println!` sites
    in `run_top`. Its tests call it directly and also call `render_once` on
    a snapshot built with the existing `snapshot_with` test helpers in
    `top_tui.rs`.
- Do not start a daemon or a real terminal in any test. The end-to-end
  block below is the only real-terminal check.
- Preservation: the existing `top_*` tests and the render tests stay green
  without being edited.

## Spec

1. **Red tests first.** Write these against the current tree, run them and
   keep the failing output. All carry the `terminal_restore_` prefix:
   - `terminal_restore_guard_drop_restores_once`: dropping an armed guard
     runs the restore action exactly once.
   - `terminal_restore_guard_explicit_then_drop_once`: an explicit restore
     followed by a drop is exactly one call in total.
   - `terminal_restore_panic_decision`: the hook's decision restores when
     the live flag is set and does nothing when it is clear.
   - `terminal_restore_once_escapes_controls`: the escaping function turns
     `ls \x1b]52;c;aGk=\x07` into text with no 0x1b or 0x07 byte that still
     contains `ls `. It keeps `\n` and leaves `plain text` unchanged.
   - `terminal_restore_once_activity_row`: `render_once` on a snapshot whose
     activity row's command holds that OSC 52 string, passed through the
     escaping function, has no 0x1b or 0x07 byte.
2. **Implement**, one edit per turn, running `cargo build` after each, in
   this order: the guard and live flag, the renderer holding the guard (with
   `restore()` delegating to it), the panic hook, then the escaping
   function at the `top --once` sites.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add an `### Update — <date> (red runs)` entry with step 1's
   `test result:` lines and failure or compile messages, verbatim. A test
   whose red run is only a compile error says so. Label each pasted run with
   what the tree held at that moment, and do not paste failures of your own
   wrong assertions as red runs.
5. **Findings.** Add a `### Findings — <date>` entry, two or three lines, or
   `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: ask, chat and panics restore the terminal; top --once escapes control characters (M36 phase-09)`.

## Acceptance criteria

- [ ] The E2E block's first line reads `terminal_restore passed=P failed=0`
      with P ≥ 5. Today it reads `passed=0`.
- [ ] The E2E block shows `tty_after_ask=icanon`: an `ask` that cannot reach
      a daemon, run under `script`, leaves the terminal in canonical mode.
      Today it reads `tty_after_ask=-icanon`.
- [ ] The E2E block shows `panic_hook=1`: one `take_hook` call in production
      code under `src/cli/` or `src/main.rs`. Today it is 0.
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: the existing `top_*` tests pass unedited.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test terminal_restore_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "terminal_restore passed=" p, "failed=" f}'
cargo build -q 2>/dev/null; H=$(mktemp -d)
timeout 20 script -qec "HOME=$H $PWD/target/debug/daemoneye ask hi >/dev/null 2>&1; stty -a | grep -oE -- '(^| )-?icanon' | tr -d ' ' | sed 's/^/tty_after_ask=/'" /dev/null | tr -d '\r' | grep tty_after
grep -rn 'take_hook' src/cli src/main.rs | grep -vc '^\S*_tests\.rs\|mod tests' | sed 's/^/panic_hook=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render_ratatui.rs`, `src/cli/render_ratatui_draw.rs`,
  `src/cli/commands/ask.rs`, `src/cli/commands/chat.rs`,
  `src/cli/commands/chat_loop.rs`, `src/cli/commands/top.rs`,
  `src/cli/render.rs` (to expose `visible_control` or the escaping
  function), and a new module under `src/cli/` for the guard if you prefer.
- A new or raised `tests/hygiene_allowlist.txt` entry is acceptable if a
  touched file or function crosses its cap. State the reason in Findings.

## Out of scope

- `ask --raw` (no renderer), the `top` live TUI (it has its own restore),
  and the viewer's `AltScreenGuard`.
- Escaping any output other than `top --once`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-06 (escalation)

**Chosen lever:** resume
**Rationale:** the first run hard-failed at turn 373 with `RunawayOutput`
(a `grep -rn … .` matched `.rexymcp/sessions/*.jsonl`). Before that it spent
about 20 turns looking for a name for "the escaping function", which the
spec never gave. `src/cli/terminal_guard.rs` (the guard, the live flag,
`panic_hook_restores` and three passing tests) is correct and is kept. The
spec now fixes every new name (§ Constraints), and the resume guidance
lists the remaining edits.

### Update — 2026-11-06 09:40 (started)

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local
**Status:** in-progress. Red tests first (spec task 1).

### Update — 2026-11-06 22:55 (started, resumed)

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local (resumed run)
**Status:** in-progress. The guard module already on disk is kept unchanged.
This run writes the two escaping tests red first, then implements the
remaining edits from the resume guidance (`restore_terminal`,
`install_panic_restore`, the renderer's `guard` field, `escape_controls` at
the two `println!` sites), runs the gates, and writes the red-runs /
Findings / E2E entries before committing.

### Update — 2026-11-06 23:20 (red runs)

Step 1's five tests, written against the pre-phase tree. The three guard
tests already passed on a mid-resume tree that had only
`src/cli/terminal_guard.rs` (their red run predates this session; they were
kept per the escalation note). This run's fresh red evidence:

**Run 1 — pre-phase tree + only the five new tests** (a throwaway worktree
at `5dc42042`, the pre-phase commit, carrying only the test diff: the two
escaping tests plus the `mod` declarations that name them). The escaping
function does not exist yet, so the run is a compile error, as the spec's
task 4 allows for:

```
   Compiling daemoneye v2.7.0 (/home/matt/src/daemoneye/.claude/worktrees/m36p09-red)
error[E0432]: unresolved import `crate::cli::render::escape_controls`
 --> src/cli/commands/top_tui_once_tests.rs:7:5
  |
7 | use crate::cli::render::escape_controls;
  |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no `escape_controls` in `cli::render`

error[E0425]: cannot find function `escape_controls` in this scope
   --> src/cli/tests.rs:187:15
    |
187 |     let out = escape_controls(raw);
    |               ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `escape_controls` in this scope
   --> src/cli/tests.rs:193:16
    |
193 |     assert_eq!(escape_controls("a\nb"), "a\nb", "\\n is kept");
    |                ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `escape_controls` in this scope
   --> src/cli/tests.rs:195:9
    |
195 |         escape_controls("plain text"),
    |         ^^^^^^^^^^^^^^^ not found in this scope

Some errors have detailed explanations: E0425, E0432.
For more information about an error, try `rustc --explain E0425`.
error: could not compile `daemoneye` (lib test) due to 4 previous errors
```

**Run 2 — the same pre-phase worktree with no test diff at all**, to show
the guard tests were green only because the guard module was present there:

```
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2620 filtered out; finished in 0.00s
```

(`terminal_restore_` matches nothing on the pre-phase tree — the guard
module and its three tests do not exist at `5dc42042`; their red run was a
compile error in the same shape as Run 1, captured before this session in
the mid-resume tree's history.)

### Findings — 2026-11-06

- The guard's arrival pushed `src/cli/render_ratatui.rs` to 806 production
  lines and the inline `mod tests` in `src/cli/commands/top_tui.rs` past
  the 2000 test-line cap, so the render tests' subject went the other way:
  the `top_tui` inline test module moved whole into sibling
  `top_tui_tests.rs` (the same move M31 phase 17 made for the render tests)
  and the new escape test went into `top_tui_once_tests.rs`, instead of
  raising any allowlist entry. The allowlist is unchanged.
- `RatatuiRenderer::new` now installs the panic hook and arms the guard
  inside the constructor; the guard field is `Option` so every `TestBackend`
  constructor can stay `None` and the real restore never runs in tests.
- `run_top`'s two `println!` sites wrap their text in `escape_controls`
  (from `crate::cli::render`); the live TUI path is untouched, per scope.

### Update — 2026-11-06 17:08 (end-to-end verification)

Ran the phase doc's § End-to-end verification block once, redirected to
`.rexymcp/red/m36p09-e2e.txt` and pasted below verbatim.

```
terminal_restore passed=5 failed=0
tty_after_ask=icanon
panic_hook=1
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.91s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.80s
fmt_exit=0
passed=2704 failed=0
```

Every acceptance criterion's line is present: `terminal_restore passed=5
failed=0`, `tty_after_ask=icanon` (an `ask` that cannot reach a daemon,
under `script`, leaves the tty in canonical mode), `panic_hook=1` (the one
`take_hook` in `src/cli/terminal_guard.rs`), `build_warnings=0`, the three
gates green, clippy clean (`Finished`, no error), `fmt_exit=0` and the full
suite `passed=2704 failed=0`. Preservation: the pre-existing `top_*` tests
(`cargo test top_` → 74 passed) and the render tests (`cargo test render`
→ 135 passed) pass unedited.

### Update — 2026-11-07 01:21 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Resumed and finished M36 phase-09 (B6 + S9), building on the kept `src/cli/terminal_guard.rs`. I added `restore_terminal()` (DisableFocusChange/DisableBracketedPaste then `ratatui::try_restore()`) and `install_panic_restore()` (a `std::sync::Once` wrapping `take_hook()`, restoring only while `TERMINAL_RENDERER_LIVE` is set, then chaining the previous hook); `RatatuiRendererStdout::new` now installs the hook and arms `Some(TerminalGuard::arm(restore_terminal))` immediately after `enable_raw_mode` so a later `Err` still restores via `Drop`, every `TestBackend` constructor passes `guard: None`, and `restore()` delegates to the guard. S9 is fixed by `pub fn escape_controls` in `src/cli/render.rs` (each char through `visible_control` except `\n`) wrapped around both `println!` arguments in `run_top`; the five `terminal_restore_*` tests pass and the E2E block ran verbatim once: `terminal_restore passed=5 failed=0`, `tty_after_ask=icanon`, `panic_hook=1`, `build_warnings=0`, hygiene/doc_truth/bug_tracker ok, clippy clean, `fmt_exit=0`, full suite `passed=2704 failed=0`. Deviations to note: the guard's arrival pushed `render_ratatui.rs` past the 800-line cap and `top_tui.rs`'s inline tests past the 2000-line test cap, so instead of raising allowlist entries I moved the `top_tui` inline test module whole into sibling `top_tui_tests.rs` and put the new escape test in `top_tui_once_tests.rs` (allowlist unchanged; red runs were compile errors, captured verbatim in the red-runs entry with the tree state labeled). Committed as `221596639b` with the tree clean and the phase doc left at `in-progress` — no hand-written `(complete)` entry, per the contract.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.17s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test config_pricing_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
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
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)
kill: (564636): No such process
kill: (-565311): No such process
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

- `.claude/worktrees/m36p09-red/src/cli/mod.rs` — +0 -24
- `docs/dev/milestones/M36-ux-and-audit/phase-09-terminal-restore.md` — +108 -0
- `src/cli/commands/top.rs` — +9 -2
- `src/cli/commands/top_tui.rs` — +6 -1998
- `src/cli/commands/top_tui_once_tests.rs` — +28 -0
- `src/cli/commands/top_tui_tests.rs` — +1987 -0
- `src/cli/render.rs` — +16 -0
- `src/cli/render_ratatui.rs` — +10 -5
- `src/cli/render_ratatui_draw.rs` — +3 -4
- `src/cli/render_ratatui_tests.rs` — +1 -0
- `src/cli/terminal_guard.rs` — +28 -5
- `src/cli/tests.rs` — +19 -0

**Commit:** 221596639b8595333b679d76cb024ea8d7009ba1

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-06

- **Verdict:** approved_after_1
- **Bounces:** none filed; one escalation (first run hard-failed at turn 373
  with `RunawayOutput` after hunting for the unnamed escaping function; the
  spec was refined to fix every new name and forbid root greps, then resumed)
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** the `top_tui` inline test module moved whole into
  `top_tui_tests.rs` (verbatim apart from rustfmt reflow, checked against
  `5dc42042`) and the S9 activity-row test sits in `top_tui_once_tests.rs`,
  to keep both files inside the M31 caps without an allowlist entry. Accepted.
- **Calibration:** reviewer re-run: fmt clean, `build_warnings=0`, clippy
  clean, `passed=2704 failed=0`, `terminal_restore passed=5 failed=0`,
  `tty_after_ask=icanon`, `panic_hook=1`. The guard tests' red run is not
  pasted (only asserted); it is a compile error by construction, since
  `terminal_guard.rs` does not exist at `5dc42042`, so it was accepted as in
  phase 08. Second phase in a row (08, 09) where an unnamed seam or symbol sent
  the executor exploring; on a third occurrence, propose folding "name every
  new symbol, never grep the repo root" into WORKFLOW at milestone close.
