# Phase 08f: prompt-on-the-real-prompt — a PTY password prompt is answered once, only when it is really on screen

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** phase-01a (the PTY credential round trip), phase-01b (the
host-side input scrub), phase-08b (the `read_to_marker` slice structure)
**Dispatched ahead of:** 08c (ghost-remote-pty), which runs ghost commands —
elevated ones included — through this path with nobody to answer
**Estimated diff:** ~250 lines across 6 files, plus 10 tests
**Tags:** language=rust, kind=bugfix, size=m, security

## Goal

A password prompt on a PTY shell is recognised **only when the program is
really prompting**, answered **exactly once per prompt**, and — when nobody
can answer — **cancelled so the shell stays usable**. Today none of those
three holds, on the local host or a remote one:

- the prompt is "recognised" on the shell's **echo of the typed command**,
  before the program has run, so a command that never prompts is given up
  with a false error;
- after a correct answer, **any later output re-triggers the answer**: the
  password is sent again and runs **as a shell command, in plaintext**, in the
  shell and its log (measured: 11–13 plaintext copies of the secret in one
  cast);
- a give-up types `\n`, which real sudo treats as a wrong password and
  re-prompts — so the **next command's bytes are eaten** as a password and it
  waits out its whole timeout.

This is the substrate 08c needs: a ghost has nobody to answer, so its
elevated command must fail fast and honestly and leave its shell usable.

## Architecture references

- `docs/security.md` § 2 — shell logs at rest are 0600 and the host scrubs
  secret input; a credential must never reach the shell or its log as text.
- `docs/dev/milestones/M21-host-model/README.md` § Notes — the 08f entry
  (measurements and the PE decision to insert this row ahead of 08c).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

**Shell-gate note.** Your shell refuses any command you type that contains
the literal word for the privilege tool this phase is about. Every pinned
test name below avoids it, every path is written with a `[d]` glob
(`src/daemon/utils/su[d]o.rs`), and every grep pattern uses `su[d]o`. Do the
same in anything you type. Source-file **content** is unaffected — edit it
with the patch tool as usual.

## Current state

Measured on **2026-09-27** at commit `873187e0`, on a clean tree.

Baseline: `cargo test --lib` → **2043 passed / 0 failed / 9 ignored**.

Every measurement below was taken with a throwaway in-crate probe that drove
the **real** `run_in_shell_with_prompts` against a real PTY host — a local
`bash`, or real `ssh -o BatchMode=yes pinky` — then read the host's cast
log. `pinky` runs **sudo-rs 0.2.13**; the daemon host runs **sudo 1.9.17p2**.

### 1. Defect A — the sentinel is matched on the echo, and it is the wrong sentinel

The command is wrapped **twice**. `run_in_shell_with_prompts`
(`src/daemon/executor/shell_exec.rs:105-110`) wraps it with sentinel N:

```rust
    let (has_sudo, _sentinel_value, wrapped) = if command_has_sudo(cmd) {
        let idx = PTY_SENTINEL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sentinel = sudo_sentinel(idx);
        (true, sentinel.clone(), with_sudo_sentinel(cmd, &sentinel))
    } else {
        (false, String::new(), cmd.to_string())
    };
```

and `run_sudo_command` (`shell_exec.rs:523-525`) wraps the already-wrapped
text again with N+1 and classifies on N+1:

```rust
    let idx = PTY_SENTINEL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let sentinel = sudo_sentinel(idx);
    let wrapped_cmd = with_sudo_sentinel(cmd, &sentinel);
```

What reaches the shell (copied from a cast):

```
SUDO_PROMPT='[de-sudo-prompt-1]' SUDO_PROMPT='[de-sudo-prompt-0]' sudo -k; sudo whoami
```

Bash gives the program the **later** assignment (`-0`), so the real prompt
carries `-0`; the classifier looks for `-1`, which appears **only in the
shell's echo** of the typed line. `classify_prompt` scans the whole
accumulated buffer (echo included), so it fires ~1 ms after the echo — before
the program has run. Measured on `pinky`: `sudo -k`, which **never prompts**,
failed in 17 ms with `sudo in shell s0 did not complete within the 20s limit:
credentials could not be read`, while the cast shows its `DE_END … 0` (it
succeeded).

Two more facts about the assignment form:

- `SUDO_PROMPT='…' cmd` applies to the **first simple command only**. In
  `sudo -k; sudo whoami` and `cd /tmp && sudo true` the program that prompts
  gets its **default** prompt, which nothing classifies. Today the echo match
  hides this by accident.
- The host-side scrub (`src/shell/backend.rs:50`, `looks_like_secret_prompt`)
  arms on `de-sudo-prompt-` in **any** output chunk, so it too arms on the echo.

### 2. Defect B — a correct answer is sent again, into the shell, in plaintext

`read_to_marker` (`src/shell/client.rs:404-420`) offers the callback the
**whole** accumulated buffer on every new chunk:

```rust
                if last_answered_at(&answered_at) != Some(buf.len()) {
                    let seen = String::from_utf8_lossy(&buf).into_owned();
                    match on_chunk(&seen).await {
                        PromptReply::Wait => {}
                        PromptReply::Send(answer) => {
                            write_answer(writer, &answer).await?;
                            answered_at = Some(buf.len());
                        }
```

After an answer the prompt's sentinel is still in `buf`, so the next chunk of
ordinary output — the program's own output, once the password is accepted —
re-classifies as a prompt and **sends the password again**. The comment at
`client.rs:326-328` ("A prompt still on screen in the next chunk does not
re-trigger the same callback answer") is false.

Measured with a **fake** program (a bash function that reads a password with
`read -rs -p "$SUDO_PROMPT"` and prints its length), a transport that answers
immediately, and `sudo true; sleep 1; echo TAIL`: the password was typed
three times; the two extras landed in the shell **after** the program had
accepted the first, ran as commands (`bash: s3cret-answer-q: command not
found`, twice), and the cast held **13** plaintext copies of the secret (as
`"i"` events and echoed `"o"` events — the scrub had cleared on the first
answer's newline). The run was reported as `sudo authentication failed after
3 incorrect password attempts` although it succeeded. In chat the extra
answers come from the user, who is simply asked for the password again.

### 3. Defect C — the give-up does not un-park the shell

`client.rs:412-418`:

```rust
                        PromptReply::GiveUp(reason) => {
                            // Un-park the shell: sudo at an empty newline fails
                            // fast with "no password was provided" and the end
                            // marker still arrives. Never the credential, never
                            // a signal.
                            write_answer(writer, b"\n").await?;
                            anyhow::bail!("{reason}");
                        }
```

Measured with a Python PTY driver at a **real** password prompt:

| | `\n` | `^C` (the byte `0x03`) |
|---|---|---|
| `pinky`, sudo-rs 0.2.13 | `Authentication failed, try again.` and **re-prompts**; the next command is eaten | killed by SIGINT (status 130); bash abandons the **rest of the line**, so **no end marker**; next command runs |
| daemon host, sudo 1.9.17p2 | `Sorry, try again.` and **re-prompts**; the next command is eaten | `a password is required`, exit 1; the line continues; next command runs |

The comment's claim is false on **both**. `0x03` is a byte written as input,
not a signal frame — the "never a signal" rule is kept.

**The settle problem.** On `pinky`, writing `0x03` and returning at once let
the next command's bytes arrive in the **same millisecond**: sudo-rs read them
as password characters (echoed as `*`) before it died, and the next command
timed out. Waiting until the shell's output goes quiet fixes it (§ Spec Task
3). **Measured: this wait only matters on sudo-rs** — locally, with or
without it, the next command runs. The live test (Task 5, test 10) is the only
witness for it.

### 4. The error text lies twice

`run_in_shell_core`'s sudo arm (`shell_exec.rs:388-399`, after 08b) wraps
**every** error as `sudo in shell {id} did not complete within the {limit}
limit` — false for a give-up that took 0.3 s. Then
`shell_run_error_text` (`src/daemon/executor/mod.rs:204`) tells the model the
command "may still be running … retrying could double-apply its effect" —
false after a Ctrl-C.

And with nobody attached (a ghost's transport is `tokio::io::sink()` /
`BufReader::new(tokio::io::empty())`, `src/daemon/ghost.rs:730-731`),
`credential_round_trip` (`shell_exec.rs:459-496`) reads EOF, fails to parse an
empty line, and gives up with `credentials could not be read` — which names
the wrong problem.

### 5. The prototype that fixed all of it

Applied to a scratch tree and measured, then reverted. With the fix, over
four runs each:

| Case | `pinky` (real sudo-rs) | local (fake program) |
|---|---|---|
| never-prompting command, nobody attached | `Ok`, exit 0, 8–26 ms | `Ok`, exit 0 |
| prompt, nobody attached | `Err` in 335–340 ms | `Err` in ~300 ms |
| next command after that | `Ok` in 8–27 ms | `Ok` in 1–2 ms |
| wrong password ×3, answered | three **real** attempts over 8.2 s, then sudo-rs's own `maximum 3 incorrect authentication attempts`; rest of the line ran | — |
| correct password, answered | — | `Ok`, exit 0, output `GOT_15` then `TAIL`; **0** plaintext copies in the cast, exactly one `[redacted]` |
| `cd /tmp && <program>`, answered | classified and answered | `Ok`, `GOT_15` |

### 6. Where the tmux path stands

`with_sudo_sentinel` is also called by the **tmux** foreground path
(`src/daemon/executor/foreground.rs:320`). The tmux opt-out stays
byte-for-byte 1.x, so that function is **not changed**; the PTY path gets a
new one (Task 1).

## Spec

### Task 1 — a sentinel the echo cannot contain, on every command of the line (`src/daemon/utils/su[d]o.rs`, `shell_exec.rs`)

1. Add beside `with_sudo_sentinel` (leave that function and its test
   untouched):

   ```rust
   /// The PTY form of the prompt sentinel. `export` so every program on the
   /// line sees it, not only the first; the sentinel is split across two
   /// quoted strings so the shell's echo of the typed line never contains
   /// it — only the program's real prompt does.
   pub fn with_pty_prompt_sentinel(cmd: &str, sentinel: &str) -> String {
       format!(
           "export SUDO_PROMPT='{}'; {cmd}",
           sentinel.replacen("sudo-prompt", "sudo-''prompt", 1)
       )
   }
   ```

   This is the exact form the prototype ran. The split-quote idiom is already
   in the codebase: the marker protocol spells its begin marker `DE_''BEG` so
   the echo never matches it, and the scrub test at `src/shell/backend.rs:639`
   writes `'[de-sudo-prompt''-78] Pass''word: '` for the same reason.

2. **Wrap exactly once.** `run_in_shell_with_prompts` stops wrapping: a
   command with the privilege tool in it passes to `run_in_shell_core` **as
   given**, and `run_sudo_command` alone derives the sentinel and wraps with
   `with_pty_prompt_sentinel`. The typed line then carries **one**
   `SUDO_PROMPT` assignment, and the classifier's sentinel is the one the
   program prints. (`SudoPromptArgs.wrapped` then holds the plain command —
   rename the field if you like.)

### Task 2 — offer the callback only output after the last answer (`src/shell/client.rs`)

In `read_to_marker`, hand `on_chunk` only the bytes that arrived **after the
last `Send`** — `&buf[answered_at.unwrap_or(0)..]` — never the whole buffer.
A prompt that was answered is then never seen again; a **re-prompt** (the
program printing its prompt anew after a wrong password) is new bytes and is
answered once, which is what makes the three-attempts count real.
`parse_outcome` keeps reading the **whole** buffer — only the callback's view
changes. Make the `client.rs:326-328` comment true.

### Task 3 — give up with `^C`, let the shell settle, and say so (`src/shell/client.rs`, `src/shell/mod.rs`)

1. Add a typed error beside `ShellExited` (`client.rs:44`) and export it from
   `src/shell/mod.rs` next to `ShellExited`:

   ```rust
   /// A prompt the caller could not answer was cancelled with Ctrl-C.
   #[derive(Debug)]
   pub struct PromptGivenUp {
       pub reason: String,
   }
   // Display: the reason, verbatim.
   // plus `impl std::error::Error for PromptGivenUp {}`
   ```

2. Replace the `GiveUp` arm (`client.rs:412-418`) with: write the single
   byte `0x03`, then **read until the shell's output has been quiet for
   300 ms, capped at 3 s from the give-up**, then return
   `Err(PromptGivenUp { reason }.into())`. The prototype that ran:

   ```rust
                           write_answer(writer, b"\x03").await?;
                           let cap = tokio::time::Instant::now() + Duration::from_secs(3);
                           loop {
                               let quiet = tokio::time::Instant::now() + Duration::from_millis(300);
                               let mut scratch = Vec::new();
                               match tokio::time::timeout_at(quiet.min(cap), reader.read_until(b'\n', &mut scratch)).await {
                                   Ok(Ok(n)) if n > 0 && tokio::time::Instant::now() < cap => continue,
                                   _ => break,
                               }
                           }
   ```

   Name the two durations as constants. The scratch buffer is deliberate: the
   command is being abandoned, so a half frame left in `line` by 08b's slices
   does not matter. **Do not wait for the end marker** — on sudo-rs it never
   comes (§ Current state 3).

3. Rewrite the doc comments at `client.rs:217-219` and on the arm itself to
   say what is now true: `0x03` cancels the prompt, the shell is given time to
   settle, and the reason is returned as a `PromptGivenUp`.

### Task 4 — honest words (`shell_exec.rs`, `src/daemon/executor/mod.rs`)

1. `credential_round_trip`: a read that returns **0 bytes** (EOF — nobody is
   attached, which is every ghost) gives up with this reason, verbatim:

   `"a password prompt appeared and nobody is attached to answer it — the prompt was cancelled and the command did not run with elevated privileges. Give this command passwordless access on that host, or run it from a chat session."`

   A non-empty line that does not parse keeps `credentials could not be read`.

2. `run_in_shell_core`'s sudo arm (`shell_exec.rs:388-399`): when the error's
   chain carries a `PromptGivenUp`, use the context
   `"the password prompt in shell {id} was cancelled"` instead of the
   limit wording. `ShellExited` and real timeouts keep their 08b wording.

3. `shell_run_error_text` (`mod.rs:204`) gains a third branch: a chain
   carrying `PromptGivenUp` →

   `"Error running command in shell {shell_id}: {e:#}. The password prompt was cancelled with Ctrl-C, so the command did not run with elevated privileges. Fix what the error names before retrying."`

   It must **not** contain `may still be running`. The other two branches are
   byte-identical.

### Task 5 — the scrub stops on Ctrl-C (`src/shell/backend.rs`)

`backend.rs:154` clears the scrub only on `\n` / `\r`, so after a `0x03`
give-up the **next command's text** is logged as `[redacted]` (seen in the
prototype's cast). Factor the test into a pure `fn ends_secret_entry(bytes:
&[u8]) -> bool` — true for `\n`, `\r` **or** `0x03` — and use it there.
`looks_like_secret_prompt` is **not** narrowed (its doc comment says why).

### Task 6 — tests

Ten tests; names pinned — **none contains the privilege tool's name**, so you
can filter by them. `PROMPTING` and `SILENT` below are the fakes the
prototype measured; define them as `const`s in the `shell_exec.rs` test
module. They are spelled `f=su; f+=do; eval …` so the defining command does
not itself contain the word and is not routed through the prompt path:

```rust
const PROMPTING: &str = r#"f=su; f+=do; eval "$f() { local pw=; while [ -z \"\$pw\" ]; do read -rs -p \"\$SUDO_PROMPT\" pw; echo; done; echo GOT_\${#pw}; }""#;
const SILENT: &str = r#"f=su; f+=do; eval "$f() { echo NOPROMPT; }""#;
```

`PROMPTING` re-prompts on an empty answer, as both real versions do (§ Current
state 3) — that is what lets test 7 witness Defect C locally. Run a fake's
definition as the **first command in that owner's shell** (plain path), then
the command under test in the same shell.

In `src/daemon/utils/su[d]o.rs` tests:

1. `pty_prompt_sentinel_is_absent_from_the_typed_command` — pure:
   `with_pty_prompt_sentinel("x true", &sudo_sentinel(7))` does not contain
   `[de-sudo-prompt-7]` and does not contain `de-sudo-prompt-`; it starts with
   `export SUDO_PROMPT=`.
2. `pty_prompt_sentinel_reaches_every_command_on_the_line` — real bash:
   run `bash -c` on `with_pty_prompt_sentinel("printf '%s|' \"$SUDO_PROMPT\"; true && printf '%s' \"$SUDO_PROMPT\"", &sudo_sentinel(7))`
   → stdout is exactly `[de-sudo-prompt-7]|[de-sudo-prompt-7]`.

In `src/shell/client.rs` tests (the scripted host,
`run_command_with_prompts_answers_a_prompt` at `client.rs:1170` is the
pattern):

3. `run_command_with_prompts_offers_only_output_after_the_last_answer` —
   the callback answers `Send` whenever its input contains
   `[de-sudo-prompt-0]`; the script publishes the begin marker, the prompt
   chunk, then (after 50 ms) an ordinary chunk `"\r\nROOT_OK\r\n"`, then the
   end marker. `Ok`, and `writes.len() == 2` — the command plus **exactly
   one** answer. *(Not measured at drafting — the test does not exist yet.
   Reasoned from `client.rs:404-410`: today the `ROOT_OK` chunk re-offers the
   whole buffer, which still holds the sentinel, so a second `Send` is
   written. Record what you observe before the fix, if you write the test
   first.)*
4. **Update** `run_command_with_prompts_gives_up_without_an_answer`
   (`client.rs:1221`) — same name — so the un-parking write is `b"\x03"`, not
   `b"\n"`, and the error downcasts to (or its chain carries) `PromptGivenUp`.

In `src/daemon/executor/shell_exec.rs` tests (real bash hosts; copy
`spawn_with_cast`, `shell_exec.rs:746`; the cast-cell pattern for reading the
log is `shell_exec_masks_a_secret_that_stays_raw_in_the_cast`,
`shell_exec.rs:1070`). "Nobody attached" = `tokio::io::sink()` +
`BufReader::new(tokio::io::empty())`; "answered" = a `Vec<u8>` writer + a
`BufReader` over a `Cursor` holding three `Request::CredentialResponse` lines
(`serde_json::to_string`) with credential `s3cret-answer-q`:

5. `a_password_prompt_that_never_appears_is_not_given_up` — `SILENT`, nobody
   attached, `sudo true` → `Ok`, exit 0, text contains `NOPROMPT`.
6. `a_correct_password_is_sent_exactly_once` — `PROMPTING`, answered,
   `sudo true; sleep 1; echo TAIL` → `Ok`, exit 0, text contains `GOT_15`
   and `TAIL`; the cast contains `s3cret-answer-q` **zero** times, `[redacted]`
   **exactly once**, and no `command not found`.
7. `an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable`
   — `PROMPTING`, nobody attached, `sudo true` → `Err` in **under 5 s** whose
   `{e:#}` contains `nobody is attached to answer it` and does **not**
   contain `did not complete within`; then, in the same shell, `echo AFTER`
   → `Ok` containing `AFTER` in **under 5 s** (timeout 10 s).
8. `a_later_command_on_the_line_still_gets_the_prompt_sentinel` —
   `PROMPTING`, answered, `cd /tmp && sudo true` → `Ok`, text contains
   `GOT_15`.

In `src/daemon/executor/mod.rs` tests:

9. `a_given_up_prompt_error_does_not_say_the_command_may_still_be_running`
   — pure on `shell_run_error_text`: a `PromptGivenUp` error → names the shell
   id, contains `cancelled`, does **not** contain `may still be running`; a
   plain `anyhow!("timed out after …")` → **does** contain `may still be
   running`.

In `src/shell/backend.rs` tests:

10. `ctrl_c_ends_secret_entry` — pure: `ends_secret_entry` is true for
    `b"x\n"`, `b"x\r"`, `b"\x03"`; false for `b"abc"` and `b""`.

Live, ignored — in `shell_exec.rs` tests,
`#[ignore = "requires the configured host \`pinky\` reachable over ssh"]`:

11. `an_unanswerable_password_prompt_on_pinky_leaves_the_shell_usable` — an
    in-process host running real `ssh` with `crate::host::ssh_argv` for
    `HostEntry { ssh: "pinky".into(), ..Default::default() }` and
    `-o BatchMode=yes` inserted **before the destination** (the last argv
    element — options after it become the remote command); host
    `HostId::Named("pinky")`; nobody attached. In one shell: `sudo -k` → `Ok`,
    exit 0; `sudo whoami` → `Err` containing `nobody is attached` in under
    5 s; `echo AFTER` → `Ok` containing `AFTER` in under 5 s (timeout 10 s).
    `println!` each elapsed time.

**Test count:** tests 1–3 and 5–10 are new and run (**9**); test 4 is an
existing test, updated (no count change); test 11 is new and ignored.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 8 — the live check

Run test 11 by hand and paste its output into the same entry, after the
mutations:

```sh
cargo test --lib an_unanswerable_password_prompt_on_pinky_leaves_the_shell_usable -- --ignored --nocapture 2>&1 | grep -E 'elapsed|^test |test result'
```

**If `pinky` is unreachable, report the criterion unmet** — do not describe
what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today — checked at drafting.

- [ ] `cargo test --lib` → **at least 2052 passed / 0 failed / 10 ignored**,
      on three consecutive runs (current: 2043/0/9). 2043 + 9 new = 2052;
      test 11 is ignored (9 → 10). A floor: extra tests are fine, fewer are
      not. **The ignored count is exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The PTY sentinel exists:
      ``grep -c 'pub fn with_pty_prompt_sentinel' src/daemon/utils/su[d]o.rs``
      → **1** (current: 0).
- [ ] The run path wraps once, with the PTY form:
      ``grep -c 'with_pty_prompt_sentinel(' src/daemon/executor/shell_exec.rs``
      → **1** (current: 0), and
      ``grep -c 'with_su[d]o_sentinel(' src/daemon/executor/shell_exec.rs``
      → **0** (current: 2). *(Proxies — tests 5–8 and Mutation A pin the
      property.)*
- [ ] The tmux path is untouched (*preservation*):
      ``git diff 873187e0 -- src/daemon/executor/foreground.rs src/daemon/background/ | wc -l``
      → **0**, and
      ``grep -c 'pub fn with_su[d]o_sentinel' src/daemon/utils/su[d]o.rs`` → **1**.
- [ ] The typed give-up error exists and is exported:
      ``grep -c 'pub struct PromptGivenUp' src/shell/client.rs`` → **1**
      (current: 0), and ``grep -c 'PromptGivenUp' src/shell/mod.rs`` → **≥ 1**
      (current: 0).
- [ ] The two false comments are gone:
      ``grep -c 'no password was provided' src/shell/client.rs`` → **0**
      (current: 1), and ``grep -c 'Probe B proved' src/shell/client.rs`` →
      **0** (current: 1).
- [ ] The scrub helper exists: ``grep -c 'fn ends_secret_entry' src/shell/backend.rs``
      → **1** (current: 0).
- [ ] All nine new non-ignored tests pass:

      ```sh
      cargo test --lib -- \
        pty_prompt_sentinel_is_absent_from_the_typed_command \
        pty_prompt_sentinel_reaches_every_command_on_the_line \
        run_command_with_prompts_offers_only_output_after_the_last_answer \
        a_password_prompt_that_never_appears_is_not_given_up \
        a_correct_password_is_sent_exactly_once \
        an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable \
        a_later_command_on_the_line_still_gets_the_prompt_sentinel \
        a_given_up_prompt_error_does_not_say_the_command_may_still_be_running \
        ctrl_c_ends_secret_entry
      ```

      → **9 passed** (current: `0 passed; 0 failed; … filtered out`).
- [ ] The updated and the preserved client tests pass:
      ``cargo test --lib shell::client::tests`` → **0 failed**, including
      `run_command_with_prompts_gives_up_without_an_answer` (updated),
      `run_command_with_prompts_answers_a_prompt`,
      `client_run_command_times_out_on_a_silent_command` and
      `client_run_command_fails_fast_when_the_shell_program_has_exited`.
- [ ] Mutations A, B and C were applied, run, and their observed results
      recorded in the Update Log; Mutation D was run live or reported unmet.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.
- [ ] The Task 8 live check was run and its output pasted into the same
      entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.** Every prediction below was measured at drafting
against the prototype.

- **Mutation A — an echo-visible sentinel.** Make `with_pty_prompt_sentinel`
  emit the sentinel unsplit (drop the `replacen`). Run tests 5 and 6.
  Measured: test 5 **red** (the never-prompting command is given up on the
  echo) and test 6 **red** (the answer is typed at echo time, before the
  program disables echo — 7 plaintext copies in the cast).
- **Mutation B — offer the whole buffer.** Hand `on_chunk` the whole `buf`
  again. Run tests 3 and 6. Measured: test 6 **red** (reported as a failed
  authentication; 3 plaintext copies). Test 3 is expected **red** (a third
  write) — reasoned, not measured; record what you see.
- **Mutation C — give up with a newline.** Write `b"\n"` instead of `0x03`.
  Run test 7. Measured: **red** — `echo AFTER` waits out its 10 s timeout,
  because `PROMPTING` (like real sudo) re-prompts on an empty answer.
- **Mutation D — no settle (live only).** Delete the settle loop, keeping the
  `0x03` write. Run test 11 by hand. Measured on `pinky`: `echo AFTER`
  **times out** — sudo-rs reads its bytes as password characters. **Locally
  this mutation stays green** (measured, three runs); test 11 is its only
  witness. If `pinky` is unreachable, report D unmet.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Ten tests (Task 6): two on the sentinel's shape and reach, one on the
callback's view, one updated give-up test, four real-shell tests with a fake
password program (never prompts / answered once / nobody attached / later on
the line), one on the model-facing text, one on the scrub helper — plus the
live `pinky` test. Every existing test stays green; the client's framing
tests and 08b's exit tests are the guard on the read loop.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'client_tests=%s\n' "$(cargo test --lib shell::client::tests 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- pty_prompt_sentinel_is_absent_from_the_typed_command pty_prompt_sentinel_reaches_every_command_on_the_line run_command_with_prompts_offers_only_output_after_the_last_answer a_password_prompt_that_never_appears_is_not_given_up a_correct_password_is_sent_exactly_once an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable a_later_command_on_the_line_still_gets_the_prompt_sentinel a_given_up_prompt_error_does_not_say_the_command_may_still_be_running ctrl_c_ends_secret_entry 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'pty_sentinel=%s\n' "$(grep -c 'pub fn with_pty_prompt_sentinel' src/daemon/utils/su[d]o.rs)"
  printf 'pty_wrap_sites=%s\n' "$(grep -c 'with_pty_prompt_sentinel(' src/daemon/executor/shell_exec.rs)"
  printf 'old_wrap_sites=%s\n' "$(grep -c 'with_su[d]o_sentinel(' src/daemon/executor/shell_exec.rs)"
  printf 'tmux_untouched=%s\n' "$(git diff 873187e0 -- src/daemon/executor/foreground.rs src/daemon/background/ | wc -l)"
  printf 'gaveup_type=%s\n' "$(grep -c 'pub struct PromptGivenUp' src/shell/client.rs)"
  printf 'false_comments=%s\n' "$(grep -c 'no password was provided\|Probe B proved' src/shell/client.rs)"
  printf 'scrub_helper=%s\n' "$(grep -c 'fn ends_secret_entry' src/shell/backend.rs)"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-08f.txt
cat /tmp/e2e-08f.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`** (a `FAILED` line names the
failing tests — never paper over one), `lib_counts=` at least `2052p-0f-10i`,
`client_tests=` with `0f`, `new_tests=9p-0f`, `pty_sentinel=1`,
`pty_wrap_sites=1`, `old_wrap_sites=0`, `tmux_untouched=0`, `gaveup_type=1`,
`false_comments=0`, `scrub_helper=1`, `ignore_total=10`.

**Known pre-existing flake.** `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
(`backend.rs:542`) fails intermittently under full-suite load — measured at
08b against the pristine tree. This phase edits `backend.rs`, so **if it
fails, report its name and do not edit that test** — record it, re-run.

**Run this block with nothing else running `cargo test`.**

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-08f-prompt-on-the-real-prompt.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08f.txt
diff /tmp/pasted-08f.txt /tmp/e2e-08f.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results and
the live check belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/daemon/utils/su[d]o.rs` — Task 1 and tests 1–2.
- `src/shell/client.rs` — Tasks 2–3, tests 3–4.
- `src/shell/mod.rs` — the one `PromptGivenUp` export.
- `src/shell/backend.rs` — Task 5 and test 10. **Not** the existing
  `input_after_a_prompt_logs_a_redaction_marker` test.
- `src/daemon/executor/shell_exec.rs` — Tasks 1–2 and 4, tests 5–8, 11.
- `src/daemon/executor/mod.rs` — Task 4 and test 9.
- This phase doc's Update Log.

## Out of scope

- **The tmux path** — `with_sudo_sentinel`, `foreground.rs`,
  `background/`. Byte-for-byte 1.x until deleted.
- **A prompt inside a script** — a command that does not itself name the
  privilege tool takes the plain path and is never classified; it waits out
  its timeout, as today. Note: the `export` leaves `SUDO_PROMPT` set in that
  shell, so such a prompt now **shows** a (stale) sentinel — which arms the
  host scrub, a small gain — but nothing answers it.
- **Connection-time ssh prompts** — row 08e.
- **Ghost commands on PTY shells** — row 08c, which this row unblocks.
- **The 08b doc-comment residual** at `shell_exec.rs:76` — carried to 08c.
- **The backend redaction-marker flake** — recorded, not fixed here.

## Update Log

<!-- entries appended below this line -->
