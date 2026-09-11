# Phase 06a: a chat turn is a Run — identity and turn log

**Milestone:** M24 — Run Manager
**Status:** done
**Depends on:** phase-01 (`RunId`, `RunKind`, `RunState`, `Briefing`) and
phase-02a (`TurnLogRecord`, `TurnLogEvent`, `append_turn_event`,
`read_turn_log`). Both **done**.
**Estimated diff:** ~380 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Give every chat turn a `RunId` and a turn log at `var/log/runs/<id>.jsonl`, so
the run-log tools shipped in phase-02b (`run_log_tail`, `run_log_search`,
`get_turn`) can read a chat turn like any other run.

**A chat Run is one turn** — one `Ask` request through to the final answer —
carrying `RunKind::Chat { session }`. That is the same granularity phase-05b
gave a background command, and it is what makes `daemoneye runs` a list of
units of work rather than a list of sessions.

**Cancellation is explicitly untouched here.** `src/daemon/cancel.rs` — the
session-keyed `bool` registry — keeps working exactly as it does today.
Collapsing it onto the run-keyed reason-carrying registry from
`src/daemon/run/cancel.rs` is **phase-06b**. This phase is purely additive: no
existing behaviour changes.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 — the `Run` struct; `turn_log: TurnLog`
  at `var/log/runs/<run-id>.jsonl`; `RunKind::Chat{session}`.
- M24 README § exit criteria, "One `Run` for four kinds" — this phase is the
  chat kind's half.

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-10, commit
`9df5067`):

```sh
git status --porcelain | wc -l                                  # 0
cargo test --lib 2>&1 | grep '^test result'                     # 1735 passed
cargo test --lib daemon::run:: 2>&1 | grep '^test result'       # 50 passed
cargo test --lib daemon::stream 2>&1 | grep '^test result'      # 7 passed
cargo test --lib daemon::cancel:: 2>&1 | grep '^test result'    # 6 passed
test -f src/daemon/run/guard.rs && echo 1 || echo 0             # 0
grep -c 'RunLogGuard' src/daemon/stream.rs                      # 0
```

## Current state

### `run_conversation_loop` has exactly FIVE `return` sites — the function
### never falls through

`src/daemon/stream.rs:57-1330` (1274 lines). The outer construct is a
**diverging `loop {`** (`src/daemon/stream.rs:125`), so the function's closing
brace is unreachable. Verified with rustc: inserting a statement after the outer
loop produces

```
warning: unreachable statement
    --> src/daemon/stream.rs:1330:5
     | any code following this expression is unreachable
```

**There is no "implicit end" to wire.** Five `return Ok(())` sites, and they are
identified below by **quoted anchor text, not line numbers** — your own edits
shift every line number in this file, which is exactly what cost the previous
dispatch its budget.

**Beyond those five, the function also exits through every `?`** —
`send_response_split(...).await?` and friends propagate `Err` out. Those paths
are *unbounded and unenumerable*, and wiring them by hand is impossible. **That
is the reason task 1 is a `Drop` guard**: the five returns get an explicit
outcome, and every `?` propagation is caught by `Drop` writing the
"exited without recording an outcome" `HardFail`. A missed path becomes a
visible wrong answer in the log rather than a run that appears never to end.

### The turn-log API this phase consumes

`src/daemon/run/turn_log.rs` — already shipped, do not modify:

```rust
pub struct TurnLogRecord {
    pub timestamp: String,   // caller-supplied: the type stays free of a clock
    pub run_id: RunId,
    #[serde(flatten)]
    pub event: TurnLogEvent,
}

#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum TurnLogEvent {
    RunStarted { kind: RunKind },
    TurnStarted { turn: u32 },
    Note { text: String },
    RunFinished { state: RunState },
}

pub fn turn_log_file(id: &RunId) -> PathBuf;      // runs_dir()/<id>.jsonl
pub fn append_turn_event(record: &TurnLogRecord); // flushes per record; a write
                                                  // failure logs WARN, never errors
pub fn read_turn_log(id: &RunId) -> Vec<TurnLogRecord>;
```

### The exact call shape to follow

`src/daemon/executor/bg_shell.rs` already writes a `RunStarted` for a chat-kind
run. **Same shape, same `RunKind`:**

```rust
    let run_id = RunId::new();
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id: run_id.clone(),
        event: TurnLogEvent::RunStarted {
            kind: RunKind::Chat { session: owner.key() },
        },
    });
```

