# Phase 07: live-close — DaemonEye 2.0 runs with no tmux, observes tmux read-only when present, and is tagged `v2.0.0`

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** 01–06e (all `done`)
**Estimated diff:** `Cargo.toml` / `Cargo.lock` version, `CHANGELOG.md`, this doc's Update Log, the milestone README row
**Tags:** language=rust, kind=verification, size=m, architect-run

## Goal

Run the milestone's exit criteria for real on pinky, then close 2.0. The
phase is **architect-run** with the PE present, the M14/M23 convention. It
drives the chat client in a real terminal, which no executor can do, and
it ends in an outward-facing act (a tag), which the PE approves.

Two runs share one harness. In run A tmux is **absent**. In run B a
private tmux server is **present**. A logging `tmux` shim sits first on
`PATH` in both runs, so every tmux call the daemon or the client makes is
recorded. Every `Command::new("tmux")` resolves through `PATH`, so the shim
sees all 31 call sites. That gives one honest signal for both exit
criteria, "no execution through tmux" and "read-only when present". **The
shim log contains no write verb**: `send-keys`, `new-window`,
`new-session`, `kill-window`, `kill-session`, `set-hook`, `pipe-pane`,
`respawn-pane`, `split-window`. `display-message` is allowed. It is the
optional alert overlay.

## Architecture references

- M27 README § Exit criteria. Each bullet maps to a block below.
- M23 phase-10e's harness: isolated `HOME`/`TMUX_TMPDIR`, a private emulator
  tmux on its own socket, the throwaway webhook on port 19418, and the
  local GLM as the throwaway daemon's model, so the run costs no API credit.

## Pre-flight

1. The PE is present. They watch the emulator read-only:
   `tmux -S /tmp/de07-emu.sock attach -r`.
2. The repo is clean. Port 19418 is free. `brain:8000` is reachable.
3. **Rehearse every block once** before the PE run, and fold any block fix
   into this doc. M23's live close needed three rehearsals.
4. **Harness rules from rehearsal 1.** The architect's Bash tool runs
   *inside the PE's tmux*, so `$TMUX` is set, and tmux prefers it over
   `TMUX_TMPDIR`. Every harness tmux call uses an explicit `-S <socket>`
   with `TMUX` unset. Run A's PATH holds **no tmux at all**: a symlink farm
   of `/usr/bin` minus `tmux`. The logging shim is for run B only. Run B's
   chat waits for an attached client (`chat.rs`), so attach a headless
   read-only client with `script -qfc "tmux -S … attach -r"`. Poll for the
   approval panel (`pprove for`) and answer within the 60 s
   `APPROVAL_TIMEOUT`. In a persistent PTY shell, test exit codes with
   `sh -c 'exit N'`, never a bare `exit N`, which kills the shell.

## Spec

`R=/tmp/de07-live`, `A=/tmp/e2e-m27-07.txt`, `EMU=/tmp/de07-emu.sock`,
`BIN=target/release/daemoneye`. Every block appends to `$A`. The emulator is
driven with the **real** tmux binary (`/usr/bin/tmux -S $EMU`), so the
emulator's own calls stay out of the shim log.

- **S0 — build and fixtures.** `cargo build --release`. `setup` into `$R`.
  Write the config: local GLM, `[daemon] tmux_session = ""`, `[webhook]`
  on 127.0.0.1:19418. Add a `live-ticks` ghost runbook, the M23 fixture.
  Add a script `exit3.sh` (`echo scheduled; exit 3`). Write the shim to
  `$R/shim/tmux`. It appends `"$*"` to `$R/tmux-calls.log`. With
  `SHIM_MODE=absent` it prints `no server running` and exits 1. With
  `SHIM_MODE=pass` it execs `/usr/bin/tmux "$@"`.
- **S1 — run A, no tmux.** Unset `TMUX`/`TMUX_PANE`, isolate `TMUX_TMPDIR`,
  set `PATH=$R/shim:$PATH`, `SHIM_MODE=absent`. Start the daemon.
  `daemoneye doctor`: the tmux row is not `FAIL`, and it exits 0. Start
  `chat` in the emulator, then:
  1. Ask it to run `echo hi; exit 3` and approve with `y`. The result
     reports exit code 3.
  2. Ask it to run `sleep 5; echo done` with `wait: false`. A
     `[Background Task Completed]` message with exit 0 is injected into the
     session for the model's next turn. It is not drawn on screen; that is
     by design.
  3. Schedule `exit3.sh` once, 20 s out, with `schedule_command`. Its
     result event carries exit code 3.
  4. Add one legacy entry to `schedules.json` with the shape
     `{"type":"Command","value":"df -h"}`, set to fire in 20 s (restart the
     daemon to load it). It does not fire, and a refusal reason is logged.
  5. POST a `LiveTicks` webhook alert. The alert appears in the chat on
     screen, and the ghost runs to `ghost_complete` in `events`.
  6. Check: `grep -ci 'tmux' daemon.log` shows no error or warn line naming
     tmux. Count the shim log's write verbs: 0.
