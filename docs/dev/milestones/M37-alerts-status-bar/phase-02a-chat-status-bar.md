# Phase 02a — chat-status-bar

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-12
**Depends on:** phase 01 (the `AlertState` frame)
**Estimated diff:** ~550 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=l

## Goal

The chat status bar becomes the glyph-led line of the milestone README's
decision 3: one line, a glyph and a number per segment, zero-valued items
hidden, three glyph dialects behind `[chat] glyphs`, tiers that drop whole
segments on a narrow pane, and a firing alert leading the line. This phase
is client-side. The background, ghost and shell counters are wired to zero
until phase 02b feeds them, and hidden because they are zero.

## Behaviour

1. **One status model.** A new module `src/cli/status_bar.rs` owns a
   `StatusModel` (the fields `StatusBarState` carries today, plus: the
   chat state, the alert snapshot, the counters, the saved session name and
   whether it is dirty, the previous turn's prompt tokens, the last turn's
   output tokens and duration, the turn start instant, and the session
   start instant). The chat loop owns one `StatusModel` and updates it in
   place; every place that builds a `StatusBarState` today calls a method
   on the model instead. `StatusBarState` goes away, or becomes the pure
   view the model produces: one struct built in one place.
2. **The state slot is the emitter's state.** The OSC 7501 `Emitter`
   (`src/cli/program_status_emit.rs`) already learns `working`, `blocked`
   and `idle`. The bar reads the same transitions: a `ChatState` enum
   (`Idle`, `Working`, `AwaitingApproval`, `DaemonDown`, `Reconnecting`)
   held in the `StatusModel` and set at the points the emitter is driven,
   not inferred a second time. `Reconnecting` comes from the presence
   task's "daemon connection lost" push; `DaemonDown` from `daemon_up`.
3. **Glyph dialects.** A `GlyphSet` with the slot table of decision 3, one
   constant per dialect (`EMOJI`, `SYMBOLS`, `ASCII`). The chosen set comes
   from `[chat] glyphs` (a new `ChatConfig` in `src/config/types.rs`,
   default `"emoji"`; any other value than the three is a load error naming
   them; `chat` is runtime-swappable in `src/config/handle.rs`). When
   `LANG`, `LC_ALL` and `LC_CTYPE` all lack `UTF-8`/`utf8` (case-insensitive),
   the set is `ASCII` whatever the config says. No glyph string contains
   U+FE0F or U+FE0E.
4. **The line.** `status_bar_line(model, now, palette, width)` is pure and
   builds the spans by tier, as decision 3 states. Segments:
   - **state:** the state glyph; while `Working`, `⏱` and the seconds since
     the turn started.
   - **alerts:** hidden when none are firing. Otherwise the glyph of the
     worst severity and the count of all firing; when room, the name and
     age (`3m`, `2h`) of the newest alert of that severity. For 10 s after
     a resolve removes the last alert of a fingerprint, the slot shows the
     resolved glyph and that name instead.
   - **context:** the fill glyph for `prompt_tokens / context_window`
     (five steps: < 20 %, < 45 %, < 70 %, < 90 %, ≥ 90 %), then the
     percentage; amber ≥ 75 %, red ≥ 90 %. When room, the growth glyph and
     the tokens added since the previous turn's prompt, amber when that is
     ≥ 10 % of the window. With no usage yet, the slot shows the empty
     glyph and `—`.
   - **cost:** the cost glyph and `cost_usd` to two decimals; the unknown
     glyph after it when `has_untracked`; when room, the burn glyph and
     `cost_usd / session hours` as `1.9/h`.
   - **turn:** the turn glyph and the count; when room and a turn has
     finished, the throughput glyph (fast at ≥ 30 t/s, slow below) and
     `38t/s` from the last turn's output tokens over its duration.
   - **background, ghosts, tools, shells:** glyph and count, each hidden
     at zero.
   - **model:** the vendor mark (Anthropic, OpenAI, Gemini, otherwise the
     local mark; `SYMBOLS` has no local mark, `ASCII` no mark) and the
     short name: the vendor prefix (`claude-`, `gpt-`, `gemini-`, a
     `vendor/` path) and a trailing `-YYYYMMDD` removed, then clipped to
     12 columns.
   - **session:** hidden when unnamed; the session glyph and the saved
     name, with the dirty mark when dirty.
   - **grants:** the existing approval hint, flush right.
   Separators are ` · ` at ≥ 36 columns and a single space below.
