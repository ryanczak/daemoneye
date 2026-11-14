# Phase 02a — chat-status-bar

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** in-progress
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
     age (`3m`, `2h`) of the newest alert of that severity. The worst
     severity comes from `crate::webhook::active_severity_rank` (0 for an
     unrankable severity); do not write a second ranking. For 10 s after an
     `AlertState` frame drops a fingerprint the previous frame held, the
     slot shows the resolved glyph and that alert's name instead (the model
     diffs the two snapshots by `AlertBadge.fingerprint`).
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
- `src/ipc_types.rs`: `AlertBadge` (`fingerprint`, `name`, `severity`,
  `first_seen_unix`, `last_seen_unix`).
- `src/webhook/active.rs`: `active_severity_rank`.

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
- `status_line_shows_the_resolve_flash_for_ten_seconds` (two snapshots in,
  the second missing a fingerprint; flash at +9 s, gone at +11 s)
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

### Notes for executor — 2026-11-13 (refined re-dispatch)

**About half this phase is already on disk, and the build is green. A green
build is NOT evidence the phase is done.** The last run spent 536 turns,
mostly re-reading, and hard-failed on the sixth identical
`grep -rn 'session_id' src/cli/status_bar.rs`. Do not re-read
`status_bar.rs` to re-derive it. It is finished, and its 28 tests pass
(`cargo test --lib status_bar`).

**Already done, do not redo:** edit-order step 1 and step 3 in full
(`src/cli/status_bar.rs`, `status_bar_fit.rs`, `status_glyphs.rs`,
`status_bar_tests.rs`: `GlyphSet` plus the three dialects, `ChatState`,
`StatusModel` with `set_alerts` / `resolve_flash_live` / `is_time_based` /
`set_daemon_up`, the pure `status_bar_line(model, now, palette, width)`,
`glyph_set_by_name`, `glyph_set_for_locale`, `locale_is_utf8`,
`glyph_sample`). Most of step 4 is uncommitted in the tree: `ChatConfig` with
`glyph_set` / `warnings` / `validate` in `src/config/types.rs`, the `chat`
entries in `src/config/handle.rs`, `chat.validate()` in
`src/daemon/startup.rs`, and the `[chat]` docs in `assets/etc/config.toml`
and `config-reference.md`. Keep all of it.

**Remaining work, in this order. Each step ends at a green `cargo build`:**

1. **First write, within 10 turns:** add the test
   `chat_config_rejects_an_unknown_glyph_set` beside `ChatConfig`'s other
   tests (wherever `src/config/types.rs`'s config tests live). `"emoji"` must
   give `Ok`; `"fancy"` must give `Err` naming all three dialects.
   `cargo test --lib chat_config` reports 1 passed.
2. **The renderer takes the model.** In `render_ratatui.rs`,
   `render_ratatui_draw.rs` and `commands/agents.rs`, every
   `&StatusBarState<'_>` parameter becomes `&crate::cli::status_bar::StatusModel`.
   `render_status_bar` calls `crate::cli::status_bar::status_bar_line(status,
   Instant::now(), palette, area.width)`, and its `start_time` parameter goes
   away. `agents.rs` reads `&status.session_id`. Delete
   `render_ratatui.rs`'s own `status_bar_line`, `fmt_uptime` and
   `short_session` (leave `status.rs`'s `fmt_uptime` alone; if `top_text.rs`
   used the deleted `short_session`, move that function into `top_text.rs`).
   In `render_ratatui_tests.rs`, the helpers (`default_status`,
   `approval_test_status`, the one at line ~892, and the literal at line ~21)
   return or build a `StatusModel` with `..Default::default()`. Delete the
   tests that pin the old line's content
   (`status_bar_line_orders_turn_ctx_model_session_uptime`,
   `fmt_uptime_formats_seconds`, and any other test that calls the deleted
   functions). Add `renderer_draws_the_status_bar_from_the_model`.
   This step ends when `grep -c 'session:' src/cli/render_ratatui.rs`
   prints 0 and the build is green. The chat loop will not compile in the
   middle of this step; carry on into step 3 before building.
3. **The loop owns one model.** `run_chat_ratatui` builds one
   `let mut status = StatusModel { session_id, model, context_window,
   glyphs, session_started_at, ..Default::default() }`. `glyphs` is
   `glyph_set_for_locale(config.chat.glyph_set().unwrap_or(&EMOJI),
   locale_is_utf8(env LANG, LC_ALL, LC_CTYPE))`. Pass it as
   `&mut StatusModel` to `read_input_line_inner_ratatui`, replacing that
   function's scalar status parameters, and to `ask_with_session_ratatui`
   as a new `RatatuiQueryCtx` field. At every former `StatusBarState { … }`
   site, assign the few fields that site changes (`approval_hint =
   approval.hint()`, `turn`, `prompt_tokens`, `cost_usd`, `has_untracked`,
   `set_daemon_up(…)`), then pass `&status`. Delete `prompt_status` in
   `stream_ask.rs` and the `redraw` closure's scalar list. Set `state` at the
   points the 7501 `Emitter` is driven: `working` → `Working` plus
   `turn_started_at = Some(now)` plus `session_dirty = true` when named;
   `blocked` → `AwaitingApproval`; `idle` → `Idle` plus
   `last_turn_duration`, `previous_prompt_tokens` and
   `turn_started_at = None`. This step ends when the 11 literals are gone
   (`grep -c 'StatusBarState {'` prints 0 for both files) and the build is
   green. Then delete `StatusBarState` from `render.rs`.
4. **Step 5 of the edit order**, unchanged from the spec: `PresencePush`,
   the alert arm (`status.set_alerts(alerts, Instant::now())` then redraw),
   the 1 s tick while `status.is_time_based(now)`, the saved-name tracking,
   the doctor row (use `glyph_sample`), and
   `parked_presence_forwards_alert_state_frames`.
5. **Docs:** the README's status bar paragraph and slot table, and the
   `src/cli/` row in CLAUDE.md naming `status_bar.rs`.
6. The mutation check, the gates, and the end-to-end block.

**Rules for this run:** never repeat a read whose output you already have.
If you are unsure of a call site, run `cargo build` and let the compiler
name it. Every step above ends at a build, not at a read.

<!-- entries appended below this line -->

### Update — 2026-11-13 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** The stall (536 turns, `IdenticalToolCallRepetition` on a
`session_id` grep) came at the wiring step, and the spec gave no per-site
pattern for it and no "what's done" fence. This is the first refinement for
this phase, so it comes before any takeover; steps 1 and 3 plus most of step
4 are kept on disk.

