# Phase 01: Color-depth detection and a central palette

**Milestone:** M13 — Chat UX Polish
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye chat` (and `daemoneye status`) currently hardcode truecolor
`38;2;r;g;b` everywhere with zero terminal-capability detection, so on any
terminal that can't pass RGB through (tmux over a non-truecolor outer terminal —
the pinky.home.planetfoo.org symptom) every color quantizes into monotone red.
This phase adds one color-depth detection, one palette module, and makes every
color site in the chat/status CLI ask the palette instead of hardcoding.

## Architecture references

Read before starting:

- `docs/architecture.md#11-transport--process-layer` — the CLI client is where
  color rendering lives; the daemon never emits color.
- `docs/dev/milestones/M13-chat-ux/README.md` § "Derived code facts" — the
  milestone-wide inventory this phase's facts come from.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- There is **no** capability detection anywhere in `src/` — zero hits for
  `COLORTERM`, `terminfo`, `truecolor`, `DAEMONEYE_COLOR`.
- Truecolor sites (complete list, verified by grep 2026-08-09):
  - `src/cli/commands/chat.rs:755` and `:757` — banner `Color::Rgb(180, 0, 0)`
    / `Color::Rgb(220, 160, 0)` in `banner_lines()` (function at `:725`,
    called once at `:329`).
  - `src/cli/render_ratatui.rs:270` / `:272` — same two RGB values in
    `draw_spinner`.
  - `src/cli/render_ratatui.rs:354-355` — same two values in `commit_panel`.
  - `src/cli/render_ratatui.rs:1171` / `:1192` — **test** assertions
    (`commit_panel_uses_blood_red_border_and_yellow_title`); these stay.
  - `src/cli/status.rs:13-36` — eight helper fns (`c_accent`, `c_key`,
    `c_val`, `c_ok`, `c_err`, `c_warn`, `c_num`, `c_dim`), each formatting a
    hardcoded `\x1b[38;2;R;G;Bm` string.
- `apply_sgr` (`src/cli/render_ratatui.rs:59-93`) parses `38;5;<idx>` but
  silently drops `38;2;r;g;b` — a legacy truecolor string routed through
  `parse_ansi_to_spans` loses its color.
- `RatatuiRenderer` struct (`render_ratatui.rs:152-155`) has two fields:
  `terminal`, `start_time`. It is constructed in production by
  `RatatuiRendererStdout::new` (`:169`) and by **struct literal** in three
  test sites: `make_test_renderer` (`:668`), and inline at `:1420` and
  `:1471`. Adding a field means updating exactly those three literals plus
  `new()`.
- `src/cli/mod.rs` declares modules at lines 3-11 (`commands`, `input`,
  `local_cmds`, `markdown`, `notify`, `render`, `render_ratatui`, `status`).

## Spec

### Task 1 — Create `src/cli/palette.rs`

New module with three pieces. Shapes below are prescriptive; exact code is
yours except where a literal is pinned.

**(a) `ColorDepth` + pure detection.** The detection function takes env values
as *parameters* — it must never read the environment itself, so tests stay
hermetic (this crate is edition 2024: `std::env::set_var` is `unsafe` and
env-reading tests would need the global test-home lock; parameters avoid all
of that).

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    Truecolor,
    Xterm256,
    Ansi16,
}

/// Decide the color depth from environment values, in precedence order.
pub fn detect_color_depth(
    override_var: Option<&str>, // $DAEMONEYE_COLOR
    colorterm: Option<&str>,    // $COLORTERM
    term: Option<&str>,         // $TERM
) -> ColorDepth {
    // 1. Explicit override wins: "truecolor"|"24bit" → Truecolor,
    //    "256" → Xterm256, "16"|"basic" → Ansi16. Unrecognized → fall through.
    // 2. COLORTERM containing "truecolor" or "24bit" (case-insensitive) → Truecolor.
    // 3. TERM containing "direct" → Truecolor (xterm-direct family).
    // 4. TERM containing "256color" → Xterm256.
    // 5. Otherwise → Ansi16.
}