5. **Fitting.** The line is built at full detail, measured with
   `unicode-width`, and if it does not fit the pane the tiers drop in the
   order decision 3 gives, one item at a time, re-measuring each time. The
   grant hint drops after tier 2 and before tier 1. Nothing is clipped
   mid-segment except the model name's own 12-column cap.
6. **Alert state reaches the loop.** The parked presence task forwards
   `Response::AlertState` as well as `SystemMsg`. The channel it forwards
   on carries a small enum (`PresencePush::Text(String)` /
   `PresencePush::Alerts(Vec<AlertBadge>)`) in place of the bare `String`;
   the loop's arm stores the alerts in the model and redraws, and the
   text arm is as today.
7. **Ticking.** The input loop redraws the bar once a second while
   something on it is time-based: the state is `Working`, an alert is
   shown, or a resolve flash is live. Otherwise no timer runs.
8. **Dirty and saved.** The model learns the saved name from
   `SessionSaved` and `SessionLoaded` replies, the `/session rename` path
   in `slash_session.rs`, and from the
   resume banner's name, and marks dirty on each turn start, clean on a
   save.
9. **`doctor` prints a glyph line** for the configured set, so the user
   sees what the terminal does with it: one `info` row, never a FAIL.
10. **Docs.** `[chat]` with its `glyphs` key in `assets/etc/config.toml`
    and `assets/memory/knowledge/config-reference.md`; the README's status
    bar paragraph and its slot table; CLAUDE.md's `src/cli/` row names
    `status_bar.rs`.

## Constraints

- Pin behaviour, not pixels: tests assert what a line contains or omits at
  a width, and the measured width, never a full string of the bar.
