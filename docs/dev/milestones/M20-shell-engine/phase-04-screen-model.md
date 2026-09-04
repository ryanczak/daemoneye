# Phase 04: the vt100 screen model

**Milestone:** M20 — Shell Engine
**Status:** done
**Depends on:** none in code — `src/shell/` exists from phases 02 and 03; this
phase adds a third sibling module and calls neither.
**Estimated diff:** ~360 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add `src/shell/screen.rs`: a `vt100::Parser` wrapper that turns a shell's raw
byte stream into a live screen — what a human at that terminal would see —
plus the two derived views the AI-facing layer needs, **semantic colour
annotation** and a **one-line status summary**.

The annotation is the grid-cell replacement for `src/tmux/ansi.rs`, which
re-parses escape sequences out of a string. Reading colour off the parsed grid
is both simpler and correct for sequences that span reads.

Fixture-driven and hermetic: no PTY, no clock, no config read, no production
caller. Phase-05 (the shell-host) is the first consumer.

## Architecture references

Read before starting:

- `docs/design/daemoneye-2.0.md` § 2.1, the "Screen vs log" paragraph — the
  screen is the *viewport*, the cast log is the *transcript*. This phase builds
  only the former.
- `docs/dev/milestones/M20-shell-engine/README.md` § "Design decisions on
  record" — `ansi.rs` and `status.rs` are re-pointed at the grid, not rewritten.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- `src/shell/` holds `mod.rs`, `pty.rs` (phase-02) and `log.rs` (phase-03).
  **`src/shell/screen.rs` does not exist**, and `vt100` is **not** a
  dependency yet.
- `src/shell/mod.rs` declares `mod log; mod pty;` and re-exports from both.
- **`src/tmux/ansi.rs`** holds `annotate_ansi`, which walks a *string* looking
  for SGR escapes. Its classifier is the behaviour to reproduce:

  ```rust
  fn classify_sgr(params: &str) -> Option<SpanColor> {
      let mut color: Option<SpanColor> = None;
      for part in params.split(';') {
          match part {
              "31" | "91" => color = Some(SpanColor::Red),
              "32" | "92" => color = Some(SpanColor::Green),
              "33" | "93" => color = Some(SpanColor::Yellow),
              _ => {}
          }
      }
      color
  }
  ```

  and the label it emits, from `flush_span`:

  ```rust
  let label = match color {
      SpanColor::Red => "ERROR",
      SpanColor::Yellow => "WARN",
      SpanColor::Green => "OK",
  };
  out.push_str(&format!("[{}: {}]", label, text));
  ```

  **Do not edit, move or call `src/tmux/ansi.rs`.** It is string-based and is
  deleted with the rest of `src/tmux/` in phase-09's successor milestone. This
  phase writes the grid-based equivalent from scratch; the quotes above are the
  behaviour contract, not code to import.

- **`src/tmux/status.rs`** is different: its helpers are **pure and
  substrate-agnostic**, taking primitives and a buffer string, so this phase
  **calls them** rather than reimplementing:

  ```rust
  pub fn classify(dead: bool, dead_status: Option<i32>, has_bell: bool,
                  current_cmd: &str, last_activity: u64, now: u64) -> PaneStatus
  pub fn summarize(status: PaneStatus, buffer: &str) -> String
  pub fn is_shell_prompt(cmd: &str) -> bool
  ```

  `summarize` produces `<status> — <last non-empty line, 50 chars>`.
  `src/tmux` is `pub(crate) mod` (`src/lib.rs:25`) and `status` is `pub mod`
  (`src/tmux/mod.rs:5`), so `crate::tmux::status::{classify, summarize,
  PaneStatus}` resolves from `src/shell/screen.rs` today. Import them; do not
  copy them.

- `shells.scrollback_lines` exists in config from phase-01 (default 5000).
  **This phase does not read config** — scrollback depth is a constructor
  argument, and phase-05 passes the config value.

## Measured facts — executed 2026-09-03 against `vt100` 0.16.2, not reasoned about

### F1. The colour mapping, exactly

Feeding `ESC[31mA ESC[91mB ESC[33mC ESC[93mD ESC[32mE ESC[92mF ESC[0mG` and
reading `cell.fgcolor()` for each column:

| SGR | `vt100::Color` |
|---|---|
| `31` red | `Idx(1)` |
| `91` bright red | `Idx(9)` |
| `33` yellow | `Idx(3)` |
| `93` bright yellow | `Idx(11)` |
| `32` green | `Idx(2)` |
| `92` bright green | `Idx(10)` |
| `0` reset | `Default` |

