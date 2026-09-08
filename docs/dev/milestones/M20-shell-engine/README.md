# M20 — Shell Engine

**Goal:** The daemon owns its own PTY-backed shells — spawned by detached
shell-host processes so they survive a daemon restart, logged byte-exact as
asciicast v2, captured through a vt100 screen model, and completed by a
deterministic marker protocol with real exit codes — behind an
`[execution] backend = "tmux" | "pty"` flag, so that `run_terminal_command`
from a real `daemoneye chat` turn runs on the new substrate with tmux
untouched.

**Status:** in-progress — scoped 2026-09-03; **PE sign-off 2026-09-06**.

**Sign-off record (2026-09-06).** Reviewed against the tree after six phases:
the goal, non-goals and exit criteria are unchanged and hold. Two structural
changes since scoping, both from measured seams rather than estimation — phase
05 split into 05a/05b/05c, phase 06 into 06a/06b — so the milestone is **twelve
phases, not nine**. Every "not yet measured" item from the original list has
since been measured, including the `less`/resize leg that was inconclusive at
scoping.

One gap was found and closed at sign-off: after the two splits, the **masking**
half of the "output reaching the model is masked, the `.cast` at rest is raw and
`0600`" criterion had **no owning phase**. It is now phase-07's (see the phase
table). The `0600` half was measured at sign-off and already holds — and holds
even under `umask 022`, because `main()` sets `umask(0o077)` unconditionally and
the shell-host runs through the same `main()`:

```
-rw------- s1-1788698980.cast     (default umask)
-rw------- s1-1788698981.cast     (umask 022)
```

It holds *by umask* rather than by an explicit mode, which is the same
fragility phase-05a deliberately removed for the socket. Phase-07 also sets the
mode explicitly.

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
| 05a | shell-host-protocol ([phase-05a-shell-host-protocol.md](phase-05a-shell-host-protocol.md)) | **done** (approved_after_1, 2026-09-03; 1 bug, resolved) | `src/shell/proto.rs` + `src/shell/host.rs`: the newline-delimited JSON frame set (subscribe / input / resize / signal / status), a socket server that binds `var/run/shells/sN.sock`, checks peer uid and dispatches to a `ShellBackend` trait. Hermetic — fake backend, no PTY, no fork. |
| 05b | shell-host-backend ([phase-05b-shell-host-backend.md](phase-05b-shell-host-backend.md)) | **done** (approved_after_1, 2026-09-04; 1 bug, resolved) | A public raw-output stream on `PtyShell`, and `PtyBackend` implementing 05a's `ShellBackend` over PTY + cast log + screen. Real PTY, no fork, no CLI. Resize recreates the screen rather than calling `set_size` (see § Notes). |
| 05c | shell-host-binary ([phase-05c-shell-host-binary.md](phase-05c-shell-host-binary.md)) | **done** (approved_after_2, 2026-09-05; 1 hard_fail escalation + 1 bounce, 1 bug resolved) | The `daemoneye shell-host --id sN` subcommand, detached spawn via `process_group(0)`, and a `Status`-round-trip readiness check. Settles the 05a carry: an `InstanceLock` per shell id is what authorizes unlinking its socket. |
| 06a | shell-registry-core ([phase-06a-shell-registry-core.md](phase-06a-shell-registry-core.md)) | **done** (approved_after_1, 2026-09-05; 1 hard_fail escalation, no bug filed) | `src/shell/registry.rs`: `ShellId`, `Owner`, `ShellRegistry` mirroring `SessionStore`'s newtype+accessor shape, hole-filling id allocation, per-owner caps, and `spawn_for` — which reserves the slot under the lock and spawns **outside** it. |
| 06b | shell-registry-adoption ([phase-06b-shell-registry-adoption.md](phase-06b-shell-registry-adoption.md)) | **done** (approved_first_try, 2026-09-06) | `host_is_alive` (the flock probe, failing **closed**), `scan_shell_ids`, `parse_cast_name`, `adopt_at_startup` (live hosts → `Owner::Adopted`) and `sweep_stale` (dead hosts only). Flips the `var/run/shells` lifecycle row from `Pending`. Hermetic — a held `InstanceLock` *is* a live host to the probe, so no child processes. |
| 07a | shell-client ([phase-07a-shell-client.md](phase-07a-shell-client.md)) | **done** (approved_after_1) 2026-09-06 | `src/shell/client.rs`: `run_command` over `var/run/shells/<id>.sock` — `Subscribe` + `Input` on one connection, chunk accumulation, `parse_outcome` as the terminator, real exit code. Also sets the `.cast`/`.meta.json` modes explicitly (`0600`) rather than inheriting the umask. |
| 07b | shell-session-wiring ([phase-07b-shell-session-wiring.md](phase-07b-shell-session-wiring.md)) | **done** (approved_after_1) 2026-09-07 | `src/shell/session.rs`: `shell_for_owner` — reuse the owner's live shell or create one, `await_ready`, roll the record back when readiness fails. Stands a live `ShellRegistry` up in `run_daemon` (sweep, then adopt), gated on `[execution] backend = "pty"`. Nothing consumes it yet. |
| 07c | run-terminal-command-pty ([phase-07c-run-terminal-command-pty.md](phase-07c-run-terminal-command-pty.md)) | **done** (approved_after_2, 2026-09-08 — [bug-07c-1](bugs/bug-07c-1.md), [bug-07c-2](bugs/bug-07c-2.md), [bug-07c-3](bugs/bug-07c-3.md), all fixed) | Route `run_terminal_command` through 07b's `shell_for_owner` + 07a's client when `backend = "pty"` (local host only), masked + ANSI-annotated result, real exit code in the tool result and in the `events.jsonl` command record. **Owns the masking exit criterion** (PE decision 2026-09-06). First phase that runs a command on the new substrate from chat. |
| 08 | interactive-and-signals | todo (not drafted) | `is_interactive_command()` → return immediately with the shell id; pause / resume / cancel on the shell API (`SIGSTOP` / `SIGCONT` / `SIGINT` to the foreground pgrp); state transitions `Idle → Running → Paused → Exited`. **Also owns exited-shell GC under `shells.exited_retention_secs`** (moved from 06b at drafting, 2026-09-06 — see § Notes). |
| 09 | restart-survival-and-close | todo (not drafted) | Adoption end to end: shell running across `daemoneye stop` + `daemoneye daemon`; the live sweep of every exit criterion; `CLAUDE.md`, `README.md`, `architecture.md` § 5 updated; retrospective. |

