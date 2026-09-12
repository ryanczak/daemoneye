# Phase 09a: the resume seed — briefing, not transcript

**Milestone:** M24 — Run Manager
**Status:** todo
**Depends on:** phase-01 (`RunKind`, `Briefing`, `RunState`), phase-02a
(`TurnLogRecord`, `read_turn_log`) and phase-03 (`Briefing::render()`). All
**done**.
**Estimated diff:** ~330 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

M24's resume criterion is specific about *what a resumed run is seeded with*:

> `daemoneye run resume <id> "<directive>"` restarts from runbook + briefing +
> directive + current state (**not transcript rehydration**).

This phase builds exactly that seed, and nothing else. Given a finished run's
id and a human directive, produce the text a resumed run would start from:
the run's kind (which names its runbook), its terminal `Briefing` when it has
one, and the directive.

**No CLI, no IPC, no ghost spawn.** Those are phase-09b. This half is pure and
heavily testable; the wiring half is where the risk lives, and separating them
is the same additive/rewire seam that worked for 05, 06, 07 and 08.

The "not transcript rehydration" clause is enforced **structurally**: the
rendering function's signature cannot see the transcript — it takes a kind, an
optional briefing and a directive, and a criterion pins that `TurnLogRecord`
does not appear in it.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 — resume seeds from briefing + directive
  + current state, not from replaying turns.
- M24 README § exit criteria, "**Briefing-seeded resume.**"

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-12, commit
`5138a33`):

```sh
git status --porcelain | wc -l                                       # 0
cargo test --lib 2>&1 | grep '^test result'                          # 1766 passed
cargo test --lib daemon::run::turn_log 2>&1 | grep '^test result'    # 7 passed
cargo test --lib daemon::run::log_read 2>&1 | grep '^test result'    # 11 passed
cargo test --lib daemon::run::result 2>&1 | grep '^test result'      # 11 passed
test -f src/daemon/run/resume.rs && echo 1 || echo 0                 # 0
grep -c '^mod resume;' src/daemon/run/mod.rs                         # 0
```

## Current state

### Two unrelated things are both called "briefing" — do not confuse them

This is the single most likely way to get this phase wrong.

| | `crate::daemon::briefing` | `crate::daemon::run::Briefing` |
|---|---|---|
| What | The **G4 agent briefing**: free-form AI-written prose saved per *agent* | The **run briefing**: a bounded struct from phase-01 |
| API | `read_briefing(agent_name: &str) -> Option<String>` (`src/daemon/briefing.rs:138`) | `Briefing::new(goal, criteria, working_files, attempts, blocker, budget_left)` + `render()` |
| Where it lives | `~/.daemoneye/agents/<name>/…` | inside `RunState::HardFail` / `BudgetExceeded` in the run's turn log |

**This phase uses only the second one.** Do not call
`crate::daemon::briefing::read_briefing`; it answers a different question about
a different subject.

### The turn log is the source

```rust
// src/daemon/run/turn_log.rs:38-47 — the record shape
pub enum TurnLogEvent {
    RunStarted { kind: RunKind },
    TurnStarted { turn: u32 },
    Note { text: String },
    RunFinished { state: RunState },
}
pub fn read_turn_log(id: &RunId) -> Vec<TurnLogRecord>;   // :99
```

`RunState` (`src/daemon/run/mod.rs:303`) carries a `Briefing` in exactly two
variants:

```rust
pub enum RunState {
    Running,
    Complete,
    HardFail(Briefing),
    BudgetExceeded(Briefing),
    Cancelled { stage: String, turns: u32 },
}
```

So a `Complete` or `Cancelled` run has **no** briefing — and resuming one is
still legitimate (the directive alone is the instruction). A run with no
`RunFinished` at all is still in flight and must **not** be resumable.

### `Briefing::render()` — use it, do not re-implement it

```rust
// src/daemon/run/result.rs:256 — emits six headed sections, always all six
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# Goal\n\n");
        push_block(&mut out, &self.goal);
        out.push_str("# Criteria\n\n");
        …
```

