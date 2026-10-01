# M23 — Client decoupling & shell UX

**Goal:** `daemoneye chat` runs in any terminal with no tmux server anywhere,
and the user can see, follow, attach to, pause, resume and replay the PTY
shells the daemon and its agents run: `/shells`, `/tail`, `/agents` with
attached mode, `/logs`, `/log`, `/hosts` and `/shell`.

**Status:** in-progress — scoped 2026-09-30 on live measurements (below);
**PE signed off on the breakdown 2026-09-30** with three decisions (§ Notes →
"PE decisions at sign-off"). Next: `/rexymcp:architect next` → 01a.

**Depends on:** M20 (the PTY shell substrate and its host protocol), M21 (hosts,
per-host shells, credential prompts), M22 (`list_shells` / `[SHELLS]`), M24
(runs, `/runs`, `/stop`, cooperative cancel). Blocks M26 (tmux removal):
tmux cannot be deleted while chat still needs it.

**Exit criteria:**

- **The design's exit, live:** from a terminal with **no tmux server
  running**, pick a running ghost from `/agents`, attach, pause it (`Ctrl-p`),
  resume it, detach (`Ctrl-d`), then `/log` replay of a finished shell.
  Architect-run per the M14 convention, with the PE at the keyboard. "No tmux
  server" is checked, not assumed: `tmux ls` under the client's and the
  daemon's `TMUX_TMPDIR` must report no server before, during and after the
  run.
- **Nothing starts tmux.** Launched outside tmux, neither `daemoneye daemon`
  nor `daemoneye chat` creates, attaches to or execs a tmux session. Inside
  tmux, today's behaviour is unchanged until M26 deletes it.
- **The design's IPC additions exist and are tested:** `ShellList`,
  `ShellTail { id, follow }` → `ShellChunk` / `ShellExited`, `ShellInput`,
  `ShellResize`, `ShellLogList`, `ShellLogRead`, with the 1 MiB payload cap
  and chunked tails.
- **Every attached keystroke is accountable:** typed input lands in the shell's
  `"i"` log and in `events.jsonl` as a user-authored input event, and the
  owning agent is told on its next turn. A password typed while attached is
  scrubbed, as 01b does for daemon-answered prompts.
- **`[HOSTS]` context block** renders configured hosts and ControlMaster
  state (M21 and M22 deferred it here).
- **M21's carry-forwards are closed** (rows 01a–01d).
- **42 tools, 28 core + 14 deferred, unchanged** — every M23 feature is client
  and IPC surface, not a model tool. `tests/doc_truth.rs` stays green.
- README.md documents every new slash command (PE rule: README is current at
  every 2.0 milestone close).
- All four gates green.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 "Chat client" — the slash-command
  table, attached mode and its three chords (PE decision), and the IPC
  additions. § 2.1 — the shell host protocol these relay to.
- `docs/design/transcript-view.md` — the M17 alt-screen viewer that `/tail`
  and `/log` extend.
- `docs/dev/milestones/M21-host-model/README.md` § Retrospective → "Carried
  forward" — rows 01a–01d.

## Phases

Proposed, pre-measurement per row — **each is re-derived at drafting and may
split on a measured seam**, as M20–M22 and M24 rows did. Order: M21's
carry-forwards first (two of them sit on the PTY run path everything here
builds on); then the no-tmux startup, because every later live check needs
it; then the IPC the UX consumes; then the UX, cheapest first; the live close
last.

