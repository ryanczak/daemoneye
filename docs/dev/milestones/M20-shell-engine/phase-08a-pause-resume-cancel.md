# Phase 08a: pause, resume and cancel on the shell substrate

**Milestone:** M20 — Shell Engine
**Status:** in-progress
**Depends on:** phase-05b (`PtyBackend`), phase-05a (`proto.rs` frame set),
phase-07a (`client::run_command`) — all done.
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make a shell's foreground job **pausable, resumable and cancellable**, and make
`Status` tell the truth about which of those states it is in.

Today the wire protocol already carries `Signal { STOP | CONT | INT | TERM }`
and `PtyBackend::signal` already delivers it. What does **not** work is the
half that matters: **a stopped job cannot be resumed**, because the resume
signal is sent to the wrong process group (F2 below — measured, not reasoned).
And `Status` reports `Running` for a shell that is stopped, idle, or busy
alike, because `ShellState` has only `Running` and `Exited`.

This phase is the **mechanism only**. No AI tool, no chat UX, no key bindings.

## Architecture references

- `docs/design/daemoneye-2.0.md` § M20 row (line 543): M20 owns *"pause/resume/
  cancel signalling"*; the attached-mode `Ctrl-p` / `Ctrl-c` UX that drives it
  is **M23** (line 546). Build the mechanism, not the interface.
- `docs/design/daemoneye-2.0.md` line 452: *"Pause and cancel are user-only
  controls … no tool can pause or resume."* So nothing in this phase is
  reachable from `src/ai/`, and no `ToolDef` is added.
- `docs/design/daemoneye-2.0.md` line 371: the state a paused shell reports is
  `Paused`, and a run owning it *"parks at its next turn boundary"* — the
  parking is M24's, the `Paused` state it reads is this phase's.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- The counts in § Current state must match before you start. If any differs,
  stop and say so.

## Current state

Counts today: `shell::proto` **5**, `shell::pty` **13**, `shell::backend`
**7**, `shell::host` **8**, `shell::client` **6**, `shell::registry` **17**,
`shell::session` **6**, `shell::launch` **8**, `shell::log` **13**,
`shell::screen` **11**. Full lib suite: **1639 passed; 0 failed; 4 ignored**.

`grep -rc 'Paused' src/shell/*.rs` is **0** in every file — the state does not
exist yet.

### What already works, and must not be rewritten

```rust
// src/shell/proto.rs:5-32 — the frame set. Signal already carries all four.
pub enum ShellSignal { Int, Term, Stop, Cont }   // serde renames: "INT" "TERM" "STOP" "CONT"
pub enum ShellState  { Running, Exited }         // <- this phase adds Idle and Paused
pub enum ShellRequest { Subscribe, Input{..}, Resize{..}, Signal{signal}, Status }

// src/shell/host.rs:165 — dispatch is done; do not touch it.
ShellRequest::Signal { signal } => match backend.signal(signal).await { .. }

// src/shell/backend.rs:122 — delivery is done.
async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()> {
    self.shell.lock().unwrap_or_log().signal_foreground(signal_name(sig))
}
```

## Measured facts — executed 2026-09-08, not reasoned about

Probes compiled into `src/shell/pty.rs`, run, then reverted. A real `bash` in a
real PTY, `sleep 30` as the foreground job.

**F1 — the foreground pgid is how you tell busy from idle.**

```
PROBE shell_pid=147364
PROBE fg_pgrp_while_running=Some(147511)      <- differs from the shell pid: a job is running
PROBE fg_pgrp_after_INT=Some(147364)          <- equals the shell pid: no job, shell is idle
```

`master.process_group_leader()` returns the **job's** pgid while a job holds
the terminal, and the **shell's own pid** when none does. That equality is the
`Idle` / `Running` discriminator. There is nothing else to consult.

**F2 — the load-bearing defect: a stopped job cannot be resumed today.**

```
PROBE fg_before_stop=Some(148623)
PROBE fg_AFTER_stop=Some(148476)   <- the SHELL's pid, not the job's
PROBE after_STOP=T
PROBE after_CONT_t0=T   t1=T   t2=T   t3=T   t4=T   t5=T     <- 3 s, still stopped
PROBE after_second_CONT=S     <- a CONT aimed at the REMEMBERED pgid 148623 resumed it
```

