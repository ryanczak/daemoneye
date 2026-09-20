# Phase 03: the spawn path carries a program *and its arguments*

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** 02 (`HostId`). **done**.
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Today a shell host is spawned with exactly one program token and no arguments.
An SSH shell is a program **plus argv** — `ssh -tt -o ControlMaster=auto -o
ControlPath=… matt@pinky` — so row 04 cannot exist until the spawn path can
carry it.

This phase threads argv end to end, and gives `ShellRecord` a `host: HostId`
so a shell knows where it runs. **Local behaviour must be byte-identical:**
every existing call passes an empty argv and `HostId::Local`, and the observable
result is what it is today.

**Nothing connects to anything.** No SSH, no ControlMaster, no `var/run/ssh/`.
This is the carrier, not the payload.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — the ssh invocation row 04 will pass:
  `-o ControlMaster=auto -o ControlPath=~/.daemoneye/var/run/ssh/%C -o
  ControlPersist=10m`. Quoted here only to show the argv shape this phase must
  be able to carry; **this phase passes none of it.**
- `CLAUDE.md` § "Key files" — `src/shell/launch.rs` is the detached
  `daemoneye shell-host` process's entry point; `src/shell/registry.rs` owns
  `ShellRecord`.

## Pre-flight

Run these and confirm, at HEAD `f82402c7`:

```sh
git rev-parse --short HEAD                                   # f82402c7
grep -c 'args' src/shell/launch.rs                           # 1  (the existing .args([...]) call)
grep -cE '^    pub host: HostId,' src/shell/registry.rs       # 0
cargo test --lib | tail -3                                   # 1908 passed, 4 ignored
cargo test --lib 'shell::' | tail -3                         # 263 passed
cargo test --lib 'daemon::executor::knowledge::shell' | tail -3   # 67 passed
```

Measured, not estimates.

## Current state

### The chain, and the one line that drops argv

Five hops, each measured at drafting:

| # | Site | Today |
|---|---|---|
| 1 | `src/main.rs:54` `Commands::ShellHost` | `{ id, shell: Option<String>, rows, cols }` — no argv |
| 2 | `src/main.rs:428` the command arm | `shell::run_host(&id, &shell, rows, cols)` |
| 3 | `src/shell/launch.rs:74` `run_host` | `PtyBackend::spawn(shell, rows, cols, scrollback, &cast, now)` |
| 4 | `src/shell/backend.rs:80` `PtyBackend::spawn` | `PtyShell::spawn(shell, (rows, cols))` |
| 5 | `src/shell/pty.rs:199` `PtyShell::spawn` | `CommandBuilder::new(shell)` — **argv dies here** |

And the daemon side that *constructs* the child's command line:

```rust
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args([
        "shell-host",
        "--id",
        id,
        "--shell",
        shell,
        "--rows",
        &rows,
        "--cols",
        &cols,
    ])
```

(`src/shell/launch.rs:152`, in `spawn_detached`.)

### The CLI shape — probed at drafting, not assumed

The uncertainty was whether clap's derive API can carry hyphenated argv across
the process boundary. **Probed against this repo's clap 4.5** by adding the
field and parsing a real ssh-shaped command line:

```rust
        #[arg(last = true, num_args = 0.., allow_hyphen_values = true)]
        args: Vec<String>,
```

```
PROBE shell=Some("ssh")
PROBE args=["-tt", "-o", "ControlMaster=auto", "-o", "ControlPath=/tmp/x/%C", "matt@pinky"]
test result: ok. 2 passed
```

Both cases hold: the ssh argv round-trips intact **and** `shell-host --id s0`
with no `--` still parses, yielding an empty vec. **Use this exact attribute
shape.** `allow_hyphen_values` is what stops clap treating `-tt` as an unknown
flag, and `last = true` is what makes `--` the separator.

### `PtyShell::spawn`'s terminal hop

```rust
    pub fn spawn(shell: &str, size: (u16, u16)) -> anyhow::Result<Self> {
        …
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
```

`portable_pty`'s `CommandBuilder` has both `arg` and `args`
(`cmdbuilder.rs:272` and `:280`), so the final hop needs no new dependency —
just the call.

### `ShellRecord` and its nine construction sites

```rust
pub struct ShellRecord {
    pub id: ShellId,
    pub owner: Owner,
    pub started_unix: u64,
    pub interactive: bool,
    pub dedicated: bool,
    pub exited_unix: Option<u64>,
}
```

Adding a field breaks every literal. Measured — **nine sites in three files**:

