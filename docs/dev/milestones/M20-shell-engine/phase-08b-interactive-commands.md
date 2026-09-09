# Phase 08b: interactive commands on the shell substrate

**Milestone:** M20 — Shell Engine
**Status:** todo
**Depends on:** phase-07c (`run_in_shell`), phase-07b (`shell_for_owner`) —
both done. Independent of 08a; touches none of its signal code.
**Estimated diff:** ~280 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Stop an interactive command — `ssh host`, `mosh box` — from **permanently
poisoning** a chat session's shell.

Today, with `[execution] backend = "pty"`, `ssh host` is written into the
owner's shell, the remote session opens, the end marker never arrives, and
`run_command` times out after 45 s. That much was expected. What was not:
**the shell stays captured, and every later command in that session times out
too** (F2 below — measured). The session's only shell is gone for good.

This phase makes an interactive command return **immediately**, naming the
shell it is connected on, and stops that shell being handed to the next
command.

## Architecture references

- `docs/design/daemoneye-2.0.md` line 158-161: interactive commands are routed
  **heuristically** — *"`is_interactive_command()` (kept from
  `utils/shell.rs`) routes `ssh user@host` to `open_shell(transport: Ssh)`"*.
  The `open_shell` transport is later work; this phase is the routing and the
  shell bookkeeping only.
- `docs/design/daemoneye-2.0.md` line 168: non-interactive `ssh -T` exec is
  therefore **not** this path — the classifier already excludes it (F3).

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- The counts in § Current state must match before you start. If any differs,
  stop and say so.

## Current state

Counts today: `shell::registry` **17**, `shell::session` **6**,
`daemon::executor::shell_exec` **7**, `daemon::utils::shell` **43**. Full lib
suite: **1649 passed; 0 failed; 4 ignored**.

`grep -c 'is_interactive_command' src/daemon/executor/shell_exec.rs` → **0**:
nothing on the shell path consults the classifier.

### What already exists and must not be rewritten

```rust
// src/daemon/utils/shell.rs:39 — the classifier, already correct (F3).
pub fn is_interactive_command(cmd: &str) -> bool;
// src/daemon/utils/shell.rs:88 — the destination, for the message.
pub fn interactive_destination(cmd: &str) -> Option<String>;

// src/shell/registry.rs:74 — the record this phase adds one field to.
pub struct ShellRecord { pub id: ShellId, pub owner: Owner, pub started_unix: u64 }

// src/shell/registry.rs:108 — the ONLY way into the map. Not reentrant:
// never call it again inside its own closure.
pub fn with_shells<T>(reg: &ShellRegistry, f: impl FnOnce(&mut HashMap<String, ShellRecord>) -> T) -> T;
```

## Measured facts — executed 2026-09-08, not reasoned about

Probes compiled into `src/shell/session.rs`, run, then reverted. A real
in-process host, a real `bash`.

**F1 — a command that never returns to the prompt times out with no partial
output.** `cat` stands in for an interactive session (it holds the terminal
and never reaches the next prompt), with a 3 s timeout standing in for the
45 s `LOCAL_CMD_TIMEOUT` so the probe is cheap:

```
PROBE elapsed=3.001363195s is_err=true
PROBE err=Some("timed out after 3s waiting for command output: \"cat\"")
```

An `Err`, not a partial `CommandOutcome` — so today an `ssh host` produces
07c's error path, which tells the model *"the command may still be running —
do not assume it failed outright"*. For an interactive session that is
actively misleading: the session **did** connect and is usable.

**F2 — the load-bearing discovery: the shell is poisoned for good.** Same
owner, after the timeout above:

```
PROBE same_shell=true id1=s0 id2=s0
PROBE next_cmd elapsed=5.000840069s ok=Err("timed out after 5s waiting for command output: \"echo ALIVE\"")
```

`shell_for_owner` hands back **the same shell** — correctly, by its own
contract: the host is alive, so the flock probe says reuse it. But the
interactive process owns the terminal and eats every byte written to it, so
the marker-wrapped command never reaches `bash`. **Every subsequent
`run_terminal_command` in that chat session times out at 45 s, forever.**

This is why returning early is not enough on its own. An early return that
leaves the shell reusable produces exactly the same poisoning one command
later.

**F3 — the classifier is already right, including both negatives.**

