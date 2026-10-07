# Phase 05a: sbx-signal-plan — tag sandbox shells and build the in-VM signal command

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** done
**Depends on:** phase-04b
**Estimated diff:** ~260 lines (about 90 code, 170 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Pausing a sandbox shell has to happen **inside the VM**: stopping the host's
`sbx exec` client freezes nothing (measured). This row adds the pure pieces
the host will use in row 05b:

- a **tag** on each sandbox shell, so a side `sbx exec` can find that shell's
  bash when one sandbox holds several shells;
- the **measured in-VM script** that finds the shell and stops, resumes,
  hangs up or reports on it;
- the argv to run it;
- parsers for its output;
- a parser that recognises a host's own sbx child argv.

Nothing calls these yet, except that `spawn_sbx_host` starts tagging its
shells.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Measured on pinky",
  rows *Pause* and *Cancel*.
- `src/shell/pty.rs` `signal_foreground`: the host-side pause rule this
  script mirrors inside the VM. Stop the shell first, then the job; resume
  the job, then the shell.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Measured on pinky, 2026-10-06 (architect probes, sandboxes removed)

Two shells were started in **one** sandbox, as row 05b's host will start
them:

```
sbx exec -it -e DE_SHELL=s3 <sandbox> bash
sbx exec -it -e DE_SHELL=s4 <sandbox> bash
```

Each ran a tick loop typed at its prompt. The script below was run with
`sbx exec <sandbox> sh -c "<script>" vmsig <tag> <OP>`:

| Probe | Output / result |
|---|---|
| `FIND s3`, `FIND s4` | `bash=378 fg=409`, `bash=367 fg=439`: different shells, each found |
| `FIND s9` (no such tag) | stdout `no-shell`, **exit 3** |
| `STOP s3` | `stopped bash=378 fg=598`; s3's loop **frozen** for 2 s, while s4's loop **kept running** |
| `CONT s3` | `cont bash=378 fg=598`; s3 resumed |
| `kill -INT -- -<fg>` in-VM on s4's loop | the loop **did not end**: bash runs on after its `sleep` child dies. So cancel is **not** an in-VM signal; it is `^C` written into the PTY (phase 01 measured that this ends the loop). Row 05b does that |
| nested `setsid -w bash -c 'sleep 30' &` inside s3 (a second tagged session leader, pid 420) | `FIND s3` still `bash=370`: the parent-tag check skips the nested leader |
| `FIND` idle / running / paused | `bash=366 fg=366 state=S` / `bash=366 fg=414 state=S` / `bash=366 fg=450 state=T` |
| `HUP s3` on a paused loop | `hup bash=366`; the loop ended; afterwards `FIND s3` gives `no-shell` (exit 3); the host's `sbx exec` client **exited with code 129** |
| `sh -n` on the script | passes under dash (`/bin/sh` on pinky) and bash |

### The script (measured verbatim; do not edit)

```sh
id=$1; sig=$2; bash_pid=""
for p in /proc/[0-9]*; do
  pid=${p#/proc/}
  tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" || continue
  # stat: pid (comm) state ppid pgrp session tty_nr tpgid ...
  rest=$(sed 's/.*) //' "$p/stat" 2>/dev/null) || continue
  set -- $rest; state=$1; sid=$4; tpgid=$6
  [ "$sid" = "$pid" ] || continue
  ppid=$2
  tr '\0' '\n' < "/proc/$ppid/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" && continue
  bash_pid=$pid; fg=$tpgid; st=$state; break
done
[ -n "$bash_pid" ] || { echo "no-shell"; exit 3; }
case $sig in
  FIND) echo "bash=$bash_pid fg=$fg state=$st";;
  HUP) kill -CONT "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -CONT -- "-$fg"; kill -HUP "$bash_pid"; echo "hup bash=$bash_pid";;
  STOP) kill -STOP "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -STOP -- "-$fg"; echo "stopped bash=$bash_pid fg=$fg";;
  CONT) [ "$fg" != "$bash_pid" ] && kill -CONT -- "-$fg"; kill -CONT "$bash_pid"; echo "cont bash=$bash_pid fg=$fg";;
esac
```

It reads `$1` = the shell's tag and `$2` = one of `FIND`, `STOP`, `CONT`,
`HUP`. It finds the process whose environment holds `DE_SHELL=<tag>`, which
is a **session leader** and whose **parent is not tagged** (the outer bash),
then acts on it and on its terminal's foreground group (`tpgid`).

### The argv builder to copy (`src/daemon/executor/sbx.rs`)

```rust
pub fn shell_exec_args(name: &str, workdir: Option<&str>) -> Vec<String> {
    let mut argv = vec!["exec".to_string(), "-it".to_string()];
    if let Some(dir) = workdir {
        argv.push("-w".to_string());
        argv.push(dir.to_string());
    }
    argv.push(name.to_string());
    argv.push("bash".to_string());
    argv
}
```

`spawn_sbx_host` (`src/shell/session.rs`) calls
`crate::daemon::executor::sbx::shell_exec_args(sandbox, None)`.

## Spec

All new items go in `src/daemon/executor/sbx.rs` unless stated otherwise, and
all are `pub`. The module is `pub mod sbx`, so unused `pub` items do not
warn.

### Task 1 — `tagged_shell_exec_args`

```rust
/// The sandbox shell argv, tagged so a side `sbx exec` can find this shell's
/// bash among the sandbox's other shells:
/// `exec -it -e DE_SHELL=<tag> <name> bash`.
pub fn tagged_shell_exec_args(name: &str, tag: &str) -> Vec<String>
```

`tagged_shell_exec_args("de-ghost-r1", "s3")` must equal
`["exec", "-it", "-e", "DE_SHELL=s3", "de-ghost-r1", "bash"]`.
Leave `shell_exec_args` as it is.

In `spawn_sbx_host` (`src/shell/session.rs`), use
`tagged_shell_exec_args(sandbox, id.as_str())` instead of
`shell_exec_args(sandbox, None)`. That is the only change in that file.

### Task 2 — The script and its argv

```rust
/// The in-VM signal script, measured on sbx v0.46.0 (see the M26 README).
pub const VM_SIGNAL_SCRIPT: &str = r#"…"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmOp { Find, Stop, Cont, Hup }

impl VmOp {
    pub fn as_str(self) -> &'static str  // "FIND" | "STOP" | "CONT" | "HUP"
}

/// `exec <sandbox> sh -c <VM_SIGNAL_SCRIPT> vmsig <tag> <OP>`.
pub fn vm_signal_args(sandbox: &str, tag: &str, op: VmOp) -> Vec<String>
```

- `VM_SIGNAL_SCRIPT` holds the script **exactly** as quoted above,
  byte-for-byte, in a raw string `r#"…"#`. The script contains no `"#`
  sequence, so `r#` is enough. Do not reformat it, re-indent it or drop the
  comment line.
- `vm_signal_args("de-ghost-r1", "s3", VmOp::Stop)` must equal
  `["exec", "de-ghost-r1", "sh", "-c", VM_SIGNAL_SCRIPT, "vmsig", "s3", "STOP"]`.
  `vmsig` becomes `$0`, so the tag is `$1` and the op is `$2`.

### Task 3 — Parse the results

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmShellState { pub bash_pid: u32, pub fg_pgid: u32, pub state: char }

impl VmShellState {
    /// The shell is stopped (`T`).
    pub fn is_paused(&self) -> bool
    /// A job holds the terminal (`fg_pgid != bash_pid`).
    pub fn has_foreground_job(&self) -> bool
}

/// Parse `FIND` output: one line `bash=<u32> fg=<u32> state=<one char>`.
pub fn parse_vm_find(stdout: &str) -> Option<VmShellState>

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmOpOutcome { Done, NoShell, Failed(String) }

/// The outcome of any `vm_signal_args` run. `exit_code` is `None` when the
/// process was killed by a signal.
pub fn vm_op_outcome(exit_code: Option<i32>, stdout: &str, stderr: &str) -> VmOpOutcome
```

- `parse_vm_find`: trim, then require exactly three whitespace-separated
  fields with the prefixes `bash=`, `fg=` and `state=`. The two numbers must
  parse as `u32`, and `state=` must be followed by exactly one char.
  Anything else → `None`. `"no-shell"` → `None`.
- `vm_op_outcome`:
  - exit `Some(0)` → `Done`;
  - exit `Some(3)` **and** trimmed stdout `== "no-shell"` → `NoShell`;
  - anything else → `Failed(<trimmed stderr, or trimmed stdout if stderr is empty, or "exit <code>">)`.

  Exit 3 with other stdout is `Failed`, not `NoShell`. Exit `None` is
  `Failed("killed by a signal")`.

### Task 4 — Recognise a host's own sbx child

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbxTarget { pub sandbox: String, pub tag: String }

/// `Some` when a shell host's `program` + `args` are exactly what
/// `tagged_shell_exec_args` builds: `program`'s file name is `sbx` and
/// `args == ["exec", "-it", "-e", "DE_SHELL=<tag>", <sandbox>, "bash"]`
/// with a non-empty tag and sandbox.
pub fn sbx_target_from_host(program: &str, args: &[String]) -> Option<SbxTarget>
```

- `program` may be a full path (`/usr/bin/sbx`). Compare
  `std::path::Path::new(program).file_name()` with `"sbx"`.
- **Must be `None`** for:
  - `program = "/usr/bin/ssh"` with otherwise matching args;
  - the untagged argv `["exec", "-it", "de-ghost-r1", "bash"]`;
  - `-e FOO=s3` (wrong variable);
  - `-e DE_SHELL=` (empty tag);
  - a seventh argument;
  - `program = "/usr/bin/sbx-helper"`.

### Task 5 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] The ten tests named in § Test plan each pass.
- [ ] `cargo test --lib executor::sbx` reports `0 failed`, and the existing
      22 sbx tests still pass, unedited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/daemon/executor/sbx.rs` tests:

- `tagged_shell_exec_args_match_the_measured_shape` — the vector in Task 1.
- `vm_signal_args_pass_tag_and_op_as_positionals` — the vector in Task 2, for
  each of the four `VmOp`s (the last element is `FIND`/`STOP`/`CONT`/`HUP`).
- `vm_signal_script_is_valid_posix_sh` — run
  `std::process::Command::new("sh").args(["-n", "-c", VM_SIGNAL_SCRIPT]).status()`
  and assert success. `sh -n` parses without executing.
- `vm_signal_script_matches_the_measured_text` — assert the constant contains
  each of these lines exactly:
  - `[ "$sid" = "$pid" ] || continue`
  - `tr '\0' '\n' < "/proc/$ppid/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" && continue`
  - `[ -n "$bash_pid" ] || { echo "no-shell"; exit 3; }`
  - `STOP) kill -STOP "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -STOP -- "-$fg"; echo "stopped bash=$bash_pid fg=$fg";;`

  Write them in the test as raw strings so the backslashes stay literal.
