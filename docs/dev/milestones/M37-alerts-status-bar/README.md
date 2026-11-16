# M37 — Alerts out of history, onto the status bar and `top`

**Goal:** fix GitHub issue #3. A webhook alert below `[webhook]
severity_threshold` no longer enters any chat session's history. Every
firing alert shows on the chat status bar and in a `top` Alerts panel
instead. The chat status bar is redesigned to say the most important
things in the fewest columns.

**Status:** done — released as 2.9.0 (2026-11-16)

**Release:** 2.9.0

**Depends on:** M36 (the tree as released in 2.8.0).

**Why now.** Issue #3 (filed 2026-11-08 from a live deadman setup): an
always-firing `info` heartbeat with Alertmanager's 15 m `repeat_interval`
appended a `[Webhook Alert]` turn to every active chat every 15 minutes,
despite `severity_threshold = "warning"`. In `process_alert`
(`src/webhook/process.rs`), history injection and the chat-pane notice run
before the severity gate, and the gate controls only the popup and ghost
analysis. The `webhook_discarded` event claims the alert was discarded
while the user still sees it in chat.

**PE decisions (2026-11-12, at scoping):**

1. **Numbering.** This is M37 and releases as 2.9.0. The MCP surface
   becomes M38 and Knowledge M39.
2. **History takes only alerts at or above the threshold.** Below it, an
   alert goes to the event log and the alert registry only: no history
   turn, no chat-pane notice, no popup, no analysis. At or above it,
   delivery is unchanged. **The gate fails closed on the alert side** (PE,
   2026-11-12, reversing the earlier fail-open): a severity that is not
   `critical` / `warning` / `info` or an alias is *below* threshold, and it
   ranks below `info` wherever alerts are ordered. A `severity_threshold`
   that is itself unrankable is a config-load warning and behaves as
   `warning`. `auto_analyze`'s own write-ups still go into history.
3. **Status bar: glyph-led, zero-valued items hidden, emoji by default**
   (PE approved 2026-11-12). One line. Every segment is a glyph and a
   number; words appear only for the alert name, the model and the
   session name. Colour sits on the glyph, not the text. Three glyph
   dialects share one slot table, chosen by `[chat] glyphs = "emoji" |
   "symbols" | "ascii"`; `emoji` is the default, and `ascii` is forced when
   the locale is not UTF-8. No glyph carries a variation selector; every
   emitted line is measured with `unicode-width`.

   | slot | emoji | symbols | ascii | meaning |
   |---|---|---|---|---|
   | state | 💤 🧠 ✋ 💀 📡 | ○ ◐ ⏸ ✖ ⟳ | `-` `*` `?` `X` `~` | idle · working (with `⏱` elapsed) · awaiting approval · daemon down · reconnecting |
   | alerts | 🔥 ⚠ ⚠ ❔ · 🧯 | ● red/amber/gray/dim · ✓ | `!!` `!` `i` `?` | the worst firing alert's severity, then the count of all firing; name and age when room; `🧯` flashes 10 s on a resolve |
   | context | 🌑 🌒 🌓 🌔 🌕 | ◔ ◑ ◕ ● | `ctx` | the window's fill, `%` after it (amber ≥ 75 %, red ≥ 90 %); `📈`/`↗` growth since last turn when room |
   | cost | 💰 · 💸 | `$` · `$/h` | `$` | session spend with its currency sign (`$`, or `?` when any call had unknown pricing) before the value; burn rate when room |
   | turn | 🔄 | ↻ | `t` | turn count; `🐇`/`🐢` (symbols `»`/`›`) last-turn tokens per second when room (fast ≥ 30 t/s) |
   | background | ⏳ | ⧖ | `bg` | this session's running `wait: false` commands |
   | ghosts | 👻 | ◆ | `gh` | active ghost runs |
   | tools | 🔧 | ⚒ | `tl` | silent tool calls this session |
   | shells | 🐚 | ⌨ | `sh` | this session's open PTY shells |
   | model | ⟡ ◎ ♊ 🦙 | ⟡ ◎ ♊ | none | vendor mark (Anthropic, OpenAI, Gemini, local) and the short name, ≤ 12 columns |
   | session | 📌 · 📌✎ | ▣ | `@` | saved name; `✎` when dirty; nothing when unnamed |
   | grants | 🔒 · 🔑 📜 📖 📄 📅 🤖 🗑 🌐 | ⚡ · # § ¶ ▤ ◷ ⚉ ⌫ ⌂ | `A` · `# s r f c g d h` | the lead glyph coloured by the riskiest grant (red: sudo, deletes or any class-wide grant; amber: per-name grants; green: commands gated), then the count of active kinds; per-kind glyph + count (`∗` for all) when room; hidden in the default state; flush right (PE, 2026-11-14, amending the original "today's approval hint") |

   Gone from the bar: the unnamed session id and the uptime.

   **Fitting:** three tiers, dropped from the lowest up, never truncated
   mid-segment. Tier 3 first: the velocities (`📈`, `💸`, `🐇`), the alert
   name and age, the session name, the model. Tier 2: shells, tools,
   ghosts, background, in that order. Tier 1 always: state, alerts,
   context, cost, turn. The grant hint goes after tier 2 and before tier
   1. Below 36 columns, separators become single spaces.

   ```
   120:  💤 · 🔥2 DiskFull 3m · 🌓 20% 📈3.1k · 💰0.42 💸$1.9/h · 🔄12 🐇38t/s · ⏳2 🔧7 👻1 🐚3 · ⟡ sonnet-5.5 · 📌 incident-42   🔒4
    80:  💤 · 🔥2 3m · 🌓 20% · 💰0.42 · 🔄12 · ⏳2 🔧7 👻1 🐚3 · ⟡ sonnet-5.5        🔒4
    80:  🧠 ⏱12s · 🌓 20% 📈3.1k · 💰0.42 · 🔄12 · 🔧7 🐚1 · ⟡ sonnet-5.5              🔒4
   120 (grants detail):  … · ⟡ sonnet-5.5 · 📌 incident-42            🔒4 🔑 📜2 📄∗ 🗑
    40:  💤 🔥2 🌓20% 💰0.42 🔄12 ⏳2
   down: 💀 daemon down · 💰0.42 · 🔄12 · ⟡ sonnet-5.5
   ```
