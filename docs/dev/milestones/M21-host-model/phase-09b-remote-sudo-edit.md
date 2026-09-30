# Phase 09b: remote-sudo-edit — `edit_file { host, sudo: true }` edits a root-owned file on a host, answering sudo's prompt through the chat

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-09a (an edit keeps the file's mode and owner), phase-08f
(PTY password prompts answered once, on the real prompt)
**Estimated diff:** ~180 lines of production code across ~12 files, plus 10 tests and 1 ignored live test
**Tags:** language=rust, kind=feature, size=m

## Goal

`edit_file` gains an optional boolean `sudo`. With `sudo: true`, an
`operation: "edit"` on a named host runs its edit **as root** on that host.
The line typed into the host's shell starts sudo; sudo's password prompt is
classified and answered through the chat's `CredentialPrompt` (the same
round trip `run_terminal_command` uses on the PTY path); and the mode and
owner the file had are kept (09a).

Three rules come with it:

- **Scope:** `sudo` is accepted only for `operation: "edit"` on a named host.
  Anything else is refused before any prompt is shown.
- **Approval:** a sudo edit is **never auto-approved**. Neither the
  session-wide file-edit approval nor a per-path session approval covers it,
  and approving one **never grants** a session approval. The panel says it
  runs as root.
- **Failure is loud:** if the edit did not run (password refused, nobody
  attached), the result says so as an `Error:`. It never says "result was
  unclear".

This row is the prerequisite for the milestone's exit criterion (row 09c).

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — remote file ops through the host's
  shell; § 3 — credential prompts surface through the approval client and
  never reach the shell log.
- `docs/dev/milestones/M21-host-model/README.md` § Notes → "Measured at 09
  drafting (2026-09-29) — for 09b".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

**Shell-gate note: read this before running anything.** Your shell refuses
any command you type that contains the literal word for the privilege tool
(the one this phase is about). That is why every test in this phase is named
`…root_edit…`, and why the criteria spell the word `su[d]o` inside grep
patterns. **Source code may contain the word.** The field is named `sudo`,
and you write it with the `patch` tool, which is not gated. Only commands
you type are gated. Filter test runs with `root_edit` or `root_flag`, never
with the word itself.

## Current state

Measured on **2026-09-30** at commit `30d8dcaa`.

Baseline: `cargo test --lib` → **2078 passed / 0 failed / 13 ignored**.

### 1. No sudo path exists

Every remote write runs through `run_in_host_shell`
(`src/daemon/executor/file_ops/mod.rs`), which calls the plain `run_command`.
That path never prompts, so a root-owned file cannot be edited today.

### 2. The line must start sudo; `command_has_sudo` decides that

`run_in_shell_with_prompts` (`src/daemon/executor/shell_exec.rs:87`) turns on
the password-prompt handling only when `command_has_sudo(cmd)` is true. That
function's regex is `(?:^|[;&|])\s*sudo\b`. The existing edit command is
`if command -v python3 …; then python3 …; else perl …; fi; echo '__DE_DONE__'`,
so putting the word inside the `then` / `else` branches is **not** detected
(measured: `false`). The measured shape that **is** detected (`true`) picks
the interpreter with `set --` and runs sudo after a `;`:

```sh
if command -v python3 >/dev/null 2>&1; then set -- python3 -c "exec(bytes.fromhex('<PY_HEX>').decode())"; else set -- perl -e 'eval(pack("H*","<PL_HEX>"))'; fi; sudo "$@" 2>&1; echo '__DE_DONE__'
```

`<PY_HEX>` and `<PL_HEX>` are **the same** hex payloads `build_remote_edit_cmd`
(`src/daemon/executor/file_ops/write.rs`) already builds, including 09a's mode
and owner handling. They must be shared, not copied.

### 3. Measured through the daemon's own run path

All three runs went through `run_in_shell_with_prompts`, the line above, and a
shell from `spawn_in_process_host`:

| Case | Result |
|---|---|
| Local shell, a fake `sudo` shell function that reads a password from `$SUDO_PROMPT` and then runs `"$@"`; transport answers with the password | **one** `CredentialPrompt`; `Ok((0, "[de-sudo-prompt-0]\nDE_OK: Edited …\n__DE_DONE__"))`; the file edited, its `0640` mode kept |
| Same, **nobody attached** (`sink()` + `empty()`) | `Err("the password prompt in shell s0 was cancelled: a password prompt appeared and nobody is attached to answer it — …")` in 305 ms; the file **unedited** |
| Same, but the fake accepts only the right password and the transport answers `wrong` | `Ok((0, "[de-sudo-prompt-1]\n__DE_DONE__"))` — **no** `DE_OK:` / `DE_ERROR:` line; the file unedited |
| **pinky** (real sudo-rs 0.2.13, real ssh), nobody attached, a `0640` file | the same "nobody is attached" `Err` in **323 ms**; the file unedited; the next command in that shell ran in 20 ms |

Note row 3: a refused password leaves **no sentinel line** at all. Today's
remote-edit scan turns that into "Edit command completed but result was
unclear". For a sudo edit that must become an `Error:` (Task 3).

### 4. The run path's deadline includes the user's typing time

`read_to_marker` (`src/shell/client.rs`) fixes its deadline when the command
starts, and the credential round trip runs inside it. The client's own
credential wait is `USER_PROMPT_TIMEOUT` = 120 s
(`src/daemon/executor/mod.rs:78`). With the 30 s `REMOTE_WRITE_TIMEOUT_SECS`,
a user who takes more than about 30 s to type the password would time the
edit out. A sudo edit gets its own constant, `SUDO_EDIT_TIMEOUT_SECS: u64 = 150`.

### 5. The client auto-approves file edits (`src/cli/commands/stream.rs`, `prompt_edit_file_ratatui`)

```rust
    let all_approved = approval.file_edits_all;
    let path_approved = approval.file_edits.contains(path);

    if all_approved || path_approved {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return (true, None);
    }

    let prompt_text = build_approval_prompt("session", true);
    let (approved, is_session, user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

    if approved {
        if is_session && !all_approved {
            approval.file_edits.insert(path.to_string());
```

The prompt carries no sudo signal, so a session approval of file edits would
cover a sudo edit too.

### 6. Every site a new field touches

`edit_file`'s arguments flow through 11 sites. Add the field to each; the
tree does not compile until the last one is done, so do them in one pass:

| File | Site |
|---|---|
| `src/ai/tools/args.rs` | `EditFileArgs` (`:157`) and its `to_event` (`:587`) |
| `src/ai/types/events.rs` | `AiEvent::EditFile` (`:75`) |
| `src/ai/types/pending.rs` | `PendingCall::EditFile` (`:84`), `to_tool_call` (`:556`), two test literals (`:1347`, `:1634`) |
| `src/daemon/stream.rs` | `AiEvent::EditFile` → `PendingCall::EditFile` (`:481`) |
| `src/daemon/ghost.rs` | the same (`:1123`) |
| `src/daemon/executor/mod.rs` | the `PendingCall::EditFile` arm → `file_ops::EditArgs` (`:1019`, `:1052`) |
| `src/daemon/executor/file_ops/write.rs` | `EditArgs` (`:4`), and `RunEditArgs` built at `:118` |

`RunEditArgs` (`ops.rs:42`) is also built in tests at `ops.rs:1161, 1201,
1613, 1898`, and `EditArgs` at `ops.rs:1699`. `Response::EditFilePrompt` is
built at **nine** sites in `ops.rs` (`:100, 204, 373, 448, 549, 638, 746,
852, 994`), declared in `src/ipc.rs`, and matched in
`src/cli/commands/stream.rs:644` and `src/cli/commands/ask.rs:184` (the
`ask.rs` match uses `..`, so it needs no change).

The idiom for an optional boolean argument is `Option<bool>`, as with
`run_terminal_command`'s `wait` (`args.rs:18`, passed through as
`wait: self.wait`). Use `sudo: Option<bool>` on `EditFileArgs`,
`AiEvent::EditFile` and `PendingCall::EditFile`, and a plain `bool` from
`EditArgs` inward (`sudo.unwrap_or(false)` at the executor arm).

