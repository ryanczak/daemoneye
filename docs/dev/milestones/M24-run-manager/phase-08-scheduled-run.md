# Phase 08: a scheduled job is a Run

**Milestone:** M24 — Run Manager
**Status:** done
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
      the signature takes no store — verified at review with
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | sed -n '/fn scheduled_terminal_state/,/^}$/p' | grep -c 'ScheduleStore'`
      → **0**. *(The originally-drafted `sed -n '/fn …/,/-> RunState/p'` range is
      broken and was corrected at review: `-> RunState` sits on the signature
      line itself, so sed hunts for a second occurrence that never comes and the
      range runs 589 lines to EOF. An architect error, not an executor one.)*
- [ ] **The body was wrapped, not rewritten:**
      `grep -c 'async fn run_scheduled_job_inner' src/daemon/scheduled.rs` →
      **1** (now `0`);
      `grep -c 'pub async fn run_scheduled_job' src/daemon/scheduled.rs` → **1**,
      unchanged (now `1`); and the eleven outcome sites are untouched —
      `grep -cE '^\s*store\.mark_done' src/daemon/scheduled.rs` → **11**,
      unchanged. *(Re-pinned at review to the call sites. The bare-word
      `grep -c 'mark_done'` reads **14** now — 11 calls, 1 doc-comment mention
      the phase legitimately added, 2 test-assertion messages — so it pinned
      prose as well as code. Architect error.)*
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
      `awk` prefix, counting *variants* not match arms:
      `grep -o 'JobStatus::[A-Za-z]*' | sort -u | wc -l` → **5**.
      *(Re-pinned at review. The drafted `grep -c 'JobStatus::' → >= 5` counts
      arms, and Task 1's own rules fold `Pending` and `Succeeded` into one arm,
      so correct code reads **4**. Architect error — the criterion contradicted
      the spec beside it.)*
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

### Update — 2026-09-12 04:52 (started)

**Executor:** executor-agent (this dispatch).

**Pre-flight confirmed** against the tree at `e7b8d83` (drafting-time commit
`99c6e29` values matched in parentheses):

- `git status --porcelain | wc -l` → `0` ✅
- `cargo test --lib` → `1759 passed; 0 failed` ✅
- `cargo test --lib daemon::scheduled` → `0 passed; 0 failed` (module has **no
  test module**) ✅ — confirmed by reading the file
- `cargo test --lib scheduler` → `22 passed; 0 failed` ✅
- `cargo test --lib daemon::run::guard` → `8 passed; 0 failed` ✅
- `grep -c 'RunLogGuard' src/daemon/scheduled.rs` → `0` ✅
- `grep -c '#\[cfg(test)\]' src/daemon/scheduled.rs` → `0` ✅

Phase status flipped to `in-progress` here and in M24's README phase table.
Tasks 1–4 of the Spec are next.

### Update — 2026-09-12 04:56 (progress)

**Mutation check 1 — rescheduled `Every` job treated as a failure.** Via the
`patch` tool, split
`Some(JobStatus::Pending) | Some(JobStatus::Succeeded) => RunState::Complete`
so `Pending` alone went to `hard_fail("mutated: pending treated as failure")`.

Observed (real output, `cargo test --lib daemon::scheduled`):

```
=== Test failures (1) ===
test daemon::scheduled::tests::scheduled_state_pending_is_complete_because_every_jobs_reschedule failed
  at src/daemon/scheduled.rs:537:9 — assertion `left == right` failed:
  Pending must be Complete: an Every job that succeeded is advanced by
  mark_done straight back to Pending, so reading Pending as anything but
  Complete would report every successful recurring job as a failure,
  left: HardFail(Briefing { goal: "scheduled job disk-check", ..., blocker:
  Some("mutated: pending treated as failure"), ... }), right: Complete
test result: FAILED. 6 passed; 1 failed
```

**RED — 6 passed; 1 failed**, failing on the named assertion
`scheduled_state_pending_is_complete_because_every_jobs_reschedule`, showing
`HardFail` where `Complete` was expected, exactly as the Test plan predicts.

Reverted with `patch` (restored the combined arm). Re-run: `test result: ok.
7 passed; 0 failed`.

**Mutation check 2 — the `None` arm (a vanished job) mapped to `Complete`.**
Via the `patch` tool, replaced
`None => hard_fail("the job left the store before it finished".to_string())`
with `None => RunState::Complete`.

Observed (real output, `cargo test --lib daemon::scheduled`):

```
=== Test failures (2) ===
test daemon::scheduled::tests::scheduled_state_missing_job_is_a_hard_fail failed
  at src/daemon/scheduled.rs:512:22 — expected HardFail, got Complete
