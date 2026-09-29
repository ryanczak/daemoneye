# Phase 08e: remote-connect-handshake — connect to a host first, answering its prompts, so no shell's first command is ever typed into one

**Milestone:** M21 — Host model & SSH
**Status:** in-progress — bounced 2026-09-28: [bug-08e-1](bugs/bug-08e-1.md) (major), [bug-08e-2](bugs/bug-08e-2.md) (major), [bug-08e-3](bugs/bug-08e-3.md) (minor)
**Depends on:** phase-04a (`ssh_argv`, ControlMaster), phase-05 (ssh prompt
classification and its credential round trip), phase-08f (answer once, only
output after the last answer)
**Estimated diff:** ~400 lines across ~9 files, plus 7 tests
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Before the first shell to a configured host is spawned, the daemon **connects
to that host first**: if no ControlMaster is running for it, it runs
`ssh -M -N -f` in a PTY it drives, answers a password or key-passphrase prompt
through the caller's transport (the chat client's `CredentialPrompt`),
refuses an unknown host key, and only then lets the shell spawn — as a
**multiplexed client that can never prompt**. So a remote shell's first
command can no longer be typed into ssh's `password:` prompt and wait out its
timeout.

All four remote callers do it (PE, 2026-09-29): `run_terminal_command { host }`,
`read_file { host }`, `edit_file { host }` and `open_shell { host }`. This
**reverses** 07a's documented rule that a file read must not prompt. A ghost
connects the same way with `BatchMode=yes`, so it never prompts and fails fast.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — ControlMaster flags; remote shells
  are PTY shells running `ssh`.
- `docs/dev/milestones/M21-host-model/README.md` § Notes — the measured
  "connection prompt eats the first command" defect (D1 → this row) and the
  PE decisions of 2026-09-29.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

**Shell-gate note.** Your shell refuses any command you type containing the
literal word for the privilege tool. Nothing in this phase needs it.

## Current state

Measured on **2026-09-29** at commit `66bb7e50`, against the live host `pinky`
(OpenSSH 10.5 locally; `pinky` is key-auth and also accepts passwords).

Baseline: `cargo test --lib` → **2064 passed / 0 failed / 11 ignored**.

### 1. The defect

A shell to a named host is a PTY running `ssh -tt -o ControlMaster=auto
-o ControlPath=<ssh_dir>/%C -o ControlPersist=10m <dest>` (`ssh_argv`,
`src/host.rs:119`). The first command is written as soon as the shell host's
socket is bound. When ssh is still authenticating, those bytes go to ssh's
password prompt (measured at 08c drafting; recorded in the README Notes) and
the run waits out its timeout. Phase-05's ssh classification lives only in the
sudo loop, which runs **after** the command has already been written.

### 2. Measured: connect first, as its own process

Driven through the crate's own `crate::shell::PtyShell` (a throwaway probe,
reverted):

| Case | Result |
|---|---|
| `ssh -M -N -f -o ControlPath=<dir>/%C -o ControlPersist=60 -o BatchMode=yes pinky`, key auth | the child **exits in 81 ms, no output**; the backgrounded master **survives dropping the `PtyShell`** |
| then `ssh -O check -o ControlPath=<dir>/%C pinky` | exit **0**, `Master running (pid=…)`, **2–3 ms** |
| then a shell `ssh -tt -o ControlPath=<dir>/%C -o ControlMaster=auto pinky` | connects through the master in **6 ms**, no auth |
| `ssh -O check` with no master | exit **255**, `Control socket connect(...): No such file or directory`, 2 ms |
| the same `-M -N -f` with `-o PubkeyAuthentication=no -o PreferredAuthentications=keyboard-interactive,password` | the prompt `matt@pinky.home.planetfoo.org's password: ` reaches a `subscribe()` taken right after `spawn` at **71 ms**; `kill()` leaves **no** master behind |

So "connected" has a deterministic test: **the `-M -N -f` child has exited
and `ssh -O check` returns 0**. `PtyShell` exposes `is_alive`, `kill`,
`subscribe`, `write_input` — not the exit status; the check replaces it.

### 3. Measured: the prompts, verbatim

Python PTY driver against `pinky`, `ControlMaster=no`:

- **Password:** `\rmatt@pinky.home.planetfoo.org's password: ` at ~60 ms.
  A wrong answer costs **~3.8 s**, then **one chunk**:
  `Permission denied, please try again.\r\r\n\rmatt@pinky.home.planetfoo.org's password: `.
  After the third wrong answer: `matt@pinky.home.planetfoo.org: Permission
  denied (publickey,password).\r\r\n` and ssh exits.
- **Unknown host key** (`-o UserKnownHostsFile=<empty> -o StrictHostKeyChecking=ask`):
  `The authenticity of host 'pinky.home.planetfoo.org (172.16.50.75)' can't be
  established.\r\nED25519 key fingerprint is: SHA256:…` ending
  `(yes/no/[fingerprint])? `; answering `no` → `Host key verification failed.`
- **Gotcha — order matters.** The retry chunk contains `Permission denied`
  **and** a fresh `password:` prompt. `is_ssh_permission_denied`
  (`src/daemon/utils/sudo.rs`) matches it too, so a handshake that checked
  "denied" before "prompt" would abort on the first wrong password. **Decide
  failure by the process exiting, never by the text** — classify prompts only.
- A line typed while ssh is **not** at its prompt (during the 3.8 s delay) is
  echoed back as plaintext. Answer only a prompt that has appeared in output
  **after** the last answer — the 08f rule.