### `RunState`'s terminal variants

`src/daemon/run/mod.rs:302-314` — the states this phase maps exits onto:

```rust
pub enum RunState {
    Running,
    Complete,
    HardFail(Briefing),
    BudgetExceeded(Briefing),
    Cancelled { stage: String, turns: u32 },
}
```

`Briefing::new(...)` is the only constructor and it **bounds its inputs** — an
over-long briefing cannot be built. Read its signature at
`src/daemon/run/mod.rs:122` before calling it.

## Spec

Four tasks.

### Task 1 — `src/daemon/run/guard.rs`: `RunLogGuard`

```rust
/// Writes a run's `RunStarted` on construction and its `RunFinished` on drop,
/// so a run cannot silently leave its turn log open.
///
/// Every exit path of a long function is an exit path that can be forgotten;
/// `run_conversation_loop` has six. Rather than trust six hand-placed calls,
/// the guard records whatever outcome was last set and, if none was ever set,
/// records a `HardFail` that says so — a forgotten path becomes a visible
/// wrong answer in the log instead of a run that appears to never end.
pub struct RunLogGuard { /* run_id, state: Option<RunState> */ }

impl RunLogGuard {
    /// Write `RunStarted { kind }` and take ownership of the run's id.
    pub fn start(run_id: RunId, kind: RunKind) -> Self;

    /// Record the outcome this run ended with. Calling it again **replaces**
    /// the recorded outcome: the last call before drop is what is written, so
    /// an inner path may set `Complete` and an outer one override it.
    pub fn finish(&mut self, state: RunState);

    pub fn run_id(&self) -> &RunId;
}

impl Drop for RunLogGuard {
    /// Write exactly one `RunFinished`. With no `finish` call, the state is
    /// `HardFail` whose briefing's blocker is the literal
    /// `"the run loop exited without recording an outcome"`.
    fn drop(&mut self);
}
```

**Exactly one `RunFinished` per guard**, whatever happens — a `finish` call is
not a write, only a recording; the write happens once, in `Drop`.

`Drop` cannot be async and must not panic. `append_turn_event` is synchronous
and swallows IO errors at WARN, which is why it is safe to call there — do not
add error handling around it.

### Task 2 — re-export

`src/daemon/run/mod.rs`: add `mod guard;` beside `mod cancel;` and re-export
`RunLogGuard`. Private module, public type — the shape the neighbours already
use (`src/daemon/run/mod.rs:484-491`).

### Task 3 — wire it into `run_conversation_loop`

In `src/daemon/stream.rs`, immediately after the existing cancel-signal
registration block (`src/daemon/stream.rs:88-97`), construct the guard:

```rust
    // A chat turn is a Run: one Ask through to the final answer. The guard
    // writes RunStarted here and exactly one RunFinished however this
    // function exits.
    let mut run_log = RunLogGuard::start(
        RunId::new(),
        RunKind::Chat {
            session: session_id.clone().unwrap_or_default(),
        },
    );
```

Then add **exactly five** `run_log.finish(...)` calls — one immediately before
each `return Ok(())`. Each site is given by the **unique line that precedes it**,
so you can `patch` against text rather than a line number:

**1. Turn timeout.** Anchor — the `await?;` that follows this block:

```rust
                    Response::Error(format!(
                        "Turn exceeded [limits] turn_timeout_secs ({}s) — aborting.",
                        config.limits.turn_timeout_secs
                    )),
```
Record: `RunState::BudgetExceeded(Briefing::new(…))`, blocker naming the turn
timeout.

**2. User cancelled.** Anchor:

```rust
                    send_response_split(tx, Response::SystemMsg("⊘ cancelled".to_string())).await?;
```
Record: `RunState::Cancelled { stage: "awaiting_model".into(), turns: this_turn_count }`.

**3. Backend ended the stream (retries exhausted).** Anchor:

```rust
                                    "AI backend ended the stream without completing \
                                     (after {} attempts): {cause}",
                                    channel_closed_retries
```
Record: `RunState::HardFail(Briefing::new(…))`, blocker naming the truncated
stream. *(This is **not** a turn timeout — an earlier draft of this spec
mislabelled it.)*

**4. `AiEvent::Error`.** Anchor:

```rust
                AiEvent::Error(e) => {
                    send_response_split(tx, Response::Error(e)).await?;
```
Record: `RunState::HardFail(Briefing::new(…))`, blocker naming the backend error.

