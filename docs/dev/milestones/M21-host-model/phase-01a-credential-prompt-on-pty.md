# Phase 01a: a sudo password prompt on a PTY shell reaches the user

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** M20 (the PTY substrate), M22 (`run_terminal_command` on that
substrate). Both **closed**.
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=bugfix+feature, size=m

## Goal

`sudo` on the **default** execution backend hangs until its timeout and the
user is never asked for a password. This is a live defect on the shipped
default, measured at drafting (§ Current state). This phase makes the PTY path
do what the tmux path has always done: notice the prompt, ask the user through
`Response::CredentialPrompt`, write the answer into the shell, and carry on.

Every remote credential prompt M21 adds later — host key, passphrase,
`Permission denied`, **remote sudo** — is the same detector over an SSH PTY.
That is why this is the milestone's first row and not part of the SSH work.

**This phase is local sudo only.** No SSH, no hosts, no config.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — *"Host-key prompts and password
  prompts surface through the existing `CredentialPrompt` panel — the PTY makes
  them detectable the same way sudo prompts are today (`utils/sudo.rs`
  patterns, extended)."* The measurement below shows the second half of that
  sentence is not yet true of the PTY path; this phase makes it true.
- `docs/security.md` § 2 — the credential must not reach the shell log. This
  phase **does not** implement the scrub; it is phase 01b. The two are split
  because the log writer lives in the `daemoneye shell-host` process and the
  detector lives in the daemon. **01a must not weaken what 01b will add**: see
  the must-survive criterion on `"i"` events.

## Pre-flight

Run these and confirm, at HEAD `bf9dd263`:

```sh
git rev-parse --short HEAD                                        # bf9dd263
grep -c 'CredentialPrompt' src/daemon/executor/shell_exec.rs      # 0
grep -c 'sudo' src/daemon/executor/shell_exec.rs                  # 0
grep -c 'fn wait_for_sudo_prompt_and_inject' src/daemon/utils/sudo.rs  # 1
cargo test --lib | tail -3                                        # 1885 passed, 4 ignored
cargo test --lib 'daemon::utils::sudo' | tail -3                  # 11 passed
cargo test --lib 'shell::client' | tail -3                        # 17 passed
cargo test --lib 'daemon::executor::shell_exec' | tail -3         # 19 passed
```

Those three module baselines are measured, not estimates: the tests you add in
Task 5 land on top of **11 / 17 / 19**.

## Current state

### The defect, measured live at drafting — not inferred

Run 2026-09-18 against a real `daemoneye shell-host` (built from `bf9dd263`)
in an isolated `HOME`, driving the Unix socket directly with the wire frames
`client.rs` sends: `Subscribe`, then `Input` carrying a `wrap_command`-shaped
payload, then reading `Chunk` frames looking for the end marker.

**Probe A — a command that blocks on a PTY password prompt:**

```
cmd  : printf 'Password: '; read -r x; echo GOT=$x
result: end-marker seen = False   (gave up after 12.0s)
tail  : ...\x1fDE_BEG PROBE...\x1f\r\nPassword:
```

**Probe C — real `sudo`, credentials cleared, no stdin redirect (the actual
chat case):**

```
cmd  : sudo -k; sudo true; echo rc=$?
result: end-marker seen = False   (gave up after 15.0s)
tail  : ...\x1fDE_BEG P66518\x1f\r\n[sudo] password for matt:
```

The end marker never arrives, so `run_command` runs to its deadline and
`run_in_shell` returns the `"command exceeded the … limit"` error. **The user
is never asked for anything.** On the default `[execution] backend = "pty"`,
with the default 45 s timeout, that is a 45-second hang followed by a timeout
error for any `sudo` command whose credentials are not cached.

**Probe B — sudo is reachable and fails fast when it cannot prompt:**

```
cmd  : sudo -k; sudo -S true </dev/null 2>&1; echo rc=$?
result: end-marker seen = True    (1.8s)
tail  : [sudo] password for matt: \r\nsudo: no password was provided\r\n
        sudo: a password is required\r\nrc=1\r\n ...\x1fDE_END P52748 0\x1f
```

So the mechanism is sound end to end; what is missing is *noticing the prompt
and answering it*. **The literal prompt this host produces is
`[sudo] password for matt: `** — do not hard-code that string, see Task 1.

**A second finding from Probe A, recorded for 01b and row 04, not fixed
here:** while the shell sat at the prompt, a *second* command written to the
same shell was consumed by the pending `read`, and the first command's end
marker then arrived carrying the second command's exit status. A shell parked
at a prompt is poisoned exactly like the interactive-session case
`shell_exec.rs:219` already guards. **This phase must not leave a shell
parked** — see Task 4's give-up path.

