# M20 — Shell Engine

**Goal:** The daemon owns its own PTY-backed shells — spawned by detached
shell-host processes so they survive a daemon restart, logged byte-exact as
asciicast v2, captured through a vt100 screen model, and completed by a
deterministic marker protocol with real exit codes — behind an
`[execution] backend = "tmux" | "pty"` flag, so that `run_terminal_command`
from a real `daemoneye chat` turn runs on the new substrate with tmux
untouched.

**Status:** in-progress — scoped 2026-09-03; phase-01 drafted 2026-09-03 on
PE direction (`/rexymcp:architect next`).

**Depends on:** M19 — Sandbox Completion (closed 2026-09-03 at the 2.0
boundary). First milestone of DaemonEye 2.0; plan of record
`docs/design/daemoneye-2.0.md` § 2.1 (shell engine) and § 6 (M20 row).

**What this milestone is not.** No tool-surface change beyond routing the
existing `run_terminal_command` (M22), no `[hosts]` or SSH (M21), no client
UX — no `/tail`, `/agents`, attach, pause keys (M23), no run model (M24), no
tmux deletion (M26). Every phase keeps `backend = "tmux"` byte-for-byte
today's behaviour; the flag defaults to `tmux` until M26.

**Pre-drafting measurements (architect-run 2026-09-03, scrappy, rustc
1.95.0, `portable-pty` 0.9.0, `vt100` 0.16.2)** — the facts below were
executed, not reasoned about, per the M18 rule:

- **Spawn + marker + exit code work.** `bash` spawned through
  `portable-pty` at 80×24; `false; printf '\n\x1fDE_''END <nonce> %s\x1f\n' $?`
  returned `exit_code=Some("1")`.
- **The PTY echoes the typed command, and a naive marker search matches the
  echo first.** The first probe run returned `exit_code=Some("%s\\x1f\\n' $?")`
  — the needle was found in the echoed command line before any output
  arrived. Fix measured and adopted: the *typed* text carries the marker word
  split (`DE_''END`), so only the shell's own output ever contains
  `DE_END <nonce>`. This is a load-bearing gotcha for phase-02 and belongs in
  its spec verbatim.
- **vt100 captures colour.** A `printf '\e[31mERR line\e[0m'` produced 8
  cells with `fgcolor() == Color::Idx(1)` in the grid — enough for
  `ansi.rs`'s `[ERROR:]` annotation to be re-pointed at cells instead of
  re-parsing SGR bytes.
- **The shell dies when the master-holder exits.** A probe that spawned bash,
  started `sleep 300 &`, and exited without killing the child left the shell
  **dead** (`ps -p <pid>` → nothing; the PTY master closing delivers SIGHUP to
  the session). This is the direct evidence for the PE's restart-survival
  requirement being met only by a separate shell-host process (§ 2.1 of the
  plan), not by an in-daemon PTY with a reconnect.
- **Not yet measured, must be before the relevant phase is drafted:** PTY
  resize propagation (`stty size` after `master.resize`), alt-screen programs
  (`less`, `vim`) in the grid, `fish` marker syntax (`set __de_ec $status`),
  8-bit/UTF-8 split across reads, and the shell-host socket protocol itself.
  The probe source is kept at `probes/ptyprobe.rs` (+ `Cargo.toml.txt`) in
  this directory so the measurements are reproducible (`cargo run` in a
  scratch crate; `ptyprobe orphan` for the SIGHUP leg); phase docs quote the
  relevant lines rather than pointing at it.

**Exit criteria:**

- `[execution] backend = "pty"` is a parsed, validated config value with
  `"tmux"` the default; with `backend = "tmux"` a full chat + ghost round
  trip performs **no** PTY spawn (negative criterion, unit + live).
- With `backend = "pty"`, `run_terminal_command` from a real `daemoneye chat`
  turn runs in a daemon-owned shell and the tool result carries the **real**
  exit code: a deliberately failing command (`false`, `exit 3`) reports
  non-zero, and the evidence anchor is the session JSONL `tool_results`
  entry, not a unit test (live, architect-run).
- The marker protocol never matches its own echo: a unit test feeds the
  echoed command line first and asserts no completion; a second feeds the
  output marker and asserts the exit code (both mutation-checked).
