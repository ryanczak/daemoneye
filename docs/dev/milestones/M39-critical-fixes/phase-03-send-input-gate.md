# Phase 03 — send-input-gate

**Milestone:** M39 — Critical fixes
**Status:** done
**Drafted:** 2026-11-22
**Depends on:** phase-02 (`call_owner`, already in the tree)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`send_input` asks before it types what it should ask about. A shell id that
cannot be a shell is refused before anything touches a socket. Input into a
shell on a configured remote host is approved as remote input on that host.
Input that types `sudo` is approved as sudo, however the call spells it.

## Statement of work

Three defects, all in the `send_input` arm
(`src/daemon/executor/dispatch_send_input.rs`):

1. **An unparseable id goes on (S2c).** When `ShellId::parse(shell)` fails,
   the arm treats the shell as `Foreground::Unknown` and carries on. Step 4
   then reads the screen from `crate::shell::socket_path(shell)` using the
   **raw** string. That happens before the ownership check and before the
   approval prompt. `"../../x"` connects to `shells_dir/../../x.sock`, and
   the answer goes into the panel. The final send is safe, because
   `send_input_with` refuses an id the registry does not hold. Everything
   before it is not.
2. **The remote host is dropped (S2b).** The arm builds
   `ApprovalRequest { host: None, … }`. The prompt is therefore classified
   `Classification::Command`, a local command. Under the shipped
   `[approvals]` defaults the client auto-approves a non-sudo `Command`, so
   typing into a shell on `[hosts.pinky]` runs with **no prompt at all**.
   The shell's `ShellRecord.host` (a `HostId`) knows the truth.
3. **Sudo is checked on the wrong string (S2b).** The client decides "is
   this sudo" with `command_has_sudo(command)`, where `command` is the
   approval panel from `knowledge::input_panel`. That panel shows the text
   Debug-quoted (`{text:?}`) and the keys as a list. Two spellings get past
   it:
   - `text: "echo hi\nsudo reboot"` becomes `"echo hi\nsudo reboot"`, with
     a **backslash-n**. `command_has_sudo` reads `hi\nsudo` as one word.
   - `keys: ["s","u","d","o","Space","r","e","b","o","o","t","Enter"]`.
     A bare printable character is a valid key (`shell/keys.rs`,
     `parse_key` → `single_char`). The panel shows `keys [s, u, d, o, …]`.

**PE decision (2026-11-22): per-host grant.** Remote `send_input` is
approved exactly like `run_terminal_command` on that host. It is classified
`RemoteShell { host }`, it is asked by default, and the user's session grant
for that host covers it. The client's existing `RemoteShell` grant logic
applies unchanged. No new classification and no new client rule are needed
for the remote case.

**What "fixed" means for each defect:**

- **S2c.** An id `ShellId::parse` rejects is refused before the probe, the
  screen read and the prompt. An id that parses but is not in
  `crate::shell::registry()` is refused before the prompt, because its host
  cannot be resolved. Both refusals name the id and say nothing was sent.
  This fails closed (M39 decision 4).
- **S2b, host.** The prompt carries the record's host: `HostId::Named(n)` →
  `Some(n)`, `HostId::Local` → `None`. So the classification is
  `RemoteShell { host: n }` for a configured host.
- **S2b, sudo.** The sudo decision for `send_input` is made on the text as
  the shell will receive it, not on the panel. That text is `text` with its
  real newlines, followed by what the keys type: each single printable
  character key as itself, `Space` as a space, and `Enter` as a newline.
  The client must treat a `send_input` prompt as sudo when that text
  contains `sudo` at a command position on any line. Deciding how the
  daemon tells the client is part of the work. Hint: `Response::ToolCallPrompt`
  is the only daemon → client channel, and its `classification` and
  `interactive` fields show the pattern for adding a field with
  `#[serde(default)]`. The client reads it in
  `src/cli/commands/stream_prompts.rs`, where `command_has_sudo(command)`
  feeds `approval.auto_approves`.

**Scoping hints.** Start here, and read further only when the work cannot
be finished from these. Record in the Update Log what you read beyond them
and why.

- `src/daemon/executor/dispatch_send_input.rs`: the whole arm, all three
  defects. Your first test goes at the bottom (see **First red test**).