### What exists on the tmux path, and does not on the PTY path

`grep -n 'sudo\|Credential\|password' src/daemon/executor/shell_exec.rs
src/daemon/executor/bg_shell.rs src/shell/client.rs` → **no matches.**

The tmux path has a complete mechanism, and it is the shape to mirror:

```rust
/// Prefix `cmd` so sudo prints `sentinel` instead of its default password
/// prompt. Same shape as the background-window form in
/// `background/run.rs` (`SUDO_PROMPT='[de-sudo-prompt]' {cmd}`).
pub fn with_sudo_sentinel(cmd: &str, sentinel: &str) -> String {
    format!("SUDO_PROMPT='{sentinel}' {cmd}")
}

/// Per-invocation sudo prompt sentinel. The closing bracket makes the match
/// exact: `[de-sudo-prompt-3]` is not a substring of `[de-sudo-prompt-33]`.
pub fn sudo_sentinel(idx: usize) -> String {
    format!("[de-sudo-prompt-{idx}]")
}
```

and the poll-and-inject loop (`utils/sudo.rs:94`), which the PTY version
replaces with a *stream* watch rather than a `capture-pane` poll:

```rust
pub async fn wait_for_sudo_prompt_and_inject(
    pane_id: &str,
    credential: &str,
    sentinel: &str,
) -> bool {
    const POLL: std::time::Duration = std::time::Duration::from_millis(200);
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    …
        if is_fingerprint_prompt(&snap) {
            return false;          // cannot be satisfied programmatically
        }
        if snap.contains(sentinel) {
            …send_keys(&p, &c)…
            return true;
        }
```

Three properties to carry over verbatim in meaning: **match the sentinel, not
`[sudo]` or `password`** (stale text from an earlier command must not trigger);
**fail fast on a fingerprint prompt** rather than waiting out the timeout; and
**bound the wait** independently of the command's own timeout.

### The call site

`src/daemon/executor/shell_exec.rs`, in the run path:

```rust
    let outcome = run_command(&socket, cmd, &login_shell(), applied_timeout)
        .await
        .map_err(|e| {
            e.context(format!(
                "command exceeded the {applied_timeout:#?} limit in shell {}",
                id.as_str()
            ))
        })?;
```

`run_command` (`src/shell/client.rs:134`) subscribes, writes one `Input`
frame, then accumulates `Chunk` frames until `parse_outcome` finds the end
marker or the deadline passes. **The prompt bytes are already arriving as
`Chunk` frames** — Probe A proves it. Nothing reads them for meaning.

### The facts the design gets slightly wrong

- § 2.2 says prompts are *"detectable the same way sudo prompts are today"*.
  Today's detection is `capture_pane` scraping a tmux pane; there is no pane
  here. The PTY analogue is the `Chunk` stream, which is strictly better —
  it needs no polling.
- The prompt is not guaranteed to be `[sudo] password for <user>: `. It is
  locale-dependent and `sudo.conf`-dependent, which is exactly why the tmux
  path sets `SUDO_PROMPT` to a sentinel instead of matching sudo's own text.

## Spec

### Task 1 — a pure prompt detector

Add to `src/daemon/utils/sudo.rs` (the existing home of this vocabulary):

```rust
/// What a chunk of shell output is asking for, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// A sudo password prompt carrying our sentinel.
    SudoPassword,
    /// A fingerprint prompt — it cannot be satisfied by typing, so the
    /// caller must give up rather than wait.
    Fingerprint,
}

/// Pure: what `output` is prompting for, given the sentinel this invocation
/// installed. `None` when it is not a prompt we can answer.
///
/// Matches the **sentinel**, never sudo's own `[sudo] password for …` text:
/// that text is locale- and `sudo.conf`-dependent, and stale output from an
/// earlier command in the same shell must not trigger a prompt. The sentinel
/// is per-invocation, so it cannot collide across commands.
pub fn classify_prompt(output: &str, sentinel: &str) -> Option<PromptKind> {
    if is_fingerprint_prompt(output) {
        return Some(PromptKind::Fingerprint);
    }
    if output.contains(sentinel) {
        return Some(PromptKind::SudoPassword);
    }
    None
}
```

**Negative cases this must satisfy, and they are the point of the function:**

- `classify_prompt("[sudo] password for matt: ", "[de-sudo-prompt-1]")` →
  **`None`**. Sudo's own text is not a trigger.
