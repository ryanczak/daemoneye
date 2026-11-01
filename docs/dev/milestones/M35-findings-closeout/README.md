# M35 — Findings closeout

**Goal:** the findings ledgers have no open row. Every finding still
deferred from M29, M31 and M32 is fixed with a test, or closed with a
reason.

**Status:** in-progress (scoped 2026-10-31; phases 01–07 done, 08–09 added 2026-10-31 for issue #1)

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

- No row in `docs/dev/milestones/M31-code-hygiene/FINDINGS.md` or
  `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` is triaged `defer`,
  and M29's O5 and O6 are marked fixed in its README.
- The four gates pass; `cargo test` passes at 2618 tests plus the new ones.
- Live check on pinky: a `wait: false` command's record carries the
  command's real exit code (O5); after `chat --resume` the status bar shows
  the resumed turn number and the session name stays visible under an
  approval panel (O6); text typed during a streaming turn is sent when the
  turn ends (#18); a `python3 - <<'EOF' … EOF` command completes with its
  exit code and its `command` event, and the next command in that shell runs
  (#1 bug 1); a foreground `sleep 150` with `timeout_secs: 200` completes
  with the chat still attached and the turn in the transcript (#1 bug 2).
- `2.7.0` is released per `REXYMCP.md` § Versioning.

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
| 08 | heredoc-wrap ([phase-08-heredoc-wrap.md](phase-08-heredoc-wrap.md)) — `wrap_command` runs the command in a brace group, so a heredoc terminator or a trailing comment on its last line no longer hangs the shell | issue #1 (bug 1) | in-progress |
| 09 | long-foreground-turn ([phase-09-long-foreground-turn.md](phase-09-long-foreground-turn.md)) — keep-alives during the foreground wait; a client-gone tool call persists the turn with a result per call and records its outcome | issue #1 (bug 2) | todo |

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

