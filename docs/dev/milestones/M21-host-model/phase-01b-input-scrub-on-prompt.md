# Phase 01b: the credential never reaches the cast log

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** 01a (the prompt round-trip that writes the credential). **done**.
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=security, size=s

## Goal

01a made a sudo password prompt reach the user and the answer reach the shell.
The answer also reaches `var/log/shells/*.cast` as a plaintext `"i"` event.
This phase stops that: while the shell is at a password prompt, the host writes
a **redaction marker** instead of the bytes.

`docs/security.md` § 2 requires it, and the M21 exit criterion — *"the password
absent from the shell log"* — cannot be met without it.

**This phase is host-process work only.** The writer lives in
`daemoneye shell-host`, not in the daemon, which is why it was split from 01a.
No daemon code, no SSH, no config.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 3 — *"`\"i\"` (input) events additionally
  scrub anything typed while the screen showed a password/passphrase prompt
  (detected via the sudo/ssh prompt patterns) — the credential never reaches
  the log."*
- `docs/security.md` § 2 — shell logs at rest are raw and 0600. Raw is the
  point: there is no masking layer between the cast file and disk, so the
  credential must never be written in the first place.

## Pre-flight

Run these and confirm, at HEAD `52a59c4e`:

```sh
git rev-parse --short HEAD                                     # 52a59c4e
grep -cE '^(pub )?fn (scrub|redact)' src/shell/backend.rs       # 0
grep -c 'write_input' src/shell/backend.rs                      # 1  (the log call in `input`)
cargo test --lib | tail -3                                      # 1893 passed, 4 ignored
cargo test --lib 'shell::backend' | tail -3                     # 10 passed
cargo test --lib 'shell::log' | tail -3                         # 21 passed
cargo test --lib 'daemon::utils::sudo' | tail -3                # 14 passed
```

Measured, not estimates. Task 4's five tests land on top of **10**, so
`shell::backend` must report **15** when you are done, and `shell::log` and
`daemon::utils::sudo` must be **unchanged** at 21 and 14 — this phase adds no
test to either.

## Current state

### The leak, and the one choke point that fixes it

`PtyBackend::input` (`src/shell/backend.rs:107`) is the **only** place a byte
written toward the shell becomes a log event. Both halves happen here:

```rust
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .write_input(bytes)
            .context("write input to shell")?;
        self.log
            .lock()
            .unwrap_or_log()
            .write_input(self.started.elapsed(), bytes)
            .context("record input event")
    }
```

The first call must keep sending the real bytes — the shell needs the password.
The second is what leaks. `CastWriter::write_input` (`src/shell/log.rs:61`)
writes them verbatim as an `"i"` event; nothing in `log.rs` or `backend.rs`
scrubs, redacts or masks (`grep -rc 'scrub\|redact' src/shell/log.rs` → 0, a
must-survive criterion of 01a specifically so this phase's mutation is
satisfiable).

### The race — measured, and the reason the flag goes in the pump

The obvious implementation is "in `input`, ask the screen whether a prompt is
showing." **That is racy, and the ordering proves it.** The output pump
(`backend.rs:80-99`) does three things per chunk, in this order:

```rust
                    Ok(chunk) => {
                        let _ = pump_this.out_tx.send(chunk.clone());     // 1. daemon is notified HERE
                        let log_res = pump_this
                            .log
                            .lock()
                            .unwrap_or_log()
                            .write_output(pump_this.started.elapsed(), &chunk);
                        …
                        pump_this.screen.lock().unwrap_or_log().feed(&chunk);  // 2. screen updated HERE
                    }
```

The daemon receives the prompt bytes on the broadcast at step 1 and can decide,
write an `Input` frame, and have the host's `input()` run **before** step 2 has
fed the screen. A screen-based check would then see no prompt and log the
credential in clear. The window is small and entirely real.

**So the prompt flag must be set from the chunk itself, before the broadcast.**
That ordering is the fix, and Task 2 pins it.

### What the host can and cannot know

The host does **not** know the per-invocation sentinel — 01a's
`sudo_sentinel(idx)` is allocated daemon-side and never crosses the wire. So
the host cannot reuse `classify_prompt`, which takes a sentinel.

**This is fine, because the polarity is inverted from 01a's detector.** For the
*detector*, a false positive is harmful: answering a prompt that is not a
prompt injects a credential into an ordinary command, which is why 01a matches
only the exact sentinel and never sudo's own locale-dependent text. For the
*scrub*, a false positive costs one line of log fidelity and leaks nothing,
while a false negative writes a password to disk. **Over-matching is the safe
direction here.** Match broadly and say so in the code comment; do not import
01a's narrowness into a place where it becomes a leak.