- `classify_prompt("[de-sudo-prompt-11]", "[de-sudo-prompt-1]")` → **`None`**.
  The closing bracket is what makes the match exact; this is the documented
  reason `sudo_sentinel` includes it.
- A fingerprint prompt returns `Fingerprint` **even when the sentinel is also
  present** — the fingerprint check comes first, because a fingerprint prompt
  cannot be answered with a password.

### Task 2 — stream the command output while it runs

`src/shell/client.rs`'s `run_command` currently accumulates `Chunk` frames and
only returns at the end marker. Give it a variant that can react mid-command.
**Do not change `run_command`'s signature** — every existing caller must keep
compiling and behaving identically. Add a sibling:

```rust
/// `run_command`, plus a callback invoked with the accumulated output each
/// time a `Chunk` arrives. The callback returns bytes to write into the
/// shell (the answer to a prompt), or `None` to keep waiting.
pub async fn run_command_with_prompts<F, Fut>(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
    on_chunk: F,
) -> anyhow::Result<CommandOutcome>
where
    F: FnMut(&str) -> Fut,
    Fut: std::future::Future<Output = PromptReply>,
```

with

```rust
/// What the prompt handler decided about the output so far.
pub enum PromptReply {
    /// Nothing to answer; keep reading.
    Wait,
    /// Write these bytes into the shell, then keep reading.
    Send(Vec<u8>),
    /// Stop: this prompt cannot be answered. The command is abandoned.
    GiveUp(String),
}
```

**`run_command` must become a thin wrapper over this** that always answers
`Wait` — one implementation, so the two can never diverge on framing, nonce
handling or the write-half lifetime rule documented at `client.rs:154`.

**The write-half lifetime is load-bearing.** `start_command` returns the
writer, and the existing comment says dropping it half-shuts the stream, which
the host reads as end-of-writes, tearing the connection down before the end
marker arrives. `Send` writes through **that same writer**; do not open a
second connection.

**Call the callback at most once per prompt.** Track which sentinel match has
already been answered so a prompt still on screen in the next chunk does not
produce a second `CredentialPrompt`. A retry is a *new* sentinel (Task 4).

### Task 3 — wire it into the run path

In `shell_exec.rs`, at the `run_command` call site quoted above:

1. If `crate::daemon::utils::command_has_sudo(cmd)` is false, call
   `run_command` exactly as today. **No behaviour change whatsoever for the
   non-sudo path** — that is the overwhelming majority of commands and a
   must-survive criterion.
2. If it is true, allocate a sentinel, wrap the command with
   `with_sudo_sentinel`, and call `run_command_with_prompts`. On
   `PromptKind::SudoPassword`, send `Response::CredentialPrompt { id, prompt }`
   to the client, await `Request::CredentialResponse`, and answer `Send` with
   the credential plus `\n`. On `PromptKind::Fingerprint`, answer `GiveUp`.

**Reuse the existing prompt text**: `sudo_password_prompt(attempt, max)`
already produces `"[sudo] password required"` and the retry wording. Do not
write new copy.

**The sentinel index must be unique per invocation.** `sudo_sentinel(idx)`
takes one; the tmux path feeds it a per-command hook counter. Use an
`AtomicUsize` in `shell_exec.rs` — the codebase's idiom for exactly this is
`FG_HOOK_COUNTER` (`CLAUDE.md` § "Global statics in daemon"). A fresh index per
invocation is what makes the "exact match" property meaningful.

### Task 4 — retries, give-up, and never park a shell

- **Retries:** sudo allows three attempts. On a wrong password sudo re-prompts
  with the same sentinel; answer up to **3** times total, using
  `sudo_password_prompt(attempt, 3)` so the user sees the retry wording, then
  give up.
- **Give-up must not leave the shell parked at a prompt.** This is the second
  finding in § Current state: a parked shell eats the next command's bytes.
  Before returning the error, write a single `\n` — sudo then fails with
  *"no password was provided"*, the command finishes, and the end marker
  arrives. **Do not** write the credential, and **do not** send a signal. Probe
  B is the evidence that a refused sudo terminates cleanly this way.
