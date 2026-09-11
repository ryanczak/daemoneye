# Phase 07a: a ghost shell is a Run — identity and turn log

**Milestone:** M24 — Run Manager
**Status:** in-progress
**Depends on:** phase-01 (`RunId`/`RunKind`/`RunState`/`Briefing`), phase-02a
(`TurnLogRecord`, `append_turn_event`, `read_turn_log`) and phase-06a
(`RunLogGuard`). All **done**.
**Estimated diff:** ~330 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`trigger_ghost_turn` runs an autonomous AI loop that is, structurally, a Run —
and today it is the only major loop in the daemon that has no `RunId`, writes no
`var/log/runs/<id>.jsonl`, and reports its outcome only as `Result<()>`.

After this phase a ghost shell has a run identity and a turn log:

1. `trigger_ghost_turn` mints one `RunId` and holds a `RunLogGuard`, so every
   exit path writes exactly one `RunFinished`.
2. The run's `RunKind` names what it actually is — `Delegated { agent, parent }`
   for a ghost spawned by another ghost, `Ghost { runbook, trigger }` otherwise.
3. Each ghost turn writes a `TurnStarted` record. **`TurnLogEvent::TurnStarted`
   has no production emitter anywhere in the tree today** — the ghost loop, which
   already counts its turns, becomes the first.
4. The terminal `RunState` distinguishes the three ways a ghost ends: clean
   (`Complete`), turn budget exhausted (`BudgetExceeded`), error (`HardFail`).

Cancellation and the mailbox are **phase-07b**. This phase is purely additive:
nothing that works today changes behaviour.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 — the Run model; a ghost is a run kind,
  not a separate concept.
- M24 README § Phases, row 07 — "`trigger_ghost_turn` onto `Run`; mailbox
  carries `RunResult`". Split here on the additive/rewire seam, the same seam
  that split 06 into 06a/06b (README § Notes).

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-11, commit
`bb6fb96`):

```sh
git status --porcelain | wc -l                                     # 0
cargo test --lib 2>&1 | grep '^test result'                        # 1745 passed
cargo test --lib daemon::ghost 2>&1 | grep '^test result'          # 2 passed
cargo test --lib daemon::run::guard 2>&1 | grep '^test result'     # 7 passed
cargo test --lib daemon::run::turn_log 2>&1 | grep '^test result'  # 7 passed
cargo test --lib daemon::session 2>&1 | grep '^test result'        # 21 passed
cargo test --lib agents::mailbox 2>&1 | grep '^test result'        # 4 passed
grep -c 'RunLogGuard' src/daemon/ghost.rs                          # 0
grep -c 'ghost_runbook_name' src/daemon/session.rs                      # 0
```

## Current state

### `do_ghost_turn` is 760 lines with four bail exits

Measured at drafting: `src/daemon/ghost.rs:304-1063`, exits at `:323`, `:413`,
`:568`, `:890`, plus the implicit end. **This is why the phase uses
`RunLogGuard` rather than hand-placed writes** — the identical reasoning that
made phase-06a a guard for `run_conversation_loop` (1321 lines, six exits). A
hand-wired exit in a function this long is a missed exit, and a missed exit
writes nothing at all.

`trigger_ghost_turn` is the thin wrapper that already owns every exit:

```rust
// src/daemon/ghost.rs:287-302
pub async fn trigger_ghost_turn(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
) -> Result<()> {
    let result = do_ghost_turn(session_id, sessions, config, cache, schedule_store).await;
    write_mailbox_on_exit(session_id, sessions, result.as_ref().err()).await;
    let (sbx, sid) = (config.sandbox.clone(), session_id.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::teardown_ghost_containers(&sbx, &sid)
    })
    .await;
    result
}
```

**This wrapper is where the guard belongs.** It sees the single `Result` every
exit funnels into, so the whole 760-line body stays untouched except for task 5.

### The guard, already shipped

```rust
// src/daemon/run/guard.rs:25-47
impl RunLogGuard {
    pub fn start(run_id: RunId, kind: RunKind) -> Self { /* writes RunStarted */ }
    /// Calling it again **replaces** the recorded outcome: the last call
    /// before drop is what is written.
    pub fn finish(&mut self, state: RunState) { self.state = Some(state); }
    pub fn run_id(&self) -> &RunId { &self.run_id }
}
```

