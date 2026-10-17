# Phase 03d: launch — an interactive command starts in its shell and returns the screen

**Milestone:** M28 — Interactive shells
**Status:** in-progress
**Depends on:** phase-03c
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Since phase 02c, `run_terminal_command("ssh host")` is refused: an
interactive command never exits, so the marker protocol would wait for
ever. The tools to drive such a session now exist (`send_input` with keys,
`read_shell_screen` with waits, the busy-shell guard), so the refusal can
become a **launch**:

1. write the bare command and Enter into the shell, with no markers;
2. wait for the screen to settle;
3. return what holds the shell and the screen, and wait for nothing more.

The shell stays busy while the session runs, so later commands go to
another shell. No tool parameter changes here; `interactive: true` for any
command is phase 03e.

## Architecture references

- `docs/design/interactive-shells.md` D6: the bare command plus `\r`, no
  markers, settle, then the `Foreground` plus the masked screen. Never wait
  for an exit, never signal the foreground group. A command
  `is_interactive_command` recognises is upgraded to this path.

## Spec

**Files:** `src/daemon/executor/shell_exec.rs` and
`src/daemon/executor/knowledge/shell.rs` (visibility only). Do not change
`is_interactive_command`, the `wait: false` path (it still refuses an
interactive command) or any tool definition.

1. **`launch_interactive`** — a new
   `async fn launch_interactive(reg, id: &ShellId, sock: &Path, cmd: &str) -> anyhow::Result<ShellRunResult>`
   in `shell_exec.rs`. In order:
   1. send `cmd` followed by `\r` with `crate::shell::send_input`, with no
      marker wrapping;
   2. call `knowledge::read_shell_screen_in(reg, id, None, None, None, false, None, Some(500), Some(5), <the standard screen timeout>)`.
      That waits up to 5 s for the screen to settle, records the shell's
      `last_foreground`, and returns the masked screen with its foreground
      header. Widen `read_shell_screen_in` to `pub(crate)` (and re-export it
      from `knowledge`) for this. Do not copy its logic;
   3. return `ShellRunResult { text, exit_code: 0 }`, where `text` is one
      header line followed by that screen read. The header line:
      `` [Interactive session started] `<cmd>` holds shell <id>; nothing waits for it. Type into it with send_input(shell: "<id>") and read it with read_shell_screen(shell: "<id>"). Later commands run in another shell while it is busy. ``

   The text names no exit code. `exit_code: 0` exists only for the stats
   and the shell-command log, which have no field for "did not exit".
2. **The branch.** In `run_in_shell_core` — the branch is the one
   `if crate::daemon::utils::is_interactive_command(cmd) {` in that
   function, and the resolved shell is bound above it by
   `let (id, socket) = match target {`, so pass `&id` and `&socket` — the
   `is_interactive_command(cmd)` branch calls `launch_interactive` with the
   resolved `id` and socket, instead of `bail!`ing. It stays where it is,
   after target resolution and the named-shell busy probe, so a named busy
   shell is still refused before anything is written. The comment above
   the branch says that the command is now launched, not refused.
3. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03d.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Gotchas.**
- These two 02c tests change, and that list is complete:
  - `shell_exec_interactive_returns_without_running_the_command` is
    **renamed** `run_terminal_command_launches_an_interactive_command`.
    `ssh nowhere.invalid` now returns `Ok` within 10 s, and its text
    contains `[Interactive session started]` and the shell id. The ssh
    really runs now and fails to resolve the host; the screen shows that
    honestly, and the test must not assert what ssh printed.
  - `run_terminal_command_refuses_an_interactive_command_and_writes_nothing`
    is **renamed** `an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another`.
    Call `launch_interactive` directly with `sleep 30` on the owner's shell.
    The owner's next `echo ALIVE` then runs in a **different** shell id,
    and its output contains `ALIVE`. Send `\x03` to the first shell at the
    end. Use `sleep 30` and not ssh, because ssh to an unresolvable host
    exits at once and leaves the shell idle.
- **How a test gets a shell to launch into** (this is what the first run
  could not find). In the `shell_exec.rs` test module:
  1. `let (_home, _dir) = test_home(); let reg = ShellRegistry::default();`
  2. `let (id, _) = run_in_shell_for_test(&reg, owner.clone(), "true", 5, None, None, spawn_in_process_host).await?;`
     — this creates the owner's shell and returns its id;
  3. `launch_interactive(&reg, &id, &socket_path(id.as_str()), "sleep 30").await`;
  4. for the next-command test, `run_in_shell_for_test(&reg, owner, "echo ALIVE", 5, None, None, spawn_in_process_host)`
     returns `(id2, result)` with `id2 != id`;
  5. clean up with `crate::shell::send_input(&socket_path(id.as_str()), b"\x03", Duration::from_secs(5))`.
  `run_in_shell_for_test` is the helper at the top of the test module;
  you do not need to read the helpers below it.

## Tests

- `launch_interactive_returns_the_settled_screen` (`shell_exec.rs` tests):
  `launch_interactive` with `sleep 30` on an in-process shell ⇒ `Ok`; the
  text starts with `[Interactive session started]`, names the shell id, and
  its screen part contains `sleep 30`. Afterwards the record's
  `last_foreground` is `Program { cmd: "sleep", .. }`. Send `\x03` at the
  end.
- The two renames in § Gotchas.

## Acceptance criteria

- [ ] The new test and the two renamed ones ⇒ `test result: ok. 1 passed`
      each.
- [ ] `grep -n 'cannot hold' src/daemon/executor/shell_exec.rs | wc -l` ⇒
      `0` (the refusal text is gone).
- [ ] `cargo test` ⇒ `passed=2391 failed=0`: exactly 2390 at `07558aa7`
      plus the 1 new test. The renames add nothing.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03d.txt; : > "$A"
for t in launch_interactive_returns_the_settled_screen run_terminal_command_launches_an_interactive_command an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another shell_exec_non_interactive_still_runs_on_the_same_shell; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "refusal_text=$(grep -n 'cannot hold' src/daemon/executor/shell_exec.rs | wc -l)" >> "$A"
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
D=docs/dev/milestones/M28-interactive-shells/phase-03d-launch.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03d.txt
diff /tmp/pasted-03d.txt /tmp/e2e-03d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- `run_terminal_command(interactive: true)` for any command, its approval
  panel line, and the `wait: false` + `interactive` refusal: phase 03e.
- An interactive command run inside a named-host (transport) shell. The
  local terminal cannot see the remote foreground (design § 9). Named-host
  shells keep today's transport classification.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-17 (refined re-dispatch)

The first run wrote no code: it hard-failed at turn 147 on a
`NoProgressStall` after reading `shell_exec.rs` in full and then **one or two
lines at a time** through lines 2040–2154 (the test helpers), looking for how
a test obtains a shell to call `launch_interactive` on. That recipe is now in
§ Gotchas, and the production edit's two anchors (the `is_interactive_command`
branch and the `let (id, socket) = match target {` binding) are named in
Spec Task 2. **Do not read files a line at a time**: find an anchor with one
`grep -n`, then read one 60–120-line window around it. Write
`launch_interactive` first, then the branch, then the three tests. Record the
executor as the model that actually runs (the first run's log entry named a
different model and has been removed).


### Update — 2026-10-17 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** first failure, wrote no code, and the stall traces to a spec gap (no recipe for a test to obtain the shell `launch_interactive` needs, and no anchor for the edit in a 3,700-line file), which a re-dispatch with those named fixes; there is no partial work for a resume to keep.
