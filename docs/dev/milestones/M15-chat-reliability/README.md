# M15 — Chat Reliability & Dialog UX

**Goal:** Fix three user-visible chat defects (read_pane grep pollution, border
corruption on resize/window-switch, false sudo password prompts) and rebuild the
two interactive prompts (command approval, sudo credential) as themed in-viewport
ratatui panels.

**Status:** done (closed 2026-08-16 — live sweep + retrospective below)

**Depends on:** M14 (live-verification practices carry forward)

**Exit criteria:**
- `read_pane` without a `grep` argument returns unfiltered pane output; the
  ToolStarted summary never shows `grep="null"` — verified live through a chat
  session (session-JSONL `tool_calls`/`tool_results` as evidence anchor).
- Switching tmux windows and resizing the chat pane during and between turns
  leaves no corrupted borders in chat history scrollback — verified live with a
  scripted resize/switch sequence.
- A foreground `sudo` command whose credentials are already cached for the
  user's pane tty runs without DaemonEye prompting for a password; an uncached
  one still prompts — both cases verified live.
- The command-approval prompt renders as a themed multicolor bordered panel
  inside the inline viewport (Y/A/N + redirect preserved), on the daemoneye
  palette; no regression to the approval round-trip proven in M14.
- The sudo credential prompt renders as the same panel style with a masked
  input field; credential zeroization behavior unchanged.