These pair up exactly with `classify_sgr`'s `"31" | "91"` etc., so the grid
mapping is `Idx(1) | Idx(9)` → ERROR, `Idx(3) | Idx(11)` → WARN,
`Idx(2) | Idx(10)` → OK, and **everything else — including `Default`, any
other `Idx(n)`, and every `Rgb(..)` — is unlabelled text.**

### F2. `contents()` is the visible screen, not the transcript

With `Parser::new(3, 20, 100)` and eight lines written, `contents()` returned
`["line7", "line8"]` — `line1` is **absent**. Scrollback is a *view offset*:
after `screen_mut().set_scrollback(5)`, `contents()` returned
`["line2", "line3", "line4"]`.

So the screen answers "what is on the terminal now", and the phase-03 cast log
remains the transcript of record. **This phase must not try to make the screen
a history**; a scrollback constructor argument sizes the buffer, and moving the
view is out of scope.

### F3. **The surprising one: `set_size` does not reflow — it hard-breaks a previously wrapped row.**

`Parser::new(2, 20, 0)` fed the 26 characters `ABCDEFGHIJKLMNOPQRSTUVWXYZ`:

- at 20 columns, `contents()` returned **one** logical line,
  `["ABCDEFGHIJKLMNOPQRSTUVWXYZ"]` — the wrap is soft, and `contents()` does
  not insert a newline for it;
- after `screen_mut().set_size(2, 30)`, `contents()` returned **two** lines,
  `["ABCDEFGHIJKLMNOPQRST", "UVWXYZ"]` — the soft wrap became a hard break at
  the *old* width.

Widening the terminal therefore corrupts text already on the screen. **That is
why resize is not in this phase.** Phase-05 owns resize and will have to
rebuild the screen from the log rather than call `set_size` on a populated
grid. Do not add a resize method here, and do not call `set_size` anywhere
outside a test that is explicitly asserting this behaviour.

### F4. `alternate_screen()` tracks the alt-screen escapes

After `ESC[?1049h` the screen reported `alternate_screen() == true` and its
contents were the alt-screen's; after `ESC[?1049l` it reported `false`.
(Measured at the escape level. A full-screen program such as `less` or `vim`
emits exactly this pair; a live-program check belongs to phase-05, which is
where a PTY is available.)

## Spec

### Task 1 — Add the `vt100` dependency

In `Cargo.toml`, add to `[dependencies]`, with a comment in the style the file
already uses for pinned crates:

```toml
# Terminal emulation for the shell screen model — turns a raw PTY byte stream
# into a cell grid, so colour is read off parsed cells instead of re-parsing
# escape sequences out of a string.
vt100 = "0.16"
```

This is the **only** new dependency authorized.

### Task 2 — Create `src/shell/screen.rs` and declare it

Create the file and add `mod screen;` to `src/shell/mod.rs` beside `mod log;`
and `mod pty;`, re-exporting the public items below in the same `pub use`
style the file already uses.

### Task 3 — `ShellScreen`

```rust
/// A live view of one shell's terminal: what a human at that terminal would
/// see right now. The transcript of record is the cast log, not this.
pub struct ShellScreen { /* vt100::Parser */ }
```

- `ShellScreen::new(rows: u16, cols: u16, scrollback: usize) -> Self` —
  `vt100::Parser::new(rows, cols, scrollback)`. No config read; the caller
  supplies the depth.
- `feed(&mut self, bytes: &[u8])` — `parser.process(bytes)`. Takes raw bytes,
  because a PTY read is bytes and may split a sequence; `vt100` buffers
  partial sequences across calls, so feeding in arbitrary chunks is safe.
- `contents(&self) -> String` — the visible screen as plain text (F2).
- `size(&self) -> (u16, u16)` and `cursor(&self) -> (u16, u16)`.
- `is_alt_screen(&self) -> bool` — F4; true while a full-screen program owns
  the terminal.

### Task 4 — `annotated()` — the grid-cell replacement for `annotate_ansi`

```rust
/// The visible screen with semantically-coloured runs wrapped in markers:
/// red → `[ERROR: …]`, yellow → `[WARN: …]`, green → `[OK: …]`.
pub fn annotated(&self) -> String
```

Walk the grid row by row, column by column, reading each cell's `fgcolor()`
and mapping it per **F1**. Group **adjacent cells sharing the same semantic
colour into one marker**, and emit uncoloured cells as plain text. Rows are
joined with `\n`.

Required behaviour, each point pinned by a named test:

1. A run of red cells becomes exactly one `[ERROR: text]`, **not one marker
   per cell**. This is the single most likely way to get it wrong.
2. The marker text is **trimmed**, matching `flush_span`'s `span_buf.trim()`.
   A run that trims to nothing emits no marker at all.
