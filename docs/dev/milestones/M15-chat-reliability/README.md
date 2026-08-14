# M15 — Chat Reliability & Dialog UX

**Goal:** Fix three user-visible chat defects (read_pane grep pollution, border
corruption on resize/window-switch, false sudo password prompts) and rebuild the
two interactive prompts (command approval, sudo credential) as themed in-viewport
ratatui panels.

**Status:** planning

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
| 04 | approval-panel ([phase-04-approval-panel.md](phase-04-approval-panel.md)) | review      |
| 05 | sudo-credential-panel        | todo   |

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