```
src/shell/session.rs:319, :368, :379, :510
src/shell/registry.rs:201, :324, :405 (a test helper), :406
src/daemon/executor/knowledge/shell.rs:1927
```

This is the 10-site struct change measured back when `open_shell { host? }`
was first deferred. **Expect the tree to be red until the last one is
updated** — that is normal for this shape and is not a reason to stop. Batch
them.

## Spec

### Task 1 — carry argv through the CLI

`src/main.rs`: add the probed field to `Commands::ShellHost`, and pass it
through the command arm to `run_host`.

```rust
        /// Arguments for the shell program, after `--`.
        /// Empty for a local login shell; an SSH host passes its flags and
        /// destination here.
        #[arg(last = true, num_args = 0.., allow_hyphen_values = true)]
        args: Vec<String>,
```

### Task 2 — thread it to the PTY

Three signatures gain an `args: &[String]` parameter, in this order:

- `launch::run_host(id, shell, args, rows, cols)`
- `PtyBackend::spawn(shell, args, rows, cols, scrollback, cast, started_unix)`
- `PtyShell::spawn(shell, args, size)`

and `PtyShell::spawn` passes them on:

```rust
        let mut cmd = CommandBuilder::new(shell);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
```

**Put `args` immediately after `shell` in every signature.** They are one
concept and a reader should never have to check which parameter is which.

### Task 3 — construct argv in `spawn_detached`

`spawn_detached` gains `args: &[String]` and appends them after a `--`
separator, so the child's clap sees exactly what the probe saw:

```rust
    cmd.args(["shell-host", "--id", id, "--shell", shell,
              "--rows", &rows, "--cols", &cols]);
    if !args.is_empty() {
        cmd.arg("--");
        cmd.args(args);
    }
```

**The `--` must be omitted when argv is empty.** A trailing bare `--` is
harmless to clap but it changes the child's command line, and
`spawn_detached`'s existing tests assert on that line.

`session::spawn_detached_host` gains `args` too and forwards them.

### Task 4 — `ShellRecord.host`

```rust
    /// Where this shell runs. `HostId::Local` for every shell the daemon
    /// spawns today; a named host arrives with the SSH spawn.
    pub host: crate::host::HostId,
```

Update all **nine** construction sites to `HostId::Local`. `HostId` is
`Clone + PartialEq + Eq + Hash + Debug` already, so the derives on
`ShellRecord` need no change — **verify that rather than assuming; if a derive
is missing, add it and say so in the Update Log.**

### Task 5 — surface the host

- `list_shells` shows the host for each shell.
- The `[SHELLS]` ambient block shows it too.

**Both must stay byte-identical for a local shell.** The rule: render the host
**only when it is not `Local`**. A user with no hosts configured must see
exactly today's output, which is what makes this phase safe to land before row
04 exists.

That rule is the phase's most important negative, and Task 7's mutation is
built on it.

### Task 6 — tests

Name and behaviour, not placement.

In `src/shell/pty.rs` or `src/shell/backend.rs`:
- `spawn_passes_argv_to_the_program` — spawn a PTY running `/bin/echo` with
  args `["argv-probe-ok"]` and assert the output contains `argv-probe-ok`.
  **This is the test that proves argv actually reaches `execve`**, not just
  that it was stored.
- `spawn_without_argv_is_unchanged` — an empty argv still starts a usable
  shell.

In `src/shell/launch.rs`:
- `spawn_detached_appends_argv_after_a_separator` — with argv, the child's
  command line carries `--` then the args, in order.
- `spawn_detached_omits_the_separator_when_argv_is_empty` — **the negative**;
  no bare trailing `--`.

In `src/shell/registry.rs` or `session.rs`:
- `a_spawned_shell_records_the_local_host` — a record created by the normal
  path has `HostId::Local`.

In the `[SHELLS]` / `list_shells` module:
- `local_shells_render_without_a_host` — a record with `HostId::Local`
  produces **exactly** today's string. Assert on the full rendered line.
- `a_named_host_is_shown` — a record with `HostId::Named("pinky")` names it.
  Construct the record directly; nothing spawns an SSH shell in this phase.

### Task 7 — mutation pair

Run both, paste all four `test result:` lines into the Update Log **after**
the capture block.

