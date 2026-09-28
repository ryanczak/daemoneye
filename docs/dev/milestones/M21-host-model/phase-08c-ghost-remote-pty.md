# Phase 08c: ghost-remote-pty — a ghost's command on a resolved host runs in a PTY shell there, and its shells close when it ends

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-08a (a runbook's `ssh_target` resolved against
`[hosts]`), phase-08b (a dead shell fails fast), phase-08f (a password prompt
nobody can answer is cancelled and leaves the shell usable)
**Estimated diff:** ~350 lines across 9 files, plus 7 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

When a ghost's runbook `ssh_target` **resolved to a configured host** (08a),
its `run_terminal_command` runs in a **PTY shell on that host** — the same
substrate chat uses — instead of `ssh <target> '<cmd>'` in a tmux window:

- the shell is owned by the ghost (`Owner::Ghost(<session id>)`) and reused
  for the ghost's later commands on that host;
- it is opened with **`-o BatchMode=yes`**, so ssh never prompts: a host that
  wants a password or an unknown key fails in milliseconds (08b reports it);
- ControlMaster comes for free (the shared `ssh_argv`);
- the command runs **synchronously, capped at 300 s** (PE, 2026-09-27);
- a password prompt from a remote elevated command is cancelled at once with
  an honest message (08f), and the shell stays usable;
- when the ghost ends, **its shells are closed** (PE, 2026-09-27).

An `ssh_target` that did **not** resolve keeps the 1.x tmux wrap byte for
byte, and so does `[execution] backend = "tmux"`.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — remote shells are PTY shells whose
  program is `ssh`; ControlMaster flags; § 8 PE decision 3 (PTY for
  everything remote).
- `docs/dev/milestones/M21-host-model/README.md` § Notes — the 08 split and
  its PE decisions; the 08f entry (BatchMode, TERM and script-streaming
  measurements).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

**Shell-gate note.** Your shell refuses any command you type that contains
the literal word for the privilege tool. Every pinned test name and every
criterion below avoids it; do the same in anything you type. Source-file
**content** is unaffected — edit it with the patch tool.

## Current state

Measured on **2026-09-28** at commit `b837bef4`, on a clean tree.

Baseline: `cargo test --lib` → **2052 passed / 0 failed / 10 ignored**.

### 1. Where a ghost's command runs today

Every ghost `run_terminal_command` becomes `PendingCall::Background`
(`src/daemon/ghost.rs:893-910`). `execute_tool_call`'s `Background` arm sends
chat to the PTY substrate and **everything else** — ghosts included — to
`foreground::run_background` (`src/daemon/executor/mod.rs:670-674`,
`uses_pty && !is_ghost && session_id.is_some()`).

`run_background` (`src/daemon/executor/foreground.rs:918`) runs, in order:
the `hosts:` fence and the `sudo_scripts` gate (08a, before approval),
`policy.resolve_command` (`:1037`), the approval prompt (`:1043`), the
remote-script build — stream by default, materialize under elevation
(`:1062-1101`) — then wraps and runs in tmux:

```rust
    // Ghost shells: wrap the approved command in `ssh <target> <cmd>` when configured.
    let ssh_wrapped_cmd;
    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
        ssh_wrapped_cmd = policy.wrap_remote(cmd);
        ssh_wrapped_cmd.as_str()
    } else {
        cmd
    };
```

(`:1103-1110`), and at the end (`:1178-1198`):

```rust
    let session_id_owned = session_id.map(|s| s.to_string());
    let output = run_background_in_window(
        session_name,
        id,
        cmd_id,
        cmd,
        credential.as_ref().map(|z| z.as_str()),
        session_id_owned,
        sessions.clone(),
    )
    .await;
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: output.clone(),
        },
    )
    .await?;
    log_command(session_id, "background", "", cmd, "approved", &output);
    Ok(ToolCallOutcome::Result(output))
```

`policy.remote_host` (`src/daemon/policy.rs:36`) is `Some(HostId::Named(key))`
exactly when 08a resolved the `ssh_target` to a configured host. It is the
switch this phase reads. With `[sandbox] enabled`, `run_background_in_window`
runs the command in a `--network=none` container, where an `ssh` wrap has no
network (read, not executed) — the PTY branch runs **before** that, so a
remote ghost command is allowed with the sandbox on (PE 2026-09-27: the
`hosts:` list is the fence).