- `src/daemon/executor/mod.rs`: `ApprovalRequest`, `tool_call_prompt_frame`,
  `prompt_classification`, `prompt_and_await_approval`.
- `src/ipc.rs`: `Response::ToolCallPrompt`, `Classification`, `classify`.
- `src/cli/commands/stream_prompts.rs`: the client's approval decision (the
  `command_has_sudo(command)` call).
- `src/daemon/utils/sudo.rs`: `command_has_sudo`. Use it as it is; do not
  change it.
- `src/daemon/executor/knowledge/shell_input.rs`: `ComposedInput`,
  `input_panel`, `send_input_with`.
- `src/shell/keys.rs`: `parse_key`, for which key names type a character.
- Not needed: `src/shell/registry.rs` beyond `ShellRecord.host`, the sbx
  modules, the AI backends, the `knowledge/*_tests.rs` files, and every
  other executor arm.

**Budget.** You have **1200 turns** to finish, so use them efficiently. Make
an edit as soon as you know what it is, and run a gate rather than
re-reading a file you have already read. **Your first source edit is the
first red test below, and it lands within 20 turns of starting.** Every
test you write red stays in the tree after its fix.

## First red test

Write this before reading anything beyond `dispatch_send_input.rs`.

- **File:** `src/daemon/executor/dispatch_send_input.rs`. Add a
  `#[cfg(test)] mod tests { use super::*; … }` at the end. The file has no
  test module yet.
- **Name:** `send_input_refuses_an_unparseable_shell_id_before_the_prompt`,
  a `#[tokio::test]`.
- **Setup, in order:**
  1. `let (_guard, _home) = crate::test_home();` gives a temp HOME and the
     test-home lock. Do **not** also call `crate::test_home_guard()`, which
     is not re-entrant and deadlocks.
  2. A chat call. Use `let store = SessionStore::new();` with nothing
     inserted, and `session_id: Some("sess-03")`. A session the store does
     not hold is not a ghost.
  3. The call is `PendingCall::SendInput { … }` with `shell: "../../x"`,
     `text: "echo hi"` and the other fields at their empty or default
     values. Read the variant's fields in `src/ai/types/pending.rs`.
  4. Drive `execute_tool_call` the way `approved_path_tests::drive` in
     `dispatch_foreground.rs` does:
     - `tx` is a `tokio::io::duplex(64 * 1024)` pair. Keep the client half
       to read the frames back.
     - `rx` is `std::io::Cursor::new(Vec::<u8>::new())`, an **empty** reply.
       A run that reaches the prompt then hits EOF at once and returns
       `Err`, rather than waiting out the approval timeout.
     - `shells` is `crate::shell::registry()`.
     - Pass `&Arc::new(SessionCache::new("home"))` and
       `&Arc::new(crate::scheduler::ScheduleStore::new_empty())`.
     - Keep the `Result`; do not `.expect` it. Drop `tx`, then read every
       frame off the client half as `Response`s.
- **Assert:**
  - The outcome is `Ok(ToolCallOutcome::Result(text))`, and `text` names
    `../../x` and says nothing was sent.
  - No frame is a `Response::ToolCallPrompt`.
- **Red on today's tree:** a `ToolCallPrompt` frame is written, and the
  empty reply makes the outcome an `Err` (EOF). Paste that failure into the
  Update Log, then fix it.

**Helpers for the remaining tests:**

- **A remote shell record.** Insert one directly into the global registry
  with `crate::shell::with_shells(crate::shell::registry(), |s| { s.insert(…) })`.
  Build the `ShellRecord` field by field, as mod.rs's
  `get_shell_context_without_chat_pane_is_shells_only` does, with
  `host: crate::host::HostId::Named("pinky".into())` and
  `owner: Owner::Chat("sess-03".into())`, so it is this session's. Give it an index no
  other test uses, such as `ShellId::from_index(903)`, and remove it at the
  end of the test. The record has no socket, so the probe and the screen
  read fail fast (2 s and 5 s caps) and fall back to their defaults.
- **The host table.** Write `"[hosts.pinky]\nssh = \"pinky\"\n"` to
  `crate::config::etc_dir().join("config.toml")`. No config handle is
  installed in tests, so `config::current()` reads that file on every call
  and `prompt_and_await_approval`'s `known_hosts` includes `pinky`.
