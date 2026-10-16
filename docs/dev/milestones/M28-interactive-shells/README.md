# M28 — Interactive shells

**Goal:** an interactive TUI — Claude Code is the reference case — runs
inside a DaemonEye PTY shell, and the model can see it, type into it
safely, wait on it, and never poison it with a command meant for a prompt.

**Status:** in-progress (scoped 2026-10-13)

**Release:** 2.1.0

**Depends on:** M20 (shell engine), M23 (attached mode), M27 (PTY-only
execution).

**Exit criteria:**

- **See.** `read_shell_screen` returns a shell's rendered screen, masked,
  with inverse-video runs marked `[SEL: …]`, optional scrollback, and a
  header naming the shell's `Foreground`.
- **Never poison.** With Claude Code running in the owner's lowest shell,
  a bare `run_terminal_command` lands in a fresh shell and `shell: "<id>"`
  naming the busy shell is refused before any byte is written. Two
  consecutive commands on a configured `[hosts.*]` host reuse one shell.
- **Type safely.** `send_input` sends `\r` for Enter, takes named `keys`
  (with CSI u for modifier combinations) and bracketed `paste`; its
  approval panel shows the foreground and a masked screen tail; a ghost
  without `allow_tui_input` is refused.
- **Launch and wait.** `run_terminal_command(interactive: true)` starts a
  TUI without waiting for it or signalling it; `read_shell_screen` with
  `wait_for` / `settle_ms` waits on the rendered screen and reports a
  screen that never settles as a result, not an error.
- **Static flag gone.** `ShellRecord.interactive` and `mark_interactive`
  no longer exist; busy-ness is observed (`last_foreground`) or known
  (`in_flight`).
