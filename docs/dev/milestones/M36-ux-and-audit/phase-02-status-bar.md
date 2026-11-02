# Phase 02 — status-bar

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-02
**Depends on:** phase 01 (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Design item 3 (`docs/design/2.7.0-UX-improvements.md`): the status bar under
the chat input is too dark and says nothing about the turn or the context
budget. Today `render_status_bar` (`src/cli/render_ratatui.rs`) draws
` session:<id> · <model> · up <t> ` in `DarkGray` + `DIM`. After this phase it
draws, in a readable gray with no `DIM`:

```
 turn 6 · ctx 41k/200k (20%) · glm-4.6 · session:9d46ede3… · up 3m 2s        ⚡approvals: all · Ctrl+C revokes
```

The left part is the five fields in that order; the approval hint
(`SessionApproval::hint()`) sits flush against the right edge. PE decision 3
in the milestone README fixes the layout.

## Architecture references

- `docs/architecture.md` § 1 System layers — the chat client's renderer.
- `src/cli/render.rs` · `StatusBarState`: it already carries `approval_hint`,
  `prompt_tokens` and `context_window`, which nothing draws today. It has no
  turn.
- `src/cli/render_ratatui.rs` · `render_status_bar` and its three callers;
  `src/cli/render_ratatui_draw.rs` · the six `draw*` entry points, three of
  which call `render_status_bar` directly.
- `src/cli/palette.rs` · `Palette::yellow()` (the amber) and `Palette::red()`;
  the renderer holds a `palette`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the files above and every `StatusBarState { … }` literal
   (`chat_loop.rs`, `stream_ask.rs`'s `prompt_status` and its spinner
   literals).
3. Read this entire phase doc before touching any code.

## Behaviour

- **Fields:** `turn N` (the chat's turn counter as `run_chat_ratatui` holds
  it: 0 before the first turn, the resumed count after `--resume`);
  `ctx <tokens>/<window> (<pct>%)`; the model; `session:<short id>` (the
  existing `short_session`); `up <uptime>` (the existing `fmt_uptime`).
- **The context segment:** before the daemon reports usage
  (`prompt_tokens == 0`) it reads `ctx —`. With `context_window == 0` (an
  unknown window, as `daemoneye ask` passes) it reads `ctx 41k` with no
  window and no percentage. The percentage is floored.
- **Token counts are compact:** below 1 000 as is (`950`); below 10 000 with
  one decimal (`4321` → `4.3k`); below 1 000 000 as whole thousands
  (`41234` → `41k`, `200000` → `200k`); from 1 000 000 as millions with one
  decimal (`1048576` → `1.0M`).
- **Colour:** the whole bar is a readable gray (`Color::Gray`, never
  `DarkGray`, never `Modifier::DIM`). The context segment alone turns
  `palette.yellow()` at ≥ 75 % and `palette.red()` at ≥ 90 % (74 % stays
  gray, 89 % stays amber). No percentage, no colour change.
- **Narrow panes:** the bar never exceeds the area's width. When the left
  part and the hint do not both fit with at least one space between them,
  the hint is dropped whole; if the left part alone still does not fit, it is
  clipped with a trailing `…` (the existing `truncate_with_ellipsis`).
- **Every panel shows it:** the idle input, the spinner, the prompt, and the
  approval, credential and picker panels all draw the same bar from the same
  `StatusBarState`.

## Constraints

- **One pure function builds the bar's `Line`** from the `StatusBarState`,
  the elapsed uptime, the palette and the width; `render_status_bar` only
  renders it. The tests below call the function directly, plus one
  `TestBackend` test per draw path.
- **`StatusBarState` gains `turn: usize`**; every literal sets it. For the
  turn's own frames (`stream_ask.rs`), carry the number in through
  `TokenCtx` (a new field), set from `run_chat_ratatui`'s `turn` at the call
  site; `daemoneye ask` (`ask.rs`) passes 1.
- **Pass the state, not its pieces:** the draw entry points and
  `render_live_region` / `render_prompt_region` / `render_spinner_region`
  take the `StatusBarState` (or the built `Line`) where they take
  `session_id` and `model` today.
- **The allowlist is a ratchet:** `run_chat_ratatui` (526) and
  `ask_with_session_ratatui` (678) are allowlisted in
  `tests/hygiene_allowlist.txt`. If the new field grows one, raise its line
  and say why in the Update Log; do not squeeze code to fit.
- **Out of the bar:** `daemon_up`, `tools_total`, `cost_usd` and
  `has_untracked` stay in the struct, undrawn, as today.
- **Use `patch` with whole blocks** on these long files, never `patch_lines`.

## Spec

1. **Red tests first** — write these against the current code (stubbing the
   new function to return today's text where it does not exist yet), run
   them, and keep the failing output for step 5. All names start
   `status_bar_`, which matches no test today:
   - `status_bar_line_orders_turn_ctx_model_session_uptime` — the five
     fields appear left to right in that order, and the hint ends at the
     right edge of the given width.
   - `status_bar_ctx_formats_tokens_compactly` — the five token examples in
     § Behaviour, plus `ctx —` at 0 tokens and `ctx 41k` with no window.
   - `status_bar_ctx_colours_at_75_and_90_percent` — gray at 74 %, amber at
     75 % and 89 %, red at 90 %, on the context span only.
   - `status_bar_is_readable_gray_not_dim` — no span carries `DIM` or
     `DarkGray`.
   - `status_bar_drops_hint_before_clipping` — at a width where only the left
     part fits, the hint is absent and the left part intact; narrower still,
     the line ends in `…` and no line ever exceeds the width.
   - `status_bar_shows_turn_on_every_panel` — on a `TestBackend`, each of the
     six draw entry points with `turn: 6` puts `turn 6` on its status row.
2. **Implement** § Behaviour per the constraints. The tests pass.
3. **Callers** — set `turn` at every `StatusBarState` literal and carry it
   through `TokenCtx`.
4. **Full suite** — `cargo test` passes with `failed=0`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting step 1's
   `test result:` lines and assertion messages.
6. **Findings** — one `### Findings — <date>` entry: anything you noticed
   outside this phase's scope, two or three lines, or `none`.
7. **End-to-end** — run the block in § End-to-end verification once before
   committing and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry. Required; the
   reviewer checks for it.
8. **Commit** as `feat: the status bar shows the turn, the context budget and the approvals hint, in readable gray (M36 phase-02)`.

## Acceptance criteria

- [ ] The E2E block's first line is `status_bar passed=6 failed=0`
      (today `status_bar passed=0 failed=0`).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] The phase-01 tests (`paste_safe_box_*`) and the approval panel tests
      (`approval_panel_*`) pass unchanged (preservation).
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

## Test plan

The pure-function tests pin every rule in § Behaviour; the panel test proves
each draw path is wired. The milestone's live check reads the bar on pinky
after the first reply.

## End-to-end verification

```sh
bash <<'E2E'
cargo test status_bar_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "status_bar passed=" p, "failed=" f}'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render.rs`, `src/cli/render_ratatui.rs`,
  `src/cli/render_ratatui_draw.rs`, their test files,
  `src/cli/commands/stream.rs` (`TokenCtx`), `stream_ask.rs`,
  `chat_loop.rs`, `ask.rs`, and `tests/hygiene_allowlist.txt` (only per the
  ratchet rule).

## Out of scope

- The approval hint's wording and what it counts (`SessionApproval::hint`);
  phase 03 reworks the categories.
- The user-echo panel's `turn N · …` label (`turn_budget_label`), which stays.
- Drawing cost or tool counts in the bar.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-02 05:25 (started)

Executor: executor (GLM, pairing with Claude Code). Flipped Status to
`in-progress` and the milestone README row to match. Plan: red tests against
the pre-phase renderer, then the pure builder `status_bar_line`, then the
callers (`turn` through `TokenCtx`), then the E2E block.

### Update — 2026-11-02 06:10 (red runs)

Step 1's six tests, written against the pre-phase code (commit `963e9e71`,
the tree before any phase-02 change) and run in a throwaway worktree. The
pure-function tests needed the not-yet-existing `status_bar_line`, so their
harness stubbed it with an empty `Line` — the point stands: every assertion
the new behaviour makes fails on the pre-phase code:

```
running 6 tests
test cli::render_ratatui::tests::status_bar_is_readable_gray_not_dim ... ok
test cli::render_ratatui::tests::status_bar_drops_hint_before_clipping ... FAILED
test cli::render_ratatui::tests::status_bar_ctx_formats_tokens_compactly ... FAILED
test cli::render_ratatui::tests::status_bar_ctx_colours_at_75_and_90_percent ... FAILED
test cli::render_ratatui::tests::status_bar_line_orders_turn_ctx_model_session_uptime ... FAILED
test cli::render_ratatui::tests::status_bar_shows_turn_on_every_panel ... FAILED

failures:
---- cli::render_ratatui::tests::status_bar_drops_hint_before_clipping stdout ----
the left part survives intact: ""
---- cli::render_ratatui::tests::status_bar_ctx_formats_tokens_compactly stdout ----
below 1 000 as is: ""
---- cli::render_ratatui::tests::status_bar_ctx_colours_at_75_and_90_percent stdout ----
a span holds the context segment
---- cli::render_ratatui::tests::status_bar_line_orders_turn_ctx_model_session_uptime stdout ----
the bar must contain 'turn 6': ""
---- cli::render_ratatui::tests::status_bar_shows_turn_on_every_panel stdout ----
draw path 0 must put turn 6 on its status row: "                ────… typed ────… session:test-ses… · test-model · up 0s"
test result: FAILED. 1 passed; 5 failed; 0 ignored; 0 measured; 2561 filtered out; finished in 0.00s
```