Its `Drop` writes a `HardFail` when nothing was recorded — and hardcodes the
goal:

```rust
// src/daemon/run/guard.rs:51-60
    fn drop(&mut self) {
        let state = self.state.take().unwrap_or_else(|| {
            RunState::HardFail(super::Briefing::new(
                "one chat turn",
                None,
```

`"one chat turn"` is wrong for a ghost. Task 2 fixes it. **No existing test
pins that string** — `guard_drop_without_finish_records_the_missed_outcome`
(`src/daemon/run/guard.rs:149`) asserts only on `blocker`, verified at drafting.

### The ghost turn loop

```rust
// src/daemon/ghost.rs:455-487 (abridged)
        if turn >= max_ghost_turns {
            // Inject a synthetic user message asking the agent to wrap up, then
            // run one final turn so it can leave a clean handoff …
            wrap_up_turn = true;
        }
        turn += 1;
```

`turn` is a `usize` local. `max_ghost_turns` is resolved at `:331-343` from the
runbook frontmatter clamped by the daemon ceiling.

### What identifies a ghost

`SessionEntry` (`src/daemon/session.rs:22`) carries `is_ghost`, `ghost_config`,
`ghost_bg_prefix` and `ghost_task_message` — but **no runbook name**. The
session id embeds it (`format!("ghost-{}-{}", alert_name, uuid.simple())`,
`src/daemon/ghost.rs:185`); do **not** parse the id — task 1 stores the name.

`GhostConfig` (`src/ipc.rs:81`) carries `agent: Option<String>`,
`parent_job_id: Option<String>` and `spawn_depth: u8`.

`ghost_bg_prefix` is assigned exactly one non-empty value outside test
fixtures — `src/daemon/ghost.rs:235` from the `bg_prefix` parameter, which
callers pass as `GS_BG_WINDOW_PREFIX` (`"de-gs-bg-"`) or `GS_SCHED_WINDOW_PREFIX`
(`"de-gs-sj-"`), both `src/daemon/mod.rs:61,65`. That is the signal for the
`trigger` string.

## Spec

Six tasks.

### Task 1 — `SessionEntry.ghost_runbook_name: Option<String>`

**The name matters.** `ghost_runbook` (no suffix) is already taken — it is the
`schedule_command` tool's parameter, live in `src/ai/tools/args.rs`,
`src/ai/types/pending.rs`, `src/ai/types/events.rs`,
`src/daemon/executor/mod.rs` and `src/daemon/executor/schedule.rs` (5 files,
verified at drafting). Using it would make every structural grep in this phase
match unrelated code. Use `ghost_runbook_name`, which appears **nowhere** in the
tree today.

Add the field beside `ghost_config`:

```rust
    /// Name of the runbook this ghost shell follows. `None` for chat
    /// sessions. Names the run in `RunKind::Ghost { runbook, .. }`.
    pub ghost_runbook_name: Option<String>,
```

Set it to `Some(runbook.name.clone())` at the one real construction site,
`src/daemon/ghost.rs:235` (the literal that assigns `ghost_bg_prefix:
bg_prefix`). **`None` at every other construction site.**

There are **15** `ghost_task_message:` lines in the tree — one per `SessionEntry`
literal — across seven files:

```
src/daemon/session.rs            7
src/daemon/context/estimate.rs   2
src/daemon/executor/mod.rs       2
src/daemon/context/background.rs 1
src/daemon/executor/bg_shell.rs  1
src/daemon/ghost.rs              1
src/daemon/server/ask.rs         1
```

Add `ghost_runbook_name:` beside each. This is mechanical: **the compiler names every
site you miss**, so `cargo build` is the checklist.

### Task 2 — the guard's missed-exit goal names the run kind

`RunLogGuard` is handed the `RunKind` at `start` but does not keep it. Store the
kind's `label()` (`src/daemon/run/mod.rs:81`, returns `"chat"` / `"ghost"` /
`"scheduled"` / `"delegated"`) and use it for the goal:

- goal becomes `format!("one {label} run")` — so a ghost's missed exit reads
  `"one ghost run"`, a chat turn's `"one chat run"`.
- `UNRECORDED_BLOCKER` and everything else is **unchanged**.

Do not store the whole `RunKind` (it is moved into the `RunStarted` record);
take `label()` before the move — it returns `&'static str`.

### Task 3 — `ghost_run_kind`: a pure function

New in `src/daemon/ghost.rs`:

```rust
/// The `RunKind` a ghost session runs as. A ghost spawned by another ghost is
/// `Delegated` and names its parent; everything else is a `Ghost` naming its
/// runbook and what set it off.
pub fn ghost_run_kind(
    runbook: Option<&str>,
    agent: Option<&str>,
    parent_job_id: Option<&str>,
    bg_prefix: &str,
) -> RunKind
```

Rules, in order:

1. `parent_job_id` is `Some(p)` **and** `agent` is `Some(a)` →
   `RunKind::Delegated { agent: a, parent: <p as a RunId> }`.
   `parent_job_id` is a ghost **session id** (`ghost-<name>-<uuid>`), not a
   `RunId` — so `RunId::parse` will reject it. Use `RunId::parse(p)` and fall
   through to rule 3 when it fails; a delegated ghost whose parent id is not a
   run id is still a ghost, not a crash. **Pin this as a named test case.**
2. Otherwise → `RunKind::Ghost { runbook, trigger }` where:
   - `runbook` is the passed name, or `"unknown"` when `None`;
   - `trigger` is `"scheduled"` when `bg_prefix` is `GS_SCHED_WINDOW_PREFIX`,
     else `"webhook"`.
3. (the fall-through from rule 1 lands here too)

Keep it free of `SessionStore` — it takes plain borrowed strings so the tests
are pure. The caller does the lookup.

### Task 4 — `trigger_ghost_turn` holds the guard

Rewrite the wrapper quoted in § Current state:

- Read `ghost_runbook_name`, `ghost_config.agent`, `ghost_config.parent_job_id` and
  `ghost_bg_prefix` from the session in **one** `with_sessions` closure (the
  store must not be locked across an `.await` — the existing code follows this
  rule everywhere; keep it).
- `let mut run_log = RunLogGuard::start(RunId::new(), ghost_run_kind(…));`
- `let result = do_ghost_turn(…).await;` — unchanged.
- Map the outcome onto the guard **before** `write_mailbox_on_exit`:
  - `Ok(())` → `RunState::Complete`
  - `Err(e)` → `RunState::HardFail(Briefing::new(...))` with goal
    `format!("ghost shell {session_id}")`, blocker `Some(e.to_string())`, and
    empty criteria / working files / attempts / budget.
- Everything else in the wrapper (mailbox, container teardown, the returned
  `Result`) is **unchanged**. The guard drops at the end of the function and
  writes the `RunFinished`.

**`BudgetExceeded` is task 5's**, because only the loop knows the turn budget
was what ended it.

### Task 5 — the loop writes `TurnStarted`, and the budget exit is a budget state

Two edits inside `do_ghost_turn`, both in the turn loop at
`src/daemon/ghost.rs:444-500`. `do_ghost_turn` needs the run id and a way to
report the budget exit; give it **two new parameters** — `run_id: &RunId` and
`budget_exhausted: &mut bool` — rather than returning a richer type, so the four
`bail!` sites and the `Result<()>` contract stay exactly as they are.

1. Immediately after `turn += 1;`, append a `TurnStarted { turn: turn as u32 }`
   record for `run_id`. Use the same `TurnLogRecord` shape as
   `RunLogGuard::start` (`src/daemon/run/guard.rs:26-30`): an RFC-3339
   `chrono::Utc::now().to_rfc3339()` timestamp and `append_turn_event`.
   `append_turn_event` never fails the caller — a write problem is logged at
   WARN inside it.
2. In the `if turn >= max_ghost_turns` block that sets `wrap_up_turn = true`,
   also set `*budget_exhausted = true`.