Read this closely, it is the whole phase:

1. `SIGSTOP` to the foreground group works — the job goes to state `T`.
2. **The moment it stops, bash takes the terminal back**, so
   `process_group_leader()` now returns the *shell's* pgid.
3. `signal_foreground("CONT")` re-reads `process_group_leader()` and therefore
   sends `CONT` **to the shell's own group**. The stopped job never receives
   it and stays `T` indefinitely.
4. The identical `kill -s CONT -- -<pgid>` aimed at the **remembered** job pgid
   resumed it immediately.

So resume **cannot re-derive its target**. The shell must remember the pgid it
stopped. This is not a style preference — a `CONT` that re-reads the foreground
group is a no-op against a stopped job, every time.

**F3 — the shell's own `/proc` state is not the job's.**

```
PROBE shell_state_char_after_STOP=S     <- the shell is NOT stopped
PROBE job_state_char_after_STOP=T       <- the job is
PROBE is_alive_after_STOP=Ok(true)
```

`is_alive()` is `try_wait().is_none()` (`pty.rs:246`) and a stopped child has
not exited, so it stays `true`. **Do not** try to detect `Paused` from the
shell pid or from `is_alive` — read the state of the **remembered job pgid**,
whose `/proc/<pgid>/stat` state char is `T`.

**F4 — `INT` already works and leaves the shell usable.**

```
PROBE fg_pgrp_after_INT=Some(147364)   (== shell pid: the job is gone)
PROBE is_alive_after_INT=Ok(true)
PROBE run_after_INT=Ok(("STILL_ALIVE\r\n", 0))
```

Cancel needs no new mechanism — it needs the state bookkeeping to be cleared
so the next `Status` reads `Idle` rather than `Paused`.

**F5 — parsing `/proc/<pid>/stat` safely.** The comm field is parenthesised and
may itself contain spaces and parens, so the state char is the first token
**after the last `)`**, never `split_whitespace().nth(2)`:

```rust
let s = std::fs::read_to_string(format!("/proc/{pgid}/stat"))?;
let state = s.rsplit(')').next().unwrap_or("").split_whitespace().next();
```

That is the exact form the probes used to produce every reading above.

## Spec

### Task 1 — `src/shell/proto.rs`: two more states

```rust
pub enum ShellState {
    #[serde(rename = "Idle")]    Idle,
    #[serde(rename = "Running")] Running,
    #[serde(rename = "Paused")]  Paused,
    #[serde(rename = "Exited")]  Exited,
}
```

Nothing else in this file changes. `ShellSignal` already has all four signals.

### Task 2 — `src/shell/pty.rs`: remember the stopped group

`signal_foreground` currently re-derives its target every call (`pty.rs:328`).
Give `PtyShell` one new private field — the pgid it stopped, `Option<i64>`,
`None` when nothing is paused — and make the four signals use it:

- **`STOP`**: read `process_group_leader()`. If it is `None`, or equals the
  shell's own pid (F1 — no job in the foreground), do nothing and leave the
  remembered pgid `None`: there is no job to pause. Otherwise signal that
  group and **remember it**.
- **`CONT`**: target the **remembered** pgid, not `process_group_leader()`
  (F2). If nothing is remembered, do nothing. Clear it after signalling.
- **`INT` / `TERM`**: unchanged in target (the live foreground group), but
  **clear** the remembered pgid — after a cancel there is nothing paused
  (F4).

Keep the existing `kill(1)` invocation shape and the `pgid > 0` guard exactly
as they are (`pty.rs:332-341`); this task changes *which* pgid is passed and
what is recorded, not how the signal is sent.

Add one public accessor for Task 3, and one predicate:

```rust
/// The process group this shell stopped, if a job is currently paused.
pub fn paused_pgid(&self) -> Option<i64>;

/// Whether a foreground job holds the terminal — `process_group_leader()`
/// present and different from the shell's own pid (F1).
pub fn has_foreground_job(&self) -> bool;
```

### Task 3 — `src/shell/backend.rs`: `status()` tells the truth

`PtyBackend::status` (`backend.rs:130-151`) currently maps `is_alive` to
`Running` / `Exited` and nothing else. Extend it to the four-way answer, in
this precedence order:

1. not alive → `Exited`
2. a paused pgid is remembered **and** `/proc/<pgid>/stat` reports state `T`
   → `Paused` (F3, F5)
3. a foreground job holds the terminal → `Running`
4. otherwise → `Idle`

Rule 2 checks `/proc` rather than trusting the remembered pgid alone, so a job
that was resumed or killed by something outside this process does not leave the
shell reporting `Paused` forever. On any `/proc` read error treat the job as
**not** paused and fall through to rules 3–4 — a missing `/proc` entry means
the process is gone, which is not a paused state.

`rows`, `cols` and `pid` in the `Status` response are unchanged.

### Task 4 — `src/shell/client.rs`: two callable operations

`run_command` (`client.rs:24`) is the file's only public function. Add two,
built the same way — connect, write one frame, read one response frame:

```rust
/// Send `sig` to the shell hosted at `sock`. Returns when the host has
/// acknowledged, which is not the same as the job having reacted.
pub async fn send_signal(sock: &Path, sig: ShellSignal, timeout: Duration)
    -> anyhow::Result<()>;

/// The shell's current state, rows, cols and pid.
pub async fn status(sock: &Path, timeout: Duration)
    -> anyhow::Result<ShellResponse>;
```

An `Error` frame from the host is an `Err` here, carrying the host's message.
Neither function subscribes — they are single round-trips, not output streams.

**Do not** change `run_command`, and do not route these through it.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `grep -c 'Paused' src/shell/proto.rs` → **1 or more** (now `0`), and
      `grep -c 'Idle' src/shell/proto.rs` → **1 or more** (now `0`).
- [ ] `grep -c 'pub fn paused_pgid' src/shell/pty.rs` → **1** (now `0`), and
      `grep -c 'pub fn has_foreground_job' src/shell/pty.rs` → **1** (now `0`).
- [ ] **Resume targets the remembered group, not the live one (F2).** In
      `src/shell/pty.rs`, the `CONT` path must not consult
      `process_group_leader()`:
      `grep -c 'process_group_leader' src/shell/pty.rs` → **1** (now `1` — the
      count must not grow; `STOP` keeps the single existing call site and
      `CONT` reads the remembered field).
- [ ] `grep -c 'pub async fn send_signal' src/shell/client.rs` → **1**
      (now `0`), and `grep -c 'pub async fn status' src/shell/client.rs` →
      **1** (now `0`).
- [ ] `grep -c '/proc/' src/shell/backend.rs` → **1 or more** (now `0`) — the
      `/proc` read that distinguishes `Paused` (F3). *(Pinned on `/proc/`, not
      on `stat`: `grep -c 'stat' src/shell/backend.rs` prints **20** against
      today's tree because it matches `status` and `state`, so that criterion
      would pass without a line being written. Two phases in this milestone
      have already been bounced for exactly this prefix fragility — bug-07b-1
      and the 07c criteria corrected at review.)*
- [ ] **`run_command` is untouched:**
      `grep -c 'pub async fn run_command' src/shell/client.rs` → **1**, and its
      body's own comment anchor survives verbatim —
      `grep -c 'Subscribe before writing' src/shell/client.rs` → **1** (now
      `1`). *(Preservation criteria. Do not compare against `master` with
      `git diff`: `master` **is** the working branch here, so
      `git diff master -- <file>` is empty whatever you changed and would
      certify anything.)*
- [ ] **`host.rs` signal dispatch is untouched (the frames already exist):**
      `grep -c 'ShellRequest::Signal { signal } => match backend.signal(signal).await' src/shell/host.rs`
      → **1** (now `1`). The only change this phase may make to `host.rs` is a
      `ShellState` match arm the new variants force.
- [ ] No `unsafe` in non-test code across the three edited files:
      `for f in src/shell/pty.rs src/shell/backend.rs src/shell/client.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code in the same three files:
      `for f in src/shell/pty.rs src/shell/backend.rs src/shell/client.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**. *(`.unwrap_or_log()` is the project's poisoned-lock idiom and does
      not match this pattern — see CLAUDE.md § Important Invariants.)*
- [ ] `cargo test --lib shell::pty::` reports **16 or more** passing, `0 failed`
      (now `13`).