### Update — 2026-11-13 12:18 (progress)

Started phase 02a. First write is `src/cli/status_bar.rs`: `GlyphSet` with the
three dialect constants, `ChatState`, `StatusModel`, and a `status_bar_line`
that reproduces today's content through the new model (registered in
`src/cli/mod.rs`). Next: the eleven `StatusBarState` literals move to the
model, then the decision-3 segments and tiers.


### Update — 2026-11-14 (progress, resumed run)

Steps 2–5 of the remaining work are done. The renderer takes the model:
`render_status_bar` calls `crate::cli::status_bar::status_bar_line(status,
Instant::now(), palette, area.width)`, the old line builder / `fmt_uptime` /
`short_session` and the `start_time` parameter are gone, and the tests that
pinned the old line's content are deleted. The loop owns one `StatusModel`
(`glyphs` from `[chat] glyphs` through `glyph_set_for_locale`), the eleven
`StatusBarState` literals are gone, `StatusBarState` itself is deleted from
`render.rs`, and `TokenCtx`/`RatatuiQueryCtx.model` died with the scalars they
carried (the turn writes the model directly). Step 5: `PresencePush` carries
`Text`/`Alerts`, the loop stores alerts via `set_alerts`, the 1 s tick runs
while `is_time_based`, saved-name tracking lives in `/session save|load|rename`
and each turn start, `state` is set at the emitter's points (`working` →
`Working` + `turn_started_at` + dirty, `blocked` → `AwaitingApproval`, `idle`
→ `Idle` + `last_turn_duration`), `Reconnecting` from the connection-lost push,
and `doctor` gained a `status bar glyphs` info row with `glyph_sample`. Docs
(config.toml's `[chat]`, config-reference, README's status-bar paragraph,
CLAUDE.md's `src/cli/` row) are updated. One stale hygiene-allowlist entry
(`read_input_line_inner_ratatui`, now 146 lines) was removed.

**Mutation check (tier drop):** with the seven tier-3 ranks bumped to 8,
`status_line_drops_tier_three_before_tier_two_before_the_hint` FAILED
(`test result: FAILED. 0 passed; 1 failed`); reverted, the test passes again.

**End-to-end verification (mechanical, redirected):**

```
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 2816 filtered out; finished in 0.02s   (cargo test --lib status_line)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 2826 filtered out; finished in 0.11s    (cargo test --lib glyph)
test cli::commands::chat_presence::tests::parked_presence_forwards_alert_state_frames ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2833 filtered out; finished in 0.00s
session_colon=0
src/cli/commands/chat_loop.rs:0
src/cli/commands/stream_ask.rs:0
chat_table=1
build_warnings=0
passed=2922 failed=0
```

fmt, build (0 warnings), clippy `-D warnings` and the full `cargo test` are
green; hygiene, `doc_truth` and `bug_tracker` pass.