- Neither the approval dialog nor the credential dialog contains a copy of
  the command it concerns — the only command copy on screen is the
  scrollback panel directly above (added with phase-06, PE direction
  2026-08-14).
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test` all green.

## Architecture references

- `CLAUDE.md` § Request/Response lifecycle (approval flow, `ToolCallPrompt` /
  `CredentialPrompt` IPC)
- `src/cli/render_ratatui.rs` — inline viewport, panel-commit machinery
- `src/cli/palette.rs` — the daemoneye theme
- `src/daemon/utils/sudo.rs` — sudo detection helpers

## Phases

| #  | Phase                        | Status |
|----|------------------------------|--------|
| 01 | read-pane-grep-null ([phase-01-read-pane-grep-null.md](phase-01-read-pane-grep-null.md)) | done (escalated) |
| 02 | sudo-cached-detection ([phase-02-sudo-cached-detection.md](phase-02-sudo-cached-detection.md)) | done |
| 03 | resize-border-corruption ([phase-03-resize-border-corruption.md](phase-03-resize-border-corruption.md)) | done |
| 04 | approval-panel ([phase-04-approval-panel.md](phase-04-approval-panel.md)) | done |
| 05 | sudo-credential-panel ([phase-05-sudo-credential-panel.md](phase-05-sudo-credential-panel.md)) | done |
| 06 | dedup-approval-dialogs ([phase-06-dedup-approval-dialogs.md](phase-06-dedup-approval-dialogs.md)) | done |

Phase docs are drafted one at a time via `/rexymcp:architect next`; rows gain
links as docs land.

## Notes

### Scoping findings (2026-08-14)

- **Phase 01 lead:** `PendingCall::to_tool_call()` (`src/ai/types/pending.rs:497`)
  serializes `"grep": null` (JSON null) into the tool-call history echoed back to
  the model each turn. A model imitating that field as the *string* `"null"` on
  subsequent calls produces exactly the reported symptom (output only for lines
  containing "null"). Must be reproduced live before fixing — the fix is likely
  to omit `None` fields from the serialized arguments (and audit the other
  `to_tool_call` arms, e.g. `find_in_panes` `"scope": null`, `read_file`, which
  share the shape).
- **Phase 02 lead:** `sudo_credentials_cached()` (`src/daemon/utils/sudo.rs:47`)
  runs `sudo -n true` **as the daemon process**. With sudo's default per-tty
  timestamp (`timestamp_type=tty`), credentials cached in the user's pane tty
  are invisible to the daemon's check, so it prompts when sudo would not.
  Foreground path must run the check in the context of the target pane's tty
  (or equivalent); verify which of foreground/background paths consult the
  check at all (`src/daemon/executor/foreground.rs:1183` is on the background
  branch).
- **Phase 03:** chat is a ratatui **inline viewport**; scrollback panels are
  committed once and re-anchoring on resize is hand-rolled
  (`render_ratatui.rs:230` notes `Terminal::resize` cannot be used). M13
  documented the tmux width-flip scrollback-ghost mechanism in WORKFLOW.md —
  read that before speccing; the cosmetic width-flip ghosts were previously
  judged not milestone-shaped, so pin exactly which corruption this phase owns.
- **Phases 04/05 design decision (user, 2026-08-14):** in-viewport panel —
  expand the inline viewport while a prompt is active and render a themed
  multicolor bordered panel (Y/A/N choices; masked password field for sudo).
  No alternate screen, no floating overlay. 05 reuses 04's panel widget, so
  04 → 05 is a hard dependency.
- Phases 01–03 are independent bugfixes; 04/05 are UX. Order bugs first so the
  live-verification of 04/05 isn't confounded by known defects.

### M15 retrospective (closed 2026-08-16)

**Verdicts:** 6 phases — phase-01 escalated (2× Nemotron NoProgressStall →
architect takeover; executor switched to Qwen3.8-27B-FP8), phases 02–06 all
approved_first_try on Qwen3.8 (five for five, 59–93 turns). Zero bounces,
one bug filed at close (below).

**Close live sweep (2026-08-16, evidence in session scratchpad `m15-close/`,
sessions `8ae14171`/`30f6529d`, binary at post-M15 HEAD `5d8cdcb`):**

- **read_pane grep (01) — PASS.** `read_pane` with no grep returned all 3
  seeded lines unfiltered; session-JSONL tool_calls show
  `{"pane_id":"%82"}` — no `grep` key at all, zero `grep:null` matches.
- **sudo cached/uncached (02) — uncached half PASS; cached half not
  exercisable by the sweep.** Uncached tty → credential panel appeared as
  required. The cached case needs the user's password to establish a sudo
  timestamp; PE should spot-check once interactively (run `sudo -v` in a
  pane, then have the AI run a sudo command targeting that pane).
- **resize/border (03) — PASS on the owned corruption class.** 10 width
  flips + 2 window switches during and between streaming turns: zero
  unclosed/over-wide border rows; all `┌`/`└` pairs width-matched in a
  460-line scrollback trace. The phase's documented accepted residual
  (full old-width live-region blocks when a flip lands without a reanchor)
  was observed 4× under the rapid-flip stress — cosmetic, complete boxes.
  Note post-M15 commit `9e9c680` (repaint-from-history) further reworked
  this area; the sweep therefore certifies the shipped HEAD behavior.
- **approval panel (04) + dedup (06) — PASS.** Themed in-viewport bordered
  panel, `[Y]es [A]pprove for sudo session [N]o or type to redirect`,
  ANSI-colored; the command text appears only in the scrollback panel
  above, not inside the dialog. Non-sudo commands auto-approved via
  `[approvals] commands = true` (configured intent, not a regression).
- **credential panel (05) + dedup (06) — render PASS, one defect found.**
  Panel renders with masked input and no command suffix. **bug-05-1
  (major, open):** `[Esc] cancel` actually submits an empty password —
  three Escs burned all three sudo attempts. Root cause is a pre-existing
  protocol gap (`CredentialResponse` cannot express cancel) surfaced by
  the panel's new help text; filed for PE scheduling, does not invalidate
  the milestone's written exit criteria.
- **Gates:** `cargo fmt --all --check` and clippy `-D warnings` green at
  HEAD. `cargo test` green except `hooks_land_on_private_server` — a
  **post-M15 regression**, bisected to `90567c3` ("security: reject
  control chars in file paths/patterns", the parallel LLM-API-client work
  stream): test passes at M15's close commit `1d53673` and at `feef5ad`,
  fails from `90567c3` on. Reported to PE; not M15's defect.

**Calibration:** one data point, no new folds — phase-06's E2E block omitted
the PASTE MATCH self-check clause and the paste came back with two retyped
lines (repaired at review). The fold already exists in WORKFLOW.md § E2E;
this was an architect application miss when drafting, not a doc gap. Second
occurrence of omitting the clause should trigger a pre-dispatch checklist
item. Qwen3.8-27B-FP8 scorecard: 5/5 approved_first_try on mechanical and
UX phases with front-loaded specs.
