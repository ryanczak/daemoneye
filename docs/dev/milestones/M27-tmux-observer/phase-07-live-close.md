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
     `[Background Task Completed]` message with exit 0 arrives on screen.
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
  pane?" The answer names `top`, and the turn log's context carries
  `[PANE MAP]`. Run one command and approve it. Then check: the window
  count is unchanged, `show-hooks -g` lists no `daemoneye`, and the shim
  log's write verbs since S3 are 0. List the read verbs as evidence.
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
- [ ] Run B: the pane answer names `top`. `[PANE MAP]` is in the context. The
      window count is unchanged. No daemoneye hook is set. The shim log has
      **0** write verbs.
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