### 2. The substrate, measured on `pinky` (2026-09-27)

A throwaway in-crate probe drove the real `run_in_shell_with_prompts` over an
in-process host running real `ssh` with `ssh_argv` plus `-o BatchMode=yes`
(inserted **before** the destination), owner `Owner::Ghost`, nobody attached:

| Command | Result |
|---|---|
| `hostname; echo X_OK` (first, cold) | exit 0, `pinky\nX_OK`, 470 ms |
| `false` (reused shell) | exit 1, 12 ms |
| `remote_stream_cmd` of a 3-line script with args `a b` | exit 3, `ARGS=a b`, 19 ms |
| `remote_stream_cmd` of a ~3 KB script (a 25 KB typed command) | exit 0, `BIG_OK`, 102 ms |

So the script-shipping commands `remote_stream_cmd` / `remote_materialize_cmd`
build (`src/scripts.rs:396`, `:299`) work typed into the interactive remote
shell — their semantics are untouched. 08f measured the rest: an elevated
command whose password prompt nobody answers gives up in ~0.33 s with
`nobody is attached to answer it`, and the shell's next command runs in
~0.01–0.03 s. 08b measured an unknown host key under BatchMode: ssh exits in
17 ms and the run reports `exited`.

### 3. Closing a shell: TERM, measured

