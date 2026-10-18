# Phase 04: live-close — Claude Code runs inside a DaemonEye shell, and 2.1.0 ships

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** 01–03e (all `done`)
**Estimated diff:** this doc's Update Log, the milestone README, then the
release (`Cargo.toml` / `Cargo.lock`, `CHANGELOG.md`)
**Tags:** kind=verification, size=m, architect-run

## Goal

Run the milestone's exit criteria for real on pinky, the M14/M27 way. The
architect rehearses every block in a throwaway root. The PE then runs it
once while watching. Defects become findings `F1…` and are fixed in phases
`04a…` before the close. Unit tests cannot see these paths. M27's live
close found eleven defects that all passed unit tests.

## Pre-flight

1. **Harness (M27's rules, carried over).**
   - `R=/tmp/de28-live`, run as `HOME=$R`.
   - `BIN=target/release/daemoneye`.
   - The emulator is a private tmux on `EMU=/tmp/de28-emu.sock`. Drive it
     with `tmux -S $EMU` and `TMUX` unset (the architect's Bash runs inside
     the PE's tmux). The PE watches with `tmux -S $EMU attach -r`.
   - The chat's first input is `/approvals off`; send nothing until the
     greeting turn is idle. Match the approval panel on `pprove for`.
   - Test exit codes with `sh -c 'exit N'`, never a bare `exit N`.
   - S-last kills every process with `HOME=$R`.
2. **Model.** The throwaway daemon uses the local GLM (`brain:8000`), so
   the chat's own turns cost no API credit.
3. **Config in `$R`.** `[hosts.zoidberg]`; `[shells]` left at its
   defaults (40×120); a runbook `tui-ghost` whose agent allows
   `send_input`, without `allow_tui_input`; a second runbook
   `tui-ghost-ok` with `allow_tui_input: true`; the webhook on port 19418.
4. **Side effects the PE approves before the first rehearsal.**
   - **B2–B5 launch the real `claude`** inside a daemon PTY. It uses the
     PE's Claude Code login, and a typed prompt is a real (small) request.
   - **B8–B9 ssh to zoidberg.**
   - Nothing is pushed, and the tag is created only after sign-off.

## Blocks

Each block appends what it observed to `/tmp/e2e-m28-04.txt`. "The model"
means a chat prompt that asks for the call, approved at the panel.

| # | Block | Pass when |
|---|---|---|
| B1 | **See.** `less` on a file in a shell; `read_shell_screen` | `less`'s inverse status line comes back as `[SEL: …]`; the header says `running less (TUI)` |
| B2 | **Launch.** The model: `run_terminal_command("claude", interactive: true)` | The result starts `[Interactive session started]`, and the screen shows Claude Code's UI; `list_shells` shows that shell `[running claude (TUI)]` |
| B3 | **Never poison.** The model: `run_terminal_command("echo hi")`, then the same command with `shell: "<claude shell>"` | The first runs in a different shell; the second is refused naming `claude`, and Claude Code's screen is unchanged |
| B4 | **Type and wait.** The model: `send_input(text: "say hi in five words")`, then `read_shell_screen(settle_ms: 1500)`, then `scrollback: 50` | The answer is on the settled screen; scrollback returns the rows above |
| B5 | **Keys.** The model: `send_input(keys: ["BTab"])`; a two-line `text`; `keys: ["S-Enter"]` | The panel shows `foreground: running claude (TUI)` and a masked 5-row tail; `BTab` cycles Claude Code's mode; the two lines arrive as one paste, not two submits; the `S-Enter` result names CSI u, and the screen shows what Claude Code made of it |
| B6 | **A screen that never settles.** The model launches `top` (`interactive: true`), then `read_shell_screen(settle_ms: 500, timeout_secs: 5)` | The result arrives within ~5 s, starting `[did not settle within 5 s …`, and the screen is shown |
| B7 | **Geometry.** The model: `open_shell(cols: 160)`, then `run_terminal_command("stty size", shell: <it>)` | The open result names `40 rows × 160 columns`; `stty size` prints `40 160` |
| B8 | **ssh auto-upgrade.** The model: `run_terminal_command("ssh zoidberg")`; `send_input(text: "hostname")`; `send_input(text: "exit")`; then a bare `run_terminal_command("echo back")` | A launch, not a refusal; `zoidberg` on the screen; after `exit` the shell reads `shell prompt` again, and a following command may reuse it |
| B9 | **Host reuse.** The model: `run_terminal_command("hostname", host: "zoidberg")`, twice | The same shell id both times |
| B10 | **Ghost guard.** Webhook → `tui-ghost`, which tries `send_input` into a shell running `top` | Refused naming the shell, `running top (TUI)` and `allow_tui_input`; the same through `tui-ghost-ok` is allowed |
| B11 | **Attached mode.** The PE: `/attach` the Claude Code shell, type, detach | The keys reach Claude Code and the screen is intact after detach |

## Carried decisions (the PE decides at the run)

1. **D6's approval panel line.** Was it missed in B2? If so, decide
   whether to add it, knowing the panel text is the approval `cmd`, which
   feeds approval classification.
2. **03e's missing `(wait: false ignored: …)` prefix.** `sre.toml` states
   the rule. Decide whether a per-result line is still worth a plumbing
   pass.

## Close (after the PE run passes)

The retrospective in the README Notes; calibration folds proposed to the
PE; `NEXT.md` set to none; then the release per `REXYMCP.md` § Versioning
(`2.1.0`, the CHANGELOG entry, the four gates, the commit, the annotated
tag `v2.1.0`). **Push only on the PE's explicit go-ahead.**

## Update Log

<!-- entries appended below this line -->
