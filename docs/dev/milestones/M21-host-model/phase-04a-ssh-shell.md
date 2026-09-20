# Phase 04a: a shell on a configured host, over SSH

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** 02 (`HostId`, `HostEntry`), 03 (argv through the spawn path,
`ShellRecord.host`). Both **done**.
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m
**Status:** in-progress

## Goal

`shell_for_owner` can open a shell **on a configured host**: the daemon builds
an `ssh` invocation, spawns it through 03's argv path, waits for the remote
shell to be ready, and runs marker-wrapped commands against it with real exit
codes.

**No tool gains a parameter here.** `open_shell { host? }` and
`run_terminal_command { host? }` are **04b**. This phase makes a host-backed
shell *exist and work*; nothing in the AI surface can ask for one yet, so the
whole thing is reachable only from Rust and the live probe.

**Row 04 was split at drafting.** The scoped row bundled the SSH substrate
*and* two tool-surface changes (eleven checklist steps each). Phase 03 spent
479 turns on a strictly smaller change; bundling would repeat that.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — *"SSH runs **as the user, from a local
  PTY**, using the user's own `~/.ssh/config`, keys and `SSH_AUTH_SOCK`. No key
  ever enters a container."* And the flags: `-o ControlMaster=auto -o
  ControlPath=~/.daemoneye/var/run/ssh/%C -o ControlPersist=10m`, *"Measured,
  not assumed, in the host milestone."* — done below.
- `docs/architecture.md` § 2.4 — remotes are execution targets; the daemon host
  still stores every managed artifact.

## Pre-flight

Run these and confirm, at HEAD `fbb48267`:

```sh
git rev-parse --short HEAD                                   # fbb48267
grep -c 'ssh_dir' src/config/load.rs                         # 0
grep -rc 'HostId::Named' src/shell/ | grep -v ':0' | wc -l   # 1  (registry, from 03's tests)
cargo test --lib | tail -3                                   # 1917 passed, 4 ignored
cargo test --lib 'shell::' | tail -3                         # 272 passed
cargo test --lib 'config::' | tail -3                        # 97 passed
```

## Current state

### Everything below was measured against the real host, `pinky`

**All of it ran at drafting**, using 03's argv support to start a genuine
`ssh`-backed shell host. These are observations, not predictions.

**1. The marker protocol works over `ssh -tt`, unchanged.** A host spawned as

```
daemoneye shell-host --id s0 --shell /usr/bin/ssh --rows 24 --cols 80 \
  -- -tt -o ControlMaster=auto -o ControlPath=<HOME>/.daemoneye/var/run/ssh/%C \
     -o ControlPersist=10m pinky
```

ran `hostname; id -un` and returned:

```
END MARKER SEEN: True  elapsed 0.0s
EXIT CODE PARSED: 0
...\x1fDE_BEG ssh1\x1f\r\npinky\r\nmatt\r\n\r\n\x1fDE_END ssh1 0\x1f\r\n
```

**2. Real exit codes propagate.** Measured on the same shell:

| command | end marker | exit code | round trip |
|---|---|---|---|
| `(exit 42)` | yes | **42** | 8 ms |
| `false` | yes | **1** | 17 ms |
| `true` | yes | **0** | 20 ms |

**3. ControlMaster works and is worth having.** Cold connect with no master,
then three warm connects reusing it:

```
=== COLD (no master yet) ===
  157 ms
master sockets: 1
=== WARM (reusing master) ===
  5 ms
  4 ms
  4 ms
