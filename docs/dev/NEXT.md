# NEXT

The one live pointer and nothing else. The append-only record of every earlier
milestone and phase — and every `**Superseded — …**` pointer that once stood
here — lives in [NEXT-HISTORY.md](NEXT-HISTORY.md).

## M22 — Tool surface & context (active, scoped 2026-09-12)

**Active phase: M22 phase-05 — send-input**
(`docs/dev/milestones/M22-tool-surface/phase-05-send-input.md`, status: todo,
drafted 2026-09-14). Dispatch with `/rexymcp:dispatch phase-05`.

Phase-04b — watch-shell closed **approved_after_2 2026-09-14**
(`docs/dev/milestones/M22-tool-surface/phase-04b-watch-shell.md`, status:
done; `bugs/bug-04b-1.md` and `bugs/bug-04b-2.md` both `closed`).

**Scope for 05:** `send_input { shell, text, enter? }` — core,
`Category::Run`, **approval-gated**, **46 tools: 34 core + 12 deferred**. The
milestone's one genuinely dangerous addition, and the first tool in the shell
row that types rather than observes.

**The gated-list entry and the real prompt are one decision, not two.**
`APPROVAL_GATED_TOOLS` is also what exempts a tool from every budget cap in
`daemon::stream`, so a list entry without a working
`prompt_and_await_approval` hands a keystroke tool an uncapped call budget.
A criterion pins both halves — the list membership *and* the file's approval
call sites going 3 → 4.

**Two design claims measured before drafting, and this time both hold.** The
`"i"` log is real — `PtyBackend::input` writes the bytes to the PTY *and*
records them via `CastWriter::write_input` on every frame. And **no read tool
can surface them**: `read_command_output` and 04b's `read_output_since` both
filter to `"o"` events, so text typed with this tool is auditable but
unreachable from `read_shell` / `find_in_shells` / `watch_shell`. That is a
safety property, and a named test pins it rather than leaving it incidental.
This is the first M22 phase where the design's own description of a shell tool
survived contact with the tree unchanged — after 03a's collapsed `scope`, 04a's
two non-existent `open_shell` parameters, and 04b's dead `.meta.json` index.

**Default-deny for ghosts.** The central `ToolPolicy::permits` check is not
enough: a deny-list that merely omits `send_input` would permit it. The phase
adds `ghost_may_send_input` mirroring `ghost_may_use_tmux_control`'s
`explicitly_allows` shape — a deliberate second copy, with the third being the
point at which they collapse into one parameterised helper.

**One inversion to watch:** `send_input` must **not** join the
`should_emit_tool_feedback` chain that 03a–04b all joined. It has an approval
panel; a `ToolStarted` line as well is duplicate UI. It does get a `summary`
arm. Both are compiler-invisible and point opposite ways.

**Four mis-specified criteria caught by running them before dispatch** — and
three of them were the *proxy* shape the 04b review just folded, not broken
`sed` ranges: a `grep -A30` window too small to reach the approval call (the
same window misses on `tmux_control` even at `-A60`), a "passes no
`ghost_policy`" grep that was measuring a *comment*, and a `\r` escape that
matched nothing anywhere. All three replaced with properties: an approval
call-site count, a named test plus a mutation for default-deny, and an
**injected sender seam** so the newline is asserted on the exact bytes rather
than inferred from a length — a byte count cannot tell `\n` from `\r`, which
is precisely why the seam earns its place.

`watch_shell { shell, pattern?, timeout_secs? }` ships core + `Category::Read`,
not approval-gated — **45 tools: 33 core + 12 deferred**. It is the second
tool that genuinely blocks the turn, and its executor arm wraps in
`with_keepalive`; without that a 300 s block trips the CLI's dead-daemon
deadline and nothing but a criterion catches it.

**Both bounces were minor and two of the three defects were architect-side.**
bug-04b-1: a `#[allow(dead_code)]` on a constant with no reader, plus two new
production `expect()`s. bug-04b-2: the pattern test asserted
`out.contains("READY")` while the outcome sentence echoes the pattern itself,
so it passed even with the rendered match replaced by an empty string — found
by mutating at review, not by reading.

**Three architect-side criterion defects on one phase, all the same shape:**
pinning a *number* the intended property does not imply — a call's placement
instead of its behaviour, a count that contradicted the box authorising the
thing it counted, and a test's *name* where its assertion's discriminating
power was what mattered. `WORKFLOW.md` § "Specs pin behavior, not rendering"
covers this and was not followed. **Fold candidate at milestone close**; M22's
running total is twelve mis-specified instruments, and these three are a
distinct sub-species from the earlier broken `sed`/`grep` ranges.