- **The prompt's fields.** With the empty reply above, read the
  `ToolCallPrompt` frame and check its `classification` (and whatever sudo
  signal you add).
- **The client side.** `SessionApproval::from_config(&ApprovalsConfig::default())`
  and `auto_approves(&class, is_sudo)` are pure. The tests in
  `src/cli/commands/stream.rs` (`the_local_approval_flow_is_unchanged`)
  show the pattern.

## Evidence

Checked against the tree at `4bb142c6`:

- `dispatch_send_input.rs` step 2: `Err(_) => crate::shell::Foreground::Unknown`.
  Step 4: `crate::shell::screen(&crate::shell::socket_path(shell), …)`.
  Step 5: `host: None`.
- `socket_path(id)` is `shells_dir().join(format!("{id}.sock"))`
  (`src/shell/launch.rs`), with no validation.
- `ipc::classify(None, false, _)` is `Classification::Command`.
  `classify(Some("pinky"), false, &["pinky"])` is `RemoteShell { host: "pinky" }`.
- In the client, `command_has_sudo(command)` runs on the panel string
  (`stream_prompts.rs`). `SessionApproval::from_config(&default).auto_approves(&Command, false)`
  is `true`, as the test `the_local_approval_flow_is_unchanged` pins.
- `input_panel` formats the text as `{text:?}` and the keys as `keys [a, b]`.
- `parse_key` accepts a single printable character as `Key::Char`.

## Acceptance criteria

Each behaviour is covered by a test that fails on the current tree, unless
it is marked as preservation. Show each red run in the Update Log before
its fix lands. Every such test stays in the tree.

- [ ] **S2c:** `send_input` with an id `ShellId::parse` rejects (for example
      `../../x`) returns a refusal naming the id. No `ToolCallPrompt` is
      sent and nothing is typed. (This is the first red test.)
- [ ] **S2c:** `send_input` with an id that parses but is not in the
      registry is refused the same way, before any prompt.
- [ ] **S2b, host:** `send_input` into a registered shell whose host is
      `HostId::Named("pinky")`, with `[hosts.pinky]` configured, sends a
      `ToolCallPrompt` whose `classification` is
      `RemoteShell { host: "pinky" }`.
- [ ] **S2b, host (preservation):** `send_input` into a local shell is
      still classified `Command`.
- [ ] **S2b, sudo:** each of these is treated as sudo by the client
      decision, so under `ApprovalsConfig::default()` it is **not**
      auto-approved:
      - `text: "echo hi\nsudo reboot"`
      - `keys: ["s","u","d","o","Space","r","e","b","o","o","t","Enter"]`
      - `text: "sudo reboot"`
- [ ] **S2b, sudo (negative):** `text: "echo pseudo"`, `text: "cat /etc/sudoers"`
      and `keys: ["C-c"]` are **not** treated as sudo.
- [ ] **bug-03-1, key spellings:** `keys: ["s","u","d","o","space","r","e","b","o","o","t","enter"]`,
      `keys: ["e","c","h","o","C-m","s","u","d","o","Space","x","C-m"]` and
      the same list with `C-j` in place of `C-m` are each treated as sudo
      (the prompt carries `sudo: true`, and `ApprovalsConfig::default()`
      does not auto-approve it). The key parser lowercases names, and
      `C-m`/`C-j` end the line, so a different spelling of the same key must
      not change the decision. **Fails on the current tree.**
- [ ] **bug-03-1, preservation:** a bracketed paste of `text: "sudo reboot"`
      (`paste: Some(true)`) is still sudo, and the negatives above still
      hold.
- [ ] **bug-03-1, names:** `grep -n '_for_tests' src/cli/commands/approval.rs`
      and `grep -n '_owner_key' src/daemon/executor/dispatch_send_input.rs`
      both print nothing. **Both print a line on the current tree.**