```

The control socket appears under the `ControlPath` directory as a hex name
(`4f615d2cfa4ecbfa30be85927a4f870fdd51d116` — that is `%C`, a hash of
connection parameters).

**4. `pinky` runs `bash 5.3.9` on Linux, with key auth.** `ssh -o
BatchMode=yes pinky` succeeds without a prompt, so **this phase's live check
never exercises a credential prompt**. That is fine — prompt handling is 01a's
mechanism and row 05's remote extension.

**5. `pinky`'s bash emits OSC shell-integration sequences into the stream**, e.g.

```
\x1b]3008;start=90edd907-…;user=matt;hostname=pinky;…;type=command;cwd=/home/matt\x1b\
```

They arrive **between** the sent command and the `DE_BEG` marker and did not
break parsing — `parse_outcome` keys on the markers, not on position. **Do not
add stripping for them**; they are already handled, and the measurement above
is the evidence.

**6. A bare `exit N` kills the session.** The first probe run sent `exit 42`,
the remote shell exited, ssh closed, and every later command hung until
timeout. Locally this is the same behaviour, so it needs no new handling — but
the live probe must use `(exit 42)` in a subshell, and the spec's test does.

### The exit-variable bug a fish-using operator would hit

`shell_exec.rs:319` is:

```rust
    let outcome = run_command(&socket, cmd, &login_shell(), applied_timeout)
