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
| 01 | host-truth ([phase-01-host-truth.md](phase-01-host-truth.md)) — the host's `Screen` frame with `[SEL: …]` markers, the three `Status` foreground facts, the pure `Foreground` classifier (local and transport); no daemon behaviour change | review      |
| 01b | screen-tool — `read_shell_screen` (no waits yet); `last_foreground` and `in_flight` on `ShellRecord` | todo |
| 02 | safety — live probes in shell reuse and the explicit path, the busy refusal, the static flag removed, `send_input` `\r` + `keys` + `paste` + guard + panel + ghost refusal (split at drafting) | todo |
| 03 | launch-wait — `interactive: true`, the ssh auto-upgrade, screen waits, `[shells] rows`/`cols`; live close (split at drafting) | todo |

## Notes

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
