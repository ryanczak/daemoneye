# M37 — Alerts out of history, onto the status bar and `top`

**Goal:** fix GitHub issue #3. A webhook alert below `[webhook]
severity_threshold` no longer enters any chat session's history. Every
firing alert shows on the chat status bar and in a `top` Alerts panel
instead. The chat status bar is redesigned to say the most important
things in the fewest columns.

**Status:** active

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
   | alerts | 🔥 ⚠ ℹ ❔ · 🧯 | ● red/amber/gray/dim · ✓ | `!!` `!` `i` `?` | the worst firing alert's severity, then the count of all firing; name and age when room; `🧯` flashes 10 s on a resolve |
   | context | 🌑 🌒 🌓 🌔 🌕 | ◔ ◑ ◕ ● | `ctx` | the window's fill, `%` after it (amber ≥ 75 %, red ≥ 90 %); `📈`/`↗` growth since last turn when room |
   | cost | 💸 · ⛽ | `$` · `$/h` | `$` | session spend; burn rate when room; `❔` suffix when any call had unknown pricing |
   | turn | ↻ | ↻ | `t` | turn count; `🐇`/`🐢` last-turn tokens per second when room (🐇 ≥ 30 t/s) |
   | background | ⏳ | ⧖ | `bg` | this session's running `wait: false` commands |
   | ghosts | 👻 | ◆ | `gh` | active ghost runs |
   | tools | 🛠 | ⚒ | `tl` | silent tool calls this session |
   | shells | 🐚 | ⌨ | `sh` | this session's open PTY shells |
   | model | ⟡ ◎ ♊ 🦙 | ⟡ ◎ ♊ | none | vendor mark (Anthropic, OpenAI, Gemini, local) and the short name, ≤ 12 columns |
   | session | 📌 · 📌✎ | ▣ | `@` | saved name; `✎` when dirty; nothing when unnamed |
   | grants | ⚡ | ⚡ | `A:` | today's approval hint, flush right |

   Gone from the bar: the unnamed session id and the uptime.

   **Fitting:** three tiers, dropped from the lowest up, never truncated
   mid-segment. Tier 3 first: the velocities (`📈`, `⛽`, `🐇`), the alert
   name and age, the session name, the model. Tier 2: shells, tools,
   ghosts, background, in that order. Tier 1 always: state, alerts,
   context, cost, turn. The grant hint goes after tier 2 and before tier
   1. Below 36 columns, separators become single spaces.

   ```
   120:  💤 · 🔥2 DiskFull 3m · 🌓 20% 📈3.1k · 💸0.42 ⛽1.9/h · ↻12 🐇38t/s · ⏳2 👻1 🛠7 🐚3 · ⟡ sonnet-5.5 · 📌 incident-42   ⚡cmds
    80:  💤 · 🔥2 3m · 🌓 20% · 💸0.42 · ↻12 · ⏳2 👻1 🛠7 🐚3 · ⟡ sonnet-5.5        ⚡cmds
    80:  🧠 ⏱12s · 🌓 20% 📈3.1k · 💸0.42 · ↻12 · 🛠7 🐚1 · ⟡ sonnet-5.5              ⚡cmds
    40:  💤 🔥2 🌓20% 💸0.42 ↻12 ⏳2
   down: 💀 daemon down · 💸0.42 · ↻12 · ⟡ sonnet-5.5
   ```
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
  `[chat] glyphs = "symbols"` swaps every glyph after a reload. With the
  daemon stopped, the bar shows `💀 daemon down`.
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

Drafted one at a time with `/rexymcp:architect next`. Expected count: 4 (phase 02 split into 02a/02b at scoping).

| #  | Phase | Scope | Status |
|----|-------|-------|--------|
| 01 | daemon-alert-state ([phase-01-daemon-alert-state.md](phase-01-daemon-alert-state.md)): the severity gate moves ahead of history injection and the chat-pane notice. A daemon-wide active-alert registry, the `AlertState` frame on the presence connection and at attach, and `Request::AlertList` | issue #3, decisions 2 and 4 | todo |
| 02a | chat-status-bar ([phase-02a-chat-status-bar.md](phase-02a-chat-status-bar.md)): the glyph-led bar of decision 3 on the client: the slot table and three dialects, `[chat] glyphs`, the tiers, the state slot shared with the OSC 7501 emitter, alert state from `AlertState` frames, the ticking ages; counters fed zero until 02b | decisions 3 and 4 | todo |
| 02b | session-counters: the daemon's `SessionCounters` push and `SessionInfo`'s per-turn output tokens; the client feeds them to the bar | decision 6 | todo |
| 03 | top-alerts-panel: `top` loads `Request::AlertList` into an Alerts panel | decision 5 | todo |

## Notes