- [ ] `cargo test --lib shell::backend::` reports **9 or more** passing,
      `0 failed` (now `7`).
- [ ] `cargo test --lib shell::client::` reports **7 or more** passing,
      `0 failed` (now `6`).
- [ ] Unchanged: `shell::proto::` **5 or more** (now `5`), `shell::host::`
      **8**, `shell::registry::` **17**, `shell::session::` **6**,
      `shell::launch::` **8**, `shell::log::` **13**, `shell::screen::` **11**.
- [ ] `cargo test --lib` reports `0 failed` on **three consecutive runs** at the
      default thread count — no `RUST_TEST_THREADS` cap, no skip filter.
      *(bug-07a-1: a phase's own PTY-spawning tests turned this gate red 19 runs
      in 20 while every per-module count stayed green.)*
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not. Real-PTY tests spawn `bash` and must hold
`crate::test_home_guard()` bound to a named variable (`_home`, not bare `_`)
across the spawn (bug-05c-1).

**Use `sleep 30` as the foreground job** — it is what every probe above used,
it holds the terminal without reading it, and it never completes on its own
inside a test's lifetime.

- `pty_pause_then_resume_actually_restarts_the_job` — **the primary-use test
  and the F2 regression guard.** Spawn `bash`, `sleep 30`, `STOP`, assert the
  job's `/proc` state is `T`; then `CONT` and assert it returns to `S` or `R`
  **within a bounded poll** (poll, do not sleep once and hope). This test must
  be **red against a `CONT` that re-reads `process_group_leader()`** — that is
  precisely the shipped-today behaviour F2 measured, so write it so reverting
  Task 2's `CONT` change turns it red, and say in the Update Log that you ran
  it that way and saw it fail.
- `pty_stop_with_no_foreground_job_remembers_nothing` — the **negative** case:
  `STOP` on an idle shell (no job) leaves `paused_pgid()` `None` and does not
  signal the shell's own group. Assert the shell still runs a command
  afterwards — a `STOP` that hit the shell itself would hang the next command.
- `pty_cancel_clears_the_paused_group` — `sleep 30`, `STOP`, then `INT`:
  `paused_pgid()` is `None` afterwards and the shell still answers
  `echo STILL_ALIVE` with exit code 0 (F4).
- `backend_status_reports_paused_then_running` — through `PtyBackend::status`,
  not through `/proc` directly: `Idle` before any job, `Running` with
  `sleep 30` in the foreground, `Paused` after `STOP`, and `Running` again
  after `CONT`. One state per assertion with its own message.
- `backend_status_is_idle_on_a_fresh_shell` — the state a shell reports before
  anything has run. Guards the rule-4 fallthrough.