Every section is emitted even when empty (`(none)` / `(no prior attempts)` /
`(within budget)`), which is deliberate: an empty section stays
distinguishable from one that was never written. The seed embeds this verbatim.

### `RunKind` names the runbook

```rust
// src/daemon/run/mod.rs:68-77
pub enum RunKind {
    Chat { session: String },
    Ghost { runbook: String, trigger: String },
    Scheduled { job: String },
    Delegated { agent: String, parent: RunId },
}
```

`label()` (`:81`) returns `"chat"` / `"ghost"` / `"scheduled"` / `"delegated"`.

## Spec

Four tasks.

### Task 1 — `src/daemon/run/resume.rs`: the error type

```rust
/// Why a run cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeError {
    /// No turn log exists for this id, or it holds no records.
    NoRunLog,
    /// The log has no `RunFinished` — the run is still in flight.
    StillRunning,
}
```

Give it a `Display` impl so the 09b CLI can print it: `NoRunLog` →
`"no run log for that id"`, `StillRunning` →
`"that run is still running; stop it first"`. Implement `std::error::Error`
for it too, so `anyhow` can carry it later.

### Task 2 — `terminal_outcome`: pure over records

```rust
/// The kind a run started with and the briefing it ended with, read from its
/// turn log records. Pure — takes the records, does no I/O.
pub fn terminal_outcome(
    records: &[TurnLogRecord],
) -> Result<(RunKind, Option<Briefing>), ResumeError>
```

Rules:

- Empty slice → `Err(ResumeError::NoRunLog)`.
- No `RunStarted` record → `Err(ResumeError::NoRunLog)`. *(A log without its
  opening record cannot name a kind; treat it as absent rather than guessing.)*
- No `RunFinished` record → `Err(ResumeError::StillRunning)`.
- Otherwise → `Ok((kind, briefing))` where `kind` comes from the **first**
  `RunStarted` and `briefing` from the **last** `RunFinished`:
  `HardFail(b)` / `BudgetExceeded(b)` → `Some(b.clone())`;
  `Complete` / `Cancelled { .. }` / `Running` → `None`.

**First `RunStarted`, last `RunFinished`.** The guard writes exactly one of
each today, but taking first/last makes the function total over a log that was
appended to twice, and the Test plan pins it.

### Task 3 — `render_seed`: the seed text, pure

```rust
/// The text a resumed run starts from: what it was, what it learned, and what
/// it is now being told to do.
///
/// Takes no transcript. Resume seeds from the briefing and the directive —
/// design § 2.3, "not transcript rehydration" — and this signature is what
/// makes that structural rather than aspirational.
pub fn render_seed(kind: &RunKind, briefing: Option<&Briefing>, directive: &str) -> String
```

Emit, in this order, each section headed so the model can find it:

1. `# Resumed run` — one line naming the kind via `kind.label()`, plus the
   kind's own identifying field: the runbook for `Ghost`, the job for
   `Scheduled`, the session for `Chat`, the agent for `Delegated`.
2. `# Prior briefing` — `briefing.render()` verbatim when `Some`; the literal
   `(no briefing — the run ended without one)` when `None`.
3. `# Directive` — the directive text.

**The directive must always appear**, including when it is empty: emit
`(none given)` rather than an empty section, matching `Briefing::render()`'s
own convention that an empty section renders a placeholder. A named test pins
this.

### Task 4 — `resume_seed`: the one I/O function, and the re-export

```rust
/// Read a run's turn log and build its resume seed.
pub fn resume_seed(run_id: &RunId, directive: &str) -> Result<String, ResumeError>
```

Body: `read_turn_log(run_id)` → `terminal_outcome(&records)?` → `render_seed`.
That is the whole function; all the logic lives in the two pure halves.

In `src/daemon/run/mod.rs`: `mod resume;` beside `mod result;`, and
`pub use resume::{ResumeError, render_seed, resume_seed, terminal_outcome};` —
same shape as the `pub use guard::RunLogGuard;` line at `:499`.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-09a.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