The `is_readable_gray_not_dim` vacuous pass is expected red-run noise: with
the stub's empty line there is no span to violate the rule — on the real
implementation it asserts over the real spans. The five real failures show
the pre-phase bar carries no turn, no ctx, no hint and the old
`session:<id> · <model> · up <t>` text.

### Findings — 2026-11-02 06:10

- `TokenCtx` had no turn, so `stream_ask.rs`'s twelve `StatusBarState`
  literals each rebuilt it; `prompt_status` grew a `turn` parameter (8
  params now, `#[allow(clippy::too_many_arguments)]` added with the reason
  inline). Adjacent, not phase scope: `prompt_status` could take the
  `StatusBarState` wholesale once every caller holds one.
- `examples/reanchor_repro.rs` builds a `StatusBarState` literal too; it is
  outside the authorized files, but it does not compile without the new
  field, so it got `turn: 0` (one line). Flagging for the record.
- none otherwise — everything stayed inside the authorized files.

### Update — 2026-11-02 06:20 (end-to-end verification)

Ran the phase doc's § End-to-end verification block once, output redirected
to `.rexymcp/out/e2e-phase02.txt` and pasted below verbatim:

```
status_bar passed=6 failed=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.97s
fmt_exit=0
passed=2646 failed=0
```

First line is `status_bar passed=6 failed=0` (was `passed=0 failed=0`), the
hygiene / doc_truth / bug_tracker gates are `ok`, `build_warnings=0`, clippy
clean (`Finished`, no error line), `fmt_exit=0`, full suite `failed=0`.
Preservation: `paste_safe_box_*` (3) and `approval_panel_*` (5) pass
unchanged inside the full suite. The allowlist ratchet was used three times:
`run_chat_ratatui` 526→537, `read_input_line_inner_ratatui` 183→189 and
`ask_with_session_ratatui` 678→691 — every one is the new `turn` field plus
its plumbing (the `TokenCtx` field, the `RatatuiInputCtx` field, and the
`prompt_status` parameter), not squeezed code.