- `client_send_signal_round_trips` and `client_status_reports_state` — against
  a **fake** `ShellBackend` over a real socket (the pattern `shell::host`'s
  tests already use — no PTY needed for these two).

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-08a.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test .*(pty_pause_|pty_stop_with_|pty_cancel_|backend_status_|client_send_signal_|client_status_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. edited-module totals (pty >=16, backend >=9, client >=7) =="
for m in shell::pty shell::backend shell::client; do
  printf '%-18s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== D. untouched modules (proto>=5, host 8, registry 17, session 6, launch 8, log 13, screen 11) =="
for m in shell::proto shell::host shell::registry shell::session shell::launch shell::log shell::screen; do
  printf '%-18s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "Paused in proto          (>=1): "; grep -c 'Paused' src/shell/proto.rs
echo -n "Idle in proto            (>=1): "; grep -c 'Idle' src/shell/proto.rs
echo -n "paused_pgid accessor       (1): "; grep -c 'pub fn paused_pgid' src/shell/pty.rs
echo -n "has_foreground_job         (1): "; grep -c 'pub fn has_foreground_job' src/shell/pty.rs
echo -n "process_group_leader calls (1): "; grep -c 'process_group_leader' src/shell/pty.rs
echo -n "client send_signal         (1): "; grep -c 'pub async fn send_signal' src/shell/client.rs
echo -n "client status              (1): "; grep -c 'pub async fn status' src/shell/client.rs
echo -n "run_command still one fn   (1): "; grep -c 'pub async fn run_command' src/shell/client.rs
echo -n "proc read in backend     (>=1): "; grep -c '/proc/' src/shell/backend.rs
echo -n "run_command body preserved (1): "; grep -c 'Subscribe before writing' src/shell/client.rs
echo -n "host signal dispatch kept  (1): "; grep -c 'ShellRequest::Signal { signal } => match backend.signal(signal).await' src/shell/host.rs
echo -n "no unsafe pre-test         (0): "
for f in src/shell/pty.rs src/shell/backend.rs src/shell/client.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
for f in src/shell/pty.rs src/shell/backend.rs src/shell/client.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\('
echo "== G. no shell-host or stray job leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
echo -n "stray 'sleep 30' from the tests      (0): "
ps -eo args | awk '/^sleep 30$/ {n++} END{print n+0}'
} > /tmp/e2e-08a.txt 2>&1
cat /tmp/e2e-08a.txt
```

Paste the contents of `/tmp/e2e-08a.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-08a-pause-resume-cancel.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08a.txt
diff /tmp/pasted-08a.txt /tmp/e2e-08a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
`PASTE MATCH` is not optional and has no "by design" exemption: commentary,
interpretation and any argument that a failure is pre-existing belong in prose
**outside** the fenced block.

**Your prose about section F must state each grep's value as it printed.**
bug-07b-1 was filed on an earlier phase in this milestone for a transcript that
honestly printed `2` under prose claiming "every structural grep prints its
pinned number". The transcript is the evidence; a sentence summarising it is a
claim that has to match.

**Section G is a leak check, and `sleep 30` is why it exists.** A test that
stops a job and never resumes or kills it leaves a stopped `sleep` on the host
after `cargo test` exits. Both counts must be `0`; if they are not, your tests
are not cleaning up their jobs. **Do not** clean up with `pkill`, `killall`, or
any signal to a process group you did not create — kill the specific child.

**A `0` in section F can be an error, not a measurement.** Every line of
section F must carry a number on the *same* line as its label; a `grep: … No
such file or directory` followed by a bare `0` is a failure, not a pass.

## Authorizations

You may edit, and nothing else:

- `src/shell/proto.rs` — the two new `ShellState` variants (Task 1)
- `src/shell/pty.rs` — the remembered-pgid field, the four signal paths, and
  the two new accessors (Task 2)
- `src/shell/backend.rs` — `status()`'s four-way answer (Task 3)
- `src/shell/client.rs` — `send_signal` and `status` (Task 4); **additions
  only**, `run_command`'s body does not change
- `src/shell/host.rs` — **only** if the new `ShellState` variants force a match
  arm. No change to signal dispatch.
- this phase doc's Update Log

**Do not** change `run_command`, `registry.rs`, `session.rs`, `launch.rs`,
`log.rs`, `screen.rs`, anything under `src/daemon/` or `src/ai/`, or the
`ShellSignal` enum.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.** Your tests signal only process
groups they created and can name. Use `find <dir> -depth -delete` for cleanup,
never `rm -rf`.

## Out of scope

- **Any AI tool, and any chat UX.** Design § line 452: pause and cancel are
  user-only controls; no tool can reach them. The `Ctrl-p` / `Ctrl-c` /
  `Ctrl-d` attached mode that drives this mechanism is **M23**.
- **`is_interactive_command` routing** — an `ssh` through `run_terminal_command`
  still hits `LOCAL_CMD_TIMEOUT` today. That is **phase-08b**.
- **Exited-shell GC under `shells.exited_retention_secs`** — also 08b.
- **Parking a run at its turn boundary when its shell pauses** — that needs the
  `Run` model, which is **M24** (design line 371).
- **Non-Linux `/proc`.** The `Paused` detection in Task 3 reads `/proc`; this
  project targets Linux and every other phase in M20 already assumes it.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Update — 2026-09-08 10:20 (progress)

Executor started phase-08a (headless dispatch). Pre-flight verified: on
`master`, tree clean, `git rev-parse --abbrev-ref HEAD` → `master`. All ten
Current-state module counts reproduced exactly before any edit (pty 13,
backend 7, client 6, proto 5, host 8, registry 17, session 6, launch 8, log
13, screen 11), and `grep -rc 'Paused' src/shell/*.rs` is 0 everywhere — the
state does not exist yet, as the spec states. Status flipped to `in-progress`
here and in the milestone README's phase table. Tasks 1–4 next, in order.

<!-- entries appended below this line -->

### Update — 2026-09-08 11:25 (blocker)

**Blocked on:** Task 3's fourth status leg ("`Running` again after `CONT`",
pinned in `backend_status_reports_paused_then_running`) is unimplementable
under the spec's own rule precedence and its own measured facts — and meeting
it would require editing `launch.rs`, which the Authorizations forbid.