## Notes

- **Phase 05b was split again at drafting (2026-09-03) into 05b and 05c**, on a
  real seam found by measurement rather than by guessing at size. Two findings
  drove it:
  1. **The architect-authorship reservation is lifted.** The README reserved
     05b because detaching a child looked to need `fork`/`setsid`, which
     STANDARDS forbids an executor from writing. Measured instead:
     `std::os::unix::process::CommandExt::process_group(0)` — safe and stable —
     puts the child in **its own process group** (measured pgid == pid, versus
     a control child that inherited the parent's), and an orphaned child
     survives its parent's exit either way. **No `unsafe` is required**, so
     05c is dispatchable like any other phase.
  2. **`PtyShell` cannot stream.** Its output arrives on a *private*
     `mpsc::Receiver` consumed by `run()`, and `mpsc` is single-consumer — so a
     host that must feed subscribers, the cast log and the screen at once
     cannot use it as it stands. 05b adds a public stream and builds the
     backend on it; 05c then only has to wire a binary around that.

- **Carries into phase-05b (recorded at 05a close, 2026-09-03):**
  1. **A deterministic seam for the concurrency guard.**
     `host_answers_a_request_split_around_a_chunk` catches its regression only
     about two thirds of the time (measured: 8 failures in 12 runs with the bug
     reintroduced). Making it deterministic needs either a real-clock sleep —
     which this project deliberately removed from its suite — or a seam in the
     backend that forces the interleaving. 05b builds the real backend and is
     the natural place for that seam.
  2. **Who may unlink a socket.** `host::bind` removes whatever occupies the
     path unconditionally, so a second host claiming the same id would unlink a
     live host's socket. Id assignment belongs to the phase-06 registry; decide
     the ownership guard there, the way the daemon's instance flock does it.

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

- **Phase 06 was split at drafting (2026-09-05) into 06a and 06b**, the same
  narrowing 05 took twice. As scoped it bundled the id/owner model, per-owner
  caps, startup adoption, a stale-socket sweep and exited-shell GC — comfortably
  past the ~500-line guideline, and phase-02 already showed what an oversized
  phase costs here. The seam is clean: **06a is what the registry *is*** (types,
  allocation, caps, spawn-and-track), **06b is what it *does* at startup and
  over time** (adoption, sweep, GC). 06b also owns flipping the
  `var/run/shells` lifecycle row, because it is the phase that implements the
  behaviour that row describes — flipping it in 06a would make the table claim
  something untrue.

- **Measured for 06b while drafting 06a (2026-09-05) — the liveness oracle.**
  Run against the real `daemoneye shell-host` binary under a throwaway `HOME`:

  | state | `flock` on `<id>.lock` | `connect()` to `<id>.sock` | files on disk |
  |---|---|---|---|
  | host alive | **BLOCKED** | succeeds | `.lock` + `.sock` |
  | host killed | **ACQUIRED** | `ECONNREFUSED` | `.lock` + `.sock` **both remain** |

  Three things follow, and 06b's spec must carry them:

  1. **Nothing cleans up after a dead host.** Both the socket and the lock file
     survive the process, so the presence of `<id>.sock` proves nothing about
     liveness. That is exactly what the sweep is for.
  2. **The flock is the oracle, not `connect()`.** `connect()` happens to give
     `ECONNREFUSED` for a *fully dead* host, but phase-05c measured the case
     where it lies: a socket that is bound and listening but not yet accepting
     completes a client `connect()` and even accepts a write. A host mid-startup
     is indistinguishable from a healthy one by connect alone. This also matches
     the standing `CLAUDE.md` invariant — *instance ownership is decided only by
     the `InstanceLock` flock*, and liveness probes are reports, never
     authorizations.
  3. **A probe that acquires the lock must release it immediately** and must not
     be treated as authorization to delete anything on its own.

  Also re-confirmed: `bind()` over a stale socket path without unlinking fails
  with `EADDRINUSE`, which is why `host::bind` unlinks first.

- **Exited-shell GC moved from 06b to 08 at drafting (2026-09-06), on a
  dependency-order finding rather than on size.** `shells.exited_retention_secs`
  governs "how long an **exited** shell stays listed before it is reaped" — but
  **nothing marks a shell exited yet**: the `Idle → Running → Paused → Exited`
  state model is phase-08's, `ShellRecord` has no state field, and
  `exited_retention_secs` is referenced only by config tests today. Building a
  GC over a state that does not exist would invert the dependency. **No M20 exit
  criterion is affected** — the sweep criterion is about a *host that is gone*
  (06b's flock probe), not about an exited shell.

- **Measured while drafting 06b (2026-09-06), and this one is a trap for later
  phases.** `LifecycleIntent::ClearAtStartup` appears **only** in
  `src/config/lifecycle.rs` — no code implements it, despite the `var/run` row
  being marked `Implemented`. So adoption works today **by accident**: nothing
  clears `var/run` at startup, and therefore nothing deletes a surviving
  shell's socket. If `ClearAtStartup` is ever implemented for `var/run`, it will
  delete `var/run/shells/` and silently break restart survival — M20's headline
  requirement — unless the `var/run/shells` exemption is honoured explicitly.
  Recorded here rather than fixed: the mismatch is pre-existing and out of
  M20's scope, but any phase that implements it must honour the exemption.

- **`owner` is not recoverable from disk (measured 2026-09-06).** A host leaves
  a `.sock`, a `.lock` (payload: the *host* pid) and a `.cast`
  (`<id>-<started_unix>.cast`); a `Status` reply gives state/rows/cols and the
  *shell child* pid. Nothing records which session owned the shell, so 06b
  adopts under a new `Owner::Adopted` rather than reconstructing one, and
  `started_unix` comes from the cast filename. Note the two pids are different
  and neither is used for liveness — the flock is.

- **Drafting 07c (2026-09-07) — five probes, and two of them changed the
  spec.** Compiled into the tree, run, reverted:

  - **The whole chain already works, exit code included**: one
    `shell_for_owner` + one `client::run_command` returned `exit_code=3` and
    the command's bytes, the second call reused the same id, and `cd /tmp`
    then `pwd` returned `/tmp`. So 07c is composition, not mechanism.
  - **`exit 3` kills the shell.** The first probe used a bare `exit 3` and hung
    to the 15 s timeout — `exit` ends the bash session, so the end marker never
    arrives. `sh -c 'exit 3'` is the shape that yields a non-zero code with the
    shell alive. Pinned in the test plan; it would otherwise have cost the
    executor a full timeout to rediscover.
  - **`annotate_ansi` is the sanitiser, not just the annotator** — measured, it
    strips `\x1b[2J`, `\x1b[H`, `\x1b[?25l` entirely and folds CRLF, while
    `normalize_output` leaves raw ESC bytes in the model's input. That decided
    the pipeline order (annotate → mask → normalize) rather than a guess.
  - **Today's foreground result is not annotated at all**: `capture_pane` omits
    `-e`, so tmux strips the escapes before we see them. The PTY hands us
    escapes for the first time, which is why annotation is real work here.
  - **`log_command` has 11 call sites** (9 in `foreground.rs`, 2 in
    `executor/mod.rs`) and no exit-code field, so 07c adds a sibling rather
    than a parameter. The first draft said "ten"; running `grep -c` corrected
    it to 9 + 2 before dispatch — the same class of unrun number that bounced
    07b.

  **A registry global, not a threaded parameter.** `cache: &Arc<SessionCache>`
  reaches `execute_tool_call` through `AskCtx` → `ConversationLoopCtx` and a
  second chain through `trigger_ghost_turn` → `do_ghost_turn`; threading a
  second handle the same way is a ~10-site breaking type change across five
  files for one consumer — the `multi_site_break` failure class. The daemon
  already keeps its process-lifetime singletons as globals (CLAUDE.md § "Global
  statics in daemon"), so `shell::registry()` joins them, and `run_in_shell`
  still takes `&ShellRegistry` so tests never touch the global.

  **Scope left open for the PE:** `background=true` stays on the tmux
  `de-bg-*` path. Its shell-substrate equivalent needs async completion
  notification and **no phase owns it** — 08 is interactive/signals, 09 is
  restart survival. Raise at 07c's close.

- **Phase 07b was split again at drafting (2026-09-06) into 07b and 07c**, on
  the same kind of missing-piece finding that split 07 — and found the same
  way, by measuring before speccing. `grep -rn 'crate::shell::' src/` outside
  `src/shell/` returns **nothing**: the only production consumer of the whole
  shell module is `main.rs:412`, the `shell-host` subcommand. There is no
  `ShellRegistry` instance in the daemon and `adopt_at_startup` has **no
  caller**, so "route the tool through the registry" presupposed a registry
  that does not exist. 07b builds it; 07c routes the tool.

  Drafting measurements (a real detached host, driven from a separate process,
  then torn down):

  - **The full chain works against a genuinely detached host** — `daemoneye
    shell-host --id s99` under a throwaway `$HOME`, driven by 07a's
    `run_command`: `exit_code=7`, output `"PROBE_OUT\r\n"`. The socket comes
    out `0700` and the lock `0600` from the host itself, so 07b creates
    neither.
  - **Shell state persists across calls** — `cd /tmp` then `pwd` returns
    `/tmp`; `export DE_PROBE=kept` is visible to the next call. Reusing the
    owner's shell is a behaviour requirement, not an optimisation.
  - **Eight concurrent commands on one shell do not corrupt each other** —
    all eight returned their own exit code and their own bytes (`bad=0`).
    Each `Input` frame is one complete line, so the PTY line discipline
    serialises them and each nonce isolates its own output. No per-shell
    mutex is needed for correctness.
  - **`spawn_detached` cannot be reached from a test** (05c: `current_exe()`
    resolves to the libtest binary), which is why `shell_for_owner` takes the
    spawn step as a parameter — the same shape `spawn_for` already uses.
  - **Section G of the E2E block was wrong on first draft and was corrected by
    running it.** `test -d var/run/shells` is `1` under a fresh `$HOME`
    because first-run seeding creates the whole FHS tree — an impossible
    criterion. It now counts `.sock`/`.lock` files, which only exist if a host
    actually started. [[spec-facts-must-be-executed]] caught this before
    dispatch rather than at review.

- **Phase 07 was split at drafting (2026-09-06) into 07a and 07b**, on a
  missing-piece finding rather than on size alone. Measuring before speccing
  showed that **no client for the shell socket exists in production code** —
  phase-05a built the server, and the only connect outside `#[cfg(test)]` is
  `launch::await_ready`'s `Status` round-trip. Routing `run_terminal_command`
  through a daemon-owned shell therefore needs a client first: the daemon does
  not hold a handle to the shell (`ShellRecord` carries id/owner/started_unix
  and nothing else), so it must reach it over the socket. 07a is that client;
  07b is the tool wiring. Together they were ~580 lines, past the guideline.

- **Measured while drafting 07a (2026-09-06) — the round trip works today.**
  A probe compiled into the tree: `PtyBackend` + `serve` on a temp socket,
  one connection, `Subscribe` then `Input` with a marker-wrapped command,
  accumulating `Chunk` bytes until `parse_outcome` answers:

  ```
  PROBE WIRE exit_code=5 output="WIRE_OK\r\n"
  ```

  Two things this settles for 07a: **`Subscribe` + `Input` on the same
  connection is the supported shape** (that is what `bug-05a-1` fixed), and
  **no new wire frames are needed** — the existing frame set carries a full
  command round trip.

- **Also measured: `PtyShell::run` works while `PtyBackend`'s pump is
  draining the same shell** (`exit_code=7`, clean output, and the cast log
  still received the bytes). `run()` subscribes to the same broadcast the pump
  reads, so both get every chunk. Recorded because it is the in-process
  equivalent of the wire path and rules out a whole class of "the pump stole
  the output" worry for later phases.