7. **Status bar words** (PE, 2026-11-15, amending decision 3). Decision 3's
   "every segment is a glyph and a number" gives way: each item carries a
   word (`idle`, `thinking 12s`, `ctx 2%`, `turn 7`, `2 shells`, `grants 3`,
   `scripts 2`, `model deepseek-v4`), in all three dialects. When the line
   does not fit, the words drop first, all at once, back to the
   glyph-and-number form; the state word never drops (the alert severity
   word does drop, to `⚠1`; PE, 2026-11-15). Phase 09; the model word is
   phase 11. The working state's `⏱` elapsed mark went with it:
   `thinking 12s` already reads as elapsed.
8. **The bar's final form** (PE, 2026-11-15, from a rendered pass at
   `c176348c`). Three changes and the defects they came from:
   - **Drop order.** The words no longer drop before any rank drop. New
     order, lowest drops first: grants detail → the velocities (growth,
     burn, throughput) → alert name → alert age → **the words, all at
     once** → session → model → shells → tools → ghosts → background →
     grants lead → turn → cost → context → alerts → state. The state word
     and the alert glyph stay. When the words drop, every part dropped to
     make room for them comes back — the words cost the columns, not the
     detail — and the compact drops run from the lowest rank again
     (architect, phase-12 takeover, 2026-11-15). The ascii dialect's letter
     glyphs (`t`, `bg`, `tl`) are replaced by their word in the wide form,
     never doubled (`turn 12`, `7 tools`).
   - **Glyphs** (PE, 2026-11-15, after the phase-12 review): the emoji turn
     glyph is `🔄` (was `↻`; symbols keep `↻`), tools is `🔧` (was `🛠`),
     and the grants lead is `🔒` (was `⚡`; symbols keep `⚡`, ascii `A`).
     The `gated` detail glyph is gone in every dialect — the lead already
     carries it, green when commands are gated — so no lock is doubled.
     Burn is `💸` (was `⛽`) and its rate carries the spend's currency sign
     in both forms (`💸$0.4/h`, `💸?0.4/h` under unknown pricing). The
     counters sit in the order background, tools, ghosts, shells
     (`⏳2 🔧7 👻1 🐚3`); their drop ranks are unchanged. The burn part has
     no word: `/h` is its word, so wide is `💸 $0.4/h` (PE, 2026-11-15).
     The model word is `model`, no colon: `🦙 model deepseek-v4` (PE,
     2026-11-15).
   - **Counter grouping.** The four counters (`⏳2 🔧7 👻1 🐚3`) are one
     slot, single spaces inside it, ` · ` between slots; each keeps its own
     rank and drops alone.
   - **Unknown pricing is a currency sign.** `?` sits where known pricing's
     `$` sits — before the value (`💰?0.42` compact, `💰 ?0.42` wide,
     symbols/ascii `?0.42`); the `❔` suffix is gone in every dialect.
   Defects fixed with them: the wide cost form lost the unknown mark; burn
   in symbols/ascii doubled its `/h` (`$/h0.4/h` → `$0.4/h`); ascii
   throughput doubled its `t/s` (`t/s38t/s` → `38t/s`); symbols used the
   emoji `🐇`/`🐢` (now `»`/`›`); the symbols agents glyph is `⚉`.
   Phase 12.
