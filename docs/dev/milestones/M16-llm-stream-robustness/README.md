# M16 — LLM Stream Robustness

**Goal:** A `daemoneye chat` turn can never fail silently during a long-running
LLM query — every stall, drop, truncation, or abort becomes a user-visible
message within a bounded time, and long generations are never killed by an
arbitrary total-request timeout.

**Status:** planning

**Depends on:** M15 — Chat Reliability & Dialog UX (must close first)

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
| 05 | turn-loop-hardening ([phase-05-turn-loop-hardening.md](phase-05-turn-loop-hardening.md)) | in-progress |
| 06 | client-liveness ([phase-06-client-liveness.md](phase-06-client-liveness.md)) | todo |
| 07 | surface-silent-conditions ([phase-07-surface-silent-conditions.md](phase-07-surface-silent-conditions.md)) | todo |
| 08 | cancellation ([phase-08-cancellation.md](phase-08-cancellation.md)) | todo |

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

(retrospective at close)
