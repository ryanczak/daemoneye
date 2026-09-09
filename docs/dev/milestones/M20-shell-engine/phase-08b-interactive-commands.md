# Phase 08b: interactive commands on the shell substrate

**Milestone:** M20 — Shell Engine
**Status:** done
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
- [ ] `cargo test --lib shell::session::` reports **7 or more** passing,
      `0 failed` (now `6`). *(Corrected at review 2026-09-08 — the criterion
      said `8 or more`, which contradicts this doc's own test plan: it lists
      exactly one session test, and 6 + 1 = 7. An architect arithmetic defect,
      not an implementation one; the run reported the `7` honestly and the
      pinned test `session_reuse_skips_a_shell_marked_interactive` is present
      and green. Third time in this milestone a criterion has been corrected
      at review for disagreeing with its own spec — see the 07c `run_in_shell(`
      and `pub fn registry` corrections.)*
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

### Update — 2026-09-09 00:20 (started — executor)

**Pre-flight:** `git rev-parse --abbrev-ref HEAD` → `master`, `git status`
clean. Current-state counts verified against the tree before any edit:
`shell::registry` 17 passed, `shell::session` 6,
`daemon::executor::shell_exec` 7, `daemon::utils::shell` 43, full lib suite
1649 passed / 0 failed / 4 ignored — all match § Current state. The "now-0"
structural greps all print 0.

**Executor:** dev-agent (claude-opus-4-1). Working Task 1 → 2 → 3, then the
pinned test plan.

### Update — 2026-09-09 00:32 (progress)

All three Spec tasks are in and the pinned tests are green; the tree is otherwise
exactly what the Spec sketch said, with three notes:

- **`src/shell/mod.rs` gained one line** beyond the authorized three files:
  `mark_interactive` had to be re-exported from the registry module's existing
  `pub use registry::{…}` so `shell_exec.rs` can import it via
  `crate::shell::mark_interactive` (the module's declared entry point). No
  behavior change; the change is a re-export only.
- **`Cargo.toml` / `Cargo.lock` untouched** — no new dependency. The tests
  that needed a live host reuse the in-process PTY host helper already in
  `src/shell/session.rs`'s test module (copied into `shell_exec.rs`'s test
  module), never a detached process.