4. **Many alerts: the worst, plus a count.** The alert glyph is the worst
   firing severity, the number is every firing alert; the name and age
   are the newest alert of that severity.
5. **`top` gets an Alerts panel** (PE, 2026-11-12) listing every firing
   alert from the same daemon state.
6. **The daemon pushes the counters** the bar shows and the client never
   polls: a `SessionCounters` frame on the presence connection whenever a
   session's background commands, active ghosts or open shells change,
   and once at attach. Throughput comes from a per-turn output-token count
   the turn's `SessionInfo` carries.

**Rules every phase carries:**

- **A defect fix comes with a test that fails on the pre-phase tree.** The
  phase doc names the test, and the executor pastes the red run.
- **Tests call the production seam** (STANDARDS.md § 1, folded at the M36
  close). A copy of production logic in a test module is a fake test.
- **A UX change pins behaviour on a `TestBackend`**, not pixels: what a
  row contains or omits, and what a key does.
- **Every first dispatch carries the anti-read-loop block** (WORKFLOW.md,
  folded at the M36 close): a symbol map, an edit order whose step 1 is a
  write, and "first edit within 15 turns".
- **CLAUDE.md and README.md follow the code**. `tests/doc_truth.rs` is
  the gate.

**Exit criteria:**

- **A below-threshold alert stays out of history.** An `info` alert posted
  to the webhook with `severity_threshold = "warning"` adds no message to a
  live chat's session jsonl. The chat's status bar shows it within a few
  seconds, and `top`'s Alerts panel lists it.
- **An at-threshold alert is delivered as before.** A `critical` alert is
  injected into history, as today, and also leads the status bar in red.
- **A resolved alert clears.** Its resolve notification removes it from
  the status bar and from `top`.
- **The bar reads as in decision 3**, live on pinky at 120 and 80
  columns: the state glyph changes through a turn and an approval prompt,
  a firing alert leads with its severity glyph, the context moon matches
  the percentage, zero-valued counters are absent, the background and
  shell counts follow a `wait: false` command and `open_shell`, and
  `[chat] glyphs = "symbols"` swaps every glyph on the next chat start
  after a reload (the client reads `[chat]` when it starts; the daemon's
  reload does not reach a running client). With the daemon stopped, the bar
  shows `💀 daemon down`.
- **The four gates pass**, and `cargo test` passes at 2894 tests plus the
  new ones.
- **`2.9.0` is released** per `REXYMCP.md` § Versioning, and issue #3 is
  closed with a link to the release.

## Architecture references

- `src/webhook/process.rs`: `process_alert`, `inject_into_sessions`,
  `notify_chat_panes`, `severity_rank`.