**A pre-existing flake is now measured and recorded** in the milestone README:
`pty_stop_with_no_foreground_job_remembers_nothing` fails about 1 full-suite
run in 4, reproduces at the pre-phase baseline `fa10352a` in a throwaway
worktree, and `src/shell/pty.rs` is untouched by the whole phase. Re-measure
at the baseline before attributing it to a phase.

**Scope for 04b:** `watch_shell { shell, pattern?, timeout_secs? }` — core,
`Category::Read`, not approval-gated, **45 tools: 33 core + 12 deferred**. It
is the second tool that genuinely blocks the turn, so the
executor arm must wrap in `with_keepalive` (the CLI's dead-daemon deadline is
derived from `KEEPALIVE_PERIOD_SECS`, and nothing but a criterion catches a
missing wrap). `await_agent_result` is the one existing blocking tool and is
quoted verbatim as the pattern.

**Drafting measured two defects that reshaped the phase, both recorded in the
milestone README, neither fixed here:**

1. **Nothing in production writes a `.meta.json` command index.**
   `MetaIndex::save` has no production caller — the host writes the `.cast`
   and never the sidecar, and the real home shows a live shell's cast with no
   sidecar beside it. So `read_shell` and `find_in_shells` land on their
   "command index is missing" branch against any real shell, and a
   `watch_shell` built on 03b's reader — which is what the charter row
   proposed — **could never match in production**. 04b reads the live cast
   file directly instead; `CastWriter` flushes per record, so a tail sees
   motion immediately.
2. **The registry record's `started_unix` and the cast filename's are stamped
   in different processes** (`spawn_for` in the daemon, `run_host`'s own `now`
   in the child), and nothing reconciles them for a live shell. A spawn that
   crosses a second boundary leaves `cast_path(id, record.started_unix)`
   naming no file. 04b resolves the cast with the already-public
   `parse_cast_name` over `shell_logs_dir()` and pins it with a named test.

That makes **three** consecutive M22 phases where the design's own description
of a shell tool did not survive contact with the tree — after 03a's `scope`
collapsing three ways to two and 04a's two non-existent `open_shell`
parameters. That is now the milestone's most-earned
rule — write specs against the tree as it is, not as the design describes it.

**`watch_pane` is the analogue and deliberately not the model.** Its `ToolDef`
says *"Blocks until the pane's command completes"* and it does not — it spawns
a watcher and injects a `[Watch Pane Match]` user message later. `watch_shell`
returns its own answer as the tool result, so none of that machinery ports.
Only two numbers do: the 500 ms poll and the 5 s start grace.

**Deliberate limits, each with its reason in the doc:** no ownership check
(observers read any shell; only `close_shell` checks), the watch starts at the
cast's **current length** so a pattern already in the log must time out (the
load-bearing negative test), and the `Running → Idle` arm on a real command is
verified by hand in § End-to-end verification rather than by a flaky timing
test. The phase also folds the third copy of the live-shell-ids block into
`live_shell_ids`, with no message text changed.

**Four mis-specified criteria were caught by running them before dispatch** —
two `sed` ranges that restarted on same-named test functions (fixed with
`pub fn` anchors), one arm range that closed at the destructuring brace before
reaching `with_keepalive`, and one `fn` grep that missed every
`#[tokio::test] async fn`. **That is the eighth through eleventh
mis-specified grep in M22**, still with no code defect behind any of them, and
still only ever caught by running the instrument rather than reasoning about
it.

`open_shell {}` + `close_shell { shell }` ship core + `Category::Run` — M22's
first non-read-only tools. **44 tools: 32 core + 12 deferred.** `src/shell/` is
untouched: no `ShellSignal` variant, no `ShellRecord` field.

**The honesty requirement is now enforced by a test.**
`open_shell_returns_a_new_id` asserts `dedicated == false` — a dedicated shell
can never be selected by `shell_for_owner`'s reuse rule, so the tool would
return an id nothing could reach — **and** asserts the returned sentence
contains `"lowest-numbered"`, the reuse caveat bug-03a-1 established. A
description that overclaims now fails a test rather than waiting for review.

**The bug-03c-1 capture fix worked on its first outing:** section E of the
filed transcript is three clean runs with failure names preserved.

