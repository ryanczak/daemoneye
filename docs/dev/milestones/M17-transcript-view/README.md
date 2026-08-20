# M17 — Transcript View

**Goal:** A `daemoneye chat` user can open the full conversation — including
every byte of elided tool output — in an alternate-screen viewer with `ctrl+o`,
scroll and search it, copy any block to a tmux buffer, and close it back to an
inline chat surface that is byte-identical to the one they left.

**Status:** done (closed 2026-08-20, PE sign-off)

**Depends on:** M16 — LLM Stream Robustness (all 8 phases done 2026-08-18;
**PE sign-off outstanding**, five live exit criteria unrun). M17 phases touch
`src/cli/` and `src/ipc.rs`; M16's outstanding live checks are daemon-stream
behaviours, so the two do not collide — but M16 should be closed before M17
dispatches.

**Exit criteria:**

- From the chat prompt in a real tmux pane, `ctrl+o` opens the transcript over
  the full pane and `esc` returns to the inline chat with the input box, status
  bar and prior scrollback intact — no border corruption, no duplicated or lost
  committed rows (live check; the M13 width-flip / M15 border-corruption blast
  radius). **Amended 2026-08-19 after phase-02's two bounces:** this check must
  exercise the alternate screen being left on the *normal* exit (`esc`), not
  only that the viewer opens. Phase-02 review mutation Mb showed that changing
  `let _guard = AltScreenGuard::new(…)` to `let _ = …` — which leaves the screen
  before the loop even runs — keeps all 10 headless tests green. Nothing below
  the live door covers the guard binding's lifetime.
- A `run_terminal_command` whose output exceeded the 10-line inline cap shows
  **every** captured line in the viewer, byte-for-byte equal to the
  `Response::ToolResult` payload (live check; evidence anchor = the session
  JSONL `tool_results` entry for the same `tool_call_id`).
- Copying a block writes it to a tmux buffer: `tmux show-buffer` after the copy
  emits the block text (live check).
- Resizing the tmux pane while the viewer is open reflows the transcript with no
  corruption, and closing afterwards still restores the inline surface (live
  check).
- Search finds a string present only in an expanded output body and navigates
  between matches.
- Starting a fresh client on a resumed session and pressing `ctrl+o` shows the
  prior turns rehydrated from `var/log/sessions/<id>.jsonl`.
- No secret masked in the inline panel appears unmasked in the viewer; the
  viewer reads no file under `var/log/panes/`.
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