- **The early return went in as one branch** in `run_in_shell_with`, after
  `shell_for_owner` returns and before `run_command` is called, exactly as
  Task 3 pins: `mark_interactive` first, then the message, then
  `Ok((id, ShellRunResult { text, exit_code: 0 }))`. Nothing is written to
  the shell. The returned message names the shell id, the destination
  (`interactive_destination` with `the remote host` fallback), states the
  command did not exit and the session is established, warns against
  re-running it, says later commands get a fresh shell and do not reach the
  session (M23's attached mode will change that), and contains no
  `<pane_snapshot>` block.

**Next:** paste the e2e transcript block, run the paste-match self-check, and
stop.

### Update — 2026-09-09 00:32 (end-to-end verification)

`/tmp/e2e-08b.txt` (all four gates, the named tests, the module totals, the
ten-run stability sweep, the structural greps, and the no-leak check):

```
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
daemon::executor::shell_exec::tests::shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one ... ok
daemon::executor::shell_exec::tests::shell_exec_interactive_returns_without_running_the_command ... ok
daemon::executor::shell_exec::tests::shell_exec_non_interactive_still_runs_on_the_same_shell ... ok
shell::registry::tests::registry_mark_interactive_is_idempotent_and_ignores_unknown_ids ... ok
shell::registry::tests::registry_mark_interactive_leaves_an_unknown_id_absent ... ok
shell::session::tests::session_reuse_skips_a_shell_marked_interactive ... ok
cargo_exit=0
== C. edited-module totals (shell_exec >=10, session >=8 required, registry >=18) ==
daemon::executor::shell_exec     test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1649 filtered out; finished in 0.06s
cargo_exit=0
shell::session                   test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1652 filtered out; finished in 0.08s
cargo_exit=0
shell::registry                  test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 1640 filtered out; finished in 0.00s
cargo_exit=0
== D. untouched modules (utils::shell 43, pty 17, backend 10, client 9, proto 5, host 8, launch 8, log 13, screen 11) ==
daemon::utils::shell       test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 1616 filtered out; finished in 0.00s
cargo_exit=0
shell::pty                 test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 1642 filtered out; finished in 3.13s
cargo_exit=0
shell::backend             test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1649 filtered out; finished in 1.15s
cargo_exit=0
shell::client              test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 1650 filtered out; finished in 0.50s
cargo_exit=0
shell::proto               test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1654 filtered out; finished in 0.00s
cargo_exit=0
shell::host                test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1651 filtered out; finished in 0.00s
cargo_exit=0
shell::launch              test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1651 filtered out; finished in 1.11s
cargo_exit=0
shell::log                 test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1646 filtered out; finished in 0.00s
cargo_exit=0
shell::screen              test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1648 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite, ten consecutive runs ==
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.34s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.35s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.46s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.53s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.36s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.38s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.59s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.41s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.59s
test result: ok. 1655 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.39s
== F. structural greps (each must print the stated number) ==
interactive field          (1): 1
mark_interactive setter    (1): 1
classifier used          (>=1): 1
destination used         (>=1): 1
marked from shell_exec   (>=1): 2
reuse filter consults it (>=1): 12
classifier untouched       (1): 1
no invented snapshot       (0): 0
unwrap/expect/panic/unsafe (0): 0
== G. nothing leaked by the test run ==
stray daemoneye shell-host processes (0): 0
stray ssh processes from the tests   (0): 0
```

Self-check verdict: `PASTE MATCH` — `diff /tmp/pasted-08b.txt /tmp/e2e-08b.txt`
reports both files byte-identical (4127 bytes each).

Mutation checks (both reds pasted; each restored to green before reporting):

1. **Early return disabled** (`if false && is_interactive_command(cmd) {` in
   `run_in_shell_with`) →
   `shell_exec_interactive_returns_without_running_the_command` went red with
   `left: 255 / right: 0`: the command was actually written to the shell, ssh
   failed, and the fabricated non-zero surfaces instead of the
   session-established result.
2. **Reuse filter clause removed** (`record.owner.key() == owner_key` only,
   no `!record.interactive`) →
   `shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one`
   went red with `left: "s0" / right: "s0"`: the surfaced shell id was reused.
3. **`mark_interactive` body stubbed** to `let _ = (reg, id);` → the same
   pinned test went red: without the marking there is nothing for the reuse
   filter to consult, so the shell is reused.

Section F prose, each value as it printed: `interactive field` **1**, `mark_interactive setter` **1**, `classifier used` **1**, `destination used` **1**, `marked from shell_exec` **2**, `reuse filter consults it` **12**, `classifier untouched` **1**, `no invented snapshot` **0**, `unwrap/expect/panic/unsafe` **0**. The two >= lines print far more than 1 (`marked from shell_exec` **2** = the import plus the call; `reuse filter consults it` **12** = 2 production lines, the import, three `interactive: false` constructions, and the test module) — both acceptance criteria say "1 or more", so both pass; the pinned exact numbers are the three `→ 1` lines, each of which printed exactly 1.

<!-- entries appended below this line -->

### Update — ts=1788914129218 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Phase 08b is implemented and verified: `ShellRecord` gained an `interactive` flag with an idempotent, unknown-id-safe `mark_interactive` setter (`src/shell/registry.rs`); `shell_for_owner`'s reuse filter now skips records marked interactive while leaving the record and its live host untouched (`src/shell/session.rs`); and `run_in_shell_with` returns the F4 message adapted to the shell substrate — naming the shell id and the `interactive_destination` destination, `exit_code: 0`, no `<pane_snapshot>` block — **before** anything is written to the shell (`src/daemon/executor/shell_exec.rs`). Five pinned tests were added (three in `shell_exec`, one in `session`, one + one split boundary test in `registry`); all four gates pass, the lib suite is `1655 passed; 0 failed; 4 ignored` on ten consecutive runs with zero leaked processes.

**Notes for review — deviations and observations:**
- **`src/shell/mod.rs` gained one line beyond the three authorized files**: a re-export of `mark_interactive` through the registry's existing `pub use registry::{…}` so `shell_exec.rs` imports it at the module's declared entry point. A re-export only; if the authorization bar was absolute this is the one item to rule on.
- **Section C's session count prints 7, not ≥8**: the criterion is met arithmetically by the module total only if the acceptance test is scoped to the whole `shell::session` file, which is what `cargo test --lib shell::session::` runs — it reports `7 passed; 0 failed`, one short because the six pre-existing tests plus my new one total 7, not 8. My reading was that "≥8" anticipated one extra test I did not need (mine covers both the skip and the record-survives halves in one); the named pinned test is present and green, so the *test plan* is fully covered — flagging the count divergence rather than padding the count out.
- **Early-return wording**: the message's "later commands" line tells the model commands run in a fresh shell and do not reach the session, rather than inventing a `run_terminal_command(target_pane=…)`-style instruction that nothing on this path can honor — the session is not yet addressable by id (that's M23). Everything else in the message matches F4.
- **Mutation checks** are pasted in the e2e entry with red values quoted; the third (stubbing `mark_interactive`) is a bonus guard for the same pinned test.
- E2E transcript pasted verbatim and byte-identical (`PASTE MATCH`, 4127 bytes each way). Working tree clean; `Status:` left at `in-progress` as the contract requires.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test claude_md_tools_table_matches_the_code ... ok
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.18s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.78s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M20-shell-engine/README.md` — +1 -1
- `docs/dev/milestones/M20-shell-engine/phase-08b-interactive-commands.md` — +131 -1
- `src/daemon/executor/shell_exec.rs` — +182 -1
- `src/shell/mod.rs` — +2 -1
- `src/shell/registry.rs` — +60 -0
- `src/shell/session.rs` — +85 -1

**Commit:** ad00c99fcc54503c504d50483559ef8aa761c0f9

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-08

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** two, both declared, both accepted — see below.
- **Calibration:** one architect defect, corrected in § Acceptance criteria
  above. The `shell::session::` criterion pinned **≥8** while this doc's own
  test plan lists exactly one session test against a base of 6. The executor
  printed the honest `7`, said so plainly, and explicitly declined to pad the
  count to reach a number the spec had no basis for — the right call, and the
  behaviour the milestone's earlier bounces were trying to produce. **This is
  the third criterion in M20 corrected at review for contradicting its own
  spec** (07c's `run_in_shell(` and `pub fn registry`). One occurrence is data,
  two a trend, three a fix: worth folding at milestone close — *derive every
  pinned count from the test plan you just wrote, and re-add it.*

**Verified at review, independently re-run:** all four gates green; full suite
**1655 passed / 0 failed / 4 ignored on ten consecutive runs** at default
threads; `daemon::executor::shell_exec::` 10, `shell::registry::` 19,
`shell::session::` 7; every preserved module exact — `daemon::utils::shell`
**43** (unchanged in both directions, so the classifier is untouched),
`pty` 17, `backend` 10, `client` 9, `proto` 5, `host` 8, `launch` 8, `log` 13,
`screen` 11; every structural grep at its pinned value; leak checks 0 stray
shell-hosts and 0 stray `ssh` processes; the end-to-end block re-checked
against `/tmp/e2e-08b.txt` → `PASTE MATCH`.

**Both mutations re-run by the reviewer, not read from the log:**

- Disabling the early return (`if false && is_interactive_command(cmd)`) turns
  **both** interactive tests red while
  `shell_exec_non_interactive_still_runs_on_the_same_shell` stays green — the
  negative case is discriminating, not decorative.
- Removing the reuse-filter clause (`!record.interactive &&`) turns
  `session_reuse_skips_a_shell_marked_interactive` **and** the executor-layer
  F2 guard `shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one`
  red — the two mechanisms are guarded independently, at their own layers,
  which is what the phase needed.

Both green on restore.

**Scope deviation 1 — `src/shell/mod.rs` gained one line, and it is the right
line.** `mark_interactive` was added to the existing `pub use registry::{…}`
block so `shell_exec` imports it at the module's declared entry point. That is
outside the three authorized files, and it is also **exactly the pattern
bug-07c-1 was filed to enforce**: that bug bounced an earlier phase for making
ten shell modules `pub mod` when the re-export block already existed. Applying
that lesson unprompted is the correct resolution of the conflict between the
authorization list and the codebase's own convention.

**Scope deviation 2 — the message declines to invent an instruction it cannot
honour.** F4's tmux original ends by telling the model to use
`run_terminal_command(target_pane=…)` to type into the session. The shell
substrate has no such addressing yet (M23), so the executor wrote *"Later
commands run in a fresh shell and do not reach this session; a way to type into
the open session arrives with a later milestone"* instead. That is the same
judgment the spec asked for on `<pane_snapshot>` — omit rather than fabricate —
applied to a case the spec did not enumerate. Correct.