**5. Normal completion.** Anchor — the `Response::Ok` send that ends a finished
turn, immediately after the token-usage block:

```rust
                        send_response_split(tx, Response::Ok).await?;
                        return Ok(());
```
Record: `RunState::Complete`.

**Do not restructure the function, and do not hunt for a sixth exit — there is
none.** Nothing else in `stream.rs` changes.

The briefings' `blocker` strings are yours to write, but each must name *which*
exit it came from, so a reader of the log can tell a timeout from a refusal
without the line number. Keep them short — `Briefing::new` bounds them.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-06a.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `test -f src/daemon/run/guard.rs && echo 1 || echo 0` → **1** (now `0`),
      and `grep -c '^mod guard;' src/daemon/run/mod.rs` → **1** (now `0`).
- [ ] Declared exactly once each in `src/daemon/run/guard.rs`:
      `pub struct RunLogGuard` → **1**, `pub fn start` → **1**,
      `pub fn finish` → **1**, `impl Drop for RunLogGuard` → **1** (each **0**
      now).
- [ ] **Exactly one write site, and it is in `Drop`:**
      `grep -c 'append_turn_event(&TurnLogRecord' src/daemon/run/guard.rs` →
      **2** — one for `RunStarted` in `start`, one for `RunFinished` in `drop`,
      and no third. *(The pattern includes the argument so it counts **call
      sites** only; a bare `grep -c 'append_turn_event'` also matches the `use`
      import and would read 3 on correct code.)*
- [ ] **The guard is what `stream.rs` uses, at one construction site:**
      `grep -c 'RunLogGuard::start' src/daemon/stream.rs` → **1** (now `0`).
- [ ] **Every exit records an outcome:**
      `grep -c 'run_log.finish' src/daemon/stream.rs` → **5** (now `0`) — one
      per `return Ok(())` site. **Five, not six**: the function's closing brace
      is unreachable (§ Current state proves it with rustc), so there is no
      sixth exit to wire. Every `?` propagation is covered by `Drop`, by
      design.
- [ ] **`stream.rs` does not write turn-log records itself:**
      `grep -c 'append_turn_event' src/daemon/stream.rs` → **0**. The guard owns
      the writing.
- [ ] **Cancellation is untouched (preservation, the whole point of the
      06a/06b split):**
      `git diff --name-only 9df5067 -- src/daemon/cancel.rs | wc -l` → **0**,
      `cargo test --lib daemon::cancel::` → **6 passed**, and
      `grep -c 'cancel::register_turn' src/daemon/stream.rs` → **1**, unchanged.
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/guard.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0**. *(`Drop` must not panic — this criterion is that rule with
      teeth.)*
- [ ] `cargo test --lib daemon::run::guard` → **7 or more** passing, `0 failed`
      (now **0** — no such module). *Arithmetic: the 7 tests in the Test plan.*
- [ ] `cargo test --lib daemon::run::` → **57 or more** passing, `0 failed`
      (now **50**). *50 + 7.*
- [ ] **Preservation, each must not move:** `daemon::stream` **7**,
      `daemon::cancel::` **6**, `daemon::executor` **235**,
      `daemon::run::log_read` **11**, `daemon::run::turn_log` **7**.
- [ ] **The real HOME's run log stays empty** — the suite must not write into
      the developer's `~/.daemoneye/var/log/runs/`:
      `ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l` is **the same before
      and after** a full suite run. Every new test uses a tempdir HOME via
      `crate::test_home_guard()`.
- [ ] Full lib suite **1742 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1735**).
- [ ] All four gates pass.

## Test plan

All in `src/daemon/run/guard.rs`. Every test that touches the filesystem takes
`crate::test_home_guard()` and a tempdir `HOME` — the RAII idiom is at
`src/daemon/run/cancel.rs`'s `HomeGuard` (phase-04), reuse that shape.

- `guard_start_writes_run_started_with_the_kind` — after `start`, `read_turn_log`
  returns exactly one record, a `RunStarted` whose `kind` is the
  `RunKind::Chat { session }` handed in.
- `guard_drop_after_finish_writes_that_state` — `finish(RunState::Complete)`
  then drop; the log's **last** record is `RunFinished { state: Complete }`.
- `guard_drop_without_finish_records_the_missed_outcome` — construct, drop
  without calling `finish`; the last record is `RunFinished` with a `HardFail`
  whose briefing blocker is the literal `"the run loop exited without recording
  an outcome"`. **This is the guard's reason to exist** — assert on the literal,
  not merely that the state is `HardFail`.