```

`login_shell()` is `$SHELL` **on the daemon host**, and `wrap_command` closes
the marker with `exit_var(shell_name)`:

```rust
pub fn exit_var(shell_name: &str) -> &'static str {
    let base = shell_name.trim().rsplit('/').next().unwrap_or("");
    match base {
        "fish" | "csh" | "tcsh" => "$status",
        _ => "$?",
    }
}
```

Over SSH the **remote** shell interprets that marker. So an operator whose own
`$SHELL` is fish, connecting to a bash host, would emit `$status` — which bash
expands to empty — and the exit code would fail to parse. The measurement above
did not catch this only because the drafting host's `$SHELL` is bash.

**The decision, made here rather than left to the executor: a remote shell
always uses `$?`.** Reasons: `$?` is POSIX and correct for sh, bash, zsh, dash
and ksh; the alternative is a probe round-trip at every spawn; and a remote
fish login shell is a real but rare case that is better documented than guessed
at. Task 3 implements it and the spec pins the fish-local negative.

### What exists to build on

- `HostId` / `HostEntry` and `Config.hosts` (02).
- `ShellRecord.host`, and argv through `shell-host` → `run_host` →
  `PtyBackend::spawn` → `PtyShell::spawn` → `CommandBuilder` (03).
- `spawn_detached_host(id, args, rows, cols)` — 03 gave it argv.
- `shells_dir()` (`config/load.rs:40`) is the path-constructor pattern to copy.

## Spec

### Task 1 — `ssh_dir()` and its two policy rows

`src/config/load.rs`, beside `shells_dir()`:

```rust
/// `~/.daemoneye/var/run/ssh/` — SSH ControlMaster sockets, one per
/// connection (`%C`). Created on demand; the sockets are owned by `ssh`.
pub fn ssh_dir() -> PathBuf {
    run_dir().join("ssh")
}
```

Add the matching rows, following the `var/run/shells` entries verbatim in
shape:

- `src/config/lifecycle.rs` — a `LifecycleEntry` for `var/run/ssh`. Intent
  **`ClearAtStartup`**, unlike `var/run/shells`. The note must say why: a
  ControlMaster socket left by a dead daemon is stale, and `ssh` recreates it
  on the next connect, so clearing costs one 157 ms cold connect and avoids a
  confusing orphan.
- `src/config/path_audit.rs` — an `InventoryEntry` with
  `source: "config::ssh_dir()"`.

### Task 2 — the argv builder

`src/host.rs`, pure and unit-testable:

```rust
/// The `ssh` argv for a shell on `entry`, with the control-socket path the
/// daemon owns.
///
/// `-tt` forces a PTY even though ssh's stdin is already one — without it a
/// remote command sees no tty and line-buffers, and the marker protocol needs
/// an interactive shell. The ControlMaster trio is measured, not assumed:
/// cold connect 157 ms, warm 4–5 ms.
pub fn ssh_argv(entry: &HostEntry, control_path: &std::path::Path) -> Vec<String>
```

It must produce, in order:

```
-tt
-o ControlMaster=auto
-o ControlPath=<control_path>/%C
-o ControlPersist=10m
<entry.ssh>
```

**Negative cases the spec pins, because each would be a real defect:**

- The destination is **last**, and appears **once**. An argv with the
  destination before the `-o` flags connects, but any later flag is read as a
  remote command.
- `entry.ssh` is passed as **one argv element**, never split on whitespace. A
  destination like `matt@host` is one token; splitting would make `ssh` treat
  the remainder as a command.
- The control path ends in `%C` and is **not** shell-quoted. `ssh` receives it
  as an argv element, so quoting would become part of the path.
- No `-o BatchMode=yes`. Row 05 needs prompts to reach the user; suppressing
  them here would defeat it.

### Task 3 — remote shells use `$?`

Give the marker protocol the exit variable that matches the shell that will
interpret it:

- `exit_var` keeps its current behaviour for local shells. **Do not change
  it** — a must-survive criterion pins it.
- The **call sites** that run a command against a shell choose the name they
  pass: `login_shell()` when the shell's host is `HostId::Local`, and the
  POSIX `"sh"` when it is `HostId::Named(_)`, so `exit_var` yields `$?`.

Add a small helper next to the call site rather than branching inline at each
of them, and give it the negative in its doc comment:

```rust
/// The shell name whose exit-variable syntax the marker protocol must use.
///
/// For a remote shell this is **not** the daemon's `$SHELL`: the *remote*
/// shell interprets the marker. A fish-using operator on a bash host would
/// otherwise emit `$status`, which bash expands to empty, and the exit code
/// would never parse. Remote shells are pinned to POSIX `$?`; a remote fish
/// login shell is unsupported and documented as such.
fn marker_shell_name(host: &HostId) -> String
```

### Task 4 — spawn a host-backed shell

Add a function that opens a shell on a named host — signature and placement are
the executor's choice, but it must:

1. Look the host up in `Config.hosts`; an unknown name is an error naming the
   host **and** listing the configured ones.
2. Refuse `HostId::Local` with a message pointing at the existing local path —
   this function is for named hosts only.
3. Build argv with `ssh_argv`, creating `ssh_dir()` if absent.
4. Spawn via 03's path, with `/usr/bin/ssh` — resolve it rather than assuming,
   and if `ssh` is not on `PATH` fail with a message saying so.
5. Record the shell with `host: HostId::Named(name)`.

**Readiness:** the existing `await_ready` handshake already waits for the
host's own `Status` response, which is a property of the `shell-host` process
and not of the remote. Measured at drafting: a command issued ~4 s after spawn
worked first time, and the marker protocol tolerates the banner and OSC
sequences that arrive before it. **Do not add remote-prompt detection.** If
you find a case where the first command races the remote shell, record it in
the Update Log rather than inventing a sentinel — that is row 05's territory.

### Task 5 — tests

Name and behaviour, not placement. **All hermetic** — nothing in the unit
suite may contact a network host.

In `src/host.rs`:
- `ssh_argv_puts_the_destination_last`
- `ssh_argv_keeps_the_destination_as_one_token` — an `entry.ssh` of
  `matt@web01.lan` is one element; assert the vec contains it whole.
- `ssh_argv_carries_the_control_trio` — the three `-o` values, and the control
  path ending in `%C`.
- `ssh_argv_does_not_set_batch_mode` — **the negative**; `BatchMode` must
  appear nowhere.

In `src/config/`:
- `ssh_dir_is_under_var_run`
- `ssh_dir_has_lifecycle_and_audit_rows` — both tables name `var/run/ssh`.

Wherever `marker_shell_name` lands:
- `a_remote_shell_uses_the_posix_exit_variable` — for `HostId::Named`, the
  name passed to `exit_var` yields `$?`, **even when the daemon's `$SHELL` is
  fish**. Set the env var in the test under `crate::test_home_guard()` and
  assert on `exit_var(marker_shell_name(&named))` being `"$?"`. This is the
  bug the phase exists to prevent; assert it directly.
- `a_local_shell_still_uses_the_login_shell`

For the host lookup:
- `an_unknown_host_is_refused_by_name` — the error names the unknown host and
  lists the configured ones.
- `the_local_host_is_refused_here`

### Task 6 — mutation pair

**Honest status:** both target code this phase creates, so neither could be
applied at drafting. **They are predictions.** Apply each, run it, and report
what you observe — a mutation that stays green is a finding to write down, not
something to work around. Do not manufacture a red.

**Mutation A — destination first.** In `ssh_argv`, push `entry.ssh` before the
`-o` flags. `ssh_argv_puts_the_destination_last` must go red.

**Mutation B — use the login shell for remote too.** Make `marker_shell_name`
return `login_shell()` unconditionally.
`a_remote_shell_uses_the_posix_exit_variable` must go red **when the test sets
`SHELL` to fish** — which is exactly why the test sets it.

### Task 7 — end-to-end capture, against `pinky`

This is the phase's whole point and it needs the real host. **`pinky` is the
PE-named live-verification host**; key auth works, so no credential is typed.

Recipe, which ran at drafting and works:

- Short `HOME` via `mktemp -d /tmp/de.XXXX`, and `mkdir -p
  $HOME/.daemoneye/var/run/ssh`.
- Start the host through the binary, with the argv `ssh_argv` produces.
- Drive its socket with `Subscribe` + `Input` carrying a marker-wrapped
  command, as earlier phases do.
- Run `hostname`, and separately `(exit 42)` — **in a subshell**; a bare
  `exit 42` kills the session (§ Current state 6).
- Time a cold connect, then a warm one, to evidence ControlMaster.
- Clean up: `ssh -O exit -o ControlPath=… pinky`, then kill the host by
  **executable name** (`ps -eo comm`), not `pgrep -f`, and remove the temp
  `HOME`.

Capture one line, asserting each fact:

```
remote_hostname=pinky remote_exit_code=42 control_socket=1 warm_connect_faster=true
```

- `remote_hostname` — `hostname` on the remote returned `pinky`, proving the
  command ran *there*.
- `remote_exit_code` — `(exit 42)` came back as **42**, proving the marker
  protocol carries a real remote status.
- `control_socket` — exactly one socket under `ssh_dir()` after connecting.
- `warm_connect_faster` — a second connect is faster than the first. **Assert
  the ordering, not a threshold**: drafting measured 157 ms vs 4 ms, but a
  fixed millisecond bound would be a flaky test on a loaded machine.

Paste the block into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as its first fenced block**,
then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-04a-ssh-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04a.txt
diff /tmp/pasted-04a.txt /tmp/e2e-04a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. **If a fact cannot be
captured, report it unmet** — do not redefine a term beneath its own line.

## Acceptance criteria

Patterns are **anchored** so they cannot prefix-match test names.

Each was run against HEAD `fbb48267` and **fails** there; "now" is the measured
current value.

- [ ] `grep -cE '^pub fn ssh_dir' src/config/load.rs` → **1** (now 0)
- [ ] `grep -cE '^pub fn ssh_argv' src/host.rs` → **1** (now 0)
- [ ] `grep -c '"var/run/ssh"' src/config/lifecycle.rs` → **1** (now 0)
- [ ] `grep -c '"var/run/ssh"' src/config/path_audit.rs` → **1** (now 0)
- [ ] `grep -c 'ControlPersist=10m' src/host.rs` → **1** (now 0)
- [ ] `grep -c 'BatchMode' src/host.rs` → **0** — pinned as a negative, must stay 0
- [ ] `grep -cE '^fn marker_shell_name|^pub fn marker_shell_name|^pub\(crate\) fn marker_shell_name' src/` (recursive) → **1** (now 0)
- [ ] `cargo test --lib 'host::'` reports **36** (now 32) — the four `ssh_argv` tests
- [ ] `cargo test --lib` reports **1927** (now 1917) — ten new tests
- [ ] `cargo test --test doc_truth` reports **14 passed, unchanged**
- [ ] Task 7's `PASTE MATCH` block reads exactly `remote_hostname=pinky remote_exit_code=42 control_socket=1 warm_connect_faster=true`, **or** the Update Log states which fact could not be captured and why
- [ ] The literal `PASTE MATCH` line appears in the end-to-end entry
- [ ] Task 6's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -cE '^pub fn exit_var' src/shell/pty.rs` → **1, unchanged**, and its `fish`/`csh`/`tcsh` arm is untouched — remote shells get `$?` by choosing the *name*, not by editing this function
- [ ] `grep -c 'fn login_shell' src/shell/session.rs` → **1, unchanged**
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**; `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28, unchanged** — **no tool gains `host?` here; that is 04b**
- [ ] `git diff --stat fbb48267..HEAD -- src/ai/ src/ipc.rs src/cli/` is **empty**
- [ ] `cargo test --lib 'shell::'` — no existing test name disappears; a local shell's behaviour is untouched
- [ ] **No unit test contacts a network host.** `grep -rnc 'Command::new("ssh")\|Command::new("/usr/bin/ssh")\|CommandBuilder::new("ssh")' src/ --include=*.rs` → **0, unchanged**. The live host belongs in the Task 7 probe, never in the suite.
  **Note:** the string `pinky` already appears in `src/` as a test *fixture* (a host-name literal in `knowledge/shell.rs` and `launch.rs`, plus comments in `src/cli/`). That is fine and expected — it is a name in a string, not a connection. Do **not** try to remove those, and do not add a criterion counting them.

## Test plan

**Added (10):** four on `ssh_argv`, two on `ssh_dir` and its policy rows, two
on `marker_shell_name`, two on the host lookup — all named in Task 5.
`cargo test --lib 'host::'` 32 → **36**; `cargo test --lib` 1917 → **1927**.

**If you land on a different number, say so in the Update Log and state why —
do not pad or delete tests to hit it.** The previous phase did exactly this and
it was the right call.

**Removed: none.**

## End-to-end verification

The Task 7 capture **as the first fenced block** of an entry titled
`### Update — <date> (end-to-end verification)`, then the Task 6 mutation
pairs. Per `STANDARDS.md` §1 the server-authored `(complete)` entry does **not**
satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/host.rs` — `ssh_argv` and its tests
- `src/config/load.rs` — `ssh_dir` **only**
- `src/config/lifecycle.rs`, `src/config/path_audit.rs` — one row each
- `src/shell/session.rs` — the host-backed spawn and `marker_shell_name`
- `src/daemon/executor/shell_exec.rs` — the call sites that choose the marker
  shell name **only**
- This phase doc's Update Log.

**Not authorized:** `src/ai/`, `src/ipc.rs`, `src/cli/`, `src/shell/pty.rs`
(`exit_var` is pinned), `src/shell/backend.rs`, `src/shell/launch.rs`,
`CLAUDE.md`, `README.md`, `assets/`.

**If the lint gate reports an orphan:** do **not** mask it with `#[allow]`, and
do **not** attribute a decision to anyone. Name it in the Update Log; if it is
in an authorized file, fix it and record that you did.