## Spec

### Task 1 — the flag on the tool surface

1. `src/ai/tools/defs.rs`, the `edit_file` `ToolDef`: add a `ParamDef` after
   `host`:

   ```rust
            ParamDef {
                name: "sudo",
                ty: ParamTy::Bool,
                required: false,
                description: "Optional. true runs an operation=\"edit\" as root on the \
                              configured host named by host, answering the password prompt \
                              through the user. Only for operation=\"edit\" with a host; \
                              always asks for approval, never approved for the session.",
            },
   ```

2. Thread `sudo` through every site in § Current state 6. `to_tool_call`'s
   JSON gains `"sudo": sudo`. The `stream.rs` / `ghost.rs` arms pass it
   through unchanged.
3. `src/daemon/executor/mod.rs`, the `EditFile` arm: pass
   `sudo: sudo.unwrap_or(false)` into `file_ops::EditArgs`.

### Task 2 — the refusals (`src/daemon/executor/file_ops/write.rs`, `run_edit_file`)

After the existing ghost refusal and host resolution, when `sudo` is true:

- if `operation` is anything but `"edit"` (the default), return
  `Ok(ToolCallOutcome::Result("Error: sudo is supported for operation=\"edit\" only.".into()))`;
- if the host resolved to `HostId::Local`, return
  `Ok(ToolCallOutcome::Result("Error: a sudo edit runs on a configured host — name it with host. A sudo edit on the daemon host is not supported.".into()))`.

Both refusals come **before** any `EditFilePrompt`. Pass `sudo` on into
`RunEditArgs`.

### Task 3 — the sudo edit (`src/daemon/executor/file_ops/write.rs` + `ops.rs`)

1. **Share the payloads.** Factor `build_remote_edit_cmd`'s two payload
   strings into
   `pub(super) fn remote_edit_payload_hex(path: &str, old_string: &str, new_string: &str) -> (String, String)`,
   which returns `(py_hex, pl_hex)`. `build_remote_edit_cmd` calls it, and its
   output stays **byte-identical**: `the_write_sentinels_are_unchanged` and
   09a's tests must stay green unchanged.
2. **The sudo line.** Add
   `pub(super) fn build_remote_sudo_edit_cmd(path: &str, old_string: &str, new_string: &str) -> String`,
   returning exactly the § Current state 2 line with the two hex strings from
   (1) substituted. In a Rust `format!` string that is:

   ```rust
    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            set -- python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\"; \
         else \
            set -- perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))'; \
         fi; sudo \"$@\" 2>&1; echo '__DE_DONE__'"
    )
   ```

3. **The run.** In `run_edit_with_spawn`'s `HostId::Named(name)` branch:
   - The `EditFilePrompt` path becomes
     `format!("{} (remote via host {}, as root via sudo)", path, name)` when
     `sudo`, and sets the new `sudo` field (Task 4).
   - After approval, when `sudo`: run `build_remote_sudo_edit_cmd` through
     `super::super::shell_exec::run_in_shell_with_prompts(crate::shell::registry(), Owner::Chat(owner_key()), &cmd, max_per_owner, None, Some(SUDO_EDIT_TIMEOUT_SECS), id, tx, rx, host.clone(), spawn)`.
     Take `max_per_owner` the way `run_in_host_shell` does:
     `crate::config::Config::load().unwrap_or_default().shells.max_per_owner`.
     Its `Ok((_, result))` gives `result.text`; `Err(e)` gives
     `Ok(ToolCallOutcome::Result(format!("Error: {e}")))` after
     `finish_command(cmd_id, 1)`.
   - When not `sudo`, the existing `run_in_host_shell` call is unchanged.
   - Scan the text exactly as today (`DE_OK:` → success, `DE_ERROR:` →
     error). **When `sudo` and neither line is present**, return
     `format!("Error: the sudo edit of {path} on host {name} did not run — the password was not accepted or sudo refused it. Output: {tail}")`,
     where `tail` is the last non-empty line of the text, and do
     `finish_command(cmd_id, 1)`. The non-sudo "result was unclear" sentence
     stays as it is.
   - Add `const SUDO_EDIT_TIMEOUT_SECS: u64 = 150;` beside
     `REMOTE_WRITE_TIMEOUT_SECS`, with a one-line comment: the run's deadline
     includes the user typing the password, and the client waits up to 120 s
     for it.

