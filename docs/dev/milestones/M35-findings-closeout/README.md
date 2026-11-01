# M35 — Findings closeout

**Goal:** the findings ledgers have no open row. Every finding still
deferred from M29, M31 and M32 is fixed with a test, or closed with a
reason.

**Status:** done (closed 2026-11-01, released as 2.7.0)

**Release:** 2.7.0

**Depends on:** M33 (the tree as released in 2.6.0). M34 was withdrawn.

**Why now.** Fifteen findings were carried from milestone to milestone.
Most were waiting on M34's function splits, which the PE withdrew
(2026-10-31). None of them needs a split: each fix is local to the code
it names.

**PE decisions (2026-10-31):**

1. **#18, text typed during a turn, is queued.** It stays in the input box
   and is sent as the next message when the turn ends. Nothing typed is
   dropped, and the running turn is not interrupted.
2. **#3 and #9 are closed as rejected.** #3 would change a model-facing
   refusal string with no defect behind it; #9 is a question with no
   observed misbehaviour. The ledger says so.
3. **F8 (ghost foreground) is out of scope.** It is a feature and stays
   unscheduled. The M28 `wait: false` note stays closed by the earlier PE
   decision.
4. **#75, a session-less call's shells (2026-10-31, at phase-03 drafting):**
   `daemoneye ask` sends no session id, so refusing such calls would stop
   `ask` running commands. Asks keep sharing one owner, but a named one
   (`ask`) through one `chat_owner` helper, in place of nine copies of
   `unwrap_or_default()`. M32 #14 was found stale at the same drafting and
   closed as rejected.
5. **Numbering:** this is M35; MCP surface becomes M36 and Knowledge M37.
6. **GitHub issue #1 is in scope (2026-10-31, before the close).** The
   issue's headline — command events attributed to the wrong session — did
   not hold: one session issued the commands, and its turn was lost. The
   two defects behind it are fixed here: a command whose last line is a
   heredoc terminator hangs its shell (phase 08), and a foreground command
   past 120 s loses its whole turn to the client's silence timeout
   (phase 09). The issue stays open until both land and the live check
   passes.

**Rules every phase carries:**

- **A defect fix comes with a test that fails on the pre-phase tree.** The
  phase doc names the test and what it asserts; the executor pastes the red
  run.
- **A deduplication preserves behaviour** and holds the suite count exactly.
- **The phase marks its findings fixed** in the ledger row's triage cell
  (`fixed: M35 phase-NN, <test name>` or `fixed: M35 phase-NN, dedupe`).
- **The function cap is a ratchet** (PE, 2026-10-31): a fix that grows an
  allowlisted function raises its `tests/hygiene_allowlist.txt` line and says
  why in the Update Log. A new function over 150 lines gets a line with its
  reason. Nothing is squeezed to fit.
- **Every Spec step terminates:** named commands or files, and the output
  that ends the step.

**Exit criteria:**

- **Met.** No row in `docs/dev/milestones/M31-code-hygiene/FINDINGS.md` or
  `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` is triaged `defer`,
  and M29's O5 and O6 are marked fixed in its README.