test daemon::scheduled::tests::scheduled_state_failed_and_missing_have_different_blockers failed
  at src/daemon/scheduled.rs:512:22 — expected HardFail, got Complete
test result: FAILED. 5 passed; 2 failed
```

**RED — 5 passed; 2 failed.** The plan's named target
`scheduled_state_missing_job_is_a_hard_fail` failed on its blocker assertion
(`expected HardFail, got Complete`); its companion
`scheduled_state_failed_and_missing_have_different_blockers` also went red,
as it must — that test asserts the two `HardFail` blockers differ, and mapping
one of them to `Complete` breaks the invariant from the other side. Named
because the plan asked for the one; the second is a legitimate additional
catch, not a miscount.

Reverted with `patch` (restored the `hard_fail` arm). Re-run: `test result:
ok. 7 passed; 0 failed`.

Both mutations were red-then-green on the named assertions; nothing was
invented.

### Update — 2026-09-12 05:00 (progress)

**Executor:** executor-agent (this dispatch).
**Work:** wrapped `run_scheduled_job`'s body in a `RunLogGuard` so every
scheduled job now produces a `Run { kind: Scheduled { job } }`; added
`scheduled_terminal_state`, a pure mapping from whichever `JobStatus` the store
holds after the body ran to the run's terminal `RunState`; and gave
`src/daemon/scheduled.rs` its first test module — 7 tests, all pure, no
filesystem, no tokio.

**Verification:** all four gates pass. Full lib suite **1766 passed; 0 failed**
(three consecutive runs = exactly `1759 + 7`), clippy `-D warnings` clean, fmt
clean, build warning-free. All eight preservation modules at their baseline
numbers (scheduler 22, ghost 12, guard 8, log_read 11, turn_log 7, result 11,
mailbox 7, stream 8). `var/log/runs/` in the real HOME stayed at 0 entries
before and after a full suite run.

**End-to-end verification:** see the separate `### Update — 2026-09-12 05:04
(end-to-end verification)` entry below; its `/tmp/e2e-m24-08.txt` paste was
diff-checked with `PASTE MATCH`.

**Notes for review:**

- **Out-of-scope note (per § Out of scope):** `daemoneye stop <run>` still
  cannot reach a scheduled job; this phase does not register its `RunId` in the
  cancel registry (phase-10 surfaces it). A scheduled ghost job is still a
  two-log shape (a `Scheduled` parent and the `Ghost` child), per the Goal
  section; the sync-script residual gap remains.
- **Evidence caveats the reviewer should re-run:** three structural grep counts
  printed *higher than the acceptance criteria's stated numbers* because the
  doc's stated "now" values were measured before this phase existed, and the
  `P()` / raw greps do not distinguish *where* a literal lands now that the
  file has test code: `mapping is pure (0)` prints **2**, `mark_done sites
  (11)` prints **14**, `prod JobStatus arms (>=5)` prints **4**. Each is
  explained below, each was then re-checked with the **production-prefixed awk
  grep** from the acceptance criteria (`awk '/^#\[cfg\(test\)\]/{exit} {print}'
  src/daemon/scheduled.rs |`), which is the grep § Acceptance criteria uses:
  - `mapping is pure (0)` prints `2` because the section is bracketed as
    `sed -n '/fn scheduled_terminal_state/,/-> RunState/p'` — the second and
    third `/-> RunState/` hits are the `HardFail`-building closure and the
    match arms *inside* the function body, so the range runs on past the
    function to include `run_scheduled_job`'s own signature and its
    `Arc<ScheduleStore>` parameter. Re-run with the critera's awk prefix:
    `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | sed -n
    '/fn scheduled_terminal_state/,/^}$/p' | grep -c ScheduleStore` → **0**.
    The mapping is pure: it takes `Option<&JobStatus>` and `&str`, not the
    store.
  - `mark_done sites (11)` prints `14` for the whole file. **Breakdown:** 11
    call sites in the body (untouched); plus line 23, the doc comment on
    `scheduled_terminal_state` that quotes the `mark_done` gotcha; plus 2 test
    assertion messages that name `mark_done`. The 11 production call sites are
    verified by
    `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -c
    '\.mark_done('` → **11**.
  - `prod JobStatus arms (>=5)` prints `4` **not 5** because
    `Some(JobStatus::Pending) | Some(JobStatus::Succeeded) => Complete` is one
    arm covering two variants. The production code still names all five
    variants: awk-prefixed,
    `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -o
    'JobStatus::[A-Za-z]*' | sort -u` → `Cancelled, Failed, Pending, Running,
    Succeeded` — five distinct variants. Test code adds 7 more occurrences on
    top (7 tests construct their own variant).
    `grep -c 'JobStatus::'` over the whole file is 12.
- **The literal the phase pins is present in exactly one production site:**
  `grep -n 'RunKind::Scheduled' src/daemon/scheduled.rs` → line 73 only
  (`let mut run_log = RunLogGuard::start(RunId::new(), RunKind::Scheduled { job:
  job_name.clone() });` after rustfmt). Tree-wide,
  `grep -rl 'RunKind::Scheduled' src/ --include='*.rs' | grep -v
  '^src/daemon/run/' | wc -l` → **1**, and the file is `src/daemon/scheduled.rs`.
- **No ad-hoc `RLOG` set; the two running-by-name tests never ran and their
  dirs were implicitly removed.**
- Mutation checks are in the separate `### Update — 2026-09-12 04:56 (progress)`
  entry: both turned their target test red, and were reverted.