### 4. What exists to reuse (`src/daemon/executor/shell_exec.rs`)

- `classify_ssh_prompt(out)` (`src/daemon/utils/sudo.rs`, re-exported via
  `crate::daemon::utils`): `SshHostKey` on "Are you sure you want to continue
  connecting", `SshPassphrase` on "Enter passphrase for", `SshPassword` on
  "'s password:".
- `credential_round_trip(id, tx, rx, prompt) -> PromptReply` (`:459`) — one
  `CredentialPrompt` / `CredentialResponse` round trip; on EOF (nobody
  attached — a ghost) it gives up with 08f's "nobody is attached to answer it"
  sentence.
- `ssh_credential_prompt(kind)` (`:439`) — "ssh login password required" /
  "ssh key passphrase required".
- The host-key refusal sentence (`:602-606`):

  ```rust
  "ssh is asking whether to trust host {host_name}'s unknown host key, \
   which cannot be answered here — accept the fingerprint once from \
   your own terminal, or wait for the new-host approval flow"
  ```

- `crate::shell::PtyShell` (`src/shell/pty.rs:189`, exported from
  `src/shell/mod.rs:35`): `spawn(program, &args, (rows, cols))`, `subscribe()`
  (a `tokio::sync::broadcast::Receiver<Vec<u8>>` — use `try_recv` in a loop),
  `write_input(&[u8])`, `is_alive()`, `kill()`. A chunk published before the
  first `subscribe` is dropped; ssh's prompts arrive ≥ 50 ms after spawn.
- `crate::shell::{host_entry_or_bare, which_ssh}`, `crate::config::ssh_dir()`.

### 5. The four callers (all in `src/daemon/executor/mod.rs` unless noted)

Every one holds `tx`, `rx` and the call id where it acquires the remote
shell, and **no existing test reaches these points** (tests enter below them,
through `run_in_shell_with_prompts`, `run_ghost_remote`, `*_with_spawn` or
`open_shell_with`, with in-process hosts named like `e2e-host` — a connect
step there would try a real `ssh e2e-host`).

- **`run_terminal_command { host }`** — the `HostId::Named(name)` arm, after
  its `host_entry_or_bare` check and **before** `run_in_shell_with_prompts`
  (`~:577-596`). The approval prompt has already run.
- **`read_file { host }`** — the `PendingCall::ReadFile` arm, before
  `file_ops::run_read_file(…)` (`~:931`).
- **`edit_file { host }`** — the `PendingCall::EditFile` arm, before
  `file_ops::run_edit_file(…)` (`~:946`). **Consequence:** `edit_file`'s own
  approval runs inside the operation, so on a *cold* host the ssh password is
  asked **before** the edit's approval panel. Accepted for this row.
- **`open_shell { host }`** — the `PendingCall::OpenShell` arm, before
  `knowledge::open_shell(…)` (`~:1156`).
- **The ghost** — `src/daemon/executor/foreground.rs`, 08c's branch, before
  `run_ghost_remote(…)` (`~:1160`), with `BatchMode`.

`file_ops::run_in_host_shell`'s doc comment (`src/daemon/executor/file_ops/mod.rs:100-105`)
says *"A file read must **not** prompt … a remote shell whose ssh handshake
needs a credential fails here …"* — false after this row.

### 6. The 08d residual

`GhostPolicy::for_host`'s `Err` already begins `Error: `
(`src/daemon/policy.rs:179`), and `run_background` wraps it in
`format!("Error: {e}")`, so the model reads `Error: Error: 'nope' is not a
configured host …`.

## Spec

### Task 1 — the connect argv and the master check (`src/host.rs`, `src/shell/session.rs`, `src/shell/mod.rs`)

1. In `src/host.rs`, beside `ssh_argv`:

   ```rust
   /// The argv that establishes (and backgrounds) this host's ControlMaster
   /// without a remote command: `-M -N -f`, the same `ControlPath` and
   /// `ControlPersist` as [`ssh_argv`] so every later shell multiplexes
   /// through it, and `-o BatchMode=yes` when `batch` (a ghost: never prompt).
   /// The destination is last — options after it become the remote command.
   pub fn connect_argv(entry: &HostEntry, control_path: &std::path::Path, batch: bool) -> Vec<String>
   ```

   Exactly: `-M`, `-N`, `-f`, `-o`, `ControlPath=<control_path>/%C`, `-o`,
   `ControlPersist=10m`, then `-o`, `BatchMode=yes` only when `batch`, then
   `entry.ssh`. **No `-tt`.** Build the `ControlPath=` element with the same
   `format!` `ssh_argv` uses, so the two can never differ.

2. In `src/shell/session.rs`, beside `spawn_ssh_host`, and export it from
   `src/shell/mod.rs` (`:46-49`):

   ```rust
   /// True when a ControlMaster is already running for `entry` under
   /// `control_path`: `ssh -O check` exits 0 (measured 2–3 ms).
   pub fn master_running(entry: &crate::config::HostEntry, control_path: &std::path::Path) -> bool
   ```

   `std::process::Command::new(which_ssh()?)` with args `-O`, `check`, `-o`,
   `ControlPath=<control_path>/%C`, `entry.ssh`; stdout/stderr null; `false`
   on any error.

### Task 2 — the connect driver (`src/daemon/executor/shell_exec.rs`)