- Live close on pinky per the M14 convention; `tests/doc_truth.rs` passes;
  `2.1.0` released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/design/interactive-shells.md` — the design; D1–D10 and the § 5
  matrix are what the phases implement.
- `docs/design/daemoneye-2.0.md` § 6 — the roadmap row.

## Phases

Drafted one at a time with `/rexymcp:architect next`. The design's three
phases are split further where one is more than a session.

| # | Phase | Status |
|---|---|---|
| 01 | host-truth ([phase-01-host-truth.md](phase-01-host-truth.md)) — the host's `Screen` frame with `[SEL: …]` markers, the three `Status` foreground facts, the pure `Foreground` classifier (local and transport); no daemon behaviour change | done (approved_first_try, 2026-10-13) |
| 01b | record-truth ([phase-01b-screen-tool.md](phase-01b-screen-tool.md)) — `last_foreground` and `in_flight` on `ShellRecord`, `classify_record`; the `unreachable!` carried from 01. *Re-scoped 2026-10-13 after two stalls: the tool moved to 01c* | done (approved_first_try, 2026-10-13) |
| 01c | screen-tool ([phase-01c-screen-tool.md](phase-01c-screen-tool.md)) — `read_shell_screen` (no waits yet), prompt asset, tool tables | done (escalated: architect takeover, 2026-10-14) |
| 02a | busy-shells ([phase-02a-busy-shells.md](phase-02a-busy-shells.md)) — a live foreground probe; reuse skips a busy shell; a named busy shell is refused before any write; the prompt digit rule narrowed to `%` | done (approved_first_try, 2026-10-14) |
| 02b | foreground-listed ([phase-02b-foreground-listed.md](phase-02b-foreground-listed.md)) — `list_shells` and `[SHELLS]` show the observed foreground instead of `[interactive]`; `[DEFAULT SHELL]` skips a shell last seen busy | done (approved_first_try, 2026-10-14) |
| 02c | flag-gone ([phase-02c-flag-gone.md](phase-02c-flag-gone.md)) — remove `ShellRecord.interactive`, `mark_interactive`, the reuse predicates and the IPC / `/shells` uses; the interactive-command path becomes an honest refusal | done (approved_first_try, 2026-10-14) |
| 02d | keys ([phase-02d-keys.md](phase-02d-keys.md)) — the pure key encoder (tmux names, xterm modifiers, CSI u) and paste rule in `src/shell/keys.rs`; `ScreenText` reports bracketed paste and application cursor; `send_input`'s Enter is `\r` | done (approved_first_try, 2026-10-14) |
| 02e | keys-tool ([phase-02e-keys-tool.md](phase-02e-keys-tool.md)) — `send_input` gains `keys` and `paste`, the panel and result name them, prompt and doc tables. *Split from 02d at drafting* | done (approved_first_try, 2026-10-14) |
| 02f | input-guard ([phase-02f-input-guard.md](phase-02f-input-guard.md)) — `send_input` probes the live foreground before the panel; the panel names it and shows a TUI's masked screen tail; a ghost may type only at a prompt | done (approved_first_try, 2026-10-14) |
| 02g | allow-tui-input ([phase-02g-allow-tui-input.md](phase-02g-allow-tui-input.md)) — the runbook `allow_tui_input` field through `GhostConfig` / `GhostPolicy`, lifting 02f's ghost refusal; docs. *Split from 02f at drafting* | done (approved_first_try, 2026-10-14) |
| 03a | screen-wait ([phase-03a-screen-wait.md](phase-03a-screen-wait.md)) — `read_shell_screen` gains `wait_for`, `settle_ms`, `timeout_secs`; quiescence on rendered text; a wait that runs out is a result | done (approved_after_1, 2026-10-16) |
| 03b | geometry ([phase-03b-geometry.md](phase-03b-geometry.md)) — `[shells] rows`/`cols` (40×120) through one `spawn_size()` at all 13 spawn sites | done (approved_first_try, 2026-10-16) |
| 03c | open-shell-size ([phase-03c-open-shell-size.md](phase-03c-open-shell-size.md)) — `open_shell(rows, cols)`; the result names the size. *Split from 03b at drafting* | review      |
| 03d | launch — `run_terminal_command(interactive: true)` (write, settle, return the screen); the ssh auto-upgrade replacing 02c's refusal | todo |
| — | live close (M14 convention) at milestone close: the design § 6 phase-02 and phase-03 live checks | todo |

## Notes

- **The interactive-command path was a stub (found drafting 02c).** For
  `ssh host` and friends, `run_terminal_command` wrote nothing to the
  shell but told the model the session was established; only the flag made
  the next command use a fresh shell. 02c turns it into an honest refusal
  pointing at `open_shell` + `send_input`; phase 03's `interactive: true`
  replaces the refusal.
- **Phase 02 split (2026-10-14).** Five phases (02b split again at drafting: the flag has four deciding files and five rendering ones, so the model-facing surfaces go first and the field removal second) of two or three files each,
  per the stall note below. Drafting 02a found that phase 01's digit rule
  rejects bash's default `bash-5.2$ ` prompt, which minimal hosts and sbx
  sandboxes show; 02a narrows it to `%`.
- **M28 stalls, so far (2026-10-14).** Four `NoProgressStall`s across
  phases 01, 01b and 01c, each traced to the spec: an impossible assertion
  (an idle interactive bash reads raw), an open-ended caller audit, a
  nine-file checklist, and in 01c a wrong fixture pointer plus count-pinned
  tests the spec did not name. Narrowing 01b to two files worked. 01c,
  already one-file-per-task, still stalled reading every file before the
  first write, so it was taken over. For phase 02: name every test that
  pins a count the phase moves, name the real fixture, and keep a phase to
  a handful of files.
- **Phase 01b re-split (2026-10-13).** Two dispatches stalled at 120
  read-only calls, the second with zero edits, after reading the nine files
  the tool checklist names. The record work stays in 01b; the tool is 01c
  with one file per task. Same class as M26 10b: breadth, not difficulty.
- **Phase 01 split (2026-10-13).** The design's phase 01 covers the host,
  the classifier, a new tool and two record fields. That is two sessions:
  01 stops at the shell layer, 01b adds the tool and the record fields.
- **Transport prompt rule refined at drafting.** D3 reads a last row ending
  in `$ # % >` as a prompt. Phase 01 adds one negative: the character must
  not follow a digit, so a progress line such as `Progress 50%` is not a
  prompt. The error direction stays "busy".
- **Name collision to keep straight.** `src/daemon/executor/foreground.rs`
  already exists (foreground command execution). The classifier is
  `src/shell/foreground.rs`, a different module.