| # | Phase | Status |
|---|---|---|
| 01a | pty-spawn-transient ([phase-01a-pty-spawn-transient.md](phase-01a-pty-spawn-transient.md)) — **re-diagnosed at drafting:** not a `TIOCSCTTY` race. portable-pty starts every child in `$HOME` when no cwd is set, and the tests delete temp `HOME`s, so a spawn's `chdir` hits `ENOENT` (reproduced deterministically with the exact 08e string). Fix: an explicit cwd, `$HOME` or `/`; the connect tests take the HOME guard; the test-only retry and the wrong comments go. Measured: the fix alone still failed 2 runs in 3; with the guard, 5 of 5 green. Production exposure is only a daemon whose `HOME` is not a directory **Done 2026-09-30** (`approved_first_try`) | done |
| 01b | prompt-wait-outside-deadline ([phase-01b-prompt-wait-outside-deadline.md](phase-01b-prompt-wait-outside-deadline.md)) — the time spent waiting on a prompt's answer is added back to the command's deadline (`read_to_marker`). **Measured:** with a 2 s timeout and an answer at 3 s the run failed "did not complete within the 2s limit" **after the password was sent**; with the fix it returned exit 0. An instantly answered prompt followed by a hang still timed out at 2.00 s. Also retires 09b's `SUDO_EDIT_TIMEOUT_SECS` workaround and carries 01a's comment residual (`PTY_TEST_LOCK`'s wrong-diagnosis clause) **Done 2026-09-30** (`approved_first_try`) | done |
| 01c | file-op-error-label ([phase-01c-file-op-error-label.md](phase-01c-file-op-error-label.md)) — a failed sudo edit gets its own error wording (file, host, what to do next) instead of `shell_run_error_text("edit_file", …)`. **Measured at drafting:** all three branches said "in shell edit_file", the exited branch advised a `shell` parameter `edit_file` lacks, and the give-up branch doubles its full stop (`…chat session.. The`) **for every caller**, `run_terminal_command` included, so the chain's trailing period is trimmed in the shared helper too. The spec's helper code and test assertions were dry-run and hold **Done 2026-10-01** (`approved_first_try`) | done |
| 01d | session-log-retention ([phase-01d-session-log-retention.md](phase-01d-session-log-retention.md)) — `[sessions] log_retention_days` (default 0) deletes an inactive session's `<id>.jsonl` and `<id>.meta.json`. **Measured at drafting:** the directory holds four file kinds; the memory index reads only `.archive.jsonl` and `.epochs.jsonl`, so those are never swept by this key. **Gotcha:** `log_retention_days` already exists under `[shells]`; the criteria pin whole-word counts against that baseline (dry-run: template 1→2, README 2→4) **Done 2026-10-01** (`approved_first_try`) | done |
| 02 | no-tmux-startup ([phase-02-no-tmux-startup.md](phase-02-no-tmux-startup.md)) — with `tmux_session = ""` (or `--session ""`), a daemon launched outside tmux creates/adopts no session, registers no tmux hooks and runs no background-window GC; inside tmux and for non-empty names, unchanged (PE: default stays `"daemoneye"`). **Measured at drafting:** a probe daemon with the branch alone started no server and served a full chat turn, but logged 4 hook `ERROR`s and a GC `WARN` that would recur every 60 s; the **whole** change dry-run in `isolation.rs` with `start_daemon("")` gave no `tmux-*` dir, 0 hook errors, 0 GC warnings, all isolation tests green **Done 2026-10-01** (`approved_first_try`, one stop-and-resume on evidence formatting) | done |
| 03a | shell-ipc-relay ([phase-03a-shell-ipc-relay.md](phase-03a-shell-ipc-relay.md)) — `Request::{ShellList, ShellInput, ShellResize}` + `Response::ShellList { shells: Vec<ShellInfo> }`, handlers in a new `server/shells.rs` over an injected registry; `ShellList` lists **every** shell (the caller is the user), with each shell's last recorded command. **Split at drafting:** the host streams only from `Subscribe` with no backlog, so a correct starting screen for `ShellTail` is its own design. **Measured:** a raw `Resize {30,100}` frame was acked and `status()` then reported 30×100 (was 24×80); `send_input` against an in-process host acks. No client UI **Bounced 2026-10-01** ([bug-03a-1](bugs/bug-03a-1.md): `ShellList` hardcodes three record flags) **Done 2026-10-01** (`approved_after_1`) | done |
| 03b | shell-screen-snapshot ([phase-03b-shell-screen-snapshot.md](phase-03b-shell-screen-snapshot.md)) — host side: `ShellRequest::SubscribeScreen` → `ShellResponse::Screen { bytes }` (vt100 `state_formatted`, alt-screen re-entered) then live `Chunk`s, atomic under the screen lock; the pump now feeds and sends under that lock. **Measured at drafting:** `state_formatted` round-trips text, colour, cursor and a following live chunk exactly (vt100 0.16.2); alt mode needs a `?1049h` prefix; the whole change dry-run green (5 tests 6/6, lib 2110, mutations A/B/C red 6/6); the send-before-feed race is **not** observable by test (12/12 green on the old order), so it is reviewed by reading **Done 2026-10-01** (`approved_first_try`) | done |
| 03c | shell-tail-relay — `Request::ShellTail { id, follow }` over the daemon socket: the host's `Screen` then live chunks as `Response::{ShellScreen, ShellChunk, ShellExited}` until the client disconnects or the shell exits; falls back to plain `Subscribe` when an adopted old host answers `malformed frame`; decides lag handling (the host drops `Lagged` chunks silently) | todo (not drafted) |
| 04 | shells-and-hosts — `/shells` (id, host, owner, state, last command, age) and `/hosts` (configured hosts, ControlMaster state, shells per host); the `[HOSTS]` context block. `/panes` stays until M26 | todo (not drafted) |
| 05 | tail — `/tail <sN>` (alias `/watch`): live tail in the M17 alt-screen viewer through the vt100 grid, `f` toggles follow, `/` searches, `Esc` returns. Read-only | todo (not drafted) |
| 06 | logs-and-replay — `ShellLogList` / `ShellLogRead`; `/logs [host] [--owner] [--since]` and `/log <id>` with marker navigation (`n` / `p`) and asciicast-timed replay (`space`) | todo (not drafted) |
| 07 | agents-and-attach — the `/agents` picker (alias `/attach`, and `/attach <sN>`) and attached mode: full-screen pass-through via `ShellInput` / `ShellResize`, the grid rendered locally, `Ctrl-d` detaches, `Ctrl-d Ctrl-d` sends EOF | todo (not drafted) |
| 08 | attach-chords — `Ctrl-p` pause/resume (`SIGSTOP` / `SIGCONT` to the foreground group; the owning run parks `Paused` and is told how long) and `Ctrl-c` cancel (cooperative run cancel + `SIGINT`, `Cancelled{stage: "user_attached"}`); attached input logged as user-authored and reported to the agent; scrubbed at a secret prompt | todo (not drafted) |
| 09 | user-shell — `/shell [host]` opens a user-owned shell (owner `User`) agents can see but drive only through approval; useful to pre-authenticate to a host | todo (not drafted) |
| 10 | live-close — the exit criterion end to end, with no tmux server. Architect-run; the phase exists so the obligation is a tracked `## Spec` task, not a close-out promise | todo (not drafted) |

## Notes

### Measured before scoping (2026-09-30, HEAD `f872adaa`)

Each fact was run, not read. Probes used a separate tmux server on its own
socket **only as the terminal emulator**, with the client's `TMUX` and
`TMUX_PANE` unset and `TMUX_TMPDIR` pointed at an empty directory, so the
client could reach no tmux server. (`script` alone is not enough: with no
real terminal on its far side, the ratatui renderer's cursor-position query
times out and the client exits before doing anything.) The throwaway daemon
ran in its own `HOME` with the webhook disabled, so its port could not collide
with the real one.

- **None of the seven new slash commands exists** (`/shells`, `/tail`,
  `/agents`, `/logs`, `/log`, `/hosts`, `/shell`), and there is **no
  shell-facing IPC**. `/runs`, `/run` and `/stop` exist (M24).
- **The host protocol is ready.** `ShellRequest` already carries `Subscribe`,
  `Input`, `Resize`, `Signal { INT | TERM | STOP | CONT }` and `Status`
  (`src/shell/proto.rs`). Row 03 is a relay, not a new protocol.
- **`chat` outside tmux bails by default.** `run_chat_inner`
  (`src/cli/commands/chat.rs:42`) execs `tmux attach-session` into the managed
  session when `TMUX_PANE` is unset; `daemon.tmux_session` defaults to
  `"daemoneye"` (`src/config/types.rs:366`), and with no such session the
  client prints "Managed tmux session 'daemoneye' does not exist yet" and
  exits.
- **With `tmux_session = ""`, the client works without tmux.** Banner, status
  bar, the opening greeting and a full tool-calling turn
  (`echo M23-PROBE $(tty)` → `M23-PROBE /dev/pts/10` in a PTY shell, output
  returned and reported) all completed with no tmux reachable from the client.
- **The daemon starts a tmux server when launched outside tmux**, even with
  `tmux_session = ""`: it runs `tmux new-session -d -s ''`
  (`src/daemon/mod.rs:626`) and logs "Created managed tmux session ''." A
  session literally named empty appeared on a fresh server. An empty name is
  not treated as "off".
- **Gotcha for every live check here:** the client drops input sent while it
  is streaming (the opening greeting included). A scripted turn must wait for
  idle, then confirm the text landed in the transcript before waiting on
  anything else.

### Calibration queue (for the M23 close)

- **Fold C — never trust the executor's self-reported model name — is at
  three occurrences** (M21 09a, M21 09b, M23 01b "Codex"). Propose it at the
  close: the review verdict's `Executor:` line comes from the server-authored
  completion entry or `rexymcp.toml`, never from the executor's own Update
  Log entries. *(Fourth occurrence at 01c: "Claude".)*
- **A counted `grep -cx 'PASTE MATCH'` criterion fixes the claimed-but-absent
  line.** At 01c, the first phase where it was a counted criterion, the line
  was printed first try. Candidate fold: every end-to-end spec carries it.
- **The capture instruction is ambiguous** (02: "paste the file" was read as
  "paste the block", so the paste check could never match and looped ~15
  times). From 03, specs say "paste the **contents of** `/tmp/e2e-….txt` —
  never the script". One occurrence; watch.
- **Claimed-but-absent `PASTE MATCH`** (M21 09a: the entry was missing; M23
  01b: the line was missing, and the reviewer ran the check). Two occurrences.
  Hold.

### PE decisions at sign-off (2026-09-30)

1. **`daemon.tmux_session` keeps its `"daemoneye"` default.** Only `""` means
   off. Users who launch outside tmux keep today's managed session until M26
   deletes it; row 02 makes `""` really mean off on both the client and the
   daemon side, and changes nothing inside tmux.
2. **The live close runs in the measured substitute:** a private tmux server
   on its own socket, used only as the terminal emulator, with the client's
   `TMUX` / `TMUX_PANE` unset and `TMUX_TMPDIR` pointed at an isolated
   directory that `tmux ls` reports empty before, during and after. A real
   alacritty window is not required.
3. **`/panes` stays until M26.** Row 04 adds `/shells` beside it and does not
   retire it.

*(The three questions below were settled by these decisions; kept for the
record.)*

### Open questions for the PE (to settle at sign-off)

1. **Default for `daemon.tmux_session`.** Keep `"daemoneye"` (users who launch
   outside tmux keep today's managed session until M26) and make only `""`
   mean off? Or change the default to off now, since nothing in 2.0 needs the
   managed session?
2. **Where the live close runs.** The design says a plain `alacritty` window;
   that needs the PE at the machine. The alternative is the measured
   substitute above: a private tmux server used only as the emulator, with
   `TMUX_TMPDIR` isolated and checked empty.
3. **`/panes`.** Keep it until M26 deletes tmux (proposed), or retire it in
   row 04 when `/shells` replaces it?