```
PROBE is_interactive("ssh host")=true          PROBE is_interactive("ssh -N host")=false
PROBE is_interactive("ssh user@host")=true     PROBE is_interactive("ssh host uptime")=false
PROBE is_interactive("mosh box")=true          PROBE is_interactive("ls")=false
PROBE dest(ssh user@host)=Some("user@host")
```

Do **not** touch `is_interactive_command` or `interactive_destination`. They
have 43 tests in `daemon::utils::shell` and this phase adds none.

**F4 — the tmux path already has the message to copy.**
`src/daemon/executor/foreground.rs:911-930` returns this for an interactive
command, and the shell path's answer should read the same way with the shell
id in place of the pane:

```rust
format!(
    "[Interactive session started]\n\
     `{cmd}` opened an interactive session in pane \
     {target_str} — now connected to {destination}.\n\
     The command did not exit; the pane is running an \
     interactive shell on the remote host.\n\
     ...
     Do NOT call `{cmd}` again — the session is already \
     established.\n\
     <pane_snapshot>\n{pane_snap}\n</pane_snapshot>"
)
```

**There is no snapshot to include on this path.** The shell substrate has no
`capture_pane`; reading the live grid is a separate M20 exit criterion (07c
§ Out of scope). Omit the `<pane_snapshot>` block rather than inventing one.

## Spec

### Task 1 — `src/shell/registry.rs`: mark a shell as captured

Add one field to `ShellRecord`:

```rust
/// Set when an interactive command (ssh, mosh, …) took this shell's
/// terminal. Such a shell still passes the liveness probe — its `bash` is
/// alive — but every byte written to it is consumed by the interactive
/// process, so it must never be handed to another command (F2).
pub interactive: bool,
```

`false` at every existing construction site (`registry.rs:174`,
`registry.rs:289`, `session.rs:268`, `session.rs:314`, `session.rs:322`, and
the test helper at `registry.rs:322`). Add one setter beside `with_shells`:

```rust
/// Mark `id`'s shell captured by an interactive session. Idempotent; a no-op
/// when the id is not in the registry.
pub fn mark_interactive(reg: &ShellRegistry, id: &ShellId);
```

### Task 2 — `src/shell/session.rs`: never reuse a captured shell

In `shell_for_owner`'s reuse step, a record with `interactive == true` is not
a reuse candidate. The current filter is:

```rust
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| record.owner.key() == owner_key)
            .map(|record| record.id.clone())
            .collect();
```

Extend that predicate only. **Do not** remove the record and do not kill the
host: the session is still running and later work (M23 attached mode) will
address it by id. The owner simply gets a fresh shell for its next command,
which is what the per-owner cap is for.

Note the consequence, and let it happen: an owner that opens
`max_per_owner` interactive sessions can no longer allocate. That is the cap
working as designed, and the error it produces already names the cap.

### Task 3 — `src/daemon/executor/shell_exec.rs`: return before running

In `run_in_shell_with`, **after** `shell_for_owner` returns and **before**
`run_command` is called, take the interactive path when
`crate::daemon::utils::is_interactive_command(cmd)`:

1. `mark_interactive(reg, &id)`.
2. Return `Ok((id, ShellRunResult { text, exit_code: 0 }))` where `text` is
   the F4 message adapted: name **the shell id**, not a pane; name the
   destination from `interactive_destination(cmd)`, falling back to
   `"the remote host"`; say the command did not exit and the session is
   established; and tell the model not to call `cmd` again. Omit the
   `<pane_snapshot>` block (F4).

`exit_code` is `0` because nothing failed — the session opened. It must not
be a fabricated non-zero, and `exit_status_annotation(Some(0))` returns
`None`, so a clean interactive result carries no status note.

**The command is never written to the shell.** Returning after writing it
would poison the shell exactly as F2 measured; the whole point is that the
write does not happen.

**Do not** touch `format_for_model`, the `CONT`/`STOP` code from 08a, or
`run_in_shell`'s signature.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `grep -c 'pub interactive: bool' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c 'pub fn mark_interactive' src/shell/registry.rs` → **1**
      (now `0`).
- [ ] `grep -c 'is_interactive_command' src/daemon/executor/shell_exec.rs` →
      **1 or more** (now `0`), and
      `grep -c 'interactive_destination' src/daemon/executor/shell_exec.rs` →
      **1 or more** (now `0`).
- [ ] `grep -c 'mark_interactive' src/daemon/executor/shell_exec.rs` → **1 or
      more** (now `0`).
- [ ] **The reuse filter consults the flag:**
      `grep -c 'interactive' src/shell/session.rs` → **1 or more** (now `0`).