- `now` and the instants are passed in; no test reads the clock.
- The 7501 emitter's own tests and behaviour are unchanged.
- Nothing polls the daemon. The counters stay zero in this phase.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/cli/render_ratatui.rs`: `status_bar_line`, `render_status_bar`,
  `compact_tokens`, `short_session`, `fmt_uptime`, `truncate_with_ellipsis`.
- `src/cli/render.rs`: `StatusBarState`.
- `src/cli/commands/chat_loop.rs` and `src/cli/commands/stream_ask.rs`:
  the eleven `StatusBarState { … }` literals.
- `src/cli/commands/chat_presence.rs`: the parked task's
  `serde_json::from_str::<Response>` match and `pushes()`.
- `src/cli/program_status_emit.rs`: `Emitter::working`, `blocked`, `idle`.
- `src/cli/commands/approval.rs`: `SessionApproval::hint`.
- `src/config/types.rs`: `Config`, `WebhookConfig` (the shape a new section
  copies); `src/config/handle.rs`: `RESTART_ONLY_SECTIONS` and the reload
  report.
- `src/cli/doctor.rs`: `run_doctor`.
- `src/ipc_types.rs`: `AlertBadge` (from phase 01).

**Edit order.** Each step ends at a green `cargo build`:
1. Write `src/cli/status_bar.rs`: `GlyphSet` and the three constants,
   `ChatState`, `StatusModel`, and a `status_bar_line` that reproduces
   today's content through the new model. Register the module. This is
   your first write.
2. Replace the eleven literals with the model; delete `StatusBarState` or
   reduce it to the model's view. Build.
3. Switch the line to the decision 3 segments and tiers, with its tests.
4. `ChatConfig`, the locale rule, the reload entry, the docs.
5. The presence push enum, the alert arm, the ticking, the saved-name
   tracking, the doctor row.

**Your first edit lands within your first 15 turns.** Never run `tmux`.
`unicode-width` is already a dependency (`src/cli/render.rs` imports it).
`⚠`, `ℹ` and `↻` measure 1 column even in the emoji set; that is correct
and expected, measure, don't assume.

## Test plan

In `src/cli/status_bar.rs` (or a sibling `status_bar_tests.rs`), all pure:
- `glyph_sets_contain_no_variation_selectors`
- `status_line_fits_its_width_at_every_width_from_20_to_160` (every width,
  three dialects, a full model: measured width ≤ pane width)
- `status_line_hides_zero_counters_and_shows_nonzero_ones`
- `status_line_leads_with_the_worst_alert_and_counts_all`
- `status_line_shows_the_resolve_flash_for_ten_seconds`
- `status_line_drops_tier_three_before_tier_two_before_the_hint`
- `status_line_context_fill_steps_and_budget_colours`
- `status_line_working_state_shows_elapsed_seconds`
- `status_line_daemon_down_shows_the_down_glyph_and_text`
- `status_line_omits_an_unnamed_session_and_shows_a_saved_one_dirty`
- `short_model_name_strips_vendor_and_date_and_picks_the_mark`
- `glyphs_fall_back_to_ascii_without_a_utf8_locale`
- `chat_config_rejects_an_unknown_glyph_set` (beside `ChatConfig`)

In `src/cli/commands/chat_presence.rs`:
- `parked_presence_forwards_alert_state_frames`

On a `TestBackend`, beside the existing renderer tests:
- `renderer_draws_the_status_bar_from_the_model` (the bar row contains
  the turn segment and omits `session:` and `up `)

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `grep -c 'session:' src/cli/render_ratatui.rs` prints 0 (today 3).
- [ ] `grep -c 'StatusBarState {' src/cli/commands/chat_loop.rs src/cli/commands/stream_ask.rs`
      prints `0` for both files (today 7 and 4).
- [ ] `grep -c '^\[chat\]' assets/etc/config.toml` prints 1 (today 0).
- [ ] `grep -c 'fn status_bar_line' src/cli/status_bar.rs` prints 1 and
      the same grep on `src/cli/render_ratatui.rs` prints 0.
- [ ] Mutation check, pasted in the Update Log: make the tier drop skip
      tier 3, confirm `status_line_drops_tier_three_before_tier_two_before_the_hint`
      fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib status_line 2>&1 | grep -E '^test result'
cargo test --lib glyph 2>&1 | grep -E '^test result'
cargo test --lib parked_presence_forwards_alert_state_frames 2>&1 | grep -E '^test |^test result'
grep -c 'session:' src/cli/render_ratatui.rs | sed 's/^/session_colon=/'
grep -c 'StatusBarState {' src/cli/commands/chat_loop.rs src/cli/commands/stream_ask.rs
grep -c '^\[chat\]' assets/etc/config.toml | sed 's/^/chat_table=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Create `src/cli/status_bar.rs` (and a `status_bar_tests.rs` sibling if
  the file would pass the hygiene test-line cap).
- Edit the files in the symbol map, `src/cli/mod.rs`, `src/config/mod.rs`
  (re-export), `assets/etc/config.toml`,
  `assets/memory/knowledge/config-reference.md`, `README.md` and
  `CLAUDE.md`.
- Remove `StatusBarState`, `short_session` and `fmt_uptime` if nothing
  else uses them (`cargo build` names any survivor).

## Out of scope

- The daemon's `SessionCounters` push and `SessionInfo`'s output tokens
  (phase 02b): the counters are zero here.
- `top` (phase 03).
- Any change to the OSC 7501 frames the client emits.

## Update Log

<!-- entries appended below this line -->