1. The testable core — a driver with the program and the "connected" test
   injected:

   ```rust
   pub(super) async fn drive_connect<W, R, C>(
       program: &str,
       argv: &[String],
       connected: C,
       host_name: &str,
       prompt_id: &str,
       tx: &mut W,
       rx: &mut R,
       timeout: Duration,
   ) -> Result<(), String>
   where
       W: tokio::io::AsyncWriteExt + Unpin,
       R: tokio::io::AsyncBufReadExt + Unpin,
       C: Fn() -> bool,
   ```

   - `PtyShell::spawn(program, argv, (24, 80))`, then `subscribe()`
     **immediately**. Keep `buf: Vec<u8>` and `answered_at: usize` (08f's
     rule).
   - Loop, sleeping 20 ms (`tokio::time::sleep`) per turn:
     - drain `try_recv()` into `buf`;
     - **if the child has exited** (`!is_alive()`): return `Ok(())` when
       `connected()`, else `Err` naming the host and the last non-empty line
       of `buf` (lossy, `mask_sensitive`, trimmed) — e.g.
       `"ssh could not connect to host pinky: …Permission denied (publickey,password)."`;
     - classify **only** `buf[answered_at..]` with `classify_ssh_prompt`:
       `SshPassword` / `SshPassphrase` → `credential_round_trip(prompt_id,
       tx, rx, ssh_credential_prompt(kind))`; on `Send(bytes)` →
       `write_input(&bytes)` and `answered_at = buf.len()`, and restart the
       timeout; on `GiveUp(reason)` → `kill()` and `Err(reason)`.
       `SshHostKey` → `kill()` and `Err` with the host-key sentence
       (§ Current state 4, `{host_name}` filled). `None` → keep looping.
     - past `timeout` since start or the last answer → `kill()` and
       `Err(format!("ssh to host {host_name} did not connect within {timeout:?}"))`.
   - **Never** decide failure from the text `Permission denied` — only from
     the process exiting (§ Current state 3's gotcha).
   - The password is written only to the child's PTY. Nothing logs `buf` or
     the answer.

2. The production wrapper:

   ```rust
   /// Connect to named host `name` before any shell there is spawned: a no-op
   /// when its ControlMaster already runs, otherwise `ssh -M -N -f` driven
   /// through [`drive_connect`]. `batch` = a ghost (never prompt).
   pub(super) async fn ensure_connected<W, R>(
       name: &str,
       batch: bool,
       prompt_id: &str,
       tx: &mut W,
       rx: &mut R,
   ) -> Result<(), String>
   ```

   `host_entry_or_bare(name)` (map `Err` to its string), `ssh_dir()` +
   `create_dir_all`, return `Ok(())` if `master_running`, else
   `drive_connect(&which_ssh()?, &connect_argv(&entry, &control, batch),
   || master_running(&entry, &control), name, prompt_id, tx, rx,
   CONNECT_TIMEOUT)` with `const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);`.

### Task 3 — call it from the five production sites (`executor/mod.rs`, `foreground.rs`)

1. In `executor/mod.rs`, a small helper for the three tool arms that take a
   raw `host` string:

   ```rust
   /// Connect first for a named host; `None`, `"local"` and a malformed name
   /// pass through untouched — the tool itself refuses a malformed one with
   /// its existing wording.
   async fn connect_named_host<W, R>(host: Option<&str>, batch: bool, id: &str, tx: &mut W, rx: &mut R) -> Result<(), String>
   ```

   (`HostId::parse`; only `Ok(HostId::Named(name))` calls `ensure_connected`.)
2. `ReadFile`, `EditFile` and `OpenShell` arms: call
   `connect_named_host(host.as_deref(), is_ghost, id, tx, rx)` **before** the
   tool call; on `Err(msg)` return `Ok(ToolCallOutcome::Result(format!("Error: {msg}")))`
   (sending the `ToolResult` the same way each arm already does for its own
   refusals). Place it **after** each arm's existing ghost `hosts:` gate, so a
   refused host is never connected to.
3. The chat `run_terminal_command` `HostId::Named(name)` arm: call
   `shell_exec::ensure_connected(name, false, id, tx, rx)` after the
   `host_entry_or_bare` check and before `run_in_shell_with_prompts`; on
   `Err(msg)` report it exactly as that arm reports a `host_entry_or_bare`
   error (`"Error: {msg}"`, `finish_command(cmd_id, 1)`, `ToolResult`,
   `log_shell_command(…, "host-error", …)`).
4. The ghost branch in `foreground.rs`: before `run_ghost_remote`, call
   `super::shell_exec::ensure_connected(name, true, id, &mut tokio::io::sink(),
   &mut tokio::io::BufReader::new(tokio::io::empty()))`; on `Err(msg)` the
   branch's `output` is `format!("Error: {msg}")` with exit code 1, and it
   continues to its existing `finish_command` / `ToolResult` / `log_command`
   tail.

### Task 4 — the leftovers and docs

1. `src/daemon/executor/file_ops/mod.rs:100-105`: rewrite the comment — the
   read's command still runs through the plain `run_command` (no prompt
   mid-command), but the **connection** to a cold host may ask for a
   credential first, in the executor arm.
2. `src/daemon/policy.rs:179`: drop the leading `Error: ` from `for_host`'s
   message (the caller adds it). Test 2 of 08d still passes (it asserts on
   `nope`).
3. `README.md`: after the **Remote script streaming** paragraph (`:201`), one
   paragraph beginning **`**Remote hosts — first connection**`** saying: the
   first connection to a configured host is made before any shell opens
   there; a password or key passphrase is asked in the chat (never typed into
   a shell), an unknown host key is refused (accept it once from your own
   terminal), later shells to that host reuse the connection for 10 minutes
   without asking; a Ghost Shell never asks — a host that needs a credential
   fails for it at once.

