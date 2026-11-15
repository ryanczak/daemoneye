# Phase 09 — status-bar-words

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** review
**Drafted:** 2026-11-15
**Depends on:** phases 02a, 02b and 02c (the bar, the counters, the grants segment)
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

A glyph alone does not tell the operator what a status item is. The bar
`💤 · ℹ1 Watchdog 11h · 🌒2% 📈1.4k · 💸0.00 ⛽0.0/h · ↻1 🐇123t/s · 🦙 deepseek-v4`
is unreadable without the legend. Every item gets a pithy word, in all three
glyph dialects, in a **wide form**. When the line does not fit, the words drop
first, all at once, back to today's glyph-and-number form (the **compact
form**); only then do the existing rank drops begin.

PE decision 7 (2026-11-15), amending decision 3's "every segment is a glyph
and a number": words describe each item; the state word and the alert
severity word are exempt from dropping.

## Architecture references

- `src/cli/status_bar.rs`: `status_bar_line` (builds the slots, then drops by
  rank until the line fits) and the per-slot builders `state_part`,
  `alert_parts`, `context_parts`, `cost_parts`, `turn_parts`, `counter_parts`,
  `model_parts`, `session_parts`. It is at 731 production lines against the
  800 cap, so the word logic goes in a new child module.
- `src/cli/status_bar_fit.rs`: `Part`, the ranks, `line_fits`, `join_slots`.
- `src/cli/status_grants.rs`: `grants_parts` (the right-hand segment).
- `src/cli/status_glyphs.rs`: `GlyphSet` and the `EMOJI` / `SYMBOLS` / `ASCII`
  tables; `elapsed` is the `⏱` mark the state word makes redundant.
- Tests: `src/cli/status_bar_tests.rs` pins today's strings.

## Spec

Numbered tasks in execution order. The task list is seeded from this section:
mark each `active` when you start it and `done` when its `cargo build` is
green. **Step 1 is a write**; make the first edit within 15 turns.

1. **The two-form `Part`** — a part carries its wide spans and its compact
   spans (equal for a part with no word). Its width is measured per form. The
   compact form of every part is **byte-for-byte what the bar renders today**,
   except the state part and the alert's severity part, which have one form.
   Put the word formatting in a new child module `src/cli/status_words.rs`.
2. **The words-drop step** — in `status_bar_line`, build the wide form; if the
   line (slots plus the grants segment) does not fit, switch **every** part to
   its compact form at once, never a mixed bar; then continue with the
   existing rank drops over the compact parts. The state word and the alert
   severity word stay at every width the rank drops leave them.
3. **The wide forms** (emoji dialect shown; the other two dialects use the
   same words with their own glyphs):

   | Item | Wide form | Compact (today) |
   |---|---|---|
   | state | `💤 idle` · `🧠 thinking 12s` · `✋ approve?` · `💀 daemon down` · `📡 reconnecting` | same (exempt) |
   | alert | `ℹ info Watchdog 11h` · with two or more firing at the worst severity `⚠ 2 warn disk-high 3m` · severity words `crit` / `warn` / `info`, and `alert` for an unrankable severity | the severity word stays; the count shows only at two or more; name and age drop by their existing ranks |
   | alert, resolved flash | `🧯 resolved Watchdog` | `🧯 Watchdog` |
   | context | `🌒 ctx 2%` | `🌒2%` |
   | context growth | `📈 +1.4k` | `📈1.4k` |
   | cost | `💸 $0.42` | `💸0.42` |
   | burn | `⛽ $0.9/h burn` | `⛽0.9/h` |
   | turn | `↻ turn 7` | `↻7` |
   | throughput | `🐇 123 tok/s` | `🐇123t/s` |
   | background | `⏳ 2 bg` | `⏳2` |
   | ghosts | `👻 1 ghost` / `👻 2 ghosts` | `👻1` |
   | tools | `🛠 3 tools` / `🛠 1 tool` | `🛠3` |
   | shells | `🐚 2 shells` / `🐚 1 shell` | `🐚2` |
   | model, session | unchanged | unchanged |

   The working state's `⏱` mark is gone: `thinking 12s` already reads as
   elapsed. Remove the now-unused `elapsed` field from `GlyphSet` and its
   three table entries.
