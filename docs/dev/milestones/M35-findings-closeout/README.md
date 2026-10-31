# M35 — Findings closeout

**Goal:** the findings ledgers have no open row. Every finding still
deferred from M29, M31 and M32 is fixed with a test, or closed with a
reason.

**Status:** todo (scoped 2026-10-31)

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
  turn ends (#18).
- `2.7.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/FINDINGS.md` rows #36, #37, #40,
  #75, #78, #79.
- `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` rows #8, #13, #14,
  #18 (and #3, #9, closed here).
- `docs/dev/milestones/M29-post-2.0-fixes/README.md` § deferred, O5 and O6.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Expected count: 6.

| #  | Phase | Findings | Status |
|----|-------|----------|--------|
| 01 | copy-path ([phase-01-copy-path.md](phase-01-copy-path.md)) — the remote copy builds its shell after approval, one dest-exists rule and message for both branches, a per-invocation tmp path | M31 #36, #37, #40 | done |
| 02 | host-prologue ([phase-02-host-prologue.md](phase-02-host-prologue.md)) — one shared host-parse + ghost-refusal prologue for `read_file`, `open_shell` and foreground; `read_file`'s connect failure sends no extra `ToolResult`; one id source for the connect-failure reply | M31 #78, #79; M32 #13 | done |
| 03 | small-tidies ([phase-03-small-tidies.md](phase-03-small-tidies.md)) — one `chat_owner` rule (session-less asks share the named owner `ask`), one output-header format; #14 closed stale at drafting | M31 #75; M32 #8 (#14 rejected) | done |
| 04 | run-records ([phase-04-run-records.md](phase-04-run-records.md)) — a `wait: false` command record carries the command's exit code, and a denial's `mode` matches the call | M29 O5 | todo |
| 05 | status-bar — the turn number after a resume, and the session name under an approval panel | M29 O6 | todo |
| 06 | typed-input-queue — text typed during a turn is kept and sent when the turn ends | M32 #18 | todo |

## Notes

- **Scoped 2026-10-31**, after M34 was withdrawn. O6 was reproduced in
  the M33 live check: the status bar read `session: ·` while an approval
  panel was up.