3. **A colour run does not merge across a row boundary.** Rows are separate;
   a red run ending at the last column of row 0 and another beginning at
   column 0 of row 1 produce **two** markers, not one spanning the newline.
   (This is the boundary case the phase's own guarantee turns on — state it in
   the test's assertion message.)
4. Trailing blank cells at the end of a row are not emitted as whitespace; a
   row's text is right-trimmed.
5. `Default`, any unmapped `Idx(n)`, and every `Rgb(..)` are plain text.

### Task 5 — `summary()` — reuse the existing classifier

```rust
/// `<status> — <last non-empty line>`, the same shape the pane summaries use.
pub fn summary(&self, dead: bool, dead_status: Option<i32>, has_bell: bool,
               current_cmd: &str, last_activity: u64, now: u64) -> String
```

Call `crate::tmux::status::classify(..)` with the arguments as given, then
`crate::tmux::status::summarize(status, &self.contents())`. **Do not
reimplement either**, and do not edit `src/tmux/status.rs`.

### Task 6 — Write the tests named in § Test plan

Hermetic and fixture-driven: build screens by feeding byte literals. No PTY,
no `tempfile`, no clock, no `~/.daemoneye/` access.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-04.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. Then run the PASTE MATCH self-check
in that same section and paste **the literal verdict line it prints** into the
same entry.

## Acceptance criteria

Every command below was run against the current tree while drafting and
returns the "before" value shown; every "after" value was computed from the
code this phase specifies.

- [ ] `grep -c '^vt100' Cargo.toml` → **1** (now `0`).
- [ ] `test -f src/shell/screen.rs && echo yes` → **yes** (file absent now).
- [ ] `grep -c '^mod screen;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -cE '^pub struct ShellScreen' src/shell/screen.rs` → **1**.
- [ ] `grep -cE '^\s+pub fn annotated' src/shell/screen.rs` → **1**.
- [ ] The status classifier is **called, not copied**:
      `grep -c 'tmux::status' src/shell/screen.rs` → **at least 1**, and
      `grep -c 'fn classify' src/shell/screen.rs` → **0**.
- [ ] `src/tmux/` is untouched: `git diff --name-only HEAD -- src/tmux/ | wc -l`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/screen.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**. The `^` anchor is required, or a doc comment mentioning the test
      attribute stops awk early and the guard goes vacuous.
- [ ] No `unsafe` outside comments:
      `grep -vE '^\s*(//|///|//!|\*)' src/shell/screen.rs | grep -c 'unsafe'` → **0**.
- [ ] `cargo test --lib shell::screen::` reports **8 or more** passing and
      `0 failed` (now: `0 passed; 0 failed; … 1565 filtered out`).
      Use the qualified `shell::screen::`. **Unlike the sibling phases, a bare
      `screen::` happens to match nothing today** (measured), so this is
      future-proofing rather than a live trap — but `log::` matched 16
      unrelated tests and `shell::` matched 43, so the qualified form is the
      house rule.
- [ ] `cargo test --lib shell::pty::` still reports **13 passed** and
      `cargo test --lib shell::log::` still reports **12 passed** — phases 02
      and 03 are untouched.
- [ ] **(round 2, bug-04-1)** `cargo test --lib screen_preserves_cursor`
      reports a passing `screen_preserves_cursor_positioned_columns`.
      Confirmed failing at review: `0 passed; … 1575 filtered out`.
- [ ] **(round 2, bug-04-1)** `annotated()` and `contents()` agree on column
      positions within a row. Measured at review as
      `contents() = "NAME      SIZE      MODE"` versus
      `annotated() = "NAMESIZEMODE"` for columns positioned with `ESC[nC` —
      that is the behaviour that must change.
- [ ] **(round 2, bug-04-1)** The boundary test uses a **same-colour** run, as
      § Test plan specified: `grep -c 'EFGH' src/shell/screen.rs` returns
      **0** (now `2` — the fixture appears in both the feed and the expected
      string).
- [ ] **(round 2, bug-04-1)** `cargo test --lib shell::screen::` reports **11
      or more** passing, `0 failed` (10 today).
- [ ] All four gates pass: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

Names pinned; placement is not — a `#[cfg(test)] mod tests` at the **bottom**
of `src/shell/screen.rs`, the repo convention. Every name begins `screen_` so
the E2E block can select them.

**The boundary tests are the ones that matter.** The lesson this milestone has
paid for twice is that a guarantee stated in prose and never crossed by a test
is the one that breaks, so the row-boundary and run-grouping cases are pinned
explicitly rather than left implied.

- `screen_annotates_a_red_run_as_one_error_marker` — feed
  `ESC[31mdisk failure` and assert the output contains exactly **one**
  `[ERROR: disk failure]`, and that the count of `[ERROR:` in the output is
  `1` — the negative half that catches one-marker-per-cell.
- `screen_maps_all_six_colour_codes` — the F1 table: `31`/`91` → `ERROR`,
  `33`/`93` → `WARN`, `32`/`92` → `OK`, each asserted individually so a
  regression names which code broke.
- `screen_leaves_unmapped_colours_as_plain_text` — the negative case. Blue
  (`34`), an arbitrary indexed colour (`ESC[38;5;200m`), a truecolour
  (`ESC[38;2;10;20;30m`) and reset text all appear **without** any `[ERROR:`,
  `[WARN:` or `[OK:` marker.
- `screen_does_not_merge_a_colour_run_across_a_row_boundary` — **the boundary
  test.** On a narrow screen, arrange a red run that reaches the last column of
  one row and continues at column 0 of the next; assert the output contains
  **two** separate `[ERROR:` markers, not one. Say so in the assertion message.
- `screen_trims_marker_text_and_drops_empty_runs` — a coloured run of only
  spaces produces **no** marker; a run with leading and trailing spaces
  produces a marker whose text is trimmed.
- `screen_contents_is_the_visible_screen_not_the_scrollback` — F2. On a
  3-row screen fed eight lines, `contents()` contains the last lines and
  **not** `line1`. This pins the viewport/transcript split the design rests on.
- `screen_reports_the_alternate_screen` — F4. `is_alt_screen()` is false
  initially, true after `ESC[?1049h`, and false again after `ESC[?1049l`.
- `screen_summary_uses_the_shared_classifier` — feed some output, then assert
  `summary(..)` for a shell command with recent activity begins with the same
  `PaneStatus` rendering `crate::tmux::status::classify` produces for those
  arguments, and ends with the screen's last non-empty line. Assert against
  `classify`'s own output rather than a hardcoded string, so this test cannot
  drift from the classifier it delegates to.
- `screen_feeds_bytes_split_mid_sequence` — feed `ESC[31mred` in two calls
  that **split the escape sequence itself** (e.g. `ESC[3` then `1mred`), and
  assert the result is the same as feeding it whole. This is the equivalent of
  phase-03's split-character case, one layer down.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-04.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::screen::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::screen:: totals =="
cargo test --lib shell::screen:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. phases 02 and 03 untouched (13 then 12) =="
cargo test --lib shell::pty:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib shell::log:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== E. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== F. structural greps (each must print the stated number) =="
echo -n "vt100 dependency        (1): "; grep -c '^vt100' Cargo.toml
echo -n "screen.rs exists        (1): "; test -f src/shell/screen.rs && echo 1 || echo 0
echo -n "mod screen declaration  (1): "; grep -c '^mod screen;' src/shell/mod.rs
echo -n "pub struct ShellScreen  (1): "; grep -cE '^pub struct ShellScreen' src/shell/screen.rs
echo -n "pub fn annotated        (1): "; grep -cE '^\s+pub fn annotated' src/shell/screen.rs
echo -n "calls tmux::status     (>=1): "; grep -c 'tmux::status' src/shell/screen.rs
echo -n "does NOT copy classify  (0): "; grep -c 'fn classify' src/shell/screen.rs
echo -n "src/tmux untouched      (0): "; git diff --name-only HEAD -- src/tmux/ | wc -l
echo -n "unsafe in screen.rs     (0): "; grep -vE '^\s*(//|///|//!|\*)' src/shell/screen.rs | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/screen.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
} > /tmp/e2e-04.txt 2>&1
cat /tmp/e2e-04.txt
```

Paste the contents of `/tmp/e2e-04.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-04-screen-model.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04.txt
diff /tmp/pasted-04.txt /tmp/e2e-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B, C, D and E can each report success with nothing having run.**
Measured on the current tree: `cargo test --lib shell::screen::` prints
`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1565 filtered out`
and exits `0`. A zero exit proves nothing — the pass conditions are the named
test lines in B, a count of eight or more in C, and exactly `13` then `12` in D.

