# Phase 06a: a chat turn is a Run — identity and turn log

**Milestone:** M24 — Run Manager
**Status:** todo
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

### `run_conversation_loop` is 1321 lines with six exit paths

`src/daemon/stream.rs:57-1378`. **Five explicit returns plus the implicit
fall-through**, and they are scattered across a very long function:

| Line | What it is |
|---|---|
| `174` | turn-budget / limit refusal |
| `203` | user cancelled (`⊘ cancelled`) |
| `227` | turn timeout |
| `791` | `AiEvent::Error` — the backend failed |
| `1004` | normal completion of a batch cycle |
| `1378` | the function's implicit end |

**This is why task 1 is a Drop guard and not six hand-placed calls.** In a
1321-line function a hand-wired exit is a missed exit, and a missed exit writes
no `RunFinished` at all — the run would look like it is still running forever.
The guard makes a missed path *visible* instead of silent.

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

Then set the outcome at each of the six exits from § Current state:

| Exit | State to record |
|---|---|
| `174` limit refusal | `RunState::BudgetExceeded(Briefing::new(…))` |
| `203` user cancelled | `RunState::Cancelled { stage: "awaiting_model".into(), turns: this_turn_count }` |
| `227` turn timeout | `RunState::BudgetExceeded(Briefing::new(…))` |
| `791` `AiEvent::Error` | `RunState::HardFail(Briefing::new(…))` |
| `1004` normal completion | `RunState::Complete` |
| `1378` implicit end | `RunState::Complete` |

**Do not restructure the function.** Add one `run_log.finish(...)` call before
each return and one before the implicit end. Nothing else in `stream.rs`
changes.

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
      `grep -c 'append_turn_event' src/daemon/run/guard.rs` → **2** — one for
      `RunStarted` in `start`, one for `RunFinished` in `drop`, and no third.
      *(Run it: a `finish` that writes would make this 3.)*
- [ ] **The guard is what `stream.rs` uses, at one construction site:**
      `grep -c 'RunLogGuard::start' src/daemon/stream.rs` → **1** (now `0`).
- [ ] **Every exit records an outcome:**
      `grep -c 'run_log.finish' src/daemon/stream.rs` → **6** (now `0`) — one
      per exit path in § Current state's table.
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
printf 'exactly two write sites    (2): %s\n' "$(grep -c 'append_turn_event' src/daemon/run/guard.rs)"
printf 'one construction in stream (1): %s\n' "$(grep -c 'RunLogGuard::start' src/daemon/stream.rs)"
printf 'six exits record           (6): %s\n' "$(grep -c 'run_log.finish' src/daemon/stream.rs)"
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

<!-- entries appended below this line -->