`close_shell_with` (`src/daemon/executor/knowledge/shell.rs:1913`) sends
`ShellSignal::Term` and observes. Measured on in-process hosts: **ssh**
(the ghost's program) exits ~100 ms after TERM; `cat` and
`sh -c 'sleep 1000'` exit in under 1 ms; **an interactive `bash` ignores
TERM** (still `Idle` after 3 s). So a close test must use a
`sh -c 'sleep 1000'` host, **not** bash. `close_shell_with` also polls the
host's lock for up to 1.5 s; an in-process host never releases it, so its
**returned sentence** will say the lock is still held — assert on
`crate::shell::status`, not on that sentence.

Nothing closes shells by owner today, and every ghost run is a new owner:
a scheduled remote ghost would leave one idle ssh shell host per run.

### 4. Where a ghost run ends

`trigger_ghost_turn` (`src/daemon/ghost.rs:481`) runs the whole ghost loop and
then, **on every exit path**, writes the mailbox and tears down the ghost's
containers:

```rust
    let (sbx, sid) = (config.sandbox.clone(), session_id.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::teardown_ghost_containers(&sbx, &sid)
    })
    .await;
    result
}
```

That is where the ghost's shells are closed (Task 4).

### 5. Four small leftovers this phase carries

- `ghost_host_refusal` is `pub(super)` (`src/daemon/executor/mod.rs:180`); it
  only needs to be private — `foreground.rs` is a child module and imports it
  with `use super::ghost_host_refusal;`, which a private item allows (08a
  review verified the tree builds).
- `refusal_gate_tests` (`src/daemon/executor/foreground.rs:1534`) defines its
  own `HomeGuard` (`:1588-1616`) that re-implements `crate::test_home_guard()`
  and `std::mem::forget`s its `TempDir`, **leaking one directory per run**.
- `run_in_shell_with_prompts`'s doc comment (`src/daemon/executor/shell_exec.rs:76`)
  opens *"The non-chat entry point: the entry point `executor/mod.rs` calls
  with a pub/sub transport"* — self-contradictory.
- `src/daemon/ghost.rs:899` says *"Ghost Shell host routing is row 08b, not
  this row"* — that row is now **08d**.

## Spec

### Task 1 — a BatchMode argv and a ghost spawn (`src/host.rs`, `src/shell/session.rs`, `src/shell/mod.rs`)

1. In `src/host.rs`, beside `ssh_argv` (leave it and its
   `ssh_argv_does_not_set_batch_mode` pin untouched):

   ```rust
   /// `ssh_argv` for a ghost's shell: the same argv with `-o BatchMode=yes`
   /// inserted **before the destination**, so ssh never prompts — a host that
   /// wants a password or an unknown key fails at once instead of waiting for
   /// an answer nobody can give. Options after the destination would become
   /// the remote command, so the insertion point is load-bearing.
   pub fn ghost_ssh_argv(entry: &HostEntry, control_path: &std::path::Path) -> Vec<String> {
       let mut argv = ssh_argv(entry, control_path);
       let dest = argv.len() - 1;
       argv.splice(dest..dest, ["-o".to_string(), "BatchMode=yes".to_string()]);
       argv
   }
   ```

2. In `src/shell/session.rs`, beside `spawn_ssh_host` (`:146`), the same shape
   with the **strict** lookup and the ghost argv:

   ```rust
   pub async fn spawn_ghost_ssh_host(id: ShellId, name: &str, rows: u16, cols: u16) -> anyhow::Result<()> {
       let entry = host_entry(name)?;
       let control = crate::config::ssh_dir();
       std::fs::create_dir_all(&control).with_context(|| format!("creating {}", control.display()))?;
       let argv = crate::host::ghost_ssh_argv(&entry, &control);
       let program = which_ssh()?;
       launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await
   }
   ```

   `host_entry` (strict), not `host_entry_or_bare`: 08a only sets
   `remote_host` for a **configured** host. Export it from `src/shell/mod.rs`
   beside `spawn_ssh_host` (`:46-49`).

### Task 2 — the ghost's remote run (`src/daemon/executor/ghost_remote.rs`, new)

Declare `mod ghost_remote;` in `src/daemon/executor/mod.rs` beside
`mod shell_exec;`. The module holds:

```rust
/// How long a ghost's remote command may run: the ghost turn budget
/// (`GHOST_TURN_TIMEOUT_SECS` in `daemon/ghost.rs`). PE 2026-09-27.
pub(super) const GHOST_REMOTE_TIMEOUT_SECS: u64 = 300;
```

1. `pub(super) fn ghost_remote_host(is_ghost: bool, uses_pty: bool, policy: Option<&GhostPolicy>) -> Option<crate::host::HostId>`
   — `policy.remote_host.clone()` when `is_ghost && uses_pty`, else `None`.
   Pure. The sandbox is deliberately **not** an input.

2. `pub(super) async fn run_ghost_remote<F, Fut>(reg: &ShellRegistry, owner: Owner, host: HostId, cmd: &str, call_id: &str, spawn: F) -> (String, i32)`
   — calls `shell_exec::run_in_shell_with_prompts` with `max_per_owner` from
   `Config::load().unwrap_or_default().shells.max_per_owner`, `shell: None`,
   `Some(GHOST_REMOTE_TIMEOUT_SECS)`, `call_id` as the prompt id, and **its
   own** `tokio::io::sink()` / `tokio::io::BufReader::new(tokio::io::empty())`
   as the transport — a ghost never answers a prompt, whatever the caller's
   `tx`/`rx` are. `Ok((_, result))` → `(result.text, result.exit_code)`;
   `Err(e)` → `(super::shell_run_error_text(&shell_id, &e), 1)` where
   `shell_id` is `crate::shell::registry_lookup(&owner, |id| id.to_string()).unwrap_or_else(|| "s?".to_string())`
   — the exact shape the chat arm uses (`mod.rs`, the PTY `Err` arms).

### Task 3 — route a resolved host onto it (`src/daemon/executor/foreground.rs`)

In `run_background`, **after** the remote-script build (`:1062-1101`) and
**before** the `wrap_remote` block (`:1103`), branch:

```rust
    let uses_pty = crate::config::Config::load()
        .unwrap_or_default()
        .execution
        .uses_pty();
    if let Some(host) = super::ghost_remote::ghost_remote_host(is_ghost, uses_pty, ghost_policy)
        && let crate::host::HostId::Named(name) = &host
    {
        let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
        let (output, exit_code) = super::ghost_remote::run_ghost_remote(
            crate::shell::registry(),
            owner,
            host.clone(),
            cmd,
            id,
            |shell_id| crate::shell::spawn_ghost_ssh_host(shell_id, name, 24, 80),
        )
        .await;
        crate::daemon::stats::finish_command(cmd_id, exit_code);
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: output.clone(),
            },
        )
        .await?;
        log_command(session_id, "background", name, cmd, "approved", &output);
        return Ok(ToolCallOutcome::Result(output));
    }
```

`ghost_policy` is already `Option<&GhostPolicy>` here (`GhostCtx.policy`,
`src/daemon/executor/mod.rs:33`), which is what `ghost_remote_host` takes. The
rows/cols match the chat arm's `spawn_ssh_host(id, name, 24, 80)`.
Everything **before** the branch — both 08a gates, `resolve_command`, the
approval prompt, the remote-script build — is unchanged, so a ghost's
command is fenced, resolved and approved exactly as today. Everything
**after** it (the `wrap_remote` block, the credential block,
`run_background_in_window`) is unchanged and still serves an unresolved
`ssh_target`, a local ghost, and `backend = "tmux"`.

### Task 4 — close a ghost's shells when it ends (`src/daemon/executor/knowledge/shell.rs`, `knowledge/mod.rs`, `src/daemon/ghost.rs`)

1. Beside `close_shell_with` in `knowledge/shell.rs`:

   `pub(crate) async fn close_owner_shells(reg: &ShellRegistry, owner: &Owner, signal_timeout: Duration) -> Vec<(String, String)>`
   — collect (under `with_shells`) the ids of every record whose
   `owner.key() == owner.key()`, sorted by index; for each, call
   `close_shell_with(reg, &id, owner, signal_timeout)`; return
   `(id, sentence)` pairs. Records of any other owner are never touched.
   Re-export it from `knowledge/mod.rs` next to `close_shell_with` (`:25`).

2. In `trigger_ghost_turn`, after the container teardown and before `result`:
   close `Owner::Ghost(session_id.to_string())`'s shells through
   `crate::shell::registry()` with a 5 s signal timeout, and when any were
   closed log one `ghost_shells_closed` event with
   `{"session": session_id, "shells": [<ids>]}` (`crate::daemon::utils::log_event`,
   which stamps `ts`/`pid` itself).

### Task 5 — the leftovers and the docs

1. `ghost_host_refusal` (`mod.rs:180`): `pub(super) fn` → `fn`.
2. `refusal_gate_tests`: delete `HomeGuard` and its `impl`s; add the
   `test_home()` helper below (the exact shape `shell_exec.rs:883` uses) and
   make both callers `let (_home, _dir) = test_home();`:

   ```rust
   fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
       let guard = crate::test_home_guard();
       let dir = tempfile::tempdir().expect("tempdir");
       unsafe {
           std::env::set_var("HOME", dir.path());
       }
       crate::config::Config::ensure_dirs().ok();
       (guard, dir)
   }
   ```

   Drop the now-unused `use crate::TEST_HOME_LOCK;` in that module.
3. `shell_exec.rs:76`: the doc comment opens
   "The entry point `executor/mod.rs` calls with a pub/sub transport." —
   delete the words "The non-chat entry point:" and re-wrap the paragraph.
4. `ghost.rs:899`: "row 08b" → "row 08d".
5. `README.md`, the `ssh_target` row of the runbook-field table (`:1201`):
   replace "When set, all commands are transparently wrapped in
   `ssh <target> <cmd>` before execution." with a sentence saying that when
   the value resolves to a configured `[hosts]` entry, commands run in a
   PTY shell on that host opened with `-o BatchMode=yes` (key auth only —
   a host that asks for a password fails at once), reused for the ghost's
   later commands, capped at 300 s, and closed when the ghost ends; when it
   does not resolve, commands are wrapped in `ssh <target> <cmd>` as before.
   Keep the rest of the row.

### Task 6 — tests

Seven tests; names pinned. None contains the privilege tool's name.

In `src/host.rs` tests:

1. `ghost_ssh_argv_sets_batch_mode_before_the_destination` — for
   `entry("db01")` and `/tmp/cp`: the last element equals the destination;
   the two elements immediately before it are `"-o"`, `"BatchMode=yes"`; and
   the argv **minus** those two equals `ssh_argv(...)` for the same input.

In `src/daemon/executor/ghost_remote.rs` tests (in-process hosts: copy
`spawn_with_cast`, `shell_exec.rs:896`; hold `crate::test_home_guard()` and
point `HOME` at a temp dir as `shell_exec.rs:883` does):

2. `a_ghost_reaches_the_pty_path_only_on_a_resolved_host_with_the_pty_backend`
   — pure truth table on `ghost_remote_host`, with a policy built as
   `GhostPolicy { remote_host: Some(HostId::parse("db01").unwrap()), ..GhostPolicy::from_config(&GhostConfig::default()) }`:
   (ghost, pty, that policy) → `Some(db01)`; (ghost, **tmux**, that policy)
   → `None`; (ghost, pty, a policy with `remote_host: None`) → `None`;
   (**not** ghost, pty, that policy) → `None`; (ghost, pty, `None`) → `None`.
3. `a_ghost_remote_command_runs_in_a_ghost_owned_shell_on_that_host` — a fresh
   `ShellRegistry`, `run_ghost_remote(&reg, Owner::Ghost("g1".into()),
   HostId::Named("e2e-host".into()), "echo REMOTE_OK", "c1", <bash spawn>)`
   → text contains `REMOTE_OK`, exit 0; the registry holds **exactly one**
   record, whose `owner.key()` is `ghost:g1` and whose `host` is
   `HostId::Named("e2e-host")`.
4. `a_ghost_reuses_its_shell_for_later_commands_on_that_host` — same owner
   and host: `export DE_REUSE=kept` then `echo "v=$DE_REUSE"` → second text
   contains `v=kept`; still exactly one record.
5. `a_ghost_remote_password_prompt_fails_fast_and_says_nobody_can_answer` —
   define the fake with the **byte-for-byte** `PROMPTING` constant from
   `shell_exec.rs:740` (copy it; every `\$` matters), then run `sudo true`
   → returns in **under 5 s**, text contains `nobody is attached` and
   `cancelled`, exit code 1; then `echo AFTER` in the same shell → contains
   `AFTER`.

In `src/daemon/executor/knowledge/shell.rs` tests:

6. `closing_an_owners_shells_ends_only_that_owners_shells` — two owners
   (`Owner::Ghost("a")`, `Owner::Ghost("b")`), each with one shell registered
   via `crate::shell::spawn_for_on_host(&reg, owner, 5, HostId::Local, false, <spawn>)`
   whose in-process host runs **`sh -c 'sleep 1000'`** (not bash —
   § Current state 3), then `crate::shell::await_ready`. `close_owner_shells(&reg,
   &Owner::Ghost("a".into()), Duration::from_secs(3))` → returns exactly
   one pair, naming a's shell; polling `crate::shell::status` for up to 2 s,
   a's shell reports `ShellState::Exited` and b's does **not**.

Live, ignored — in `ghost_remote.rs` tests,
`#[ignore = "requires the configured host \`pinky\` reachable over ssh"]`:

7. `a_ghost_command_runs_on_pinky_over_a_batchmode_shell` — an in-process
   host running real `ssh` with `crate::host::ghost_ssh_argv(&HostEntry {
   ssh: "pinky".into(), ..Default::default() }, &crate::config::ssh_dir())`
   (the shape of `spawn_pinky_ssh`, `knowledge/shell.rs:1843`, with the
   ghost argv); `run_ghost_remote` with `HostId::Named("pinky")`:
   `hostname` → text contains `pinky`, exit 0; `sudo -k` → exit 0; then
   `close_owner_shells` for that owner → `status` reports `Exited` within
   2 s. `println!` each elapsed time.

**Test count:** tests 1–6 are new and run (**6**); test 7 is new and ignored.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 8 — the live check

Run test 7 by hand and paste its output into the same entry, after the
mutations:

```sh
cargo test --lib a_ghost_command_runs_on_pinky_over_a_batchmode_shell -- --ignored --nocapture 2>&1 | grep -E 'elapsed|pinky|^test |test result'
```

**If `pinky` is unreachable, report the criterion unmet** — do not describe
what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today — checked at drafting.

- [ ] `cargo test --lib` → **at least 2058 passed / 0 failed / 11 ignored**,
      on three consecutive runs (current: 2052/0/10). 2052 + 6 = 2058;
      test 7 is ignored (10 → 11). A floor; **the ignored count is exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The ghost argv exists: ``grep -c 'pub fn ghost_ssh_argv' src/host.rs``
      → **1** (current: 0).
- [ ] The ghost spawn exists and is exported:
      ``grep -c 'pub async fn spawn_ghost_ssh_host' src/shell/session.rs`` →
      **1** (current: 0), and ``grep -c 'spawn_ghost_ssh_host' src/shell/mod.rs``
      → **1** (current: 0).
- [ ] The timeout is the ghost turn budget:
      ``grep -c 'const GHOST_REMOTE_TIMEOUT_SECS: u64 = 300' src/daemon/executor/ghost_remote.rs``
      → **1** (current: the file does not exist).
- [ ] `run_background` routes through it:
      ``grep -c 'ghost_remote_host(' src/daemon/executor/foreground.rs`` →
      **1** (current: 0). *(Proxy — tests 2–5 pin the behaviour.)*
- [ ] The tmux wrap is still there for everything else (*preservation*):
      ``grep -c 'ssh_wrapped_cmd = policy.wrap_remote(cmd)' src/daemon/executor/foreground.rs``
      → **1**.
- [ ] A ghost's shells close at its end:
      ``awk '/^pub async fn trigger_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'close_owner_shells('``
      → **1** (current: 0). *(Proxy — test 6 pins the closing; the live run
      in row 09 exercises the call site.)*
- [ ] The leftovers are gone:
      ``grep -c '^fn ghost_host_refusal' src/daemon/executor/mod.rs`` → **1**
      (current: 0); ``grep -c 'struct HomeGuard' src/daemon/executor/foreground.rs``
      → **0** (current: 1);
      ``grep -c 'The non-chat entry point' src/daemon/executor/shell_exec.rs``
      → **0** (current: 1); ``grep -c 'row 08b, not this row' src/daemon/ghost.rs``
      → **0** (current: 1).
- [ ] README says what now happens:
      ``grep '^| `ssh_target`' README.md | grep -c 'BatchMode=yes'`` → **1**
      (current: 0).
- [ ] All six new non-ignored tests pass:

      ```sh
      cargo test --lib -- \
        ghost_ssh_argv_sets_batch_mode_before_the_destination \
        a_ghost_reaches_the_pty_path_only_on_a_resolved_host_with_the_pty_backend \
        a_ghost_remote_command_runs_in_a_ghost_owned_shell_on_that_host \
        a_ghost_reuses_its_shell_for_later_commands_on_that_host \
        a_ghost_remote_password_prompt_fails_fast_and_says_nobody_can_answer \
        closing_an_owners_shells_ends_only_that_owners_shells
      ```

      → **6 passed** (current: `0 passed; 0 failed; … filtered out`).
- [ ] The pinned neighbours still pass (*preservation*):
      ``cargo test --lib ssh_argv`` → **0 failed**, including
      `ssh_argv_does_not_set_batch_mode`; ``cargo test --lib refusal_gate_tests``
      → **0 failed**.
- [ ] Mutations A–C were applied, run, and their observed results recorded in
      the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.
- [ ] The Task 8 live check was run and its output pasted into the same
      entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.** These are reasoned against code this phase
writes; record what you actually observe.

- **Mutation A — never route.** Make `ghost_remote_host` return `None`
  unconditionally. Run test 2 — expected **red** on its first row.
- **Mutation B — BatchMode after the destination.** In `ghost_ssh_argv`,
  `push` the two elements instead of splicing them before the destination.
  Run test 1 — expected **red**.
- **Mutation C — close everyone's shells.** In `close_owner_shells`, drop the
  owner filter. Run test 6 — expected **red** (b's shell exits too).

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Seven tests (Task 6): the argv, the routing truth table, three real-shell
runs on the ghost path (runs on the host in a ghost-owned shell / reuses it /
a password prompt fails fast and honestly), the owner-scoped close, and the
live `pinky` run. The 08a refusal tests and the `ssh_argv` pins are the guard
that nothing before or beside the branch moved.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- ghost_ssh_argv_sets_batch_mode_before_the_destination a_ghost_reaches_the_pty_path_only_on_a_resolved_host_with_the_pty_backend a_ghost_remote_command_runs_in_a_ghost_owned_shell_on_that_host a_ghost_reuses_its_shell_for_later_commands_on_that_host a_ghost_remote_password_prompt_fails_fast_and_says_nobody_can_answer closing_an_owners_shells_ends_only_that_owners_shells 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'argv_pins=%s\n' "$(cargo test --lib ssh_argv 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'refusal_tests=%s\n' "$(cargo test --lib refusal_gate_tests 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'ghost_argv=%s\n' "$(grep -c 'pub fn ghost_ssh_argv' src/host.rs)"
  printf 'ghost_spawn=%s\n' "$(grep -c 'pub async fn spawn_ghost_ssh_host' src/shell/session.rs)"
  printf 'timeout=%s\n' "$(grep -c 'const GHOST_REMOTE_TIMEOUT_SECS: u64 = 300' src/daemon/executor/ghost_remote.rs)"
  printf 'routed=%s\n' "$(grep -c 'ghost_remote_host(' src/daemon/executor/foreground.rs)"
  printf 'tmux_wrap=%s\n' "$(grep -c 'ssh_wrapped_cmd = policy.wrap_remote(cmd)' src/daemon/executor/foreground.rs)"
  printf 'closed_at_end=%s\n' "$(awk '/^pub async fn trigger_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'close_owner_shells(')"
  printf 'leftovers=%s-%s-%s-%s\n' "$(grep -c '^fn ghost_host_refusal' src/daemon/executor/mod.rs)" "$(grep -c 'struct HomeGuard' src/daemon/executor/foreground.rs)" "$(grep -c 'The non-chat entry point' src/daemon/executor/shell_exec.rs)" "$(grep -c 'row 08b, not this row' src/daemon/ghost.rs)"
  printf 'readme=%s\n' "$(grep '^| `ssh_target`' README.md | grep -c 'BatchMode=yes')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-08c.txt
cat /tmp/e2e-08c.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=` at least
`2058p-0f-11i`, `new_tests=6p-0f`, `argv_pins=` and `refusal_tests=` with
`0f`, `ghost_argv=1`, `ghost_spawn=1`, `timeout=1`, `routed=1`,
`tmux_wrap=1`, `closed_at_end=1`, `leftovers=1-0-0-0`, `readme=1`,
`ignore_total=11`.

**Known pre-existing flake.** `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
fails intermittently under full-suite load. If it fails, report its name —
do not edit it — and re-run.

**Run this block with nothing else running `cargo test`.**

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-08c-ghost-remote-pty.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08c.txt
diff /tmp/pasted-08c.txt /tmp/e2e-08c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results and
the live check belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/host.rs` — Task 1.1 and test 1.
- `src/shell/session.rs`, `src/shell/mod.rs` — Task 1.2.
- `src/daemon/executor/ghost_remote.rs` (new), and the one `mod` line in
  `src/daemon/executor/mod.rs` — Task 2, tests 2–5 and 7.
- `src/daemon/executor/foreground.rs` — Task 3 and Task 5.2.
- `src/daemon/executor/knowledge/shell.rs`, `knowledge/mod.rs` — Task 4.1
  and test 6.
- `src/daemon/ghost.rs` — Task 4.2 and Task 5.4.
- `src/daemon/executor/mod.rs` — Task 5.1 (and the `mod` line).
- `src/daemon/executor/shell_exec.rs` — Task 5.3 only (a doc comment).
- `README.md` — Task 5.5 only.
- This phase doc's Update Log.

## Out of scope

- **`spawn_ghost_shell { host? }`** and the ghost's dropped `host` argument
  on `run_terminal_command` (`ghost.rs:893-910`) — row 08d.
- **Connection-time ssh prompts** — row 08e. BatchMode makes them fail fast
  here instead.
- **The approval request's `host`.** `run_background`'s approval still sends
  `host: None`; a ghost's approval goes through its policy, and the panel
  classes are chat's (06a/06b).
- **`wait: false` / background notification** for a ghost's remote command —
  PE chose synchronous with a 300 s cap.
- **A ghost command that itself starts `ssh …`** — on the PTY path it is an
  interactive command and handled as one; not specially routed.
- **The backend redaction-marker flake** — recorded, not fixed here.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-28 13:15 (progress)

Started 08c. Read STANDARDS, the phase doc and the M21 README; repo confirmed
clean at `ef6b6e86` (08f approved at `b837bef4`). Baseline on this tree:
`cargo test --lib` → 2052 passed / 0 failed / 10 ignored, matching the doc.
Status flipped to `in-progress` here and in the README row. Beginning Task 1
(the BatchMode argv and the ghost spawn).

### Update — 2026-09-28 13:50 (progress)

Tasks 1–6 implemented. The BatchMode argv (`ghost_ssh_argv`) splices
`-o BatchMode=yes` before the destination; `spawn_ghost_ssh_host` resolves the
host **strictly** (a ghost's `remote_host` is always a configured host) and is
exported from `shell/mod.rs`. The new `daemon/executor/ghost_remote.rs` holds
the pure routing truth table (`ghost_remote_host`) and the synchronous run
(`run_ghost_remote`, 300 s cap, its own sink/EOF transport so a ghost never
answers a prompt); `run_background` branches onto it after the remote-script
build and before the tmux wrap, so both 08a gates, `resolve_command`, the
approval and the script build are unchanged, and an unresolved `ssh_target`,
a local ghost and `backend = "tmux"` still take the 1.x wrap. `close_owner_shells`
(kind `knowledge/shell.rs`, re-exported `pub(crate)` from `knowledge/mod.rs`)
closes only the owner's records; `trigger_ghost_turn` calls it after the
container teardown and logs one `ghost_shells_closed` event when any closed.
The leftovers are done: `ghost_host_refusal` is private, `refusal_gate_tests`
uses `test_home()` over the leaking `HomeGuard`, the `run_in_shell_with_prompts`
doc comment opens correctly, and `ghost.rs` says row 08d. README's `ssh_target`
row describes the PTY path. Test 6 needed two fixes while writing: the status
poll matches `ShellResponse::Status { state: Exited }` (the wire's Exited
frame arrives only from a finished run) and `await_ready` takes a
`std::time::Instant` deadline. `cargo test --lib` → 2058 passed / 0 failed /
11 ignored. One environment note: the first run of tests 3–5 failed with
`No space left on device` — a 33 GB `/tmp/fakeout.txt` from an unrelated
shell process of 2026-09-27 was filling `/tmp`; killed the process and
removed the file, after which all three passed.

### Update — 2026-09-28 13:55 (end-to-end verification)

The § End-to-end verification block, run verbatim as its own capture:

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2058p-0f-11i
new_tests=6p-0f
argv_pins=5p-0f
refusal_tests=2p-0f
ghost_argv=1
ghost_spawn=1
timeout=1
routed=1
tmux_wrap=1
closed_at_end=1
leftovers=1-0-0-0
readme=1
ignore_total=11
```

PASTE MATCH

**Mutations** (each applied with the patch tool, run once, restored;
`git diff --stat` confirmed back to its pre-mutation state after each):

**Mutation A — never route** (`ghost_remote_host` returned `None`
unconditionally, ran test 2):

```
assertion `left == right` failed: a ghost with the PTY backend and a resolved
host takes the branch
  left: None
 right: Some(Named("db01"))
test result: FAILED. 0 passed; 1 failed
```

Red on its first row, as reasoned. Restored.

**Mutation B — BatchMode after the destination** (`push` instead of
`splice`, ran test 1):

```
assertion `left == right` failed: the destination is still the last element:
["-tt", "-o", "ControlMaster=auto", "-o", "ControlPath=/tmp/cp/%C", "-o",
"ControlPersist=10m", "db01", "-o", "BatchMode=yes"]
  left: Some("BatchMode=yes")
 right: Some("db01")
test result: FAILED. 0 passed; 1 failed
```

Red, as reasoned: the `-o` pair after the destination would become the
remote command. Restored.

**Mutation C — close everyone's shells** (dropped the owner filter in
`close_owner_shells`, ran test 6):

```
assertion failed: b's shell must not be touched by a's teardown
test result: FAILED. 0 passed; 1 failed
```

Red exactly on the owner scoping the phase pins. Restored.

**Task 8 live check** (`pinky` reachable over ssh, BatchMode):

```
pinky `hostname`: 285.592748ms (exit 0)
pinky `sudo -k`: 20.048249ms (exit 0)
pinky close: 1.503587002s ([("s0", "Sent TERM to shell s0, but its host is still holding its lock — the shell may still be finishing (there is no kill frame on this wire, so escalation is not possible). list_shells still shows it; check again before assuming it is closed.")])
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2068 filtered out; finished in 1.81s
```

`hostname` answered `pinky` in 286 ms and `sudo -k` in 20 ms on the
ghost-owned BatchMode shell, and the closed shell reported `Exited` within
2 s. The close sentence says the lock is still held — § Current state 3
names that: an in-process host never releases its lock, so the assert is on
`crate::shell::status` (which read `Exited`), not on the sentence.