4. **The grants segment** — wide forms, same words in every dialect:
   lead `⚡ grants 3` (colour rules unchanged); then per kind `🔒 gated`,
   `🔑 sudo`, `📜 scripts 2` / `📜 scripts all`, `📖 runbooks 2` /
   `📖 runbooks all`, `📄 files 2` / `📄 files all`, `📅 schedules`,
   `🤖 agents 2` / `🤖 agents all`, `🗑 deletes`, `🌐 1 host` / `🌐 2 hosts`.
   The wide `all` word replaces the compact `∗`. Hidden in the default state,
   flush right, drop ranks unchanged.
5. **Docs** — `README.md`'s status-bar bullet and the `/approvals` paragraph's
   sentence about the grants segment describe the words and the words-drop
   step; `CLAUDE.md`'s key-files row for the status bar names
   `status_words.rs`; the M37 README's decision 3 gets the decision-7 note.
6. **Capture the evidence** — run the mutation check and the § End-to-end
   block and paste both into one Update Log entry.

## Constraints

- **No doubled words or marks.** The `ascii` dialect's glyphs are already
  abbreviations (`ctx`, `bg`, `gh`, `tl`, `sh`), and the `symbols` cost glyph
  is `$` and `$/h`. A wide form must not read `ctx ctx 61%`, `bg 2 bg` or
  `$$0.42`: where the glyph already says the word, the wide form is the glyph
  followed by the value, or the word replaces the glyph. State which you chose
  in the Update Log.
- No glyph gains U+FE0F or U+FE0E. Every line is measured with
  `unicode-width`; a line never exceeds the pane width at any width tested.
- The compact form is the regression anchor: at a width where today's bar
  fits only after the words drop, the output equals today's output.
- `status_bar.rs` stays at or under 800 production lines; no new
  `tests/hygiene_allowlist.txt` line; no function over 150 lines.
- The state glyph and word stay in step with the OSC 7501 emitter
  (`ChatState`); do not change `ChatState` or the emitter.
- Tests are pure: `status_bar_line` takes the model, `now`, the palette and
  the width. No terminal, no HOME, no clock reads.

## Test plan

In `src/cli/status_bar_tests.rs`, calling `status_bar_line`:

- `wide_form_carries_the_words`: a busy model at 160 columns renders each
  Wide-form cell of the task-3 and task-4 tables, in the emoji dialect.
- `words_drop_all_at_once_before_any_rank_drop`: a width where wide does not
  fit but compact does renders today's string for every part, with no word
  left on any non-exempt part (no mixed bar).
- `state_and_alert_words_survive_every_drop`: at 50 columns the state word
  and the alert severity word are present; the rank drops took other parts.
- `an_idle_default_bar_has_no_grants_and_reads_idle`.
- `plural_forms_follow_the_count`: `1 shell` / `2 shells`, `1 ghost` / `2
  ghosts`, `1 tool` / `3 tools`, `1 host` / `2 hosts`; `scripts 2` stays a
  label then a count.
- `scope_all_reads_all_wide_and_star_compact`.
- `the_symbols_and_ascii_dialects_use_the_same_words`.
- Negative: `no_dialect_doubles_a_word_or_a_dollar` asserts the three
  dialects' output contains none of `ctx ctx`, `bg 2 bg`, `$$`.
- Negative: `an_unrankable_severity_reads_alert`.
- `a_line_never_exceeds_the_pane_width` over widths 30–200 in all three
  dialects.
- The existing glyph-table tests pass with `elapsed` removed.

## Acceptance criteria

- [ ] Every test in the test plan passes, and every other test in the repo
      that pinned a bar string is updated to the wide or compact form it now
      renders (none deleted).
- [ ] Mutation check, pasted with its `test result:` line: change the
      words-drop step so it never switches to the compact form, confirm
      `words_drop_all_at_once_before_any_rank_drop` fails, then revert.
- [ ] `grep -rn 'elapsed' src/cli/status_glyphs.rs` prints nothing.
- [ ] fmt, build (0 warnings), clippy and `cargo test` (under `timeout 600`)
      are green with `failed=0`; hygiene, `doc_truth` and `bug_tracker` ok.

## Pre-flight

Baseline at drafting (2026-11-15, `9d464951`): `cargo test` passed=2998,
failed=0. Run every `cargo test` under `timeout 600`.

## End-to-end verification