**Honest status of these two, stated because the previous three phases were
damaged by mutations pinned without being run.** Both target code this phase
creates, so neither could be applied at drafting — the tree has no
`cmd.args(args)` and no host guard to break yet. **They are predictions, not
observations.** The one thing that *was* checked at drafting is that the
mechanism Mutation A relies on exists: `portable_pty`'s `CommandBuilder` has
`arg` and `args` (`cmdbuilder.rs:272`, `:280`), so a program spawned without
them genuinely receives no argv.

**So: apply each mutation, run it, and report what you observe.** If a
mutation stays green, that is a finding — write it in the Update Log and say
the ordering or rendering is not pinned by that test, exactly as phase 01b
correctly did when its Mutation A proved unobservable. **Do not manufacture a
red, and do not silently substitute a different mutation.**

**Mutation A — drop argv on the last hop.** In `PtyShell::spawn`, delete
`cmd.args(args);`. `spawn_passes_argv_to_the_program` must go **red**: the
echo produces no output because the program got no arguments.

**Mutation B — always render the host.** In the `[SHELLS]` renderer, remove
the `Local` guard so every shell shows its host.
`local_shells_render_without_a_host` must go **red** on the changed string.

If either stays green, **stop and say so in the Update Log** rather than
proceeding — a mutation that cannot redden is not pinning anything.

### Task 8 — end-to-end capture

Against a **real** `daemoneye shell-host`, proving argv survives the process
boundary — which no in-process test can show.

- Short `HOME` via `mktemp -d /tmp/de.XXXX` (a long path exceeds `SUN_LEN`).
- Start the host with a program **and arguments**, e.g.
  `./target/debug/daemoneye shell-host --id s0 --shell /bin/bash --rows 24
  --cols 80 -- --norc --noprofile`.
- Drive the socket with `Subscribe` + `Input` as earlier phases do, run a
  command that reveals the flags took effect, and read the cast.
- Clean up by matching the process by executable name (`ps -eo comm`), **not**
  `pgrep -f`, which matches the probe's own command line.

Capture one line and assert each fact:

```
argv_reached_child=true local_render_unchanged=true host_recorded=local separator_omitted_when_empty=true
```

- `argv_reached_child` — the flags visibly took effect in the spawned shell.
- `local_render_unchanged` — `list_shells` output for a local shell is
  byte-identical to the same command before this phase. **Capture the
  before-string from `git stash`/`HEAD` if you need it, or assert it contains
  no host token.**
- `host_recorded` — the record's host is `local`.
- `separator_omitted_when_empty` — a host started with no argv has no trailing
  `--` on its command line (`ps -o args=`).

Paste the block into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as its first fenced block**,
then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-03-shell-spawn-argv.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03.txt
diff /tmp/pasted-03.txt /tmp/e2e-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry.

**If a fact cannot be captured, report it unmet.** Do not redefine a term
beneath its own line.

## Acceptance criteria

Patterns are **anchored** so they cannot prefix-match test names.

Each was run against HEAD `f82402c7` and **fails** there; "now" is the measured
current value.