### Task 5 — tests

Seven tests; names pinned.

In `src/host.rs` tests:

1. `connect_argv_establishes_the_master_the_shells_reuse` — for
   `entry("db01")`, `/tmp/cp`: begins `-M`, `-N`, `-f`; contains
   `ControlPersist=10m`; its `ControlPath=` element **equals** `ssh_argv`'s;
   last element is the destination; contains no `-tt`; `BatchMode=yes`
   present only with `batch = true`.

In `src/daemon/executor/shell_exec.rs` tests — `drive_connect` over a **fake
ssh**: `program = "sh"`, `argv = ["-c", <script>]`, and
`connected = || flag.exists()` for a temp-dir `flag` file. Each script
**sleeps 0.2 s before its first prompt** (a chunk published before
`subscribe` is dropped — measured to work without the sleep 3/3, kept as
margin). "Answered" transport = a `Vec<u8>` writer + a `BufReader` over a
`Cursor` of `Request::CredentialResponse` lines (credential `right-pw`);
"nobody attached" = `tokio::io::sink()` + `BufReader::new(tokio::io::empty())`:

2. `a_host_that_needs_no_credential_connects_without_a_prompt` — script:
   `sleep 0.2; touch <flag>; exit 0` → `Ok`; the writer received **no**
   `CredentialPrompt`.
3. `a_password_prompt_is_answered_once_and_the_host_connects` — script:
   `sleep 0.2; printf "u@h's password: "; read -r pw; [ "$pw" = right-pw ] && touch <flag> && exit 0; exit 255`
   → `Ok`; exactly **one** `CredentialPrompt` was written.
4. `a_wrong_password_is_asked_again_and_the_retry_line_does_not_abort` —
   script answers wrong once: prints the prompt, reads; if not `right-pw`,
   prints `Permission denied, please try again.\r\n` **and** the prompt again
   in one `printf`, reads again; `right-pw` → flag, exit 0. Transport holds
   `wrong-pw` then `right-pw` → `Ok`; **two** `CredentialPrompt`s.
5. `an_unanswerable_connection_prompt_fails_fast_and_leaves_nothing_running`
   — the password script, nobody attached → `Err` containing
   `nobody is attached` in **under 5 s**; no flag.
6. `an_unknown_host_key_is_refused` — script:
   `sleep 0.2; printf "Are you sure you want to continue connecting (yes/no/[fingerprint])? "; read -r a; exit 255`
   → `Err` containing `unknown host key` and `pinky` (pass `host_name =
   "pinky"`).

Live, ignored — in `shell_exec.rs` tests,
`#[ignore = "requires the configured host \`pinky\` reachable over ssh"]`:

7. `a_cold_host_connects_first_and_a_warm_one_is_reused_on_pinky` — hold
   `crate::test_home_guard()` with `HOME` at a temp dir (so `ssh_dir()` is
   fresh and cold) and `Config::ensure_dirs()`; `ensure_connected("pinky",
   false, "c", &mut sink, &mut eof)` → `Ok`; `master_running` for `pinky`
   under `ssh_dir()` → `true`; a second `ensure_connected` → `Ok` in under
   100 ms. Afterwards run `ssh -O exit -o ControlPath=<ssh_dir>/%C pinky`
   (`std::process::Command`) so no master outlives the test. `println!` each
   elapsed time.

**Test count:** six new and run; one new and ignored → at least **2070**
passed, **12** ignored.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 7 — the live check

Run test 7 by hand and paste its output into the same entry, after the
mutations:

```sh
cargo test --lib a_cold_host_connects_first_and_a_warm_one_is_reused_on_pinky -- --ignored --nocapture 2>&1 | grep -E 'elapsed|^test |test result'
```

**If `pinky` is unreachable, report the criterion unmet** — do not describe
what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today — checked at drafting.

- [ ] `cargo test --lib` → **at least 2070 passed / 0 failed / 12 ignored**,
      on three consecutive runs (current: 2064/0/11). **The ignored count is
      exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The pieces exist: ``grep -c 'pub fn connect_argv(' src/host.rs`` → **1**
      (current: 0); ``grep -c 'pub fn master_running(' src/shell/session.rs``
      → **1** (current: 0); ``grep -c 'async fn drive_connect<' src/daemon/executor/shell_exec.rs``
      → **1** (current: 0); ``grep -c 'async fn ensure_connected<' src/daemon/executor/shell_exec.rs``
      → **1** (current: 0).