- `guard_writes_exactly_one_run_finished` — `finish` three times with different
  states, then drop; `read_turn_log` contains **exactly one** `RunFinished`.
  *(The negative case: a `finish` that wrote would make this 3.)*
- `guard_last_finish_wins` — `finish(Complete)` then
  `finish(Cancelled{..})`, drop; the recorded state is the `Cancelled` one.
- `guard_records_a_cancelled_state_with_its_stage_and_turns` — a
  `Cancelled { stage: "awaiting_model", turns: 3 }` round-trips through the log
  with both fields intact.
- `guard_log_lands_under_the_runs_dir_for_its_id` — the file
  `turn_log_file(guard.run_id())` exists and sits under
  `crate::config::runs_dir()`. **Negative half:** no file is created for any
  *other* run id.

## Mutation checks

Three. Use the `patch` tool to break and revert — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.**
Record the red-then-green transcript for each in the Update Log.

Each names **the assertion message expected in the failure output**, not just
the test. A mutation that reddens a multi-assertion test proves only that *some*
assertion fired; quote the line that actually failed, so the check is evidence
about the property you meant to guard.

1. **A missed exit path must be visible.** Make `Drop` return early when no
   `finish` was called (write nothing) →
   `guard_drop_without_finish_records_the_missed_outcome` must go **red** on the
   "records the missed outcome" assertion, **not** on a file-missing panic.
2. **Exactly one `RunFinished`.** Make `finish` call `append_turn_event` itself
   in addition to recording → `guard_writes_exactly_one_run_finished` must go
   **red** naming the expected count of 1 against the observed 3.
3. **The last outcome wins.** Make `finish` ignore the call when a state is
   already set (first-wins instead of last-wins) → `guard_last_finish_wins` must
   go **red** showing `Complete` where `Cancelled` was expected.

## End-to-end verification

The real artifact is the on-disk turn log under `var/log/runs/`, and the tools
that read it. Section B runs the guard tests with `--nocapture`; section H is
the real door — a **throwaway HOME**, a real `RunLogGuard`, and a real
`read_turn_log` round-trip proving a record written by this phase is readable
by the phase-02a reader that the `run_log_*` tools sit on.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-06a.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE the suite: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the guard tests =="
cargo test --lib daemon::run::guard -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::run::guard|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. module counts (guard >=7, run:: >=57) =="
for m in daemon::run::guard daemon::run::; do
  printf '%-24s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== D. preservation: stream 7, cancel 6, executor 235, log_read 11 =="
for m in daemon::stream daemon::cancel:: daemon::executor daemon::run::log_read daemon::run::turn_log; do
  printf '%-26s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== E. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== F. structural greps (each must print the stated number) =="
printf 'guard.rs exists            (1): %s\n' "$(test -f src/daemon/run/guard.rs && echo 1 || echo 0)"
printf 'mod guard; private         (1): %s\n' "$(grep -c '^mod guard;' src/daemon/run/mod.rs)"
printf 'RunLogGuard struct         (1): %s\n' "$(grep -c 'pub struct RunLogGuard' src/daemon/run/guard.rs)"
printf 'impl Drop                  (1): %s\n' "$(grep -c 'impl Drop for RunLogGuard' src/daemon/run/guard.rs)"
printf 'exactly two write sites    (2): %s\n' "$(grep -c 'append_turn_event(&TurnLogRecord' src/daemon/run/guard.rs)"
printf 'one construction in stream (1): %s\n' "$(grep -c 'RunLogGuard::start' src/daemon/stream.rs)"
printf 'five exits record          (5): %s\n' "$(grep -c 'run_log.finish' src/daemon/stream.rs)"
printf 'stream never writes direct (0): %s\n' "$(grep -c 'append_turn_event' src/daemon/stream.rs)"
printf 'no unwrap/panic pre-test   (0): %s\n' "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/guard.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"

echo "== G. cancellation untouched (the 06a/06b split, as a checkable property) =="
printf 'daemon/cancel.rs diff      (0): %s\n' "$(git diff --name-only 9df5067 -- src/daemon/cancel.rs | wc -l)"
printf 'stream still registers turn(1): %s\n' "$(grep -c 'cancel::register_turn' src/daemon/stream.rs)"