- [ ] **The classifier is untouched (F3):**
      `grep -c 'pub fn is_interactive_command' src/daemon/utils/shell.rs` →
      **1** (now `1`), and `cargo test --lib daemon::utils::shell::` reports
      **43** passing, `0 failed` (now `43`) — *preservation criteria, the
      count must not change in either direction.*
- [ ] **No snapshot was invented (F4):**
      `grep -c 'pane_snapshot' src/daemon/executor/shell_exec.rs` → **0**
      (now `0`) — a *preservation* criterion: it passes today and must still
      pass after your change. The shell substrate has no `capture_pane`, so
      copying F4's message verbatim would mean fabricating the block.
- [ ] No `unsafe`, `unwrap`, `expect` or `panic!` outside test code in the
      three edited files:
      `for f in src/shell/registry.rs src/shell/session.rs src/daemon/executor/shell_exec.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0**. *(`.unwrap_or_log()` is the project's poisoned-lock idiom and
      does not match this pattern — CLAUDE.md § Important Invariants.)*
- [ ] `cargo test --lib daemon::executor::shell_exec::` reports **10 or more**
      passing, `0 failed` (now `7`).
- [ ] `cargo test --lib shell::session::` reports **8 or more** passing,
      `0 failed` (now `6`).
- [ ] `cargo test --lib shell::registry::` reports **18 or more** passing,
      `0 failed` (now `17`).
- [ ] Unchanged: `shell::pty::` **17**, `shell::backend::` **10**,
      `shell::client::` **9**, `shell::proto::` **5**, `shell::host::` **8**,
      `shell::launch::` **8**, `shell::log::` **13**, `shell::screen::` **11**.
- [ ] `cargo test --lib` reports `0 failed` on **ten consecutive runs** at the
      default thread count — no `RUST_TEST_THREADS` cap, no skip filter. *(Ten,
      not three: bug-08a-1 was a 27% flake that a three-run sweep passes on
      luck 39% of the time.)*
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not. Any test that spawns a PTY must hold
`crate::test_home_guard()` bound to a named variable (`_home`, not bare `_`)
across the spawn (bug-05c-1) and substitute the spawn closure with an
in-process host — `src/shell/session.rs`'s test module has the helper to copy.

**Do not use a real `ssh` in any test.** There is no host to reach and the
network is not available to the suite. The classifier keys on the command
*string*, so `"ssh nowhere.invalid"` exercises the whole path without a
connection — nothing is ever written to the shell (Task 3), which is exactly
the property under test.

- `shell_exec_interactive_returns_without_running_the_command` — **the
  primary-use test.** `run_in_shell_with` on `"ssh nowhere.invalid"` against a
  real in-process host returns in well under `LOCAL_CMD_TIMEOUT` (assert an
  elapsed bound, e.g. under 5 s — the point is that it did not wait 45 s),
  with `exit_code == 0` and text containing `nowhere.invalid`. This must be
  **red against a build that runs the command anyway**: without Task 3 it
  takes 45 s and returns `Err`.
- `shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one`
  — **the F2 regression guard, and the reason this phase exists.** Same owner,
  twice: first `"ssh nowhere.invalid"`, then `"echo ALIVE"`. Assert the two
  calls report **different** `ShellId`s and that the second returns `ALIVE`
  with `exit_code == 0`. Asserting only the marking would pass an
  implementation that still handed back the poisoned shell.
- `shell_exec_non_interactive_still_runs_on_the_same_shell` — **the negative
  case.** `"echo one"` then `"echo two"` for one owner: same `ShellId` both
  times, both outputs correct. Guards against the filter excluding shells it
  should not.
- `session_reuse_skips_a_shell_marked_interactive` — at the `shell_for_owner`
  layer, no executor: mark a record interactive, then ask for that owner's
  shell and assert a different id comes back and the marked record is **still
  in the registry** (not removed, not killed).
- `registry_mark_interactive_is_idempotent_and_ignores_unknown_ids` — the
  boundary cases the setter's contract names: marking twice leaves it marked,
  and marking an id that is not in the map is a no-op rather than an insert.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-08b.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test .*(shell_exec_interactive|shell_exec_non_interactive|session_reuse_skips|registry_mark_interactive).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. edited-module totals (shell_exec >=10, session >=8, registry >=18) =="
for m in daemon::executor::shell_exec shell::session shell::registry; do
  printf '%-32s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== D. untouched modules (utils::shell 43, pty 17, backend 10, client 9, proto 5, host 8, launch 8, log 13, screen 11) =="
for m in daemon::utils::shell shell::pty shell::backend shell::client shell::proto shell::host shell::launch shell::log shell::screen; do
  printf '%-26s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, ten consecutive runs =="
for r in 1 2 3 4 5 6 7 8 9 10; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "interactive field          (1): "; grep -c 'pub interactive: bool' src/shell/registry.rs
echo -n "mark_interactive setter    (1): "; grep -c 'pub fn mark_interactive' src/shell/registry.rs
echo -n "classifier used          (>=1): "; grep -c 'is_interactive_command' src/daemon/executor/shell_exec.rs
echo -n "destination used         (>=1): "; grep -c 'interactive_destination' src/daemon/executor/shell_exec.rs
echo -n "marked from shell_exec   (>=1): "; grep -c 'mark_interactive' src/daemon/executor/shell_exec.rs
echo -n "reuse filter consults it (>=1): "; grep -c 'interactive' src/shell/session.rs
echo -n "classifier untouched       (1): "; grep -c 'pub fn is_interactive_command' src/daemon/utils/shell.rs
echo -n "no invented snapshot       (0): "; grep -c 'pane_snapshot' src/daemon/executor/shell_exec.rs
echo -n "unwrap/expect/panic/unsafe (0): "
for f in src/shell/registry.rs src/shell/session.rs src/daemon/executor/shell_exec.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
echo "== G. nothing leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
echo -n "stray ssh processes from the tests   (0): "
ps -eo args | awk '/^ssh nowhere\.invalid$/ {n++} END{print n+0}'
} > /tmp/e2e-08b.txt 2>&1
cat /tmp/e2e-08b.txt
```

