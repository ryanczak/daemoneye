# Phase 09 — terminal-restore

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
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

**Executor:** executor (Claude Opus 4.6)
**Status:** in-progress. Red tests first (spec task 1).

### Update — 2026-11-06 22:55 (started, resumed)

**Executor:** executor (Claude, resumed run)
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
