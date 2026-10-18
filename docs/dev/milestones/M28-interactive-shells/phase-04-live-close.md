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

### Update — 2026-10-18 (rehearsal 1, architect)

Harness as in Pre-flight. `~/.claude`, `~/.claude.json` and `~/.ssh` are
symlinked into `$R`: shell-hosts inherit `HOME=$R`, and without the links
`claude` lands on its login screen and ssh has no keys. The raw notes are
in `/tmp/e2e-m28-04.txt`. Stopped after B9: F2 and F5 make every GLM-driven
block unreliable.

| Block | Result |
|---|---|
| B1 see | **pass**: `less` launched with `interactive: true`; header `running less (TUI)`, alt screen, 40×120 (03b's default); status line `[SEL: /tmp/de28-live/lessfile.txt]` |
| B2 launch | **partial**: `[Interactive session started] \`claude\` holds shell s1`, and `list_shells` shows `[running claude (TUI)]`, **but F1** |
| B3 never poison | **pass**: `echo hi` ran in a fresh shell; `shell: "s1"` refused with "shell s1 is running claude (TUI) …" (**F3**: wording) |
| B4 type and wait | not run (blocked: F2 kept the trust dialog open) |
| B5 keys | **fail → F2** |
| B6 never settles | `top` settled (**F4**); `top -d 0.2`: **pass**, `[did not settle within 5.0 s — … use wait_for …]` with the screen |
| B7 geometry | **pass**: `open_shell(cols: 160)` → "40 rows × 160 columns"; `stty size` → `40 160` |
| B8 ssh upgrade | not run |
| B9 host reuse | **half**: the first call opened `s4 host:zoidberg`; the second was lost to F5 |
| B10 ghost | not run |

**Findings.**
- **F1 (major), launch settles before the program draws.** 03d's launch
  settle (500 ms, two equal reads) passed on the bare echo of `claude`,
  before Node started. The returned screen held only
  `matt@pinky:~$ claude`, which defeats the point of returning the screen.
  A re-read with `settle_ms: 1500` showed Claude Code's trust dialog.
  `sleep` in the unit tests draws nothing, so they cannot see this. Fix
  constraint: the launch must not call a screen settled while it still
  shows only what was there before the program started.
- **F2 (blocker), `send_input` `keys` cannot be sent.** `defs.rs` declares
  `keys` as `ParamTy::Str` (there is no array type). GLM follows the schema
  and sends `"Down"` or the JSON-encoded string `"[\"Down\",\"Enter\"]"`,
  which do not deserialize into `Vec<String>`. All 8+ calls were rejected.
  (GLM's claim that it also sent a real array is false; `daemon.log` shows
  only strings.) 02e's dispatch test fed a real array, so it passed.
  Precedent: `load_tools` `groups` accepts a real array or a JSON-encoded
  string of one.
- **F3 (minor), wrong wrapper on the busy refusal.** The refusal is
  wrapped in the generic run-error text ("don't assume failure / don't
  retry: double-apply risk"), which contradicts a refusal made before
  anything was written.
- **F4 (minor, docs), `top` does settle.** `top` redraws every 3 s, so a
  500 ms settle passes between redraws. The `settle_ms` description, the
  `sre.toml` example and design D7 all name `top` as a screen that never
  settles. Use a fast redraw (`top -d 0.2`, `watch -n 0.1`) as the example.
- **F5 (major; predates M28 but disruptive), GLM's `background` is
  rejected, and the error never says so.** Every rejected
  `run_terminal_command` (16 in the run) carried `"background":"false"`, a
  string, including calls GLM insisted were clean. Twice, chat prose leaked
  into its value. GLM's tool-call output injects it whatever the model
  intends. The invalid-arguments error lists only the declared parameters
  and never names `background`, so the model cannot self-correct, and each
  storm burns 6–9 retries. When told, its calls succeeded until the
  injection came back. Fix constraint: a string boolean for
  `background`/`wait`/`interactive` must parse, and an invalid-arguments
  error must name every offending field, declared or not.

**Carried decision 1, evidence.** B1 and B2's approval panels showed only
`$ less …` and `$ claude`, with no sign that the call was an interactive
launch.

**Next.** Fix phases before rehearsal 2: **04a** argument tolerance (F2,
F5); **04b** launch settle and wording (F1, F3, F4). Then rehearsal 2 runs
all of B1–B10.

### Update — 2026-10-18 (rehearsal 2, architect, after 04a + 04b)

The release was rebuilt with 04a and 04b and the runtime state reset. B2
now launches `claude` from `/home/matt/src/daemoneye`, a folder already
trusted, so no new trust entry is written into the PE's `~/.claude.json`.
The raw notes are in `/tmp/e2e-m28-04.txt` (section R2).

| Block | Result |
|---|---|
| B1 see | **pass** |
| B2 launch | **pass, F1 fixed**: `[waited 4.7 s: the screen settled]` and Claude Code's full UI on the launch result |
| B3 never poison | **pass, F3 fixed**: "…refused before anything was written. Nothing was written to that shell, so it is safe to act on the message above." |
| B4 type and wait | **pass**: the panel showed `foreground: running claude (TUI)` and a 5-row tail; `settle_ms: 1500` gave `[waited 3.1 s: the screen settled]` with the answer |
| B5 keys | **pass, F2 fixed**: `BTab` (3 bytes) flipped `⏵⏵ auto mode on` → `⏸ manual mode on`; a two-line `text` went as one bracketed paste (34 bytes) into the composer, unsubmitted; `S-Enter` sent 7 bytes and the result said it went out as CSI u; Claude Code added a composer line |
| B6 never settles | **pass**: `top -d 0.2` → `[did not settle within 5.0 s …]` |
| B7 geometry | **pass**: 160 columns, `stty size` → `40 160` |
| B8 ssh upgrade | **pass** after F6's workaround: `ssh zoidberg` auto-launched (`running ssh (TUI)`); `hostname` → `zoidberg`; `exit` → the shell is back at `shell prompt` |
| B9 host reuse | **pass**: two `host: "zoidberg"` calls with no `shell` both reused `s4` (still 5 shells, at the cap) |
| B10 ghost guard | **cannot be run live → F8** |

No invalid-arguments line in `daemon.log` all run: F5's fix holds.

**New findings.**
- **F6 (major).** GLM sends an empty `keys` as the **string** `"[]"`
  alongside a `text` send. 04a's `lenient_keys` treats an empty
  array-string as a one-name list, `["[]"]`, so `send_input(text: "exit")`
  failed twice with `unknown key "[]"`. The 04a reviewer had accepted this
  as a "harmless nit", wrongly. Fix constraint: a string that parses as a
  JSON array, empty included, gives exactly that array.
- **F7 (minor).** The per-owner cap refusal is also pre-write, but it
  still carries "may still be running … do not assume it failed". It also
  names `shell s1`, a shell it never touched (the error path's owner-shell
  lookup). Same family as F3.
- **F8 (design).** Ghosts are background-only: `ghost.rs` maps every ghost
  `run_terminal_command` to `PendingCall::Background` and drops
  `wait`/`interactive`/`shell`. A never-exiting program fills the turn
  until the 300 s command limit, then its dedicated shell is gone. A ghost
  therefore never holds a live shell running a TUI it could type into, and
  D9's ghost rows (02f/02g) are defence in depth with unit-test coverage
  only. (GLM's ghost call was `{"background":true,"command":"top"}`
  despite the runbook saying `interactive: true`.)
- **F9 (minor).** A ghost's `interactive: true` is dropped silently rather
  than refused with a reason.
- **Observations, predate M28:** O3, the masker turns the echoed word
  `hostname` into `<SSH_HOST>`; O4, a successful run's result does not name
  its shell id.
- **Harness:** start the PE run's daemon from a clean terminal. Started
  from inside Claude Code, it inherited `CLAUDE_CODE_CHILD_SESSION`, which
  turned transcript saving off in the PTY's Claude Code.

**Next.** F6 must be fixed before the PE run; with GLM, `send_input` is
not dependable without it. Proposed **04c**: F6, plus the minors F7 and
F9. F8 is recorded for the retrospective as a design note. Then the PE run
(B1–B9, B11); B10 is noted as not live-reachable.
