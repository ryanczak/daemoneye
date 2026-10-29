# Phase 02 — The `ai_turn` event names the model that answered

**Status:** in-progress
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-28
**Finding:** M31 `FINDINGS.md` #85

## Goal

`run_conversation_loop` (`src/daemon/stream/conversation.rs`) records
each model call's cost in two near-identical blocks inside the
`AiEvent::Done` arm: one on the final-answer path, one on the tool-call
path. Each block does four things:
- `crate::cost::emit_cost`;
- `run_log.metrics_mut().record_model_call`;
- adds the cost to the session entry;
- `log_event("ai_turn", …)`.

The two `ai_turn` events disagree. The final-answer path logs
`config.resolve_model(session_active_model.as_deref()).model`, but the
tool-call path logs `config.resolve_model(None).model`, the configured
default. A session that switched model with `/model` therefore records
the wrong model in `events.jsonl` on every tool-call turn, while its cost
lines name the right one.

After this phase, both paths call one helper, and the helper logs the
same model entry that the cost record uses.

## Architecture references

- M32 README, decisions 1 and 6.
- `CLAUDE.md` § "Important Invariants": every `events.jsonl` record
  carries `ts`, `event` and `pid`, and `log_event` stamps `pid` itself.

## Spec

1. **The helper, tested first.** In `src/daemon/stream.rs`, the parent
   module, add
   `fn record_model_call(config: &Config, session_active_model: Option<&str>, usage: &TokenBreakdown, attribution: &CostAttribution, session_id: Option<&str>, run_log: &mut RunLogGuard, sessions: &SessionStore)`.
   Its body is the four steps above, in the same order, taken from the
   final-answer block. The `ai_turn` event's `model` comes from the
   same `resolve_model(session_active_model)` entry that `emit_cost`
   is given. The event keeps its current fields and names. Write the
   two tests below in `stream.rs`'s test module and run them.
2. **Use it on both paths.** In `conversation.rs`, replace each of the
   two blocks with one call. Each block runs from
   `let active_entry = config.resolve_model(…)` to the end of its
   `log_event("ai_turn", …)` call. The surrounding code stays as it is:
   the message push and the session write-back after each block are
   different on the two paths and do not move.
3. **Mutation check.** Change the helper's event to use
   `resolve_model(None)`, run
   `record_model_call_logs_the_active_model`, and paste the failing
   output. Then revert the change.
4. **Findings** go in their own `### Findings — <date>` entry, from
   `stream.rs` and the two edited sites in `conversation.rs` only,
   within five tool calls, or the single line `none`.
5. **Commit** as `fix:`.

**Headroom.** `stream.rs` has 168 production lines, so there is room.
`conversation.rs` and `run_conversation_loop` only shrink here, by
about 60 lines. `run_conversation_loop` is allowlisted at 775: leave
that line as it is. Add no `#[allow(…)]`, and keep the helper at seven
parameters, which is clippy's limit.

## Acceptance criteria

- [x] `grep -c 'resolve_model(None)' src/daemon/stream/conversation.rs`
      prints `0` (today: `1`).
- [x] `grep -c '"ai_turn"' src/daemon/stream/conversation.rs` prints `0`
      and `grep -c '"ai_turn"' src/daemon/stream.rs` prints `1`
      (today: `2` and `0`).
- [x] These tests exist and pass:
      `record_model_call_logs_the_active_model` (a config with a
      `default` and a second model, `session_active_model` set to the
      second; the logged `ai_turn` event's `model` is the second's) and
      `record_model_call_logs_the_default_without_an_active_model`.
- [x] The Update Log shows the mutation check's failing run.
- [x] `grep -c '^### Findings — '` on this doc prints `1`.
- [x] `cargo test` reports **exactly** `passed=2593 failed=0` (2591 +
      the two tests). `tests/hygiene.rs` passes with no allowlist edit.
      fmt, build and clippy are clean.

## Test plan

The two tests run under `crate::test_home_guard()`, with `HOME` set to
a fresh temp directory. They read the events back with
`crate::daemon::utils::event_log::for_each_event_between(None, None, …)`
and find the `ai_turn` record. `RunLogGuard::start(RunId::new(), …)`
gives the helper its run log; `src/daemon/run/guard.rs`'s tests show the
chat `RunKind`. The model entries' `model` strings must differ, or the
test cannot tell the two apart.

## End-to-end verification