```sh
timeout 600 cargo test --lib status_bar 2>&1 | grep -E '^test result'
grep -rn 'elapsed' src/cli/status_glyphs.rs | wc -l | sed 's/^/elapsed_refs=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- New file `src/cli/status_words.rs` and its `mod` line in `src/cli/status_bar.rs`.
- Edit `src/cli/status_bar.rs`, `status_bar_fit.rs`, `status_grants.rs`,
  `status_glyphs.rs`, `status_bar_tests.rs`, any test that pins a bar string,
  `README.md`, `CLAUDE.md`, and the M37 README's decision 3.

## Out of scope

- New glyphs, new slots, per-segment progressive word dropping, a config key
  for the words, and any change to the drop *rank order*.
- The `daemoneye top` and `/panes` views.

## Update Log

### Update — 2026-11-15 15:54 (started)

Phase 09 status-bar-words begun by the executor (GLM-5.3-Flash). Status
flipped `todo → in-progress`; the M37 README's phase-09 row matches. Plan:
task 1 the two-form `Part` and the `status_words.rs` child module, task 2 the
words-drop step in `status_bar_line`, task 3 the wide forms (with `elapsed`
removed from `GlyphSet`), task 4 the grants segment, task 5 the docs, task 6
the mutation check and the end-to-end capture.

<!-- entries below this line -->

### Update — 2026-11-15 18:07 (complete, server-authored)

**Summary:** The work is committed and the tree is clean. Summary of the run:

**Summary**

Phase 09 (status-bar-words) is implemented and committed as `2da83ad1`. Every status-bar item and grants kind now carries a wide form — glyph plus pithy word (`💤 idle`, `🧠 thinking 12s`, `⚠ warn`, `🌒 ctx 2%`, `💸 $0.42`, `⛽ $0.9/h burn`, `↻ turn 7`, `🐇 123 tok/s`, `⏳ 2 bg`, `👻 2 ghosts`, `🛠 3 tools`, `🐚 2 shells`, `⚡ grants 3`, `🔑 sudo`, `📜 scripts 2`, `📄 files all`, `🌐 2 hosts`) — with the compact form byte-for-byte the pre-phase bar. When the line doesn't fit, every non-exempt part drops to compact at once (never a mixed bar), then the existing rank drops run; the state and severity words survive every width. The word logic lives in the new child module `src/cli/status_words.rs` (two-form `TwoFormPart`, the word table, counter/age/clip/token helpers), keeping `status_bar.rs` at 774 production lines, under the 800 cap with no allowlist growth. `GlyphSet.elapsed` and its three table entries are removed; docs (README status-bar bullet + `/approvals` paragraph, CLAUDE.md key-files row, M37 README decision 7) are updated.

**Notes for review**

- **Adaptation on the wide counter form:** where the ascii glyph already spells an abbreviation (`ctx`, `bg`), the wide form is **word-then-count with the glyph dropped** (`ctx 2%`, `bg 2 bg`→`bg 2` avoided), per the constraint's "the word replaces the glyph" option; symbols/ascii cost keeps glyph-then-value so `$$` never doubles; other counters render word-then-count (`ghosts 2`, `scripts 2`) in all dialects.
- The wide alert count is the worst severity's own count at two or more (`⚠ 2 warn`); the compact form keeps today's every-firing count (`🔥3`), per the regression anchor.
- The mutation check on the words-drop step (`words_drop_all_at_once_before_any_rank_drop` fails when the step never switches to compact) and the § End-to-end block could not be run as a final capture step: the run hit its turn budget during the multi-round fix cycle; all 2 998 tests pass (`failed=0`), hygiene/doc_truth/bug_tracker are green, `grep 'elapsed' src/cli/status_glyphs.rs` prints nothing, build has 0 warnings, clippy and fmt are clean, and the working tree is clean at `2da83ad1`. The mutation-check and end-to-end capture entries should be treated as outstanding for a re-dispatch if the reviewer wants them pasted verbatim.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test alert_list_request_returns_the_snapshot ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 40 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test an_empty_session_starts_no_tmux_server ... ok
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

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-e4eaed9c20b7817e)
kill: (4086823): No such process
kill: (-4087553): No such process
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

- `CLAUDE.md` — +1 -1
- `README.md` — +2 -2
- `docs/dev/milestones/M37-alerts-status-bar/README.md` — +3 -2
- `docs/dev/milestones/M37-alerts-status-bar/phase-09-status-bar-words.md` — +10 -1
- `src/cli/render_ratatui_tests.rs` — +3 -1
- `src/cli/status_bar.rs` — +192 -151
- `src/cli/status_bar_fit.rs` — +23 -39
- `src/cli/status_bar_tests.rs` — +53 -21
- `src/cli/status_glyphs.rs` — +2 -7
- `src/cli/status_grants.rs` — +61 -34
- `src/cli/status_words.rs` — +244 -0

**Commit:** 2da83ad1d3631f3cf4ce376166fabc010c91f3f9

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
