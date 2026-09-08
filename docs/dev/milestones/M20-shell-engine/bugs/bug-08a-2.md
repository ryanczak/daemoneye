# Bug 2 on phase-08a: a job can be paused only once — the second STOP is a silent no-op

**Severity:** major
**Status:** open
**Filed:** 2026-09-08

## What's wrong

Pause works exactly once per job. After a pause/resume cycle the job keeps
running and can never be paused again; the `STOP` returns `Ok(())` having done
nothing, so nothing surfaces the failure.

Measured at review, one `bash`, one `sleep 30`, through the shipped API
(probe compiled into `src/shell/pty.rs`, run, reverted):

```
PROBE shell=613121 job=613268
PROBE 1_running fg=Some(613268) has_fg=true  job_state=S
PROBE 2_paused  fg=Some(613121) has_fg=false job_state=T paused_pgid=Some(613268)
PROBE 3_resumed fg=Some(613121) has_fg=false job_state=S resumed_alive=Some(613268)
PROBE 4_pause2  fg=Some(613121) has_fg=false job_state=S paused_pgid=None
                                             ^^^^^^^^^^^^^ still running, nothing remembered
```

Line 4 is the bug: a second `signal_foreground("STOP")` against a job that is
demonstrably alive and running (`job_state=S`, and `status()` reports `Running`
for it) leaves it running and remembers nothing.

## What should happen

`Ctrl-p` is a **toggle** — `docs/design/daemoneye-2.0.md` line 371: *"`Ctrl-p`
pauses the shell … `Ctrl-p` again resumes"*. A user pausing, resuming, and
pausing again is the ordinary use of the control this phase builds the
mechanism for, and this phase's Goal is to make a job *"pausable, resumable and
cancellable"* — not pausable once.

The same asymmetry was already found and fixed on the read side during this
phase. `status()` cannot see a resumed job through the terminal either, so
Task 3 consults the remembered group (`backend.rs:158`,
`shell.resumed_pgid_alive()`). The write side kept the terminal-only rule and
so cannot signal the job it just resumed.

Note the consequence for the state model as well: with `paused_pgid` cleared
and `resumed_pgid` still set, `resumed_pgid_alive()` accepts `/proc` state `T`
(`pty.rs:~410`), so a resumed job stopped by anything else reports `Running`.
Making the second `STOP` reach the resumed group closes that too, because rule
2 fires again once the group is remembered.

## Root cause

`signal_foreground`'s `"STOP"` arm (`src/shell/pty.rs:352-363`) derives its
target only from `current_foreground_pgid()`, and treats "the foreground group
is the shell's own pgid" as *"there is no job to pause"*. After a resume that
premise is false: bash holds the terminal while the resumed job runs in the
group this shell remembers (`resumed_pgid`) — measured at line 3 of the probe
above, `has_fg=false` with the job in state `S`.

This is architect-origin. The phase doc's Task 2 wrote the rule as *"If it is
`None`, or equals the shell's own pid (F1 — no job in the foreground), do
nothing"*, and F1 was measured only on a **fresh** job that had never been
stopped, so it never saw the state a resume leaves behind. The executor
implemented the spec as written. The gap is still real, user-visible, and
fixable only here.

## Definition of done

Each was run against the current tree and returns the "now" value shown.

- [ ] A test named `pty_pause_resume_pause_toggles_more_than_once` exists and
      passes: one `bash`, one `sleep 30`, then STOP → CONT → **STOP** again,
      asserting the job reaches `/proc` state `T` a second time and
      `paused_pgid()` is `Some` again after it. Poll for each transition — do
      not assert instantly (bug-08a-1).
      `grep -c 'fn pty_pause_resume_pause_toggles_more_than_once' src/shell/pty.rs`
      → **1** (now `0`).
- [ ] `cargo test --lib shell::pty::` reports **17 or more** passing,
      `0 failed` (now `16`).
- [ ] **`status()` follows the toggle end to end**, through `PtyBackend`, not
      `/proc` directly: a test asserts the sequence `Running → Paused →
      Running → Paused` across STOP/CONT/STOP. `cargo test --lib
      shell::backend::` reports **10 or more** passing, `0 failed` (now `9`).
- [ ] **The single-`process_group_leader`-call-site criterion still holds:**
      `grep -c 'process_group_leader' src/shell/pty.rs` → **1** (now `1`). The
      fix consults the *remembered* group, exactly as `CONT` and
      `resumed_pgid_alive` already do — it does not add a second tty query.
- [ ] **The existing negative case still holds:**
      `pty_stop_with_no_foreground_job_remembers_nothing` still passes. A
      `STOP` on a shell that has never run a job must still do nothing and must
      not signal the shell's own group — remembering a *resumed* group is not
      the same as inventing one.
- [ ] All four gates pass, and `cargo test --lib` reports `0 failed` on ten
      consecutive runs (bug-08a-1's gate, which this work must not reopen).