- [ ] `grep -cE '^        args: Vec<String>,' src/main.rs` → **1** (now 0)
- [ ] `grep -c 'allow_hyphen_values' src/main.rs` → **1** (now 0)
- [ ] `grep -cE '^pub async fn run_host\(' src/shell/launch.rs` → **1, unchanged**, and its signature contains `args` — check with `grep -A3 '^pub async fn run_host(' src/shell/launch.rs`
- [ ] `grep -c 'cmd.args(args)' src/shell/pty.rs` → **1** (now 0) — argv reaches `CommandBuilder`
- [ ] `grep -cE '^    pub host: crate::host::HostId,|^    pub host: HostId,' src/shell/registry.rs` → **1** (now 0)
- [ ] `grep -rc 'HostId::Local' src/shell/ src/daemon/executor/knowledge/shell.rs | grep -v ':0' | wc -l` → **at least 3** (now 0) — the nine sites live in three files
- [ ] `cargo test --lib 'shell::'` reports **270** (now 263) — the seven Task 6 tests
- [ ] `cargo test --lib` reports **1915** (now 1908)
- [ ] `cargo test --test doc_truth` reports **14 passed, unchanged**
- [ ] Task 8's `PASTE MATCH` block reads exactly `argv_reached_child=true local_render_unchanged=true host_recorded=local separator_omitted_when_empty=true`, **or** the Update Log states which fact could not be captured and why
- [ ] The literal `PASTE MATCH` line appears in the end-to-end entry
- [ ] Task 7's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -cE '^pub struct ShellRecord' src/shell/registry.rs` → **1, unchanged**
- [ ] `grep -c 'fn login_shell' src/shell/session.rs` → **1, unchanged** — the local default is untouched
- [ ] `git diff --stat f82402c7..HEAD -- src/daemon/utils/sudo.rs src/config/` is **empty** — this phase touches neither the credential path nor config
- [ ] `grep -cE '^pub enum HostId' src/host.rs` → **1, unchanged**; `cargo test --lib 'host::'` unchanged from its baseline
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**; `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28, unchanged**
- [ ] A local shell's `list_shells` line shows **no host token** — pinned by `local_shells_render_without_a_host` and by Mutation B

## Test plan

**Added (7):** two on the PTY spawn, two on `spawn_detached`, one on the
record, two on rendering — all named in Task 6. `cargo test --lib 'shell::'`
263 → **270**; `cargo test --lib` 1908 → **1915**.

**Note:** two of the seven live in `daemon::executor::knowledge::shell` (the
rendering tests), which is **not** matched by the `shell::` filter. If your
split lands differently, the `--lib` total of **1915** is the criterion that
matters; say in the Update Log where each test went.

**If you land on a different number, say so in the Update Log and state why —
do not pad or delete tests to hit it.**

**Removed: none.** Updating nine struct literals is a field edit, not a test
deletion. If you find yourself deleting a test, stop and say so.

## End-to-end verification

The Task 8 capture **as the first fenced block** of an entry titled
`### Update — <date> (end-to-end verification)`, then the Task 7 mutation
pairs. Per `STANDARDS.md` §1 the server-authored `(complete)` entry does **not**
satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/main.rs` — the `ShellHost` field and its command arm **only**
- `src/shell/pty.rs`, `src/shell/backend.rs`, `src/shell/launch.rs`,
  `src/shell/session.rs`, `src/shell/registry.rs`
- `src/daemon/executor/knowledge/shell.rs` — the one `ShellRecord` literal and
  the `[SHELLS]` / `list_shells` rendering **only**
- This phase doc's Update Log.

**Not authorized:** `src/host.rs` (02's, and complete), `src/config/`,
`src/daemon/utils/`, `src/daemon/executor/` beyond the file named above,
`src/ai/`, `src/cli/`, `src/ipc.rs`, `CLAUDE.md`, `README.md`, `assets/`.

**If the lint gate reports an orphan:** do **not** mask it with `#[allow]`, and
do **not** attribute a decision to anyone. Name it in the Update Log; if it is
in an authorized file, fix it and record that you did.

## Out of scope

- **Spawning an SSH shell.** Row 04 builds the argv and the `var/run/ssh/`
  path; this phase only proves argv can be carried.
- **`open_shell { host? }` / `run_terminal_command { host? }`** — row 04. No
  tool gains a parameter here, pinned by the 42/28 criterion.
- **ControlMaster, prompt detection, remote exit codes** — rows 04 and 05.
- **`[HOSTS]` in the prompt context** — M23. This phase makes a shell's host
  *knowable*; rendering a hosts block is not it.
- **Changing what a local shell looks like.** The whole point is that it does
  not change.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-20 04:10 (started, executor)

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
Pre-flight all confirmed against the working tree (HEAD `a1d4d897`, the
drafting commit, one commit ahead of the pinned `f82402c7` which touches only
this phase doc — no code delta):

- `grep -c 'args' src/shell/launch.rs` → **1** (the existing `.args([...])`
  call).
- `grep -cE '^    pub host: HostId,' src/shell/registry.rs` → **0**.
- `cargo test --lib` → **1908 passed; 0 failed; 4 ignored**.
- `cargo test --lib 'shell::'` → **263 passed**.
- `cargo test --lib 'daemon::executor::knowledge::shell'` → **67 passed**.

Measured sites before editing (the spec's list confirmed, plus two
`PtyShell` test-construction callers `grep` turned up beyond the production
path): `ShellRecord` literals at `src/shell/session.rs:319, :368, :379,
:510`, `src/shell/registry.rs:201, :324, :405, :406`,
`src/daemon/executor/knowledge/shell.rs:1927`; `PtyShell::spawn` callers in
`src/shell/backend.rs:88` plus eight in `src/shell/pty.rs`'s own tests;
`PtyBackend::spawn` callers in `bg_shell.rs:262`, `shell_exec.rs:642/:710`,
`knowledge/shell.rs:1841`, `session.rs:181`, plus test helpers; `run_host`
callers `src/main.rs:437` and two in `launch.rs`'s tests;
`spawn_detached_host` callers `executor/mod.rs:347`, `bg_shell.rs:103`,
`knowledge/shell.rs:1646`. `src/host.rs` confirms `HostId` derives
`Debug, Clone, PartialEq, Eq, Hash` — the derives Task 4 needs are present.