**On transcripts:** record only what you actually observed. If a mutation does
not turn its test red, say so plainly and say why — that is an accepted,
useful outcome. Never write a transcript for a run you did not make, and never
fill in a predicted failure message from this document. Hand-written commentary
around a pasted transcript is fine, but anything presented as command output
must be command output, **including line numbers**.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.
**Counts scoped `prod:` use the production-only prefix**
`awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/resume.rs |` — this
phase's own tests name the same symbols, and an unscoped grep would pin them
too.

- [ ] **The module exists and is wired:**
      `test -f src/daemon/run/resume.rs && echo 1 || echo 0` → **1** (now `0`);
      `grep -c '^mod resume;' src/daemon/run/mod.rs` → **1** (now `0`);
      `grep -c 'pub use resume::' src/daemon/run/mod.rs` → **1** (now `0`).
- [ ] **Declared exactly once each**, prod-scoped: `grep -c 'pub fn terminal_outcome'`
      → **1**; `grep -c 'pub fn render_seed'` → **1**;
      `grep -c 'pub fn resume_seed'` → **1**;
      `grep -c 'pub enum ResumeError'` → **1** (each **0** now).
- [ ] **The seed cannot see the transcript** — this is the design clause made
      structural: `sed -n '/pub fn render_seed/,/^}$/p' src/daemon/run/resume.rs | grep -c 'TurnLogRecord'`
      → **0**, and the signature takes an optional briefing:
      `sed -n '/pub fn render_seed/,/-> String/p' src/daemon/run/resume.rs | grep -c 'Option<&Briefing>'`
      → **1** (now `0`). *(Pinned as a property — it takes a briefing and
      cannot see records — not as an exact signature string: parameter names
      and order are rendering, and WORKFLOW.md § "Specs pin behavior, not
      rendering" applies.)*
- [ ] **`terminal_outcome` does no I/O**, prod-scoped:
      `sed -n '/pub fn terminal_outcome/,/^}$/p' src/daemon/run/resume.rs | grep -cE 'read_turn_log|std::fs|File::'`
      → **0**.
- [ ] **`resume_seed` is the only reader**, prod-scoped:
      `grep -c 'read_turn_log'` → **1**.
- [ ] **The G4 briefing is not involved** — the wrong-`Briefing` trap:
      `grep -c 'daemon::briefing' src/daemon/run/resume.rs` → **0**, and
      `grep -rc 'daemon::briefing' src/daemon/run/ --include='*.rs' | grep -v ':0' | wc -l`
      → **0**.
- [ ] **`Briefing::render` is reused, not re-implemented**, prod-scoped:
      `grep -c '.render()'` → **1**, and
      `grep -c '# Goal'` → **0** *(the six section headings belong to
      `result.rs`; re-emitting one here would mean a second copy of that
      format)*.
- [ ] **Both error variants are constructed**, prod-scoped:
      `grep -c 'ResumeError::NoRunLog'` → **2 or more** and
      `grep -c 'ResumeError::StillRunning'` → **2 or more** (the variant plus
      its `Display` arm and its construction site).
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` in production code, prod-scoped:
      `grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'` → **0**.
- [ ] `cargo test --lib daemon::run::resume` → **10 or more** passing,
      `0 failed` (now **0** — no such module). *Arithmetic: the 10 in the Test
      plan.*
- [ ] **Preservation, each must not move:** `daemon::run::turn_log` **7**,
      `daemon::run::log_read` **11**, `daemon::run::result` **11**,
      `daemon::run::guard` **8**, `daemon::run::active` **6**,
      `daemon::run::cancel` **13**, `daemon::scheduled` **7**,
      `daemon::ghost` **12**.
- [ ] **Nothing outside the run module changed:**
      `git diff --stat -- src/ ':!src/daemon/run/' | wc -l` → **0**.
- [ ] **Nothing leaks into the developer's real HOME:**
      `ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l` → **0**, unchanged by
      a full suite run.
- [ ] Full lib suite **1776 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1766**). *Arithmetic: 1766 + 10 = 1776.*
- [ ] All four gates pass.

## Test plan