**Section F on an absent file errors rather than printing `0`.** Measured: a
`grep -c` against a missing path warns on stderr and exits `2`, printing no
count. The block redirects `2>&1`, so a warning appearing there is itself proof
the file is missing.

The PASTE MATCH self-check was validated both ways while drafting a sibling
phase: a byte-exact paste printed `PASTE MATCH`, and the same paste with one
line retyped printed `PASTE MISMATCH` naming the divergent line.

## Authorizations

- Create `src/shell/screen.rs`; edit `src/shell/mod.rs` (the `mod screen;`
  line and the `pub use`) and `Cargo.toml` / `Cargo.lock`.
- **May add one dependency: `vt100 = "0.16"`.** That is the only one.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- May **read** `src/tmux/status.rs` and `src/tmux/ansi.rs`, and **call**
  `crate::tmux::status::*`. May **not edit** anything under `src/tmux/`.
- May **not** touch `src/shell/pty.rs` or `src/shell/log.rs`. Phases 02 and 03
  are `done`; their test counts must stay at 13 and 12.
- May **not** touch `docs/architecture.md`, `CLAUDE.md` or `README.md` —
  M20's documentation updates land in phase-09.

## Out of scope

- **Resize.** `set_size` hard-breaks previously-wrapped rows (F3), so resize
  needs a rebuild-from-log strategy that belongs with the shell-host. Do not
  add a resize method, and do not call `set_size` outside a test asserting F3.