/// Read the real environment. The only place the env is consulted.
pub fn detect_from_env() -> ColorDepth {
    let ov = std::env::var("DAEMONEYE_COLOR").ok();
    let ct = std::env::var("COLORTERM").ok();
    let t = std::env::var("TERM").ok();
    detect_color_depth(ov.as_deref(), ct.as_deref(), t.as_deref())
}
```

Must-NOT-match cases (these are the bug): `COLORTERM` unset +
`TERM=tmux-256color` is **Xterm256, not Truecolor** — that is the tmux/pinky
configuration this phase exists for. `COLORTERM=yes` is **not** Truecolor.
All three unset is **Ansi16**.

**(b) `Palette`** — the ratatui-color side used by the chat renderer:

```rust
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub depth: ColorDepth,
}

impl Palette {
    pub fn for_depth(depth: ColorDepth) -> Self { Self { depth } }
    pub fn from_env() -> Self { Self::for_depth(detect_from_env()) }

    /// The DaemonEye blood-red (borders, logo body, spinner frame).
    pub fn red(&self) -> ratatui::style::Color {
        match self.depth {
            ColorDepth::Truecolor => Color::Rgb(180, 0, 0),
            ColorDepth::Xterm256 => Color::Indexed(124),
            ColorDepth::Ansi16 => Color::Red,
        }
    }

    /// The DaemonEye deep-yellow (eye highlight, titles, spinner glyph).
    pub fn yellow(&self) -> ratatui::style::Color {
        match self.depth {
            ColorDepth::Truecolor => Color::Rgb(220, 160, 0),
            ColorDepth::Xterm256 => Color::Indexed(178),
            ColorDepth::Ansi16 => Color::Yellow,
        }
    }
}
```

The 256 indices are pinned: **124** (`af0000`, nearest to 180,0,0) and **178**
(`d7af00`, nearest to 220,160,0). Do **not** use the legacy `88`/`136` pair
from `render.rs` — those are a darker, superseded shade.

**(c) `sgr_fg`** — the raw-escape side used by `status.rs`:

```rust
/// Foreground-color escape for the given depth. `basic` is a full SGR code
/// (e.g. 96 = bright cyan), not a color index.
pub fn sgr_fg(depth: ColorDepth, rgb: (u8, u8, u8), idx256: u8, basic: u8) -> String {
    match depth {
        ColorDepth::Truecolor => format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2),
        ColorDepth::Xterm256 => format!("\x1b[38;5;{idx256}m"),
        ColorDepth::Ansi16 => format!("\x1b[{basic}m"),
    }
}
```

### Task 2 — Register the module

In `src/cli/mod.rs`, add `pub mod palette;` alongside the existing
declarations (lines 3-11).

### Task 3 — Thread `Palette` through `RatatuiRenderer`

In `src/cli/render_ratatui.rs`:

1. Add a field to the struct (`:152-155`): `palette: crate::cli::palette::Palette`.
2. In `RatatuiRendererStdout::new` (`:169`), set
   `palette: Palette::from_env()`.
3. Update the three test struct-literals — `make_test_renderer` (`:668`) and
   the inline constructions at `:1420` and `:1471` — adding
   `palette: Palette::for_depth(ColorDepth::Truecolor)`. Truecolor keeps every
   existing color assertion (`Rgb(180,0,0)` cells etc.) passing unchanged.
4. In `draw_spinner` (`:269-272`), replace the two hardcoded styles:

   ```rust
   let blood_red = Style::default()
       .fg(self.palette.red())
       .add_modifier(Modifier::BOLD);
   let bright_yellow = Style::default().fg(self.palette.yellow());
   ```

5. In `commit_panel` (`:354-355`), replace
   `let border_color = Color::Rgb(180, 0, 0);` → `self.palette.red()` and
   `let title_color = Color::Rgb(220, 160, 0);` → `self.palette.yellow()`.

After this task, no `Color::Rgb` remains in this file outside `mod tests`.

### Task 4 — Teach `apply_sgr` the `38;2` form

In `apply_sgr` (`render_ratatui.rs:59-93`), the existing 256-color arm is:

```rust
"38" if i + 2 < parts.len() && parts[i + 1] == "5" => {
    if let Ok(idx) = parts[i + 2].parse::<u8>() {
        style = style.fg(color_from_256(idx));
        i += 2;
    }
}
```

Add a sibling arm for truecolor, same shape:

```rust
"38" if i + 4 < parts.len() && parts[i + 1] == "2" => {
    if let (Ok(r), Ok(g), Ok(b)) = (
        parts[i + 2].parse::<u8>(),
        parts[i + 3].parse::<u8>(),
        parts[i + 4].parse::<u8>(),
    ) {
        style = style.fg(Color::Rgb(r, g, b));
        i += 4;
    }
}
```

(Note the guard: `parts` for `38;2;r;g;b` has length 5, so `i + 4 <
parts.len()` is the correct bound, mirroring the existing arm's `i + 2` for
its length-3 case. This `Color::Rgb` is a *parser output*, not a palette
choice — it is exempt from the "no `Color::Rgb` in production" criterion
below, which is scoped to files other than this arm's; see the criterion's
exact check.)

### Task 5 — Banner uses the palette

In `src/cli/commands/chat.rs`:

1. `banner_lines` (`:725`) gains a parameter:
   `fn banner_lines(chat_width: usize, palette: &crate::cli::palette::Palette)`.
   Replace the two hardcoded styles (`:754-757`) with `palette.red()` /
   `palette.yellow()`. `Color::White` and the DIM subtitle stay as they are —
   named colors render at every depth.
2. Update the single call site (`:329`):
   `renderer.commit_styled(&banner_lines(chat_width, &Palette::from_env()))`
   (adjust imports as needed).

### Task 6 — `status.rs` helpers go depth-aware

In `src/cli/status.rs`, add one lazy depth lookup at module level:

```rust
use crate::cli::palette::{self, ColorDepth};