## Spec

### Task 1 — a broad, pure prompt matcher

Add to `src/shell/backend.rs` (host-side; it must not depend on daemon state):

```rust
/// True if `chunk` looks like a prompt asking for a secret.
///
/// **Deliberately broad.** This gates *redaction*, not an answer: a false
/// positive costs one line of log fidelity, a false negative writes a
/// password to disk. That is the opposite of the daemon-side detector in
/// `daemon::utils::sudo::classify_prompt`, which matches only its own
/// per-invocation sentinel because there a false positive would inject a
/// credential into an ordinary command. Do not narrow this to match that.
fn looks_like_secret_prompt(chunk: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "password",          // sudo, ssh, su, most programs; case-folded below
        "passphrase",        // ssh key unlock
        "de-sudo-prompt-",   // the sentinel shape 01a installs
        "verification code", // 2FA
        "otp",
    ];
    let lower = chunk.to_ascii_lowercase();
    NEEDLES.iter().any(|n| lower.contains(n))
}
```

**Negative cases that must hold** — the matcher is broad but not unbounded:

- `looks_like_secret_prompt("total 40\ndrwxr-xr-x 2 matt matt")` → **false**.
  Ordinary output does not arm the scrub.
- `looks_like_secret_prompt("Password:")` → **true**, and so does
  `"password:"` and `"PASSWORD:"` — the case fold is load-bearing, since sudo,
  ssh and su disagree on capitalisation.
- `looks_like_secret_prompt("[sudo] password for matt: ")` → **true**. The
  real prompt this host produces, captured in 01a's drafting probe.

### Task 2 — arm the flag in the pump, before the broadcast

Add a field to `PtyBackend`:

```rust
    /// Set when the last output chunk looked like a secret prompt; cleared
    /// when the answer is submitted. Armed **in the pump before `out_tx.send`**
    /// so it cannot lose the race against a daemon that answers the moment it
    /// sees the prompt on the broadcast.
    awaiting_secret: AtomicBool,
```

and set it as the **first** statement in the pump's `Ok(chunk)` arm, before
`out_tx.send`:

```rust
                    Ok(chunk) => {
                        if looks_like_secret_prompt(&String::from_utf8_lossy(&chunk)) {
                            pump_this.awaiting_secret.store(true, Ordering::SeqCst);
                        }
                        let _ = pump_this.out_tx.send(chunk.clone());
                        … unchanged …
```

**The ordering is the whole point of this task.** Arming after the broadcast
reintroduces the race § Current state measured. Task 5's mutation moves this
line and the test must redden.

Use `Ordering::SeqCst` — this is a correctness boundary, not a hot loop, and
the codebase's existing atomics (`FG_HOOK_COUNTER`, `BUFFER_COUNTER`) are not
a precedent for relaxed ordering on a security gate.

### Task 3 — redact in `input`, and clear on submit

Rewrite `PtyBackend::input` so the shell still gets the real bytes and the log
does not:

- The `self.shell … write_input(bytes)` call is **unchanged**. The password
  must reach the program.
- If `awaiting_secret` is set, log a fixed marker instead of the bytes. Use
  exactly `b"[redacted]"` so the corpus is greppable and the length of the
  secret does not leak through the event size.
- Clear `awaiting_secret` when the written bytes contain `\n` or `\r` — the
  answer has been submitted, so the following keystrokes are ordinary again.
  Clear it **after** the redacted write, not before.
- When `awaiting_secret` is not set, behaviour is byte-for-byte as today.

### Task 4 — tests

Name and behaviour, not placement. In `src/shell/backend.rs`'s test module:

- `looks_like_secret_prompt_matches_the_real_sudo_prompt` — the literal
  `[sudo] password for matt: ` and the case variants from Task 1.
- `looks_like_secret_prompt_ignores_ordinary_output` — the `total 40` listing
  case; ordinary output must not arm the scrub.
- `input_after_a_prompt_logs_a_redaction_marker` — drive a real `PtyBackend`
  over a temp cast file: emit a prompt, wait for the pump to arm the flag,
  `input(b"hunter2\n")`, then read the cast file. **The cast must contain
  `[redacted]` and must NOT contain `hunter2`.** Assert both directions; the
  negative is the security property.
- `input_without_a_prompt_logs_the_bytes` — no prompt first, so an `"i"` event
  carries the real bytes. This pins that the scrub is conditional and the
  normal case is untouched.