- **S2 — catch-up.** Quit the chat. Wait 35 s. POST a second alert. Run
  `chat --resume <id>`. A `[Catch-up]` brief appears before the first
  token.
- **S3 — run B, tmux present.** `SHIM_MODE=pass`, `TMUX_TMPDIR=$R`. Start
  a private server, session `obs`, with two panes, one of them running
  `top`. Stop the run-A daemon. Start the daemon and `chat` **inside**
  `obs`. Record the window count before. Ask "what is running in my other
  pane?" The first-turn context in the session log carries the `top` pane's
  contents and `[PANE MAP] … (top, …)`. Check the context, not the model's
  answer: in rehearsal 1 the model misread a correct context. Run one command and approve it. Then check: the window
  count is unchanged, `show-hooks -g` lists no `daemoneye`, and the shim
  log's write verbs since S3 are 0. List the read verbs as evidence.
- **S3b — remote host (zoidberg).** The throwaway config also carries
  `[hosts.zoidberg] ssh = "zoidberg"`. Record `ssh zoidberg tmux ls` before.
  In the run-B chat, ask it to run `hostname; exit 4` with
  `host: "zoidberg"`, and approve with `y`. The result names `zoidberg` and
  reports exit code 4. Then check: `ssh zoidberg tmux ls` is unchanged, so
  the daemon created no remote session. The shim log's write verbs since
  S3 are still 0. The daemon's ControlMaster socket under
  `$R/.daemoneye/var/run/ssh/` was used.
- **S4 — gates.** `cargo test --test doc_truth`, all green. Run
  `daemoneye doctor` against the real `~/.daemoneye` for the record.
- **S5 — carried items, decide each.**
  - (a) Is README's **Remote script streaming** reachable? Trace
    `foreground::run_background`'s callers. Its only route to
    `scripts::remote_stream_cmd` is from `executor/mod.rs`, where a ghost
    fits no other route. If no chat route reaches it, the paragraph
    overclaims.
  - (b) README's audit-log sample shows `[ts] key=value` lines. The daemon
    writes a JSON `command` event (`log_command`: `session`, `mode`,
    `pane`, command, status, output excerpt).
  - (c) FR-1.2.2 (c)'s `[BACKGROUND PANE]` legacy-window classification.

  Anything confirmed stale goes into a docs phase **07a**, drafted per
  line in 06d's work-list form and dispatched before S6.
- **S6 — close (PE approval).** Bump `Cargo.toml` to `2.0.0`, and update
  `Cargo.lock` by building. Add a `CHANGELOG.md` 2.0.0 entry from the
  milestone README's goal and exit criteria. Run all four gates. Commit
  with the `-c` identity. Create an annotated tag `v2.0.0`. **Push only on
  the PE's explicit go-ahead.**
- **S7 — clean up.** Stop the throwaway daemon, kill the emulator and the
  private server, and `rm -rf $R`. No `daemoneye` process is left from `$R`.

## Acceptance criteria

- [ ] Run A: exit code 3 reported. The `wait: false` completion arrives. The
      scheduled script reports exit 3. The legacy `Command` is refused with a
      logged reason. The alert shows on screen. The ghost completes. No tmux
      error appears in `daemon.log`. The shim log has **0** write verbs.
- [ ] Catch-up brief shown after `chat --resume` following ≥ 30 s away.
- [ ] Run B: the context carries the `top` pane and `[PANE MAP]`. The
      window count is unchanged. No daemoneye hook is set. The shim log has
      **0** write verbs.
- [ ] S3b: the remote command reports `zoidberg` and exit code 4.
      zoidberg's `tmux ls` is unchanged. The shim log still has **0** write
      verbs.
- [ ] `doctor` exits 0 with tmux absent. `doc_truth` is all green.
- [ ] S5's three items are each decided in the Update Log. Any stale one is
      fixed by `07a` before S6.
- [ ] `Cargo.toml` reads `version = "2.0.0"`. `git tag -l v2.0.0` prints
      `v2.0.0`. All four gates are green at the tagged commit.
- [ ] The Update Log carries `$A` verbatim with `PASTE MATCH: yes`.

## Authorizations