- `parse_vm_find_reads_the_measured_lines` — the three measured lines
  (`bash=366 fg=366 state=S`, `bash=366 fg=414 state=S`,
  `bash=366 fg=450 state=T`) parse. The first is not paused and has no
  foreground job; the second has a job; the third is paused.
- `parse_vm_find_rejects_other_output` — `no-shell`, `""`, `bash=x fg=1 state=S`,
  `bash=1 fg=1`, `bash=1 fg=1 state=ST`, `fg=1 bash=1 state=S` → `None`.
- `vm_op_outcome_maps_exit_codes` — `(Some(0), "stopped bash=1 fg=2", "")` →
  `Done`; `(Some(3), "no-shell\n", "")` → `NoShell`; `(Some(3), "other", "")`
  → `Failed`; `(Some(1), "", "error: sandbox 'x' not found")` →
  `Failed` containing `not found`; `(None, "", "")` → `Failed("killed by a signal")`.
- `sbx_target_from_host_reads_a_tagged_shell` — `/usr/bin/sbx` and `sbx` with
  the tagged argv → `Some(SbxTarget { sandbox: "de-ghost-r1", tag: "s3" })`.
- `sbx_target_from_host_rejects_everything_else` — every negative in Task 4.
- `sbx_target_round_trips_the_builder` —
  `sbx_target_from_host("sbx", &tagged_shell_exec_args("de-chat-x", "s7"))` ==
  `Some(SbxTarget { sandbox: "de-chat-x", tag: "s7" })`.