**Two of the phase's criteria were mis-specified instruments — both mine,
neither a defect.** One greped `close_shell_in` where the codebase (following
`open_shell_with` / `run_in_shell_with`) named it `close_shell_with`; the other
used a `sed` range whose `^    }` terminator does not close the function where
assumed, so it counted 4 where 2 was pinned. Both properties verified correct
with corrected greps. **That is the sixth and seventh mis-specified grep in
M22** — every one caught by running the criteria rather than reasoning about
them, and none ever a code defect. Worth stating plainly at milestone close as
the milestone's dominant architect-side failure mode.

**PE signed off on the ten-phase breakdown 2026-09-12.** Phase-01 — pty-default
also closed **approved_after_1 2026-09-12**; both of M22's closed phases bounced
once, and both bounces were evidence defects on correct code with an
architect-side root cause.

**What 02 shipped:** `Category` (`Read | Write | Search | Run | Meta`) is now a
required field on all 39 `ToolDef` literals, with `Category::mutates_state()`
derived from it and `category_of(name)` resolving through the table. Read 17 /
Write 13 / Search 4 / Run 4 / Meta 1, verified tool-by-tool at review. **M25's
governor dependency is satisfied** — `docs/design/daemoneye-2.0.md` § 2.3 named
it outright.

**Two deliberate divergences from the rexyMCP original**, worth remembering
because copying that code verbatim gets both wrong: `mutates_state()` counts
`Run` as mutating (rexyMCP's narrower `mutates_files` excludes `bash`; a
DaemonEye `Run` tool restarts services on a live host), and `category` is a
**struct field** rather than a name-keyed `match`, so a tool added without one
is a compile error rather than a silent `None`.

The live invariant that keeps it from being speculative until M25 arrives:
**every tool in `APPROVAL_GATED_TOOLS` mutates state**, pinned by
`approval_gated_tools_all_mutate_state`. That list is not just documentation —
it exempts a tool from every budget cap in `daemon::stream`, so a read-only
entry would hand it an uncapped call budget.

Tool count unchanged at **39 (27 core + 12 deferred)**; `doc_truth` stays at 10.

Phase-01's one bounce (bug-01-1, `closed`) was an evidence defect on correct
code — the mutation-2 entry applied the inverse mutant and reported a red that
mutant cannot produce. The re-dispatch changed **no `src/` line**. The review
re-ran the specified mutant by hand rather than reading the transcript, and it
reproduces. One calibration recorded there, first occurrence: **a criterion that
pins a test count must pin the exact invocation that yields it** — bug-01-1
demanded the literal string `4 passed; 1 failed` without naming the filter that
produces it, and that alone caused a second, milder mismatch in the corrected
entry.

**Still owed from phase-01:** the four-run-kind live check on the PTY
substrate — chat, ghost, scheduled, background — is **architect-run** per the
M14 convention and was not part of 01's executor work. It is an M22 exit
criterion, not a phase-01 one; run it before the milestone closes.

PE chose M22 next at the M24 close, ahead of M21 and M23: `Category` on `TOOLS`
is M25's stated dependency, and M21's `host?` params and M23's
`[HOSTS]/[SHELLS]` blocks both land on a cleaner surface once the pane tools are
gone. Scope, exit criteria and a proposed 10-phase breakdown are in
`docs/dev/milestones/M22-tool-surface/README.md`.

Headline exit criterion: **zero `%`-pane identifiers in any prompt asset or tool
description**. Measured at scoping — `sre.toml` 47 pane/tmux refs, tool defs 78,
5 seeded memories.

**Two facts found while scoping, both recorded in the README:**

- **The design's "36 → 42" arithmetic is stale.** The tree has **39** tools, and
  three of § 2.5's twelve "additions" (`run_log_tail`, `run_log_search`,
  `get_turn`) already shipped in M24 phase-02b. Real delta: **39 − 6 + 9 = 42**.
  Phase docs cite the README, not § 2.5's numbers.
- **`[execution] backend` defaulted to `"tmux"`**, so everything M20 and M24
  built was opt-in. **Phase 01 flipped it** (PE decision at the M24 close) so
  later milestones' live checks exercise the real path. Done 2026-09-12; the
  stale config doc comment went with it.

**Next step:** `/rexymcp:dispatch phase-05`. Phases 06–10 are not drafted;
each is drafted after its predecessor is approved.

**Carried out of M24, still open:**

- One calibration fold **held at two occurrences**: *state the property to
  break, not the line to edit* (M24 09c, 10). M22 has nine near-identical tool
  phases; a third instance here should be folded.
