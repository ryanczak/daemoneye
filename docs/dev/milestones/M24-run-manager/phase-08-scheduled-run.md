# Phase 08: a scheduled job is a Run

**Milestone:** M24 — Run Manager
**Status:** todo
**Depends on:** phase-01 (`RunId`/`RunKind`/`RunState`/`Briefing`), phase-02a
(the turn log) and phase-07a (`RunLogGuard` with a kind-aware missed-exit goal).
All **done**.
**Estimated diff:** ~260 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`RunKind::Scheduled { job }` has existed since phase-01 and **no production
code has ever constructed one**. This phase closes that: every scheduled job
becomes a Run with its own `RunId` and `var/log/runs/<id>.jsonl`, and its
terminal `RunState` is derived from the outcome the job actually recorded.

After it, the milestone's "One `Run` for four kinds" exit criterion holds for
three of four — chat (06a/06b), ghost (07a/07b) and scheduled. Delegated is
already covered: a ghost spawned by another ghost runs as
`RunKind::Delegated`.

**A scheduled *ghost* job produces two run logs, and that is correct** — a
`Scheduled` run for the job, and the `Ghost` run the job started
(`trigger: "scheduled"`). The job is the parent; the ghost is the work. Do not
try to collapse them.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 "Run manager" — one `Run` model across
  the four execution loops.
- M24 README § exit criteria, "**One `Run` for four kinds.** Chat, ghost,
  scheduled and delegated execution all construct a `Run { kind, … }`; there is
  one turn-log format at `var/log/runs/<id>.jsonl`."

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-11, commit
`99c6e29`):

```sh
git status --porcelain | wc -l                                    # 0
cargo test --lib 2>&1 | grep '^test result'                       # 1759 passed
cargo test --lib daemon::scheduled 2>&1 | grep '^test result'     # 0 passed
cargo test --lib scheduler 2>&1 | grep '^test result'             # 22 passed
cargo test --lib daemon::run::guard 2>&1 | grep '^test result'    # 8 passed
grep -c 'RunLogGuard' src/daemon/scheduled.rs                     # 0
grep -c '#\[cfg(test)\]' src/daemon/scheduled.rs                  # 0
```

**`src/daemon/scheduled.rs` has no test module at all** — this phase adds the
first one.

## Current state

### `run_scheduled_job` — 435 lines, seven returns, eleven outcome sites

`src/daemon/scheduled.rs:27`. Its shape:

```rust
pub async fn run_scheduled_job(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    session: String,
    sessions: SessionStore,
    config: Config,
    cache: Arc<SessionCache>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
```

It returns `()`, not `Result`, and records its outcome by calling
`store.mark_done(&job.id, success, error_msg)` — **eleven call sites**
(`:54, :74, :102, :134, :146, :169, :190, :203, :234, :295, :410`), spread
across a ghost branch that returns early, an alert branch that returns early,
and the script/command branch that drives a tmux `de-sj-*` window.

**Do not rewrite those eleven sites**, and do not restructure the body. The
outcome is already durable in the store — this phase reads it back.

### The guard, and why it fits here too

`RunLogGuard` (`src/daemon/run/guard.rs:25-47`) writes `RunStarted` on
construction and exactly one `RunFinished` on drop, recording a `HardFail`
naming the missed path when nothing was set. Phase-07a made its missed-exit
goal kind-aware, so a scheduled run's forgotten exit already reads
`"one scheduled run"`:

```rust
// src/daemon/run/guard.rs — Drop, after phase-07a
        let label = self.kind_label;
        let state = self.state.take().unwrap_or_else(|| {
            RunState::HardFail(super::Briefing::new(
                format!("one {label} run"),
                …
                Some(UNRECORDED_BLOCKER.to_string()),
```

The same reasoning that put the guard in `trigger_ghost_turn`'s 16-line wrapper
rather than its 760-line body applies here: wrap, don't thread.

### Reading the outcome back

`ScheduleStore::list()` (`src/scheduler.rs:348`) returns `Vec<ScheduledJob>`;
each carries `status: JobStatus` (`src/scheduler.rs:201`). The enum
(`src/scheduler.rs:162`):

```rust
pub enum JobStatus {
    Pending,            // waiting to fire
    Running,            // currently executing
    Succeeded,          // completed successfully (for `Every` jobs this transitions back to `Pending`)
    Failed(String),     // failed; the tmux window is left open for inspection
    Cancelled,          // cancelled by the user
}
```

