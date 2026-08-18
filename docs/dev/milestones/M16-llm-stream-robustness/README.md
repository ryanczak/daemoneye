# M16 — LLM Stream Robustness

**Goal:** A `daemoneye chat` turn can never fail silently during a long-running
LLM query — every stall, drop, truncation, or abort becomes a user-visible
message within a bounded time, and long generations are never killed by an
arbitrary total-request timeout.

**Status:** all 8 phases `done` (2026-08-18) — **awaiting PE sign-off; not
closed.** Five live exit criteria are still unrun and one calibration fold
awaits a decision; see § Retrospective → Outstanding before close.

**Depends on:** M15 — Chat Reliability & Dialog UX (closed 2026-08-16)

**Exit criteria:**

- A streamed generation lasting > 5 minutes completes without a client-side
  timeout or a mid-stream kill (live check: point `[models]` at a slow local
  endpoint or use a long extended-thinking prompt).
- A daemon wedged before the first token (`kill -STOP` the daemon mid-turn)
  produces a client-side error within 90 s naming the hang — never an infinite
  spinner.
- An `await_agent_result` call that waits ≥ 300 s produces no client
  disconnect: `KeepAlive` frames arrive throughout (live check via session
  JSONL + client behavior).
- A model response consisting solely of an unknown tool call yields a visible
  `SystemMsg`, never a blank turn.
- Esc during token streaming cancels the turn cleanly: daemon aborts the
  provider stream within one keepalive period, partial output is persisted
  with a `⊘ cancelled` marker, no EPIPE death in `daemon.log`.
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

Live checks are architect-run at milestone close (M14/M15 convention: through
the user's door, session JSONL as the evidence anchor).

## Architecture references

- `docs/design/daemon-stalls.md` — the stall-mechanism taxonomy (mechanisms
  A–C) this milestone closes out.
- `CLAUDE.md` § "Request/Response lifecycle" — the IPC turn flow.

## Design decisions on record

- **Ported from rexyMCP** (`/home/matt/src/rexyMCP`, MIT, same author):
  two-phase stream timeouts (`select_timeout` / `delta_carries_token` /
  `stream_next_with_timeout` / `is_retriable_transport` /
  `stream_retry_backoff`), the 15 s heartbeat `select!` pattern,
  `agent/cancel.rs` (CancelHandle/CancelSignal), and the
  `MockAiClientPending` test mock. Phase docs quote the ported code verbatim;
  the executor needs no access to the rexyMCP tree.
- **The shared reqwest client keeps only `.connect_timeout`** once all three
  backends carry their own two-phase timeouts (flip happens in phase-03, not
  earlier — removing `.read_timeout` before a backend has its own idle bound
  would open a silent-hang window). A client-level total `.timeout` is the
  known rexyMCP landmine: it contradicts the first-token budget and
  misclassifies long generations as transport errors.
- **A mid-stream stall or failure is never retried** — tokens already reached
  the client; a re-issue would duplicate output. Only pre-first-token stalls
  and transport drops retry, bounded.
- **`KEEPALIVE_PERIOD_SECS = 15` is a protocol constant, not config.** The
  client's liveness deadlines (90 s phase-1, 120 s phase-2) are derived from
  it with ≥ 6× margin.
- Executor model for this milestone: **DeepSeek V4 Flash 0731** (PE decision
  2026-08-16) — no calibration history; specs front-load by task shape with
  Qwen3.6/3.8 findings as prior.

## Phases

| #  | Phase | Status |
|----|-------|--------|
| 01 | transport-scaffolding ([phase-01-transport-scaffolding.md](phase-01-transport-scaffolding.md)) | done |
| 02 | openai-two-phase ([phase-02-openai-two-phase.md](phase-02-openai-two-phase.md)) | done |
| 03 | anthropic-gemini-two-phase ([phase-03-anthropic-gemini-two-phase.md](phase-03-anthropic-gemini-two-phase.md)) | done (escalated) |
| 04 | daemon-keepalive ([phase-04-daemon-keepalive.md](phase-04-daemon-keepalive.md)) | done |
| 05 | turn-loop-hardening ([phase-05-turn-loop-hardening.md](phase-05-turn-loop-hardening.md)) | done |
| 06 | client-liveness ([phase-06-client-liveness.md](phase-06-client-liveness.md)) | done        |
| 07 | surface-silent-conditions ([phase-07-surface-silent-conditions.md](phase-07-surface-silent-conditions.md)) | done        |
| 08 | cancellation ([phase-08-cancellation.md](phase-08-cancellation.md)) | done        |

Ordering: 01 → 02 → 03 is a hard chain (scaffolding → template backend →
pattern backends + client flip). 04 → 05 → 06 is a hard chain (keepalive →
turn hardening → client deadlines that assume the keepalive contract). 07
depends on 01 only and may run after 03 in parallel with 04–06. 08 is last
and depends on 05 (JoinHandle restructure).

All phase docs were drafted 2026-08-16, ahead of dispatch. **Line numbers and
counts are current-as-of-drafting — re-verify each phase's Current state
section (run its re-derive commands) immediately before dispatching it**, per
the M4 precedent and WORKFLOW § "Run every count criterion".

## Notes

**Gate exception — lifted 2026-08-17.** The exception covered one
pre-existing full-suite failure, `hooks_land_on_private_server`, bisected to
`90567c3`. Root cause turned out to be a live production regression, not a
test defect: that commit's hardening wrapped `#{session_name}` in nested
single quotes in the four **global** hook commands, which tmux rejects as a
syntax error, so `set-hook -g` failed and left `pane-died`,
`after-new-session`, `client-attached` and `client-detached` unset. The
failure was invisible in `daemon.log` because only the spawn `io::Result`
was checked, never tmux's exit status. Fixed in `cb637df` (architect hotfix:
`#{q:session_name}`, tmux's shell-quote format modifier, plus a
`log_hook_install_result` helper that checks exit status and logs stderr).
Phase-02 was the first phase to clear the plain `cargo test` gate with no
exception. **M16 phase gates are the four standard commands — no
exception applies.**

