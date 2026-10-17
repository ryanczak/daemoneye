# Phase 03e: interactive-param — `run_terminal_command(interactive: true)`

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-03d
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Since 03d, a command that `is_interactive_command` recognises (`ssh`,
`mosh`, …) is **launched**: it is typed into the shell, the call waits only
for the screen to settle, and the result is the screen. Any other program
that never exits, such as `vim`, `top`, `claude` or a REPL, still goes
through the marker protocol and times out. This phase lets the model ask
for the launch path explicitly with `run_terminal_command(interactive: true)`.

**Design adjustment, recorded here:** D6 said `interactive` combined with
`wait: false` stays refused. Here instead, `interactive: true` always takes
the waited, foreground path, and `wait: false` is ignored. A launch never
waits on the command anyway, and the refusal existed only to keep an
interactive command out of the dedicated background shell, which this
routing already does. The result says the `wait` was ignored.

## Architecture references

- `docs/design/interactive-shells.md` D6.

## Spec

1. **The parameter.** `run_terminal_command` gains `interactive: bool`
   (optional, default false). Carry it on `RunTerminalCommandArgs`, then
   `AiEvent::ToolCall`, then `PendingCall::Foreground` (only `Foreground`:
   an interactive call is never `Background`), then `to_tool_call` (which
   writes it back into the arguments JSON). `defs.rs` gets a `ParamDef`.
   Its description says: for a program that does not exit (a TUI such as
   `vim`, `top` or `claude`, or a REPL); the command is typed into the
   shell, the call waits only for the screen to settle (up to 5 s) and
   returns the screen; nothing waits for the program to finish; drive it
   with `send_input` and read it with `read_shell_screen`; `ssh` and `mosh`
   are launched this way automatically. Every other `AiEvent::ToolCall` and
   `PendingCall::Foreground` literal gains `interactive: false`; the
   compiler names them. Today there are 8 `AiEvent::ToolCall` literals
   (`gemini.rs`, `args.rs`, 4 in `dispatch.rs`, `ghost.rs`, `stream.rs`)
   and 9 `PendingCall::Foreground` literals (7 in `pending.rs`,
   `executor/mod.rs`, `stream.rs`).
2. **Never backgrounded.** `routes_to_background` (`src/daemon/stream.rs`)
   takes `interactive` as well, and returns false whenever it is true. Its 4
   callers pass the event's flag; the existing test
   `wait_false_runs_in_the_background` passes `false` and keeps its
   assertions.
3. **The launch path.** In `src/daemon/executor/shell_exec.rs`:
   - `run_in_shell_core` (one caller) gains `interactive: bool`, and the
     03d branch becomes `if interactive || is_interactive_command(cmd)`;
   - `run_in_shell_with_prompts` keeps its signature and passes `false`;
   - a new `pub(crate) async fn run_interactive_in_shell(reg, owner, cmd, max_per_owner, shell, host, spawn)`
     calls `run_in_shell_core` with no prompts, no timeout and
     `interactive: true`. To call it with no prompts, pass `None` the same
     way the test helper `run_in_shell_for_test_on_host` does.
4. **The executor arm.** In `src/daemon/executor/mod.rs`, the two
   `PendingCall::Foreground` call sites of `run_in_shell_with_prompts` (the
   local one and the named-host one) call `run_interactive_in_shell`
   instead when `interactive` is true, after the same approval. When the
   call came in with `wait: false` and `interactive: true`, prefix the
   result with `(wait: false ignored: an interactive launch never waits on
   the command.)` and a newline. Carry what the arm needs to know that.
5. **Docs.** Add `interactive` to the `run_terminal_command` bullet in
   `sre.toml`, with one example: `run_terminal_command("claude", interactive: true)`,
   followed by `send_input` and `read_shell_screen(settle_ms: …)`. Add one
   sentence to the `run_terminal_command` rows of `CLAUDE.md` and
   `README.md`. `tests/doc_truth.rs` must pass unmodified.
6. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03e.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Gotchas.**
- Do not change the approval request's `cmd`: it also feeds the approval
  classification. The panel line D6 describes is deferred (§ Out of scope).
- A test recipe for Task 3, as in 03d: `test_home()`, then
  `ShellRegistry::default()`, then
  `run_interactive_in_shell(&reg, owner, "sleep 30", 5, None, HostId::Local, spawn_in_process_host)`,
  then clean up with `send_input(&socket_path(id), b"\x03", …)`. Do not read
  files a line at a time: find an anchor with `grep -n`, then read one
  window around it.

## Tests

- `run_terminal_command_args_carry_interactive` (`dispatch.rs` tests,
  through `dispatch_tool_event`): `{command: "vim", interactive: true}` ⇒
  `AiEvent::ToolCall` with `interactive: true`; with no `interactive` ⇒
  `false`.
- `an_interactive_call_is_never_backgrounded` (`stream.rs` tests, next to
  `wait_false_runs_in_the_background`): `routes_to_background(Some(false), true)`
  is false, and `routes_to_background(Some(false), false)` is true.
- `run_interactive_in_shell_launches_any_command` (`shell_exec.rs` tests):
  `sleep 30`, which `is_interactive_command` does **not** recognise, ⇒ `Ok`
  within 10 s. The text starts with `[Interactive session started]`, and
  the record's `last_foreground` is `Program { cmd: "sleep", .. }`. Send
  `\x03` at the end.

## Acceptance criteria

- [ ] The 3 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2394 failed=0`: exactly 2391 at `178f6db9`
      plus the 3 new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03e.txt; : > "$A"
for t in run_terminal_command_args_carry_interactive an_interactive_call_is_never_backgrounded run_interactive_in_shell_launches_any_command wait_false_runs_in_the_background launch_interactive_returns_the_settled_screen; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then run the self-check, and put the line it prints into the entry after
the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-03e-interactive-param.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03e.txt
diff /tmp/pasted-03e.txt /tmp/e2e-03e.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `run_terminal_command`
      rows only).

## Out of scope

- D6's approval panel line ("interactive: the command keeps the shell;
  nothing waits for it"). The panel's text is the approval request's `cmd`,
  which also feeds the approval classification. Changing it is a separate
  decision for the live close to weigh.
- Ghost use of `interactive`. A ghost has no foreground path today
  (`refuse_foreground`), and that is unchanged.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