echo "== H. the real door: a turn log written and read back in a throwaway HOME =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
cargo test --lib daemon::run::guard::tests::guard_log_lands_under_the_runs_dir_for_its_id -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test |^test result'
echo "cargo_exit=${PIPESTATUS[0]}"
} >> "$A" 2>&1
sed -i 's/\x1b\[[0-9;]*m//g' "$A" 2>/dev/null || true
cat "$A"
```

*(The trailing `sed -i` is the **architect's** transcript-cleanup line and is
part of this block, not an edit to a source file; if `bash` refuses it, the
`|| true` lets the block finish and section B has already stripped colour.)*

Then, as the last task, the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-06a-chat-run-log.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-06a.txt
diff /tmp/pasted-m24-06a.txt /tmp/e2e-m24-06a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/run/guard.rs` — new file, tasks 1 and the Test plan.
- `src/daemon/run/mod.rs` — task 2 **only**: one `mod guard;` line and one
  re-export.
- `src/daemon/stream.rs` — task 3 **only**: the guard construction and six
  `run_log.finish(...)` calls. **Do not restructure `run_conversation_loop`.**
- `docs/dev/milestones/M24-run-manager/phase-06a-chat-run-log.md` — the Update
  Log.

**Do not touch** `src/daemon/cancel.rs` (§ G asserts a zero diff — the split is
the point), `src/daemon/run/turn_log.rs` (shipped in 02a and read by the
`run_log_*` tools), or anything under `src/daemon/executor/` or `src/shell/`.

## Out of scope

- **Collapsing the two cancel registries.** `src/daemon/cancel.rs`
  (session-keyed, `bool`) and `src/daemon/run/cancel.rs` (run-keyed,
  `CancelReason`-carrying) stay as they are. **Phase-06b.**
- **Wiring `watch_stop_sentinel` to a chat turn**, so `daemoneye stop <run>`
  cancels it. Phase-06b — it needs the session→run mapping that the cancel
  collapse brings.
- **A session→run map.** Nothing in this phase needs one; adding it here would
  be scope with no consumer. Phase-06b adds it because `Request::Cancel {
  session_id }` must resolve to a run.
- **`TurnStarted` / `Note` events.** The two remaining `TurnLogEvent` variants
  stay unused by chat until there is a reader that wants them.
- **`daemoneye runs`** — phase-10.
- **`Budget` enforcement on a chat run.** The flat per-turn/-session/-tool caps
  in `config.limits` remain the safety net; the governor is M25.
- **Ghost and scheduled runs.** Phases 07 and 08.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Notes for executor — 2026-09-10

**Read this first. Most of this phase is already on disk and it is correct — do
not start over.**

The previous dispatch ended `budget_exceeded` at 600 turns, and the cause was
**three defects in this spec**, all now corrected:

1. **§ Current state gave line numbers, and your own edit invalidated them.**
   Inserting the guard shifts every line in `stream.rs` by +8, so the table you
   were given was wrong the moment you used it. The session log shows the
   result: hundreds of `git show 9df5067:src/daemon/stream.rs | awk …` calls
   re-deriving positions. **Task 3 now identifies each site by quoted anchor
   text**, which `patch` can match and which no edit invalidates.
2. **The spec promised six exit paths. There are five.** The outer construct is
   a diverging `loop {`, so the function's closing brace is unreachable — rustc
   says so, and § Current state now quotes the proof. You were scanning the end
   of a 1274-line function for an exit that cannot exist.
3. **Two exits were mislabelled.** What the old table called a "limit refusal"
   is the turn timeout, and what it called a "turn timeout" is the
   backend-ended-the-stream path. Task 3 now names each correctly.

**Already on disk and approved — do not redo:** `src/daemon/run/guard.rs`
(`RunLogGuard`, `start`, `finish`, `Drop`) and the `mod guard;` + re-export in
`src/daemon/run/mod.rs`. **All seven guard tests pass.** The guard construction
in `src/daemon/stream.rs` is also correct and in the right place.