**What I tried, in order:**

1. Implemented Tasks 1, 2 and 3 exactly as specified. Task 3's four-way
   precedence (not-alive → Exited; remembered-pgid with `/proc` state `T` →
   Paused; foreground job → Running; else Idle) compiles and its first three
   legs hold in tests.
2. Ran the pinned `backend_status_reports_paused_then_running`. Legs 1–3 pass
   (Idle→Running→Paused). The fourth leg, `Running` after `CONT`, never
   satisfies: `status()` reads `Idle` at every poll across 10 s.
3. Instrumented `status()` under a temp env-gated print, then deleted it:
   after `CONT`, `tcgetpgrp(master)` reads the **shell's own pid**, and the
   remembered pgid is consumed, so rule 2 is false, rule 3 is false, rule 4
   fires → `Idle`. Not an implementation race — the value is stable for
   seconds.
4. Re-measured outside the crate with a scratch `portable-pty` probe
   (same `bash` + `sleep 30` + the same `kill -s STOP -- -<pgid>` /
   `kill -s CONT -- -<pgid>` shape): after `CONT` the job's `/proc/<pgid>/stat`
   state returns to `S` (it **did** resume), but `fg==shell_pid` stays true at
   every sample across 7 s, and writing further input (`echo probed`) does not
   change it. So bash takes the terminal when the job stops and **`CONT` does
   not give it back** — exactly consistent with the spec's own F1/F2 probes
   (`fg_pgrp_after_INT=Some(shell_pid)`, `after_second_CONT=S` while the fg
   pgrp is the shell's).
5. Checked the escape routes the spec itself excludes: `has_foreground_job()`
   is F1's discriminator and there is "nothing else to consult" (spec, F1). Reporting `Running` after `CONT` would require consulting something else
   (e.g. remembering that a resume happened) — a design change beyond Task 3's
   pinned rules.
6. Interviewed the rest of the tree: `launch_run_host_serves_status_over_its_socket`
   (`launch.rs:302`) pins the **old** two-way semantic by asserting the fresh
   shell's `Status` reply carries `state: Running`. Task 3 changed exactly
   that semantic (a fresh idle shell now reports `Idle`), so this test now
   fails on every `cargo test --lib` run. `launch.rs` is in the phase's
   "Do not change" list, so I cannot fix it; without the edit the criterion
   "`shell::launch::` 8, 0 failed" cannot hold.

So the phase doc carries two contradictions that cannot both be satisfied:
(a) F1+the four rules versus the pinned test's fourth leg, and (b) the
Authorizations' launch.rs prohibition versus the unchanged-count criterion
once Task 3's semantic change lands.

**What I need:** a decision on both:

1. What should `status()` report after a successful `CONT`? Measured truth:
   the job resumed (`/proc` state `S`) but does **not** own the terminal —
   bash kept it. Per the spec's rules that is `Idle`; to report `Running`
   means adding a fourth input to the discriminator (e.g. treat
   "a job is alive in the group last stopped" as `Running` for one resume
   generation), which is a design change. Whoever decides should also
   reconcile the `pty_pause_then_resume_actually_restarts_the_job` wording
   (which only pins the job's `/proc` state after `CONT`, and passes today).
2. Authorization to touch the one `ShellState::Running` assertion in
   `launch.rs`'s readiness test (it is a *readiness* probe; its shape —
   status answered in time — survives the change; only the pinned state
   literal went stale), or to have the architect edit it.

Work already landed on `master`-to-be (committed as part of this phase's WIP
commit): Task 1 (two new `ShellState` variants + proto tests), Task 2
(remembered-pgid STOP/CONT/INT/TERM paths + `paused_pgid()` +
`has_foreground_job()`, the F2 regression test red-verified against a
re-derived `CONT`), Task 4 (`client::{send_signal, status}` + three hermetic
socket tests; `run_command` byte-untouched). Task 3's `status()` implements
the pinned four rules with the `/proc`-guarded `Paused` check. The one test
that cannot pass is the fourth leg above, plus launch's stale literal.

### Update — 2026-09-08 11:30 (progress)

F2 red-leg verification done as the spec demands: with `CONT` temporarily
mutated to re-derive its target from `tcgetpgrp` (the shipped-today behaviour
F2 measured), `pty_pause_then_resume_actually_restarts_the_job` went red with
"the remembered job to resume (state S or R) after CONT not reached within the
wait" — the job stayed `T`. Correct `CONT` restored; test green again (3/3 in
`pause_resume_tests`). The blocker above stands unmodified and unchanged in
scope.

