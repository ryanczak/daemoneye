# Phase 03 — send-input-gate

**Milestone:** M39 — Critical fixes
**Status:** todo
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