fn depth() -> ColorDepth {
    static DEPTH: std::sync::OnceLock<ColorDepth> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(palette::detect_from_env)
}
```

Then rewrite the eight helpers (`:13-36`) to delegate. Worked example —
`c_accent` today:

```rust
fn c_accent(s: &str) -> String {
    format!("\x1b[1m\x1b[38;2;100;210;255m{s}\x1b[0m")
}
```

becomes:

```rust
fn c_accent(s: &str) -> String {
    format!("\x1b[1m{}{s}\x1b[0m", palette::sgr_fg(depth(), (100, 210, 255), 117, 96))
}
```

Do the same for the rest with this pinned mapping (rgb → 256 index → basic
SGR code). `c_accent` and `c_val` keep their `\x1b[1m` bold prefix; the
others have none today and gain none:

| helper | rgb | idx256 | basic |
|---|---|---|---|
| `c_accent` | (100, 210, 255) | 117 | 96 |
| `c_key` | (140, 140, 165) | 103 | 90 |
| `c_val` | (220, 220, 240) | 254 | 97 |
| `c_ok` | (80, 210, 130) | 78 | 92 |
| `c_err` | (230, 80, 80) | 167 | 91 |
| `c_warn` | (250, 190, 50) | 214 | 93 |
| `c_num` | (130, 195, 255) | 111 | 94 |
| `c_dim` | (80, 80, 105) | 60 | 90 |

After this task `status.rs` contains no `38;2` literal.

### Task 7 — Tests

Write the tests named in § Test plan. The detection and palette tests live in
`src/cli/palette.rs`'s own `#[cfg(test)] mod tests`; the `apply_sgr` and
`commit_panel` tests join the existing `mod tests` in `render_ratatui.rs`
(follow the shape of `parse_ansi_to_spans_handles_color` at `:869` and
`commit_panel_uses_blood_red_border_and_yellow_title` at `:1135`).

### Task 8 — Mutation: apply

Using the `patch` tool on `src/cli/palette.rs`, change `Indexed(124)` to
`Indexed(88)` (old_str `Indexed(124)`, new_str `Indexed(88)`). Then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-01.txt
grep -c 'Indexed(88)' src/cli/palette.rs >> /tmp/e2e-m13-01.txt
cargo test --lib cli::palette 2>&1 | tail -5 >> /tmp/e2e-m13-01.txt
```

The test run must show `palette_256_red_is_indexed_124` **FAILED**. If every
test stays green under this mutation, report a blocker — do not adjust a test
to make it fail.

### Task 9 — Mutation: restore

Using the `patch` tool, change `Indexed(88)` back to `Indexed(124)`. Then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-01.txt
grep -c 'Indexed(88)' src/cli/palette.rs >> /tmp/e2e-m13-01.txt
cargo test --lib cli::palette 2>&1 | tail -5 >> /tmp/e2e-m13-01.txt
```

The grep count must be `0` and the tests all green.

### Task 10 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m13-01.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `src/cli/palette.rs` exists and `cargo test --lib cli::palette` passes
      with the § Test plan names present. (Currently: no such file.)
- [ ] `grep -c '38;2' src/cli/status.rs` prints `0`. (Currently: 8.)
- [ ] `grep -c 'Color::Rgb' src/cli/commands/chat.rs` prints `0`.
      (Currently: 2.)
- [ ] In `src/cli/render_ratatui.rs`, the first `Color::Rgb` hit is *inside*
      the code region at or after the first `#[cfg(test)]` line **or** inside
      the `apply_sgr` truecolor arm added by Task 4 — checked mechanically as:
      every `grep -n 'Color::Rgb' src/cli/render_ratatui.rs` hit line is
      either within `apply_sgr` (`:59-110` region) or greater than the
      `#[cfg(test)]` line. (Currently: hits at 270, 272, 354, 355 are in
      neither.)
- [ ] Test `detect_tmux_256color_is_not_truecolor` passes. (Currently: does
      not exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] `commit_panel_uses_blood_red_border_and_yellow_title` still passes
      (truecolor test palette preserves the RGB cell assertions).
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

In `src/cli/palette.rs` `mod tests`:

- `detect_override_wins` — `detect_color_depth(Some("256"), Some("truecolor"), Some("xterm-direct"))` → `Xterm256`.
- `detect_colorterm_truecolor` — `(None, Some("truecolor"), Some("xterm-256color"))` → `Truecolor`.
- `detect_tmux_256color_is_not_truecolor` — `(None, None, Some("tmux-256color"))` → `Xterm256`. **The pinky case.**
- `detect_term_direct_is_truecolor` — `(None, None, Some("xterm-direct"))` → `Truecolor`.
- `detect_colorterm_yes_is_not_truecolor` — `(None, Some("yes"), Some("xterm"))` → `Ansi16`.
- `detect_all_unset_is_ansi16` — `(None, None, None)` → `Ansi16`.
- `palette_256_red_is_indexed_124` — `Palette::for_depth(Xterm256).red()` == `Color::Indexed(124)` and `.yellow()` == `Color::Indexed(178)`. (Mutation target of Tasks 8/9.)
- `palette_truecolor_matches_legacy_rgb` — truecolor palette returns `Rgb(180,0,0)` / `Rgb(220,160,0)`.
- `sgr_fg_emits_escape_per_depth` — all three depths for one tuple: `38;2;100;210;255`, `38;5;117`, `\x1b[96m`.

In `render_ratatui.rs` `mod tests`:

- `apply_sgr_parses_truecolor_foreground` — `parse_ansi_to_spans("\x1b[38;2;220;160;0mX")` yields a span with fg `Color::Rgb(220, 160, 0)`.
- `commit_panel_borders_follow_palette_depth` — a renderer built with `Palette::for_depth(Xterm256)` commits a panel whose border cells carry `Color::Indexed(124)` and title cells `Color::Indexed(178)` (mirror the cell-assertion shape of the existing test at `:1135`).

## End-to-end verification

```sh
: > /tmp/e2e-m13-01.txt
echo "== GATES ==" >> /tmp/e2e-m13-01.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-01.txt; echo "fmt exit=$?" >> /tmp/e2e-m13-01.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-01.txt; echo "build exit=$?" >> /tmp/e2e-m13-01.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-01.txt; echo "clippy exit=$?" >> /tmp/e2e-m13-01.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-01.txt; echo "test exit=$?" >> /tmp/e2e-m13-01.txt
echo "== NO STRAY TRUECOLOR ==" >> /tmp/e2e-m13-01.txt
echo "status.rs 38;2 count: $(grep -c '38;2' src/cli/status.rs)" >> /tmp/e2e-m13-01.txt
echo "chat.rs Rgb count: $(grep -c 'Color::Rgb' src/cli/commands/chat.rs)" >> /tmp/e2e-m13-01.txt
grep -n 'Color::Rgb' src/cli/render_ratatui.rs >> /tmp/e2e-m13-01.txt
grep -n '#\[cfg(test)\]' src/cli/render_ratatui.rs | head -1 >> /tmp/e2e-m13-01.txt
wc -l /tmp/e2e-m13-01.txt >> /tmp/e2e-m13-01.txt
```

(The two mutation runs of Tasks 8/9 append into the same file between the
gates and this block's tail — the artifact accumulates in task order.)

The remote/live confirmation on pinky (tmux termfeatures, visual check over
ssh) is an **architect/human review step** at milestone close, per the
milestone README's § Risks — it is not part of this phase's executor block.

## Authorizations

None.

## Out of scope

- The throbber's two-space indent, the `you` label, runtime-in-border, cursor
  math, resize handling — phases 02-05.
- Deleting the dead legacy printers in `render.rs` — phase 05.
- The 256-color codes in `src/cli/markdown/syntax.rs` and `render.rs` — they
  are already `38;5` (render everywhere); converting them to palette roles is
  not needed for the bug and stays out.
- `NO_COLOR` support, config-file color settings, any daemon-side change.
- Do not modify `src/cli/render.rs`, `src/cli/markdown/`, or anything under
  `src/daemon/`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-09 22:58 (started)

**Executor:** model

Created `src/cli/palette.rs` with `ColorDepth`, `Palette`, `sgr_fg`, and detection logic. Registered the module. Threaded `Palette` through `RatatuiRenderer`. Taught `apply_sgr` the `38;2` truecolor form. Banner and `status.rs` helpers now use the palette. Added all tests from the test plan.

### Update — 2026-08-09 22:58 (end-to-end verification)

```
== M1 APPLIED ==
1
    cli::palette::tests::palette_256_red_is_indexed_124

test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 1202 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
== M1 RESTORED ==
0
test cli::palette::tests::detect_tmux_256color_is_not_truecolor ... ok
test cli::palette::tests::sgr_fg_emits_escape_per_depth ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 1202 filtered out; finished in 0.00s

== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.19s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.35s
clippy exit=0
test result: ok. 1211 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.08s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
== NO STRAY TRUECOLOR ==
status.rs 38;2 count: 0
chat.rs Rgb count: 0
94:                    style = style.fg(Color::Rgb(r, g, b));
896:        assert_eq!(spans[0].style.fg, Some(Color::Rgb(220, 160, 0)));
1194:        let border_color = Color::Rgb(180, 0, 0);
1215:        let title_color = Color::Rgb(220, 160, 0);
666:#[cfg(test)]
38 /tmp/e2e-m13-01.txt
```