**Reviewer mutations:** drop the parent-tag `continue` line from
`VM_SIGNAL_SCRIPT` (`vm_signal_script_matches_the_measured_text` fails), and
accept any program name in `sbx_target_from_host`
(`sbx_target_from_host_rejects_everything_else` fails).

## End-to-end verification

```sh
cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
cargo test --lib -- tagged_shell_exec_args_match_the_measured_shape vm_signal_args_pass_tag_and_op_as_positionals vm_signal_script_is_valid_posix_sh vm_signal_script_matches_the_measured_text parse_vm_find_reads_the_measured_lines parse_vm_find_rejects_other_output vm_op_outcome_maps_exit_codes sbx_target_from_host_reads_a_tagged_shell sbx_target_from_host_rejects_everything_else sbx_target_round_trips_the_builder 2>&1 | grep -E '^test |^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**.

## Authorizations

- [x] May change the one `shell_exec_args` call in `spawn_sbx_host`
      (`src/shell/session.rs`) to `tagged_shell_exec_args`.

## Out of scope

- Running the script, or any process spawning from `sbx.rs`.
- Host or daemon wiring: pause/resume/close/status for sandbox shells, and
  `^C` for cancel. That is row 05b.
- Pause-vs-deadline (Q6): row 05c.
- Editing `shell_exec_args` or any existing sbx test.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-07 01:30 (started)

**Executor:** Claude Opus 4.6 (run 05a-1). Pre-flight read: STANDARDS.md,
this phase doc, the M26 README § Measured on pinky. Branch `master` clean at
`962c775a`. Scope confirmed: pure argv builders, parsers and the measured
script in `src/daemon/executor/sbx.rs`, plus the one authorized
`spawn_sbx_host` call swap in `src/shell/session.rs`.


### Update — 2026-10-06 (complete — architect takeover of Task 5)

**Executor for Tasks 1–4:** GLM-5.3-Flash (run `1bce96d8`). It finished the
code and all ten tests, then re-ran the ten-test evidence command **482 times
in 35 minutes** (turn 570) and was stopped on PE instruction. Cause: rexyMCP's
`cargo` output filter collapses `test … ok` lines to the summary line, so the
executor never saw the ten lines the evidence block expects and kept
re-running the command. The read-only exemption kept the identical-call
detector from firing on 482 byte-identical calls. The executor's code is
committed unchanged as `22541ff3`.

**Task 5, architect-run.** Each command below ran once:

```
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 2260 filtered out; finished in 0.00s
test daemon::executor::sbx::tests::parse_vm_find_rejects_other_output ... ok
test daemon::executor::sbx::tests::parse_vm_find_reads_the_measured_lines ... ok
test daemon::executor::sbx::tests::sbx_target_from_host_reads_a_tagged_shell ... ok
test daemon::executor::sbx::tests::vm_op_outcome_maps_exit_codes ... ok
test daemon::executor::sbx::tests::sbx_target_from_host_rejects_everything_else ... ok
test daemon::executor::sbx::tests::tagged_shell_exec_args_match_the_measured_shape ... ok
test daemon::executor::sbx::tests::sbx_target_round_trips_the_builder ... ok
test daemon::executor::sbx::tests::vm_signal_args_pass_tag_and_op_as_positionals ... ok
test daemon::executor::sbx::tests::vm_signal_script_matches_the_measured_text ... ok
test daemon::executor::sbx::tests::vm_signal_script_is_valid_posix_sh ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 2282 filtered out; finished in 0.00s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.43s
fmt_exit=0
passed=2342 failed=0
 docs/dev/milestones/M26-sbx-isolation/README.md    |   2 +-
 .../M26-sbx-isolation/phase-05a-sbx-signal-plan.md |  11 +-
 src/daemon/executor/sbx.rs                         | 379 +++++++++++++++++++++
 src/shell/session.rs                               |   2 +-
 4 files changed, 391 insertions(+), 3 deletions(-)