### Update — 2026-09-08 11:37 (progress)

Pre-completion gate re-checked as instructed; the failure set is exactly the
two covered by the blocker above and nothing new: 1645 passed; 2 failed; 4
ignored. `cargo build` clean, `cargo clippy --all-targets --all-features -- -D
warnings` green, `cargo fmt --all --check` green. No further code movement is
possible inside this phase's Authorizations: the fourth status leg needs a
decision (see blocker item 1) and the launch.rs literal needs write access or
an architect edit (see blocker item 2). Stopping here with Status left
`in-progress`, as the contract directs for a blocker.

### Update — 2026-09-08 11:38 (end-to-end verification — PARTIAL, see blocker)

No end-to-end entry is written because STANDARDS §1 boxes cannot all be
ticked while two acceptance criteria fail; per the contract a blocked phase
stops at the blocker and the E2E capture belongs to the round that lands.
For the record, the greps that were pinned with "each must print the stated
number" and that the landed code does satisfy, captured mechanically from the
committed tree (`6c047b7`):

```
Paused in proto          (>=1): 1
Idle in proto            (>=1): 1
paused_pgid accessor       (1): 1
has_foreground_job         (1): 1
process_group_leader calls (1): 1
client send_signal         (1): 1
client status              (1): 1
run_command still one fn   (1): 1
proc read in backend     (>=1): 1
run_command body preserved (1): 1
host signal dispatch kept  (1): 1
no unsafe pre-test         (0): 0
unwrap/expect/panic pre-test (0): 0
```

Every F line prints its pinned number — stated here one by one to satisfy the
prose requirement that beat phase-07b (bug-07b-1). Module counts on the
committed tree: `shell::proto` 5, `shell::host` 8, `shell::registry` 17,
`shell::session` 6, `shell::launch` **7 passed / 1 failed** (the blocked
literal), `shell::log` 13, `shell::screen` 11, `shell::pty` 16, `shell::`
backend 8 passed / 1 failed (the blocked leg), `shell::client` 9. Three
consecutive `cargo test --lib` runs: 1645 passed, 2 failed (both above), 4
ignored — the third-run flakeClass of bug-07a-1 is not in play; the same two
fail every run deterministically, matching the blocker's measurements.

### Update — 2026-09-08 11:41 (blocker, addendum)

The pre-completion gate re-failed on the same two tests and nothing else. The
blocker's core contradiction can be stated in one line now, on the phase doc's
own pinned tests: `backend_status_is_idle_on_a_fresh_shell` (guards rule 4)
demands `Idle` for a fresh shell, while `launch_run_host_serves_status_over_its_socket`
(`launch.rs:302`, outside this phase's write scope) demands `Running` for the
same fresh-shell `Status` — both cannot pass, for any implementation of
Task 3. And F1/F2's own probe measurements (bash reclaims the terminal the
instant a job stops; `CONT` does not return it) mean the fourth leg of
`backend_status_reports_paused_then_running` (`Running` again after `CONT`) is
unreachable through rules 1–4 — the job *is* resumed (`/proc` back to `S`),
but rule 3's discriminator reads the shell's own group. Decision needed on:
(a) what `Paused`→`CONT` reports (extra discriminator input, or accept `Idle`
and amend the pinned test), (b) permission to update launch.rs's stale
literal. Code, README row, and status are left as committed at `7f032e7`.