- Every shell has an asciicast v2 log at `var/log/shells/<id>-<ts>-<label>.cast`
  and a sidecar `.meta.json` whose command index gives, for command N, the
  event range, exit code and start/end times; reading "the output of command
  N" is a slice, not a scan (unit with a real log written by a real shell).
- The vt100 screen of a shell renders through the existing `ansi.rs`
  annotation and `status.rs` classification: a red line becomes `[ERROR: …]`
  and an idle prompt classifies as `Idle` (unit, fixture-driven, no PTY).
- **Restart survival (the PE's hard requirement):** with a shell running
  `sleep 60` under `backend = "pty"`, `daemoneye stop` then `daemoneye daemon`
  leaves the shell alive, re-listed under the same id, still logging, and its
  `sleep` completes with a marker the re-attached daemon observes (live,
  architect-run; the evidence is the `.cast` file containing bytes on both
  sides of the restart).
- A shell-host whose daemon is gone keeps writing its log; a shell-host that
  is gone is swept from `var/run/shells/` at daemon start without touching a
  live one (unit for the sweep decision, live for the adoption).
- Interactive commands (`is_interactive_command()` true, e.g. `vim`, `less`,
  `ssh user@host`) do **not** wait for a marker: the tool returns with the
  shell id and a `[Interactive: …]` note within 2 s (unit + live).
- Pause/resume/cancel exist on the shell API — `SIGSTOP`/`SIGCONT`/`SIGINT`
  to the PTY's foreground process group — and are unit-tested against a real
  `sleep` in a real PTY; **no client keys yet** (M23 wires them).
- Shell output that reaches the model is masked and the `.cast` at rest is
  raw and `0600`, matching the session-transcript rule in `security.md` § 2
  (unit: a seeded secret appears in the log and not in the tool result).
- The M6 lifecycle-policy table covers `var/log/shells/` and
  `var/run/shells/` (the existing test fails on any uncovered class).
- `CLAUDE.md` § Key files, `README.md` § Configuration (the `[execution]`
  and `[shells]` sections) and `docs/architecture.md` § 5 are updated in the
  closing phase — README currency is a per-milestone rule (plan § 5).
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
  PTY-spawning tests run in CI (bash is present everywhere the suite runs);
  they take `crate::test_home_guard()` and a private `var/run/shells/`.

Live checks are architect-run (M14–M19 convention: through the user's door,
session JSONL / `.cast` files as evidence anchors, isolated `HOME`, and —
until M26 — an isolated `tmux -L` server for the chat client).

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.1 (shell engine, shell-host process,
  marker protocol, asciicast v2), § 3 (security deltas), § 5 (README rule),
  § 8 (PE decisions).
- `docs/architecture.md` § 1.4 — the tmux layer this milestone sits beside.
- `CLAUDE.md` § Key files — `src/daemon/background/run.rs` (the 1.x
  background wrapper the marker protocol descends from),
  `src/daemon/utils/shell.rs` (`is_interactive_command`, `shell_exit_var`),
  `src/tmux/ansi.rs` and `src/tmux/status.rs` (re-pointed, not rewritten),
  `src/daemon/instance.rs` (the flock idiom the shell-host reuses),
  `src/daemon/ready.rs` (the readiness pipe the shell-host spawn reuses).
- `docs/security.md` § 1 (peer-uid check — the shell-host socket gets the
  same one) and § 2 (raw-at-rest, masked-on-egress).

## Design decisions on record

- **Shell-host process from the start.** Measured: a PTY whose master-holder
  exits SIGHUPs the shell. The daemon-side `Shell` API is a trait with an
  in-process implementation for unit tests and the shell-host client for
  production; there is no in-process production mode.
- **Marker protocol with a split nonce.** The typed command carries
  `DE_''END`, the output carries `DE_END <nonce> <code>`. Nonce is 128-bit
  random per command; `\x1f` framing bytes are stripped from any displayed or
  model-visible output; a second marker with the same nonce is an anomaly
  event, never a second completion.
- **asciicast v2 with `"m"` marker events** for command boundaries (PE
  decision). The `.meta.json` sidecar is derived from the cast and can be
  rebuilt from it — a `daemoneye reindex`-shaped repair, not a second source
  of truth.
- **`portable-pty` and `vt100`** — chosen so executor phases stay
  `unsafe`-free (STANDARDS § 1). The shell-host's detach (double fork,
  `setsid`) reuses the daemon's existing `libc::fork` path in `main.rs`; that
  phase is architect-authored if it needs new `unsafe`.