- **A phase doc is append-only and nothing enforces it** — M24 phase-10's run
  overwrote its own spec, and no gate caught it. Wants a guard test or a
  template line; until then the reviewer checks the headings survived.
- One local-only `WORKFLOW.md` edit on the upstream push backlog: the
  `NEXT-HISTORY.md` row in the directory layout.
- **Deferred to M25 with the governor:** cross-run telemetry (`RunRecord` →
  scorecard/profile).

## Historical

### M24 — Run Manager: **closed 2026-09-12, PE signed off**

**Active phase: none.**

All **16** M24 phases are `done` as of 2026-09-12 (10 charter rows; 05, 06, 07
and 09 split at drafting on measured seams). The milestone retrospective is in
`docs/dev/milestones/M24-run-manager/README.md` § Retrospective. **PE signed
off 2026-09-12.** Carried out of the close, still open:

1. **A phase doc is append-only, and nothing enforces it.** Phase-10's run
   overwrote its own spec (556 lines, including the `Status:` line), and no gate
   caught it — the suite was green with the spec gone and `bug_tracker` passed
   with no status line. Restored from git at review. Wants a guard test or a
   line in the phase template.
2. **Calibration folds — PE decided 2026-09-12.** Two of the three proposals
   turned out to already exist in `WORKFLOW.md` and had not been followed (the
   `## Spec` mutation-task rule, 2026-08-08; corpus scoping, 2026-08-29).
   - **Landed:** the after-the-phase corpus rule, as a fourth row on the
     existing scoping table plus one paragraph (14 instances, all
     architect-side).
   - **Landed:** *pin evidence where it lives*, as a clause on bounce step 3
     (07b ×2).
   - **Held at 2:** *state the property to break, not the line* (09c, 10). It
     tenses against the existing "exact `old_str`" prescription; resolve on a
     third occurrence.
3. **One local-only `WORKFLOW.md` edit is on the upstream push backlog**: the
   `NEXT-HISTORY.md` row in the directory layout.
4. **Deferred to M25 with the governor:** cross-run telemetry (`RunRecord` →
   scorecard/profile).

**Next step:** `/rexymcp:architect` scopes the next milestone. Nothing is dispatched until
then.

### M24 — Run Manager: the phase pointers