**`src/daemon/run/resume.rs` (10 new).** Nine are pure — they build
`Vec<TurnLogRecord>` by hand and need no HOME, no tempdir, no tokio. The tenth
writes a real log, so it takes `crate::test_home_guard()` + a tempdir: reuse
`crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id}`, as
`daemon::ghost`'s `ghost_turn_log_records_a_turn_started_for_each_turn` already
does. **The turn log is keyed by run id and libtest runs in parallel — the
tenth test must use its own id.**

`terminal_outcome`:

- `terminal_outcome_returns_the_kind_and_the_hard_fail_briefing`
- `terminal_outcome_returns_the_budget_exceeded_briefing`
- `terminal_outcome_on_a_complete_run_has_no_briefing` — `Ok` with `None`, not
  an error: a completed run is resumable on the directive alone.
- `terminal_outcome_on_a_cancelled_run_has_no_briefing` — same, for
  `Cancelled { .. }`.
- `terminal_outcome_on_an_empty_log_is_no_run_log` — **negative case.**
- `terminal_outcome_without_a_run_started_is_no_run_log` — **negative case**:
  a log holding only `TurnStarted`/`Note` records cannot name a kind.
- `terminal_outcome_without_a_run_finished_is_still_running` — **negative
  case**: `RunStarted` + `TurnStarted` only.
- `terminal_outcome_takes_the_first_start_and_the_last_finish` — two of each in
  one log; assert the kind is the **first** and the briefing the **last**.

`render_seed`:

- `render_seed_embeds_the_briefing_and_the_directive` — assert the rendered
  text contains the briefing's blocker text, the directive, and the runbook
  name from a `RunKind::Ghost`; and assert it contains **`# Directive`**.
- `render_seed_without_a_briefing_says_so_and_still_carries_the_directive` —
  **negative case**: `None` briefing renders the literal placeholder, and an
  **empty** directive renders `(none given)` — assert both in one test, since
  both are the "empty section still renders" rule.