- The four gates pass; `cargo test` passes at 2618 tests plus the new ones. **Met:** 2637.
- **Met 2026-10-31 and 2026-11-01** (see the Retrospective). Live check on pinky: a `wait: false` command's record carries the
  command's real exit code (O5); after `chat --resume` the status bar shows
  the resumed turn number and the session name stays visible under an
  approval panel (O6); text typed during a streaming turn is sent when the
  turn ends (#18); a `python3 - <<'EOF' … EOF` command completes with its
  exit code and its `command` event, and the next command in that shell runs
  (#1 bug 1); a foreground `sleep 150` with `timeout_secs: 200` completes
  with the chat still attached and the turn in the transcript (#1 bug 2).
- **Met 2026-11-01.** `2.7.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/FINDINGS.md` rows #36, #37, #40,
  #75, #78, #79.
- `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` rows #8, #13, #14,
  #18 (and #3, #9, closed here).
- `docs/dev/milestones/M29-post-2.0-fixes/README.md` § deferred, O5 and O6.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Expected count: 6, plus phase 07 from the live check and phases 08–09 from issue #1.

| #  | Phase | Findings | Status |
|----|-------|----------|--------|
| 01 | copy-path ([phase-01-copy-path.md](phase-01-copy-path.md)) — the remote copy builds its shell after approval, one dest-exists rule and message for both branches, a per-invocation tmp path | M31 #36, #37, #40 | done |
| 02 | host-prologue ([phase-02-host-prologue.md](phase-02-host-prologue.md)) — one shared host-parse + ghost-refusal prologue for `read_file`, `open_shell` and foreground; `read_file`'s connect failure sends no extra `ToolResult`; one id source for the connect-failure reply | M31 #78, #79; M32 #13 | done |
| 03 | small-tidies ([phase-03-small-tidies.md](phase-03-small-tidies.md)) — one `chat_owner` rule (session-less asks share the named owner `ask`), one output-header format; #14 closed stale at drafting | M31 #75; M32 #8 (#14 rejected) | done |
| 04 | run-records ([phase-04-run-records.md](phase-04-run-records.md)) — a `wait: false` command record carries the command's exit code, and a denial's `mode` matches the call | M29 O5 | done |
| 05 | status-bar ([phase-05-status-bar.md](phase-05-status-bar.md)) — the turn number after a resume, and the session name under an approval panel | M29 O6 | done |
| 06 | typed-input-queue ([phase-06-typed-input-queue.md](phase-06-typed-input-queue.md)) — text typed during a turn is kept and sent when the turn ends | M32 #18 | done |
| 07 | greeting-typeahead ([phase-07-greeting-typeahead.md](phase-07-greeting-typeahead.md)) — the greeting turn settles its typeahead like a typed turn (found by the live check) | M32 #18 (gap) | done |
| 08 | heredoc-wrap ([phase-08-heredoc-wrap.md](phase-08-heredoc-wrap.md)) — `wrap_command` runs the command in a brace group, so a heredoc terminator or a trailing comment on its last line no longer hangs the shell | issue #1 (bug 1) | done |
| 09 | long-foreground-turn ([phase-09-long-foreground-turn.md](phase-09-long-foreground-turn.md)) — keep-alives during the foreground wait; a client-gone tool call persists the turn with a result per call and records its outcome | issue #1 (bug 2) | done |

## Notes

- **Issue #1 investigated, 2026-10-31.** The daemon's own records
  (`var/log/runs/01d000e2…`, the `s10` recording, `daemon.log`) show one
  session, whose 21:54 turn ran the grep and the ping sweep and then
  vanished: the sweep's `python3 - <<'EOF' … EOF` hung `s10` at the `>`
  prompt (`wrap_command` appends `; printf …END…` to the terminator line),
  and 120.01 s after its approval the client's silence timeout dropped the
  connection, so the turn ended through `Err(_) => return Ok(())` with
  nothing persisted. The event log's `session` field was right throughout.
  Measured at drafting: moving the END printf to its own line leaks the
  shell's echo of that line into the captured output; a brace group does
  not — phase 08 pins the group. **Candidate, not scoped:** the socket
  client's command timeout (`client.rs`) bails without signalling the
  shell, so a timed-out command's shell stays busy and the next command
  queues behind it (`PtyShell::run` does send TERM; the client path does
  not).

- **Live check, 2026-10-31** (throwaway HOME `/tmp/de35-live`, private
  tmux, GLM at brain:8000):
  - **O5 passes.** A `wait: false` `sh -c 'sleep 1; exit 3'` wrote a launch
    record (`approved`, `exit: null`) and a completion record (`completed`,
    `exit: 3`).
  - **O6 passes.** The status bar read `session:9d46ede3…` under the approval
    panel, and `chat --resume` labelled its first typed turn `turn 6` after
    five turns.
  - **#18 passes on typed turns.** Text typed and entered mid-turn showed in
    the box under the spinner and was sent as the next turn, and the model
    answered `ACK`. Without Enter it pre-filled the input line.
  - **#18 fails on the greeting turn**, where it was first seen. Text typed
    during it is shown, then discarded: phase 06 gave the greeting a
    throwaway typeahead, which the spec allowed. Phase 07 fixes it; that
    step is re-checked live before the release.

- **Scoped 2026-10-31**, after M34 was withdrawn. O6 was reproduced in
  the M33 live check: the status bar read `session: ·` while an approval
  panel was up.

## Retrospective — 2026-11-01

**Outcome.** Nine phases, every one `approved_first_try`, no bug filed.
The M31 and M32 ledgers have no `defer` row (#3, #9 and #14 closed as
rejected with their reasons; the rest fixed with a test), and M29's O5 and
O6 are marked fixed. GitHub issue #1, filed by the SRE session on 2026-10-31,
was investigated before the close: its headline (events attributed across
sessions) was wrong, but two real defects sat behind it and phases 08–09
fixed both. The suite went from 2618 to 2637 tests. The allowlist lost four
lines (`aggregate_costs`, `cmd_session`, `activity_row_in`,
`search_memory_fts`, from the withdrawn M34's one phase) and three of its
remaining entries grew by a few lines under the ratchet rule
(`run_chat_ratatui` 500 → 526, `ask_with_session_ratatui` 673 → 678,
`run_copy_with_spawn` 213 → 214), each with its reason in the phase doc.

| Phase | Verdict | Turns | Note |
|---|---|---|---|
| 01 copy-path | approved_first_try | 142 | Both red runs showed the old behaviour; the temp name keeps the file name plus a UUID |
| 02 host-prologue | approved_first_try | 334 | The #79 test drives `execute_tool_call`, stronger than asked; `executor/mod.rs` 788 → 754 production lines |
| 03 small-tidies | approved_first_try | 114 | One `chat_owner` rule replaced nine `unwrap_or_default()` copies; #14 closed stale at drafting |
| 04 run-records | approved_first_try | 493 | Skipped the end-to-end Update Log entry; the spec marked it "required" from 05 on and it was never skipped again |
| 05 status-bar | approved_first_try | 185 | Both red runs real (`""` vs the session id, `0` vs `7`) |
| 06 typed-input-queue | approved_first_try | 361 | The PE's queue decision (#18); the greeting turn's throwaway typeahead was allowed by the spec and found live |
| 07 greeting-typeahead | approved_first_try | 136 | The criterion's grep counted the new test's three calls as well (5, not 2); the executor named the gap and did not bend the code |
| 08 heredoc-wrap | approved_first_try | 87 | Skipped the Findings entry; the brace group was pinned after a PTY experiment at drafting showed the bare-newline form leaks the shell's echo |
| 09 long-foreground-turn | approved_first_try | 380 | Suite pinned at 2636, landed at 2637: the executor split a separable case into its own test, rightly; `run_conversation_loop` shrank a line |

**Behaviour changes shipped.**
- A remote `copy` builds its shell after approval, one dest-exists rule and
  message for both branches, a per-invocation temp path (01).
- One host-parse + ghost-refusal prologue for `read_file`, `open_shell` and
  the foreground arm; `read_file`'s connect failure sends no extra
  `ToolResult` (02).
- Session-less asks share the named owner `ask`; one output-header format
  (03).
- A `wait: false` command's record carries the command's real exit code; a
  denial's `mode` matches the call (04, M29 O5).
- The status bar shows the resumed turn number and keeps the session name
  under an approval panel (05, M29 O6).
- Text typed during a streaming turn — the greeting included — is kept:
  Enter sends it as the next message, otherwise it pre-fills the line (06,
  07, M32 #18).
- A command whose last line is a heredoc terminator or a trailing comment
  completes with its exit code: `wrap_command` runs the command in a brace
  group (08, issue #1).
- A foreground command sends keep-alives while it runs, so a long command no
  longer trips the client's 120 s silence budget; and a tool call that fails
  with the client gone persists the turn with a result for every call and
  records why it ended, in place of dropping the turn silently (09, issue #1).

**What the milestone taught (architect side).**
1. *Three count criteria misfired, each on an artifact the phase itself
   created.* Phase 04's criterion counted a behaviour whose test the spec had
   not named; phase 07's `grep -c` counted the new test's own call sites;
   phase 09's pinned suite total missed a test the executor rightly split.
   Each time the executor reported the gap honestly and the reviewer
   re-derived the number. Three is a fix: **fold candidate A** — a phase doc
   pins the named tests and `failed=0`, never an exact suite total, and a
   call-site count stops at `#[cfg(test)]`. Needs the PE's sign-off before
   it lands in `WORKFLOW.md`.
2. *A Spec step that is not marked "required" gets skipped.* Phase 04 skipped
   the end-to-end entry, phase 08 the Findings entry. Marking the E2E step
   "required" from phase 05 on fixed that one for good. Two occurrences; held.
3. *Read the records before believing the report.* Issue #1's conclusion was
   wrong, and `var/log/runs/`, the shell recording and `daemon.log` settled it
   in minutes. The bug worth fixing was two layers under the one reported.
   One occurrence.
4. *Run the experiment the spec depends on.* The obvious fix for the heredoc
   (the END printf on its own line) would have leaked the shell's echo into
   every command's output; a ten-line PTY script at drafting showed it and the
   brace group was pinned instead. GLM landed it in 87 turns. One occurrence.
5. *rexyMCP:* executor reasoning (`</think>` spans) leaked into the
   server-authored completion summary twice (phases 03, 07). Upstream defect,
   not this project's.

**Live check — passed 2026-10-31 (O5, O6, #18 typed turns) and 2026-11-01
(#18 greeting, issue #1)** — throwaway HOME `/tmp/de35-live`, private tmux,
GLM at brain:8000, the 2026-11-01 run on the phase-09 tree (`8c0737b4`):
- `reply with exactly the word GREETACK` typed and entered while the
  greeting streamed was sent as turn 1 with no second Enter; the transcript
  holds it as the user message and `GREETACK` as the reply (#18, greeting).
- `python3 - <<'EOF'\nprint('DE35-HEREDOC-OK')\nEOF` ran as one call: the
  `command` event shows `exit: 0` 50 ms after the approval, and
  `echo DE35-AFTER-$((40+2))` then ran in the same shell `s0` (#1 bug 1).
- `sleep 150; echo DE35-LONG-DONE` with `timeout_secs: 200` ran 16:26:25 →
  16:28:55 UTC, `exit: 0`. At the 130 s mark the chat client was still
  attached and spinning; the turn, both tool calls and both results are in
  the transcript; the run log ends `Complete`; `daemon.log` has no
  disconnect line (#1 bug 2).
- A multi-line paste into the chat submits at its first newline and queues
  the rest as typeahead, which phase 06 then delivered as the next message.
  Expected behaviour, noted for the harness.

**Deferred (candidates, unscheduled).**
- The socket client's command timeout (`src/shell/client.rs`) bails without
  signalling the shell, so a timed-out command's shell stays busy and the
  next command queues behind it. `PtyShell::run` sends TERM; the client path
  does not.
- Keep-alives on the sudo arm of the foreground wait (its `tx` travels inside
  `SudoPromptArgs`).
- `end_client_gone` reports "disconnected" for any `execute_tool_call` error,
  keeps an unused `tx`, and carries a `debug_assert_eq!` on an invariant its
  helper guarantees.
- F8 (ghost foreground) stays unscheduled, as before.

