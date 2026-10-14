# Phase 02c: flag-gone — remove the static `interactive` flag

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-02b
**Estimated diff:** ~250 lines, mostly deletions
**Tags:** language=rust, kind=refactor, size=m

## Goal

`ShellRecord.interactive` is set by one heuristic and never cleared. Phase
02a made reuse probe each shell live, and 02b moved the listings to the
observed foreground, so nothing needs the flag. This phase deletes it,
`mark_interactive`, and everything that reads them.

It also fixes a bug the flag hid. When `run_terminal_command` gets an
interactive command such as `ssh host`, it writes **nothing** to the shell,
yet tells the model "`ssh host` opened an interactive session … the session
is already established". This phase makes that path an honest refusal.

## Architecture references

- `docs/design/interactive-shells.md` D4 (the flag is replaced by observed
  state) and D6 (phase 03 replaces the refusal with `interactive: true`).

## Spec

**The site list below came from the compiler** (field and function deleted,
`cargo build --tests` run). It is complete for the lib and its tests. After
your edits `cargo build --tests` is the check: if it names a site not
listed here, fix it the same way and say so in your Update Log entry.

1. **Delete the field and the setter** — `src/shell/registry.rs`: the
   `interactive` field of `ShellRecord` with its doc comment, and
   `mark_interactive`. Drop `interactive: false` from the three
   `ShellRecord` literals in the same file. Remove `mark_interactive` from
   the re-export in `src/shell/mod.rs`.
2. **Delete the IPC field** — `src/ipc.rs`: `ShellInfo.interactive`.
   `src/daemon/server/shells.rs`: stop setting it. `src/cli/commands/
   slash.rs`: `render_shell_list` drops the `, interactive` state.
3. **Reuse no longer reads it** — `src/shell/session.rs`: drop
   `!record.interactive &&` from the reuse predicates in `shell_for_owner`
   and `shell_for_owner_in_sandbox`. The live probe from phase 02a is what
   keeps a busy shell out.
4. **The interactive-command path refuses honestly** — `src/daemon/
   executor/shell_exec.rs`, the `is_interactive_command(cmd)` early return:
   stop calling `mark_interactive`, and return an **error** instead of the
   `[Interactive session started]` result. Still write nothing to the shell.
   Wording: ``Error: `<cmd>` starts an interactive session, which
   run_terminal_command cannot hold: nothing was run. To start it, open a
   shell with open_shell, type the command into it with send_input, and see
   the session with read_shell_screen.`` Remove `mark_interactive` from the
   file's imports.