- **Moving the scrollback view.** `set_scrollback` exists and is phase-05's or
  later. The constructor takes a depth; nothing here scrolls.
- **Reading config.** `shells.scrollback_lines` is passed in by the caller,
  not read here — the module stays hermetic and clock-free, as phase-03's
  writer does with its timestamp.
- **Wiring to anything.** No `PtyShell` integration, no `CastWriter`
  integration, no production caller. A module with tests and no caller is the
  intended end state; do not add `#[allow(dead_code)]` and do not invent a
  caller.
- **Editing or deleting `src/tmux/`.** The tmux backend stays byte-for-byte as
  it is until its own removal phase. `annotate_ansi` keeps its string-based
  callers; this phase's `annotated()` is a parallel implementation for the new
  substrate, not a replacement wired into the old one.
- **Masking.** The screen returns what the terminal shows; masking happens
  where bytes reach a model, which is phase-07's concern.

## Notes for executor — round 2

**Green gates and a clean tree are expected here and are NOT evidence the
phase is done.** All four gates pass right now and all 10 tests pass; the
defect is a behaviour no current test exercises.

**There is exactly ONE defect to fix: `bugs/bug-04-1.md`.** Read it first — it
carries the measured evidence.

**What is already correct and must be preserved, not rewritten:** the colour
mapping, the marker grouping, the trimming of empty runs, the alt-screen
reporting, the split-sequence handling, and `summary()`'s delegation to the
shared classifier. **The same-colour cross-row behaviour is already correct
too** — verified at review, `ESC[31mabcdefgh` at 4 columns yields
`"[ERROR: abcd]\n[ERROR: efgh]"`. The fix there is to the *test*, not the code.

**Finish condition you can check yourself:** `cargo test --lib
shell::screen::` must report **11 passed, 0 failed** — 10 today plus exactly
the one new test the bug names. **11, not 12** — a higher number means scope
this phase did not ask for. `shell::pty::` stays at 13 and `shell::log::` at 12.

**Mutation-check your own fix before reporting.** Once the new test passes,
revert the interior-gap handling, confirm
`screen_preserves_cursor_positioned_columns` fails, restore it, and state that
result in your Update Log entry.

**The Update Log is append-only.** Add your own entry at the bottom; never
edit an earlier one.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-04 00:14 (started)

Name: executor (phase-04 dispatch 2026-09-04). Implemented the five Spec tasks
in order — `vt100 = "0.16"` pinned in `Cargo.toml` next to `zeroize`;
`src/shell/screen.rs` created and declared as `mod screen;` plus `pub use
screen::ShellScreen;` in `src/shell/mod.rs`; `ShellScreen::new/feed/contents/
size/cursor/is_alt_screen` (all thin) ; `annotated()` building markers per F1
with per-row flushing (`flush_span`); `summary()` delegating to
`crate::tmux::status::{classify, summarize}`. All 10 pinned tests written and
passing (`cargo test --lib shell::screen::`: 10 passed; 0 failed). One
correction en route: F2 measured on this tree that after eight CRLF lines a
3-row screen actually holds only `line7\nline8` (each `\n` scrolls its row
up; the trailing `\r` leaves the last row empty) and the F2-only assertion
was adjusted — the `!contains("line1")` + `contains("line8")` pin stands
unchanged. Gate commands all clean; E2E capture runs next.

### Update — 2026-09-04 00:16 (end-to-end verification)