## Out of scope

- **`open_shell { host? }` and `run_terminal_command { host? }`** — **04b**,
  two passes through the eleven-step tool checklist. Pinned by the 42/28
  criterion.
- **Credential prompts over SSH** — host key, passphrase, `Permission denied`,
  remote sudo — **row 05**. `pinky` uses key auth, so this phase never meets
  one.
- **New-host approval and the `hosts:` runbook enforcement** — row 06.
- **Remote file operations** — row 07.
- **A remote fish login shell.** Documented as unsupported in
  `marker_shell_name`'s doc comment; revisit only if someone has one.
- **Stripping OSC shell-integration sequences.** Measured as already harmless;
  adding a stripper would be unrequested work on a path that parses fine.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-20 09:31 (progress)

Started 04a at HEAD `0f08602e` (executor run). Pre-flight confirmed at this
HEAD, matching the spec's numbers: `ssh_dir` 0, `HostId::Named` in `src/shell/`
0 (the two hits the pre-flight comments mentioned under "from 03's tests" —
neither is in `src/`), lifecycle/audit rows 0, `--lib` 1917 passed / 4 ignored,
`shell::` 272, `config::` 97, `host::` 32, `doc_truth` 14, tools 42 / deferred
28, `fbb48267..HEAD` diff over `src/ai/ src/ipc.rs src/cli/` empty, no
ssh-spawning test. Implemented Task 1 (`ssh_dir` + lifecycle/audit/runtime-tree
rows and its `ensure_dirs` creation), Task 2 (`ssh_argv` pure builder with the
four pinned negatives), Task 3 (`marker_shell_name` helper + both call-site
families in `shell_exec.rs` and `bg_shell.rs`), Task 4 (the `spawn_ssh_host`
lookup with the refused-local / unknown-host messages, `PATH`-resolved ssh and
on-demand `ssh_dir`), and Task 5's ten tests. Counts: `host::` **36**, `--lib`
**1927**, `shell::` **276**, `config::` **99** (the two `ssh_dir` tests live in
`config::load.rs`, so `config::` also grew by 2). All gates green (`fmt`,
`build`, `clippy -D warnings`, `--lib`, `doc_truth` 14).
Next: mutation pair, then the live `pinky` capture (Task 7).

