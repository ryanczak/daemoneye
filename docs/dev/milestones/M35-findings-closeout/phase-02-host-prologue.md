# Phase 02 — host-prologue

**Status:** todo
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 01 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

Three executor findings are closed (M31 FINDINGS #78, #79; M32 FINDINGS #13):

- **#78, a dedupe.** The `read_file`, `open_shell` and foreground
  `run_terminal_command` arms each repeat the same pair:
  `policy.may_use_host` → `ghost_host_refusal(...)` →
  `log_event("ghost_host_refused", {session, tool, requested_host,
  allowed_hosts})`, differing only in `"tool"`. `read_file` and `open_shell`
  also repeat the host parse (`None` → `HostId::Local`, `Some(name)` →
  `HostId::parse(name)`) that `resolve_foreground_host` already does. After
  this phase one helper does the refusal and its event, and the parse is
  `resolve_foreground_host`'s, renamed `resolve_host`.
- **#79, a defect.** On a cold-host connect failure, `read_file`
  (`src/daemon/executor/dispatch_files.rs`) writes an explicit
  `Response::ToolResult` frame before returning. `read_file` is a silent
  tool: it reports through `ToolStarted`/`ToolFinished`, as `open_shell`'s
  identical failure does, so the client gets an extra frame. After this phase
  the `read_file` arm writes no `ToolResult` frame on that path. The returned
  outcome text (`Error: <msg>`) and the `ssh_connect_failed` event are
  unchanged.
- **#13, closed by #79.** Once `read_file` writes no frame, only the
  approval-gated `edit_file` arm builds a `ToolResult` reply, which is right
  for an approval-gated tool. Nothing further to change.

The suite goes from **2620 to exactly 2621**.

## Constraints

- **`src/daemon/executor/mod.rs` is at 788 of 800 production lines.** The new
  helper must not go there. Create `src/daemon/executor/host_gate.rs`, a child
  module of `mod.rs` like `dispatch_files.rs`, and move `ghost_host_refusal`
  and `resolve_foreground_host` into it along with the new helper. `mod.rs`
  shrinks.
- **The refusal text and the event fields do not change.** Same sentence,
  same four JSON fields, same `"tool"` value per arm (`read_file`,
  `open_shell`, `run_terminal_command`).
- **Each arm keeps its own frames.** The foreground arm is approval-gated and
  still sends its `ToolResult` after a refusal; `read_file` and `open_shell`
  send none. The helper returns the message and writes no frame.
- **`open_shell` still parses only under a ghost policy.** A chat
  `open_shell` with a malformed host must keep reaching `open_shell`'s own
  error wording.
- The other `ghost_host_refused` writers (`foreground.rs`,
  `knowledge/ghost.rs`) are out of scope: list them in Findings, do not
  change them.
- **Use `patch` with whole blocks,** never `patch_lines`. If a fix grows an
  allowlisted function, raise its `tests/hygiene_allowlist.txt` line and say
  why in the Update Log (M35 README).

## Spec

1. **Create `host_gate.rs`** — move `ghost_host_refusal` and
   `resolve_foreground_host` (renamed `resolve_host`; update its callers)
   from `mod.rs` into it, and add
   `refuse_ghost_host(policy: &GhostPolicy, host: &HostId, tool: &str,
   session_id: Option<&str>) -> Option<String>`: `None` when the policy allows
   the host, otherwise it writes the `ghost_host_refused` event and returns
   the refusal sentence.
2. **Use it in the three arms** — `dispatch_files.rs` (`ReadFile`),
   `dispatch_open_shell.rs` (`OpenShell`) and `dispatch_foreground.rs`; the
   `ReadFile` and `OpenShell` parses call `resolve_host`. Run `cargo test`:
   still exactly 2620 passed.
3. **Write the #79 test** — `read_file_connect_failure_writes_no_tool_result_frame`
   in the `tests` module of `dispatch_files.rs`: drive `dispatch` with a
   `PendingCall::ReadFile` whose host is `de-test-unreachable.invalid`,
   `is_ghost = true` (BatchMode, so ssh fails fast instead of prompting), a
   `Vec<u8>` as `tx` and an empty reader as `rx`, under the test HOME guard
   the executor tests use (see `shell_exec_connect_tests.rs`). Assert the
   outcome starts `Error:` and that nothing written to `tx` is a `ToolResult`
   frame. Run it now and keep the failing output.
4. **Fix #79** — drop the `send_response_split(... ToolResult ...)` from the
   `ReadFile` connect-failure branch; the step 3 test passes.
5. **CLAUDE.md** — add a § Key files row for `host_gate.rs`, in the style of
   the `dispatch_*` rows.
6. **Mark the ledgers** — replace the triage prefix `act: M35 phase-02` with
   `fixed: M35 phase-02, <what>` in M31 rows #78 (`dedupe, host_gate.rs`) and
   #79 (`read_file_connect_failure_writes_no_tool_result_frame`), and M32 row
   #13 (`closed by #79`). Keep the `(was: …)` text; touch no other row.
7. **Red run** — one `### Update — <date> (red run)` entry pasting step 3's
   `test result:` line and assertion message.
8. **Findings** — one `### Findings — <date>` entry from exactly
   `grep -rn '"ghost_host_refused"' src --include=*.rs | grep -v _tests`:
   list the writers left outside `host_gate.rs`, one line each, or `none`.
9. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry.
10. **Commit** as `fix: one ghost host gate; read_file sends no extra ToolResult (M31 #78 #79, M32 #13)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `arm_pairs=0 gate=1 rf_frames=1 new_test=1 fixed=3 key_file=1`
      (today `arm_pairs=3 gate=0 rf_frames=2 new_test=0 fixed=0 key_file=0`).
- [ ] The red-run entry shows the new test failing before the fix.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2621 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
d=src/daemon/executor
echo "arm_pairs=$(cat $d/dispatch_files.rs $d/dispatch_open_shell.rs $d/dispatch_foreground.rs | grep -c '"ghost_host_refused"') gate=$(grep -c 'fn refuse_ghost_host' $d/host_gate.rs 2>/dev/null || echo 0) rf_frames=$(grep -c 'Response::ToolResult' $d/dispatch_files.rs) new_test=$(grep -c 'fn read_file_connect_failure_writes_no_tool_result_frame' $d/dispatch_files.rs) fixed=$(( $(grep -cE '^\| (78|79) \|.*fixed: M35 phase-02' docs/dev/milestones/M31-code-hygiene/FINDINGS.md) + $(grep -cE '^\| 13 \|.*fixed: M35 phase-02' docs/dev/milestones/M32-findings-fixes/FINDINGS.md) )) key_file=$(grep -c 'executor/host_gate.rs' CLAUDE.md)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Create `src/daemon/executor/host_gate.rs`; edit `executor/mod.rs`,
  `dispatch_files.rs`, `dispatch_open_shell.rs`, `dispatch_foreground.rs`,
  any caller of the renamed `resolve_host`, `CLAUDE.md` (one row), the three
  ledger rows, and `tests/hygiene_allowlist.txt` (only per the rule above).

## Out of scope

- The `ghost_host_refused` writers in `foreground.rs` and
  `knowledge/ghost.rs`; any refusal wording; `edit_file`'s frames.

## Update Log

<!-- entries appended below this line -->