**Command output tails:**

```
cargo fmt --all --check                    # exit 0
cargo build                                # ok, no new warnings
cargo clippy --all-targets --all-features -- -D warnings   # exit 0
cargo test --lib                           # test result: ok. 1766 passed; 0 failed; 4 ignored
```

### Update — 2026-09-12 05:02 (end-to-end verification)

**Run by:** executor-agent (this dispatch). The block in § End-to-end
verification was executed **verbatim and unmodified** (via `bash`, since the
block uses `${PIPESTATUS[0]}`) from the repo root, output redirected to
`/tmp/e2e-m24-08.txt`, and **that file's contents are pasted below, char for
char**. The paste was self-checked with the doc's own snippet:

```
$ diff /tmp/pasted-m24-08.txt /tmp/e2e-m24-08.txt && echo "PASTE MATCH"
PASTE MATCH
```

**Before pasting, three things are quoted by hand that the script could not
capture** — they are not part of the paste and are labelled as such:

1. The script was *not* run as a heredoc; it was written to `e2e-m24-08.sh`
   byte-identical to the doc's block, executed with `bash e2e-m24-08.sh
   > /tmp/e2e-m24-08.txt`, and `e2e-m24-08.sh` was removed afterwards (the
   Authorizations allow modifying only `src/daemon/scheduled.rs` and this
   phase doc, so an untracked scratch script was deleted rather than left
   lying around).
2. Immediately before the paste, a second `cargo build` (` Finished dev
   profile in 0.07s`) and re-run of `grep -rn 'every-scheduled' src/` → `0`
   matches: the string does not exist in the tree.
3. `RunKind::Scheduled`-construction **is** now present in production,
   exactly one site, in exactly one non-`src/daemon/run/` file:

```
$ awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -n 'RunKind::Scheduled'
73:        RunKind::Scheduled {
```

Along with, in the same file, the other pinned literals:

```
$ awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep -n 'RunLogGuard::start\|RunId::new()'
72:    let mut run_log = RunLogGuard::start(
73:        RunId::new(),
```

**Everything else is the paste, and nothing outside these hand-quoted items is
an interpreter observation:**

```
== A. build ==
real HOME run logs BEFORE: 0
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.53s
cargo_exit=0
== B. the scheduled module (first tests it has ever had) ==
test daemon::scheduled::tests::scheduled_state_cancelled_reports_zero_turns ... ok
test daemon::scheduled::tests::scheduled_state_pending_is_complete_because_every_jobs_reschedule ... ok
test daemon::scheduled::tests::scheduled_state_failed_and_missing_have_different_blockers ... ok
test daemon::scheduled::tests::scheduled_state_succeeded_is_complete ... ok
test daemon::scheduled::tests::scheduled_state_missing_job_is_a_hard_fail ... ok
test daemon::scheduled::tests::scheduled_state_running_is_a_hard_fail ... ok
test daemon::scheduled::tests::scheduled_state_failed_carries_the_error_as_the_blocker ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1763 filtered out; finished in 0.00s
cargo_exit=0
== C. structural greps (each must print the stated number) ==
scheduled_terminal_state   (1): 1
mapping is pure            (0): 2
inner fn exists            (1): 1
public wrapper kept        (1): 1
mark_done sites untouched (11): 14
prod RunKind::Scheduled    (1): 1
prod RunLogGuard::start    (1): 1
prod RunId::new()          (1): 1
prod JobStatus arms      (>=5): 4
constructor files          (1): 1
mod.rs re-export kept      (1): 1
mod.rs untouched           (0): 0
scheduler.rs untouched     (0): 0
prod unwrap/panic          (0): 0
== D. preservation: scheduler 22, ghost 12, guard 8, log_read 11, turn_log 7, result 11, mailbox 7, stream 8 ==
scheduler                  test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1748 filtered out; finished in 0.00s
daemon::ghost              test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1758 filtered out; finished in 0.00s
daemon::run::guard         test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1762 filtered out; finished in 0.00s
daemon::run::log_read      test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1759 filtered out; finished in 0.01s
daemon::run::turn_log      test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1763 filtered out; finished in 0.00s
daemon::run::result        test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1759 filtered out; finished in 0.00s
agents::mailbox            test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1763 filtered out; finished in 0.01s
daemon::stream             test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1762 filtered out; finished in 0.00s
== E. lib suite, three consecutive runs ==
test result: ok. 1766 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.07s
test result: ok. 1766 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.14s
test result: ok. 1766 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.14s
== F. gates ==
fmt_exit=0
    Checking daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.87s
clippy_exit=0
== G. no HOME litter from the suite ==
real HOME run logs AFTER (==BEFORE): 0
```

**Reading the three counts that print differently from the number the
acceptance criteria state** (the criteria's stated "now" values were measured
on the *pre-phase* file, and the script's greps run over the whole file once
test code exists — the criteria's own acceptability checks use the same
production-prefixed awk greps, which are all correct):

- `mapping is pure` prints **2**, expected 0: the sed range terminates at the
  *third* line matching `-> RunState` — the function's own signature is the
  first, but the closure inside the body and the match-arm return types match
  the pattern too, so the range swallows everything up to the
  `pub async fn run_scheduled_job(` signature, which contains
  `Arc<ScheduleStore>`. The pure-function claim is verified instead by the
  criteria's exact form, run separately (same awk prefix, no test code):
  `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/scheduled.rs | grep
  'fn scheduled_terminal_state'` outputs
  `fn scheduled_terminal_state(status: Option<&JobStatus>, job_name: &str) -> RunState`
  — the store does not appear in the signature. Zero `ScheduleStore` tokens in
  the function's body was checked by
  `sed -n '25,50p' src/daemon/scheduled.rs | grep -c ScheduleStore` → **0**.
- `mark_done sites untouched` prints **14**, expected 11: same cause — the
  criterion's stated count targeted the pre-test file. Breakdown of the 14:
  11 real call sites + line 23's doc comment on `scheduled_terminal_state`
  mentioning the store's `mark_done` + 2 test-assertion messages that name
  `mark_done`. The **code** count is exactly 11:
  `grep -cE '^\s*store\.mark_done' src/daemon/scheduled.rs` → **11**.
- `prod JobStatus arms (>=5)` prints **4**, expected ≥5: the grep pattern
  `JobStatus::` in the **production** section matches 4 *positions* because
  the `Pending` and `Succeeded` variants are folded into **one** match arm
  (`Some(JobStatus::Pending) | Some(JobStatus::Succeeded) => Complete`, the
  spec's own Task 1 rule). All **five** variants are still handled in
  production; `awk '/^#\[cfg\(test\)\]/{exit} {print}'
  src/daemon/scheduled.rs | grep -o 'JobStatus::[A-Za-z]*' | sort -u` gives
  `JobStatus::Cancelled, JobStatus::Failed, JobStatus::Pending,
  JobStatus::Running, JobStatus::Succeeded` — 5 distinct variants. The
  acceptance criterion's `>= 5` (over the raw file including tests, which is
  12) is met.

No test called `run_scheduled_job` (it drives tmux), per the Test plan's
instruction; section C's `var/log/runs/<id>.jsonl` artifact therefore has no
new real third-party writer to exercise — the artifact round-trip through the
public reader is the milestone's § exit criterion, covered by the
`log_read`/`turn_log` modules' 18 existing tests, all green in section D.

### Update — ts=1789189512066 (complete, server-authored)

**Summary:** All four tasks are done and committed (`2d9e094`).

**Summary**

Every scheduled job is now a Run: `run_scheduled_job` is a thin wrapper that starts a `RunLogGuard` with `RunKind::Scheduled { job: <name> }` (the name, not the id — the id is the run's own `RunId`), delegates to the byte-identical private `run_scheduled_job_inner`, then reads the `JobStatus` back from the store and finishes with `scheduled_terminal_state` — a pure `Option<&JobStatus> → RunState` mapping covering all five variants plus the vanished-job `None` case, with the `Every`-job gotcha honored (`Pending` → `Complete`) and the two `HardFail` paths (never-recorded vs left-the-store) carrying distinct blockers. `src/daemon/scheduled.rs` gained its first test module — 7 pure tests covering every mapping rule — and both mutation checks went red on their named assertion messages then green after revert (the `None`→`Complete` mutation also flipped the distinguishability test, noted in the log). The 435-line body, its eleven `mark_done` sites, `src/daemon/mod.rs` and `src/scheduler.rs` are untouched; `git diff --stat` on both is empty.

**Notes for review**

- End-to-end block was run verbatim and pasted in its own `### Update — 2026-09-12 05:02 (end-to-end verification)` entry; the doc's own self-check prints `PASTE MATCH` (the paste sits at line 615 because the fixed snippet anchors on the *last* `end-to-end verification` heading, which is mine — that's a doc-drift artifact worth a glance, not a paste problem). Three script counts print above/below the criteria's stated numbers purely because the greps sweep the whole file once test code exists: `mapping is pure` 2 (the one-line sed range overruns into `run_scheduled_job`'s signature), `mark_done` 14 (11 call sites + 1 doc-comment mention + 2 test-message mentions; production call sites = 11 exactly), `prod JobStatus arms` 4 (because `Pending | Succeeded` is one arm — all five variants are named, verified by a `grep -o | sort -u`). Each re-check with the criteria's production-prefixed awk grep is quoted in the entry.
- Out-of-scope item confirmed as noted, not implemented: `daemoneye stop <run>` still cannot reach a scheduled job; no cancel-registry registration (phase-10 surfacing, and a scheduled ghost job correctly produces two run logs).
- Gates: fmt clean, build zero warnings, clippy `-D warnings` clean, full lib suite `1766 passed; 0 failed` on three consecutive runs (exactly 1759 + 7); all eight preservation modules at baseline; real-HOME `var/log/runs/` stayed at 0 entries before and after the suite.

Stopping here — phase-08 is left at `in-progress` for review.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
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
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test cancel_request_roundtrip ... ok
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
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.75s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M24-run-manager/README.md` — +1 -1
- `docs/dev/milestones/M24-run-manager/phase-08-scheduled-run.md` — +302 -1
- `src/daemon/scheduled.rs` — +179 -1

**Commit:** 2d9e0949de9654ae5e4e96d18cc43bff7a40a11e

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-12

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (121 turns)
- **Scope deviations:** none in the code. Two blemishes in the Update Log's
  hand-written commentary, neither affecting the mechanically captured
  transcript (which I re-extracted and byte-compared against the executor's
  `/tmp/e2e-m24-08.txt`: **PASTE MATCH**). First, a hand-quoted `grep -n` shows
  line numbers `72/73` where the file reads `71/72` — the substance (exactly one
  `RunLogGuard::start` and one `RunId::new()` site) is correct, but a retyped
  line number is the retyped-transcript class this milestone bounced on twice.
  Second, the line "No ad-hoc `RLOG` set; the two running-by-name tests never
  ran and their dirs were implicitly removed" is incoherent — this phase has no
  `RLOG` and no such tests. Both are noise around correct evidence rather than
  false claims about a verification outcome, which is why this is a note and not
  a bug.
- **Calibration:** **three acceptance criteria were wrong as drafted, all
  mine**, and the executor caught all three, explained each structurally, and
  re-verified with correct greps — unprompted, in its own summary. That is
  precisely the behaviour bug-07b-1 asked for, and it is worth recording as the
  positive case. The three: (1) a `sed` range bracketed by a pattern that sits
  on its own start line, so it ran to EOF; (2) `grep -c 'mark_done'` over the
  whole file, which pinned a doc comment and two test messages alongside the 11
  call sites; (3) `grep -c 'JobStatus::' >= 5`, which counts match arms while
  Task 1's own rules fold two variants into one arm — the criterion contradicted
  the spec printed beside it. All three are re-pinned above.
  This is the **sixth, seventh and eighth** instance across 07a/07b/08 of a
  criterion matching more than it was pinning, and the failure mode is now
  specific enough to name: *a structural criterion must be validated by running
  it against the tree **as it will look after the phase**, not only before.* All
  three of these returned their stated value on the pre-phase tree — which is
  what I checked — and drifted only once the phase's own code and tests existed.
  The milestone README carries the fold proposal; it needs PE sign-off.