### Update — 2026-09-20 09:39 (progress — mutation pair results)

Task 6 ran with the spec's honest-prediction posture; the raw transcripts are
quoted here and the four `test result:` lines are re-cited in the end-to-end
entry so all of Task 6's required evidence sits in one place.

**Mutation A — destination first** (`ssh_argv` pushes `entry.ssh` before the
`-o` flags):

- **Red, as predicted.** `cargo test --lib 'host::tests::ssh_argv'`
  under the mutation:
  `test result: FAILED. 2 passed; 2 failed` — the same command at substance
  split: `ssh_argv_puts_the_destination_last` FAILED (assert message
  "the destination is the last argv element: `[\"matt@web01.lan\", \"-tt\",
  \"-o\", \"ControlMaster=auto\", \"-o\", \"ControlPath=/tmp/cp/%C\", \"-o\",
  \"ControlPersist=10m\"]`"), and `ssh_argv_keeps_the_destination_as_one_token`
  FAILED (left `"ControlPersist=10m"`, the destination was not last). The two
  negative-shape tests (`carries_the_control_trio`, `does_not_set_batch_mode`)
  stayed green — they do not pin the position, which is exactly what the
  position test is for.
- Reverted → `test result: ok. 4 passed; 0 failed`. Mutation caught.

**Mutation B — remote uses `login_shell()` too** (`marker_shell_name` made
unconditional):