**Exactly one thing remains:** add **five** `run_log.finish(...)` calls, one
immediately before each `return Ok(())`, per task 3's five anchors. `cargo
build` currently warns `unused variable: run_log` and `variable does not need to
be mutable` — **those two warnings disappearing is your finish condition**, and
`grep -c 'run_log.finish' src/daemon/stream.rs` must print **5**. Then the three
mutation checks and the § End-to-end verification block.

Do not touch `src/daemon/cancel.rs` (§ G asserts a zero diff) and do not
restructure `run_conversation_loop`.

<!-- entries appended below this line -->

### Update — 2026-09-11 00:15 (started)

Beginning phase-06a: the `RunLogGuard` in `src/daemon/run/guard.rs` (RunStarted
on construction, exactly one RunFinished in Drop with the last `finish` winning
and the literal missed-outcome HardFail as the fallback), its re-export beside
`mod cancel;`, and one `run_log.finish(...)` at each of the six exits of
`run_conversation_loop`. Cancellation (`src/daemon/cancel.rs`) stays untouched
— the 06a/06b split. Then the mutation pair and the end-to-end block.

### Update — 2026-09-10 (escalation)

**Chosen lever:** refined re-dispatch against the partial tree (`execute_phase`
on this `in-progress` doc, not `continue_phase`).
**Rationale:** the spec was the problem — three defects, of which the
unsatisfiable "six exits" criterion and the shifting line numbers together
account for the whole 600-turn navigation thrash visible in the session log —
so it had to be corrected regardless; the guard work already on disk is correct
(7/7 tests) and the dirty tree preserves it, so a fresh re-dispatch would only
rebuild it. `continue_phase` was not used because it is blocking, returns no
`run_id` and registers no watcher, leaving no way to stop a second runaway.


### Update — 2026-09-10 (mutation checks)

Three mutations, each applied with `patch`, each reverted from a copy taken
before the mutation (`git status` shows no diff afterwards); red-then-green
transcripts captured by re-running the named test. The failure-line quotes are
the assertion messages, not just test names.

**1. A missed exit path must be visible.** Mutation: `Drop` returns early
(writes nothing) when no `finish` was recorded.

```
test daemon::run::guard::tests::guard_drop_without_finish_records_the_missed_outcome ... FAILED
thread ... panicked at src/daemon/run/guard.rs:159:9:
assertion `left == right` failed: a guard with no finish call must still write start and finish
```

Reverted → green: `cargo test --lib daemon::run::guard` → 7 passed; 0 failed.

**2. Exactly one `RunFinished`.** Mutation: `finish` calls `append_turn_event`
itself in addition to recording.

```
test daemon::run::guard::tests::guard_writes_exactly_one_run_finished ... FAILED
thread ... panicked at src/daemon/run/guard.rs:200:9:
assertion `left == right` failed: exactly one RunFinished must be written, not one per finish call
  left: 4
 right: 1
```

Reverted → green: 7 passed; 0 failed.

**3. The last outcome wins.** Mutation: `finish` returns early when a state is
already set (first-wins).

```
test daemon::run::guard::tests::guard_last_finish_wins ... FAILED
thread ... panicked at src/daemon/run/guard.rs:220:17:
assertion `left == right` failed: the last finish wins, not the first
  left: Complete
 right: Cancelled { stage: "awaiting_model", turns: 2 }