Then in `trigger_ghost_turn`: when `result` is `Ok(())` **and**
`budget_exhausted` is true, record `RunState::BudgetExceeded(Briefing::new(...))`
with goal `format!("ghost shell {session_id}")`, blocker
`Some("ghost turn budget exhausted".into())` and
`budget_left: Some(BudgetDimension::Turns)` — instead of `Complete`.

**Order matters:** a run that both exhausted its budget and errored is a
`HardFail` — the error is the more specific outcome. Check `Err` first.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-07a.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] **The session carries its runbook name:**
      `grep -c 'pub ghost_runbook_name: Option<String>' src/daemon/session.rs` → **1**
      (now `0`), and `grep -rc 'ghost_runbook_name:' src/ --include='*.rs' | grep -v ':0' | wc -l`
      → **7 or more** files (now `0`).
- [ ] **Exactly one site sets it to a real name:**
      `grep -rc 'ghost_runbook_name: Some' src/ --include='*.rs' | grep -v ':0' | wc -l`
      → **1** (now `0`).
- [ ] **The guard's missed-exit goal is no longer chat-specific:**
      `grep -c '"one chat turn"' src/daemon/run/guard.rs` → **0** (now `1`), and
      `grep -c 'label()' src/daemon/run/guard.rs` → **1 or more** (now `0`).
- [ ] **The kind function exists and is pure:**
      `grep -c 'pub fn ghost_run_kind' src/daemon/ghost.rs` → **1** (now `0`);
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/ghost.rs | grep -c 'fn ghost_run_kind(\s*$\|fn ghost_run_kind('`
      → **1**; and the signature takes no `SessionStore`:
      `sed -n '/pub fn ghost_run_kind/,/^) -> RunKind/p' src/daemon/ghost.rs | grep -c 'SessionStore'`
      → **0**.
- [ ] **The ghost run has one guard and one id:**
      `grep -c 'RunLogGuard::start' src/daemon/ghost.rs` → **1** (now `0`);
      `grep -c 'RunId::new()' src/daemon/ghost.rs` → **1** (now `0`).
- [ ] **All three terminal states are recorded:** in `src/daemon/ghost.rs`,
      `grep -c 'RunState::Complete'` → **1**,
      `grep -c 'RunState::HardFail'` → **1**,
      `grep -c 'RunState::BudgetExceeded'` → **1** (each **0** now).
- [ ] **`TurnStarted` finally has a production emitter:**
      `grep -rl 'TurnStarted' src/ --include='*.rs' | grep -v 'run/turn_log.rs\|run/log_read.rs' | wc -l`
      → **1 or more** (now **0**), and that file is `src/daemon/ghost.rs`:
      `grep -c 'TurnStarted' src/daemon/ghost.rs` → **1 or more** (now `0`).
- [ ] **The 760-line body was not restructured:** `do_ghost_turn` still returns
      `Result<()>` and still has its four bails —
      `awk '/^async fn do_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'anyhow::bail!'`
      → **4**, unchanged (now `4`).
- [ ] **No *new* `unsafe`/`unwrap`/`expect`/`panic!` in production code:**
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/ghost.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **1**, unchanged (now **1**). *The one hit is the pre-existing
      `.expect("CostRecord serialization is infallible")` at
      `src/daemon/ghost.rs:863`, carrying an INVARIANT comment. Leave it; the
      criterion is that the count does not grow.*
- [ ] `cargo test --lib daemon::ghost` → **8 or more** passing, `0 failed` (now
      **2**). *Arithmetic: 2 existing + 6 in the Test plan.*
- [ ] `cargo test --lib daemon::run::guard` → **8 or more** passing, `0 failed`
      (now **7**). *7 existing + 1 new.*
- [ ] **Preservation, each must not move:** `daemon::run::turn_log` **7**,
      `daemon::session` **21**, `agents::mailbox` **4**, `daemon::stream` **8**,
      `daemon::run::active` **6**, `daemon::run::cancel` **13**.
- [ ] **The mailbox is untouched this phase:**
      `git diff --stat -- src/agents/mailbox.rs | wc -l` → **0**, and
      `awk '/^async fn write_mailbox_on_exit/,/^}/' src/daemon/ghost.rs | grep -c 'RunResult'`
      → **0**. *That is 07b's work.*