- [ ] **bug-03-1, count:** the lib suite reports more than 3065 passed.
- [ ] **Preservation:** `run_terminal_command`'s prompts are unchanged. A
      foreground command's sudo decision still comes from its command
      text, and the existing `stream.rs` approval tests and
      `prompt_classification_marks_the_users_shell` pass unchanged.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings` and `cargo test` all pass, with
      `tests/hygiene.rs` and `tests/doc_truth.rs` included.

## Spec

1. **Break down** — read the statement of work and the acceptance criteria,
   then write the first red test and run it red. Then write a numbered task
   list into a progress entry in the Update Log, one line per task, each
   naming the acceptance item it closes. Ends when that entry is written.
2. **Implement** — work through your task list, writing each test red
   before its fix. Ends at a green `cargo build --tests`.
3. **Meet the acceptance criteria** — check off every item against your
   tests and the gates. Ends when every item is checked, with its evidence
   named.
4. **Capture the end-to-end evidence** — run the four gate commands, then
   paste each `test result:` line and your red runs into a new Update Log
   entry. Do not paste per-test `… ok` lines.

## Authorizations

- May touch `src/daemon/executor/**`, `src/ipc.rs` (one defaulted field on
  `Response::ToolCallPrompt`, if you choose that route), and
  `src/cli/commands/stream_prompts.rs` (the client's sudo decision), plus
  their test files. Every other constructor of `Response::ToolCallPrompt`
  that the compiler names may be updated to fill the new field. If a change
  outside these seems necessary, record why in the Update Log before
  making it.

## Out of scope

- `command_has_sudo` itself: quote and backslash splitting, redirections,
  other elevators. That is phase 05. Use it as it is.
- A ghost's `send_input` classification (`EscapeHatch`) and the ghost
  gates. They are unchanged.
- The client's `RemoteShell` grant logic. The PE decision keeps it as is.
- `read_shell_screen` and the other shell readers.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-11-22 23:40 (progress)

Executor started the phase (status flipped to `in-progress`, README row
updated). Read the statement of work, evidence, acceptance criteria and
scoping hints only, then: `dispatch_send_input.rs` (the whole arm),
`executor/mod.rs` (`ApprovalRequest`, `tool_call_prompt_frame`,
`prompt_classification`, `prompt_and_await_approval`), `ipc.rs`
(`ToolCallPrompt`, `Classification`, `classify`),
`cli/commands/stream_prompts.rs` (the client's sudo decision) and
`daemon/utils/sudo.rs` (`command_has_sudo`, read only). Beyond the hints,
for the helpers the first red test needs: `dispatch_foreground.rs`'s
`approved_path_tests::drive` (the spec names it as the pattern),
`ai/types/pending.rs` (the `SendInput` variant's fields),
`shell/registry.rs` (`ShellRecord`'s fields, `with_shells`),
`shell/launch.rs` (`validate_shell_id` via `ShellId::parse`),
`cli/commands/approval.rs` (`SessionApproval::auto_approves`) and
`cli/commands/stream.rs` (`the_local_approval_flow_is_unchanged`).

Task list (each closes the acceptance item in brackets):

1. Write the first red test
   `send_input_refuses_an_unparseable_shell_id_before_the_prompt` in
   `dispatch_send_input.rs`, run it red, paste the failure [S2c, first
   bullet].
2. Refuse an id `ShellId::parse` rejects before the probe, the screen read
   and the prompt; refusal names the id and says nothing was sent [S2c
   bullet 1].
3. Refuse an id that parses but is not in the registry the same way, before
   any prompt [S2c bullet 2].
4. Carry the shell record's `HostId` into `ApprovalRequest.host`:
   `Named(n)` → `Some(n)`, `Local` → `None` [S2b host].
5. Test: a registered shell with `HostId::Named("pinky")` and
   `[hosts.pinky]` sends a `ToolCallPrompt` classified
   `RemoteShell { host: "pinky" }` [S2b host].
6. Test (preservation): a local shell is still classified `Command` [S2b
   host preservation].
7. Derive the sudo-decision text (text with real newlines, then keys typed
   as characters: single printable char keys, `Space` → space, `Enter` →
   newline), add a defaulted `sudo` signal field on
   `Response::ToolCallPrompt`, and have the client read it in
   `prompt_tool_call_ratatui` instead of `command_has_sudo(command)`
   [S2b sudo].
8. Tests red first: `text: "echo hi\nsudo reboot"`, keys
   `["s","u","d","o","Space","r","e","b","o","o","t","Enter"]` and
   `text: "sudo reboot"` each treat the prompt as sudo (not auto-approved
   under `ApprovalsConfig::default()`); negatives `text: "echo pseudo"`,
   `text: "cat /etc/sudoers"` and `keys: ["C-c"]` are not [S2b sudo +
   negative].
9. Preservation: `run_terminal_command` prompts unchanged — the existing
   `stream.rs` approval tests and `prompt_classification_marks_the_users_shell`
   pass unchanged [preservation].
10. All four gates plus `tests/hygiene.rs` and `tests/doc_truth.rs` [gate
    bullet].
11. Capture the end-to-end evidence entry [Spec task 4].

**First red run** (`cargo test --lib --
send_input_refuses_an_unparseable_shell_id_before_the_prompt`, verbatim):

```
running 1 test
test daemon::executor::dispatch_send_input::tests::send_input_refuses_an_unparseable_shell_id_before_the_prompt ... FAILED

failures:

---- daemon::executor::dispatch_send_input::tests::send_input_refuses_an_unparseable_shell_id_before_the_prompt stdout ----

thread 'daemon::executor::dispatch_send_input::tests::send_input_refuses_an_unparseable_shell_id_before_the_prompt' (1977423) panicked at src/daemon/executor/dispatch_send_input.rs:193:34:
the run must not error: EOF
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::executor::dispatch_send_input::tests::send_input_refuses_an_unparseable_shell_id_before_the_prompt

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3067 filtered out; finished in 0.01s
```

Exactly the predicted red: the arm reaches the prompt (the empty reply
makes `approval_reply` hit EOF and the outcome an `Err`), because today
the unparseable id is treated as `Foreground::Unknown` and carried on.

### Update — 2026-11-23 01:05 (progress)

Tasks 1–6 of the list are done: the first red test landed red (above) and
then green; the id gate refuses both an unparseable id and an id the
registry does not hold before the probe, the screen read and the prompt;
`ApprovalRequest.host` now carries the record's `HostId` (`Named(n)` →
`Some(n)`, `Local` → `None`), and the two classification tests (remote →
`RemoteShell { host: "pinky" }` with `[hosts.pinky]` written to the test
HOME's `config.toml`; local → `Command` preservation) pass through
`execute_tool_call`. `unknown_shell_error` was widened `pub(super)` →
`pub(crate)` in `knowledge/shell_watch.rs` and re-exported from
`knowledge/mod.rs` so the arm shows the same refusal wording every shell
tool shows — recorded here per the authorizations' "outside these" note;
it is a visibility widening only, no behaviour change. Next: task 7, the
sudo-decision text and its wire signal, with the sudo tests red first.


### Update — 2026-11-23 (escalation)

**Chosen lever:** resume
**Rationale:** the run hard-failed at turn 417 on `VerifierFailurePersistent`,
one E0382 in `executor/mod.rs`, after finishing tasks 1–8. The first source
edit came at turn 66. The architect checked the partial tree with the
two-line borrow fix applied, then reverted it. With the fix, build, clippy,
`hygiene` and the full test suite pass (lib 3065 passed). Only `cargo fmt`
fails. What caused the error: adding the `sudo` field took `executor/mod.rs`
toward its 800-line production cap. The run compacted
`tool_call_prompt_frame` to take `ApprovalRequest` by value, then used `req`
after the move. That is one wall at the end of a correct phase, and the spec
was not the problem, so resume rather than re-dispatch.

### Update — 2026-11-23 01:20 (progress, resumed run)

Applied the architect's two-line borrow fix exactly as given: `req:
ApprovalRequest<'_>` → `req: &ApprovalRequest<'_>` in
`fn tool_call_prompt_frame(`, and `tool_call_prompt_frame(req, …)` →
`tool_call_prompt_frame(&req, …)` in `prompt_and_await_approval`. Nothing else
in that function changed. `cargo build` then passed clean.

Red evidence for the sudo tests (task 8), as the mutation the architect asked
for — in `tool_call_prompt_frame`, `sudo: req.sudo.unwrap_or(false)` →
`sudo: false`, then `cargo test --lib send_input_`:

```
running 20 tests
test daemon::executor::dispatch_send_input::tests::send_input_types_sudo_on_a_newline_and_the_client_is_asked ... FAILED
test daemon::executor::dispatch_send_input::tests::send_input_types_sudo_through_single_character_keys_and_the_client_is_asked ... FAILED
test daemon::executor::dispatch_send_input::tests::send_input_with_a_plain_sudo_text_is_asked ... FAILED

---- send_input_types_sudo_on_a_newline_and_the_client_is_asked stdout ----
thread '…' panicked at src/daemon/executor/dispatch_send_input.rs:494:9:
the daemon says the input types sudo
---- send_input_types_sudo_through_single_character_keys_and_the_client_is_asked stdout ----
thread '…' panicked at src/daemon/executor/dispatch_send_input.rs:523:9:
the daemon says the keys type sudo
---- send_input_with_a_plain_sudo_text_is_asked stdout ----
thread '…' panicked at src/daemon/executor/dispatch_send_input.rs:544:9:
the daemon says the input types sudo

test result: FAILED. 20 passed; 3 failed; 0 ignored; 0 measured; 3052 filtered out; finished in 0.08s
```

Failing sudo tests: `send_input_types_sudo_on_a_newline_and_the_client_is_asked`,
`send_input_types_sudo_through_single_character_keys_and_the_client_is_asked`,
`send_input_with_a_plain_sudo_text_is_asked`. Mutation restored
(`sudo: req.sudo.unwrap_or(false)`) and the same run is green (23 passed).

All four gates and the acceptance criteria are confirmed; evidence entry
follows.

### Update — 2026-11-23 01:25 (end-to-end verification)

Ran on the resumed tree after the borrow fix, with the sudo mutation reverted.
Each gate command's captured `test result:` lines, verbatim:

```
$ cargo fmt --all && cargo fmt --all -- --check && echo FMT_OK
FMT_OK

$ cargo build 2>&1 | tail -3
   Compiling daemoneye v2.10.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized+debuginfo] target(s) in 8.38s

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3
    Checking daemoneye v2.10.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized+debuginfo] target(s) in 12.79s

$ cargo test 2>&1 | grep -E "^test result"
test result: ok. 3065 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 143.16s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.21s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.22s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The 16-passed suite is `tests/hygiene.rs`, the 19-passed suite is
`tests/doc_truth.rs`; both pass with the phase's changes included.

Acceptance criteria, each with the test that covers it:

- **S2c, unparseable id** —
  `send_input_refuses_an_unparseable_shell_id_before_the_prompt`
  (`dispatch_send_input.rs`): the run returns
  `Ok(ToolCallOutcome::Result)` naming `../../x` and "nothing was sent", no
  `ToolCallPrompt` frame is written. Red first (escalation entry above).
- **S2c, unregistered id** —
  `send_input_refuses_an_id_the_registry_does_not_hold_before_the_prompt`:
  `s990` refused with `Error: …`, no prompt frame.
- **S2b host, remote** — `send_input_into_a_remote_shell_is_classified_remote_shell`:
  the prompt's `classification` is `RemoteShell { host: "pinky" }`, driven
  through `execute_tool_call` with `[hosts.pinky]` in the test HOME's
  `config.toml`.
- **S2b host, local preservation** —
  `send_input_into_a_local_shell_is_still_classified_command`:
  `classification` stays `Command`.
- **S2b sudo, three positive spellings** —
  `send_input_types_sudo_on_a_newline_and_the_client_is_asked`,
  `send_input_types_sudo_through_single_character_keys_and_the_client_is_asked`,
  `send_input_with_a_plain_sudo_text_is_asked`: the prompt carries `sudo:
  true` and `SessionApproval::from_config(&ApprovalsConfig::default())
  .auto_approves(&Command, true)` is false — the client asks. Red first (the
  `sudo: false` mutation above).
- **S2b sudo negatives** —
  `send_input_sudo_decision_text_rejects_sudo_substrings_and_control_keys`:
  `echo pseudo`, `cat /etc/sudoers` and keys `["C-c"]` are not sudo.
- **Preservation, run_terminal_command prompts** — the existing
  `stream.rs` approval tests and
  `prompt_classification_marks_the_users_shell` pass unchanged (both green in
  the full run; the arm's `ApprovalRequest.sudo` is `None`, so
  `command_has_sudo(command)` still decides, and `tool_call_prompt_frame`
  fills `sudo: false`).

Grep for the spec-pinned literals: `sudo: bool` lands on `Response::ToolCallPrompt`
(`src/ipc.rs:439`), the client reads it in `prompt_tool_call_ratatui`
(`src/cli/commands/stream_prompts.rs:395`,
`let is_sudo = prompt_says_sudo || command_has_sudo(command);`), and
`sudo_decision_text` produces the `sudo` word from real newlines and
single-character keys (`src/daemon/executor/knowledge/shell_input.rs:71`).

### Update — 2026-11-23 01:19 (complete, server-authored)

**Summary:** **Summary:** Resumed phase 03 and finished it. I applied the architect's two-line borrow fix exactly as given (`tool_call_prompt_frame` now takes `&ApprovalRequest<'_>`, and the call site passes `&req`) — nothing else in that function changed — then captured the requested red evidence by mutating `sudo: req.sudo.unwrap_or(false)` to `sudo: false`: exactly the three sudo tests failed (`send_input_types_sudo_on_a_newline_and_the_client_is_asked`, `send_input_types_sudo_through_single_character_keys_and_the_client_is_asked`, `send_input_with_a_plain_sudo_text_is_asked`), and the run went green (23 passed) after restoring the line. The phase's three defects are closed by the code already on disk: an unparseable or unregistered shell id is refused before the probe, screen read and prompt; the approval request carries the shell record's `HostId` so remote input classifies `RemoteShell { host: "pinky" }` while local stays `Command`; and the prompt carries a sudo fact derived from the text the shell will receive, which the client ORs into its decision.

**Notes for review:** All four gates ran clean on the final tree — `cargo fmt --all -- --check` OK, build and clippy (`-D warnings`) clean, full `cargo test` green with the lib suite at **3065 passed**, `tests/hygiene.rs` (16 passed) and `tests/doc_truth.rs` (19 passed) included; preservation tests `prompt_classification_marks_the_users_shell` and `the_local_approval_flow_is_unchanged` pass unchanged. Both the mutation red run and the full gate evidence are pasted verbatim in new Update Log entries, with each acceptance criterion mapped to its covering test. One earlier adaptation to note: `unknown_shell_error` was widened `pub(super)` → `pub(crate)` so the arm's registry-miss refusal shows the standard wording (recorded in the 01:05 progress entry per the authorizations' "outside these" note). Phase doc status and README row remain `in-progress` for the server's completion tail; everything is committed as `fix: gate send_input on shell id validity, host and sudo` and `git status --short` is clean.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test attach_sends_session_counters_after_ok ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.21s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_installs_no_tmux_hooks ... ok
test an_empty_session_starts_no_tmux_server ... ok
test notices_follow_streams_a_webhook_alert ... ok
test default_server_unchanged ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-c067c2b4b53c4301)
kill: (2073053): No such process
kill: (-2073717): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-d9a9e35dfb95286a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-2e2bade347dfc2bf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-fec57f080473704c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-b7049ce40c11944f)
     Running tests/integration.rs (target/debug/deps/integration-521aa0dc073b7f75)
     Running tests/isolation.rs (target/debug/deps/isolation-4e1a6c89c680c6e7)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-25580a42c302b017)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-a1f9f82f934e25c3)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M39-critical-fixes/phase-03-send-input-gate.md` — +224 -1
- `src/daemon/executor/dispatch_send_input.rs` — +468 -40
- `src/daemon/executor/mod.rs` — +3 -8

**Commit:** 608fd422c0272d0dea896db3d139c859d8dd3c4f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review — 2026-11-23 (bounced: bug-03-1)

The id gate, the host classification and the three pinned sudo spellings
are correct and tested, and all four gates pass on the reviewer's re-run
(lib 3065 passed). The bounce is for
[bug-03-1](bugs/bug-03-1.md).

1. **(major)** `sudo_decision_text` recognises only the exact strings
   `Space` and `Enter`. The key parser also accepts `space` and `enter` in
   any case, plus `C-m` and `C-j` as line ends. The reviewer probed it:
   keys `s,u,d,o,space,x,enter` give the decision text `sudox`, while the
   shell receives `sudo x⏎`, so the call auto-approves. This gap came from
   this doc, whose statement of work named only `Space` and `Enter`.
2. **(minor)** The production approval decision lives in
   `auto_approves_for_tests`, and `_owner_key` is computed and then
   discarded.

The new acceptance criteria above express the remaining work. **The tree is
clean and the gates are green, which is expected. That is NOT evidence the
phase is done.** Already approved, so do not redo it: the id gate (both
S2c tests), the host classification (both S2b host tests), the
`ToolCallPrompt.sudo` field and the client OR. What remains is the
decision-text mapping, the new tests, and the rename/cleanup in item 2.
Keep every existing phase-03 test. Finish condition: the lib suite reports
more than 3065 passed, and the mutation check named in the bug is pasted
here.

### Update — 2026-11-23 (escalation)

**Chosen lever:** session takeover
**Rationale:** the bug-03-1 re-dispatch hard-failed with `NoProgressStall`
after 200 read-only turns and no edits. That is glm-5.3-flash's
read-paralysis pattern, seen eight times before, on a bounce whose
remaining work was one function and a rename. The PE's standing guidance
for a small bounce in that state is to take it over rather than
re-dispatch.

The architect fixed it directly in `68ce1c3a`:

- `sudo_decision_text` now encodes each key through `encode_keys`, the
  parser `send_input` sends with. A key that encodes to `\r` or `\n` is a
  newline, a space or tab is a space, and a printable byte is itself.
  Anything else contributes nothing. So `space`, `enter`, `C-m` and `C-j`
  count exactly like `Space` and `Enter`. `text` is still read as written,
  so a bracketed paste's start marker cannot hide `sudo`.
- `SessionApproval::auto_approves` holds the decision again, and
  `auto_approves_for_tests` is gone. The thrown-away `_owner_key` is gone
  too.
- New tests: `send_input_sees_sudo_through_every_spelling_of_space_and_enter`,
  which drives the lowercase, `C-m` and `C-j` lists through
  `execute_tool_call`, and `send_input_bracketed_paste_of_sudo_is_still_sudo`.

Mutation check: `sudo_decision_text` reverted to the two-name mapping,
then `cargo test --lib send_input_`:

```
test daemon::executor::dispatch_send_input::tests::send_input_sees_sudo_through_every_spelling_of_space_and_enter ... FAILED
thread 'daemon::executor::dispatch_send_input::tests::send_input_sees_sudo_through_every_spelling_of_space_and_enter' (2107002) panicked at src/daemon/executor/dispatch_send_input.rs:580:13:
test result: FAILED. 24 passed; 1 failed; 0 ignored; 0 measured; 3052 filtered out; finished in 0.08s
```

Restored, the same run reports `25 passed`.

Gates on the final tree: `cargo fmt --all -- --check` passes, and build and
clippy (`-D warnings`) are clean. `cargo test`:

```
test result: ok. 3067 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out
```

`hygiene` reports 16 passed and `doc_truth` 19 passed. Both bug-03-1 greps
print nothing.

### Review verdict — 2026-11-23

- **Verdict:** escalated
- **Bounces:** 1 (bug-03-1, major: other spellings of Space and Enter hid `sudo`)
- **Executor:** GLM-5.3-Flash (run 1 hard-failed at 417 turns on E0382, the
  resume completed in 42 turns, and the bounce run stalled at 200 turns with
  no edits); Claude (direct) for bug-03-1
- **Scope deviations:** `unknown_shell_error` widened `pub(super)` →
  `pub(crate)` to reuse the standard refusal wording. Accepted: it widens
  visibility only and was logged by the executor.
- **Calibration:** the bounce was a spec bug. The statement of work
  dictated the exact key names `Space` and `Enter` rather than "whatever
  the key parser encodes as a space or a line end". When a spec names
  spellings for a security check, derive them from the parser, or state
  the rule as "agrees with the bytes sent". On run 1, the 800-line
  production cap on `executor/mod.rs` pushed the executor into compacting a
  signature, which produced the E0382. A resume with the exact two-line fix
  was the right lever (42 turns).
