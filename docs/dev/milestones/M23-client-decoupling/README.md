# M23 — Client decoupling & shell UX

**Goal:** `daemoneye chat` runs in any terminal with no tmux server anywhere,
and the user can see, follow, attach to, pause, resume and replay the PTY
shells the daemon and its agents run: `/shells`, `/tail`, `/agents` with
attached mode, `/logs`, `/log`, `/hosts` and `/shell`.

**Status:** **closed 2026-10-05, PE signed off** — all 32 rows done; the live
exit met on 10e's run 4. The two fold proposals were **not landed** at this
close (PE decision); they stay on the calibration queue (§ Notes →
"Retrospective", "Fold proposals").

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
| 03c | host-exits-with-its-shell ([phase-03c-host-exits-with-its-shell.md](phase-03c-host-exits-with-its-shell.md)) — **inserted at drafting (PE decision 2026-10-01)** after two pre-existing defects were measured live on a real detached host: (1) after `exit` the host stays alive holding its lock, so the reaper never marks the shell exited and a subscriber never sees EOF; (2) `close_shell`'s TERM goes to the foreground group and an idle interactive bash ignores it. Fix: a host serves only until `status()` reads `Exited`; a new `Close` request SIGHUPs the shell's pid (measured: ends bash and its foreground job); `close_shell` sends `Close`, falling back to TERM for old hosts. **Measured:** whole change dry-run green (5 tests 6/6, lib 2114 ×3, mutations A/B/C red 6/6); through the real binary a host now exits on `exit` and on `Close` (live check: exit code 124 → 0) **Done 2026-10-01** (`approved_first_try`; stale-comment residual carried to 03d) | done |
| 03d | shell-tail-relay ([phase-03d-shell-tail-relay.md](phase-03d-shell-tail-relay.md)) — `Request::ShellTail { id, follow }` → `Response::ShellScreen` (the host's `SubscribeScreen`, empty for an old host via a `Subscribe` fallback), then `ShellChunk`s, then `ShellExited` on host EOF or a `Status` poll reading `Exited`; ends when the client hangs up. Cancel-safe `Lines::next_line` in `select!`; the daemon keeps its host write half alive (the host ends a connection at write EOF). **Lag:** not detected; a viewer re-issues `ShellTail` (row 05 owns resync). Carries 03c's two stale comments. **Measured at drafting:** whole change dry-run green (6 tests 6/6, lib 2120 ×3, mutations A/B/C red 6/6); live through a **real daemon** adopting a **real** shell-host: `ShellScreen` → chunks carrying typed output → `ShellExited,EOF`, host exit 0 (current tree: `live_first=Error`) **Done 2026-10-02** (`approved_first_try`) | done |
| 04a | shells-command ([phase-04a-shells-command.md](phase-04a-shells-command.md)) — `/shells` in chat: one `ShellList`, a pure `render_shell_list` (id, host, owner — `this session` for the chat's own — live/`exited N ago` + interactive/dedicated, age, last command elided to 40 chars). Clears 03d's stray banner. **Split at drafting** from row 04. **Measured:** whole change dry-run green (5 tests, lib 2125, mutations A/B/C red — B re-measured after a first mutant hit an unreachable arm); `daemoneye chat` in a clientless private tmux hangs before drawing (M14 fact, re-measured), so the chat-UI live check stays with row 10 **Done 2026-10-02** (`approved_first_try`) | done |
| 04b | hosts-command ([phase-04b-hosts-command.md](phase-04b-hosts-command.md)) — `/hosts`: `local` first, then each `[hosts.<name>]` with ssh destination, `master:up/down` (reuses `shell::master_running`, `ssh -O check`, measured <10 ms with no network) and live-shell count (`?` without the daemon). **Plus two 03d defects found at the 04a review, required:** (1) relay — a client hang-up mid-stream returned `Err(broken pipe)` (3/8 full runs; fixed → 0/8; deterministic test with a writer that fails after its first frame, mutation red 6/6); (2) tests — assert-on-first-screen + echo-proxy markers, now polled with run-time `printf 'marker-%s'` markers (old loop 3/3 red with a delayed output-only marker, new loop green 3/3). **Measured:** whole change dry-run green, lib 2130 on 5/5 runs **Done 2026-10-02** (`approved_first_try`) | done |
| 04c | hosts-context-block ([phase-04c-hosts-context-block.md](phase-04c-hosts-context-block.md)) — the design's `[HOSTS]` ambient line (assigned to M22, never built: no `[HOSTS]` in `src/`), spliced before `[SHELLS]` in both chat prompt builders: `local (N shells) · <name> (ssh <dest>, N shells) …`, hosts sorted, live shells across owners, empty when no hosts are configured; no I/O per turn (no ControlMaster probe — `/hosts` owns that). **Measured:** whole change dry-run green (3 tests, lib 2133 ×3, mutations A–D red); one test builds **both** real prompts in ~0.02 s **Done 2026-10-02** (`approved_first_try`) | done |
| 05a | tail-model ([phase-05a-tail-model.md](phase-05a-tail-model.md)) — `/tail`'s testable core: `ShellScreen` gains `rows`/`cols` (the relay asks the host's `Status`; a client cannot draw the bytes without the size), `cli::tail::TailModel` (vt100 grid at the shell's size, 5 000-line scrollback; follow pins to newest, scroll-up pauses, bottom or `f` resumes; `ShellExited` recorded) and `render_tail` (cell-by-cell painter, 16 ANSI colours by name, 256/RGB through, bold/italic/underline/inverse, status line, clipped to top-left). **Measured:** vt100's `set_scrollback` already holds the view while output arrives, so the model only decides when to jump to 0; whole change dry-run green (7 tests, lib 2140 ×3, mutations A–F red); one `event_log` one-off, A/B 0/8 vs 0/8 **Done 2026-10-02** (`approved_first_try`) | done |
| 05b | tail-viewer ([phase-05b-tail-viewer.md](phase-05b-tail-viewer.md)) — `/tail <sN>` / `/watch` in chat: `ShellTail{follow}` opened first (an unknown id prints the daemon's error, no alt screen), then the alt-screen view on 05a's model; `f` follow, ↑↓/PgUp/PgDn/Home/End, `Esc`/`q`/`Ctrl-O` close; a closed stream marks `exited` and the view stays; help line. Dispatched in `chat.rs`'s loop (needs `stdin`/`sigwinch`); `AltScreenGuard` and `ipc_client` widened to `pub(crate)`. **Measured:** frames read with cancel-safe `Lines::next_line` (not `ipc_client::recv`'s `read_line`); the loop is generic over backend/frames/keys/resizes so a `TestBackend` + duplex + key-channel test drives the real loop; whole change dry-run green (5 tests, filter printed 5 on the dry run; lib 2145 ×3; mutations A–D red) **Done 2026-10-02** (`approved_first_try`) | done |
| 05c | tail-search ([phase-05c-tail-search.md](phase-05c-tail-search.md)) — `/` in the tail view: a status-line prompt (keys are text while typing), `Enter` finds the newest case-insensitive match at or above the view's bottom across scrollback **and** the live screen, `n` older / `N` newer, no match keeps the view, a jump pauses follow; `Ctrl-C` closes the view in both modes (05b nit). **Measured:** vt100 has no line index — every line is the top row at each scrollback offset plus the live rows (5 040 lines in 1.6 ms, scan 42 µs); a mutant that dropped query lowercasing survived an all-lowercase test query, so the test uses `"Error"` vs `ERROR`; clippy needs `saturating_sub` and a match guard. One file, given whole; dry-run green (filter printed 6, lib 2150 ×3, mutations A–E red) **Done 2026-10-02** (`approved_first_try`) | done |
| 06a | shell-log-list ([phase-06a-shell-log-list.md](phase-06a-shell-log-list.md)) — the command index records `owner` (`Owner::key()`) and `host` with the first command (older indexes still load, fields absent); `ShellLogList{since, host, owner, limit}` → `ShellLogList{logs, total}`, newest first, masked first command, an owner filter matches a whole kind; `/logs [host] [--owner chat\|ghost\|adopted] [--since 2h]`. **Measured:** a finished recording carried no owner or host anywhere (registry-only, live-only), so `--owner` needs the index to store them; there is no `sched` owner (scheduled ghosts are `Owner::Ghost`), so `--owner sched` is refused toward `--owner ghost`; `CommandRecord.started/ended` hold cast **byte offsets**, not times; 12 `MetaIndex` literals and 2 exhaustive `Response` matches break. Whole change dry-run green (filter printed 9, lib 2157 ×3, mutations A–F red); spec quotes the per-file dry-run diffs, verified to reproduce it **Done 2026-10-03** (`approved_first_try`) | done |
| 06b | shell-log-read ([phase-06b-shell-log-read.md](phase-06b-shell-log-read.md)) — `ShellLogRead{id}` → `ShellLog{id, rows, cols, events: [{t, data}], commands: [{event, command, exit_code}], truncated}`: every `"o"` event in order (input excluded, partial last line skipped), each command marked at the first output after its typed `"i"` line (else at its `first_byte`), oldest events dropped past 4 MiB with marks shifted; bare shell ids / unknown logs refused by name; no masking (the user's own terminal, as `/tail`). **Measured:** probed on three real recordings — every mark lands on the typed command's echo; `data` as `String` (cast text) not `Vec<u8>` (~4× JSON); the `"i"`→`"o"` mutant is unobservable and was dropped; every pinned mutant is clippy-clean. Dry-run green (filter printed 4, lib 2161 ×3, mutations A–D red) **Done 2026-10-03** (`approved_first_try`; spec-defect deviation: `#[rustfmt::skip]` on `ShellLogRead`, carried to 06c) | done |
| 06c | log-viewer ([phase-06c-log-viewer.md](phase-06c-log-viewer.md)) — `/log <id>`: one `ShellLogRead`, then the recording in 05b's alt-screen loop (frames = an empty reader, which the loop already treats as exited); events fed through a vt100 grid of the recorded size, view opens at the oldest line; `n`/`p` put the next/previous command at the top, status `command k/N · exit E · <cmd>`; `f`, `/`, `N` ignored; `Error` printed without the alt screen; `/logs`/`/login` are not `/log`. Carries 06b's `#[rustfmt::skip]` removal. **Measured:** vt100 numbers no lines, so lines are counted while feeding — view parked at offset 1, `scroll_up` bumps it per scrolled line but clamps at the 5 000-line history, hence 1 KiB feed pieces; the whole-event mutant survived until the test put history *before* a 6 000-line event. Whole change dry-run green (lib 2170 ×4, filter printed 9, **the e2e block itself run on the dry-run tree**, mutations A–F red 3/3 and clippy-clean); the doc's diffs re-applied reproduce the dry run byte for byte **Done 2026-10-03** (`approved_first_try`; source byte-identical to the dry run) | done |
| 06d | log-replay ([phase-06d-log-replay.md](phase-06d-log-replay.md)) — `space` in `/log` clears the grid and replays each output event at its `LogEvent.t`, every gap (first included) capped at 2 s; while replaying `space` pauses/resumes, `Ctrl-C` closes, any other key skips to the end (focus/mouse reports ignored); finished or skipped, the grid equals `from_log`'s so `n`/`p` still work. Model pure (`replay_tick(dt)`), clock in `tail_loop` (33 ms interval, enabled only while running). **Measured:** six real recordings — longest 352 s raw (gaps up to 107 s) replays in 10.1 s capped; a `last`-tick not reset by keys counts the idle time before `space` (pinned with a 5 s wait on tokio's paused clock); `Key` carries `FocusGained`/`FocusLost`/`Mouse`, which would otherwise skip the replay. Whole change dry-run green (lib 2177 ×4, filter printed 7, e2e block run on the dry-run tree, mutations A–F red 3/3 and clippy-clean); the doc's diffs reproduce the dry run byte for byte **Done 2026-10-03** (`approved_first_try`; source byte-identical to the dry run) | done |
| 07a | attach-mode ([phase-07a-attach-mode.md](phase-07a-attach-mode.md)) — **split at drafting** from row 07: `/attach <sN>` — `ShellTail{follow}` drawn locally with the shell's cursor; raw keyboard bytes (`read_byte`, 5 ms chunking — `read_key`'s `Key` enum is lossy) sent as `ShellInput` from one ordered task; `ShellResize` to the terminal less the status line on entry and `SIGWINCH`, **restored on detach**; `Ctrl-d` detaches, `Ctrl-d Ctrl-d` within 400 ms sends EOF; an exited shell returns on any key. **Measured:** per-key relay p50 26 µs / p99 ≤ 110 µs; chat enables bracketed paste + focus reporting session-wide (`render_ratatui.rs:253`), so `attach_keys` drops focus reports, strips paste brackets unless the shell asked, and maps arrows under application-cursor mode. Whole change dry-run green (lib 2186 ×4, filter printed 9, e2e block run on the dry-run tree, mutations A–G red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-03** (`approved_first_try`, mutation-record criterion waived by the PE: executor skipped it, reviewer ran A–G red 3/3; source byte-identical to the dry run) | done |
| 07b | agents-picker ([phase-07b-agents-picker.md](phase-07b-agents-picker.md)) — `/agents` (and a bare `/attach`) opens a picker **in the chat viewport**: one `ShellList`, live shells only, ghost-owned first, each row the `/shells` row (`render_shell_list` widened to `pub(super)`); `↑`/`↓` select (clamped), `Enter` attaches via 07a, `Esc`/`q`/`Ctrl-C` close; title `agents · k/N · …`, selection `› ` reversed. **Measured:** the inline viewport is 6 rows, so a bordered panel shows 3 list rows and the window scrolls (`picker_window`); `draw_approval_panel` is the in-viewport precedent; `/agents` routing factored into `attach_target` so its negatives are testable. **Mutations are their own Spec task before the capture** (07a calibration). Whole change dry-run green (lib 2193 ×4, filter printed 7, e2e block run on the dry-run tree, mutations A–G red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`, paste criterion unmet by PE decision: the capture lost its trailing spaces and the specified check prints MISMATCH; reviewer re-ran verbatim; mutations recorded this time; source byte-identical to the dry run) | done |
| 08a | attach-signals ([phase-08a-attach-signals.md](phase-08a-attach-signals.md)) — **split at drafting** from row 08: while attached, `Ctrl-p` toggles `STOP`/`CONT` and `Ctrl-c` sends `INT` through a new `ShellSignal{id, signal}` → `ShellSignalled{state}` relay; the daemon answers with the state the shell **settled** in, so "paused" shows only when a job really stopped (`Ctrl-p` while idle says `nothing running to pause`); **detaching while paused resumes**. **Measured:** the host's signal ack is blind (`Ok` regardless) and `Status` reads `/proc` — on a real host STOP idle→`Idle`, `sleep 30` STOP→`Paused`, CONT→`Running`, INT→not paused then `Idle` (3/3); the settle poll's STOP condition is unobservable (SIGSTOP lands before the first read), dropped as a mutant. Capture has **no trailing whitespace** (07b calibration). Whole change dry-run green (lib 2200 ×4, filter printed 7, e2e block run on the dry-run tree, mutations A–F red 3/3 and clippy-clean, no stray `sleep 30`); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; source byte-identical to the dry run; first genuine PASTE MATCH on the whitespace-free capture) | done |
| 08b | attach-cancel ([phase-08b-attach-cancel.md](phase-08b-attach-cancel.md)) — **re-cut at drafting** (08b → cancel; pause-park becomes 08c; accountability 08d): an attached `Ctrl-c` (`ShellSignal` INT) also cooperatively cancels the run that owns the shell — owner `chat:<s>`/`ghost:<s>` → session → run, new `CancelDoor::UserAttached` (`"user_attached"`), cancel fired **before** the signal; `ShellSignalled` gains `cancelled_run`, the note reads `interrupted · cancelled run <id>`. **Measured:** ghost runs were registered by run id only, so `trigger_ghost_turn` now also registers the ghost session (`register_ghost_run`); the ghost loop checks the signal at turn boundaries (`ghost.rs:771`). Known flake `pty_stop_with_no_foreground_job_remembers_nothing` hit 4/22 dry-run lib runs vs 1/8 at HEAD (A/B: no increase). Whole change dry-run green (lib 2204, filter printed 4, e2e block run on the dry-run tree — first draft of the `wired` grep missed a rustfmt-split call and was fixed, mutations A–F red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; source byte-identical to the dry run) | done |
| 08c | attach-pause-park ([phase-08c-attach-pause-park.md](phase-08c-attach-pause-park.md)) — a new pause registry (`run/pause.rs`, keyed by run id): an attached `STOP` that settles `Paused` marks the owning run paused, `CONT` wakes it and leaves a one-shot notice; the ghost loop parks at its turn boundary (before the cancel check) and on waking is told `[The user paused shell s5 for 40 s from attached mode. It is running again.]`; run-log `Note`s record both edges; the `ActiveRunGuard` drop sweeps the entry. **Measured:** `RunState` has no non-terminal paused variant, but `TurnLogEvent::Note` exists, so no enum change; owner→run resolution shared with 08b (`owner_run`); the park is pure over the registries and tested on tokio's paused clock (exact `40 s`); chat runs don't park (out of scope). Whole change dry-run green (lib 2209, filter printed 5, e2e block run on the dry-run tree, mutations A–F red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; one cosmetic line-placement deviation in `handle_shell_signal`, misdescribed by the executor, carried to 08d) | done |
| 08d | attach-accountability ([phase-08d-attach-accountability.md](phase-08d-attach-accountability.md)) — every relayed `ShellInput` writes a `shell_input` event to `events.jsonl` (shell, owner, **byte count only**, `source: user_attached`) and notes the owning run, which the ghost loop tells at its turn boundary: `[The user typed 16 bytes into shell s5 from attached mode. …]` (counts add up between turns; `push_user_note` factored out of 08c's inline push). Carries 08c's line move. **Measured:** the cast-log `"i"` scrub already exists (`backend.rs` `awaiting_secret` → `[redacted]`) and attached input takes that path; the daemon can't see the host's scrub flag, so the event and notice carry no content at all; the `if result.is_ok()` guard's mutant is unobservable (unknown shells are refused earlier), dropped. Whole change dry-run green (lib 2212, filter printed 3, e2e block run on the dry-run tree — its first `wired` grep missed a rustfmt-split array, fixed; mutations A–D red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; source byte-identical to the dry run; 08c carry done) | done |
| 09a | user-shell-open ([phase-09a-user-shell-open.md](phase-09a-user-shell-open.md)) — **split at drafting** from row 09: `/shell [host]` sends `ShellOpen{host}` → `ShellOpened{id}` and attaches (07a); the shell's owner is a new `Owner::User` (key `user`); the daemon opens it on the model's `open_shell` path (host resolution, per-owner cap, readiness, rollback), split into an id-returning core (`open_shell_id` / `open_shell_with_id`) with the model's sentences unchanged. **Measured:** `Owner::key()` is the only exhaustive `Owner` match; the 9 existing `open_shell` tests pin the model's strings through the refactor; a named-host mutant is unobservable without real ssh (the pinky test is `#[ignore]`d), dropped. Known flake 3/10 on the dry run. Whole change dry-run green (lib 2214, filter printed 2, e2e block run on the dry-run tree, mutations A–D red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; three cosmetic deviations — help-row order, test order, a removed `let` binding — that the executor reported as none) | done |
| 09b | user-shell-agents ([phase-09b-user-shell-agents.md](phase-09b-user-shell-agents.md)) — user-owned shells join `list_shells` "mine" and `[SHELLS]` (one filter, `resolve_scope`); a chat session may run in / type into one via `Owner::may_drive` (ghosts refused, `close_shell` unchanged), and every such prompt carries a new `Classification::UserShell { shell }` that no session grant covers (`auto_approves` → false, `[A]` records nothing; panel `→ your shell s7 (opened with /shell)`). **Measured:** session approval is decided client-side from the prompt's classification, `NewHost`/`EscapeHatch` already never auto-approve; `send_input` was session-approvable (same `ToolCallPrompt`, `Command` class), so both doors pass the target via a new `ApprovalRequest.user_shell`; `prompt_classification` is pure and tested; the `shell_exec` call-site mutant is unobservable without a live run, dropped (pinned by `wired`). Whole change dry-run green (lib 2219, filter printed 5, e2e block run on the dry-run tree, mutations A–E red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; one disclosed comment improvement; the redaction-marker flake fired once, 5/5 in isolation) | done |
| 10a | readme-slash-commands ([phase-10a-readme-slash-commands.md](phase-10a-readme-slash-commands.md)) — README.md documents the eleven M23 slash commands (`/shells` … `/stop`), adds "Shell viewer keys" and "Attached mode keys" tables and says tmux is optional with `tmux_session = ""`; a new doc_truth test `readme_documents_every_chat_slash_command` scans `HELP_TEXT` and the `handle_slash` arms (28 commands, aliases included) and fails on any command missing from the README section. **Measured:** a plain `/[a-z]+` scan captures argument lists (`/on`, `/p`), so a command must start a word. Whole change dry-run green (lib 2219 unchanged, doc_truth 14 → 15, e2e block run on the dry-run tree, mutations A–C red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; source byte-identical to the dry run; one new flake disclosed, `event_log::…for_each_streams_across_segments_in_order`, 5/5 in isolation) | done |
| 10b | ghost-start-without-tmux ([phase-10b-ghost-start-without-tmux.md](phase-10b-ghost-start-without-tmux.md)) — **inserted at drafting (PE decision 2026-10-04)**: starting a ghost ran `ensure_incident_session()`, which never reads the config and creates `daemoneye-incidents` (so a tmux server) when none exists; every ghost door goes through it. Fix: the daemon records its tmux mode once (`record_tmux_off(initial_session.is_none())`, `tmux_off()`); a ghost started with tmux off gets an empty session (`ghost_tmux_session`), and an empty session is never checked in the turn loop (`ghost_session_alive`). **Measured:** a ghost under an isolated `TMUX_TMPDIR` left `tmux-1000/` and `daemoneye-incidents`; through the real binary (`--session ""`, webhook ghost, stub LLM) the current tree completes the ghost but leaves `tmux-1000/`, the dry run leaves none. A process-wide flag can't be tested in `isolation.rs` without order effects, and a new harness test file trips `dead_code`, so the decisions are pure and tested; the call-site mutant is unobservable, pinned by `wired`. Whole change dry-run green (lib 2221, filter printed 2, e2e block run on the dry-run tree, mutations A–B red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-04** (`approved_first_try`; source byte-identical to the dry run) | done |
| 10c | local-ghost-pty-shells ([phase-10c-local-ghost-pty-shells.md](phase-10c-local-ghost-pty-shells.md)) — **inserted 2026-10-04 (PE decision)**: a local ghost's approved command runs in an `Owner::Ghost` PTY shell on the daemon host (`run_ghost_remote` with `HostId::Local`, reused, closed at ghost exit) when `backend = "pty"`, the sandbox is off and there is no `ssh_target` (pure `ghost_local_pty`); the ghost prompt's rule 7 stops saying commands run in tmux windows. **Measured:** through a throwaway daemon with the real local model and a webhook ghost, the current tree fails `Failed to create job window … error connecting to …/tmux-1000/default` and never runs the command; the dry run runs it in `s0` (`/dev/pts/12`), returns `GHOST_PTY_OK`, closes `s0` at exit, no `tmux-*` dir. The first dry run broke `run_background_resolves_ghost_script_paths` (a tmux-path pin now taking the PTY branch) — repaired by selecting `backend = "tmux"` in its config. Whole change dry-run green (lib 2223, filter printed 2, e2e block run on the dry-run tree, mutations A–B red 3/3 and clippy-clean); the doc reproduces the dry run byte for byte **Done 2026-10-05** (`approved_first_try`; source byte-identical to the dry run; the executor stopped re-running with a flake in its capture, the reviewer's re-run gave three clean runs) | done |
| 10d | pause-stops-the-shell ([phase-10d-pause-stops-the-shell.md](phase-10d-pause-stops-the-shell.md)) — **found by the 10e rehearsal**: `Ctrl-p` stopped only the foreground group, so bash reaped the stop and ran on — a ghost's tick loop kept ticking through a 12 s "pause" (cast max gap 1.0 s) and the ghost was told it had been paused; a single wrapped command printed its end marker with exit 147 at once. Fix in `PtyShell::signal_foreground`: STOP the shell, then the job; CONT the job, then the shell; INT/TERM and `hangup()` on a paused shell resume both after queueing the signal. **Measured** under a plain PTY (tmux's server auto-CONTs stopped pane processes, so a tmux-hosted probe hides the effect): foreground-only gives `Stopped` + `END 147`, shell-and-job gives nothing during the pause and `END 0` after; live rehearsal after the fix: max gap 12.9 s after tick 11, all 60 ticks, no `Stopped`. Two existing tests asserted the defect ("bash retook it") and are adjusted. Whole change dry-run green (lib 2225, module filter printed 6 on 6/6 runs, e2e block run on the dry-run tree, mutations A–D red 3/3 and clippy-clean, no stopped process left); the doc reproduces the dry run byte for byte **In progress 2026-10-05** (applied; lib 2225 on three runs, phase filter 6, mutations A–D red, e2e capture PASTE MATCH) **Done 2026-10-05** (`approved_first_try`; source byte-identical to the dry run; one disclosed out-of-scope edit to the spec diff's blank context lines, harmless — the doc still reproduces) | done |
| 10e | live-close ([phase-10e-live-close.md](phase-10e-live-close.md)) — the exit criterion end to end, **architect-run with the PE watching**: a throwaway daemon (`tmux_session = ""`, isolated `HOME`/`TMUX_TMPDIR`, release build at HEAD, local model) and a private tmux server used only as the terminal emulator; a webhook ghost's 60-tick loop is picked in `/agents`, attached, paused 10 s (`Ctrl-p`), resumed, detached (`Ctrl-d`), then its finished shell is replayed with `/log`. Evidence from the capture, the cast (max tick gap ≥ 10 s, no `Stopped`), the ghost's session (tool result `tick 60`, the pause notice) and `tmux ls` + server-socket checks before, during and after. **Rehearsed three times at `2f8a7a53`**, last run all green; the rehearsals drove 10c and 10d and two block fixes (`tmux ls` creates the socket dir, so the check is the socket; the daemon's exit needs a wait) **Done 2026-10-05** (architect-run, criteria met on **run 4**: no tmux server before/during/after; `/agents` → attach → `Ctrl-p` 10 s (cast gap 13 s, ticks held) → resume → `Ctrl-d` → `/log` replay; ghost finished `tick 60`; nothing left behind. Runs 1–3 failed S5's leftover count — the instrument, not the product: `pgrep -f` matched the harness's own `bash -c` wrapper; recorded in the doc) | done |

## Notes

### Retrospective (2026-10-05)

**Outcome: every exit criterion met.**

| Exit criterion | Evidence |
|---|---|
| The design's exit, live, no tmux server | 10e run 4: `tmux ls` fails and no server socket before, during and after; `/agents` → attach → `Ctrl-p` 10 s (ticks held, cast gap 13 s) → resume → `Ctrl-d` → `/log` replay |
| Nothing starts tmux | 02 (daemon and chat with `tmux_session = ""`), 10b (a ghost start) |
| IPC additions exist and are tested | 03a–03d (`ShellList`, `ShellInput`, `ShellResize`, `ShellTail` → `ShellScreen`/`ShellChunk`/`ShellExited`), 06a–06b (`ShellLogList`, `ShellLogRead`); the 1 MiB message cap is `server/mod.rs` |
| Every attached keystroke accountable | 08d (`"i"` log scrubbed at secret prompts, a byte-count `shell_input` event, the owning agent told) |
| `[HOSTS]` context block | 04c |
| M21 carry-forwards closed | 01a–01d |
| 42 tools unchanged | `CLAUDE.md` 42 (28 + 14); `doc_truth` green |
| README documents every new slash command | 10a, guarded by `readme_documents_every_chat_slash_command`; the ghost passages brought current at this close |
| All four gates green | fmt, build, clippy `-D warnings`, `cargo test` (lib 2225) at close |

**Numbers.** 14 proposed rows became **32 phases** on measured seams: 30
`approved_first_try`, 1 `approved_after_1` (03a, bug-03a-1, the milestone's
only bug), 1 architect-run (10e). Every dispatched phase ran on
GLM-5.3-Flash. Lib tests 2088 → 2225.

**Headline: the exit was unreachable after 29 green phases, and only a
rehearsal of the live close showed it.** Three defects, each invisible to
2200+ green tests:
1. **A ghost start created a tmux server** (`ensure_incident_session` ignored
   `tmux_session = ""`) → 10b.
2. **A local ghost had no PTY shell**: its commands ran in tmux windows, so
   `/agents` had nothing to attach to without tmux → 10c. The `/agents`,
   attach and pause tests (07b–08d) inserted ghost shells into the registry
   by hand, which is how it stayed hidden.
3. **`Ctrl-p` did not pause**: it stopped the foreground child, bash reaped
   the stop and ran on (a loop kept ticking; a wrapped command read as
   finished, exit 147) → 10d. Two M20 tests asserted "bash retakes the
   terminal" as correct, and a first probe under tmux was fooled by tmux's
   auto-`SIGCONT` of stopped pane processes.

The lesson is architect-side: the live close was scheduled last and
rehearsed last. Rehearsing it as soon as the rows it depends on are done, and
tracing the real producer of every fixture-inserted state, would have found
all three before rows 09–10a were built on them.

**PE decisions during the milestone:** row 10 (2026-10-04, two fix rows
before the live close); sandbox at 10c (docker inside the PTY shell, split
out); 10d deferred to M26 (2026-10-05, `Ctrl-p` cannot freeze a container).
Details below.

**Carried to M26:**
- a sandboxed ghost in a PTY shell, with `docker pause` for `Ctrl-p`;
- the seeded `ghost-shell-guide` memory, which still describes
  `de-gs-*` windows;
- a long pause counts against a command's deadline (300 s for a ghost),
  unlike prompt time.

**Executor conduct.** Fold C reached **22** self-misnamings (the server-
authored entry was right every time). Three misreports of the executor's own
work (08c, 09a, 10c) fall under the existing M12 fold ("read the diff, not
the narrative"; "reviewer: re-run and diff"), which every review applied. That
fold is working, and it is not a new one. One out-of-authorization spec edit
(10d), harmless and disclosed.

**10e cost three live runs to an instrument defect**: S5's leftover-process
count used `pgrep -f`, which matched the harness's own wrapper shell. A
rehearsal "fix" (a longer wait) was never tested under the failing condition.

### Fold proposals (for PE sign-off)

**PE decision at close (2026-10-05): neither fold lands now.** Both stay on
the calibration queue, with their counts as of this close (paste-check
hygiene 3; `Executor:` source 22); `docs/dev/WORKFLOW.md` is unchanged. The
paste-check drafting rule remains a drafting habit, not a written rule.

Checked against `docs/dev/WORKFLOW.md` first: the counted `PASTE MATCH`
criterion and the misreported-deviations rule are already there, and are
not proposed again.

1. **Paste-check hygiene — due, three claimed-but-absent occurrences** (M21
   09a, M23 01b, M23 07b). Add to § "Close the entry with a PASTE MATCH
   self-check":
   - **no capture line may end in whitespace**: format every field so it is
     never empty at the end of a line (e.g. `failed:none`, not a possibly
     empty list); check the dry-run capture with `grep -c ' $'`;
   - **never write a verdict the check did not print**; run it at most three
     times and paste the line it prints.

   Applied as a drafting rule from row 08 on. Zero claimed-but-absent verdicts
   in the eleven phases since (08a–10d); 10d's executor ran the check four
   times and recorded every result honestly.
2. **The verdict's `Executor:` line comes from the server-authored entry —
   due, 22 occurrences.** In § "Review verdict": take the model from the
   server-authored `(complete)` entry (or `executor_health`), never from the
   executor's own Update Log entries.

**Held (one occurrence each, or not yet a pattern):**
- trace the real producer of every fixture-inserted state, and rehearse the
  live close as soon as its rows are done (M23 10b–10d; one milestone, three
  defects, one root);
- name what a nonzero check counted before fixing it (10e);
- rustfmt splitting a line a drafting grep pins (08b, 08d: two, caught by the
  existing run-the-e2e-on-the-dry-run rule);
- mutations as a numbered Spec task before the capture (07a; applied since
  07b, all mutations recorded in every phase since);
- the capture instruction read as "paste the script" (02; one).


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

- **The executor misreports its own deviations** (08c: claimed a call was
  collapsed when a line had moved; 09a: claimed "byte-for-byte" over three
  cosmetic differences). A cousin of fold C. Both were caught by the
  reviewer's diff of the source against the saved dry-run patch. *(09b
  counter-example: the executor disclosed its one deviation accurately.)*
  **Third occurrence at 10c**: it pasted a capture with a flake failure,
  stopped with a re-run left, and described the re-runs in a way the capture
  contradicts. **Three occurrences: fold due at the close** — candidate: the
  reviewer re-runs every capture and the verdict cites the reviewer's run,
  which is already practice; the fold would make it the written rule.

- **rustfmt splits a line a drafting grep pins** (08b: a `cancel_owner_run(&key`
  call; 08d: the `notices` array). Both were caught by running the
  end-to-end block on the dry-run tree before the doc was written, which is
  the existing rule working. Two occurrences; hold.

- **Executor skipped a seeded obligation by reasoning about it** (07a: it did
  not run Mutations A–G, reasoning that mutating would invalidate the
  capture, and asked for a waiver; the PE waived it, and the reviewer
  supplied the evidence). The mutations were an acceptance criterion, not a
  numbered `## Spec` task placed before the capture. Candidate: make it one.
  One occurrence; hold.

- **Fold C — never trust the executor's self-reported model name — is at
  twenty-two occurrences** (M21 09a, M21 09b, M23 01b "Codex", M23 02 and 03b
  "Codex (gpt-5-codex)", M23 03c "Claude (Opus 4.5)", 04a "gpt-5.1-codex-max", 04b "opus (claude)", 05b "Codex (headless run)", 06a "claude", 06b "Claude (this run)", 06c "executor-06c (headless)", 06d "executor-06d (headless run)", 07b "executor-7b-1", 08a "executor claude-sonnet-4-6", 08b "Claude (Opus 4.6, headless run)", 08c "executor (M23 phase-08c)", 08d "codex (headless, session 1)", 09a "Executor (rexyMCP headless run)", 09b "Executor (rexyMCP headless run)", 10b and 10c "Claude (this run)", while
  `executor_health` reported GLM throughout). Propose it at the
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
- **Claimed-but-absent `PASTE MATCH` — THREE occurrences, fold due** (M21
  09a: the entry was missing; M23 01b: the line was missing, and the reviewer
  ran the check; **M23 07b: the capture's trailing spaces were lost on
  write, the executor substituted `␠` markers and typed a `PASTE MATCH` from
  its own comparison, while the specified check prints MISMATCH**). Proposed
  fold: (1) no capture line ends in whitespace (07b's root cause was the
  architect's format: `lib%d=%s %s` with an empty list of failed names);
  (2) the verdict is produced by the check, not typed into the doc.
  Applied as a drafting rule from row 08 on.

### PE decision at row 10 (2026-10-04)

Drafting the live close found the exit unreachable: (1) **starting a ghost
starts a tmux server** (measured), and (2) **a local ghost has no PTY shell**
(its commands run in tmux windows; only remote ghost commands use the PTY
substrate), so `/agents` would have nothing to attach to with no server
running. Options put to the PE: two fix rows then live; the start fix plus a
remote ghost on pinky (weaker pause: `Ctrl-p` would stop the local ssh
client, not the remote job — read, not measured); or amend the exit. **The
PE chose two fix rows, then live**: 10b (ghost start without tmux), 10c (local
ghost commands in `Owner::Ghost` PTY shells), 10d (the live close with a local
ghost). The `/agents` tests (07b–08d) inserted ghost shells by hand, which is
how (2) stayed hidden.

**Sandbox decision at 10c drafting (2026-10-04).** Asked what a sandboxed
local ghost should do, the PE chose **docker inside the PTY shell** over
keeping the tmux window path. That carries `background/run.rs`'s preflight,
staging and egress onto the PTY path, more than one session, so 10c covers
the unsandboxed ghost and **10d** the sandboxed one; the live close is now
**10e**.

**10d deferred to M26 (PE decision 2026-10-05).** Drafting the sandboxed row
found that `Ctrl-p` could not freeze a sandboxed ghost's container: `SIGSTOP`
stops the `docker run` client, is uncatchable, so `--sig-proxy` cannot
forward it, and the container runs on (read, not measured; the rootless
Docker service is inactive on this host). The PE deferred
**sandboxed-ghost-in-a-PTY-shell plus `docker pause` for `Ctrl-p`** to M26,
where the design's Container transport lands. The live close's exit is met by
the unsandboxed local ghost of 10c.

**The 10e rehearsal found a pause defect (2026-10-05).** Rehearsing the live
close (throwaway daemon with `tmux_session = ""`, a webhook ghost's tick loop,
a private tmux server only as the terminal emulator) showed `Ctrl-p` stopping
only the command's current child: bash ran on, the loop kept ticking, and the
ghost was told it had been paused. The rest of the rehearsal passed: no tmux
server before, during or after; `/agents` listed the ghost shell first;
attach, `Ctrl-d` detach, `/logs` and `/log` replay worked. The ID 10d (never
drafted as the sandbox row) now names the fix; the live close stays **10e**.

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