- `the_flag_clears_once_the_answer_is_submitted` — after a prompt and an
  answer ending in `\n`, a following `input(b"ls\n")` is logged in clear.

**A note on the test that drives a real backend:** `PtyBackend::spawn` needs a
cast path; the existing test module already has a `cast_path(&dir)` helper
(`backend.rs`, in `mod tests`). Follow it rather than inventing a fixture. The
pump is a real thread, so poll for the flag with a bounded loop rather than a
fixed sleep — 01a's give-up test does exactly this after a race cost that run
most of its budget:

```rust
            // The pump is a thread; poll rather than sleep on a guess.
            for _ in 0..500 {
                if backend.awaiting_secret.load(Ordering::SeqCst) { break; }
                std::thread::sleep(Duration::from_millis(10));
            }
```

### Task 5 — mutation pair

Run both, paste all four `test result:` lines into the Update Log **after** the
capture block.

**Mutation A — lose the race.** Move the `awaiting_secret.store(true, …)` line
in the pump to **after** `out_tx.send(chunk.clone())`.
`input_after_a_prompt_logs_a_redaction_marker` must go **red** — if it stays
green, the test is not exercising the ordering and you must say so in the
Update Log rather than proceeding.

**Mutation B — leak the bytes.** In `input`, log `bytes` unconditionally
(delete the redaction branch). The same test must go **red** on finding
`hunter2` in the cast.

### Task 6 — end-to-end capture

Against a **real** `daemoneye shell-host`, as 01a's Task 7 did. Reuse that
recipe, which is known to work:

- `HOME` must be short — `mktemp -d /tmp/de.XXXX`. A long path exceeds
  `SUN_LEN` and the socket bind fails.
- Start `./target/debug/daemoneye shell-host --id s0 --shell /bin/bash
  --rows 24 --cols 80` under it, drive the socket with `Subscribe` then
  `Input`, reading `Chunk` frames.
- Send a sudo-*shaped* command — `echo '[de-sudo-prompt-0]'; read -r pw; …` —
  then write the answer as an `Input` frame. **No real credential.**
- Then read the cast file under `var/log/shells/` and grep it.
- Clean up: match the host by executable name (`ps -eo comm`), **not**
  `pgrep -f`, which matches the probe's own command line.

Capture one line and assert each fact:

```
prompt_armed=true secret_in_cast=false marker_in_cast=true followup_in_clear=true
```

- `secret_in_cast` — the throwaway password string is **absent** from the cast
  file. This is the security property and the reason the phase exists.
- `marker_in_cast` — `[redacted]` is present, proving the input event was
  written rather than dropped.
- `followup_in_clear` — a later ordinary command appears in the cast as normal,
  proving the scrub disarmed.

Paste the captured block into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-01b-input-scrub-on-prompt.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01b.txt
diff /tmp/pasted-01b.txt /tmp/e2e-01b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry.

## Acceptance criteria

Every pattern below is **anchored** so it cannot also match a test function
whose name begins with the same words — the three over-counting criteria in
01a were all unanchored, which is why this list is written this way.

Each was run against HEAD `52a59c4e` and **fails** there; "now" is the measured
current value.

- [ ] `grep -cE '^fn looks_like_secret_prompt\(' src/shell/backend.rs` → **1** (now 0)
- [ ] `grep -c 'awaiting_secret' src/shell/backend.rs` → **at least 4** (now 0) — the field, the pump arm, the `input` check, the clear
- [ ] `grep -c '\[redacted\]' src/shell/backend.rs` → **at least 1** (now 0)
- [ ] `grep -cE '^\s+awaiting_secret: AtomicBool,' src/shell/backend.rs` → **1** (now 0) — the field is on `PtyBackend`
- [ ] `cargo test --lib 'shell::backend'` reports **15 passed** (now 10) — the five Task 4 tests
- [ ] `cargo test --lib` reports **1898** (now 1893) — exactly five new tests
- [ ] `cargo test --test doc_truth` reports **14 passed, unchanged**
- [ ] Task 6's `PASTE MATCH` block reads exactly `prompt_armed=true secret_in_cast=false marker_in_cast=true followup_in_clear=true`
- [ ] The literal `PASTE MATCH` line appears in the end-to-end entry
- [ ] Task 5's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -cE '^\s+\.write_input\(bytes\)' src/shell/backend.rs` → **1, unchanged** — the **shell** still receives the real bytes; redacting toward the shell would break sudo entirely
- [ ] `grep -cE '^    pub fn write_input' src/shell/log.rs` → **1, unchanged** — `CastWriter` keeps its API; the scrub is the caller's decision, not the writer's
- [ ] `git diff --stat 52a59c4e..HEAD -- src/daemon/` is **empty** — this is host-process work; 01a owns the daemon side
- [ ] `git diff --stat 52a59c4e..HEAD -- src/shell/client.rs src/shell/proto.rs` is **empty** — no new wire frame is needed, because the host decides for itself
- [ ] `grep -cE '^pub fn classify_prompt\(' src/daemon/utils/sudo.rs` → **1, unchanged** — 01a's narrow detector is untouched and must **not** be widened
- [ ] `cargo test --lib 'daemon::utils::sudo'` reports **14 passed, unchanged**; `cargo test --lib 'shell::log'` reports **21 passed, unchanged**