- [ ] Every caller connects first:
      ``grep -c 'async fn connect_named_host<' src/daemon/executor/mod.rs`` →
      **1** and ``grep -c 'connect_named_host(' src/daemon/executor/mod.rs``
      → **3** (the three arms; the definition reads `connect_named_host<W, R>(`
      and does not match; current: 0 and 0);
      ``grep -c 'ensure_connected(' src/daemon/executor/mod.rs`` → **≥ 2**
      (the helper's call + the chat arm; current: 0);
      ``grep -c 'ensure_connected(' src/daemon/executor/foreground.rs`` →
      **1** (current: 0). *(Proxies — tests 2–6 pin the driver; the live
      test pins the production wrapper.)*
- [ ] The false read comment is gone:
      ``grep -c 'A file read must \*\*not\*\* prompt' src/daemon/executor/file_ops/mod.rs``
      → **0** (current: 1).
- [ ] The 08d residual is fixed:
      ``grep -c "\"Error: '{key}' is not a configured host" src/daemon/policy.rs``
      → **0** (current: 1).
- [ ] README: ``grep -c 'Remote hosts — first connection' README.md`` →
      **1** (current: 0).
- [ ] All six new non-ignored tests pass:

      ```sh
      cargo test --lib -- \
        connect_argv_establishes_the_master_the_shells_reuse \
        a_host_that_needs_no_credential_connects_without_a_prompt \
        a_password_prompt_is_answered_once_and_the_host_connects \
        a_wrong_password_is_asked_again_and_the_retry_line_does_not_abort \
        an_unanswerable_connection_prompt_fails_fast_and_leaves_nothing_running \
        an_unknown_host_key_is_refused
      ```

      → **6 passed** (current: `0 passed; 0 failed; … filtered out`).
- [ ] Neighbours still pass (*preservation*): ``cargo test --lib ssh_argv``
      → **0 failed**; ``cargo test --lib ghost_remote`` → **0 failed**;
      ``cargo test --lib file_ops`` → **0 failed**.
- [ ] Mutations A–C were applied, run, and their observed results recorded in
      the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.
- [ ] The Task 7 live check was run and its output pasted into the same
      entry, or reported unmet with the reason.

**Bounce criteria (2026-09-28)** — each fails against the tree at `885963b1`:

- [ ] bug-08e-1 — the connect timeout restarts after an answer:
      ``cargo test --lib an_answered_prompt_restarts_the_connect_timeout``
      → **1 passed** (current: `0 passed; … filtered out`).
- [ ] bug-08e-2 — a ghost's `edit_file` never connects:
      ``cargo test --lib a_ghost_edit_file_never_connects`` → **1 passed**
      (current: `0 passed; … filtered out`). The test asserts the ghost
      refusal text and that `ensure_connected` is not reached.
- [ ] bug-08e-3 — Mutation A is observable: test 3's fake stays alive after
      the correct answer; Mutation A re-applied turns it **red**, recorded in
      the new end-to-end entry (current: recorded green).
- [ ] `cargo test --lib` → **at least 2072 passed / 0 failed / 12 ignored**
      (two new tests; the ignored count is exact).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.** Reasoned against code this phase writes — record
what you actually observe. **Do not skip them**: if the budget runs short,
say so in the Update Log — but they are part of done.

- **Mutation A — classify the whole buffer.** In `drive_connect`, classify
  `&buf` instead of `&buf[answered_at..]`. Run test 3 — expected **red** (the
  answered prompt is still in the buffer and is answered again).
- **Mutation B — fail on the retry text.** Before classifying, return `Err`
  whenever the new bytes contain `Permission denied`. Run test 4 — expected
  **red** (the retry chunk aborts the login).
- **Mutation C — skip the master check.** In `ensure_connected`, drop the
  `master_running` early return. Run test 7 by hand — expected: the second
  call is **no longer under 100 ms** (a fresh `-M` master attempt instead of a
  no-op), or report the observed timing.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Seven tests (Task 5): the connect argv (pure); the driver over a fake ssh for
no-credential, answered-once, wrong-then-right (the retry line), nobody
attached, and unknown host key; and the live cold-then-warm run on `pinky`.
Existing named-host tests enter below the connect step and are unaffected by
design (§ Current state 5).

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- connect_argv_establishes_the_master_the_shells_reuse a_host_that_needs_no_credential_connects_without_a_prompt a_password_prompt_is_answered_once_and_the_host_connects a_wrong_password_is_asked_again_and_the_retry_line_does_not_abort an_unanswerable_connection_prompt_fails_fast_and_leaves_nothing_running an_unknown_host_key_is_refused 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'neighbours=%s/%s/%s\n' "$(cargo test --lib ssh_argv 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ failed')" "$(cargo test --lib ghost_remote 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ failed')" "$(cargo test --lib file_ops 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ failed')"
  printf 'pieces=%s-%s-%s-%s\n' "$(grep -c 'pub fn connect_argv(' src/host.rs)" "$(grep -c 'pub fn master_running(' src/shell/session.rs)" "$(grep -c 'async fn drive_connect<' src/daemon/executor/shell_exec.rs)" "$(grep -c 'async fn ensure_connected<' src/daemon/executor/shell_exec.rs)"
  printf 'callers=%s-%s-%s-%s\n' "$(grep -c 'async fn connect_named_host<' src/daemon/executor/mod.rs)" "$(grep -c 'connect_named_host(' src/daemon/executor/mod.rs)" "$(grep -c 'ensure_connected(' src/daemon/executor/mod.rs)" "$(grep -c 'ensure_connected(' src/daemon/executor/foreground.rs)"
  printf 'read_comment=%s\n' "$(grep -c 'A file read must \*\*not\*\* prompt' src/daemon/executor/file_ops/mod.rs)"
  printf 'double_error=%s\n' "$(grep -c "\"Error: '{key}' is not a configured host" src/daemon/policy.rs)"
  printf 'readme=%s\n' "$(grep -c 'Remote hosts — first connection' README.md)"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
  printf 'bounce_tests=%s\n' "$(cargo test --lib -- an_answered_prompt_restarts_the_connect_timeout a_ghost_edit_file_never_connects 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
} > /tmp/e2e-08e.txt
cat /tmp/e2e-08e.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=` at least
`2070p-0f-12i`, `new_tests=6p-0f`, `neighbours=0 failed/0 failed/0 failed`,
`pieces=1-1-1-1`, `callers=1-3-` then at least `2`, then `-1`,
`read_comment=0`, `double_error=0`, `readme=1`, `ignore_total=12`,
`bounce_tests=2p-0f` (added at the 2026-09-28 bounce; `lib_counts=` now at
least `2072p-0f-12i`).

**Known pre-existing flake.** `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
fails intermittently under full-suite load. If it fails, report its name —
do not edit it — and re-run.