- [x] Architect may edit `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, and
      this milestone's docs. Creating the tag needs the PE in the room.
      Pushing needs the PE's explicit go-ahead.

## Out of scope

- Fixing a defect the live run finds. File it as a bug against the phase
  that owns it, or as a new phase. Never fix it inline during the run.
- `docs/design-reference.md`, `docs/ROADMAP.md`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-11 (rehearsal 1, architect)

The blocks were rehearsed end to end on pinky against `47d3c5ea`. The harness
fixes are folded into Pre-flight 4 and the S1/S3 wording above. **What
passed:**
- Run A, no tmux on PATH: `doctor` reports `tmux: not installed (optional)`
  and exits 0. The waited command reported exit 3 in PTY shell `s0`.
  `wait: false` ran on dedicated shell `s1`, and `[Background Task
  Completed] … exit code 0` was injected. The legacy `ActionOn::Command` job
  was refused with a logged reason. A webhook alert went through analysis to
  a ghost, which completed in 2 turns. A fresh chat showed
  `⚙ [FIRING] LiveTicks …` on screen.
- Run B, private tmux: the context carried the `top` pane and `[PANE MAP]`.
  The window count was unchanged. No daemoneye hooks were set.
- S3b: `host: "zoidberg"` ran through PTY shell `s2` over a ControlMaster,
  printed `zoidberg` and exit 4. zoidberg's `tmux ls` was unchanged (no
  server).
- S4: `doc_truth` passed 16 of 16.

**What failed. The tag is blocked.** Rehearsal findings follow, verbatim
from the run notes:

- F1 (minor): `exit 3` kills the PTY shell. The error then reads "command exceeded the 45s limit … socket closed", but the close was immediate, not a timeout. Misleading message.
- F2 (major): the model (GLM) sends `{"background": false|true}` to run_terminal_command. The dispatcher silently ignores the unknown arg, so `wait` defaults to true. Four wasted calls before the model found `wait`. Unknown args should be refused with an error naming `wait`.
- F3 (blocker): the scheduler loop (`src/daemon/mod.rs`, ~line 616) `continue`s while the startup tmux session name `sn` is empty, and `sn` never changes. With no tmux, no scheduled job ever fires. This breaks the exit criteria "Runs without tmux" and "Scheduled scripts run in PTY shells".
- obs: the audit record is JSON: `{"cmd","event":"command","exit","mode":"shell","out","pane":"s0",…}`. That is the ground truth for README's sample (S5b).
- obs: daemon.log still says "foreground command approved" / "background command approved". `JobStatus::Failed` doc says "the tmux window `de-<id>` is left open" (stale comment).
- obs: `[Background Task Completed]` is injected into the session for the model's next turn, not shown on screen. Change the criterion wording; the behaviour is by design.
- F4 (major): after the daemon restarts, a running `daemoneye chat` stays up with no notice. It never re-establishes its presence connection, so it silently gets no alerts. The first S1f alert never reached it. A fresh chat got the second alert at once.
- fixture: the ghost runbook used `exit 0`, which hit F1 (the shell died; "300s limit" message). Use `echo ghost-ran`.
- F4 mechanism: `HeldPresence::park` (src/cli/commands/chat_presence.rs) breaks out of its read loop on EOF. It never reconnects and never tells the user.
- F5 (blocker): there is no catch-up brief after `chat --resume`. In the live run the session 7888bd89 presence detached at 22:13:21 (last=true). A `[Webhook Alert]` was injected into the session file while away. The attach came at 22:14:49 (88 s away). No `⚙ [Catch-up]` appeared on screen or in 2000 lines of scrollback. The client renders a brief at chat.rs:175, so the daemon side (`attach_brief_for`) most likely returned None. Candidates: the in-memory entry was evicted or replaced between detach and attach, or the injected alert landed outside `entry.messages`. This breaks the exit criterion "Catch-up on reconnect".
- F6 (major): `daemoneye chat` inside tmux runs `tmux set -g focus-events on` (src/cli/commands/chat.rs ~146). That is a global option write on the user's server, against the "read-only observer" exit criterion. It was caught by the shim in run B.
- HARNESS GOTCHA: the architect's Bash tool runs inside the PE's real tmux, so `$TMUX` is set. tmux prefers `$TMUX` over `TMUX_TMPDIR`. Rehearsal 1 created the `obs` session in the PE's real server (killed afterwards; the PE's session `0` was untouched). Every tmux invocation in the harness must use an explicit `-S <socket>` with `TMUX` unset.
- F7 (minor): after the daemon's 60 s APPROVAL_TIMEOUT expires, the chat client still shows the approval panel. The next typed line is sent as a redirect answer on the dead connection and fails with '✗ Broken pipe (os error 32)'. The panel should close and say the approval timed out.
- S5a: remote script streaming is real, but only for ghost shells running a daemon-host script on their remote host (foreground.rs ~243). README's paragraph describes a foreground command sent to an SSH'd pane, which no longer exists. Goes to 07a.
- S5b: the audit-log ground truth is JSON, `{"cmd","event":"command","exit","mode":"shell","out","pane":"s0","session","status","ts","pid"}`. Goes to 07a.
- S5c: FR-1.2.2 (c) is accurate as written (legacy de-* filter and classification still exist). No change.
- obs: inside tmux, the other pane is labelled `[ACTIVE PANE %1]` while the user's selected pane is %0. Labelling quirk, minor.
- obs: assets/memory/session/pane-referencing-convention.md still says "if the target pane for a foreground command is ambiguous, ask first". This is stale foreground-pane guidance in a seeded memory.


**Harness incident.** The first run-B attempt created session `obs` in
the PE's real tmux server, because of `$TMUX`; see Pre-flight 4. It was
killed after its creation time was checked. The PE's session `0` was
untouched. The chat client's `set -g focus-events on` (F6) also reached
the real server. The PE's own `daemoneye chat` sets the same option on
every start, so it was left as is.

**Next.** F3 and F5 break exit criteria. F6 breaks "read-only". F2 and F4
are major. These need code phases before the PE run. S5's (a) and (b) go
to 07a. The PE run and S6 wait for all of them.