### Task 4 — the prompt carries the flag; the client never auto-approves it

1. `src/ipc.rs`, `Response::EditFilePrompt`: add
   `#[serde(default)] sudo: bool` after `dest_path`. Set `sudo: false` at
   every other construction site (§ Current state 6), and `sudo` in Task 3's
   edit branch.
2. `src/cli/commands/stream.rs`: destructure `sudo` at `:644` and pass it to
   `prompt_edit_file_ratatui` as a new last parameter `sudo: bool`. Add two
   pure helpers beside it, and use them in place of the inline conditions
   quoted in § Current state 5:

   ```rust
   /// A sudo edit is never auto-approved: neither the session-wide file-edit
   /// approval nor a per-path one covers an edit that runs as root.
   fn edit_file_auto_approved(approval: &super::approval::SessionApproval, path: &str, sudo: bool) -> bool {
       !sudo && (approval.file_edits_all || approval.file_edits.contains(path))
   }

   /// `[A]pprove for session` on a sudo edit approves this edit only.
   fn edit_file_grants_session(is_session: bool, all_approved: bool, sudo: bool) -> bool {
       is_session && !all_approved && !sudo
   }
   ```

   When `sudo`, also commit a line `"  ⚠ runs as root (sudo) on the host\n"`
   right after the "AI wants to …" line. When a sudo edit is approved with
   `[A]`, commit `"  ✓ approved — sudo edits are never approved for the session\n"`
   in place of the path-session message.

### Task 5 — docs

1. `assets/prompts/sre.toml`, the `edit_file(path, operation, …, host?)`
   bullet: change the signature to `host?, sudo?`, and add a sentence:
   `sudo: true` edits a root-owned file on a configured host
   (`operation="edit"` only); the user types the password in the chat, and a
   sudo edit is always asked for, never approved for the session.
2. `CLAUDE.md`, the `edit_file` row of the Current AI tools table: add the
   same fact in one clause after the `host?` clause.
3. `README.md`, the `edit_file` row of the AI tools table (`:506`): add
   ``; `sudo: true` edits a root-owned file on that host (`edit` only), with
   the password asked in the chat and no session approval``.

### Task 6 — tests

There are ten tests, with the names pinned. **Every test that opens a
shell with `spawn_in_process_host` must close it at the end**, the way
09a's `a_remote_edit_keeps_the_files_mode` does:

```rust
        let owner = Owner::Chat(owner_key());
        if let Some(id) = crate::shell::registry_lookup(&owner, |id| id.to_string()) {
            let _ = crate::daemon::executor::knowledge::close_shell_with(
                crate::shell::registry(),
                &id,
                &owner,
                std::time::Duration::from_millis(3000),
            )
            .await;
        }
```

A leaked shell there made unrelated PTY tests fail at random in 09a (5 red
runs out of 17).

**The fake privilege tool.** `ops.rs` tests seed the host's shell with a
shell function that prompts with `$SUDO_PROMPT`, accepts only `pw-xyz`, and
then runs its arguments. The name is assembled at run time, the way
`shell_exec.rs`'s `PROMPTING` fixture does it:

```rust
    const FAKE_ROOT: &str = r#"f=su; f+=do; eval "$f() { local pw=; read -rs -p \"\$SUDO_PROMPT\" pw || return 1; echo; [ \"\$pw\" = pw-xyz ] || return 1; \"\$@\"; }""#;
```

Define it in the shell the edit will reuse **before** the edit runs:

```rust
        run_in_host_shell(
            crate::shell::registry(),
            Owner::Chat(owner_key()),
            crate::host::HostId::Named("web01".to_string()),
            FAKE_ROOT,
            REMOTE_WRITE_TIMEOUT_SECS,
            spawn_in_process_host,
        )
        .await
        .expect("seed the fake");
```

Then call `run_edit_with_spawn` with
`Box::new(|_id| Box::pin(async { Ok(()) }))`. The idle shell is reused, so the
spawn is never called.

**The transport.** Add `approve_and_answer_channel(credential: Option<&'static str>)`
beside `auto_approve_channel`, with the same two-duplex pump. On an
`EditFilePrompt` it approves and records `format!("edit:{path}:sudo={sudo}")`.
On a `Response::CredentialPrompt { id, .. }` it records `"cred"`, and then:
- `Some(c)`: writes `Request::CredentialResponse { id, credential: c.to_string() }`;
- `None`: breaks out of the loop, which drops the pump's write half. The
  daemon reads EOF, which is the "nobody is attached" case.

In `src/daemon/executor/file_ops/ops.rs` tests (each holds
`remote_write_test_home()`; the file under test is a `0640` temp file
holding `a\nold-line\nb\n`):

1. `a_root_edit_on_a_host_answers_the_password_prompt`: seed `FAKE_ROOT`,
   `approve_and_answer_channel(Some("pw-xyz"))`, `run_edit_with_spawn` with
   `sudo: true` on `web01`. Assert:
   - the result contains `Edited … on host web01.`;
   - the file holds `new-line`, and its mode is still `0o640`;
   - the events are exactly one `edit:…:sudo=true` whose path contains
     `as root via sudo`, then exactly one `cred`.
2. `an_unanswered_root_edit_leaves_the_file_unedited`: the same with
   `approve_and_answer_channel(None)`. Assert the result starts `Error:` and
   contains `nobody is attached`, and the file still holds `old-line`.
3. `a_wrong_password_root_edit_leaves_the_file_unedited`: the same with
   `Some("wrong")`. Assert the result starts `Error:` and contains
   `did not run`, the file still holds `old-line`, and the result does **not**
   contain `unclear`.
4. `a_root_edit_needs_a_host_and_the_edit_operation`: through
   `super::super::run_edit_file`, shaped like
   `a_malformed_host_is_refused_by_edit_file` (the `EditArgs` literal at
   `ops.rs:1699`), with `sudo: true`:
   - `operation: "edit"` with `host: None` → contains
     `runs on a configured host`;
   - `operation: "create"` with `host: Some("web01")` → contains
     `operation="edit" only`.

   Use a `Vec<u8>` as the writer and assert it is still **empty** afterwards:
   no prompt came before the refusal.
5. `the_root_edit_line_is_detected_and_reuses_the_payloads`: pure. For
   `build_remote_sudo_edit_cmd("/tmp/x", "a", "b")`:
   - `crate::daemon::utils::command_has_sudo(&cmd)` is `true`;
   - it contains `set -- python3 -c` and `set -- perl -e`;
   - the hex strings after `fromhex('` and `pack("H*","` are **equal** to
     those in `build_remote_edit_cmd("/tmp/x", "a", "b")`. Reuse the
     `decode_hex_from` closure from `the_write_sentinels_are_unchanged`, or
     compare the raw hex.
6. `the_root_edit_line_runs_under_both_interpreters`: pure, in the style of
   09a's `the_python_branch_keeps_the_files_mode`. For each interpreter, make
   a `bin` temp dir holding the interpreter's symlink **plus** an executable
   file named `sudo` whose body is `#!/bin/sh\nexec "$@"\n`
   (`std::fs::write`, then `set_permissions` `0o755`). Run the built line with
   bash's absolute path and `.env("PATH", bin)`. Assert `DE_OK:`, the edit
   landed, and a `0640` file kept its mode.

In `src/cli/commands/stream.rs` tests:

7. `a_root_edit_is_never_auto_approved`: a `SessionApproval::default()` with
   `file_edits_all = true` and `file_edits` containing `"/etc/x"`:
   `edit_file_auto_approved(&a, "/etc/x", true)` → `false`, and with
   `false` → `true`.