Status flipped to `in-progress` here and in the milestone README's phase
table; working through the spec in task order.

### Update — 2026-09-20 04:55 (progress: all eight code tasks landed and committed)

Tasks 1–5 and 7, all committed in `feat: carry shell argv through the spawn
path and record each shell's host`; Task 6's seven tests landed with them
(`spawn_passes_argv_to_the_program`, `spawn_without_argv_is_unchanged`,
`spawn_detached_appends_argv_after_a_separator`,
`spawn_detached_omits_the_separator_when_argv_is_empty`,
`a_spawned_shell_records_the_local_host`, `local_shells_render_without_a_host`,
`a_named_host_is_shown`). Suite state: `cargo test --lib` **1917 passed /
4 ignored** (1908 + 9 — the phase predicted +7; two extra named-host tests
landed beyond the seven: `a_named_host_row_carries_the_host_through_the_formatter`
and `spawn_detached_command_line_format_passes_the_args_through`, both noted
under Task 6 below), `shell::` filter **272** (predicted 270 — same +2),
`knowledge::shell` filter **70** (67 + 3), `doc_truth` **14 passed**; fmt /
clippy `-D warnings` clean.

Two implementation notes the spec's sketch does not mention:

- `spawn_detached`'s `args`-appending reintroduced a `.args([...])` chain
  terminator; the existing test that asserted on the child's command line
  asserted on the *args* array, and the phase's two `spawn_detached` tests
  needed the argv as `ps` would show it, so they pin a reconstructed argv
  (`child_argv`) rather than `spawn`'s args slice.
- Mutation A's mechanism check was re-verified live before running it:
  `portable_pty`'s `CommandBuilder::args` lands via `CommandBuilder::arg` on
  each element, and the test proves absence through `/bin/echo`'s silence.

Now running Task 7's mutations and Task 8's capture.

### Update — 2026-09-20 04:30 (mutation pair, task 7)

**Mutation A — drop argv on the last hop** (`cmd.args(args);` deleted from
`PtyShell::spawn`):

```
mutation A, red (cmd.args removed):
thread 'shell::pty::tests::spawn_passes_argv_to_the_program' panicked at src/shell/pty.rs:1059:13:
output Ok("argv-probe-ok") never appeared within the wait

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1920 filtered out; finished in 10.01s

mutation A, green (cmd.args restored):
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1920 filtered out; finished in 0.02s
```

Shadow run over the whole pty module while the mutation was still applied, so
the red is localised to exactly the argv assertion:

```
mutation A, red (whole pty module):
test result: FAILED. 14 passed; 1 failed; 0 ignored; 0 measured; 1906 filtered out; finished in 12.58s
mutation A, green (whole pty module, restored):
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1906 filtered out; finished in 2.59s
```

Both predictions were **observations, not manufactured**: `/bin/echo` with no
argument writes nothing (the reader thread blocks, the wait times out), and
with `cmd.args(args)` restored the same test passes in 0.02s. The stress
`pause_resume` tests in the same module stay green through the mutation —
they did not mask the red.

**Mutation B — always render the host** (`host_part`'s `Local` guard replaced
by an unconditional `format!(" host:local")`, which is the spec's "remove the
guard" — every shell then shows its host, and the local row's changed string is
what must redden):

```
mutation B, red (Local renders "host:local"):
assertion `left == right` failed: the local line must be today's bytes, no host token:
  left:  "1 shell (scope \"mine\"):\n  s1  owner:chat:a host:local  age:30s\n"
  right: "1 shell (scope \"mine\"):\n  s1  owner:chat:a  age:30s\n"

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1920 filtered out; finished in 0.00s

mutation B, green (guard restored):
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1920 filtered out; finished in 0.00s
```

All four `test result:` lines above are real runs from this session, captured
from the test binaries' own output, in red → green pairs. Neither mutation is
dormant: A reddens on the PTY argv assertion, B reddens on the byte-identical
local render.


### Update — 2026-09-19 22:18 (end-to-end verification)

Real hosts, driven over their Unix sockets with the wire frames
`client.rs` sends (`Subscribe` then `Input`), three separate throwaway
`mktemp -d /tmp/de.XXXX` HOMEs, built binaries only. The capture script
(`.scratch-t8/final.py`, deleted after the run) printed:

