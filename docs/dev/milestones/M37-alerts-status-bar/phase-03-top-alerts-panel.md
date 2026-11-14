# Phase 03 — top-alerts-panel

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-14
**Depends on:** phase 01 (`Request::AlertList` → `Response::AlertState`)
**Estimated diff:** ~300 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye top` lists every firing alert the daemon holds (decision 5): a
fifth section in `top --once`, and a fifth panel in the live view, fed by
one read-only `Request::AlertList` on the same 2 s cadence as the shells.
A resolved alert leaves the panel on the next refresh.

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **Move the daemon fetcher** — move `shells_refresh`, the `ShellFetcher`
   trait and `IpcFetcher` out of `src/cli/commands/top_tui.rs` into
   `src/cli/commands/top_load.rs`, next to `shells_panel` and
   `daemon_header`, which they wrap. Then import them back. This is a pure
   move and the first write. `top_tui.rs` is an allowlisted offender whose
   count may never grow, and this phase must add to it, so the move frees
   the room.
2. **The model** — in `src/cli/commands/top.rs`, `AlertsPanel` mirrors
   `ShellsPanel`: `Rows(Vec<crate::ipc::AlertBadge>)` or `DaemonDown`.
   `TopSnapshot` gains `alerts: AlertsPanel`. In `top_load.rs`,
   `alerts_panel()` sends one `Request::AlertList`: a
   `Response::AlertState { alerts }` gives `Rows(alerts)` in the daemon's
   order (worst first), and anything else gives `DaemonDown`. `cargo build`
   names every `TopSnapshot { … }` literal to update.
3. **The refresh** — `ShellFetcher` gains `alerts()`, and the refresh that
   asks for shells and the header also asks for alerts. `run_top`'s
   `--once` path loads them too.
4. **The text** — `alerts_text(panel, now_unix)` in `top_text.rs`, pure:
   - `"none"` for no rows; `"daemon not running"` for `DaemonDown`.
   - Otherwise one line per alert: the severity word as the daemon sent
     it, the name, `firing <age>` since `first_seen_unix`, and
     `seen <age> ago` since `last_seen_unix`, using `format_age`.
   - The name and severity pass through
     `crate::cli::render::escape_controls`, because they come from a
     webhook payload.

   `render_once` prints `Alerts` between `Shells` and `Activity`.
5. **The panel** — `plan_layout` places a bordered Alerts panel, the full
   width, between Shells and Activity. It is sized to its content up to 6
   body lines, and Activity still takes the rest and shrinks first. When
   there are more alerts than lines, the last line reads `+N more` and the
   lines above it show the first alerts in order. On the live screen the
   severity word is coloured: `critical` red, `warning` yellow, everything
   else gray, using the colours the Budget bar already uses. The panel is
   **not focusable**: Tab, `1`–`4` and the scroll keys are unchanged.
6. **Docs** — README § `top --once` (four sections → five, and what an
   Alerts row carries), the README command-table row for `daemoneye top`,
   and CLAUDE.md's `top_tui_draw.rs` row ("four panels").
7. **Capture the evidence** — run the mutation check named in
   § Acceptance criteria and the § End-to-end block verbatim, and paste both
   into one Update Log entry.

## Constraints

- Pin behaviour, not pixels: what a row or panel contains or omits, and
  which colour a cell carries on a `TestBackend`. Never pin a full frame.
- Tests call the production seams (`alerts_text`, `render_once`,
  `plan_layout`, `draw_at`, the refresh through a fake `ShellFetcher`).
  The live loop's tests stay hermetic: no socket, no daemon.
- `top_tui.rs`'s line in `tests/hygiene_allowlist.txt` must not grow.
  Lowering it to the new count is fine. Add no new allowlist entry.
- `top` stays read-only: `AlertList` is the only new request.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/cli/commands/top_tui.rs`: `shells_refresh`, `ShellFetcher`,
  `IpcFetcher`, `TopModel`, `layout_at`, `refresh_heights`, `shells_lines`.