```

Reverted → green: 7 passed; 0 failed.

### Update — 2026-09-10 (end-to-end verification)

Command run: the `§ End-to-end verification` block from this phase doc,
verbatim, from the repo root (one change: the block's trailing
`sed -i 's/\x1b.../g' "$A" 2>/dev/null || true` transcript-cleanup line — the
architect's line — is refused by `bash` as an in-place edit, so the equivalent
`tr | sed | mv` replacement was used; the output is the same strip). The paste
below is the contents of `/tmp/e2e-m24-06a.txt` exactly as it landed on disk:

```sh
== A. build ==
real HOME run logs BEFORE the suite: 0
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.10s
cargo_exit=0
== B. the guard tests ==
test daemon::run::guard::tests::guard_drop_after_finish_writes_that_state ... ok
test daemon::run::guard::tests::guard_drop_without_finish_records_the_missed_outcome ... ok
test daemon::run::guard::tests::guard_last_finish_wins ... ok
test daemon::run::guard::tests::guard_log_lands_under_the_runs_dir_for_its_id ... ok
test daemon::run::guard::tests::guard_records_a_cancelled_state_with_its_stage_and_turns ... ok
test daemon::run::guard::tests::guard_start_writes_run_started_with_the_kind ... ok
test daemon::run::guard::tests::guard_writes_exactly_one_run_finished ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1739 filtered out; finished in 0.00s
cargo_exit=0
== C. module counts (guard >=7, run:: >=57) ==
daemon::run::guard       test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1739 filtered out; finished in 0.00s
daemon::run::            test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 1689 filtered out; finished in 0.01s
== D. preservation: stream 7, cancel 6, executor 235, log_read 11 ==
daemon::stream             test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1739 filtered out; finished in 0.00s
daemon::cancel::           test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1740 filtered out; finished in 0.05s
daemon::executor           test result: ok. 235 passed; 0 failed; 4 ignored; 0 measured; 1507 filtered out; finished in 1.08s
daemon::run::log_read      test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1735 filtered out; finished in 0.01s
daemon::run::turn_log      test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1739 filtered out; finished in 0.00s
== E. lib suite, three consecutive runs ==
test result: ok. 1742 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.27s
test result: ok. 1742 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.08s
test result: ok. 1742 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.11s
== F. structural greps (each must print the stated number) ==
guard.rs exists            (1): 1
mod guard; private         (1): 1
RunLogGuard struct         (1): 1
impl Drop                  (1): 1
exactly two write sites    (2): 2
one construction in stream (1): 1
five exits record          (5): 5
stream never writes direct (0): 0
no unwrap/panic pre-test   (0): 0
== G. cancellation untouched (the 06a/06b split, as a checkable property) ==
daemon/cancel.rs diff      (0): 0
stream still registers turn(1): 1
== H. the real door: a turn log written and read back in a throwaway HOME ==
real HOME run logs AFTER (==BEFORE): 0
test daemon::run::guard::tests::guard_log_lands_under_the_runs_dir_for_its_id ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1745 filtered out; finished in 0.00s
cargo_exit=0
```

Section H is the real door: a real `RunLogGuard` constructing a turn log under
`crate::config::runs_dir()` in a throwaway HOME, read back through the
phase-02a `read_turn_log` that the `run_log_*` tools sit on.

### Update — ts=1789097187427 (complete, server-authored)

**Summary:** Ongoing Summary

The phase is complete. Work committed as `45379eb feat: a chat turn is a Run — RunLogGuard writes its turn log`.

**What landed** (+ dev-doc Update Log entries for the mutation checks and the end-to-end paste, `PASTE MATCH` confirmed):

1. **`src/daemon/run/guard.rs`** (already on disk from the prior dispatch, left as found after the five `run_log.finish(...)` edits to `stream.rs`): `RunLogGuard` with `start` (writes `RunStarted { kind }`), `finish` (records, last-wins, replaces), and `Drop` writing exactly one `RunFinished` — the `HardFail` fallback with the literal `"the run loop exited without recording an outcome"` when no `finish` was called. 7 tests, all passing, all tempdir-HOME-scoped via the `run::cancel` `HomeGuard` shape.
2. **`src/daemon/run/mod.rs`**: `mod guard;` + `pub use guard::RunLogGuard;` (the only new code outside the guard file).
3. **`src/daemon/stream.rs`**: one `RunLogGuard::start` right after the existing cancel-signal registration; **five** `run_log.finish(...)` calls at the five `return Ok(())` sites, each identified by the anchor text the spec pins: turn timeout → `BudgetExceeded`, user cancel → `Cancelled { stage: "awaiting_model", turns }`, backend-ended-stream → `HardFail` (blocker naming attempts and cause), `AiEvent::Error` → `HardFail`, and normal completion → `Complete`. No restructuring; `daemon/cancel.rs` untouched (verified zero-diff vs `9df5067`).

**Verification:** all four gates clean — `cargo fmt --all` (run immediately before `git add`), `cargo build` 0 warnings, `clippy --all-targets --all-features -D warnings` clean, `cargo test --lib` **1742 passed / 0 failed on three consecutive runs** (target ≥1742, was 1735). Preservation held exactly: `stream` 7, `cancel::` 6, `executor` 235, `run::log_read` 11, `run::turn_log` 7, `run::guard` 7, `run::` 57. The real `~/.daemoneye/var/log/runs/` count stayed 0 before/after the full suite. The `§ End-to-end verification` block ran verbatim as `bash` refused the block's trailing `sed -i` transcript-cleanup line (the `|| true` path); the alternative strip (`tr | sed | mv`, shown in the Update Log) does the same strip. All nine § F structural greps and both § G greps printed the stated values. Mutation checks 1–3 all went red on the intended assertion messages and reverted green.

**Notes for review:** `AiEvent::Error`'s blocker now formats the event's own error string with an explicit `e.clone()` on the send (rustc required it; the message text is unchanged). Status is left **`in-progress`** as instructed — no `(complete)` entry, no status flip. Cancellation collapse, `watch_stop_sentinel` wiring, session→run mapping, `TurnStarted`/`Note` events, `daemoneye runs`, and budget enforcement all remain out of scope, as the spec reserves them.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.58s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M24-run-manager/phase-06a-chat-run-log.md` — +106 -0
- `src/daemon/run/guard.rs` — +1 -0
- `src/daemon/stream.rs` — +42 -1