**Run this block with nothing else running `cargo test`.**

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-08e-remote-connect-handshake.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08e.txt
diff /tmp/pasted-08e.txt /tmp/e2e-08e.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results and
the live check belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/host.rs` — Task 1.1, test 1.
- `src/shell/session.rs`, `src/shell/mod.rs` — Task 1.2.
- `src/daemon/executor/shell_exec.rs` — Task 2, tests 2–7.
- `src/daemon/executor/mod.rs` — Task 3.1–3.3.
- `src/daemon/executor/foreground.rs` — Task 3.4.
- `src/daemon/executor/file_ops/mod.rs` — Task 4.1 (a comment only).
- `src/daemon/policy.rs` — Task 4.2 (one message).
- `README.md` — Task 4.3 only.
- This phase doc's Update Log.
- **Bounce (2026-09-28):** `src/daemon/executor/shell_exec.rs` (bug-08e-1's
  fix and test, bug-08e-3's test change); `src/daemon/executor/mod.rs`
  (bug-08e-2's fix and test); `src/daemon/executor/file_ops/write.rs` only if
  bug-08e-2's chosen fix needs it.

## Out of scope

- **`ssh_argv` itself** — unchanged; shells still use `ControlMaster=auto`.
  A master that dies between the check and a shell's spawn makes that shell
  connect on its own, as today (rare; not handled here).
- **Accepting an unknown host key** — still refused; the new-host trust flow
  is not this row.
- **Reordering `edit_file`'s approval** before the connect step.
- **Stale masters** — `ControlPersist=10m` bounds them, as today.
- **The backend redaction-marker flake** — recorded, not fixed here.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-29 01:11 UTC (started)

Executor session (Claude) opened. Status flipped `todo` → `in-progress` and the
milestone README phase-table row updated to match; baseline confirmed:
`cargo test --lib` → 2064 passed / 0 failed / 11 ignored at commit `66bb7e50`.
Working through the Spec in order.

### Update — 2026-09-29 02:11 UTC (end-to-end verification)

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2070p-0f-12i
new_tests=6p-0f
neighbours=0 failed/0 failed/0 failed
pieces=1-1-1-1
callers=1-3-2-1
read_comment=0
double_error=0
readme=1
ignore_total=12
```

**PASTE MATCH** — `diff /tmp/pasted-08e.txt /tmp/e2e-08e.txt` exited 0 after
pasting the block above; every expected literal is in place (`lib1..3=ok` with
nothing after, `2070p-0f-12i`, `new_tests=6p-0f`, `neighbours=0/0/0`,
`pieces=1-1-1-1`, `callers=1-3-2-1`, `read_comment=0`, `double_error=0`,
`readme=1`, `ignore_total=12`).