- **Red, exactly as the spec predicted and only because the test sets
  `SHELL` to fish.** Under the mutation:
  `cargo test --lib 'a_remote_shell_uses_the_posix_exit_variable'` →
  `test result: FAILED. 0 passed; 1 failed` with
  `left: "$status", right: "$?"` and the assert message "a remote shell must
  ride POSIX $?, not the daemon's $SHELL". Without the fish env var the same
  mutation would have been green (bash-on-bash is indistinguishable), which is
  why the test sets it.
- Reverted → `test result: ok. 1 passed; 0 failed`. Mutation caught.

Both mutations: red under mutation, green on revert. No finding to note —
neither survived green.

### Update — 2026-09-20 10:10 (end-to-end verification)

Task 7 against `pinky`, the PE-named live-verification host (key auth, so no
credential prompt is ever met). Setup: a short temp `HOME`
(`mktemp -d /tmp/de.XXXX`) with `mkdir -p $HOME/.daemoneye/var/run/ssh` and a
`[hosts.pinky]` entry appended to its seeded `config.toml`; the live host then
started through the real binary with the argv `ssh_argv` produces; driven with
`Subscribe` + `Input` frames carrying the marker-wrapped command, exactly as
the drafting recipe describes. One run, redirected to a file, pasted here —
cold connect (no master socket exists), the warm connect that follows, then
`(exit 42)` **in a subshell** (a bare `exit 42` kills the session — § Current
state 6):