**The load-bearing gotcha, quoted from that enum's own doc comment:** a
successful **`Every`** job does *not* read back as `Succeeded` — `mark_done`
advances it and it returns to **`Pending`**. A mapping that treats `Pending` as
"never finished" would report every successful recurring job as a failure. This
is the single most likely way to get this phase wrong, and the Test plan pins it
as a named case.

## Spec

Four tasks.

### Task 1 — `scheduled_terminal_state`: a pure mapping

New in `src/daemon/scheduled.rs`:

```rust
/// The terminal [`RunState`] a scheduled job's run records, from the status
/// the store holds after the job ran.
fn scheduled_terminal_state(status: Option<&JobStatus>, job_name: &str) -> RunState
```

Rules:

- `Some(JobStatus::Succeeded)` → `RunState::Complete`.
- `Some(JobStatus::Pending)` → `RunState::Complete`. **An `Every` job that
  succeeded and rescheduled itself lands here** — see § Current state.
- `Some(JobStatus::Failed(msg))` → `RunState::HardFail(Briefing::new(...))`
  with goal `format!("scheduled job {job_name}")` and blocker
  `Some(msg.clone())`; criteria/working files/attempts empty, `budget_left`
  `None`.
- `Some(JobStatus::Cancelled)` → `RunState::Cancelled { stage:
  "job_cancelled".into(), turns: 0 }`. A scheduled job has no turns.
- `Some(JobStatus::Running)` → `RunState::HardFail(...)` with blocker
  `Some("the job never recorded an outcome".into())` — the job exited without
  ever calling `mark_done`, which is a bug worth seeing in the log rather than
  silence.
- `None` (the job is no longer in the store — deleted while it ran) →
  `RunState::HardFail(...)` with blocker
  `Some("the job left the store before it finished".into())`.

Take `Option<&JobStatus>` and `&str`, **not** the store — the function stays
pure and its tests need no filesystem.

### Task 2 — rename the body, wrap it

Rename `pub async fn run_scheduled_job` to
`async fn run_scheduled_job_inner` (**private**). Its body is otherwise
**byte-identical** — same parameters, same eleven `mark_done` sites, same
returns.

Add the new public wrapper with the **same signature the old function had**, so
`src/daemon/mod.rs:116`'s `pub use scheduled::run_scheduled_job;` and the call
site at `src/daemon/mod.rs:866` keep working unchanged:

- capture `let job_name = job.name.clone();` and `let job_id = job.id.clone();`
  **before** `job` is moved into the inner call;
- `let mut run_log = RunLogGuard::start(RunId::new(), RunKind::Scheduled { job: job_name.clone() });`
  — the **name**, not the id: the id is already the run's own `RunId`, and
  `RunKind::Ghost` and `RunKind::Chat` both carry human-readable labels;
- `run_scheduled_job_inner(job, store.clone(), …).await;`
- read the status back and finish:

```rust
    let status = store.list().into_iter().find(|j| j.id == job_id).map(|j| j.status);
    run_log.finish(scheduled_terminal_state(status.as_ref(), &job_name));
```

`store` is an `Arc<ScheduleStore>`; clone it for the inner call rather than
re-ordering the existing parameter moves.

### Task 3 — the imports

`src/daemon/scheduled.rs` needs `RunId`, `RunKind`, `RunLogGuard`, `RunState`
and `Briefing` from `crate::daemon::run`, and `JobStatus` from
`crate::scheduler`. The file already does `use crate::scheduler::{ActionOn,
ScheduleStore, ScheduledJob};` — extend that list rather than adding a second
`use` line for the same module.

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-08.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

**On mutation-check transcripts** (this milestone bounced twice on exactly
this): record only what you actually observed. If a mutation does **not** make
its test fail, say so plainly and say why — that is an accepted, useful
outcome. Never write a transcript for a run you did not make, and never fill in
a predicted failure message from this document.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] **The mapping exists and is pure:**
      `grep -c 'fn scheduled_terminal_state' src/daemon/scheduled.rs` → **1**
      (now `0`), and
      `sed -n '/fn scheduled_terminal_state/,/-> RunState/p' src/daemon/scheduled.rs | grep -c 'ScheduleStore'`
      → **0**.
- [ ] **The body was wrapped, not rewritten:**
      `grep -c 'async fn run_scheduled_job_inner' src/daemon/scheduled.rs` →
      **1** (now `0`);
      `grep -c 'pub async fn run_scheduled_job' src/daemon/scheduled.rs` → **1**,
      unchanged (now `1`); and the eleven outcome sites are untouched —
      `grep -c 'mark_done' src/daemon/scheduled.rs` → **11**, unchanged (now
      `11`).