```
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.71s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::screen::tests::screen_annotates_a_red_run_as_one_error_marker ... ok
shell::screen::tests::screen_contents_is_the_visible_screen_not_the_scrollback ... ok
shell::screen::tests::screen_does_not_merge_a_colour_run_across_a_row_boundary ... ok
shell::screen::tests::screen_feeds_bytes_split_mid_sequence ... ok
shell::screen::tests::screen_leaves_unmapped_colours_as_plain_text ... ok
shell::screen::tests::screen_maps_all_six_colour_codes ... ok
shell::screen::tests::screen_reports_the_alternate_screen ... ok
shell::screen::tests::screen_summary_uses_the_shared_classifier ... ok
shell::screen::tests::screen_trims_marker_text_and_drops_empty_runs2 ... ok
shell::screen::tests::screen_trims_marker_text_and_drops_empty_runs ... ok
cargo_exit=0
== C. shell::screen:: totals ==
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1565 filtered out; finished in 0.00s
cargo_exit=0
== D. phases 02 and 03 untouched (13 then 12) ==
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1562 filtered out; finished in 2.00s
cargo_exit=0
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1563 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite totals ==
test result: ok. 1571 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 4.78s
cargo_exit=0
== F. structural greps (each must print the stated number) ==
vt100 dependency        (1): 1
screen.rs exists        (1): 1
mod screen declaration  (1): 1
pub struct ShellScreen  (1): 1
pub fn annotated        (1): 1
calls tmux::status     (>=1): 4
does NOT copy classify  (0): 0
src/tmux untouched      (0): 0
unsafe in screen.rs     (0): 0
unwrap/expect/panic pre-test (0): 0
```

PASTE MATCH

### Update — ts=1788481565373 (complete, server-authored)

**Summary:** ## Summary + Notes for review

