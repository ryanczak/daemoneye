# Phase 09 — long-foreground-turn

**Status:** todo
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 08 (for the suite count)
**Tags:** language=rust, kind=fix, size=m

## Goal

GitHub issue #1, bug 2. A foreground `run_terminal_command` that runs for
more than 120 s silently deletes its whole turn. The wait in
`dispatch_foreground.rs` sends nothing while the command runs; the chat
client abandons the connection after 120 s of silence
(`PHASE2_SILENCE_TIMEOUT_SECS`, `src/cli/commands/stream.rs`); the daemon's
next write fails, `execute_tool_call` returns `Err`, and
`run_conversation_loop`'s arm for that — `Err(_) => return Ok(())` — skips
every kind of persistence. Seen live on pinky (2026-10-31, run
`01d000e2`): the user's message and the model's two tool calls never
reached the transcript, no `ToolResult` or `command` event was written, the
run log ended `HardFail: the run loop exited without recording an
outcome`, and `daemon.log` has no line for it. The documented
`timeout_secs` cap is 3600 s, so any legitimately long command hits this.

After this phase:

1. The foreground wait sends `Response::KeepAlive` every
   `KEEPALIVE_PERIOD_SECS` while the command runs, as the memory and
   shell-reader dispatches already do through `with_keepalive`, so a
   connected client never times out on a long command.
2. If the client *is* gone when a tool call fails, the turn is kept: a
   warning is logged, the messages so far are persisted with a result for
   **every** tool call (so `chat --resume` sends a valid history), and the
   run log records why it ended.

The suite goes from **2634 to exactly 2636**.

## Constraints

- **Keep-alives: the plain path, both hosts.** In `run_approved` and
  `run_on_named_host` (`src/daemon/executor/dispatch_foreground.rs`) the
  non-sudo arm — the `shell_exec::run_in_shell(...)` call — runs under
  `crate::daemon::utils::with_keepalive(tx, …)`. The sudo arm
  (`run_in_shell_with_prompts`) is out of scope: its `tx` travels inside
  `SudoPromptArgs` for the credential round trip. Name that in Findings;
  change nothing there.
- **One ending, beside its siblings.** The `Err(_) => return Ok(())` arm
  in `run_conversation_loop` (`src/daemon/stream/conversation.rs`) becomes
  one call to a new `end_client_gone` in `src/daemon/stream/endings.rs`,
  the fourth terminal reply there. It returns `Ok(())` as the arm does
  today — there is nobody to send a frame to — and it:
  - `log::warn!`s with the session id, the tool name and the error;
  - appends the turn's tool-results message: the results gathered so far
    (truncated with `truncate_tool_results` as the normal path does) plus,
    for each pending call without one, a synthetic result whose content
    says the chat client disconnected before the call finished and that
    its command may still be running (`list_shells` / `read_shell` find
    it). The assistant message listing the calls is already in
    `messages`; after this, every `tool_call` id has exactly one result;
  - persists `messages` the way the cancel arm does (the store entry,
    `dirty`, and `append_session_message` for everything past
    `post_trim_len`);
  - finishes the run log `HardFail` with a blocker naming the disconnect
    and the tool, in place of the guard's "exited without recording an
    outcome".
- The pure part — turning the pending calls and the results so far into
  one complete tool-results list — is a function on its own in
  `endings.rs`, so it can be tested without a conversation loop. Its name
  is the executor's choice.
- The other `?` exits in `run_conversation_loop` (a token send to a gone
  client, for instance) are out of scope; this phase handles the tool-call
  arm, which is the one the issue hit.
- `run_conversation_loop` is allowlisted at 775 lines. If the arm grows it,
  raise the line in `tests/hygiene_allowlist.txt` and say why in the Update
  Log; if the extraction shrinks it, lower the line.
- **Use `patch` with whole blocks,** never `patch_lines`.

## Spec

1. **Keep-alives during the wait** — wrap the non-sudo
   `shell_exec::run_in_shell(...)` await in `run_approved` and in
   `run_on_named_host` with `with_keepalive(tx, …)`. Add
   `a_long_foreground_command_sends_keepalives_while_it_runs` to
   `approved_path_tests` in `dispatch_foreground.rs`, driven like
   `an_approved_local_command_runs_in_the_sessions_shell`: the command is
   `sleep 16; echo de-ka-done` with a `timeout_secs` of 30; assert that at
   least one `Response::KeepAlive` frame lies between the
   `ToolCallPrompt` and the `ToolResult`, and that the result carries
   `de-ka-done`. This test takes ~16 s of wall clock by design — one
   keep-alive period plus a margin — and that is authorized.
   **Red run required:** run this test against the pre-phase tree (stash
   step 1's production change, or write the test first) and paste the
   failing assertion — no `KeepAlive` frame — fenced, in an
   `### Update — <date> (red run)` entry.
2. **The client-gone ending** — `end_client_gone` in `endings.rs` and its
   pure helper, per the Constraints; the `Err(_)` arm in
   `conversation.rs` becomes the one call. Add
   `client_gone_persists_the_turn_with_a_result_for_every_call` in a new
   test module in `endings.rs`: two pending `run_terminal_command` calls
   with ids `a` and `b`, one gathered result for `a`; the helper yields two
   results in call order, `a`'s content unchanged, `b`'s content naming the
   disconnect; and with no gathered results every call gets a synthetic
   one.
3. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`; name the sudo arm as the
   keep-alive gap left.
4. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry. This
   step is required.
5. **Commit** as `fix: a long foreground command keeps its turn alive and persisted (#1)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `keepalive_waits=2 client_gone_calls=1 bare_return=0 new_tests=2`
      (today `keepalive_waits=0 client_gone_calls=0 bare_return=1 new_tests=0`).
- [ ] The red run is pasted and shows
      `a_long_foreground_command_sends_keepalives_while_it_runs` failing on
      the pre-phase tree for want of a `KeepAlive` frame.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2636 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
d=src/daemon/executor/dispatch_foreground.rs
c=src/daemon/stream/conversation.rs
e=src/daemon/stream/endings.rs
nt=$(( $(grep -c 'fn a_long_foreground_command_sends_keepalives_while_it_runs' $d) + $(grep -c 'fn client_gone_persists_the_turn_with_a_result_for_every_call' $e) ))
echo "keepalive_waits=$(sed -n '/^#\[cfg(test)\]/q;p' $d | grep -c 'with_keepalive(') client_gone_calls=$(grep -c 'end_client_gone(' $c) bare_return=$(grep -c 'Err(_) => return Ok(())' $c) new_tests=$nt"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

`keepalive_waits` counts call sites in the production part of
`dispatch_foreground.rs` only — the test module is cut off first — so the
count is the two arms and nothing the test does.

## Authorizations

- Edit `src/daemon/executor/dispatch_foreground.rs`,
  `src/daemon/stream/conversation.rs`, `src/daemon/stream/endings.rs` and
  `tests/hygiene_allowlist.txt` (only per the ratchet rule above).

## Out of scope

- The sudo arm's keep-alives; the other `?` exits of the conversation loop;
  the client's silence budget (`stream.rs`), which is right once the daemon
  keeps its 15 s contract; `daemoneye ask`.

## Update Log

<!-- entries appended below this line -->
