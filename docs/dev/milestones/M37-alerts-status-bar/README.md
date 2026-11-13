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
   alert goes to the event log and the status-bar/`top` alert state only:
   no history turn, no chat-pane notice, no popup, no analysis. At or above
   it, delivery is unchanged. The gate keeps failing open: an unrankable
   severity or threshold counts as at-or-above. `auto_analyze`'s own
   write-ups still go into history.
3. **Status bar, alert-first and terse.**
   ```
    ● DiskFull 3m · 41k/200k 20% · sonnet-5.5 · t12 · $0.42        ⚡cmds
    41k/200k 20% · sonnet-5.5 · t12 · $0.42                       ⚡cmds
    ✖ daemon down · sonnet-5.5 · t12
   ```
   - The alert segment leads only while an alert is firing.
   - The `●` is red for critical, amber for warning, gray otherwise.
   - A resolved alert clears its entry.
   - The session id and uptime leave the bar. Cost and the daemon-down
     flag, which the bar already carries but never shows, join it.
   - On a narrow pane, segments drop from the right.
4. **Many alerts: the worst, plus a count.** The bar shows the most severe
   alert (the newest of that severity), its age, and `+N` for the others,
   for example `● DiskFull 3m +2`.
5. **`top` gets an Alerts panel** (PE, 2026-11-12) listing every firing
   alert from the same daemon state.

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
- **The bar reads as in decision 3.** With no alert firing, it is the
  no-alert line. With the daemon stopped, it shows `✖ daemon down`.
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

Drafted one at a time with `/rexymcp:architect next`. Expected count: 3.

| #  | Phase | Scope | Status |
|----|-------|-------|--------|
| 01 | daemon-alert-state ([phase-01-daemon-alert-state.md](phase-01-daemon-alert-state.md)): the severity gate moves ahead of history injection and the chat-pane notice. A daemon-wide active-alert registry, the `AlertState` frame on the presence connection and at attach, and `Request::AlertList` | issue #3, decisions 2 and 4 | todo |
| 02 | chat-status-bar: the client keeps the alert state from `AlertState` frames, and `status_bar_line` becomes the terse alert-first layout, with the age ticking | decisions 3 and 4 | todo |
| 03 | top-alerts-panel: `top` loads `Request::AlertList` into an Alerts panel | decision 5 | todo |

## Notes