- [ ] **The scheduled run is constructed** (production only, `awk` prefix
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs |`):
      `grep -c 'RunKind::Scheduled'` → **1**;
      `grep -c 'RunLogGuard::start'` → **1**;
      `grep -c 'RunId::new()'` → **1** (each **0** now).
- [ ] **`RunKind::Scheduled` finally has a production constructor anywhere in
      the tree:**
      `grep -rl 'RunKind::Scheduled' src/ --include='*.rs' | grep -v '^src/daemon/run/' | wc -l`
      → **1** (now **0**), and that file is `src/daemon/scheduled.rs`.
      *(`src/daemon/run/` is excluded because the variant's definition, its
      `label()` arm, its `log_read` rendering and their tests all live there and
      are not constructors.)*
- [ ] **The wrapper's public signature did not change:**
      `grep -c 'pub use scheduled::run_scheduled_job;' src/daemon/mod.rs` →
      **1**, unchanged (now `1`), and
      `git diff --stat -- src/daemon/mod.rs | wc -l` → **0**.
- [ ] **The scheduler itself is untouched:**
      `git diff --stat -- src/scheduler.rs | wc -l` → **0**.
- [ ] **All five `JobStatus` variants are mapped** — production only, same
      `awk` prefix: `grep -c 'JobStatus::'` → **5 or more**.
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` in production code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0**, unchanged (now **0**).
- [ ] `cargo test --lib daemon::scheduled` → **7 or more** passing, `0 failed`
      (now **0** — the module has no tests at all). *Arithmetic: the 7 in the
      Test plan.*
- [ ] **Preservation, each must not move:** `scheduler` **22**,
      `daemon::ghost` **12**, `daemon::run::guard` **8**,
      `daemon::run::log_read` **11**, `daemon::run::turn_log` **7**,
      `daemon::run::result` **11**, `agents::mailbox` **7**,
      `daemon::stream` **8**.
- [ ] **Nothing leaks into the developer's real HOME:**
      `ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l` → **0**, unchanged by
      a full suite run.
- [ ] Full lib suite **1766 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1759**). *Arithmetic: 1759 + 7 = 1766.*
- [ ] All four gates pass.

## Test plan