The known pre-existing flake `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
did not appear in any of the five full-suite runs captured above.

### Mutations

- **Mutation A — classify the whole buffer** (`&buf` instead of
  `&buf[answered_at..]`): applied, run — **observed GREEN on tests 3 and 4,
  not the red the spec predicted.** Reason, from the code: the child-exit
  branch precedes classification, and both fakes touch their flag and exit
  immediately after the right answer, so the next loop turn returns
  `Ok(connected())` before the stale prompt can be re-classified. The guard
  is still load-bearing for the real ssh — a warm-up chunk can arrive while
  the child stays alive (a slow handshake), which the fakes never exercise —
  so the mutation was restored and the 08f window kept. Recorded as observed,
  not as the spec's prediction.
- **Mutation B — fail on the retry text**: applied (`Err` whenever the new
  bytes contain `Permission denied`, before classifying), run test 4 —
  **observed RED**: `the retry connects: Err("MUTATION-B: denied text")`,
  exactly the recoverable-login abort the process-exit rule exists to prevent.
  Restored; `git diff --stat` back to pre-mutation.
- **Mutation C — skip the master check**: applied (`master_running` early
  return disabled), ran test 7 by hand — **observed**: `cold connect elapsed:
  109.570113ms`, `warm check elapsed: 87.773306ms` — the second call was a
  fresh master attempt (109→88 ms, both above the 100 ms line's spirit), not
  the no-op. Restored, re-run by hand: cold 110.534201 ms, **warm
  1.786245 ms** — the early return is what makes warm cheap. After each
  restore `git diff --stat` matched its pre-mutation state.

### Task 7 — the live check (host `pinky`)

```sh
$ cargo test --lib a_cold_host_connects_first_and_a_warm_one_is_reused_on_pinky -- --ignored --nocapture 2>&1 | grep -E 'elapsed|^test |test result'
cold connect elapsed: 110.534201ms
warm check elapsed: 1.786245ms
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2081 filtered out; finished in 0.12s
```

The criterion is met: cold connect succeeds with no credential asked (pinky
is key-auth), the master is found running afterwards, and the warm re-check
completes in **1.8 ms** — under the 100 ms line. The test's cleanup ran
`ssh -O exit` afterwards (exit 255 "Control socket connect … No such file or
directory" confirms no master outlived the test).

### Notes for review

- **Test-helper spawn retry.** The five non-live connect tests each fork a
  real `/bin/sh` into a PTY. Under full-suite load, portable-pty 0.9's
  `spawn_command` intermittently fails with `spawn_command failed: No such
  file or directory (os error 2)` — measured on the *first* spawn of a
  concurrently-spawning thread, diagnosed to the `TIOCSCTTY` pre_exec path
  (`controlling_tty` defaults true; a plain `std::process::Command` loop
  under the same load never failed, and 24 threads × 12 sequential PTY
  spawns in isolation never failed). `PtyShell::spawn` is production code I
  may not change; the retry-on-spawn-failure lives **only in the test
  helper** (`drive_fake`), one 20 ms pause and one re-attempt, with the
  measured evidence in this entry. Production `drive_connect` keeps a single
  spawn attempt. Adjacent observation for the principal engineer, not fixed
  here: production `PtyShell::spawn` carries the same transient exposure.
- **Test serialization.** The five non-live connect tests also hold a module
  `tokio::sync::Mutex` (`PTY_TEST_LOCK`) so they run one at a time; this
  bounded the flake while the diagnosis was under way and is kept — each
  test still does its whole job alone.

### Update — 2026-09-29 02:13 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Built phase 08e: `connect_argv` (`src/host.rs`) and `master_running` (`src/shell/session.rs`, re-exported) express "establish the ControlMaster" and "is it up"; `drive_connect`/`ensure_connected` (`shell_exec.rs`) drive `ssh -M -N -f` through a daemon-owned PTY — 20 ms poll loop, prompts classified only on bytes after the last answer (08f's rule), failure decided by the child exiting, never by the `Permission denied` text, unknown host key refused with the pinned sentence — and five production call sites connect first (the three `connect_named_host` tool arms after their ghost `hosts:` gates, the chat `run_terminal_command` `Named` arm after `host_entry_or_bare`, and the ghost branch in `foreground.rs` with `BatchMode`), plus the 08d `Error: Error:` fix in `policy.rs`, the corrected `file_ops` comment, and the README paragraph. E2E verification pasted verbatim into an `end-to-end verification` entry whose diff printed `PASTE MATCH`: `cargo test --lib` → 2070 passed / 0 failed / **12** ignored on three consecutive runs (expectation was ≥2070/0/12), `new_tests=6p-0f`, `neighbours=0/0/0`, `pieces=1-1-1-1`, `callers=1-3-2-1`; `fmt`/`build`/`clippy -D warnings`/`cargo test` all clean; the live `pinky` check ran — cold connect 110.5 ms, warm re-check **1.8 ms** — with the master cleaned up. Mutations A–C applied one run each and recorded as observed: B red as predicted; A stayed green (the exit check precedes classification, so the flag-and-exit fakes never expose a stale prompt — the 08f window was kept anyway and I flag the gap between the spec's prediction and the observation); C showed the second call degrading to a fresh master attempt (~88 ms) versus 1.8 ms with the check. One test-infrastructure adaptation worth review: portable-pty 0.9's `TIOCSCTTY` pre_exec intermittently returns ENOENT under full-suite PTY churn, so the five non-live tests hold a module `tokio::sync::Mutex` and `drive_fake` retries a failed PTY spawn once (production keeps a single attempt) — measured evidence in the phase doc's Update Log; production `PtyShell::spawn` carries the same transient exposure and is flagged, not fixed. Status remains `in-progress`; commit `885963b1` contains every change and `git status --short` is clean.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
n_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_uses_first_occurrence_only ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test readme_tools_tables_match_the_code ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 11 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 10 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +2 -0
- `docs/dev/milestones/M21-host-model/phase-08e-remote-connect-handshake.md` — +94 -1
- `src/daemon/executor/file_ops/mod.rs` — +5 -5
- `src/daemon/executor/foreground.rs` — +32 -0
- `src/daemon/executor/mod.rs` — +96 -0
- `src/daemon/executor/shell_exec.rs` — +512 -5
- `src/daemon/policy.rs` — +1 -1
- `src/host.rs` — +73 -2
- `src/shell/mod.rs` — +3 -2
- `src/shell/session.rs` — +23 -0

**Commit:** 885963b11ae2accee44e55f22c384545a877460c

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review — 2026-09-28 (bounced)

Reviewer re-ran the gates independently: `cargo fmt --all -- --check` clean,
`cargo build` clean, clippy `-D warnings` clean, `cargo test` →
lib 2070/0/12, bug_tracker 6, doc_truth 14, integration 32 (+2 ignored),
isolation 10 (+1 ignored). The E2E capture and the live pinky check stand.

Bounced on three defects, each reproduced by the reviewer:

- [bug-08e-1](bugs/bug-08e-1.md) (major) — the connect timeout is measured
  from spawn only (`shell_exec.rs:580`); an answer never restarts it,
  contradicting Task 2.1 and the code's own doc comment. Throwaway probe:
  `Err("ssh to host pinky did not connect within 700ms")` for a login that
  finished 500 ms after its answer.
- [bug-08e-2](bugs/bug-08e-2.md) (major) — the `EditFile` arm connects before
  `edit_file`'s ghost refusal (which lives in `file_ops/write.rs:89`), so a
  ghost's `edit_file { host }` opens a BatchMode ControlMaster to any host,
  listed or not.
- [bug-08e-3](bugs/bug-08e-3.md) (minor) — Mutation A survives (the executor
  recorded it honestly); `sleep 0.3` after test 3's `read` makes it red 3/3
  and leaves the unmutated tree green 3/3.

Accepted, noted for the verdict:

- `#[allow(clippy::too_many_arguments)]` on `drive_connect` — forced by the
  spec's pinned 8-parameter signature; four precedents in the same file.