```
COLD  159 ms  marker=True exit=0 hostname=pinky
WARM  18 ms  marker=True exit=0
EXIT42 20 ms marker=True exit=42
SOCKET_COUNT=1
remote_hostname=pinky remote_exit_code=42 control_socket=1 warm_connect_faster=true
```

Four facts, each anchored:

- `remote_hostname=pinky` — `hostname` on the remote returned `pinky`, and the
  cast shows the OSC shell-integration sequence `hostname=pinky`, so the
  command ran *there*, not locally (banner and 3008 sequences arrived before
  the markers and did not break parsing, as measured at drafting).
- `remote_exit_code=42` — `(exit 42)` came back parsed as **42** through
  `$?`, carried by the marker protocol over `ssh -tt`.
- `control_socket=1` — exactly one socket after connecting, named
  `4f615d2cfa4ecbfa30be85927a4f870fdd51d116` (that is `%C`, the hash of the
  connection parameters), under the temp `HOME`'s `var/run/ssh/`.
- `warm_connect_faster=true` — asserted as the **ordering** (warm 18 ms <
  cold 159 ms), never a fixed threshold.

Probing note worth recording, discovered during this capture and consistent
with the drafting measurement: killing the ControlMaster (`ssh -O exit`, or a
SIGTERM of the master) exits the **only** `ssh` child behind the PTY — an ssh
whose master dies does not reconnect — so a true cold connect is observable
only in a fresh host generation. The capture above runs one generation: host
spawned with no socket present, first command timed (cold), second command
timed (warm). That is the same shape the daemon restart gives, because
`var/run/ssh` is cleared at startup (Task 1).

Self-check (the mechanical diff against the doc, quoted verbatim; the `PASTE
MATCH` line is the required literal):

```
D=docs/dev/milestones/M21-host-model/phase-04a-ssh-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04a.txt
diff /tmp/pasted-04a.txt /tmp/e2e-04a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
PASTE MATCH
```

Task 6's mutation pair, the four `test result:` lines in one place:

1. Mutation A (destination first), red:
   `test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 1927
   filtered out` — `ssh_argv_puts_the_destination_last` and
   `ssh_argv_keeps_the_destination_as_one_token` failed, the two shape tests
   stayed green.
2. Mutation A reverted, green: `test result: ok. 4 passed; 0 failed; 0
   ignored; 0 measured; 1927 filtered out`.
3. Mutation B (remote uses `login_shell()` too), red with `SHELL=fish`:
   `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1930
   filtered out` — `left: "$status", right: "$?"`.
4. Mutation B reverted, green: `test result: ok. 1 passed; 0 failed; 0
   ignored; 0 measured; 1930 filtered out` (and
   `a_local_shell_still_uses_the_login_shell` green throughout).

Acceptance-criteria sweep, run at this tree:

```
ssh_dir=1 ssh_argv_decl=1 lifecycle_row=1 audit_row=1
ControlPersist_in_host.rs=4 BatchMode_in_host.rs=0 marker_shell_name_files=1
exit_var_pinned=1 login_shell_pinned=1 tools=42 deferred=28
ai_ipc_cli_diff_files=0 ssh_spawning_tests=0
```