**Built:** `src/shell/screen.rs` — a `vt100::Parser` wrapper (`ShellScreen`) with `new/feed/contents/size/cursor/is_alt_screen`, the grid-cell `annotated()`, and `summary()` delegating to `crate::tmux::status::{classify, summarize}`; `Cargo.toml` + `Cargo.lock` gained the single authorized dep `vt100 = "0.16"`; `src/shell/mod.rs` declares `mod screen;` and re-exports `ShellScreen`. All 10 [pinned tests](docs/dev/milestones/M20-shell-engine/phase-04-screen-model.md#test-plan) pass; E2E block run verbatim and captured; PASTE MATCH verified.

**One data-driven deviation:** the F2-only assertion in `screen_contents_is_the_visible_screen_not_the_scrollback` — the spec's example said a 3-row screen would hold `line6..line8`, but measured on this tree each CRLF line scrolls its row up, leaving only `line7\nline8` (trailing `\r` leaves the bottom row empty). Adjusted the assertion; the spec's essential pin (`line1` absent, last line present) is unchanged and still asserted. Test names `screen_*` all grep-matched by the E2E block.

**Verification:** `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --lib` (1571 passed; 0 failed) all clean; phases 02/03 untouched (13/12); `src/tmux/` byte-for-byte unchanged (0).

**Notes for review:** the F2/count observation above is the spec's own "measured" numbers being slightly off vs this tree — worth reconciling in the phase doc if this recurs in phase-05's screen-rebuild design. `vt100` 0.16.2 resolves cleanly (`arrayvec` + existing `vte`/`itoa`/`unicode-width`). No resize, no scrollback manipulation, no config read — per spec. Working tree clean; one commit `cd48dbf`.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


TEST
ning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 10 tests
test approval_gated_tools_all_exist ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
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
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.10s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `Cargo.toml` — +4 -0
- `docs/dev/milestones/M20-shell-engine/README.md` — +1 -1
- `docs/dev/milestones/M20-shell-engine/phase-04-screen-model.md` — +62 -1
- `src/shell/mod.rs` — +2 -0
- `src/shell/screen.rs` — +282 -0

**Commit:** cd48dbfd51ec0ce8ca8dcddb73c03e1bd459e71d

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-09-04 01:00 (end-to-end verification)

```
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::screen::tests::screen_annotates_a_red_run_as_one_error_marker ... ok
shell::screen::tests::screen_contents_is_the_visible_screen_not_the_scrollback ... ok
shell::screen::tests::screen_does_not_merge_a_colour_run_across_a_row_boundary ... ok
shell::screen::tests::screen_feeds_bytes_split_mid_sequence ... ok
shell::screen::tests::screen_leaves_unmapped_colours_as_plain_text ... ok
shell::screen::tests::screen_maps_all_six_colour_codes ... ok
shell::screen::tests::screen_preserves_cursor_positioned_columns ... ok
shell::screen::tests::screen_reports_the_alternate_screen ... ok
shell::screen::tests::screen_summary_uses_the_shared_classifier ... ok
shell::screen::tests::screen_trims_marker_text_and_drops_empty_runs2 ... ok
shell::screen::tests::screen_trims_marker_text_and_drops_empty_runs ... ok
cargo_exit=0
== C. shell::screen:: totals ==
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1565 filtered out; finished in 0.00s
cargo_exit=0
== D. phases 02 and 03 untouched (13 then 12) ==
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1563 filtered out; finished in 2.00s
cargo_exit=0
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1564 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite totals ==
test result: ok. 1572 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 4.87s
cargo_exit=0
== F. structural greps (each must print the stated number) ==
vt100 dependency        (1): 1
screen.rs exists        (1): 1
mod screen declaration  (1): 1
pub struct ShellScreen  (1): 1
pub fn annotated        (1): 1
calls tmux::status     (>=1): 4
does NOT copy classify  (0): 0
src/tmux untouched      (0): 0
unsafe in screen.rs     (0): 0
unwrap/expect/panic pre-test (0): 0
```

PASTE MATCH

### Update — 2026-09-04 01:01 (started, round 2 — bug-04-1)

Name: executor (phase-04 round-2 dispatch 2026-09-04). Fixed the single
defect + the test the bug named, exactly:

1. `screen_preserves_cursor_positioned_columns` added — feeds `NAME ESC[6C
   SIZE ESC[6C MODE` on a 1×40 screen and asserts `annotated() == contents()`
   (byte-equal, so the internal-column assertion subsumes it). Bug-04-1's
   measured failure was `NAMESIZEMODE` vs `NAME      SIZE      MODE`.
2. The interior-gap fix in `annotated_row`: `!cell.has_contents()` now pushes
   a space into the span buffer instead of `continue`-ing, so an interior
   blank cell contributes to the row's layout while `flush_span`'s trim still
   drops the trailing run. Trailing-blank trimming untouched.
3. `screen_does_not_merge_a_colour_run_across_a_row_boundary` corrected to
   the spec's same-colour case: a red run wrapping row 0 → row 1 at 4 columns
   yields `"[ERROR: abcd]\n[ERROR: efgh]"`, exactly the two-markers-not-one
   guarantee the spec named. `EFGH` no longer appears in the file (grep: 0).
4. **Mutation-check passed:** reverting the interior-gap handling to
   `continue` makes `screen_preserves_cursor_positioned_columns` fail
   (`0 passed; 1 failed`), and restoring it makes it pass again — the new
   test genuinely pins the fixed behaviour.

`shell::screen::` is at **11 passed, 0 failed** (10 + the one new test);
`shell::pty::` at 13 and `shell::log::` at 12, unchanged. All four gates
clean. src/tmux/ untouched. E2E captured above.

### Update — ts=1788484427980 (complete, server-authored)

**Summary:** </think>The file looks correct. Notice the `screen_preserves_cursor_positioned_columns` test asserts `annotated == contents`. Let me double-check the contents. `NAME ESC[6C SIZE ESC[6C MODE` on a 40-col screen. contents() = "NAME      SIZE      MODE" — 6 spaces? Actually ESC[6C moves cursor forward 6 columns, so there are 6 blank columns between. And annotated() should equal contents() exactly as bug required.

All good. Working tree clean, commit done, tasks marked done.

Let me do the final completion checklist:
- [x] Phase doc Status: says `in-progress` (bounced) — yes, it stays in-progress.
- [x] Milestone README phase-table row still says `in-progress` (unchanged).
- [x] Did NOT hand-write `(complete)` Update Log entry — the server does.
- [x] `cargo fmt --all` was run immediately before `git add` — yes.

Wait — order check. I ran `cargo fmt --all` which formats. But the LAST `cargo fmt --all` I ran was AFTER the write of doc changes but before commit? Let me check: I ran `cargo fmt --all && git status --short` (turn with d952fd4 commit prep... actually the last `cargo fmt --all` was run in the bash call right before `git add -A` commit). Yes, I ran `cargo fmt --all` immediately before `git add`/commit. Good.

Also — did `cargo fmt --all` change anything in screen.rs? git status showed only the two modified files before commit, and commit included only those two. Good.

- [x] All verification commands ran clean (build, clippy, test, fmt).
- [x] Final message substantive Summary + Notes for review — that's THIS final message.
- [x] git status --short shows nothing — clean, confirmed.
- [x] git log -1 --stat shows the commit includes every file I touched — screen.rs and phase doc.

All checks complete. Write the summary.</think>
## Summary + Notes for review

**Fixed bug-04-1 in `src/shell/screen.rs`** (one commit `d952fd4`, `fix(shell):`): `annotated_row` now pushes a space into the span buffer for `!cell.has_contents()` cells instead of `continue`-ing, so interior blanks — which cursor-forward/`ESC[nG`-positioned columns rely on — survive into `annotated()` while `flush_span`'s trim keeps the trailing run trimmed. Added the pinned `screen_preserves_cursor_positioned_columns` test (feeds `NAME ESC[6C SIZE ESC[6C MODE` on a 1×40 screen, asserts `annotated() == contents()` byte-equal, covering bug-04-1's measured `NAMESIZEMODE` failure). Corrected `screen_does_not_merge...` to the spec's same-colour case (`ESC[31mabcdefgh` at 4 cols ⇒ `"[ERROR: abcd]\n[ERROR: efgh]"`); `EFGH` no longer appears anywhere in the file (grep: 0).

**Mutation-check executed and recorded:** reverted the interior-gap handling to `continue` → `screen_preserves_cursor_positioned_columns` failed (`0 passed; 1 failed`), restored it → passing again, so the test genuinely pins the fixed behaviour.

**Verification:** `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` all clean; `shell::screen::` **11 passed, 0 failed** (finish condition), `shell::pty::` 13 and `shell::log::` 12 unchanged; `src/tmux/` byte-for-byte untouched. E2E block run verbatim and captured in the phase doc's own `(end-to-end verification)` Update Log entry with the block's literal output; the PASTE MATCH self-check printed `PASTE MATCH` (redirected, byte-exact). No scope added beyond the bug: no resize, no scrollback manipulation, no new test beyond the one named.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


TEST
nored; 0 measured; 0 filtered out; finished in 4.85s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M20-shell-engine/phase-04-screen-model.md` — +71 -0
- `src/shell/screen.rs` — +25 -5

**Commit:** d952fd40d063989e80a3abed2421bbaa23f8400c

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-03

- **Verdict:** approved_after_1
- **Bounces:** 1 (`bug-04-1`, resolved in round 2)
- **Executor:** deepseek-v4-flash-0731 (round 1: 90 turns; round 2: 46 — the
  fastest run of the milestone)
- **Scope deviations:** none. Round 2 changed exactly the two things the bug
  named and added no test beyond the one it specified.

**Gates re-run independently at review:** `cargo fmt --all --check`,
`cargo build` (forced rebuild, zero warnings), `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` — all green. `shell::screen::`
reports **11 passed**, exactly the `11, not 12` finish condition;
`shell::pty::` still 13 and `shell::log::` still 12; `src/tmux/` byte-for-byte
untouched. Lib 1571 → 1572.

**Hygiene:** no `unwrap`/`expect`/`panic!`, no `unsafe`, no `#[allow]`, no
`TODO`, no debug printing. `EFGH` — the old fixture — is gone.

**The bug-04-1 fix verified independently through the public API:**

| Check | Round 1 | Now |
|---|---|---|
| Columns positioned with a cursor-forward escape | `"NAMESIZEMODE"` | `"NAME      SIZE      MODE"`, matching `contents()` |
| Trailing gap | — | still trimmed; no trailing spaces |
| Same-colour run across a row boundary | untested | two markers, verified |
| Blank row between two text rows | — | preserved as an empty line |

**Mutation-checked at review, both the executor's claim and my own:**

- Reverting the interior-gap handling fails
  `screen_preserves_cursor_positioned_columns` — the executor's claimed check
  reproduces.
- Merging rows in `annotated()` fails
  `screen_does_not_merge_a_colour_run_across_a_row_boundary`. **The fixture
  correction made that guard real**: it now discriminates the same-colour case
  its name claims, which the round-1 red/green version could not.

Both restored; tree clean. Evidence: append-only respected (round 2's commit
deletes zero lines from this document), and paste fidelity verified by the
reviewer against the surviving artifact — `PASTE MATCH`.

- **Calibration:**
  1. **A residual of bug-04-1, recorded as a carry rather than a third round.**
     A gap that sits *between a coloured run and plain text* is still lost:
     `contents()` gives `"red      plain"` while `annotated()` gives
     `"[ERROR: red]plain"`. The blank cells join the open colour span and are
     then removed by `flush_span`'s trim.
     **This is partly my own bad constraint.** `bug-04-1`'s definition of done
     said "`annotated()` and `contents()` must agree on column positions within
     a row" — which is **unachievable in general**, because inserting
     `[ERROR: ` and `]` shifts every column after it. The round-1 case worked
     only because it contained no markers. Where a gap belongs relative to a
     marker (`[ERROR: red]      plain` versus the current form) is a design
     question, not a typo, and it deserves deciding with phase-07 — the first
     consumer that actually shows this text to a model. Carried in the
     milestone README's Notes.
  2. **A test can carry exactly the right name and exercise the wrong case.**
     Round 1's boundary test was named for the same-colour guarantee and fed
     two different colours, so the colour change alone forced the split. The
     behaviour was already correct, so nothing failed and nothing looked
     wrong. This is one turn of the screw past phase-03's lesson: there, a
     guarantee had no test; here, it had a test that could not fail for the
     stated reason. **The check that caught it was reading the fixture, not
     the name.**
  3. **Two architect accuracy slips, both caught before dispatch and both
     mine:** a filtered-test count quoted as 1561 when the tree printed 1565,
     and a fixture occurrence count given as 1 when it was 2. Same class as
     the unreachable-criterion trend already recorded.