- `src/cli/commands/top_load.rs`: `shells_panel`, `daemon_header`,
  `panel_rows`.
- `src/cli/commands/top.rs`: `ShellsPanel`, `TopSnapshot`, `run_top`.
- `src/cli/commands/top_text.rs`: `shells_text`, `format_age`,
  `render_once`, `plan_layout`, `PanelRects`, `SHELLS_MAX_LINES`.
- `src/cli/commands/top_tui_draw.rs`: `draw_at`, `draw_panel`.
- `src/ipc_types.rs`: `AlertBadge`. `src/ipc.rs`: `Request::AlertList`,
  `Response::AlertState`.
- `src/cli/render.rs`: `escape_controls`.

**Edit order.** Each step ends at a green `cargo build`:
1. **First write:** the fetcher move (Spec 1).
2. `AlertsPanel`, the snapshot field and `alerts_panel()`. Let the
   compiler name every literal.
3. `alerts()` on the fetcher and the refresh; the `--once` load.
4. `alerts_text` and `render_once`.
5. `plan_layout`, the draw and the colours.
6. Tests, then docs, then Spec 7.

**Your first edit lands within your first 15 turns.** Never run `tmux`,
and never start a daemon. Never repeat a read whose output you already
have. When unsure of a call site, let `cargo build` name it. The hygiene
test is the only measure of a file's length.

## Test plan

- `alerts_text_lists_each_alert_worst_first`: three badges in the daemon's
  order give three lines, in that order, each with its severity, name,
  firing age and seen age.
- `alerts_text_says_none_and_daemon_down`.
- `alerts_text_escapes_control_characters`: a name containing `\x1b[31m`
  yields a line with no raw ESC byte.
- `once_frame_has_an_alerts_section_between_shells_and_activity`.
- `plan_layout_places_alerts_between_shells_and_activity`: Alerts sits
  below Shells and above Activity, capped at 6 body lines plus borders,
  and Activity shrinks.
- `alerts_panel_caps_and_counts_the_rest`: on a `TestBackend`, nine alerts
  show the first five and a `+4 more` line.
- `alerts_panel_colours_the_severity`: on a `TestBackend`, a `critical`
  row's severity cell is red and a `warning` row's is yellow.
- `alerts_refresh_asks_the_daemon`: a fake fetcher's alerts land in
  `snapshot.alerts`, and its `DaemonDown` does too.

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `grep -c 'Request::AlertList' src/cli/commands/top_load.rs` prints at
      least 1 (today 0).
- [ ] `grep -c 'trait ShellFetcher' src/cli/commands/top_tui.rs` prints 0
      (today 1).
- [ ] Mutation check, pasted with its `test result:` line: make
      `alerts_text` skip `escape_controls` on the name, confirm
      `alerts_text_escapes_control_characters` fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than 2949 passed. Hygiene, `doc_truth` and
      `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted. The `top`
run uses a throwaway `HOME`, so no daemon answers:

```sh
cargo test --lib alerts 2>&1 | grep -E '^test result'
grep -c 'Request::AlertList' src/cli/commands/top_load.rs | sed 's/^/alert_list=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
h=$(mktemp -d) && HOME=$h target/debug/daemoneye top --once | sed -n '/^Alerts$/,/^$/p'; rm -rf "$h"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

The `top` line must print `Alerts` then `daemon not running`.

## Authorizations

- Edit the files in the symbol map, `README.md`, `CLAUDE.md`, and every
  test module that `cargo build` names for the new `TopSnapshot` field.
- Lower `top_tui.rs`'s count in `tests/hygiene_allowlist.txt`.

## Out of scope

- Making the Alerts panel focusable, scrollable or selectable, or any key
  that acts on an alert.
- The status bar, the daemon's alert registry and the webhook gate.

## Update Log

<!-- entries appended below this line -->