8. `a_root_edit_never_grants_a_session_approval`:
   `edit_file_grants_session(true, false, true)` → `false`;
   `edit_file_grants_session(true, false, false)` → `true`.

In `src/ipc_tests.rs`:

9. `edit_file_prompt_root_flag_defaults_to_false`: deserialize an
   `EditFilePrompt` JSON line **without** a `sudo` key. It parses, with
   `sudo == false`. Also, a round trip with `sudo: true` keeps it.

In `src/ai/tools/dispatch.rs` tests:

10. `edit_file_takes_a_root_flag`:
    - `TOOLS`' `edit_file` has a param named `sudo` with `ParamTy::Bool`,
      not required;
    - dispatching `edit_file` with
      `{"path":"/tmp/f","host":"pinky","sudo":true}` (the way
      `dispatch_roundtrip_all_tools` dispatches) yields
      `AiEvent::EditFile { sudo: Some(true), .. }`;
    - without the key it yields `sudo: None`.

Live, ignored (`#[ignore = "requires the configured host \`pinky\` reachable over ssh"]`),
in `ops.rs` tests:

11. `a_root_edit_on_pinky_with_nobody_attached_fails_cleanly`: use
    `edit_file_on_pinky_end_to_end`'s setup.
    - Create `/tmp/de-09b-probe` on pinky with `run_in_host_shell`
      (`printf 'a\nold-line\nb\n' > /tmp/de-09b-probe && chmod 640 /tmp/de-09b-probe`).
    - Run `run_edit_with_spawn` with `sudo: true` on
      `HostId::Named("pinky")`, `approve_and_answer_channel(None)`, and the
      `spawn_pinky_ssh` spawn.
    - `println!` the result and the elapsed time.
    - Assert the result contains `nobody is attached`, and the file (read back
      with `run_in_host_shell`) still holds `old-line`.
    - `rm -f` the probe, close the shell, `ssh -O exit`.

    The **answered** path on pinky needs the real password, so it is
    row 09c's live close, not this test.

**Test count:** ten new and run, one new and ignored, so **2088 passed /
14 ignored**.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output into a
new Update Log entry titled exactly
`### Update — <date> (end-to-end verification)` **as that entry's first
fenced block**. **Write this entry in the phase doc.** A summary in your
final message is not the entry: 09a bounced for exactly that. Then run the
paste-check snippet and put its literal `PASTE MATCH` line in the entry.

### Task 8 — the live check

Run test 11 by hand and paste its output into the same entry, after the
mutations:

```sh
cargo test --lib a_root_edit_on_pinky_with_nobody_attached_fails_cleanly -- --ignored --nocapture 2>&1 | grep -E 'nobody|elapsed|^test |test result'
```

**If `pinky` is unreachable, report the criterion unmet.** Do not describe
what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was checked at drafting.

- [ ] `cargo test --lib` → **2088 passed / 0 failed / 14 ignored**, on three
      consecutive runs (current: 2078/0/13). **Both counts are exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The ten new non-ignored tests pass:
      ``cargo test --lib -- root_edit root_flag`` → **10 passed**
      (current: `0 passed; … filtered out`).
- [ ] Neighbours still pass (*preservation*):
      ``cargo test --lib the_write_sentinels_are_unchanged`` → **1 passed**;
      ``cargo test --lib keeps_the_files_mode`` → **4 passed**;
      ``cargo test --lib edit_on_a_named_host_runs_through_the_shell`` →
      **1 passed**; ``cargo test --lib file_ops`` → **0 failed**.
- [ ] The docs name the flag: ``grep -c 'host?, su[d]o?' assets/prompts/sre.toml``
      → **1** (current: 0).
- [ ] Mutations A–C were applied, run, and their observed results recorded in
      the end-to-end entry.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the snippet printed
      the literal `PASTE MATCH` into that entry:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>``
      → **1** (current: 0).
- [ ] The Task 8 live check was run and its output pasted into the same
      entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore it, and report what you saw.
**Run budget: one run each.** Record what you actually observe, including
the assertion's `left:` / `right:` values.