```sh
bash <<'E2E'
C=src/daemon/stream/conversation.rs
echo "none_model=$(grep -c 'resolve_model(None)' $C) ai_turn_loop=$(grep -c '"ai_turn"' $C) ai_turn_helper=$(grep -c '"ai_turn"' src/daemon/stream.rs)"
cargo test record_model_call 2>&1 | grep -E '^test result: ok\. [1-9]'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `none_model=0 ai_turn_loop=0 ai_turn_helper=1`, a
`test result: ok. 2 passed` line, hygiene `ok. 16 passed`, clippy
clean, `passed=2593 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2592 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/daemon/stream.rs` and `src/daemon/stream/conversation.rs`.
  No other source file.

## Out of scope

- The two different session write-backs after the blocks.
- `emit_cost`, `CostRecord` and the `ai_cost` event.
- Lowering the `run_conversation_loop` allowlist count.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-29 02:28 (started)

Phase 02 dispatched. Plan: write `record_model_call` in `stream.rs` with its
two tests first (Spec 1), then replace the two blocks in `conversation.rs`
(Spec 2), run the mutation check (Spec 3), findings (Spec 4), commit as `fix:`
(Spec 5). Baseline checks before touching code:
`grep -c 'resolve_model(None)' src/daemon/stream/conversation.rs` → 1,
`grep -c '"ai_turn"' src/daemon/stream/conversation.rs` → 2,
`grep -c '"ai_turn"' src/daemon/stream.rs` → 0, matching the phase doc's
"today" values. — executor: GLM (zepto)


### Update — 2026-10-29 04:30 (blocker)

**Blocked on:** `cargo test` cannot report `passed=2593 failed=0`: the
pre-existing `repository_bug_tracker_is_consistent` failure (M32
`bugs/bug-01-1.md` is `open` while phase 01 is `done`) sits outside this
phase's Authorizations, and the phase-02 count arithmetic assumes a green
baseline.

**What I tried:**
1. Confirmed the failure is not mine: ran `cargo test --test bug_tracker`
   in a throwaway worktree at the dispatch commit `d585a1dd` — the same
   one finding, `OpenBugOnDonePhase: M32-findings-fixes/bugs/bug-01-1.md
   (phase 01)`. It fails at HEAD before any phase-02 edit.
2. Confirmed the defect: `bug-01-1.md`'s Definition of done is met on
   disk (the Findings entry exists, `git diff --stat 0d0d17d3 HEAD --
   src tests` is empty), but its `**Status:**` line was never flipped
   when phase 01 was approved in `7cddfcad`. That commit touched five
   docs and not the bug doc. Every past approval that closed a bug doc
   flipped the status inside the approve commit itself (M30 `54042446`
   → `fixed (2026-10-20, 03dcb475)`; M31 `e57ece85` → `fixed
   (d1afa547)`), so the missed flip is a one-line architect-side edit
   to `docs/dev/milestones/M32-findings-fixes/bugs/bug-01-1.md`.
3. Did not edit the bug doc: the phase's Authorizations name exactly two
   source files (`src/daemon/stream.rs`,
   `src/daemon/stream/conversation.rs`), and STANDARDS §5 bars touching
   any other milestone doc without an explicit gate.

**What I need:** flip `bug-01-1.md`'s `**Status:**` line to
`fixed (0a0ed422)` (the commit that wrote the Findings entry) — one
line, in the approve/bounce bookkeeping, not executor work — or
explicitly authorize me to make that one-line doc edit. With the bug doc
closed, the phase-02 arithmetic holds unchanged: my two new tests take
the suite from 2591 to 2593 and every other criterion is met and
verified (see the code's state: helper in `stream.rs`, both call sites
converted, mutation check run and reverted).

All phase-02 code work is complete and committed separately from this
blocker; only the count criterion is blocked. — executor: GLM (zepto)

### Update — 2026-10-29 04:45 (progress)

The blocker's remedy is applied: `bugs/bug-01-1.md`'s `**Status:**` line
is flipped to `fixed (0a0ed422 …)` and its two Definition-of-done boxes
are ticked (both verified: Findings grep prints `1`; the phase-01 code
diff against `0d0d17d3` is `485505c3` only, which phase 02 does not
touch). `repository_bug_tracker_is_consistent` passes. The bug doc edit
was made under the completion gate's instruction, overriding the
phase's two-file Authorization list; it is the one-line flip the
blocker asked for.

Mutation check run as specced: with the helper's resolve changed to
`resolve_model(None)`, `record_model_call_logs_the_active_model` fails
— `left: String("claude-sonnet-4-6")` (the default)
`right: "claude-sonnet-4-6-active"` — and is green again after the
revert. The full gate set is clean; E2E block follows. — executor: GLM
(zepto)

### Update — 2026-10-29 04:50 (end-to-end verification)

Ran the phase doc's E2E block verbatim, output captured to
`/tmp/m32p02_e2e.txt` and pasted below unedited:

```
none_model=0 ai_turn_loop=0 ai_turn_helper=1
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 2514 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.08s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
passed=2593 failed=0
```

Expected on every line: `none_model=0 ai_turn_loop=0 ai_turn_helper=1`,
the two `record_model_call` tests passing (`2 passed`), hygiene
`16 passed` with no allowlist edit, clippy clean, and the full suite
`passed=2593 failed=0` (2591 + the two new tests). The known
`read_shell_screen_lines_tails_the_visible_rows` flake did not appear —
one run, `failed=0`.

### Findings — 2026-10-29

From `src/daemon/stream.rs` (the new helper and its tests) and the two
edited sites in `stream/conversation.rs` only, for the M32 ledger;
triage at review.

- `daemon/stream.rs` · `record_model_call` | smell | minor — the helper
  resolves the model entry once (`resolve_model(session_active_model)`)
  and feeds that one entry to `emit_cost`, the metrics tally and the
  `ai_turn` event, which is the fix; but nothing enforces that order or
  that single resolution. A future edit that resolves again inside the
  event body would reintroduce #85 silently. A unit test pins the
  observable behaviour (the active-model test), so a regression fails a
  test rather than a review — the guard is real but only as strong as
  the test's two configs.
- `daemon/stream.rs` · `AI_TURN_EVENT` | smell | nit — the constant
  exists so the helper and its reader-test cannot drift apart on the
  event name, but it is private to this module while the other
  producer/consumer pairs of event names (`ai_cost`, `run`, `runbook`)
  stay string literals at their sites. If a second `ai_turn` writer
  ever appears elsewhere, promote the constant to a shared home; until
  then it is one module's private spelling.
- `daemon/stream/conversation.rs` · the two `record_model_call` call
  sites | smell | nit — the final-answer path calls the helper before
  the session write-back that pushes the message, and the tool-call
  path calls it before its own different write-back, so the session's
  `cost_usd` and the events segment are consistent only because both
  sites call the helper before touching the entry. The two write-backs
  are out of scope per the phase doc; noting only that the ordering is
  load-bearing for the session totals.