- **Everything behind `[execution] backend`, default `tmux`.** The flag is
  deleted in M26 along with `src/tmux/`.
- **No tool, prompt or IPC surface changes** except the minimum to route
  `run_terminal_command` (local, `background=false` semantics preserved as
  "wait for marker"); `[FOREGROUND TARGET]` and `%N` ids stay until M22.

## Phases

Ordering: 01 → 02 → 03 → 04 are hermetic and independent of each other
except that 02 precedes everything that spawns; 05 → 06 are the shell-host
and registry (05 depends on 02; 06 on 05); 07 → 08 wire the tool (07 on 06;
08 on 07); 09 is the live close-out. Phase docs are drafted one at a time via
`/rexymcp:architect next` — none are drafted ahead (M4/M16/M18 precedent:
line-number facts go stale and each landing shifts the next Current state).

| #  | Phase | Status | Scope (one line) |
|----|-------|--------|------------------|
| 01 | execution-config ([phase-01-execution-config.md](phase-01-execution-config.md)) | **done** (approved_first_try, 2026-09-03) | `[execution] backend` + `[shells]` (per-owner cap, exited retention, log retention) config schema, validation, `assets/etc/config.toml` docs, `shells_dir()` / `shell_logs_dir()` / `shell_run_dir()` path constructors, lifecycle-policy rows. Hermetic — no PTY. |
| 02 | pty-marker-protocol ([phase-02-pty-marker-protocol.md](phase-02-pty-marker-protocol.md)) | **done** (approved_after_4, 2026-09-03; 5 rounds, 3 bugs all resolved, landed on a resume) | `src/shell/pty.rs`: `portable-pty` spawn, the split-nonce wrapper for bash/zsh/fish, the pure marker parser with the echo-first negative test, exit-code extraction, `\x1f` stripping. One real-PTY test against `bash`. |
| 03 | asciicast-log ([phase-03-asciicast-log.md](phase-03-asciicast-log.md)) | **done** (approved_after_1, 2026-09-03; 1 bug, resolved) | `src/shell/log.rs`: asciicast v2 writer (header, `o`/`i`/`m` events, per-read flush) + `.meta.json` command index + reader that slices command N. Pure over byte streams; fixtures from phase-02's real capture. |
| 04 | screen-model ([phase-04-screen-model.md](phase-04-screen-model.md)) | **done** (approved_after_1, 2026-09-03; 1 bug, resolved) | `src/shell/screen.rs`: `vt100::Parser` wrapper; `ansi.rs` annotation and `status.rs` classification re-pointed at grid cells; scrollback depth from config. Fixture-driven, no PTY. |
| 05a | shell-host-protocol ([phase-05a-shell-host-protocol.md](phase-05a-shell-host-protocol.md)) | **in-progress** (bounced 2026-09-03, [bug-05a-1](bugs/bug-05a-1.md)) | `src/shell/proto.rs` + `src/shell/host.rs`: the newline-delimited JSON frame set (subscribe / input / resize / signal / status), a socket server that binds `var/run/shells/sN.sock`, checks peer uid and dispatches to a `ShellBackend` trait. Hermetic — fake backend, no PTY, no fork. |
| 05b | shell-host-process | todo (not drafted) | The `daemoneye shell-host --id sN` binary: owns the PTY, writes the cast log, drives the screen, serves 05a's protocol; detached spawn and the readiness pipe. **Architect-authored — needs new `unsafe` (fork/setsid) or a measured safe alternative.** Resize must rebuild the screen from the log, not call `set_size` (see § Notes). |
| 06 | shell-registry | todo (not drafted) | `src/shell/registry.rs`: `ShellId`, `Owner`, per-owner caps, startup adoption by scanning `var/run/shells/`, dead-socket sweep, exited-shell GC under the lifecycle policy. |
| 07 | run-terminal-command-pty | todo (not drafted) | Route `run_terminal_command` through the registry when `backend = "pty"` (local host only, wait-for-marker), masked + annotated result, real exit code in the tool result and in the `events.jsonl` command record. First phase that runs a command on the new substrate from chat. |
| 08 | interactive-and-signals | todo (not drafted) | `is_interactive_command()` → return immediately with the shell id; pause / resume / cancel on the shell API (`SIGSTOP` / `SIGCONT` / `SIGINT` to the foreground pgrp); state transitions `Idle → Running → Paused → Exited`. |
| 09 | restart-survival-and-close | todo (not drafted) | Adoption end to end: shell running across `daemoneye stop` + `daemoneye daemon`; the live sweep of every exit criterion; `CLAUDE.md`, `README.md`, `architecture.md` § 5 updated; retrospective. |