- **Mutation A:** make `edit_file_auto_approved` ignore `sudo` (return
  `approval.file_edits_all || approval.file_edits.contains(path)`). Run test 7.
  Expected **red**.
- **Mutation B:** in the sudo branch, drop the "did not run" error so a
  missing sentinel falls through to the "result was unclear" sentence. Run
  test 3. Expected **red**.
- **Mutation C:** build the sudo line with the word inside the `then` branch
  (`then sudo python3 -c …`, with no `set --`). Run test 5. Expected **red**,
  because `command_has_sudo` is false.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

There are eleven tests (Task 6):
- the answered, unanswered and wrong-password paths through the real run
  path, with a fake privilege tool;
- the refusals;
- the line's detection and the payload sharing;
- both interpreter branches;
- the client's two approval rules;
- the IPC default;
- the tool surface;
- a live pinky run with nobody attached.

The answered path against real sudo-rs is 09c's live close.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- root_edit root_flag 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'neighbours=%s/%s/%s/%s\n' "$(cargo test --lib the_write_sentinels_are_unchanged 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')" "$(cargo test --lib keeps_the_files_mode 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')" "$(cargo test --lib edit_on_a_named_host_runs_through_the_shell 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')" "$(cargo test --lib file_ops 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ failed')"
  printf 'docs=%s\n' "$(grep -c 'host?, su[d]o?' assets/prompts/sre.toml)"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-09b.txt
cat /tmp/e2e-09b.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2088p-0f-14i`,
`new_tests=10p-0f`, `neighbours=1 passed/4 passed/1 passed/0 failed`,
`docs=1`, `ignore_total=14`.

**Known pre-existing flake.** `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
fails intermittently under full-suite load. If it fails, name it and re-run
the **whole block**. **Re-run budget: three.** After three re-runs, stop,
paste the last capture as it is, and name every failing test. Do **not**
loop on `cargo test`. A failing test **other** than that one is a real
finding: report it by name, and do not re-run it away. If one of **your**
tests leaves a shell open, other PTY tests fail at random; close every shell
you open.

**Run this block with nothing else running `cargo test`.**

Paste the file's contents **into this phase doc** as the first fenced block of
an entry titled `### Update — <date> (end-to-end verification)`. **After** the
entry is written, run:

```sh
D=docs/dev/milestones/M21-host-model/phase-09b-remote-sudo-edit.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-09b.txt
diff /tmp/pasted-09b.txt /tmp/e2e-09b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. The mutation results
and the live check go in the same entry, **after** that first block.

## Authorizations

You may edit these files, and nothing else:

- `src/ai/tools/defs.rs`, `src/ai/tools/args.rs`, `src/ai/tools/dispatch.rs`
  (test 10 only), `src/ai/types/events.rs`, `src/ai/types/pending.rs`,
  `src/daemon/stream.rs`, `src/daemon/ghost.rs`: Task 1.
- `src/daemon/executor/mod.rs`: Task 1.3.
- `src/daemon/executor/file_ops/write.rs`, `src/daemon/executor/file_ops/ops.rs`:
  Tasks 2, 3 and 6.
- `src/ipc.rs`, `src/ipc_tests.rs`, `src/cli/commands/stream.rs`: Tasks 4
  and 6.
- `assets/prompts/sre.toml`, `CLAUDE.md` (the `edit_file` row only),
  `README.md` (the `edit_file` row only): Task 5.
- This phase doc's Update Log.

## Out of scope

- **A sudo edit on the daemon host**, and sudo for `create` / `delete` /
  `copy`: all refused (Task 2).
- **A session-approval class for sudo edits.** They always prompt.
- **`run_terminal_command`'s deadline including typing time.** Recorded in
  § Current state 4; only the sudo edit gets the longer timeout here.
- **The answered path against real sudo on pinky**: row 09c, the live close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-30 01:50 (start)

**Executor:** claude (sonnet-4.5, headless). Status flipped `todo` →
`in-progress`; README phase-table row updated to match. Beginning Task 1.