- **A user who declines the prompt** (the client's existing cancel path)
  takes the same give-up route.
- **Bound the prompt wait independently.** If a sentinel appears and no
  `CredentialResponse` arrives within **10 s** — the tmux path's `TIMEOUT` —
  give up. The command's own `applied_timeout` still applies on top.

### Task 5 — tests

Name and behaviour, not placement:

In `utils/sudo.rs`:
- `classify_prompt_matches_only_the_sentinel` — sudo's own
  `[sudo] password for matt: ` text yields `None`; the sentinel yields
  `SudoPassword`.
- `classify_prompt_rejects_a_longer_sentinel` — `[de-sudo-prompt-11]` does not
  match sentinel `[de-sudo-prompt-1]`.
- `classify_prompt_prefers_fingerprint` — a fingerprint prompt containing the
  sentinel yields `Fingerprint`.

In `shell/client.rs`, against an **in-process fake host** (bind a real
`UnixListener` in a temp dir and speak the frames — `launch.rs`'s test module
already does this; follow it):
- `run_command_with_prompts_answers_a_prompt` — the fake emits the sentinel,
  the callback returns `Send`, the fake sees those exact bytes arrive on the
  same connection, then emits the end marker; the outcome carries the real exit
  code.
- `run_command_with_prompts_gives_up_without_an_answer` — callback returns
  `GiveUp`; the call returns an error naming the reason, and **the fake
  received a `\n`** (the un-parking write).
- `run_command_is_unchanged_without_prompts` — a command with no sentinel
  behaves identically through both entry points.

In `shell_exec.rs`:
- `non_sudo_command_takes_the_plain_path` — a command without `sudo` never
  allocates a sentinel and never wraps the command. Assert on the **command
  string reaching the client**, not on a call count.

**`cargo test --lib` will move.** Pin the number you land on in the Update Log
and confirm no existing test name disappeared.

### Task 6 — mutation pair

Run both; paste all four `test result:` lines into the Update Log **after** the
capture block.

**Mutation A — match sudo's own text.** In `classify_prompt`, add
`|| output.contains("[sudo] password")` to the sentinel branch.
`classify_prompt_matches_only_the_sentinel` must go **red**.

**Mutation B — drop the un-parking newline.** Remove the `\n` write on the
give-up path. `run_command_with_prompts_gives_up_without_an_answer` must go
**red** on the missing byte.

If either stays green, **stop and say so in the Update Log** — the test is not
pinning the property.

### Task 7 — end-to-end capture

This phase fixes a defect that only a real shell host shows, so the evidence is
a **real host**, not a fake. Reproduce the drafting probe against the built
binary, with the fix in place.

Write `scripts/probe-sudo-prompt.sh` under the scratchpad (not the repo), or
inline the steps — either way the Update Log carries the captured output. The
shape that worked at drafting:

- `HOME` must be **short**: the probe at drafting first failed with
  `path must be shorter than SUN_LEN` under a long temp path. Use
  `mktemp -d /tmp/de.XXXX`.
- Start `./target/debug/daemoneye shell-host --id s0 --shell /bin/bash
  --rows 24 --cols 80` with that `HOME`.
- Drive the socket with the three frames `client.rs` sends.
- **Clean up**: kill the host and remove the temp `HOME`. Match the process by
  executable name (`ps -eo comm`), **not** `pgrep -f 'shell-host'` — at
  drafting that pattern matched the probe's own command line and reported a
  stray process that did not exist.

Capture **one line** and assert each fact it states:

```
sudo_prompt_detected=true credential_written=true end_marker=true parked=false
```

- `sudo_prompt_detected` — the sentinel appeared in the stream.
- `credential_written` — the answer reached the shell.
- `end_marker` — the command completed (contrast: `false` at drafting).
- `parked=false` — a following `echo ALIVE` in the same shell returns `ALIVE`,
  proving the shell was not left at a prompt.

**Do not type a real password.** Drive it with a throwaway sudo-like script on
`PATH` that prints the sentinel and accepts a fixed string, or use
`SUDO_ASKPASS`. If you cannot do it without a real credential, **say so in the
Update Log and capture the three facts you can** — a fabricated success is far
worse than a partial one.

Paste the captured block into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-01a-credential-prompt-on-pty.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01a.txt
diff /tmp/pasted-01a.txt /tmp/e2e-01a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Acceptance criteria

Each was run against HEAD `bf9dd263` and **fails** there; "now" is the measured
current value.

- [ ] `grep -c 'fn classify_prompt' src/daemon/utils/sudo.rs` → **1** (now 0)
- [ ] `grep -c 'enum PromptKind' src/daemon/utils/sudo.rs` → **1** (now 0)
- [ ] `grep -c 'fn run_command_with_prompts' src/shell/client.rs` → **1** (now 0)
- [ ] `grep -c 'enum PromptReply' src/shell/client.rs` → **1** (now 0)
- [ ] `grep -c 'CredentialPrompt' src/daemon/executor/shell_exec.rs` → **at least 1** (now 0)
- [ ] `grep -c 'with_sudo_sentinel' src/daemon/executor/shell_exec.rs` → **at least 1** (now 0)
- [ ] `cargo test --lib 'daemon::utils::sudo'` — the three `classify_prompt` tests pass
- [ ] `cargo test --lib 'shell::client'` — the three `run_command*` tests pass
- [ ] `cargo test --test doc_truth` reports **14 passed, unchanged**
- [ ] Task 7's `PASTE MATCH` block reads exactly `sudo_prompt_detected=true credential_written=true end_marker=true parked=false`, **or** the Update Log states plainly which facts could not be captured and why
- [ ] The literal `PASTE MATCH` line appears in the end-to-end entry
- [ ] Task 6's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -c 'pub async fn run_command' src/shell/client.rs` → **1, unchanged** — `run_command` keeps its name and signature; every existing caller compiles untouched
- [ ] `git diff --stat bf9dd263..HEAD -- src/daemon/background/ src/daemon/executor/foreground.rs` is **empty** — the tmux path is not this phase's to touch
- [ ] `grep -c 'fn wait_for_sudo_prompt_and_inject' src/daemon/utils/sudo.rs` → **1, unchanged** — the tmux mechanism stays; this phase adds a sibling, it does not refactor
- [ ] `grep -c 'is_interactive_command' src/daemon/executor/shell_exec.rs` → **1, unchanged** — the interactive guard at `:219` still runs before any of this
- [ ] `grep -rc 'scrub\|redact' src/shell/log.rs | grep -v ':0' | wc -l` → **0, unchanged** — **01a must not add log scrubbing.** That is 01b's, in the host process. A scrub added here would be in the wrong process and would make 01b's mutation unsatisfiable
- [ ] `grep -c '"i"' src/shell/log.rs` → **at least 1, unchanged** — the input-event writer is still there for 01b to change
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**; `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28, unchanged** — no tool changes in this phase

## Test plan

**Added (7):** three in `utils/sudo.rs`, three in `shell/client.rs`, one in
`shell_exec.rs`, named in Task 5. **Removed: none.** If you find yourself
deleting a test, stop and say so in the Update Log.

`cargo test --lib` moves from 1885; pin the landed number in the Update Log and
confirm no existing test name disappeared. `doc_truth` stays at **14**.

## End-to-end verification

The Task 7 capture **as the first fenced block** of an entry titled
`### Update — <date> (end-to-end verification)`, then the Task 6 mutation
pairs. Per `STANDARDS.md` §1 the server-authored `(complete)` entry does **not**
satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/daemon/utils/sudo.rs` — additive: `PromptKind`, `classify_prompt`, tests
- `src/shell/client.rs` — `run_command_with_prompts`, `PromptReply`, the
  `run_command` wrapper, tests
- `src/daemon/executor/shell_exec.rs` — the run-path call site and its test
- `src/shell/proto.rs` — **only** if a new frame proves necessary; say so in
  the Update Log with the reason before doing it. The expectation is that
  `Input` suffices, since it is how the credential is written.
- This phase doc's Update Log.

**Not authorized:** `src/daemon/background/`, `src/daemon/executor/foreground.rs`,
`src/shell/log.rs`, `src/shell/backend.rs`, `src/shell/host.rs`, `src/ai/`,
`src/cli/`, `CLAUDE.md`, `README.md`, `assets/`.

**If the lint gate reports an orphan this closure missed:** do **not** mask it
with `#[allow]`, and do **not** attribute a decision to anyone. Name it in the
Update Log, then — if it lives in an **authorized** file, delete it and record
that you did so; if it lives in an **unauthorized** file, either stop with a
blocker entry, **or** make the change and label it plainly as **your own**
unauthorized-but-forced decision with the evidence. **State which you chose and
that it was your decision.** There is no human in the loop mid-run.

## Out of scope

- **Scrubbing the credential out of the cast log** — **phase 01b**, in the
  `daemoneye shell-host` process. Pinned by a must-survive criterion above:
  01a adds no scrub.
- **SSH, hosts, `[hosts]` config, ControlMaster** — rows 02 and 04.
- **Host-key, passphrase and `Permission denied` patterns** — row 05. This
  phase's `PromptKind` is the enum row 05 extends; adding those variants now
  without a way to exercise them would ship untested arms.
- **Remote sudo** — row 05, over an SSH PTY, using this detector unchanged.
- **The tmux backend path.** It already works and stays byte-for-byte as is.
- **`sudo_credentials_cached()` as a pre-flight.** The tmux path calls it to
  avoid a needless pane switch; there is no pane here, and the sentinel tells
  us the truth. Adding a pre-flight would be a second source of truth about
  whether a password is needed.

## Update Log

<!-- entries appended below this line -->