- [ ] **Nothing leaks into the developer's real HOME:**
      `ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l` → **0**, unchanged by
      a full suite run.
- [ ] Full lib suite **1752 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1745**). *Arithmetic: 1745 + 6 + 1 = 1752.*
- [ ] All four gates pass.

## Test plan

**`src/daemon/ghost.rs` (6 new).** Five are pure `ghost_run_kind` cases needing
no HOME; the sixth writes a turn log, so it takes `crate::test_home_guard()` +
a tempdir — reuse `run::cancel`'s `test_support::HomeGuard`
(`src/daemon/run/cancel.rs`, made `pub(crate)` in phase-06b) and its
`fresh_run_id()`, exactly as `src/daemon/stream.rs`'s
`cancel_stage_distinguishes_the_two_doors` does. **The turn log is keyed by run
id and libtest runs in parallel — the sixth test must use its own id.**

- `ghost_run_kind_names_the_runbook_and_a_webhook_trigger`
- `ghost_run_kind_names_a_scheduled_trigger_from_the_sched_prefix` — pass
  `GS_SCHED_WINDOW_PREFIX`; assert `trigger == "scheduled"`.
- `ghost_run_kind_is_delegated_when_a_parent_run_id_and_agent_are_present` —
  pass a **valid 16-hex** parent id; assert `Delegated { agent, parent }`.
- `ghost_run_kind_falls_back_to_ghost_when_the_parent_is_not_a_run_id` —
  **negative case**: pass a real ghost session id
  (`"ghost-disk-full-0123456789abcdef0123456789abcdef"`) as `parent_job_id`
  with an agent set; assert the result is `Ghost { .. }`, **not** `Delegated`,
  and that nothing panics.
- `ghost_run_kind_without_a_runbook_says_unknown` — **negative case**:
  `runbook: None` → `Ghost { runbook: "unknown", .. }`.
- `ghost_turn_log_records_a_turn_started_for_each_turn` — append two
  `TurnStarted` records for one fresh run id through the same code path task 5
  uses, then `read_turn_log` and assert both are present with turns 1 and 2, in
  order. *(If task 5's emitter is inlined rather than factored into a helper,
  extract the two-line emitter into a small private fn so this test can call
  it — do not make the test re-implement the record.)*

**`src/daemon/run/guard.rs` (1 new).**

- `guard_missed_exit_goal_names_the_run_kind` — start a guard with a
  `RunKind::Ghost { .. }`, drop without `finish`, read the log and assert the
  `HardFail` briefing's **goal** is `"one ghost run"` while its blocker is still
  `"the run loop exited without recording an outcome"`. Pins both halves: the
  goal changed, the blocker did not.

## Mutation checks

Three. Use the `patch` tool to break and revert — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.**
Record the red-then-green transcript for each in the Update Log, naming **the
assertion message in the failure output**, not just the test.

1. **A non-run-id parent must not become a `Delegated` run.** In
   `ghost_run_kind`, replace the `RunId::parse` fall-through with an
   unconditional `Delegated` (parsing the parent id with a lossy default) →
   `ghost_run_kind_falls_back_to_ghost_when_the_parent_is_not_a_run_id` must go
   **red** on its "a parent that is not a run id must stay a Ghost" assertion.
2. **The budget exit must not be reported as `Complete`.** In
   `trigger_ghost_turn`, drop the `budget_exhausted` check so `Ok(())` always
   records `Complete` → the ghost test that pins `BudgetExceeded` must go
   **red**. *(If no unit test reaches that mapping, say so in the Update Log and
   record the § E section-D grep count moving from 1 to 0 instead — do not
   invent a test that mocks the whole 760-line loop.)*
3. **The guard's goal must follow the kind.** In `RunLogGuard::drop`, hardcode
   the goal back to `"one chat turn"` →
   `guard_missed_exit_goal_names_the_run_kind` must go **red** showing
   `"one chat turn"` where `"one ghost run"` was expected.

## End-to-end verification