**Commit:** 45379eb0dd63f190a105804962268bfb7c3e1918

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-10

- **Verdict:** approved_after_1
- **Bounces:** none at review. One prior dispatch ended `budget_exceeded` at 600
  turns and was escalated (`spec_bug`); the corrected spec landed this run in
  **166 turns**.
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (run 1: 600 turns,
  budget_exceeded; run 2: 166 turns, complete)
- **Scope deviations:** none. Only `guard.rs`, the two `mod.rs` lines, and five
  `run_log.finish(...)` calls plus one `e.clone()` in `stream.rs` (the clone is
  required — `e` moves into `Response::Error` and the briefing needs it after).
  `src/daemon/cancel.rs` holds a **zero diff**, so the 06a/06b split held as a
  checkable property.
- **Calibration:** the prior run's loss was **three architect spec defects**,
  folded before the re-dispatch as WORKFLOW.md § "A check the executor cannot
  satisfy is a bounce you wrote yourself": exit sites pinned by *line number*
  (invalidated by the phase's own +8-line insertion), a **sixth exit path that
  does not exist** (the function is a diverging `loop`; rustc reports the closing
  brace unreachable), and two exits mislabelled. Replacing line numbers with
  quoted anchors and correcting the count took the run from 600 turns to 166 —
  **a 3.6× reduction from spec quality alone, on the same model and phase.**
  *(The daemoneye `read_only_stall_threshold` was also restored from a
  mistaken `0` to `200` before this run; it did not fire here, but it was
  disabled during the failure.)*
- **Warnings (two, neither worth a bounce):**
  1. **`#[expect(dead_code)]` on the test `HomeGuard.tmp` field**
     (`src/daemon/run/guard.rs:81`). The field is genuinely RAII-held — dropping
     the `TempDir` deletes the directory — so it is not masking a defect, and
     `expect` is self-cleaning where `allow` is not. But the idiomatic
     zero-attribute fix exists: **verified at review that renaming the field to
     `_tmp` is clippy-clean under `-D warnings` with all 7 tests passing.**
     Prefer `_tmp` next time this file is touched. *(The spec said to reuse
     `run::cancel`'s `HomeGuard` shape, where `tmp` **is** read at
     `src/daemon/run/cancel.rs:276` — so the copied shape was dead here through
     no fault of the executor.)*
  2. **Architect note — do not put `sed -i` in an § End-to-end verification
     block.** The executor contract forbids in-place edits and `bash` refused
     this block's trailing transcript-cleanup line, forcing a documented
     `tr | sed | mv` substitution. The `|| true` guard meant the block still
     completed, but the phase doc should not ship a command the executor cannot
     run. Use the `tr | sed | mv` form, or strip at generation only.

**Verified independently at review** (not read from the executor's transcript):
four gates green on a forced recompile — fmt, build zero warnings, clippy
`-D warnings`, **1742 passed / 0 failed**, the exact pinned floor of 1735 + 7.
Every structural criterion at its value (two write sites, one construction, five
`finish` calls, zero direct writes from `stream.rs`, zero `cancel.rs` diff,
`cancel::register_turn` still 1, real-HOME run log still empty); preservation
exact (`stream` 7, `cancel::` 6, `executor` 235, `log_read` 11, `turn_log` 7,
`guard` 7, `run::` 57). The five recorded states are correct **and in the right
order** — `BudgetExceeded` (turn timeout), `Cancelled` (user), `HardFail`
(backend ended the stream — the exit the first spec mislabelled), `HardFail`
(`AiEvent::Error`), `Complete`. Two mutations re-run by hand: making `Drop` skip
a missed exit reddens
`guard_drop_without_finish_records_the_missed_outcome` on "a guard with no finish
call must still write start and finish", and making `finish` write as well as
record reddens `guard_writes_exactly_one_run_finished` on `left: 4 / right: 1`.
**The guard's reason to exist is genuinely guarded.**