## Notes

- **Phase 05 was split at drafting (2026-09-03) into 05a and 05b**, the same
  narrowing M18 and M19 each took. As scoped it bundled a wire protocol, a
  socket server, PTY ownership, log and screen wiring, detached spawn and a
  readiness handshake — far past one executor session, and phase-02 showed
  what an oversized phase costs here. **05a is hermetic and executor-shaped**
  (frames, server, peer-uid check, dispatch trait, fake backend). **05b holds
  everything needing a real PTY or a fork**, and is the one the README already
  reserved for architect authorship.

- **Carry into phase-07 (recorded at phase-04 close, 2026-09-03): where does a
  gap belong relative to an annotation marker?** `annotated()` preserves
  cursor-positioned column layout in uncoloured text, but a gap between a
  *coloured* run and plain text is absorbed into the colour span and trimmed:
  `contents()` gives `"red      plain"`, `annotated()` gives
  `"[ERROR: red]plain"`. Exact column agreement between the two is
  **impossible in general** — inserting `[ERROR: ` and `]` shifts everything
  after it — so this is a design choice (`[ERROR: red]      plain` versus the
  current form), not a defect to patch blind. Decide it with phase-07, the
  first consumer that puts this text in front of a model.

- **Why measurement came first, again.** M18's three disproved design claims
  and M19's five drafting corrections are the record. This milestone's first
  probe disproved the obvious marker search within one run — the echo gotcha
  would otherwise have shipped as a "works on my machine" spec and bounced
  phase-02.
- ~~**The `less`/resize leg of the probe was inconclusive.**~~ **Measured
  2026-09-03 while drafting phase-04**, and one result changes a design
  assumption: `Screen::set_size` does **not** reflow — a soft-wrapped row
  becomes a hard line break at the *old* width, so widening the terminal
  corrupts text already on screen. Resize therefore cannot be a `set_size`
  call on a populated grid; **phase-05 must rebuild the screen from the cast
  log instead.** Also measured: the six SGR colour codes map to
  `Idx(1|9|3|11|2|10)`; `contents()` is the visible screen only, with
  scrollback reachable as a view offset; and `alternate_screen()` tracks the
  alt-screen escapes. Full table in phase-04 § Measured facts.

- **Phase-02 drafting added a BEGIN marker to the design (2026-09-03).** The
  2.0 plan's § 2.1 describes only an end marker. Measurement showed the PTY
  echoes the typed command ahead of the output, so a lone end marker leaves no
  reliable left edge for the output; extracting strictly between a
  `\x1fDE_BEG <nonce>\x1f` and the end marker does. The begin marker also
  gives phase-03's asciicast `"m"` events their command-start boundary for
  free. `docs/design/daemoneye-2.0.md` § 2.1 should be amended at the phase-09
  documentation sweep.
- **Scheduled `ActionOn::Script` jobs and `[sandbox.ghost_defaults]`** are
  carried out of M19 unscheduled; neither is M20's concern — the run manager
  (M24) gives scheduled jobs the same shell path as every other run.
- **Sandbox transport.** `Transport::Container` (plan § 2.9) is *not* in M20;
  the `Transport` enum ships with `Local` only and a doc comment naming the
  two variants M21 (`Ssh`) and a later phase (`Container`) add, so the enum
  is additive from day one.