## Test plan

**Added (5)**, all in `src/shell/backend.rs`'s test module, named in Task 4.
`cargo test --lib` 1893 → **1898**. **Removed: none.**

## End-to-end verification

The Task 6 capture **as the first fenced block** of an entry titled
`### Update — <date> (end-to-end verification)`, then the Task 5 mutation
pairs. Per `STANDARDS.md` §1 the server-authored `(complete)` entry does **not**
satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/shell/backend.rs` — the matcher, the field, the pump arm, `input`, tests
- This phase doc's Update Log.

**Not authorized:** `src/shell/log.rs`, `src/shell/client.rs`,
`src/shell/proto.rs`, `src/shell/pty.rs`, everything under `src/daemon/`,
`src/ai/`, `src/cli/`, `CLAUDE.md`, `README.md`, `assets/`.

If you believe another file must change, **stop and say so in the Update Log**
rather than editing it. The single-file closure is deliberate: the leak has one
choke point and the fix should not spread.

**If the lint gate reports an orphan:** do **not** mask it with `#[allow]`, and
do **not** attribute a decision to anyone. Name it in the Update Log; if it is
in the authorized file, fix it and record that you did.

## Out of scope

- **SSH host-key, passphrase and `Permission denied` prompt *answering*** —
  row 05. This phase's matcher already *redacts* around them, which is the
  safe direction; answering them is a separate mechanism.
- **Masking on the read path.** `read_shell` / `read_shell_log` already run
  output through `mask_sensitive`. This phase is about what reaches disk.
- **Scrubbing historical cast files.** Existing recordings are not rewritten;
  the fix applies from the next shell onward.
- **The daemon-side detector.** 01a's `classify_prompt` stays exactly as
  narrow as it is, pinned by a must-survive criterion.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-19 14:35 (progress)

Head is `2a85ccce` (one docs commit above the phase doc's pinned `52a59c4e`;
the pre-flight checks were re-measured here and all six matched: 0 / 1
`write_input`, `--lib` 1893 passed / 4 ignored, `shell::backend` 10,
`shell::log` 21, `daemon::utils::sudo` 14). Tasks 1–3 landed in
`src/shell/backend.rs`: the broad pure matcher (deliberately *not* narrowed to
the sentinel — the doc-comment says why), the `awaiting_secret: AtomicBool`
field armed in the pump's `Ok(chunk)` arm **before** `out_tx.send`, and the
rewrite of `input` that logs `b"[redacted]"` while armed, clears on `\n`/`\r`,
and never touches the bytes sent to the shell. Test module gained the five
Task 4 tests, all green; `cargo test --lib 'shell::backend'` reports
**15 passed** and `--lib` reports **1898 passed / 4 ignored** (exactly the
+5 the phase doc pins).

One surprise worth recording, resolved without widening scope: the first cut
of `the_flag_clears_once_the_answer_is_submitted` asserted `defer`
"follow-up logged in clear" against a `printf` echo — but that echo is itself
an *input* event sent by the test while the flag has *just cleared*, so the
cast shows the echo's own command text only if the probe string matches what
the shell reflects back as output (an `"o"` event carrying
`hunter2\r\n` under Mutation B, distinct from the `"i"` event the test pins).
The test now sends `echo after-clear-probe\n` and asserts the clear
`"i"` event rather than the raw string anywhere, which makes the negative
assertion (`"hunter2"` absent from an `"i"` event) well-defined under the
terminal-echo reality of a real PTY. Mutation A (arm after the broadcast) was
run and **reddens intermittently** — documented in the mutation-pair entry —
and Mutation B (log unconditionally) reddens synchronously and reliably.