- **Out-of-authorization edit:** `ssh_argv_does_not_set_batch_mode`
  (`src/host.rs`) had 04a's deliberate `"Batch''Mode"` split collapsed back
  to `"BatchMode"`. Behaviour-equivalent; not required to revert, but
  counted as a scope deviation.
- The test-only PTY spawn retry and `PTY_TEST_LOCK`, with the flagged
  production exposure of `PtyShell::spawn` to the same transient ENOENT — for
  the PE.


### Update — 2026-09-29 04:14 UTC (started, bounce fixes)

Executor session (Claude) re-opened on the 2026-09-28 bounce. Status stays
`in-progress`; the milestone README's phase-table row already reads
`in-progress`, so nothing to flip. Baseline confirmed at commit `808ff7c6`:
`cargo test --lib` → 2070 passed / 0 failed / 12 ignored. Fixing
bug-08e-1 (the connect timeout never restarts after an answer),
bug-08e-2 (a ghost's `edit_file` connects before its refusal) and
bug-08e-3 (test 3's fake now stays alive after the right answer, so
Mutation A is observable). Working through the bounce criteria in order.

### Update — 2026-09-29 04:14 UTC (end-to-end verification, bounce fixes)

The bounce capture, run with nothing else running `cargo test`:

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2073p-0f-12i
new_tests=6p-0f
neighbours=0 failed/0 failed/0 failed
pieces=1-1-1-1
callers=1-3-2-1
read_comment=0
double_error=0
readme=1
ignore_total=12
bounce_tests=2p-0f
```

**PASTE MATCH** — the diff snippet below printed the literal `PASTE MATCH`
into this entry (the block above is this entry's first fenced block; the
paste was re-verified after the last capture re-run).

Bounce criteria, each against its defect:

- **bug-08e-1** — `cargo test --lib an_answered_prompt_restarts_the_connect_timeout`
  → **1 passed; 0 failed** (was `0 passed; … filtered out`). The fix:
  `started` is reset to `last_answer` on every send, so the deadline is
  measured from the later of spawn and the last answer, exactly what the
  doc comment and Task 2.1 already promised. The never-answered half is
  pinned in the same test: a fake that prints nothing times out from spawn
  with the `did not connect within 300ms` wording, and is killed at the
  deadline, not later.
- **bug-08e-2** — `cargo test --lib a_ghost_edit_file_never_connects`
  → **1 passed; 0 failed** (was `0 passed; … filtered out`). The fix: the
  `EditFile` arm's connect step is gated behind `edit_file_connects_first`
  (`executor/mod.rs`), false for every ghost — a ghost's `edit_file`
  reaches `run_edit_file`'s own `cannot run in a Ghost Shell` refusal
  **without** any ssh process. A companion test pins the chat side: chat
  connects first for a well-formed name, and passes through for `None`,
  `"local"` and a malformed name. `file_ops/write.rs` needed no change.
- **bug-08e-3** — test 3's fake now sleeps 0.3 s after the read (ssh's
  post-answer auth latency), so the answered-once window is observable.
  **Mutation A re-applied → RED 3/3**, recorded below; unmutated → green
  3/3. `cargo test --lib` → **2073 passed / 0 failed / 12 ignored**
  (bounce floor 2072; ignored count exact).

Neighbours unmoved: `ssh_argv` 0 failed, `ghost_remote` 0 failed,
`file_ops` 0 failed. The known pre-existing flake
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
did not appear in the four full-suite runs captured here.

### Mutation A re-run (bug-08e-3)

Mutation A (classify `&buf` instead of `&buf[answered_at..]`) was applied
with the `patch` tool, run once, restored:

```
test a_password_prompt_is_answered_once_and_the_host_connects ... FAILED
the fake connects: Err("a password prompt appeared and nobody is attached to answer it …")
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2084 filtered out
```

Red 3/3 on repeated runs — the stale prompt is re-classified, the second
round trip hits the exhausted transport, the give-up sentence comes back.
Restored; `git diff --stat` back to its pre-mutation state and the test
green 3/3 on the unmutated tree. The 08f window is now pinned by a test
that fails without it.

### Notes for review (bounce fixes)

- **bug-08e-1's shape.** The reviewer's probe measured "connects ~0.5 s
  after the answer, timeout 700 ms" — the test uses prompt-at-0.4 s /
  connect-0.5-s-after-answer / 700 ms and asserts `Ok(())` plus the flag,
  so the old spawn-only deadline cannot pass it (it fires at 0.7 s, before
  the 0.9 s connect).
- **bug-08e-2's chosen fix.** A pure decision function
  (`edit_file_connects_first(is_ghost, host)`) consulted by the `EditFile`
  arm, unit-tested for all four ghost host shapes and the chat side's
  pass-throughs — no change to `file_ops/write.rs` needed. The three
  `connect_named_host(` call sites remain (read_file, open_shell, and now
  edit_file behind the gate), so the `callers=` criterion is unchanged:
  `1-3-2-1` in the capture above.
- **bug-08e-3's test shape.** The reviewer's measured recipe (sleep 0.3
  after test 3's read) is applied verbatim; the comment in the test names
  the mechanism and the mutation that pins it.