The real artifact is `var/log/runs/<id>.jsonl` — a ghost run's turn log on
disk, with a `RunStarted` naming the right kind, `TurnStarted` records, and one
`RunFinished`. Section H writes one through the **public reader**
(`read_turn_log`) in a throwaway HOME, so the evidence is the file, not a mock.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-07a.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the ghost module =="
cargo test --lib daemon::ghost -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::ghost|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. the guard =="
cargo test --lib daemon::run::guard 2>&1 | grep -E '^test result'

echo "== D. structural greps (each must print the stated number) =="
printf 'ghost_runbook_name field        (1): %s\n' "$(grep -c 'pub ghost_runbook_name: Option<String>' src/daemon/session.rs)"
printf 'ghost_runbook_name files      (>=7): %s\n' "$(grep -rc 'ghost_runbook_name:' src/ --include='*.rs' | grep -v ':0' | wc -l)"
printf 'ghost_runbook_name: Some files  (1): %s\n' "$(grep -rc 'ghost_runbook_name: Some' src/ --include='*.rs' | grep -v ':0' | wc -l)"
printf 'chat-specific goal gone    (0): %s\n' "$(grep -c '"one chat turn"' src/daemon/run/guard.rs)"
printf 'guard uses label()       (>=1): %s\n' "$(grep -c 'label()' src/daemon/run/guard.rs)"
printf 'ghost_run_kind defined     (1): %s\n' "$(grep -c 'pub fn ghost_run_kind' src/daemon/ghost.rs)"
printf 'ghost_run_kind is pure     (0): %s\n' "$(sed -n '/pub fn ghost_run_kind/,/^) -> RunKind/p' src/daemon/ghost.rs | grep -c 'SessionStore')"
printf 'one RunLogGuard::start     (1): %s\n' "$(grep -c 'RunLogGuard::start' src/daemon/ghost.rs)"
printf 'one RunId::new()           (1): %s\n' "$(grep -c 'RunId::new()' src/daemon/ghost.rs)"
printf 'RunState::Complete         (1): %s\n' "$(grep -c 'RunState::Complete' src/daemon/ghost.rs)"
printf 'RunState::HardFail         (1): %s\n' "$(grep -c 'RunState::HardFail' src/daemon/ghost.rs)"
printf 'RunState::BudgetExceeded   (1): %s\n' "$(grep -c 'RunState::BudgetExceeded' src/daemon/ghost.rs)"
printf 'TurnStarted in ghost.rs  (>=1): %s\n' "$(grep -c 'TurnStarted' src/daemon/ghost.rs)"
printf 'TurnStarted prod files   (>=1): %s\n' "$(grep -rl 'TurnStarted' src/ --include='*.rs' | grep -v 'run/turn_log.rs\|run/log_read.rs' | wc -l)"
printf 'do_ghost_turn bails        (4): %s\n' "$(awk '/^async fn do_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'anyhow::bail!')"
printf 'unwrap/panic pre-test      (1): %s\n' "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/ghost.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"
printf 'mailbox.rs untouched       (0): %s\n' "$(git diff --stat -- src/agents/mailbox.rs | wc -l)"
printf 'no RunResult in mailbox fn (0): %s\n' "$(awk '/^async fn write_mailbox_on_exit/,/^}/' src/daemon/ghost.rs | grep -c 'RunResult')"

echo "== E. preservation: turn_log 7, session 21, mailbox 4, stream 8, active 6, cancel 13 =="
for m in daemon::run::turn_log daemon::session agents::mailbox daemon::stream daemon::run::active daemon::run::cancel; do
  printf '%-26s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== F. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== G. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== H. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then, as the last task, the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-07a-ghost-run-log.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-07a.txt