5. **Tests whose subject is the flag** — delete exactly these three:
   `registry_mark_interactive_is_idempotent_and_ignores_unknown_ids` and
   `registry_mark_interactive_leaves_an_unknown_id_absent` (`registry.rs`),
   `session_reuse_skips_a_shell_marked_interactive` (`session.rs`; phase
   02a's `shell_for_owner_skips_a_busy_lowest_shell` covers the behaviour).
6. **Tests rewritten to keep their intent** — exactly these three:
   - `shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one`
     (`shell_exec.rs`) becomes
     `run_terminal_command_refuses_an_interactive_command_and_writes_nothing`:
     `ssh nowhere.invalid` returns the Task 4 error (assert it names
     `send_input` and `open_shell`), and a following `echo` reuses the
     **same** shell and prints its output.
   - `shell_exec_non_interactive_still_runs_on_the_same_shell`: drop only
     its assertion on `.interactive`.
   - `the_read_transport_never_opens_an_interactive_shell`
     (`src/daemon/executor/file_ops/read.rs`): instead of setting
     `record.interactive = true`, make the held shell busy —
     `crate::shell::send_input(&socket, b"sleep 30\n", …)`, then poll
     `crate::shell::probe_foreground` until it is not `Prompt` — and keep the
     test's assertion that the second read used a different shell. End the
     sleep with `b"\x03"` at the end.
7. **Literals only** — drop the `interactive` field from the remaining
   literals and field reads, no other change: `src/shell/session.rs`
   (test literals), `src/shell/foreground.rs` (two in
   `classify_record_routes_transport_records_to_the_transport_rule`),
   `src/daemon/prompt.rs` (two), `src/daemon/executor/mod.rs`,
   `src/daemon/executor/knowledge/shell.rs`, `src/cli/commands/agents.rs`,
   `src/cli/commands/slash.rs` (`render_shell_list_shows_each_shells_fields`
   sets `gone.interactive = true`: delete that line and the assertion on
   `interactive` only), `src/daemon/server/shells.rs`
   (`shell_list_carries_each_shells_flags`: delete its two `interactive`
   assertions only), `src/ipc_tests.rs`.
8. **Capture the end-to-end evidence** — run the block in § End-to-end
   verification verbatim and paste its **output** into a new Update Log
   entry headed `### Update — <date> (end-to-end verification)`, inside the
   **only** fenced block of that entry, then add the `PASTE MATCH` line.

**Gotchas.**
- Use `patch` with the whole block as `old_str` for each deletion; do not
  use `patch_lines` (line numbers shift after every deletion).
- Do not run module test suites one by one to check for regressions; the
  evidence block's last line is the full suite.
- `is_interactive_command` and `interactive_destination` stay: the bg path
  and phase 03 use them. Only the flag goes.

## Acceptance criteria

- [ ] `cargo test --lib run_terminal_command_refuses_an_interactive_command_and_writes_nothing`
      and `cargo test --lib the_read_transport_never_opens_an_interactive_shell`
      each ⇒ `test result: ok. 1 passed`.
- [ ] `grep -rn 'mark_interactive\|\.interactive\b\|interactive: \(true\|false\)' src`
      ⇒ no output.
- [ ] `cargo test` ⇒ `passed=2356 failed=0` — exactly 2359 at `41d9bfed`
      minus the three deleted tests. **Not** 2359: a count that did not
      drop means a flag test survived.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`; clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02c.txt; : > "$A"
for t in run_terminal_command_refuses_an_interactive_command_and_writes_nothing the_read_transport_never_opens_an_interactive_shell shell_exec_non_interactive_still_runs_on_the_same_shell shell_for_owner_skips_a_busy_lowest_shell; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "flag_refs=$(grep -rn 'mark_interactive\|\.interactive\b\|interactive: \(true\|false\)' src | wc -l)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. If the full suite reports failures, re-run it at
most twice more, then paste the last capture as it is and name every
failing test.

Then the self-check. It takes the first fenced block after the entry's
heading, so the entry must hold only the output block — do not paste the
commands above into the entry:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02c-flag-gone.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02c.txt
diff /tmp/pasted-02c.txt /tmp/e2e-02c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Showing the foreground in `/shells` (the CLI listing) — a follow-on.
- `run_terminal_command(interactive: true)` and the ssh auto-upgrade —
  phase 03, which replaces the Task 4 refusal.
- `send_input` — phases 02d and 02e.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-14 (resume)

The first run did all the production work correctly and deleted the three
flag tests, then stalled reading test line ranges and git history. **Do
not read git history or old revisions.** Every remaining edit is below.

**Done — do not touch:** Tasks 1–4 in production code (`registry.rs`,
`ipc.rs`, `server/shells.rs` non-test part, `slash.rs` `render_shell_list`,
`session.rs` reuse predicates, `shell/mod.rs`, the `shell_exec.rs`
refusal) and Task 5's three deletions.

**Remaining, exactly:**

1. Rename `shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one`
   to `run_terminal_command_refuses_an_interactive_command_and_writes_nothing`
   and make it assert: `ssh nowhere.invalid` returns `Err` whose text names
   `send_input` and `open_shell`; a following `echo` on the same owner runs
   in the **same** shell id and its output contains the echoed text.
2. `shell_exec_non_interactive_still_runs_on_the_same_shell`: delete only
   the assertion that reads `.interactive`.
3. `the_read_transport_never_opens_an_interactive_shell`
   (`src/daemon/executor/file_ops/read.rs`): replace the loop that sets
   `record.interactive = true` with: `crate::shell::send_input` of
   `b"sleep 30\n"` to the held shell's socket, then poll
   `crate::shell::probe_foreground` until it is not `Foreground::Prompt`.
   Keep the rest of the test. Send `b"\x03"` to that socket at the end.
4. Every other site the compiler names: delete the `interactive: …` field
   or the assertion line that reads `.interactive`, nothing else. As of
   this note `cargo build --tests` names these lines:
   - `src/cli/commands/agents.rs:162`
   - `src/cli/commands/slash.rs:1030`
   - `src/cli/commands/slash.rs:969`
   - `src/daemon/executor/file_ops/read.rs:863`
   - `src/daemon/executor/knowledge/shell.rs:2558`
   - `src/daemon/executor/mod.rs:2155`
   - `src/daemon/executor/shell_exec.rs:1815`
   - `src/daemon/executor/shell_exec.rs:1869`
   - `src/daemon/prompt.rs:351`
   - `src/daemon/prompt.rs:477`
   - `src/daemon/server/shells.rs:1466`
   - `src/daemon/server/shells.rs:1483`
   - `src/daemon/server/shells.rs:1489`
   - `src/ipc_tests.rs:985`
   - `src/shell/foreground.rs:352`
   - `src/shell/foreground.rs:368`
   - `src/shell/foreground.rs:380`
   - `src/shell/session.rs:1136`
   - `src/shell/session.rs:1148`
   - `src/shell/session.rs:751`
   - `src/shell/session.rs:805`
   - `src/shell/session.rs:859`
   - `src/shell/session.rs:874`
   - `src/shell/session.rs:946`

Then run the § End-to-end verification block once. Finish: the block's
last line reads `passed=2356 failed=0`, and `flag_refs=0`.

### Update — 2026-10-14 (escalation)

**Chosen lever:** resume
**Rationale:** production code and the three deletions are on disk and correct; what remains is a fixed list of test edits the compiler enumerates, so a resumed run with that list keeps the work instead of redoing it.