```

The diffstat is the pre-commit tree. `PASTE MATCH: yes`.

### Review verdict — 2026-10-06

- **Verdict:** escalated
- **Bounces:** none (one `claude_stop` cancel, then takeover of Task 5 only)
- **Executor:** GLM-5.3-Flash (Tasks 1–4); Claude Code (direct) for the
  evidence entry
- **Scope deviations:** none. `VM_SIGNAL_SCRIPT` is **byte-identical** to the
  measured script (987 bytes, compared by the reviewer); the `session.rs`
  change is the one authorised line; test-only `expect`/`panic!`.
- **Reviewer re-run:** fmt and clippy clean; `executor::sbx` 32 passed (22 +
  10); full suite `passed=2342 failed=0` on the first run.
- **Reviewer mutation checks** (each turned its test red, then restored):
  dropping the parent-tag `continue` line from the script
  (`vm_signal_script_matches_the_measured_text`); accepting any program in
  `sbx_target_from_host` (`sbx_target_from_host_rejects_everything_else`).
- **Calibration (rexyMCP):** the `cargo` output filter must not strip
  `test … ok` lines when the command's own `grep` asked for them, and the
  read-only exemption should not cover byte-identical repeats. Third GLM
  evidence-step loop in this milestone (01b ×2, 05a).
