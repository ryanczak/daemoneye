# Phase 09 — terminal-restore

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
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
