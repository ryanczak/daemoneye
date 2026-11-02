# Phase 02 — status-bar

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
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
