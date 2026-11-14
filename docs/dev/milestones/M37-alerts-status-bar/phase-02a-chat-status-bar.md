# Phase 02a — chat-status-bar

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** review
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

### Update — 2026-11-14 02:21 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Picked up phase 02a at the re-dispatch's step 1 (the `chat_config_rejects_an_unknown_glyph_set` test beside `ChatConfig`'s tests — passes, `Err` names all three dialects), then completed steps 2–6: the renderer now takes `&StatusModel` and calls `status_bar::status_bar_line(model, Instant::now(), …)` (old `status_bar_line`/`fmt_uptime`/`short_session`/`start_time` deleted, `StatusBarState` removed from `render.rs`); the chat loop owns one `StatusModel` wired through `RatatuiQueryCtx.sb` and `RatatuiInputCtx.sb` with all eleven `StatusBarState` literals gone (I dropped the now-dead `TokenCtx` and `RatatuiQueryCtx.model`/`session_cost`/`session_has_untracked` out-params since the turn writes the model directly — a clean adaptation, noted here); presence pushes ride a `PresencePush::{Text, Alerts}` enum with the alert arm calling `set_alerts`, a 1 s tick gated on `is_time_based`, saved-name/dirty tracking in `/session save|load|rename` and turn starts, `ChatState` set at the 7501 emitter's points, and a `doctor` glyphs info row; docs (config.toml `[chat]`, config-reference, README, CLAUDE.md) updated and one stale hygiene-allowlist entry removed. Mutation check confirmed the tier test fails when the tier-3 ranks are flattened and passes reverted; the mechanical e2e block is pasted in the Update Log — all greps print 0/1 as specified, build has 0 warnings, clippy and the full test suite are green (2922 passed, 0 failed), and everything is committed with a clean tree.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.8.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.07s


LINT
    Checking daemoneye v2.8.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.85s


TEST
l_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test alert_list_request_returns_the_snapshot ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 38 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v2.8.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.91s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-e4eaed9c20b7817e)
kill: (2712389): No such process
kill: (-2713110): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-84596a3046ce91b6)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-61684e0b51107e5b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c69cc98b7b8dfe7a)
     Running tests/hygiene.rs (target/debug/deps/hygiene-1cb80d9fbff8faf7)
     Running tests/integration.rs (target/debug/deps/integration-9d0880a1c76a698b)
     Running tests/isolation.rs (target/debug/deps/isolation-882adad6d3253c89)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-bad9d4ccd89b2daf)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-3f0be99859fd1842)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -0
- `README.md` — +1 -1
- `assets/etc/config.toml` — +5 -5
- `examples/reanchor_repro.rs` — +5 -12
- `src/cli/commands/agents.rs` — +2 -2
- `src/cli/commands/ask.rs` — +9 -19
- `src/cli/commands/chat.rs` — +2 -5
- `src/cli/commands/chat_loop.rs` — +98 -302
- `src/cli/commands/chat_presence.rs` — +79 -24
- `src/cli/commands/slash.rs` — +3 -0
- `src/cli/commands/slash_session.rs` — +15 -4
- `src/cli/commands/stream.rs` — +3 -11
- `src/cli/commands/stream_ask.rs` — +47 -207
- `src/cli/commands/stream_prompts.rs` — +11 -11
- `src/cli/doctor.rs` — +48 -2
- `src/cli/render.rs` — +0 -19
- `src/cli/render_ratatui.rs` — +21 -142
- `src/cli/render_ratatui_draw.rs` — +11 -34
- `src/cli/render_ratatui_tests.rs` — +47 -333
- `src/cli/status_bar.rs` — +0 -1
- `src/config/types.rs` — +23 -0
- `tests/hygiene_allowlist.txt` — +1 -2

**Commit:** 510441b4402fc7dd348950af0f678ab2b0c45ad3

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