- `src/daemon/session.rs`: `PresenceConnection`, `presence_broadcast_in`.
- `src/daemon/server/chat_presence.rs`: the `ChatAttach` hold loop.
- `src/cli/render_ratatui.rs`: `status_bar_line`.
- `src/cli/render.rs`: `StatusBarState`.
- `src/cli/commands/top_tui.rs` and `top_load.rs`: `Panel`, the loaders.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Expected count: 5 (phase 02 split into 02a/02b at scoping; 02c added 2026-11-14 for the grants slot; 09 added 2026-11-15 for the status-bar words; 10 added 2026-11-15 for chat panel polish before the live check; 11 added 2026-11-15 for the model word; 12 added 2026-11-15 for the bar's final form).

| #  | Phase | Scope | Status |
|----|-------|-------|--------|
| 01 | daemon-alert-state ([phase-01-daemon-alert-state.md](phase-01-daemon-alert-state.md)): the severity gate moves ahead of history injection and the chat-pane notice. A daemon-wide active-alert registry, the `AlertState` frame on the presence connection and at attach, and `Request::AlertList` | issue #3, decisions 2 and 4 | done |
| 02a | chat-status-bar ([phase-02a-chat-status-bar.md](phase-02a-chat-status-bar.md)): the glyph-led bar of decision 3 on the client: the slot table and three dialects, `[chat] glyphs`, the tiers, the state slot shared with the OSC 7501 emitter, alert state from `AlertState` frames, the ticking ages; counters fed zero until 02b | decisions 3 and 4 | done        |
| 02b | session-counters ([phase-02b-session-counters.md](phase-02b-session-counters.md)): the daemon's `SessionCounters` push (a 1 s diff per present session, plus once at attach) and the turn's output tokens on `UsageUpdate`; the client feeds them to the bar | decision 6 | done |
| 02c | grants-segment ([phase-02c-grants-segment.md](phase-02c-grants-segment.md)): the grants slot in decision 3's grammar — a `GrantSummary` struct, lead glyph coloured by risk, a count, per-kind glyphs when room, hidden by default, dialect-aware | decision 3 (grants row, amended 2026-11-14) | done        |
| 03 | top-alerts-panel ([phase-03-top-alerts-panel.md](phase-03-top-alerts-panel.md)): `top` loads `Request::AlertList` into a fifth `--once` section and a non-focusable Alerts panel between Shells and Activity | decision 5 | done      |
| 04 | local-provider-pricing ([phase-04-local-provider-pricing.md](phase-04-local-provider-pricing.md)): `ModelEntry::pricing()` lets user `*_cost_per_mtok` rates win over the ollama/lmstudio zero; local stays free when no rate is set | issue #6 (pre-close bug fix) | done |
| 05 | adopted-shell-close ([phase-05-adopted-shell-close.md](phase-05-adopted-shell-close.md)): a chat session may `close_shell` an `Owner::Adopted` shell; the maintenance tick sweeps dead hosts' `.lock`/`.sock`/sidecar files for ids the registry no longer holds | issue #2 remainder (pre-close bug fix) | done |
| 06 | self-host-refusal ([phase-06-self-host-refusal.md](phase-06-self-host-refusal.md)): a `[hosts.<name>]` entry whose `ssh` names the daemon's own machine is warned about at startup and refused by every host-taking tool before its prompt; nothing is routed local | issue #8 (pre-close bug fix; PE: warn + refuse, 2026-11-14) | done |
| 07 | scheduler-recovery ([phase-07-scheduler-recovery.md](phase-07-scheduler-recovery.md)): a recurring job left `Running` by a restart skips to its next slot at daemon startup; a failed recurring run no longer ends the job, and still ends the run `hard_fail` | issue #9 (pre-close bug fix; PE: skip to next slot, fix failed-recurring too, 2026-11-14) | done        |
| 08 | away-digest ([phase-08-away-digest.md](phase-08-away-digest.md)): the first chat to attach after a gap gets one summary — script runs per job (ok/failed) and alerts fired per name — delivered once, joined to the attach brief | issue #5 (pre-close bug fix; PE: first-attach, once, counts since the last interactive chat, 2026-11-15) | done        |
| 09 | status-bar-words ([phase-09-status-bar-words.md](phase-09-status-bar-words.md)): every bar item and every grants kind gets a pithy word in all three dialects; when the line does not fit the words drop first, all at once, back to the glyph-and-number form | decision 7 (PE, 2026-11-15, amending decision 3) | done           |
| 10 | chat-panel-polish ([phase-10-chat-panel-polish.md](phase-10-chat-panel-polish.md)): the input box takes the history panels' bold red border and `╭`/`╮` `╰`/`╯` corners; the terminal approval panel is titled `user@host:sN`, the shell picked before the prompt (never created or connected) and pinned for the run; the info alert glyph becomes a yellow `⚠` and the cost glyph `💰` | pre-close polish (PE: pick + pin, user/host from the ssh target, 2026-11-15) | done |
| 11 | status-bar-model-word ([phase-11-status-bar-model-word.md](phase-11-status-bar-model-word.md)): the model slot's wide form reads `🦙 model: deepseek-v4.1`; the word drops with the others back to the glyph and name; the 12-column clip cuts at a token boundary (`deepseek-v4`, not `deepseek-v4.`) | decision 7 (PE, 2026-11-15) | done |
| 12 | status-bar-final-form ([phase-12-status-bar-final-form.md](phase-12-status-bar-final-form.md)): the words drop after the grants detail, the velocities and the alert name/age, not before everything; the counters form one slot; unknown pricing is `?0.42` not `0.42❔`; the `/h/h` and `t/s…t/s` doublings, the emoji hare in `symbols` and the `⚦` agents glyph are fixed; whole-line snapshot tests at 160/120/80/40 in three dialects | decision 8 (PE, 2026-11-15) | done |

## Notes

## Retrospective — 2026-11-15, live check 2026-11-16

**Outcome:** all 14 phases are `done`. That is the 5 phases planned at
scoping (01, 02a, 02b, 03 and 02c, added 2026-11-14), five pre-close bug
fixes — 04 (#6), 05 (#2), 06 (#8), 07 (#9) and 08 (#5) — and four PE-driven
status-bar and chat-panel phases added at close: 09 (the words), 10 (the
chat panel's borders and the `user@host:sN` approval title), 11 (the model
word and clip) and 12 (the bar's final form). Issues #2, #3, #5, #6, #8 and
#9 are closed. #4 (cron time zones) and #7 (a close action in `top`) stay
open and are not in M37. `cargo test` passed=3036 failed=0, against the
2894 at scoping.

| Phase | Verdict | Note |
|---|---|---|
| 01 daemon-alert-state | approved_after_1 | bug-01-1 (blocker): attach failed whenever an alert was firing |
| 02a chat-status-bar | escalated | two bounces, then a takeover; spec used `## Behaviour`, so no task list |
| 02b session-counters | approved_after_1 | first M37 phase with a seeded task list |
| 02c grants-segment | approved_after_1 | three spec errors were the architect's |
| 03 top-alerts-panel | approved_first_try | 430 turns for ~300 lines |
| 04 local-provider-pricing | approved_first_try | |
| 05 adopted-shell-close | escalated | takeover of the module wiring |
| 06 self-host-refusal | approved_first_try | |
| 07 scheduler-recovery | approved_after_1 | a 1000-turn budget run, then a network `hard_fail` |
| 08 away-digest | approved_after_1 | an `IdenticalToolCallRepetition` resume, then bug-08-1 |
| 09 status-bar-words | approved_after_1 | bug-09-1 (the test plan's tests missing), bug-09-2 (the severity word dropped) |
| 10 chat-panel-polish | escalated | bug-10-1; the re-dispatch read for 513 turns with no edit, then a takeover |
| 11 status-bar-model-word | approved_first_try | an executor-filed blocker on a render-test pin outside scope |
| 12 status-bar-final-form | escalated | bug-12-1: the snapshot tests pinned the bugs they were meant to catch; takeover |

Totals over 14 phases: 4 first try, 6 after one bounce, 4 escalated. Every
phase ran on GLM-5.3-Flash.

After phase 12, the PE adjusted the bar's glyphs and words directly in
review (no phase doc; recorded under decision 8): `🔄` turn, `🔧` tools,
`🔒` grants lead (the `gated` detail glyph removed), `💸` burn carrying the
spend's currency sign with no word, the counters ordered
`⏳ 🔧 👻 🐚`, and the model word `model` with no colon.

**Behaviour shipped:**
- A webhook alert below `severity_threshold` stays out of every chat
  session's history. It goes to the event log and to a daemon-wide registry
  of active alerts.
- The registry reaches clients as an `AlertState` frame on the presence
  connection and at attach, and through `Request::AlertList`.
- The chat status bar is glyph-led:
  - three dialects, set by `[chat] glyphs`;
  - zero-valued items are hidden;
  - the alert slot is coloured by severity;
  - session counters come from the daemon's `SessionCounters` push;
  - a grants segment.
- `top` has an Alerts panel and an alerts section in `--once`.
- A user-set `*_cost_per_mtok` rate now applies to ollama and lmstudio
  models.
- Any chat session can close an adopted shell. The maintenance tick sweeps
  files left by dead shell hosts.
- A `[hosts.*]` entry that names the daemon's own machine is warned about at
  startup and refused by every host-taking tool.
- A recurring job left `Running` by a restart skips to its next slot. A
  failed recurring run no longer ends the job.
- The first chat to attach after a gap gets one away digest: script runs per
  job and alerts fired per name.
- The status bar speaks a word with every glyph when there is room
  (`🌓 ctx 20%`, `🔄 turn 12`, `🦙 model deepseek-v4`). When it is narrow,
  the grants detail, the velocities and the alert name and age drop first,
  then every word at once, restoring what was dropped for them. The counters
  are one slot. Unknown pricing is a `?` currency sign (`💰?0.42 💸?0.2/h`).
  Whole-line snapshot tests pin the bar at fixed widths in all three
  dialects.
- The chat input box has the history panel's red border and rounded
  corners. The terminal approval panel is titled `user@host:sN`: the
  daemon picks the shell before the prompt and pins the run to it.
- Three fixes from the live check (`c5bdc1f9`): the resolve flash names the
  alert once; a `wait: false` command marks its dedicated shell in flight,
  so ⏳ counts it; a presence reconnect that fails five times announces
  `daemon down` once, and the bar reads `💀 daemon down` instead of
  `📡 reconnecting` for the rest of the outage.

**Live check — run 2026-11-16 on pinky** (throwaway `HOME=/tmp/de37-live`,
private tmux socket, a headless `script` client at 120 then 80 columns,
GLM-5.3-Flash over `[models.default]`, the webhook on 127.0.0.1:19437 with
`severity_threshold = "warning"`). Exit criteria:
- **info alert** — bar `⚠ info DiskSlowlyFilling 3s` in yellow; not in the
  session jsonl; `top --once` lists it; events `webhook_alert` then
  `webhook_discarded reason=below_threshold`. ✓
- **critical alert** — bar `🔥 crit RootDiskFull 4s` in red; `[Webhook
  Alert]` in the jsonl; `⚙ [FIRING] RootDiskFull — / at 98%` on screen. ✓
- **resolve** — a ten-second `🧯 resolved RootDiskFull` flash, then the
  slot is gone and `top --once` says `none`. ✓ — **after a fix**: the flash
  read `🧯 resolved RootDiskFull RootDiskFull` (the lead and the rank-5
  name part both wrote the name).
- **120 columns, through a turn and an approval** — `🧠 thinking 4s`, `✋
  approve?`, then `💤 idle · 🌒 ctx 1% 📈 +1.3k · 💰 $0.01 💸 $0.0/h · 🔄
  turn 2 🐢 21 tok/s · 🐚 1 shell · 🦙 model GLM-5.3    🔒 grants 4`: every
  item with its word. ✓
- **80 columns** — `💤 idle · 🌒1% 📈660 · 💰$0.01 💸$0.0/h · 🔄4 🐢11t/s ·
  🐚3 · 🦙 GLM-5.3     🔒4`, the compact form. ✓
- **`user@host:sN`** — `matt@pinky:new` for the session's first command,
  `matt@pinky:s0` for the second (the reused shell), `matt@172.16.50.70:new`
  with `→ host: zoidberg (configured)` over `[hosts.zoidberg]`, `daemon ·
  runs silently` for `wait: false`. The input box's border is bold red with
  rounded corners. ✓
- **counters** — 🐚 follows `open_shell` and the remote shell (1 → 6);
  zero-valued counters are absent. ✓ — **after a fix**: ⏳ never showed for
  `wait: false` — `finish_start` never marked the dedicated shell
  `in_flight`, and the counter reads `dedicated && in_flight`. Fixed, `⏳ 1
  bg` shows for the length of the command.
- **`glyphs = "symbols"`** — `daemoneye reload` reports `applied (1 key):
  chat.glyphs`, but the running chat keeps its dialect: the client reads
  `[chat]` at start. The next `chat --resume` read `○ idle · ◑ ctx 1% ·
  $0.02 · ↻ turn 8 · ⌨ 5 shells · model GLM-5.3    ⚡ grants 4`. ✓ with the
  exit criterion reworded to "on the next chat start after a reload".
- **`💀 daemon down`** — ✓ **after a fix**: after `daemoneye stop` the bar
  read `📡 reconnecting` for over two minutes; the presence task retried
  every 2 s and never said the daemon was down. Now the fifth failed retry
  prints `⚙ daemon down — still trying to reconnect` once and the bar
  reads `💀 daemon down` 8 s after the stop.
- Not exercised: the away digest.

The three fixes are `c5bdc1f9`; each new test was run red against its fix
reverted, and each fix was confirmed live on a rebuilt daemon.

**The approval deadlines raced — fixed before the release (`b791b890`, PE
decision 2026-11-16).** The client and the daemon both waited
`APPROVAL_TIMEOUT` (60 s) for the approval reply, and the daemon's wait
started first. A `y` typed without Enter sat until both fired; the client's
late reply then hit the closed ask connection — `✗ Broken pipe` and `💀
daemon down` with the daemon alive, until the next turn. Pre-M37 (F7). The
panel now reads against `CLIENT_APPROVAL_TIMEOUT`, five seconds inside the
daemon's wait. Live on the rebuilt binary: the panel closed at 55 s with
`⏱ approval timed out — the command was not run`, the daemon logged
`command denied`, and the bar returned to `💤 idle`. The panel reading a
line (`y` then Enter) is by design.

**Observed, not fixed — PE decision:**
- `daemoneye reload` lists `chat.glyphs` under "applied" though the daemon
  applies nothing for a `[chat]` key.
- A plain (unsandboxed) `wait: false` shell stays open after its completion
  message; only the sandbox path closes it. Pre-existing.

**Signed off 2026-11-16; released as 2.9.0.**

**Calibration, for PE decision (none folded yet):**
1. **A test fixture that does not model the real shape.**
   - Phase 05: the fence test held the lock, which skipped the branch under
     test.
   - Phase 07: a 10 s past slot left `skip_to_future` dead in the test.
   - Phase 01: the attach tests checked the server's frames but not the order
     the client's parser accepts.

   This makes three occurrences. A fold would be one drafting rule: for each
   test the spec names, the spec says which production branch it must reach,
   and the mutation check removes that branch.
2. **Hermeticity when a shared path gains I/O.** In phase 08, adding file
   writes to `handle_chat_attach` leaked from every existing attach test,
   not from the new ones. This is the first occurrence; noted only. The
   bug's "run the filtered module alone" criterion caught it, and is worth
   reusing.
3. **`## Spec` seeds the task list.** Folded at the time as the memory note
   and caught at review since phase 02b. No WORKFLOW change is proposed: the
   template already says `## Spec`.
4. **Executor turn burn.** Phase 03 took 430 turns, 07 hit the 1000-turn
   budget and 08 hit an identical-repetition stall. This is data for the
   GLM-5.3-Flash scorecard, not a WORKFLOW fold.
5. **Golden strings derived by the executor are not checked.** In phase 12
   the executor ran the code, pasted its output into six snapshot tests and
   reported complete; the tests pinned all three bugs. Phase 09's bug-09-1
   is a near relative (tests the plan named were never written, and the
   report said done). This makes two occurrences. A fold would be one
   drafting rule: a spec that asks for derived strings also names literal
   substrings each must and must not contain, as acceptance-criterion
   greps.
6. **Bounce re-dispatches to this model.** Phase 10's bounce read for 513
   turns with no edit; phase 12's bounce was taken over without a second
   dispatch. With M36's four occurrences, this is a trend. A fold would say:
   with GLM-5.3-Flash, a bounce whose fix is under ~100 lines goes to an
   architect takeover by default.
7. **Rendering a design proposal before recommending it.** The phase-12
   drop order, as recommended, discarded the alert name and velocities and
   then the words too at 160 columns with full grants; the takeover added a
   "restore what was dropped" rule. One occurrence, noted only.

**Held for PE decision, not in M37:** the active-alert registry is
in-memory, so a daemon restart blanks the bar and `top` for up to one
Alertmanager `repeat_interval`. A startup replay of the event log's last 6 h
of `webhook_alert` / `webhook_resolved` records would close it.