Paste the contents of `/tmp/e2e-08b.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-08b-interactive-commands.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08b.txt
diff /tmp/pasted-08b.txt /tmp/e2e-08b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
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

**Mutation-check your two guards before reporting**, and paste the red and
green `test result:` lines for each:

1. Remove Task 3's early return (let the command run) —
   `shell_exec_interactive_returns_without_running_the_command` must go red.
2. Remove Task 2's filter clause (let a marked shell be reused) —
   `shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one`
   must go red.

Both are verified independently at review; a claimed mutation check is not one.

**A `0` in section F can be an error, not a measurement.** Every line of
section F must carry a number on the *same* line as its label; a `grep: … No
such file or directory` followed by a bare `0` is a failure, not a pass.

## Authorizations

You may edit, and nothing else:

- `src/shell/registry.rs` — the `interactive` field, `mark_interactive`, and
  `false` at the existing construction sites (Task 1)
- `src/shell/session.rs` — the reuse predicate, and `interactive: false` at its
  three construction sites (Task 2)
- `src/daemon/executor/shell_exec.rs` — the early-return branch (Task 3)
- this phase doc's Update Log

**Do not** change `is_interactive_command`, `interactive_destination`,
`format_for_model`, `run_command`, `client.rs`, `pty.rs`, `backend.rs`,
`proto.rs`, `host.rs`, `launch.rs`, `log.rs`, `screen.rs`, or anything 08a
touched in the signal paths.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.** Use `find <dir> -depth -delete` for
cleanup, never `rm -rf`.

## Out of scope

- **Exited-shell GC under `shells.exited_retention_secs`.** Moved to
  **phase-09** at drafting (2026-09-08). It needs a second `ShellRecord` field
  (when the exit was first observed — `sweep_stale` reaps immediately today and
  records no timestamp), and `src/config/lifecycle.rs:110` already says the
  shell-log sweep *"lands with the registry in phase-09"*. It is bookkeeping
  with no dependency on this phase's work, and 09 already owns lifecycle.
- **Addressing an interactive session after it opens.** The tmux path offers
  `run_terminal_command(target_pane=…)`; the shell substrate's equivalent is
  addressing by shell id, which belongs with M23's attached mode. This phase's
  message tells the model the session exists and not to re-run the command; it
  does not offer a way to type into it.
- **Closing or killing a captured shell.** The record stays, the host stays
  alive. Reaping is phase-09.
- **`open_shell(transport: Ssh)`** — the design's eventual routing target
  (line 160). This phase keeps the session inside the local shell it opened.
- **Reading the live vt100 grid** for a snapshot — a separate M20 exit
  criterion (07c § Out of scope), which is why F4's `<pane_snapshot>` block is
  omitted rather than faked.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