Live checks are architect-run at milestone close (M14/M15 convention: through
the user's door, session JSONL as the evidence anchor).

## Architecture references

- `docs/design/transcript-view.md` — the design of record for this milestone.
- `CLAUDE.md` § "Request/Response lifecycle" — the IPC turn flow.
- `docs/architecture.md#12-orchestration-layer-srcdaemon` — where `ToolResult`
  originates.

## Design decisions on record

- **The inline `insert_before` streaming path is not touched.** It stays the
  primary surface, which is what keeps native tmux scroll, copy-mode and drag
  selection working. The viewer is modal and additive. An app-owned primary
  transcript was considered and rejected — rationale in the design doc's
  § "Non-goal".
- **The live transcript is captured client-side from the wire**, not read back
  from persisted logs. `events.jsonl` caps output at 200 chars
  (`src/daemon/utils/event_log.rs:291`); session-JSONL `tool_results` are
  truncated at `limits.tool_result_chars` (default 16 000,
  `src/config/types.rs:453`); `var/log/panes/*.log` exists for background jobs
  only and is written **unmasked**. The client already holds the exact masked
  bytes it rendered.
- **`Response::ToolResult` gains a `tool_call_id`.** Today it is a bare
  `String` (`src/ipc.rs:414`), so no rendered panel can be joined to a history
  record — which rehydration requires.
- **Clipboard is `tmux load-buffer -w -`.** The client is tmux-native by
  construction, so no OSC 52 / `set-clipboard` negotiation is needed. `-w`
  needs tmux ≥ 3.2; target is 3.7b. Block-level copy only; free-form drag
  selection is out of scope for M17.
- **Mouse is enabled only inside the viewer**, never on the inline surface —
  enabling mouse tracking on the main screen steals drag-select from the
  terminal.
- Executor model for this milestone: **DeepSeek V4 Flash 0731** (PE decision
  2026-08-16), carried from M16. Note the standing hazard recorded at M16:
  this model has acted destructively when a gate kept bouncing — the phase
  doc's acceptance criteria and the gate must agree before dispatch.

## Phases

Planned decomposition; drafted on demand via `/rexymcp:architect next`.
All seven phases are drafted.

| #  | Phase | Status |
|----|-------|--------|
| 01 | transcript-model ([phase-01-transcript-model.md](phase-01-transcript-model.md)) | done |
| 02 | viewer-shell ([phase-02-viewer-shell.md](phase-02-viewer-shell.md)) | done |
| 03 | expand-collapse ([phase-03-expand-collapse.md](phase-03-expand-collapse.md)) | done |
| 04 | search ([phase-04-search.md](phase-04-search.md)) | done |
| 05 | block-copy ([phase-05-block-copy.md](phase-05-block-copy.md)) | done |
| 06 | rehydration ([phase-06-rehydration.md](phase-06-rehydration.md)) | done        |
| 07 | viewer-mouse ([phase-07-viewer-mouse.md](phase-07-viewer-mouse.md)) | done        |

Ordering: 01 → 02 is a hard chain (nothing to render without the model).
03, 04, 05 and 07 each depend on 02 and are otherwise independent. 06 depends
on 01 only (the `tool_call_id` wire change) and may run in parallel with the
viewer work.

Phase intents:

- **01 transcript-model** — `Block` enum + bounded `Transcript` store owned by
  the chat loop; populated from `Response` events in `stream.rs` and from the
  user echo in `chat.rs`; `tool_call_id` added to `Response::ToolResult` and
  stamped daemon-side. No UI.
- **02 viewer-shell** — `Key::CtrlO` (`0x0f`) in `src/cli/input/tty.rs`;
  alternate-screen entry/exit; fullscreen ratatui render of the block list;
  keyboard scroll; resize reflow; `reanchor()` on exit. Read-only.
- **03 expand-collapse** — per-block collapsed state, full-output rendering,
  and the inline footer hint (`… N more lines · ctrl+o`).
- **04 search** — incremental find over block text with match navigation and
  highlight.
- **05 block-copy** — copy the focused block via `tmux load-buffer -w -`, with
  user-visible confirmation.
- **06 rehydration** — seed the transcript from `var/log/sessions/<id>.jsonl`
  on client start, joined by `tool_call_id`, with truncated entries labelled as
  such.
- **07 viewer-mouse** — SGR mouse parsing scoped to the viewer: wheel scroll and
  click-to-expand. Enabled on entry, disabled on exit.

## Notes

**Carried into phase-02 (from phase-01 review, 2026-08-18):**

- `Transcript::append_assistant` bypasses `evict()` on the coalescing path, so
  the byte cap is unenforced while one assistant turn streams (re-enforced on
  the next `push`; bounded by `max_tokens`). Close it when the viewer starts
  depending on the store's size guarantee.
- Two panels reach scrollback but not the transcript: the `ToolFinished` arm's
  `None` branch, and the end-of-turn flush of a started-but-never-finished tool
  (`src/cli/commands/stream.rs:712`). The viewer will show a gap where the
  inline surface shows a panel.

Freeform notes, dead ends and calibration observations accumulate here during
the milestone; the retrospective is written at close.

## Retrospective — closed 2026-08-20 (PE sign-off)

**Shipped:** an alternate-screen transcript viewer opened with `ctrl+o` from the
chat prompt **or mid-turn**, showing every block in full — including the tool
output the inline panel elides at 10 lines — with focus movement, collapse,
incremental search, block copy to a tmux buffer, rehydration from a saved
session, and wheel/click. The inline `insert_before` streaming path was never
touched, so native tmux scroll, copy-mode and drag-selection still work exactly
as before. That was the milestone's central design bet and it held.

| # | Phase | Verdict | Bugs |
|---|---|---|---|
| 01 | transcript-model | approved_first_try | — |
| 02 | viewer-shell | approved_after_3 | 3 |
| 03 | expand-collapse | approved_after_2 | 2 |
| 04 | search | approved_first_try | — |
| 05 | block-copy | approved_first_try | — |
| 06 | rehydration | approved_first_try | — |
| 07 | viewer-mouse | approved_first_try | — |

Five of seven phases were approved first try. All five bug docs are resolved.

### The headline: every bug was architect-side, and half were invisible to the gates

All five bugs were **spec gaps, not executor errors** — in each case the
executor implemented exactly what the phase doc said. Two of them could not have
been caught by any test in the suite:

- **`bug-phase-02-3`** — `ctrl+o` was swallowed mid-turn, at precisely the moment
  phase-03's `… N more lines · ctrl+o` footer advertised it. Phase-02 scoped
  mid-turn entry out (defensible alone) and phase-03 then advertised it
  unconditionally; a deliberate limitation plus an unconditional advertisement
  is a broken promise. Found by an architect live probe in an isolated tmux
  server, measuring `#{alternate_on}` before and after the keypress.
- **`bug-phase-03-1`** — the focused block rendered with `Modifier::UNDERLINED`
  on **every row**, so a long answer appeared as dozens of underlined lines.
  Found from a user screenshot, confirmed by `tmux capture-pane -e` showing
  `ESC[4m` on each row of the focused block.

Neither is observable from `cargo test`. The milestone deferred its live exit
criteria to close-out, and those checks paid for themselves twice.

### Two bounce shapes worth remembering

**Phase-02 inverted the same obligation twice.** Round 1 released the alternate
screen on the normal path but not the error path; round 2 moved the release into
a `Drop` guard and then disarmed it on the normal path, so `esc` never left the
screen at all. Round 3 deleted the disable path entirely — correct *by
construction* rather than by convention — which is why it stuck. The lesson is
in the fold applied at this close.

**Phase-03 shipped a correct fix with hollow guards.** Round 2 fixed the
underline and the mid-word wrapping; reviewer mutations that reverted both
wirings left 41/41 tests passing, because one guard tested the helper directly
instead of going through `layout_blocks`, and the other used a fixture both
wrappers split identically. Round 3 added behaviour-level guards, and the same
two mutations now fail the right test each.

### Calibration

- **Applied at close (PE-approved):** *a criterion for a cleanup obligation must
  assert the cleanup ran, and assert the count* — `docs/dev/WORKFLOW.md`, three
  occurrences.
- **Held at 2 occurrences, not applied:** *a criterion that names a function
  produces a test of that function; only a criterion that names an observable
  behaviour produces a test of the wiring.* Phase-03 round 1 (a wrapper compared
  with the function it delegates to) and phase-03 round 2 (the two hollow wrap
  guards). Per one-is-data / two-is-trend / three-is-fix, this waits for a third.

### Live checks: what was run, and what was not

**Run and passing** (isolated `tmux -L de-m17*` servers, plus the user's own
session): alt-screen entry and exit; `esc` returning to an intact inline
surface; full output visible in the viewer where the inline panel elided it;
`ctrl+o` mid-turn opening the viewer and the turn resuming afterwards; the
`tmux load-buffer -w -` round trip.

**Not run** — carried, and honestly unverified rather than assumed: resize
while the viewer is open (reflow), `y` copy invoked *through the viewer* rather
than through a shell, `/session load` rehydration against a live daemon, and
wheel/click with a physical mouse.

### For the next milestone

- The viewer prints raw markdown (`**bold**`, backticks) because phase-01 stores
  the raw token stream losslessly. Whether the viewer should re-render markdown
  is a **design decision**, deliberately left out of `bug-phase-03-1` rather
  than smuggled into a fix.
- Opening the viewer from an approval prompt (`[Y]es/[A]pprove/[N]o`) or the
  credential prompt is still unhandled — those use their own readers.
- The four unrun live checks above.