**`src/daemon/scheduled.rs` (7 new — the module's first test module).** All
seven are pure `scheduled_terminal_state` cases: no HOME, no tempdir, no store,
no tokio. Do **not** write a test that calls `run_scheduled_job` — it drives
tmux.

- `scheduled_state_succeeded_is_complete`
- `scheduled_state_pending_is_complete_because_every_jobs_reschedule` — **the
  gotcha case.** Assert `Pending` → `Complete`, and say in the test's assertion
  message that an `Every` job returns to `Pending` on success.
- `scheduled_state_failed_carries_the_error_as_the_blocker` — assert the
  briefing's `blocker` is exactly the `Failed(msg)` string, and that `goal`
  names the job.
- `scheduled_state_cancelled_reports_zero_turns` — `Cancelled { stage, turns }`
  with `turns == 0` and the stage naming the job cancellation.
- `scheduled_state_running_is_a_hard_fail` — **negative case**: a job still
  `Running` when the function is reached never recorded an outcome; assert
  `HardFail` with a blocker saying so.
- `scheduled_state_missing_job_is_a_hard_fail` — **negative case**: `None`
  (deleted mid-run) is a `HardFail`, **not** `Complete`. Assert on the blocker.
- `scheduled_state_failed_and_missing_have_different_blockers` — the two
  `HardFail` paths must stay distinguishable; assert their blockers differ.

## Mutation checks

Two. Use the `patch` tool to break and revert — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.**
Record the red-then-green transcript for each in the Update Log, naming **the
assertion message in the failure output**, not just the test. **If a mutation
does not turn its test red, record that observation instead — do not invent a
failure.**

1. **A rescheduled `Every` job is not a failure.** Map
   `JobStatus::Pending` to a `HardFail` instead of `Complete` →
   `scheduled_state_pending_is_complete_because_every_jobs_reschedule` must go
   **red** showing `HardFail` where `Complete` was expected.
2. **A job that vanished is not a success.** Map the `None` arm to
   `RunState::Complete` → `scheduled_state_missing_job_is_a_hard_fail` must go
   **red** on its blocker assertion.

## End-to-end verification

The real artifact is `var/log/runs/<id>.jsonl` — a scheduled run's turn log
carrying `RunStarted { kind: Scheduled { job } }`. Section B exercises the
mapping; section C writes and reads a real `Scheduled` turn log through the
public reader in a throwaway HOME, proving the variant round-trips the log
format the milestone's exit criterion names.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-08.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the scheduled module (first tests it has ever had) =="
cargo test --lib daemon::scheduled -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::scheduled|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. structural greps (each must print the stated number) =="
P() { awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -c "$1"; }
printf 'scheduled_terminal_state   (1): %s\n' "$(grep -c 'fn scheduled_terminal_state' src/daemon/scheduled.rs)"
printf 'mapping is pure            (0): %s\n' "$(sed -n '/fn scheduled_terminal_state/,/-> RunState/p' src/daemon/scheduled.rs | grep -c 'ScheduleStore')"
printf 'inner fn exists            (1): %s\n' "$(grep -c 'async fn run_scheduled_job_inner' src/daemon/scheduled.rs)"
printf 'public wrapper kept        (1): %s\n' "$(grep -c 'pub async fn run_scheduled_job' src/daemon/scheduled.rs)"
printf 'mark_done sites untouched (11): %s\n' "$(grep -c 'mark_done' src/daemon/scheduled.rs)"
printf 'prod RunKind::Scheduled    (1): %s\n' "$(P 'RunKind::Scheduled')"
printf 'prod RunLogGuard::start    (1): %s\n' "$(P 'RunLogGuard::start')"
printf 'prod RunId::new()          (1): %s\n' "$(P 'RunId::new()')"
printf 'prod JobStatus arms      (>=5): %s\n' "$(P 'JobStatus::')"
printf 'constructor files          (1): %s\n' "$(grep -rl 'RunKind::Scheduled' src/ --include='*.rs' | grep -v '^src/daemon/run/' | wc -l)"
printf 'mod.rs re-export kept      (1): %s\n' "$(grep -c 'pub use scheduled::run_scheduled_job;' src/daemon/mod.rs)"
printf 'mod.rs untouched           (0): %s\n' "$(git diff --stat -- src/daemon/mod.rs | wc -l)"
printf 'scheduler.rs untouched     (0): %s\n' "$(git diff --stat -- src/scheduler.rs | wc -l)"
printf 'prod unwrap/panic          (0): %s\n' "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"

echo "== D. preservation: scheduler 22, ghost 12, guard 8, log_read 11, turn_log 7, result 11, mailbox 7, stream 8 =="
for m in scheduler daemon::ghost daemon::run::guard daemon::run::log_read daemon::run::turn_log daemon::run::result agents::mailbox daemon::stream; do
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
D=docs/dev/milestones/M24-run-manager/phase-08-scheduled-run.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-08.txt
diff /tmp/pasted-m24-08.txt /tmp/e2e-m24-08.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/scheduled.rs` — all four tasks and the Test plan.
- `docs/dev/milestones/M24-run-manager/phase-08-scheduled-run.md` — the Update
  Log.

**No other file**, and two criteria assert zero diffs on `src/daemon/mod.rs`
and `src/scheduler.rs`. In particular do not touch `src/daemon/run/` (any
file), `src/daemon/ghost.rs`, `src/agents/mailbox.rs` or `src/daemon/stream.rs`.

## Out of scope

- **Cancelling a scheduled job.** `daemoneye stop <run>` does not reach one, and
  this phase does not register the run in the cancel registry. Two reasons, both
  worth stating: the script branch's work happens inside a tmux `de-sj-*` window
  that the 2.0 substrate is replacing, so cancellation logic written against it
  would be thrown away; and the branch that actually runs a long AI loop — a
  scheduled **ghost** job — is *already* cancellable through its own `Ghost` run
  (phase-07b). The residual gap is a synchronous script, which ends on its own.
  Note this in the Update Log; do not implement it.
- **Collapsing the two run logs** a scheduled ghost job produces. A `Scheduled`
  parent and a `Ghost` child is the intended shape — see § Goal.
- **Rewriting the eleven `mark_done` sites** or restructuring the 435-line body.
  The store is already the durable record; this phase reads it.
- **`Budget` accounting.** A scheduled run records no budget — M25.
- **Retiring `ActionOn::Command`.** Deprecated, still loaded for
  backwards-compatibility, out of scope here.
- **Surfacing scheduled runs** in `daemoneye runs`. Phase-10.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