**Superseded — M24 phase-09c — seeded-tasks**
(`docs/dev/milestones/M24-run-manager/phase-09c-seeded-tasks.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-09c`.

Phase-09b closed **escalated 2026-09-12** (`67028e9`) after a `hard_fail` on one
invented API and a resume. Its `## Mutation checks` went unrun — the instructions
sat outside `## Spec` and were never seeded as tasks — so the architect ran both
at review; both go red. **09c makes the mutation checks Spec task 6**, which is
the fix for that class.

Scope for 09c: M24's last behavioural exit criterion. `parse_steps` reads a
runbook's `## Steps` section; the ghost's system prompt gains a marker contract;
and the completion break at `src/daemon/ghost.rs:1203` is gated on
`uncovered_steps` — the ghost may not finish until every seeded step has a
literal `STEP <n>: DONE` or `STEP <n>: SKIPPED <reason>` line.

The emphasised half of the criterion — *a condition it can check, not an
instruction it can agree with* — is why coverage is a parsed marker rather than
a keyword scan. The codebase already learned this: `parse_ghost_trigger`'s doc
comment records that the loose `ALERT` heuristic was removed "because a model
remarking 'no ALERT condition' could spuriously launch a ghost". A transcript
saying "I could not complete step 2" must leave step 2 **uncovered**, and that
is the load-bearing negative test.

Deliberate limits: only the one completion break is gated (cancel, wrap-up and
session-gone stay ungated, pinned by a criterion); the existing turn budget is
what stops a ghost that refuses to mark its steps, so no new terminal state and
no second counter; and no `complete_step` AI tool — a parsed line is already a
checkable condition, and a tool would drag in `TOOLS`, three provider renderings
and the doc_truth tables.

After 09c only **10** (cli listing) remains, and it is not drafted.

**Superseded — M24 phase-09b — resume-wiring**
(`docs/dev/milestones/M24-run-manager/phase-09b-resume-wiring.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-09b`.

Phase-09a closed **approved_first_try 2026-09-12** (`fe3f6f3`). Its executor
recorded a vacuously-green mutation placement before finding the one that
discriminates, and disclosed an intermediate `PASTE MISMATCH` before correcting
it — both reproduced at review. That is the standard the milestone's two earlier
bounces were asking for.

Scope for 09b: the four surfaces that make `daemoneye run resume <id>
"<directive>"` real — a pure `resumable_runbook` rule, `Request::ResumeRun`,
a daemon handler that re-spawns the ghost seeded with 09a's text, and a `Run`
CLI subcommand group. **The seed is the alert message** passed to
`GhostManager::start_session_with_config`; that is the whole point.

09b also **retires `resume_seed`** in favour of `resume_plan`. 09a shipped it
with zero production callers, and `resume_plan` supersedes it by resolving the
runbook too — STANDARDS § 2.2 says delete an unused symbol rather than keep a
second entry point speculatively.

Only a **ghost** run is resumable: it is the only kind carrying a runbook name.
Chat, scheduled and delegated are refused with a reason the user reads, each a
named negative test — `Delegated` being the subtle one, autonomous but naming an
agent rather than a runbook.

Pre-injected: `trigger_ghost_turn` must be `tokio::spawn`ed **after** the IPC
reply, never awaited inside `handle_client` — it runs a whole AI loop while the
client waits.

Remaining after 09b: **09c** (seeded tasks from a runbook `## Steps` section)
and **10** (cli listing). Neither drafted.

**Superseded — M24 phase-09a — resume-seed**
(`docs/dev/milestones/M24-run-manager/phase-09a-resume-seed.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-09a`.

Phase-08 closed **approved_first_try 2026-09-12** (`5138a33`). Three of its
acceptance criteria were wrong as drafted — all mine — and the executor caught
all three unprompted, explained each structurally and re-verified with correct
greps. The rule that generalises them: **validate a structural criterion against
the tree as it will look after the phase, not only before.**

Scope for 09a: the resume **seed**, pure. Given a finished run's id and a
directive, read its turn log, take the kind from the first `RunStarted` and the
briefing from the last `RunFinished`, and render the text a resumed run starts
from. `Complete` and `Cancelled` runs carry no briefing and are still resumable
on the directive alone; a run with no `RunFinished` is **not** resumable. 10
tests, nine of them pure.

The design's "not transcript rehydration" clause becomes structural:
`render_seed(kind, Option<&Briefing>, directive)` cannot see `TurnLogRecord`,
and a criterion pins that.

Pre-injected trap: **two unrelated types are called "briefing"** —
`daemon::briefing::read_briefing(agent)` (G4 prose) versus `run::Briefing`
(phase-01's bounded struct). 09a uses only the second, and a criterion asserts
the first is not referenced from `src/daemon/run/` at all.

Phase 09 split three ways: **09a** seed (this), **09b** wiring (CLI + IPC +
re-spawn), **09c** seeded tasks from a runbook `## Steps` section — none of
which exists in the tree today. Then 10 (cli). 09b and 09c are not drafted.

**Superseded — M24 phase-08 — scheduled-run**
(`docs/dev/milestones/M24-run-manager/phase-08-scheduled-run.md`, status: todo,
drafted 2026-09-11). Dispatch with `/rexymcp:dispatch phase-08`.

Phase-07b closed **approved_after_2 2026-09-11** (`99c6e29`). Both bounces were
evidence defects on correct code — the mutation-check-2 transcript, first in
prose (bug-07b-1) then surviving in the fenced block (bug-07b-2); the shipped
code never changed across either. Calibration recorded in the phase doc and the
milestone README: **pin evidence where it lives**, and scope doc-content
criteria to the section they police.

Scope for 08: `RunKind::Scheduled` gets its **first production constructor** —
it has existed since phase-01 and nothing has ever built one. `run_scheduled_job`
(435 lines, seven returns, eleven `mark_done` sites, returns `()`) is renamed to
`_inner` and wrapped; the wrapper holds a `RunLogGuard` and derives the terminal
`RunState` from the status the store holds after the job ran. Pure mapping
function, 7 tests — `daemon::scheduled` has **no test module at all** today.

Pre-injected gotcha: a successful **`Every`** job reads back as `Pending`, not
`Succeeded` (`mark_done` advances and reschedules it), so the `Pending` arm must
map to `Complete`. Pinned as a named test and as mutation check 1.

Out of scope with reasons stated: scheduled-job cancellation (the script branch
lives in a tmux window the substrate replaces; the ghost branch already cancels
via 07b), collapsing the two run logs a scheduled ghost job produces, and
rewriting the eleven `mark_done` sites.

Remaining after 08: 09 (resume), 10 (cli). Neither drafted.