diff /tmp/pasted-m24-07a.txt /tmp/e2e-m24-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/ghost.rs` — tasks 3, 4, 5 and the Test plan. **Do not restructure
  `do_ghost_turn`**: its two new parameters, the `TurnStarted` emitter after
  `turn += 1`, and `*budget_exhausted = true` in the existing
  `if turn >= max_ghost_turns` block are the only edits inside it.
- `src/daemon/run/guard.rs` — task 2 **only** (the stored label and the goal
  string) plus the one new test. Do not change `UNRECORDED_BLOCKER`, `finish`,
  `run_id` or what `start` writes.
- `src/daemon/session.rs` — task 1 **only**: the new field and its seven
  literals.
- `src/daemon/context/estimate.rs`, `src/daemon/context/background.rs`,
  `src/daemon/executor/mod.rs`, `src/daemon/executor/bg_shell.rs`,
  `src/daemon/server/ask.rs` — task 1 **only**: `ghost_runbook_name: None` in the
  `SessionEntry` literal. **Nothing else in these five files.**
- `docs/dev/milestones/M24-run-manager/phase-07a-ghost-run-log.md` — the
  Update Log.

No other file. In particular do not touch `src/agents/mailbox.rs`,
`src/daemon/run/turn_log.rs`, `src/daemon/run/result.rs`,
`src/daemon/run/cancel.rs`, `src/daemon/run/active.rs`, `src/daemon/stream.rs`,
`src/daemon/scheduled.rs` or `src/webhook/`.

## Out of scope

- **Cancelling a ghost run.** The ghost loop gains no cancel check and
  `daemoneye stop <run>` does not reach it — that is **phase-07b**, together
  with the `select!`/turn-top check the design calls for.
- **The mailbox carrying a `RunResult`.** Phase-07b. `write_mailbox_on_exit`
  keeps its current `MailboxStatus` shape here, and a § D criterion asserts it.
- **`RunKind::Scheduled`.** A scheduled job that starts a ghost is still a
  `Ghost` run with `trigger: "scheduled"`. Making the *job* its own run is
  phase-08.
- **`Budget` enforcement.** The ghost's `max_ghost_turns` stays exactly the
  ceiling it is today; this phase only *reports* that it was what ended the run.
  Real `Budget` accounting is M25.
- **Surfacing the ghost's run id** to the user (`daemoneye runs`, the
  `[Ghost Shell Started]` event text). Phase-10.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-11 14:29 (progress)

Started phase-07a. Pre-flight confirmed against the tree at `9e7aa150`: clean
worktree, lib suite **1745 passed / 0 failed**, `daemon::ghost` 2, `run::guard` 7,
`run::turn_log` 7, `session` 21, `mailbox` 4 — all matching the drafting
numbers except the commit hash. `RunLogGuard` appears **0** times in
`src/daemon/ghost.rs` and `ghost_runbook_name` **0** times in
`src/daemon/session.rs`, as expected. Wiring the runbook name, the guard, and
the `TurnStarted` emitter next.
### Update — 2026-09-11 14:46 (mutation checks)

All three mutations: break with the `patch` tool, observe the intended
assertion go red, revert, observe green.

## Mutation 1 — unconditional Delegated (lossy parent)
test daemon::ghost::tests::ghost_run_kind_falls_back_to_ghost_when_the_parent_is_not_a_run_id ... FAILED
expected Ghost for the non-run-id parent, got Delegated { agent: "analyst", parent: RunId("00000000de1e6a7e") }
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1755 filtered out; finished in 0.00s
## Mutation 1 reverted — green again
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1755 filtered out; finished in 0.00s


## Mutation 2 — budget exit dropped, Ok(()) always Complete
MUT2 APPLY: BudgetExceeded occurrences in trigger_ghost_turn before:
1
MUT2 BUILD:
warning: unused import: `BudgetDimension`
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
MUT2 GREP BudgetExceeded in ghost.rs (structure criterion moves 1 -> 0):
0
MUT2 TEST attempt:
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1748 filtered out; finished in 0.00s
MUT2 REVERTED - green:
1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.74s


## Mutation 3 — goal hardcoded back to 'one chat turn'
test daemon::run::guard::tests::guard_missed_exit_goal_names_the_run_kind ... FAILED
---- daemon::run::guard::tests::guard_missed_exit_goal_names_the_run_kind stdout ----
thread 'daemon::run::guard::tests::guard_missed_exit_goal_names_the_run_kind' (1265896) panicked at src/daemon/run/guard.rs:300:21:
assertion `left == right` failed: the missed-exit goal must name the run kind, not hardcode chat
    daemon::run::guard::tests::guard_missed_exit_goal_names_the_run_kind
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1755 filtered out; finished in 0.00s
assertion `left == right` failed: the missed-exit goal must name the run kind, not hardcode chat
  left: "one chat turn"
 right: "one ghost run"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

## Mutation 3 reverted — green again
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1755 filtered out; finished in 0.00s


**Mutation 2 note (per the Test plan's parenthetical):** no unit test reaches
the `budget_exhausted` mapping — reaching it means running the whole 760-line
`do_ghost_turn` against a live AI backend, which the phase doc forbids
mocking. As instructed, the mutation is instead evidenced by the § D structural
grep count for `RunState::BudgetExceeded` in `src/daemon/ghost.rs` moving
**1 → 0** (printed under `MUT2 GREP` above), with `cargo build` still
succeeding (only an `unused import` warning) and `daemon::ghost` tests staying
green — i.e. the mapping was genuinely deleted and the suite did not notice,
so the grep is the observable. The check was reverted and the count is back
to 1.

### Update — 2026-09-11 14:46 (end-to-end verification)

Ran the § End-to-end verification block verbatim from the repo root
(`bash /tmp/run-e2e-07a.sh`, which contains the block unchanged), redirecting
into `/tmp/e2e-m24-07a.txt`. Pasted in full below.

```sh
== A. build ==
real HOME run logs BEFORE: 0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
cargo_exit=0
== B. the ghost module ==
test daemon::ghost::tests::ghost_run_kind_names_a_scheduled_trigger_from_the_sched_prefix ... ok
test daemon::ghost::tests::ghost_run_kind_without_a_runbook_says_unknown ... ok
test daemon::ghost::tests::ghost_run_kind_is_delegated_when_a_parent_run_id_and_agent_are_present ... ok
test daemon::ghost::tests::ghost_run_kind_names_the_runbook_and_a_webhook_trigger ... ok
test daemon::ghost::tests::ghost_run_kind_falls_back_to_ghost_when_the_parent_is_not_a_run_id ... ok
test daemon::ghost::tests::capacity_allows_when_under_limit ... ok
test daemon::ghost::tests::capacity_zero_disables_cap ... ok
test daemon::ghost::tests::ghost_turn_log_records_a_turn_started_for_each_turn ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1748 filtered out; finished in 0.00s
cargo_exit=0
== C. the guard ==
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1748 filtered out; finished in 0.00s
== D. structural greps (each must print the stated number) ==
ghost_runbook_name field        (1): 1
ghost_runbook_name files      (>=7): 7
ghost_runbook_name: Some files  (1): 1
chat-specific goal gone    (0): 0
guard uses label()       (>=1): 1
ghost_run_kind defined     (1): 1
ghost_run_kind is pure     (0): 0
one RunLogGuard::start     (1): 1
one RunId::new()           (1): 1
RunState::Complete         (1): 1
RunState::HardFail         (1): 1
RunState::BudgetExceeded   (1): 1
TurnStarted in ghost.rs  (>=1): 4
TurnStarted prod files   (>=1): 1
do_ghost_turn bails        (4): 4
unwrap/panic pre-test      (1): 1
mailbox.rs untouched       (0): 0
no RunResult in mailbox fn (0): 0
== E. preservation: turn_log 7, session 21, mailbox 4, stream 8, active 6, cancel 13 ==
daemon::run::turn_log      test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1749 filtered out; finished in 0.00s
daemon::session            test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 1735 filtered out; finished in 0.00s
agents::mailbox            test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1752 filtered out; finished in 0.01s
daemon::stream             test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1748 filtered out; finished in 0.00s
daemon::run::active        test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1750 filtered out; finished in 0.00s
daemon::run::cancel        test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1743 filtered out; finished in 0.00s
== F. lib suite, three consecutive runs ==
test result: ok. 1752 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.16s
test result: ok. 1752 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.18s
test result: ok. 1752 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.14s
== G. gates ==
fmt_exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
clippy_exit=0
== H. no HOME litter from the suite ==
real HOME run logs AFTER (==BEFORE): 0
```