`resume_seed` (the HOME-touching one — fold it into the count above by
replacing one pure test only if you must; otherwise this is an eleventh and the
criterion's "10 or more" still holds):

- `resume_seed_reads_a_real_log_and_returns_the_seed` — write `RunStarted` +
  `RunFinished(HardFail)` for a fresh id through `append_turn_event`, then
  `resume_seed(&id, "try the other mount")`; assert the returned text carries
  the blocker and the directive. *This is the only test that proves the I/O
  wrapper is wired to the pure halves.*

## Mutation checks

Two. Use the `patch` tool to break and revert — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.**
Record the red-then-green transcript for each, naming **the assertion message
in the failure output**. **If a mutation does not turn its test red, record
that observation instead — do not invent a failure.**

1. **A running run must not be resumable.** Make the missing-`RunFinished`
   case return `Ok((kind, None))` instead of `Err(StillRunning)` →
   `terminal_outcome_without_a_run_finished_is_still_running` must go **red**.
2. **The directive must survive an empty briefing.** In `render_seed`, return
   early with just the briefing section when `briefing` is `None` →
   `render_seed_without_a_briefing_says_so_and_still_carries_the_directive`
   must go **red** on its directive assertion.

## End-to-end verification

The real artifact is a `var/log/runs/<id>.jsonl` written by `append_turn_event`
and read back by `resume_seed` — section B exercises exactly that in a
throwaway HOME, through the public API, with no fake in between.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-09a.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the resume module =="
cargo test --lib daemon::run::resume -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::run::resume|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. structural greps (each must print the stated number) =="
P() { awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/resume.rs | grep -c "$1"; }
printf 'resume.rs exists           (1): %s\n' "$(test -f src/daemon/run/resume.rs && echo 1 || echo 0)"
printf 'mod resume;                (1): %s\n' "$(grep -c '^mod resume;' src/daemon/run/mod.rs)"
printf 'pub use resume::           (1): %s\n' "$(grep -c 'pub use resume::' src/daemon/run/mod.rs)"
printf 'prod terminal_outcome      (1): %s\n' "$(P 'pub fn terminal_outcome')"
printf 'prod render_seed           (1): %s\n' "$(P 'pub fn render_seed')"
printf 'prod resume_seed           (1): %s\n' "$(P 'pub fn resume_seed')"
printf 'prod ResumeError enum      (1): %s\n' "$(P 'pub enum ResumeError')"
printf 'seed cannot see records    (0): %s\n' "$(sed -n '/pub fn render_seed/,/^}$/p' src/daemon/run/resume.rs | grep -c 'TurnLogRecord')"
printf 'render_seed takes briefing (1): %s\n' "$(sed -n '/pub fn render_seed/,/-> String/p' src/daemon/run/resume.rs | grep -c 'Option<&Briefing>')"
printf 'terminal_outcome no I/O    (0): %s\n' "$(sed -n '/pub fn terminal_outcome/,/^}$/p' src/daemon/run/resume.rs | grep -cE 'read_turn_log|std::fs|File::')"
printf 'prod read_turn_log         (1): %s\n' "$(P 'read_turn_log')"
printf 'no G4 briefing here        (0): %s\n' "$(grep -c 'daemon::briefing' src/daemon/run/resume.rs)"
printf 'no G4 briefing in run/     (0): %s\n' "$(grep -rc 'daemon::briefing' src/daemon/run/ --include='*.rs' | grep -v ':0' | wc -l)"
printf 'prod .render() reused      (1): %s\n' "$(P '.render()')"
printf 'prod no section copy       (0): %s\n' "$(P '# Goal')"
printf 'prod NoRunLog            (>=2): %s\n' "$(P 'ResumeError::NoRunLog')"
printf 'prod StillRunning        (>=2): %s\n' "$(P 'ResumeError::StillRunning')"
printf 'prod unwrap/panic          (0): %s\n' "$(P '\.\(unwrap\|expect\)(\|panic!(\|unsafe')"
printf 'nothing outside run/       (0): %s\n' "$(git diff --stat -- src/ ':!src/daemon/run/' | wc -l)"

echo "== D. preservation: turn_log 7, log_read 11, result 11, guard 8, active 6, cancel 13, scheduled 7, ghost 12 =="
for m in daemon::run::turn_log daemon::run::log_read daemon::run::result daemon::run::guard daemon::run::active daemon::run::cancel daemon::scheduled daemon::ghost; do
  printf '%-26s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== E. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== F. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== G. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then, as the last task, the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-09a-resume-seed.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-09a.txt
diff /tmp/pasted-m24-09a.txt /tmp/e2e-m24-09a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/run/resume.rs` — new file, all tasks and the Test plan.
- `src/daemon/run/mod.rs` — task 4 **only**: the `mod resume;` line and one
  `pub use` line. Nothing else in that file.
- `docs/dev/milestones/M24-run-manager/phase-09a-resume-seed.md` — the Update
  Log.

No other file — a criterion asserts a zero diff across all of `src/` outside
`src/daemon/run/`. In particular do not touch `src/main.rs`, `src/cli/`,
`src/ipc.rs`, `src/daemon/ghost.rs`, `src/daemon/briefing.rs` or
`src/runbook.rs`; the CLI and IPC surface is phase-09b's.

## Out of scope

- **The `daemoneye run resume` CLI command, its IPC request, and actually
  restarting a run.** Phase-09b. This phase builds the seed text and stops;
  nothing calls `resume_seed` in production yet, which is why the
  Authorizations forbid touching `src/main.rs` and `src/cli/`.
- **Seeded tasks from a runbook `## Steps` section** and the completion
  coverage check. That is the milestone's other resume-adjacent exit criterion
  and gets its own phase — no `## Steps` parsing exists anywhere in the tree
  today (verified at drafting).
- **The G4 agent briefing** (`src/daemon/briefing.rs`). Different subject,
  different type; a criterion asserts it is not referenced from
  `src/daemon/run/`.
- **`await_agent_result` → re-spawn using resume.** The exit criterion names it;
  it needs 09b's spawn path first.
- **Rendering a `RunResult`** rather than a `Briefing`. The seed wants the
  briefing's six sections; `RunResult` carries status/turns/budget a resumed
  run does not need.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