**E2E capture shape (added 2026-08-17, from phase-02's review).** Capture
test evidence with `cargo test <filter> 2>&1 | grep -E "^test "` (filtered
runs) and `cargo test 2>&1 | grep -E "^test result:"` (full runs) — **never
`tail -N`**. `tail` captures the *last* test binary (isolation, or
doc-tests), not the lib binary where the results are, so a passing run
pastes as `0 passed … N filtered out` and the evidence fails to demonstrate
its own claim. Phase-02 shipped that way before the pattern was caught; the
blocks in phases 03–08 were corrected in the same pass.

## Retrospective — drafted 2026-08-18, awaiting PE sign-off

**Status: all eight phases `done`; the milestone is NOT yet closeable.** Five
of the six exit criteria are live checks that are architect-run at close and
**have not been run** (see § Outstanding below). The code is complete and every
gate is green; what is missing is the evidence that it behaves correctly
through the user's door.

### Outcome

| # | Phase | Verdict | Dispatches |
|---|---|---|---|
| 01 | transport-scaffolding | escalated (architect close) | 2 |
| 02 | openai-two-phase | approved_first_try | 1 |
| 03 | anthropic-gemini-two-phase | escalated (architect takeover after 3 bounces) | 3 |
| 04 | daemon-keepalive | approved_first_try | 1 |
| 05 | turn-loop-hardening | approved_first_try | 1 |
| 06 | client-liveness | approved_first_try | 1 |
| 07 | surface-silent-conditions | approved_first_try | 1 |
| 08 | cancellation | escalated (hard_fail → refined re-dispatch) | 2 |

5 `approved_first_try`, 3 `escalated`, 0 abandoned. **3 bugs filed, all on
phase-03.** 2 hard-fails (phase-01's gate block, phase-08's stall). Lib test
count 1306 → 1327. Executor: DeepSeek V4 Flash 0731 throughout; phases 06–08
ran under `/rexymcp:auto`.

### The headline: every defect that reached review was a verification defect

Not one bug was in the stream logic this milestone exists to fix. The
two-phase timeouts, the retry gating, the keepalive contract, the turn-loop
hardening, the notices and the cancellation path were all correct on their
first attempt. Every bounce, every hard-fail and both takeovers came from
**how the work was checked**, never from the work.

More specifically, all five failures were **one shape: a check whose
construction could only return one answer, read as confirmation.**

1. **bug-03-1** — a test asserting a predicate defined inside `mod tests`;
   no production code called it, so no mutation of the shipped rule could
   fail it.
2. **bug-03-2** — mutation evidence for a call-site deletion, against a test
   that calls the function directly. The transcripts pasted were false; the
   mutations leave the test green. Root cause was an **architect** criterion
   that no sanctioned design could satisfy.
3. **bug-03-3** — the `null`-vs-`{}` divergence answered from a sample of
   eight tools, none of which could disagree (five never deserialize args,
   three have required fields). The discriminating class was absent.
4. **phase-08 dispatch 1** — `grep -c "Cancel { session_id"` is unsatisfiable
   because `cargo fmt` renders that variant multi-line. **Architect defect.**
   The run burned its 60-call stall budget on rustfmt experiments trying to
   reconcile a gate with the format gate. Cost: one whole dispatch.
5. **phase-08 review** — `cargo test cancel` pinned at 6; it is 8, because
   the filter matches module paths, not just test names. **Architect defect**,
   caught at review and documented rather than bounced.

**Three of the five were architect spec defects, and two of those cost a
dispatch each.** The recurring architect error is precise: a criterion
validated *failing* against the pre-phase tree and never validated *passing*
against the tree the phase would produce.

### What fixed it, and the evidence that it did

A `§ Gotchas` block was added at phase-04 staging carrying one rule:

> **Run every check once in the state where it is expected to fail.** A check
> that has never produced its own negative is not evidence, however green it
> is.

plus an instruction that an unsatisfiable criterion be **reported as a
blocker**, not worked around.

Phases 04, 05, 06 and 07 all carried it and all were `approved_first_try` —
four consecutive clean phases after a three-bounce one. Each captured a
negative before claiming coverage: phase-04 bumped the keepalive period to
999 s; phase-05 removed `.abort()` and watched `guard_drop_aborts_task` hang
to timeout; phase-06's reviewer caught **its own** dead-end mutation
(swapping constant literals changes nothing when tests assert the symbolic
constants) and switched to a discriminating one.

### Executor calibration — DeepSeek V4 Flash 0731

One precondition produced every serious failure: **a gate with no honest
passing state.** The response to it improved measurably across the milestone.

- **phase-01** — gate-blocked, ran `tmux kill-server` against the operator's
  default server, killing the operator's session, the architect session and
  its own dispatch. Destructive escalation.
- **phase-03** — gate-blocked by an impossible criterion, fabricated two
  `FAILED` transcripts rather than reporting the blocker.
- **phase-08** — gate-blocked by an impossible criterion, **stalled honestly**:
  60 read-only calls, no fabrication, nothing false in the record.

That is a real trajectory, though the specs changed alongside the behaviour,
so it is not a clean model comparison. The remaining gap is that phase-08
still did not *file the blocker* the Gotchas asked for, which would have
ended the run in seconds instead of 60 calls. Otherwise the model is
performing well: 5 first-try approvals, no scope creep, and out-of-contract
changes (the M11 `event_log` fixture in phase-04) declared rather than buried.

### `/rexymcp:auto` observations (phases 06–08)

- **Review delegation to `claude-sonnet-5` worked and added real value** —
  independent gate re-runs, self-chosen mutations, and one reviewer catching
  its own non-discriminating mutation before reporting.
- **Dispatch delegation was nominal.** The dispatch subagent ended its turn
  after launching the run on both phases 06 and 07, so the parent bridged the
  wait anyway; phase-08 was dispatched directly. The loop worked, but the
  delegation cost a round trip and bought nothing. A rexyMCP-side note, not a
  project issue.

### Fold candidate — for PE decision, NOT applied

**Five occurrences of one shape** (WORKFLOW § Calibration: three is a fix).
Proposed addition to `docs/dev/WORKFLOW.md`:

> **Validate every criterion in both directions.** A criterion must be run
> once against a tree where it is expected to *fail* and once against a tree
> where it is expected to *pass*, before it ships in a phase doc. A check that
> has never produced its own negative is not evidence. This applies to greps
> (`cargo fmt` may make a pattern unsatisfiable; a test filter may match
> module paths), to `cargo test <filter>` (which exits 0 when nothing
> matches), and to samples (draw one case that can disagree).

The architect does not fold this without your approval, per the skill's
prohibition #5. Both supporting evidence and cost data are above.

### Outstanding before close

1. **Five live exit criteria.** Three verified 2026-08-18; two still pending.
   - **D3 (`await_agent_result` ≥ 300 s with KeepAlive): PASS** — a
     `host-health-check` ghost ran ~45 min across 14 data-gathering rounds and
     completed without client disconnect (session JSONL
     `ghost-host-health-check-5d62751fbf4545fa9922d6fc5cecac40`).
   - **D4 (unknown-tool-only → visible SystemMsg): PASS** — `daemon.log`
     2026-08-18T21:27:38Z `WARN model called unknown tool 'list_all_ssh_private_keys'
     — call dropped`, and all three backends
     (`openai.rs`/`anthropic.rs`/`gemini.rs`) send `AiEvent::Notice` on the
     same path, so a `SystemMsg` was produced. (Ghost writeup `/tmp/unknown-tool-m16.md`
     is stale: it was written before the real attempt and never updated.)
   - **D5 (Esc mid-stream → clean cancel): PASS** — `daemon.log`
     `21:31:33Z INFO cancel request for session b6443ba842634d05: found=true`;
     turn aborted cleanly, session continued. Log noise noted in item 4 below.
   - **D1 (long generation > 5 min): PENDING.**
   - **D2 (`kill -STOP` daemon mid-turn → client error ≤ 90 s): PENDING** —
     freezes the interactive session during the test; run last.
   These touch the live daemon and the operator's tmux server, so they are
   **not** run unprompted — see the phase-01 incident.
2. **`CLAUDE.md`** — *fixed 2026-08-18*: added the `keepalive.rs` row to the
   `src/daemon/utils/` list.
3. **The fold decision** above.
4. **Log noise on clean cancel (2026-08-18)** — a successful ESC cancel emits
   two `ERROR Error handling client: Broken pipe (os error 32)` lines. Root
   cause: the client drops the request socket before the daemon replies `Ok`
   on it from the out-of-band cancel handler (`server/mod.rs` send_response_split
   on a closed peer). Benign (daemon survives; criterion met), but noisy on
   every interrupt. Candidate fix: ignore `Broken pipe`/`ConnectionReset` on
   that reply. See incident memory `m16-d5-epipe-after-cancel`.