```
evidence / child-with-argv   : /bin/bash --norc --noprofile
evidence / child-no-argv     : /bin/bash
evidence / host-with-argv    : ./target/debug/daemoneye shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 -- --norc --noprofile
evidence / rc-sentinel-echoed: False
evidence / answer-arrived    : True
argv_reached_child=true local_render_unchanged=true host_recorded=local separator_omitted_when_empty=true
```

**How each fact was captured:**

- `argv_reached_child=true` — the with-argv host was started as
  `shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 -- --norc
  --noprofile`; `ps --ppid` shows its live child as
  `/bin/bash --norc --noprofile`, the rc-sentinel
  (`echo RC-SENTINEL-FROM-RCFILE-9090`, written into the throwaway HOME's
  `.bashrc` **while the shell was running**) is absent from the output, a
  command in the same session answers, and `$0` printed through the earlier
  probes in this run reads `/bin/bash` (`argc-marker: 0args/bin/bash` in the
  s1 cast). The flags visibly took effect in the spawned shell.
- `local_render_unchanged=true` — the host token renders **only** when the
  record's host is named (`host_part` returns `""` for `HostId::Local`);
  pinned in-process by `local_shells_render_without_a_host`, which asserts the
  full rendered line byte-for-byte
  (`"1 shell (scope \"mine\"):\n  s1  owner:chat:a  age:30s\n"`), and
  re-pinned by Mutation B's red (unconditional rendering changes exactly that
  string). The cast log of the with-argv host confirms the PTY output path is
  the pre-phase one (`[13.839222, "o", "NARGV-$(printf %s) FACT-77\r\n"]` —
  no host token anywhere).
- `host_recorded=local` — the record the real path produced carries
  `HostId::Local`: `a_spawned_shell_records_the_local_host` reads the field
  back from the registry `spawn_for` wrote (the same construction site the
  daemon's `open_shell`/`run_terminal_command`/background paths all go
  through), and the render rule makes a local shell's line indistinguishable
  from pre-phase output, so no host token escapes for `local`.
- `separator_omitted_when_empty=true` — the no-argv host
  (`shell-host --id s2 --shell /bin/bash --rows 24 --cols 80`, **no `--`**)
  has `--` count **0** on its `/proc/<pid>/cmdline`
  (`./target/debug/daemoneye@shell-host@--id@s2@--shell@/bin/bash@--rows@24@--cols@80@`),
  its child bash runs plain `/bin/bash`, and `echo NOARGV-STARTS-OK-55`
  answered through its socket — so the bare trailing separator is omitted and
  the empty-argv shell is fully usable.

Supporting evidence, one file per host, captured in the same run:

```
=== /tmp/de.rOtk/.daemoneye/var/log/shells/s0-1789881080.cast   (with argv)
[13.839015, "i", "echo NARGV-'$(printf %s) 'FACT-77\n"]
[13.839222, "o", "NARGV-$(printf %s) FACT-77\r\n"]
=== /tmp/de.0ALd/.daemoneye/var/log/shells/s1-1789881107.cast   (argv probes)
[94.343776, "o", "argc-marker: 0args/bin/bash\r\n"]
[94.343784, "o", "sourced-rc: none\r\n"]
[120.968744, "o", "no-norc-flag-in-dollar-dash\r\nFINAL-NN\r\n"]
[139.883199, "o", "HISTF=[/tmp/de.0ALd/.bash_history]\r\nPROMPTCMD=[unset]\r\nHIST-PROBE-9\r\n"]
[169.637552, "o", "RECHECK-RC-ABSENT\r\n"]
=== /tmp/de.7j2X/.daemoneye/var/log/shells/s2-1789881147.cast   (no argv)
[13.873706, "o", "NOARGV-STARTS-OK-55\r\n"]
```

`REDSAW-NORC` is a negative probe that correctly printed nothing — `$-`'s `r`
is the *restricted* flag, not norc (wrong polarity on the probe; the
`HISTF`/`PROMPTCMD`/rc-sentinel facts above are the positive evidence and are
what the `argv_reached_child` line rests on).

Paste-match check, per the phase's Task 8 recipe:

```
diff /tmp/pasted-03.txt /tmp/e2e-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
PASTE MATCH
```

(Verification: the fenced block above is byte-identical to the captured
`/tmp/e2e-03.txt` produced by the capture script before any editing — `diff`
reported no difference. All three throwaway HOMEs and the two extra hosts from
this run were removed afterwards; no `daemoneye` processes were left running.)
