# NEXT

**Active phase: M5 phase-06a — tmux-off-runtime** (`todo`, drafted 2026-07-27).
Doc: `docs/dev/milestones/M5-ux-stability/phase-06a-tmux-off-runtime.md`.

Dispatch with `/rexymcp:dispatch phase-06a-tmux-off-runtime`.

## Mechanism B is bigger than the milestone table implied

The survey found **88 tmux subprocess calls inside `async fn`s across 16 files**,
not the single phase "06 — tmux-call-hardening" suggested. Every one blocks a
tokio worker until tmux answers.

| Population | Count |
|---|---|
| directly inside an `async fn` | **18** |
| inside `src/tmux/` sync helpers, called from async code | 46 helpers, 33+ call paths |
| sync-only callers (CLI) | ~18 — not a defect; blocking a CLI process is fine |

**Approach (PE-decided): B first, then A.**

- **B — get async callers off the runtime.** `spawn_blocking` + `tokio::time::timeout`
  at the async sites. This is what the exit criterion literally specifies, and it
  is what ticks it.
- **A — harden the sync helpers** with a timeout inside `src/tmux/`, so a wedged
  tmux cannot leak blocking-pool threads and future sync callers are bounded too.
  A **later** stage, after B.

**06 is now ~5 phases**: 06a (adapter + `background/run.rs`) → 06b `respawn.rs`
→ 06c `executor/` → 06d `daemon/` core → 06e `cli/`. Only 06a is drafted.

### Why this is not a textual substitution

`spawn_blocking` requires `F: 'static`, so **every borrowed argument at every call
site must become owned first**. That is real per-site work, which is why 88 sites
is five phases rather than one. 06a establishes the adapter and converts one file
as the worked example the rest copy.

**No new dependency needed** — `tokio`'s `rt-multi-thread` and `time` features are
already enabled, and `libc`/`nix` are present if stage A later needs them.

**`spawn_blocking` appears nowhere in this codebase today.** 06a introduces the
pattern, which makes it design-discovery rather than mechanical — hence the four
worked shapes in its Spec.

## Drafting caught three of my own criterion errors

All three were mine, and two were the *same* mistake the folds warn about:

1. **`grep -c "tmux::" run.rs` — I wrote 20, it is 14.** I had quoted numbers from
   a python census (which counts *calls*, including inline
   `Command::new("tmux")`) as though they were `grep -c` *line* counts. Two
   instruments, one number. The census was right; the criterion was wrong.
2. **`respawn.rs` — I wrote 12, it is 10 `tmux::` lines + 3 inline.** Same cause.
3. **`grep -c "off_runtime" src/tmux/mod.rs` — I wrote 2, it is 1.** I assumed the
   doc comment referenced the function by name; it references `TMUX_TIMEOUT`.

Also corrected a **false premise**: the spec said to skip `tmux::wait_for` calls in
`run.rs`. There are none — the two `wait_for` hits are
`wait_for_sudo_prompt_and_inject`, not a tmux call at all. A spec instruction to
avoid something that does not exist is the kind of trusted-but-wrong detail that
sends an executor looking.

These are the third, fourth and fifth counting errors of the milestone, and the
fold *"Run every count criterion; never derive it"* is exactly right — I ran the
census but then wrote the criteria from it instead of running the criteria.
**Refinement worth noting: running *a* command is not running *the* criterion.**

## Still open — four threads, all single occurrences

3. Fixture defaults neutering assertions (resolved by 05g). 5. Partially-transcribed
spec quotes. 8. Piping gates through `tail`. 9. A refinement built on a
harness-blocked command.

Plus two refinements to the folds themselves, from 05h's review: import liveness
must be checked **per import scope**, not per file; and when a phase needs a
multi-line census, **every** count criterion in it needs the same treatment.

## After 06a

06b–06e (the tmux tail) → **stage A** (harden the sync helpers) → **07**
(stall-instrumentation), plus the drafted **08–11** instance-hardening set. Four
exit criteria remain open; criterion 3 was ticked at 05f.

---

## Superseded: the renumber-and-split note

---

## The renumber and split (PE decision, 2026-07-26)

The numbering had `04k` (newtype) sorting before `05` (the restructures), which was
backwards — the newtype makes raw `.lock()` stop compiling, so it cannot land while
any raw acquisition remains. Renumbered; **nothing on disk was renamed**, since 04k
was never drafted:

| Was | Now | Scope | Sites |
|---|---|---|---|
| `05` (6 restructures) | **05a** | `background/helpers.rs::notify_session`, `background/gc.rs::gc_bg_windows`, `hook.rs:92` | 3 |
| — | **05b** | `webhook/process.rs` (2) + `stream.rs:722` | 3 |
| `04k` | **05c** | sessionstore-newtype + enforce — runs **last** | — |

06 (tmux-call-hardening), 07 (stall-instrumentation) and the drafted 08–11
instance-hardening set are untouched.

### The split is by file, not by fix shape — surveying changed my plan

I intended to split by shape (subprocess-under-lock vs file-write-under-lock).
Reading the six sites showed **`webhook/process.rs` holds one of each**:

- `notify_chat_panes` (`:162`) spawns a `tmux display-message` **per session**,
  inside `for entry in guard.values()`, under the guard.
- `inject_into_sessions` (`:149`) calls `append_session_message` **per session**
  under the guard — file writes only. It also carries a `let _ = entry;` purely to
  suppress an unused-variable warning, which disappears once the fix collects only
  the session ids.

Splitting by shape would have put two phases into the same file. **Splitting by
file keeps each file wholly owned by one phase** — which matters here, because the
non-zero-count criteria that guard these splits get fiddly when two phases share a
file, and those criteria are what has caught over-reach in the last four phases.

### 05a is uniform; 05b is small but mixed

**05a — all three sites spawn tmux subprocesses while holding the guard**, and all
three take the same fix: collect under the lock, release, then act.

- `helpers.rs::notify_session` — 2 file writes **plus** a `tmux display-message`,
  with the guard held from acquisition to the end of the function (~50 lines).
- `gc.rs::gc_bg_windows` — `kill_job_window` per window, looped over **every**
  session.
- `hook.rs:92` — `store.retain(|_, entry| { … entry.cleanup_bg_windows(); … })`,
  where `cleanup_bg_windows` runs `kill_job_window` per window **plus**
  `stop_pipe_pane`.

**`cleanup_pass` (`src/daemon/session.rs`) is the worked example for all three** —
it is the fix that resolved the confirmed production hang, and `hook.rs:92` is
literally the same defect, never fixed there. Quote it when drafting.

**05b — 3 small sites, two shapes:** one subprocess loop (`notify_chat_panes`), two
plain file-write hoists (`inject_into_sessions`, `stream.rs:722`'s
`write_session_meta`).

### Line numbers moved — re-derive before drafting

These were recorded earlier as `stream.rs:719` and `hook.rs:91`. After 04j's
conversions they are **`stream.rs:722`** and **`hook.rs:92`**. Re-derive every site
with the multi-line-aware scan when drafting; `grep -c` stays retired.

### Also unassigned

The two `ask.rs` multi-line stragglers (`:519`, `:686`) are **conversions**, not
restructures. Natural home is **05c**, where they will fail to compile if missed —
but they could equally ride along with whichever phase next touches `ask.rs`.
05c also inherits 04f's coverage follow-up (three vacuous `compaction_in_flight`
assertions to make real, mutation-checked) and the 13 test-module acquisitions.
**05c is still the phase most in need of re-scoping.**

### Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 7 clean confirmations).**
2. **Specs asserting test coverage (3rd occurrence, 6 clean confirmations).**
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Now rides on **05c**, not 05 — it is a
   test-hygiene item and 05a/05b add no tests.

**All four await your sign-off.** Threads 1 and 2 have substantial data now; the
conversion sweep is finished and the remaining phases are a different shape, so
this is a natural moment to fold either.

---

## Superseded: the 04j completion note

> **Everything below this line is historical** — per-phase notes kept as a record
> of what was true at the time. Where it discusses numbering (`04k`, an unsplit
> `phase 05`) or an open ordering question, **the top of this file supersedes it.**

---

## M5 phase-04j — convert-stream-hooks is `done`

(2026-07-26, `approved_first_try`, no bounces, 89 turns, commit `3e8466e`.)

All 10 conversion sites done — `stream.rs` 8, `hook.rs` 2 — each file left with
exactly the **1** raw acquisition phase 05 owns (`stream.rs:722`,
`hook.rs:92`). Gates green, 915 tests unchanged, every criterion exact including
all four non-zero ones and the `UnpoisonExt` asymmetry (deleted from `stream.rs`,
kept in `hook.rs` for `bg_session`).

Verified by reading rather than counting:

- **Task 3's boundary is correct** — the persist closure closes with `});` on the
  line immediately before `if needs_compaction {`, leaving `write_session_file`,
  `append_session_message`, `stream.rs:722` and `spawn_compaction` outside.
- **`spawn_compaction` is not inside any closure**, and its warning comment
  survives verbatim, including the "`std::sync::Mutex` is not reentrant and
  re-locking would deadlock" line. That comment is the institutional memory of a
  confirmed production defect.
- **Task 4's one-shot semantics are intact** — the flag assignment is inside the
  closure, the threshold test is still `==`, and the `.await` is outside. Moving
  the flag out would let two turns both suggest; loosening `==` would suggest
  every turn after the threshold. Neither would fail a test.

### The conversion sweep is done

Converted across 04a–04j: `server/handlers.rs`, `server/ask.rs` (bar two
multi-line stragglers), the whole `executor/` subtree, `context/background.rs`,
`briefing.rs`, `ghost.rs`, `background/run.rs`, `background/respawn.rs`,
`stream.rs`, `hook.rs`.

**Still holding raw acquisitions — all six are phase 05's, all mechanism A/B:**

| Site | Blocking work under the guard |
|---|---|
| `webhook/process.rs` (2) | disk writes + a timeout-free tmux subprocess |
| `background/helpers.rs::notify_session` | 2 file writes **+** a `tmux display-message` subprocess |
| `background/gc.rs::gc_bg_windows` | `kill_job_window` per window, looped over every session |
| `stream.rs:722` | `write_session_meta` (file write) |
| `hook.rs:92` | `cleanup_bg_windows()` → `kill_job_window` per window **+** `stop_pipe_pane` |

Plus the two unassigned `ask.rs` multi-line stragglers (`:519`, `:686`), which are
conversions rather than restructures.

## ~~What to draft next — an ordering decision comes first~~ — RESOLVED

> Resolved by the PE on 2026-07-26: renumber so 05 lands first, split in two. See
> "The renumber and split" at the top. The three options below are kept only to
> show what was weighed.

The numbering currently implies **04k (newtype) → 05 (restructures)**, and that is
**backwards**. 04k makes raw `.lock()` stop compiling, so it cannot land while six
raw acquisitions remain. Three options, and this is a scoping call rather than
something I should pick unilaterally:

1. **Renumber: 05 before 04k.** Draft the six restructures first, then the newtype
   closes the door behind them. Cleanest dependency order; costs one renumbering.
2. **04k absorbs the six.** Makes the newtype phase enormous — 13 `Arc::clone`
   sites, 13 test-module acquisitions, 2 `ask.rs` stragglers, 6 restructures, and
   04f's coverage follow-up. Almost certainly too big for one session.
3. **Split 05 first, then newtype.** Phase 05 is already 6 restructures sharing one
   shape (`cleanup_pass` in `session.rs` is the worked example for four of them);
   it may want to be two phases — the two tmux-subprocess ones and the rest.

My recommendation is **option 1 with 05 split in two**: the subprocess-under-lock
group (`helpers.rs`, `gc.rs`, `hook.rs:92`) shares the `cleanup_pass`
collect-then-act shape, while `stream.rs:722` and `webhook/process.rs` are plainer
file-write hoists. That also lets the two `ask.rs` stragglers ride along with
whichever touches `ask.rs` next, or fold into the newtype phase where they will
fail to compile if missed.

### Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 7 clean confirmations).** Third draft running
   with no pre-dispatch correction needed.
2. **Specs asserting test coverage (3rd occurrence, 6 clean confirmations).** 04j
   was the strongest instance: it stated that **none** of its ten sites is covered
   by the unit suite, making a coverage claim impossible rather than unproven.
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Still riding on phase 05.

**All four await your sign-off.** Threads 1 and 2 now have substantial supporting
data — seven and six clean confirmations respectively, against the four counting
errors and three false coverage claims that motivated them. If you want either
folded into `WORKFLOW.md`, this is a natural moment; the milestone's conversion
work is finished and the next phases are a different shape.

---

## Superseded: the 04j dispatch note

## 12 sites, but only 10 are conversions

Reading all twelve found **two more mechanism-A/B defects**, moved to **phase 05**
on the same principle applied to `background/`: restructures go with the phase whose
purpose is removing blocking work from critical sections, not into a conversion
sweep. No renumbering — phase 05 grows from 4 restructures to **6**.

- **`stream.rs:719`** holds the guard across `write_session_meta` — a file write
  inside the critical section. Needs the build-inside / write-outside hoist.
- **`hook.rs:91`** is the more serious one. Its
  `store.retain(|_, entry| { … entry.cleanup_bg_windows(); … })` calls a function
  that spawns **one `tmux kill_job_window` per background window plus
  `stop_pipe_pane`** — every one of them under the global session lock. **This is
  the same defect class as the confirmed production hang** that opened this
  milestone, and `cleanup_pass` in `session.rs` already models the fix (collect
  under the lock, act outside). It has simply never been applied here.

Phase 05 is now large enough that it should be **re-scoped or split when drafted** —
six restructures, though they share one shape and one worked example.

## What 04j does

10 sites: `stream.rs` 8 + `hook.rs` 2. `size=m`, ~150 lines. Nine are mechanical;
**one is not.**

**`stream.rs:751`** is a six-element `let`-chain that is simultaneously a guard and
a mutation — it decides whether to suggest a session name *and* sets
`auto_name_suggested = true` so the suggestion fires exactly once. The spec pins
three things: all five remaining conditions stay inside the closure in order, the
flag assignment stays inside (setting it after would let two turns both suggest),
and the threshold test stays `==` rather than `>=`.

**`stream.rs:689` is the most consequential boundary in the phase.** Its closure
must close before `if needs_compaction`, because everything after it —
`write_session_file`, `append_session_message`, `stream.rs:719`, and
`spawn_compaction` — must stay outside. `spawn_compaction` **re-locks the store**,
and the code already carries a comment saying so; the spec requires keeping it
verbatim. Worth noting: because `context/background.rs` was converted earlier, a
closure enclosing it would now trip the re-entrancy **assertion** — a loud panic
instead of the silent hang it used to be. Still a bug, just a louder one.

**`stream.rs:896` is the multi-line site** `grep -c` cannot see. The Pre-flight
makes the gap concrete: the scan prints **9** for `stream.rs` while `grep -c` prints
**8**, and the doc says so explicitly so the discrepancy reads as expected rather
than as a stale count.

## Two asymmetries the spec calls out, either of which breaks the build if inverted

1. **`UnpoisonExt`: delete from `stream.rs`, keep in `hook.rs`.** `stream.rs`'s only
   `unwrap_or_log` is the site being converted. `hook.rs:116` uses it on
   **`bg_session`** — a different mutex entirely — so deleting it there breaks the
   build. Recent phases have all ended in "delete the import", which makes this the
   easy mistake.
2. **`hook.rs` defines its own local `SessionStore` type alias** rather than
   importing the canonical one. It expands to the identical type, so
   `with_sessions(&sessions, …)` type-checks unchanged. The spec explicitly forbids
   replacing the alias — that is a separate cleanup and would widen the diff for no
   behavioral gain.

Both files pass `&sessions` with the ampersand: `stream.rs` destructures it by value
out of `ConversationLoopCtx`, and all three `hook.rs` handlers take it by value.

## Criteria validated before pinning — third clean draft running

Every value checked against the tree: stream.rs 9, hook.rs 3, `grep -c` 8 (the gap),
`with_sessions` 0/0, `UnpoisonExt` 1/1, `spawn_compaction` 2, helpers/gc 1 each,
earlier phases 0. **No corrections needed.**

**Four criteria are deliberately non-zero:** `stream.rs` → 1, `hook.rs` → 1,
`helpers.rs` → 1, `gc.rs` → 1. All four are phase 05's sites. A zero anywhere means
the executor converted a restructure out of scope — the specific over-reach these
splits create, and the guard that has now caught nothing precisely because it is
stated.

One criterion is explicitly marked as unprovable by counting: `spawn_compaction`
must appear twice **and not be inside a closure** — the doc says the count cannot
show that and it must be read.

## Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 6 clean confirmations).**
2. **Specs asserting test coverage (3rd occurrence, 5 clean confirmations).** 04j is
   the strongest instance yet: it states that **none** of its ten sites is covered
   by the unit suite — `run_conversation_loop` needs a live AI client, tmux session
   and IPC peer, and `hook.rs` has no test module at all — so a coverage claim here
   would be plainly false.
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Riding on phase 05.

**All await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change made.**

## Remaining after 04j

**04k** (newtype + enforce) → **05** (6 mechanism-A restructures, re-scope) → 06 →
07, plus the independent 08–11 instance-hardening set.

**04k still needs re-scoping** — 13 `Arc::clone` sites, 13 test-module acquisitions,
the two unassigned `ask.rs` multi-line stragglers, and 04f's coverage follow-up.
Note it will **not** compile until phase 05's six sites are also converted, since
the newtype makes raw `.lock()` illegal — so **05 must land before 04k**, or 04k
must absorb them. Decide that when drafting; the current numbering implies the wrong
order.

---

## Superseded: the 04i completion note

---

## M5 phase-04i — convert-background-windows is `done`

(2026-07-26, `approved_first_try`, no bounces, 76 turns, commit `08e2b49`.)

All 7 mechanical sites converted — `run.rs` 4, `respawn.rs` 3, both at **0** raw
acquisitions. Gates green, 915 tests unchanged, every criterion exact.

Verified by reading rather than counting: all seven calls use `&sessions` with the
ampersand (4 + 3, none reverting to the reference convention used elsewhere in the
daemon); all three `.find(…)` lookups stayed inside their closures rather than
being hoisted or dodged by cloning; `respawn.rs:106` still assigns
`w.exit_code = None` for the retry reset while the other two assign
`Some(exit_code)`; and `tmux::kill_job_window` remains outside its closure in both
files.

**The two non-zero criteria did their job.** `helpers.rs` and `gc.rs` each still
hold exactly **1** raw acquisition — pinning them at 1 rather than 0 is what would
have caught an over-eager sweep into phase 05's restructure sites.

### Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 5 clean confirmations).** Second phase running
   with no draft correction needed.
2. **Specs asserting test coverage (3rd occurrence, 4 clean confirmations).** 04i
   stated outright that the `bg_windows` updates these sites perform are **not**
   covered by the unit suite, so a coverage claim would have been false rather than
   merely unproven — and none was made.
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Riding on phase 05.

**All await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change made.**

### Where the conversion stands

Converted: `server/handlers.rs`, `server/ask.rs` (bar two known multi-line
stragglers), the whole `executor/` subtree, `context/background.rs`,
`briefing.rs`, `ghost.rs`, `background/run.rs`, `background/respawn.rs`.

Remaining: **04j** `stream.rs` (9, incl. the multi-line site at `:896`) +
`hook.rs` (3) = 12 → **04k** newtype + enforce → **05** (now 4 mechanism-A
restructures) → 06 → 07, plus the independent 08–11 instance-hardening set.

**Two things to carry into drafting 04j.** `stream.rs` calls `spawn_compaction`,
and holding the `sessions` guard across that call is a confirmed historical
self-deadlock in this codebase; `context/background.rs` was converted first
precisely so that a `stream.rs` closure enclosing it now trips the re-entrancy
**assertion** (a loud panic) instead of hanging silently. Say so in the hazard
table. And **re-derive every line number with the scan** — `stream.rs` has one
multi-line acquisition that `grep -c` cannot see.

**04k still needs re-scoping when drafted** — 13 `Arc::clone` sites, 13
test-module acquisitions, the two unassigned `ask.rs` stragglers, and 04f's
coverage follow-up. It may need its own split.

---

## Superseded: the 04i dispatch note

## `background/` was 9 sites; only 7 belong in a conversion phase

Reading all nine found that **two are not conversions at all** — they are
mechanism-A/B defects that need restructuring, and they have moved to **phase 05**
(`unlock-blocking-paths`), whose stated purpose is exactly that. No renumbering was
needed; phase 05's scope grew from 2 sites to 4.

- **`helpers.rs::notify_session`** holds the guard from acquisition to the **end of
  the function** — roughly 50 lines — spanning `related_knowledge_hints`,
  `append_session_message` (**two file writes**), and a `tmux display-message`
  **subprocess spawn**. All under the global session lock. Fixing it needs a
  read phase → unlocked work phase → short write phase, not a wrap.
- **`gc.rs::gc_bg_windows`** holds the guard across `tmux::kill_job_window` inside
  a loop over **every session** — one subprocess per window, under the global lock.
  `cleanup_pass` in `session.rs` is the established precedent for the fix shape
  (collect under the lock, act outside).

Both are squarely mechanism A + mechanism B, and phase 05 already owns that
territory. Bundling them into a conversion phase would have mixed a mechanical
7-site sweep with two restructures — the same mistake that made the ghost group
need splitting.

## 04i is the easiest phase of the 04x sequence

7 sites: `run.rs` (4) + `respawn.rs` (3). A scoped read and six `let`-chains, none
containing an early `return`, `break`, `.await`, or blocking work. `size=s`,
~90 lines, 7 sites → 7 calls, no collapses.

**The one thing that will bite a careless edit** is the receiver form. Both
enclosing functions take `sessions: SessionStore` **by value**, so every call is
`with_sessions(&sessions, …)` **with the ampersand** — unlike everywhere else in
the daemon, where the parameter is `&SessionStore`. The previous phase hit exactly
this mismatch when my snippet used the wrong convention, so 04i states it up front
and uniformly: all seven take `&sessions`.

Three of the seven are 4-element chains whose `.find(…)` must stay **inside** the
closure, since it borrows `entry` and a `&mut` into the map cannot escape. Exact
target code given for each.

**`run.rs` loses its `UnpoisonExt` import** — it has exactly one `unwrap_or_log`
(the site being converted) and no test module, so the outcome is deterministic:
delete. `respawn.rs` never had the import and must not gain one. Task 8 also
frames a surviving `unwrap_or_log` as evidence of a missed conversion rather than a
reason to keep the import.

## Criteria validated before pinning — and this time nothing was wrong

Every pinned value was checked against the tree while drafting: run.rs 4, respawn.rs
3, helpers.rs 1, gc.rs 1, ghost/background/executor 0, `with_sessions` 0/0,
`UnpoisonExt` 1, and **no `#[cfg(test)]` module in either file** (so every hit is
production). No corrections were needed — the first clean draft in four phases.

**Two criteria are deliberately non-zero:** `helpers.rs` and `gc.rs` must each
still print **1**. A zero there means the executor converted phase 05's
restructure sites out of scope, which is the specific over-reach this split
creates.

## Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 4 clean confirmations).** 04i's draft needed
   no correction at all.
2. **Specs asserting test coverage (3rd occurrence, 3 clean confirmations).** 04i
   goes further than "name no discriminator": it states that the `bg_windows`
   registry updates these sites perform are **not** covered by the unit suite — a
   pre-existing gap it neither widens nor closes — so a coverage claim here would
   be not just unproven but false.
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Riding on phase 05, which just grew.

**All await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change made.**

## Remaining

04i (7) → 04j (`stream.rs` 9 + `hook.rs` 3 = 12) → **04k** (newtype + enforce) →
05 (now **4** mechanism-A restructures) → 06 → 07, plus the independent 08–11
instance-hardening set.

**04k still needs re-scoping when drafted** — 13 `Arc::clone` sites, 13
test-module acquisitions, the two unassigned `ask.rs` multi-line stragglers, and
04f's coverage follow-up. It may need its own split.

---

## Superseded: the 04h completion note

---

## M5 phase-04h — convert-ghost-turn-loop is `done`

(2026-07-26, `approved_first_try`, no bounces, 84 turns, commit `aca7dc2`.)

All 8 remaining sites in `ghost.rs` converted — **11 `with_sessions` calls, 0 raw
acquisitions**, `UnpoisonExt` import deleted exactly as predicted. Gates green,
915 tests unchanged, every count exact.

**This phase also fixed a live mechanism-A defect**, not just converted syntax:
`append_session_message` was writing two files *inside* the critical section. It
now runs after the closure, gated on a `pushed` flag so a vanished session still
appends nothing. That conditionality was the phase's only silent-failure risk — an
unconditional hoist would have kept every gate green while appending for entries
that no longer exist. Verified by reading, not inferred.

Also verified by reading (counts cannot prove these): both `break` statements
stayed outside their closures — including task 7's, which abuts the closing brace
and was flagged as easy to swallow — task 8's `append_session_message` ordering is
unchanged, `write_session_file` stayed outside, and the `bail!` string is
byte-identical by literal `grep -cF`.

**One correct deviation:** task 1 uses `with_sessions(&sessions, …)` because
`start_session_with_config` takes an owned `SessionStore`, not a reference. My
snippet was written from the `do_ghost_turn` convention; the executor adapted
rather than copying it verbatim. Worth noting as evidence that quoted snippets are
read as intent, not transcribed — but also that a snippet whose receiver type
differs from its destination is a small drafting flaw I should catch.

### Where the conversion stands

Converted: `server/handlers.rs`, `server/ask.rs` (bar two known stragglers),
`executor/` (whole subtree), `context/background.rs`, `briefing.rs`, `ghost.rs`.

Remaining: **04i** `background/{run,respawn,helpers,gc}.rs` (9) → **04j**
`stream.rs` (9, incl. one multi-line at `:896`) + `hook.rs` (3) = 12 → **04k**
newtype + enforce.

### Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 3 clean confirmations).** 04h's counts all
   came out exact; the pre-dispatch validation caught two draft errors
   (`append_session_message` is 4 not 3; 7 `unwrap_or_log` calls not 8).
2. **Specs asserting test coverage (3rd occurrence, 2 clean confirmations).**
   Third consecutive phase where the Test plan named no discriminator and the
   Update Log made no coverage claim — nothing to refute at review.
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Still riding on phase 05.

**All await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change made.**

**04k still needs re-scoping when drafted** — the 13 `Arc::clone` sites, plus 13
test-module acquisitions, plus the two unassigned `ask.rs` multi-line stragglers,
plus 04f's coverage follow-up (three vacuous `compaction_in_flight` assertions to
make real, mutation-checked). It may need its own split.

---

## Superseded: the 04h dispatch note

Converts the last **8** sites in `ghost.rs` — 1 in `start_session_with_config`,
7 in `do_ghost_turn` — finishing the file. **Finish condition: 11
`with_sessions` calls (3 from 04g + 8 here), 0 raw acquisitions.** `size=m`,
~140 lines, 8 sites → 8 calls with no collapses.

## Three hard cases, each failing differently

- **Site 306 — `anyhow::bail!` inside the guard.** Expands to `return Err(..)`
  from `do_ghost_turn`; inside a closure it returns from the closure and the
  types no longer line up. Spec has the `Option` + outside-`bail!` shape.
- **Site 466 — a blocking file write inside the critical section.** A live
  mechanism-A defect: `append_session_message` writes two files while the global
  session lock is held. **And the hoist is not mechanical** — the write sits
  *inside* the `if let Some(entry)`, so it currently only happens when the entry
  exists. Hoisting it unconditionally would append for a vanished session. The
  spec returns a `pushed` flag and gates the write on it.
- **Site 485 — a bare `break;` inside the guard.** Exits the turn loop; inside a
  closure that is `E0267`. Fails loudly rather than silently, but the spec tells
  the executor to write it correctly rather than discover the error.

**A fourth trap, easy to hit by accident:** site 847's `break;` sits flush against
the closing brace of the block being converted, and belongs to the surrounding
event loop. Pulling it into the closure is the same `E0267`. Called out explicitly.

## Worked example for the hoist comes from the same file

`ghost.rs:1003` already does it right — `append_session_message` **before** the
lock, lock-free by construction:

```rust
append_session_message(session_id, &assistant_msg);
{ let mut store = sessions.lock()…; if let Some(entry) = … { … } }
```

Task 4 reaches the same property from the other side (write *after*, gated on the
flag), and the spec explicitly forbids "harmonizing" the two — 1003's write is
unconditional and 466's must stay conditional.

## Re-deriving the line numbers caught the shift, and validating criteria caught two more errors

**All eight sites moved by −3** after 04g edited the top of `ghost.rs` (257→254,
309→306, 328→325, 469→466, 488→485, 511→508, 850→847, 1008→1005). Re-derived with
the scan, exactly as the previous entry warned.

Then validating the criteria against the tree — rather than deriving them — caught
two further errors in my own draft:

1. I wrote `grep -c "append_session_message"` should return **3**. It is **4**:
   there is an unrelated call at `ghost.rs:212` in `start_session_with_config`
   that appends the initial user message. The criterion now says 4, names that
   call as out of bounds, and notes that the count alone cannot prove none sits
   inside a closure — that needs reading.
2. I wrote that the phase removes "the last 8 `unwrap_or_log` calls". It is **7** —
   site 847 uses `if let Ok(mut store) = sessions.lock()` and has no
   `unwrap_or_log`. Task 9 now states the verified expected outcome (delete the
   `UnpoisonExt` import, since none of the 7 are in the test module) while still
   requiring the executor to confirm before acting, and treats "hits remain
   outside `mod tests`" as evidence of a missed conversion rather than a reason to
   keep the import.

That is three drafting errors caught pre-dispatch in two consecutive phases, all
by running checks instead of reasoning about them. The practice is working; the
underlying tendency has not gone away.

## Calibration — four threads, none folded

1. **Count criteria (4th occurrence).** Now **3 clean confirmations** — 04g's
   pre-dispatch catch, and 04h's two.
2. **Specs asserting test coverage (3rd occurrence, 1 confirmation).** 04h's Test
   plan again names no discriminator and states the rule explicitly: "the tests
   pass" is admissible, "the tests would catch a regression in task 4" is not.
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Still riding on phase 05.

**All await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change made.**

## After 04h

`ghost.rs` and the whole `executor/` subtree will be fully converted. Remaining:
04i (`background/*`, 9) → 04j (`stream.rs` 9 + `hook.rs` 3 = 12) → **04k**
(newtype + enforce).

**04k still needs re-scoping when drafted** — the 13 `Arc::clone` sites, plus 13
test-module acquisitions, plus the two unassigned `ask.rs` multi-line stragglers,
plus 04f's coverage follow-up (three vacuous `compaction_in_flight` assertions to
make real, mutation-checked). It may need its own split.

---

## Superseded: the 04g completion note

---

## M5 phase-04g — convert-ghost-exit-paths is `done`

(2026-07-26, `approved_first_try`, no bounces, 71 turns, commit `6a2c035`.)

All 4 exit-path sites converted — 3 in `write_mailbox_on_exit`, 1 in
`generate_and_save_briefing`. `ghost.rs` is at **8** raw acquisitions (the turn
loop, untouched as required), `briefing.rs` at **0**. Every count came out exact
under the multi-line-aware scan; gates green; 915 tests unchanged.

Verified by reading the diff rather than the summary: the two miss paths in
`write_mailbox_on_exit` stayed separate ("entry absent" vs "entry present, no
agent"), `log::warn!` moved outside the closure so `briefing.rs` no longer logs
under the global lock, and all three contract-bearing strings are byte-identical
by literal `grep -cF` against the parent. The `UnpoisonExt` conditional resolved
correctly — top-level import removed, `mod tests`' own retained at line 151, with
both `cargo build` and `cargo clippy --all-targets` passing.

### Both corrected drafting practices held — first clean confirmation

Worth recording after four counting slips and three false coverage claims:

1. **Running the criterion instead of deriving it caught an error pre-dispatch.**
   The scan said `ghost.rs: 11` where my draft said 12, in two places. Fixed
   before dispatch — and then **every count criterion came out exact at review**,
   the first phase in this milestone needing no post-hoc correction.
2. **Naming no discriminating test produced an honest Update Log.** The Test plan
   asked only for tests run and observed and forbade unproven coverage claims. The
   executor reported exactly that, plus the reasoning check on the two miss paths.
   **No coverage claim was made, so none needed refuting** — versus the previous
   two phases, where a planted conclusion produced a false claim that took a
   mutation to disprove.

That is now two clean data points for thread 1 and one for thread 2.

### Next up: 04h is the hard one

`start_session` (1 site) + `do_ghost_turn` (7) = **8 sites**, and it carries all
three cases that motivated splitting the ghost group:

- **`ghost.rs:309`** — `anyhow::bail!` inside the guard (a `return Err` from the
  enclosing async fn).
- **`ghost.rs:469`** — `append_session_message(...)` called **inside** the
  critical section: blocking file I/O under the global session lock, a live
  mechanism-A defect the conversion must **hoist**, not preserve. Note
  `ghost.rs:1008` already does it the right way (append *before* the lock) — quote
  that as the worked example when drafting.
- **`ghost.rs:488`** — a bare `break;` inside the guard, exiting the turn loop.
  Inside a closure that is a **compile error**, so it fails loudly rather than
  silently — but it will stall an executor that tries the mechanical wrap first.
  Spell out the extract-then-act shape.

**Line numbers will have shifted** — 04g removed ~3 lines from the top of
`ghost.rs`. Re-derive every site with the scan before drafting; do not reuse the
numbers above without re-checking them.

### Calibration — four threads, none folded

1. **Count criteria (4th occurrence, 2 clean confirmations).**
2. **Specs asserting test coverage (3rd occurrence, 1 clean confirmation).**
3. **Fixture defaults neutering assertions (1st, from 04f).**
4. **Lock/HOME test hygiene (3 deep).** Still on phase 05.

**All await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change made.**

### Remaining 04x work

04h (8, the hard ghost cases) → 04i (`background/*`, 9) → 04j (`stream.rs` 9 +
`hook.rs` 3 = 12) → **04k** (newtype + enforce).

**04k still needs re-scoping when drafted** — the 13 `Arc::clone` sites, plus 13
test-module acquisitions, plus the two unassigned `ask.rs` multi-line stragglers,
plus 04f's coverage follow-up (three vacuous `compaction_in_flight` assertions to
make real, mutation-checked). It may need its own split.

---

## The ghost group was split — 04g (4 sites) + 04h (8 sites)

The planned "04g = ghost.rs + briefing.rs, 12 sites" is now two phases. Reading
all 12 sites found **three individually hard cases, all inside `do_ghost_turn`**,
each with a different failure mode:

- **`ghost.rs:309`** — `anyhow::bail!` inside the guarded block, i.e. a
  `return Err(..)` from the enclosing async fn.
- **`ghost.rs:469`** — `append_session_message(...)` is called **inside** the
  critical section. That is blocking file I/O under the global session lock — a
  live mechanism-A defect, and the conversion must **hoist** it, not preserve it.
- **`ghost.rs:488`** — a bare `break;` inside the guarded block, exiting the
  enclosing turn loop. Inside a closure that is **a compile error**, not a silent
  behaviour change — so unlike 04f's traps it fails loudly, but it will stall an
  executor that tries the mechanical wrap first.

Bundling those with the four easy exit-path sites risked one confusion consuming
the run, on the most safety-sensitive subsystem in the daemon (autonomous
remediation). So:

- **04g — ghost exit paths (drafted):** `write_mailbox_on_exit` (3) +
  `briefing.rs` (1) = **4 sites**, `size=s`, ~70 lines. Two have a plain `return`
  inside the guard; the region has **no store-touching callees at all** (verified:
  `src/agents/mailbox.rs` has zero `SessionStore` references, and
  `do_generate_briefing` does not take `sessions`), so the §3.5 deadlock hazard is
  absent here.
- **04h — ghost turn loop (not drafted):** `start_session` (1) + `do_ghost_turn`
  (7) = **8 sites**, carrying all three hard cases above.

Undrafted phases renumbered: background-windows → 04i, stream-hooks → 04j,
newtype → 04k. Second free renumbering; the tail is now 04d–04k.

## What 04g pins, and how the counts were derived

**Two acceptance criteria are deliberately non-zero:** `ghost.rs` must end at
**8** raw acquisitions, not 0. A zero there means the turn loop was converted out
of scope, which is the specific over-reach this split creates.

`briefing.rs` carries a conditional: converting its only site may leave
`use crate::util::UnpoisonExt;` unused, and `cargo build` vs
`cargo clippy --all-targets` **disagree** about whether a test-only import counts
as used. That disagreement is what produced 04f's `hard_fail`, so the spec makes
the check conditional (grep for remaining `unwrap_or_log`, then delete *or* move
into `mod tests`) and requires both commands.

**I ran the scan before pinning, and it caught me.** I had written 12 as
`ghost.rs`'s production count in two places; the actual value is **11** (plus
`briefing.rs`'s 1 = 12 for the group). Both were corrected before the doc landed.
That is the fourth counting slip in this milestone and the first one caught before
dispatch rather than at review — the practice of running the criterion instead of
deriving it is what made the difference, and the doc now says so inline.

The scan script is inlined in 04g's Pre-flight as `/tmp/scan_locks.py` rather than
referenced, so the executor does not have to reconstruct it from the bug doc.

## Also applied from the 04f review

**The Test plan no longer names a discriminating test.** Given three consecutive
phases where I asserted coverage that didn't exist, 04g's Test plan says: run the
mailbox/briefing tests, report what you observed, and — explicitly — *"Do not
claim any of these 'guards' a specific line. A claim about what a test would catch
is only admissible in this project if you demonstrate it by mutation."* No
mutation is required, because no test change is required.

## Still open for the PE — four threads, none folded

1. **Count criteria (4th).** `grep -c` retired; multi-line-aware scan now inlined
   in the phase doc that needs it.
2. **Specs asserting test coverage (3rd).** 04g is the first phase drafted under
   the corrected rule (never name the discriminator; require mutation proof or
   claim nothing).
3. **Fixture defaults neutering assertions (1st).** From 04f —
   `make_test_entry()` defaulting `compaction_in_flight: false` made three
   assertions tautological.
4. **Lock/HOME test hygiene (3 deep).** Still riding on phase 05.

## Remaining 04x work

04g (4) → 04h (8, the hard ghost cases) → 04i (`background/*`, 9) → 04j
(`stream.rs` 9 + `hook.rs` 3 = 12) → **04k** (newtype + enforce).

**04k is materially larger than its name suggests** — the 13 `Arc::clone` sites,
**plus** 13 test-module acquisitions (11 in `context/background.rs`, 2 in
`session.rs`), **plus** the two unassigned `ask.rs` multi-line stragglers,
**plus** 04f's coverage follow-up (make three vacuous `compaction_in_flight`
assertions real, mutation-checked). Re-scope it when drafting; it may need its own
split.

---

## M5 phase-04f — convert-context-background is `done`

(2026-07-26, `approved_after_1`, 1 bounce + 1 `hard_fail`, 209 executor turns
across three runs: 37 + 71 + 101. Commits `4f60a9a`, `0984efa`.)

All **4** production sites in `context/background.rs` now use `with_sessions`,
verified by a multi-line-aware scan: 0 production raw acquisitions, 4
`with_sessions(` calls, 11 test-module acquisitions untouched, `UnpoisonExt`
import relocated to `mod tests`. Gates green, 915 tests unchanged.

**Before drafting 04g, read this — it is the one durable lesson from the run.**

### The three-run arc, and what each failure actually was

1. **First run → bounced (`spec_bug`).** Converted the 2 sites my spec inventoried.
   The other 2 were invisible to `grep -c "sessions\.lock()"` because they split
   `sessions` and `.lock()` across lines. My criterion reported success while the
   goal was unmet.
2. **Re-dispatch → `hard_fail` (`NoProgressStall`, 60 read-only turns).** It made
   both remaining conversions correctly and deleted the now-unused header import —
   then stalled for ~40 turns re-grepping `UnpoisonExt` without ever running a
   gate. Clippy would have printed the fix.
3. **Resume → complete.** One-line edit (`use crate::util::UnpoisonExt;` inside
   `mod tests`). Resume was the right lever: the spec already contained the
   remaining edit verbatim, so refined re-dispatch would have added emphasis, not
   information.

### The coverage finding — this is the part worth carrying forward

Bug-doc item 3 asked the executor to prove that
`background_swap_discards_on_new_turn` guards the **stale branch's** flag-clear
ordering. **It could not, because the claim was false**, and it said so instead of
faking it. Resolved by mutation at review: that test's snapshot holds one message,
so step 1 finds no viable cut and it exits through the **"no viable cut"** branch —
which it *does* genuinely guard (removing that clear makes it fail).

Mutation-established coverage of all four flag-clearing sites:

| Site | Guarded? |
|---|---|
| "no viable cut" discard | **yes** — one of the two sites this phase converted |
| idempotency-guard discard | no |
| stale-branch discard | no |
| swap path | no |

**Root cause, and the generalisable trap:** `make_test_entry()` defaults
`compaction_in_flight: false`, so `assert!(!entry.compaction_in_flight)` is
**tautological** in any test that hand-builds a `CompactionSnapshot` instead of
routing through `try_snapshot`. Only one of the three tests asserting that flag
calls `try_snapshot`, so the other two are decorative.

Approved rather than bounced twice because 3 of the 4 gaps **pre-date this phase**
(no regression; it net *added* one real guard), and a third cycle would charge an
architect error to the model.

**Follow-up assigned to 04j**, which must rewrite all 11 test-module acquisitions
anyway once the newtype makes raw `.lock()` stop compiling: make the three vacuous
flag assertions real and mutation-check each. Do not open a separate phase.

### Calibration — the count is now four threads, three of them mine

1. **Count criteria (4th).** Retired `grep -c` for this purpose. Every remaining
   phase must use the multi-line-aware scan in `bugs/bug-04f-1.md` § Verification.
2. **Specs asserting test coverage (3rd).** 04d's vacuous `try_lock` proxy, 04f's
   wrong-branch claim, and now the confirmed fixture-default trap behind it. The
   candidate fold has sharpened: **a spec must never name the discriminating test;
   it must require the executor to demonstrate discrimination by mutation and quote
   the fail/pass pair.** That shape worked here — it is exactly what surfaced the
   defect from the executor instead of from my review.
3. **Fixture defaults make assertions vacuous (new, 1st occurrence).** A shared
   `make_*` fixture that defaults a field to the asserted-for value silently
   neuters every assertion on it. Worth watching for recurrence before folding.
4. **Lock/HOME test hygiene (3 deep).** Still riding on phase 05.

**None folded. All await your sign-off.**

### Remaining 04x work

04g (`ghost.rs` 11 + `briefing.rs` 1 = 12) → 04h (`background/*`, 9) → 04i
(`stream.rs` 8 + 1 multi-line + `hook.rs` 3 = 12) → 04j (newtype + enforce; the 13
`Arc::clone` sites, **plus** 13 test-module acquisitions, **plus** the two
unassigned `ask.rs` multi-line stragglers, **plus** the coverage follow-up above).
**04j is materially larger than its one-line description suggests** — re-scope it
when drafting.

## ⚠ The measuring instrument was wrong for six phases

`grep -c "sessions\.lock()"` — the criterion used by **every** count check in
04a–04f — **cannot see an acquisition that splits `sessions` and `.lock()` across
lines.** A multi-line-aware scan found **5 production sites** that every prior
survey and every acceptance criterion missed:

| File:line | Consequence |
|---|---|
| `context/background.rs:118`, `:137` | **04f bounced** — 2 of its 4 sites left raw |
| `server/ask.rs:519`, `:686` | **04c was approved as "fully converted" and is not** |
| `stream.rs:896` | 04i's problem; add to that phase's inventory (9 + 3 = 12 sites) |

This is my third counting error in this milestone, and the worst of the three,
because the previous two were bad arithmetic while this one was a **blind
instrument that reported success**. 04f's Finish condition ("0 raw
`sessions.lock()` in the production region") was satisfied as measured and false in
substance.

**Every remaining phase must use a multi-line-aware scan.** A working one is in
`bugs/bug-04f-1.md` § Verification. `grep -c` is retired for this purpose.

### 04c: corrected, not reopened

`ask.rs:519` and `:686` are genuine `SessionStore` acquisitions; the first still
carries the `.ok()?` poison-bail this milestone exists to remove. A correction
block is appended to `phase-04c-convert-ask.md`.

The phase **stays `done`** and the verdict **stays `approved_first_try`**: the
executor converted every site the spec inventoried, and the miss originates in my
survey instrument. The two stragglers are **unassigned** in the README count
table — fold them into whichever phase next touches `ask.rs`, or leave them for
the newtype phase, which will fail to compile until they are converted.

### 04f bounce, in two parts

**Part 1 (major, the reason for the bounce).** `run_compaction` still holds two
raw locks at `background.rs:118` and `:137`, both clearing
`compaction_in_flight` on an early-discard path. The bug doc has exact target
code for both.

There is a trap in the fix, called out in the bug doc: converting those two makes
`use crate::util::UnpoisonExt;` **unused by production code** while the test module
still needs it. `cargo build` and `cargo clippy --all-targets` disagree about
whether a test-only import counts as used, so the import must move into
`mod tests` and **both** commands must be re-run.

**Part 2 (the test was vacuous).** My spec asserted
`background_swap_discards_on_new_turn` guards the stale branch's
flag-clear-before-return ordering, and asked the executor to confirm it by reading
the assertions. The executor reported confirmation. **Both of us were wrong** —
proven by mutation:

```
$ # with `entry.compaction_in_flight = false;` deleted from the stale branch
$ cargo test --lib background_swap_discards_on_new_turn
test ... ok
```

`make_test_entry()` builds the entry with `compaction_in_flight: false` and the
test calls `run_compaction` directly rather than through `try_snapshot`, so the
flag is never `true` and `assert!(!entry.compaction_in_flight)` cannot fail. Tree
restored after the check.

**The production ordering is correct** (`background.rs:237-238`, verified by
reading). Only the net was missing. The bug doc amends the "no new tests"
instruction to permit modifying that one test so the flag is `true` before the
call, and requires the executor to demonstrate the fail/pass pair.

**This is the second consecutive phase where I claimed test coverage that did not
exist** — 04d's `try_lock`-after-return proxy, now this. Both times the spec
planted the conclusion and asked the executor to confirm it, which is a leading
question, not a verification.

## Calibration — now three threads, and two are about my specs

1. **Count criteria (fourth occurrence).** Was "check criteria against the spec's
   own identifiers"; the real lesson is bigger — **a count criterion is only as
   good as its pattern**, and a criterion that can report success while the goal is
   unmet is worse than no criterion. Candidate fold: count criteria must use a
   scan proven against the actual code shape, and the architect must run it before
   pinning it. (I did run 04f's — and it was still blind, because I validated it
   against the count I already believed.)
2. **Specs that assert test coverage (second occurrence).** Candidate fold: a spec
   may not tell the executor which test is the discriminator; it must instead
   require the executor to *demonstrate* discrimination by mutation. The bug doc
   uses that shape — fail/pass pair quoted — and it is the first time this
   milestone has asked for proof rather than assertion.
3. **Lock/HOME test hygiene (three deep).** Still riding on phase 05.

**All three still await your sign-off; no `WORKFLOW.md` or `STANDARDS.md` change
has been made.**

## ⚠ I had the site count wrong — corrected while drafting

**`context/background.rs` has 2 production sites, not 13.** The earlier figure
came from a plain `grep -c` that counted the `#[cfg(test)]` module. `#[cfg(test)]`
starts at line 279; **11 of the 13 hits are test code.**

I re-derived every remaining group by splitting each file at its `#[cfg(test)]`
line. **Only `context/background.rs` was wrong** — 04g/04h/04i were already right,
because those files hold no `sessions.lock()` in their test modules. Corrected
totals are in the milestone README. The true total is **54 production sites**
(18 converted, 34 remaining, plus webhook's 2 in phase 05), not 65.

This is the same class of mistake I flagged for `session.rs` two phases ago and
then failed to check for the other files. **Consequence for 04j:** it inherits
**13 test-module sites** (11 here + 2 in `session.rs`) because the newtype makes
raw `.lock()` stop compiling — its scope is bigger than "the 13 `Arc::clone`
sites" implies.

## What 04f actually does

Two sites, both **non-mechanical** — each holds the guard across an early `return`
from the enclosing function. `size=s`, ~60 lines.

- **`try_snapshot` (line 67)** carries a `?`, a `return None`, *and* an explicit
  `drop(store)`. The whole body is one locked region whose result is the
  function's return value, so the closure returns the `Option` directly and
  `drop(store)` is deleted. The spec pins that
  `entry.compaction_in_flight = true` must stay **inside** the closure — setting
  it after would reopen a race where two turns both pass the check.
- **`run_compaction` step 2 (line 231)** has **two `return Ok(())` from the async
  fn** inside a block expression, with two different discard paths. The closure
  returns `Option<(usize, usize)>` and the caller uses `let … else { return Ok(()) }`.

**The single most dangerous line in the phase**, called out as such in the spec:
the stale branch's `return None` must come *after*
`entry.compaction_in_flight = false`. Reversing them leaves the flag set forever
and permanently blocks future compaction for that session — silent, permanent, and
covered by no test I could find. The evicted path must *not* clear the flag; the
`?` handles that by construction.

## Why this file is ordered before `stream.rs`

`stream.rs` calls `spawn_compaction`, and a `sessions` guard held across that call
is a **confirmed historical defect in this codebase** — the caller held the lock
while the callee re-locked. Converting the callee first changes the failure mode
for 04i: once `try_snapshot` goes through `with_sessions`, a `stream.rs` closure
enclosing `spawn_compaction` trips the re-entrancy **assertion** — a loud panic —
instead of hanging silently. Strictly better, and the reason for the ordering.

## Coverage note — unusually good here

`background.rs`'s own test module already covers all three converted paths (swap,
stale-discard, evicted-discard), so the stale test is a genuine discriminator for
the dangerous line above. That is why 04f specifies **no new tests** and names
those tests as the net instead — the fourth consecutive pure-conversion phase to
do so (04b, 04c, 04e, 04f), a pattern that is 3-for-3 on `approved_first_try` so
far.

## Still open for the PE

1. **The count-grep calibration fold** (third occurrence, raised at 04d). 04e and
   04f both self-check their criteria against the spec's own identifiers, and 04f
   additionally pins a **non-zero** expected count (11 test sites remain) plus a
   `sed`-based production-region command I verified works before pinning it. Two
   clean data points now. **No `WORKFLOW.md` change made.**
2. **Lock/HOME test-hygiene** (three deep). Still riding on phase 05.

I would add a third, and it is about my own work rather than the executor's: the
site-count error above is the second time a survey figure I recorded turned out to
be wrong (the first being the 65-vs-54 total). If you want a fold, the candidate is
that architect surveys must split production from test code before publishing a
count.

---

## M5 phase-04e — convert-executor-tail is `done`

(2026-07-26, `approved_first_try`, no bounces, 84 turns, commit `3f39732`.)

All 8 remaining `sessions.lock()` sites under `src/daemon/executor/` now go
through `with_sessions` — **8 calls for 8 sites**, and **zero raw locks anywhere
in the subtree**. Every per-file count came out exact (4 / 1 / 2 / 1, with
`executor/mod.rs` still at 6 from 04d). Gates re-run independently: fmt/build/
clippy exit 0, **915** lib-unit + 27 integration green, unchanged — no new tests,
as specified — and the run terminated.

**Both non-mechanical rewrites landed correctly**, which was the whole risk of
this phase:

- **`foreground.rs:232`** — the read is hoisted *above* the IIFE. The first
  branch is byte-identical, the second reads `default_target` with no lock held,
  both `cache.panes.read()` calls are outside any sessions closure, and both
  `return`s stayed inside the IIFE. The failure mode — moving the `return` into
  a `with_sessions` closure so the IIFE falls through and `target_hint` silently
  becomes `None`, **which compiles** — did not occur.
- **`knowledge/pane.rs:19`** — the closure returns
  `Result<(String,String,bool), String>` and the caller matches. All three
  user-facing strings verified byte-identical against the parent commit with
  literal `grep -cF`, not by trusting the summary.

**Two clean phases in a row with zero defects of any kind** (04e and, on the
executor's side, 04d). The spec shape that produced this: exact target code for
every site, an explicit hazard table, and no new tests in a pure-conversion
phase.

### Calibration threads — both now have supporting data

**Thread 1 — count-grep criteria.** 04e's criteria were self-checked against the
spec's own identifiers, with an explicit prohibition on writing the literal
`sessions.lock()` or `with_sessions(` in a comment. **It worked** — all six counts
came out exact. That is one clean data point for the candidate fold raised at 04d
review (third occurrence of the "same doc contradicts itself" pattern).
**Still unanswered by the PE; no `WORKFLOW.md` change has been made.**

**Thread 2 — no-new-tests in pure-conversion phases.** 04b, 04c, and 04e all
specified zero new tests and all three landed `approved_first_try`. 04d, the one
phase in this sequence that mandated a test, produced a non-discriminating one.
That is now **three clean data points for the pattern and one counter-example** —
worth weighing as guidance rather than a rule, since a structural change *does*
deserve a test; it just has to be a test that can fail.

### Residual risk recorded, not papered over

`target_hint` has no unit-test coverage, before this phase or after. The task-3
failure mode would degrade the approval prompt's pane hint to `None` without
failing any test. Coverage was not reduced, and inventing a test for an
approval-prompt string was deliberately out of scope — but if a later phase
touches `find_best_target_pane` or `target_hint`, that is the moment to add it.

### Remaining 04x work — 45 sites in four phases, then enforcement

04f (`context/background.rs`, 13) → 04g (`ghost.rs` + `briefing.rs`, 12) → 04h
(`background/*`, 9) → 04i (`stream.rs` + `hook.rs`, 11) → 04j (newtype +
enforce, converts the 13 `Arc::clone` sites). Then 05 (`webhook/process.rs`,
mechanism A, carrying the two 04d follow-ups), 06, 07, and the independent 08–11
instance-hardening set.

**The §3.5 migration hazard shrinks with each phase but is still live**, and it is
worst at the converted/unconverted boundary. `webhook/process.rs` (2 sites) is now
the most-reached unconverted store-toucher — `inject_ghost_event` is called from
`ghost.rs`, `knowledge/ghost.rs`, and the scheduler — so every remaining
conversion phase must keep naming it in its hazard table until 05 lands.

**Sizing:** 84 turns for 8 sites with two structural rewrites and no new tests —
against 128 for 04d's 10 sites plus a hoist and a test, and 95 for 04b's 15
uniform conversions. The ~10–12 turns/site estimate for structural phases held.

---

## Superseded: the original 04e dispatch note

Converted the last 8 `sessions.lock()` sites under `src/daemon/executor/` —
`foreground.rs` (4), `knowledge/mod.rs` (1), `knowledge/pane.rs` (2),
`knowledge/ghost.rs` (1) — finishing the executor subtree.

**Finish condition: 8 `with_sessions` calls for 8 former sites**, zero raw locks
anywhere under `src/daemon/executor/`, and **915** lib-unit tests unchanged. There
is no collapse in this phase, so the arithmetic is 1:1 — every site reads a
different thing at a different point.

**Two of the eight hold the guard across an early `return` from the enclosing
function**, which is the trap that made 04d's site 922 non-mechanical. Both get
exact target code:

- **`foreground.rs:232`** is the 922 shape one level deeper — the guard is held
  across `cache.panes.read()` inside an **IIFE** that `return`s. Moving the read
  into a `with_sessions` closure makes the `return` exit *that* closure, so the
  IIFE falls through to `None` and `target_hint` silently becomes `None`. **It
  compiles.** The spec hoists the read above the IIFE.
- **`knowledge/pane.rs:19`** is worse: the locked block contains **three**
  `return`s from `close_bg_window`, two carrying distinct user-facing error
  strings. The spec has the closure return `Result<(String,String,bool), String>`
  and match outside, with the messages pinned byte-identical.

**Cross-module hazard, verified while drafting** (task 9): `inject_ghost_event`
→ `inject_into_sessions`/`notify_chat_panes` in `webhook/process.rs` **is** an
unconverted store-toucher, and it is called immediately after task 8's closure in
`knowledge/ghost.rs`. So is `GhostManager::start_session_with_config` (`ghost.rs`,
11 sites) just before it, and `respawn_background_in_pane`/
`run_background_in_window` (`background/`, 9 sites) reached from `foreground.rs`.
All three already sit outside the converted regions; the spec forbids widening a
closure over them. By contrast `append_session_message` (`session.rs:281`) is file
I/O only — mechanism-A relevant, but it will not deadlock.

### Both 04d spec defects were addressed in this doc, not just noted

- **No new tests.** 04e is a pure conversion with no structural change, so the
  existing 915 tests are the net — matching 04b and 04c, both of which landed
  `approved_first_try`. This structurally avoids repeating 04d's
  non-discriminating-test defect: there is no test to get wrong.
- **Count criteria self-checked against the spec's own identifiers**, which is
  what 04d got wrong. The `use` lines are safe (`with_sessions` in an import has
  no trailing `(`), and the doc now explicitly forbids writing the literal
  `sessions.lock()` or `with_sessions(` **in a comment**, since the greps count
  raw text including comments. That was the exact failure mode of 04d's criterion 3.
- **Authorizations says None and explains the boundary**: no tests means no
  `HOME` redirection means no `unsafe`, and the doc tells the executor to file a
  blocker rather than improvise if it thinks it needs one.

**⚠ Still open for the PE:** the calibration question raised at 04d review — third
occurrence of the "same doc contradicts itself" pattern — is **unanswered**. No
`WORKFLOW.md` fold has been made (§5 requires your sign-off). The candidate fold:
before dispatch, the architect mechanically checks every count-grep criterion
against the spec's own mandated identifiers. 04e applies that check by hand; the
question is whether it becomes a standing rule.

Also unanswered and now three-deep: the lock/HOME **test-hygiene** thread (04a→04b
fast-fail carry, 04c `try_lock` follow-up, 04d's missing RAII guard). Both
follow-ups still ride on **phase 05**, which owns mechanism A and needs the
`load_agent` seam anyway.

---

## M5 phase-04d — convert-executor-dispatch is `done`

(2026-07-26, `approved_first_try`, no bounces, 128 turns, commit `1ea8c7e`.)

All 10 `sessions.lock()` sites in `src/daemon/executor/mod.rs` now go through
`with_sessions` — **6 calls for 10 former sites**, the pinned finish condition.
Gates re-run independently: fmt/build/clippy exit 0, **915** lib-unit (914 + 1)
+ 27 integration green, run terminated.

**The mechanism-A defect is fixed.** `load_agent` sits at `mod.rs:93`, outside
every closure, so `build_memory_namespaces` no longer holds the global session
map across a config-file read. `ask.rs:566` got the fix without an edit, which
was the point of fixing it inside the shared function.

**Site 922 was converted correctly** — the one that could not be done
mechanically. `default_target` is now read and the guard released *before*
`cache.panes.read()` (killing the nested-lock hazard), and the `return Ok(dtp)`
stayed in the function body instead of moving inside a closure. Task 2's
`effective_parent_job_id: gc.map(|_| sid.to_string())` subtlety also survived, so
non-ghost sessions still report no parent job id.

### ⚠ Two architect spec defects found at review — read before drafting 04e

Both are mine. The executor conformed to the spec exactly and is not at fault.

**1. An acceptance criterion was unsatisfiable.** Criterion 3 demanded
`grep -c load_agent == 1` while task 7 mandated a test whose *name* contains
`load_agent`, forcing the count to 2. Annotated inline in the phase doc.

**This is the third occurrence of the phase-01 pattern** — "pinning an
implementation that could not satisfy the behavior stated elsewhere in the same
doc." Phase-01 logged two and said a third warrants raising with the PE. **Raised
2026-07-26; awaiting the PE's call on whether this becomes a `WORKFLOW.md` fold.**
The candidate fold: before dispatch, the architect mechanically checks every
acceptance criterion that greps a count against the spec's own mandated
identifiers.

**2. Task 7 specified a test that cannot fail — verified by mutation, not by
reading.** The review reverted `build_memory_namespaces` to the chained body and
ran the new test:

```
test build_memory_namespaces_does_not_hold_the_lock_across_load_agent ... ok
```

It passes against the un-hoisted code. The guard is function-local in both
implementations, so `try_lock` *after the call returns* can never distinguish
them. The spec said "assert the observable proxy" without noticing the proxy is
vacuous. Tree restored afterwards; confirmed clean.

**The production fix is real** (greppable at `mod.rs:93`); only the regression net
is missing.

### Follow-ups for phase 05 (`unlock-blocking-paths`), not dispatched as a bounce

05 already owns mechanism A and will need a seam for `webhook/process.rs`, so
both items belong there:

- **Put `load_agent` behind a trait seam** and rewrite
  `build_memory_namespaces_does_not_hold_the_lock_across_load_agent` so a stub can
  attempt `try_lock` *during* the call. It must fail against the chained body.
- **That test restores `HOME` without an RAII guard**, so a failing assertion
  leaks a temp `HOME` *and* poisons `TEST_HOME_LOCK` for the rest of the run. M4
  phase-06 fixed this exact class ("HOME-leak → RAII guard"); apply the guard when
  the test is rewritten. **This is the third HOME/lock-test-hygiene item in M5** —
  after the 04a→04b fast-fail carry and the 04c `try_lock` follow-up — which is
  itself worth weighing as a `STANDARDS.md` fold on lock/HOME test hygiene.

**Accepted rather than bounced:** the new test adds three `unsafe` blocks
(`env::set_var`/`remove_var`, unsafe in edition 2024). 04d's Authorizations said
"None", but this is the established codebase idiom — `with_test_home` at
`src/daemon/utils/event_log.rs:288-299` does the same and there is no safe
alternative. **Every future phase whose tests redirect `HOME` needs this
pre-authorized in its Authorizations section.** Drafting omission, not a
violation.

**Nit, not filed:** `// ── Delegation depth tracking` at `mod.rs:186` now heads a
comment-only stub. Delete both lines when the region is next touched.

### Remaining 04x work

04e (`foreground.rs` + `knowledge/*`, 8 sites) → 04f (`context/background.rs`,
13) → 04g (`ghost.rs` + `briefing.rs`, 12) → 04h (`background/*`, 9) → 04i
(`stream.rs` + `hook.rs`, 11) → 04j (newtype + enforce). Then 05, 06, 07, and the
independent 08–11 instance-hardening set.

**Sizing data point:** 128 turns for 10 conversions + 1 hoist + 1 test, against
95 for 04b's 15 conversions and ~70–90 predicted here. The prediction was low;
the collapse and the hoist cost more per site than a uniform conversion does.
Budget ~10–12 turns/site for phases with a structural change, ~6 for uniform ones.

---

## Superseded: the original 04d dispatch note

Converted the 10 `sessions.lock()` sites in `src/daemon/executor/mod.rs` and
**fixes the mechanism-A defect the 04c hazard note pointed at**:
`build_memory_namespaces` (`executor/mod.rs:88`) holds the global session lock
across `crate::agents::load_agent()`, a config-file read. Task 1 hoists the read
out while keeping the signature, so `ask.rs:566` — the other caller — gets the
fix without being touched.

Finish condition: **6 `with_sessions` calls for 10 former sites**, zero
`sessions.lock()` left in the file, and 915 lib-unit tests (914 + one).

The five sites at 130/150/169/205/207 are five separate acquisitions reading
**the same entry** within ~80 lines at the top of `execute_tool_call`. They
collapse into one `DispatchSnapshot` read — the same collapse 04b did at 166/173
and 04c did at 579/590/600, but larger. The spec gives the exact target code,
including the one subtlety that will bite a mechanical rewrite:
`effective_parent_job_id` must stay `Some(sid)` **only when `ghost_config` is
present**, because the original `.map(|gc| (gc.spawn_depth, Some(sid…)))` coupled
both to `gc` being `Some`. Getting it wrong makes a non-ghost session report a
parent job id.

**⚠ One site cannot be converted mechanically.** `find_best_target_pane`
(`executor/mod.rs:922`) holds the sessions guard across
`cache.panes.read()` — a second lock inside the first — and contains
`return Ok(dtp.clone())`. Wrapping that in a `with_sessions` closure makes the
`return` exit the *closure*, not the function: either a compile error or silently
changed control flow. The spec pins extract-then-act with the exact target code.
Both defects die in the same rewrite.

**Hazard discipline carried forward.** Per `daemon-stalls.md` § 3.5, task 6
tabulates every store-touching callee in this file's region that 04d does **not**
convert — `knowledge/mod.rs:38`, `knowledge/pane.rs:19,52`,
`foreground.rs:170,199,232,885` — so no closure encloses a raw `.lock()`. The
re-entrancy assertion cannot catch that shape; it hangs instead of panicking, so
04d's acceptance criteria include "`cargo test` completes without hanging."

**M5 phase-04c — convert-ask is `done`** (2026-07-26, `approved_first_try`, no
bounces, commit `b054759`). All 13 `sessions.lock()` sites in `ask.rs` now go
through `with_sessions` — 0 raw locks, 11 calls, with 579/590/600 collapsed into
one acquisition. Gates re-run independently at review: fmt/build/clippy exit 0,
914 lib-unit + 27 integration green, and **the run terminated** (the failure mode
this milestone's conversion phases carry is a hang, not a red gate).

Spec conformance was checked by reading the diff rather than the executor's
summary. Both of spec 2's must-preserve details hold: `tool_policy`/`agent` are
`None` for non-ghost sessions, and `parent_job_id` is read regardless of
`is_ghost` with no ghost check added. The `is_ghost_session` shadowing survives —
`build_memory_namespaces` gets the pre-collapse value, `cost_attribution` the
shadowed one. `build_memory_namespaces` sits outside every closure, so the § 3.5
hazard is avoided. `this_turn_count`'s `.and_then` → `.map` change is correct and
required, since `with_sessions` returns `usize` rather than `Option<usize>`.

The poison-semantics change (`.ok()?` bail → `unwrap_or_log()` recover) was
anticipated by the spec at doc lines 98–101 and matches the `CLAUDE.md`
invariant — not a deviation.

**Two open items recorded in the 04c verdict, neither blocking:**

1. **The architect-side real-binary exercise was not performed.** 04c's E2E
   section assigns it to the architect. The running daemon is built from
   `~/.cargo/bin/daemoneye` (17:59) which predates commit `b054759` (18:56), so
   exercising the converted path means displacing a daemon that was just
   restarted. Worth doing before 04d lands, since 04d touches the same dispatch
   path.
2. **Calibration (one occurrence — data, not a fold):** the server-authored
   completion entry does not emit the "End-to-end verification" heading that
   phase docs require, and the executor no longer owns the Update Log tail
   (M27 phase-03). Every future phase hits this. If it recurs, the fix belongs in
   the server's completion template — **not** in the phase docs, and not by
   restating the instruction as a workaround.

## The 04d tail is now five phases, not three

A site-by-site survey replaced the earlier "04d×3, ~60 sites" estimate. Actual
count is **65 production sites** in six groups; the newtype phase moved from
**04e → 04j** (undrafted and unstarted, so the renumbering is free):

| Phase | Files | Sites |
|---|---|---|
| 04d (drafted) | `executor/mod.rs` | 10 |
| 04e | `executor/foreground.rs` + `executor/knowledge/*` | 8 |
| 04f | `context/background.rs` | 13 |
| 04g | `ghost.rs` + `briefing.rs` | 12 |
| 04h | `background/{run,respawn,helpers,gc}.rs` | 9 |
| 04i | `stream.rs` + `hook.rs` | 11 |
| — | `webhook/process.rs` (2) belongs to **phase 05** | — |

**`src/daemon/session.rs` contributes zero production sites**, despite four
`sessions.lock()` greps: `:432` is `with_sessions`'s own acquisition (correct and
permanent), `:443` is a doc comment, and `:1204`/`:1226` are tests. Nobody needs
to re-derive this.

**Sizing:** at 04b's measured ~6 turns/site plus fixed overhead, 04d's 10 sites
plus the hoist and one test should land near 70–90 turns, against 95 for 04b's
15 sites and 46/50/70 for the three `size=s` phases before it. Every remaining
group is 8–13 sites, comfortably inside one session.

---

## M5 phase-04c — convert-ask is `done` (approved_first_try, 2026-07-26)

Doc: `docs/dev/milestones/M5-ux-stability/phase-04c-convert-ask.md`.

---

## Drafted 2026-07-26 and dispatchable now: M5 phases 08–11 (instance hardening)

**Phase 08 — instance-lock is the one to dispatch first, and it does not depend
on the 04x sequence.** Dispatch with `/rexymcp:dispatch phase-08-instance-lock`.

These four phases came out of a live incident, not a survey. On 2026-07-25 two
daemons ran concurrently against one `~/.daemoneye/` tree for ~64 s, served two
different chat sessions, and the second unlinked the first's socket to bind its
own. Design doc: `docs/design/daemon-instance.md` (§ 1 has the timeline).

| Phase | Doc | Delivers |
|---|---|---|
| 08 instance-lock | `phase-08-instance-lock.md` | `flock` on `var/run/daemoneye.pid` acquired before every side effect; socket unlink licensed by ownership; identity-checked teardown |
| 09 fatal-bind-honest-liveness | `phase-09-fatal-bind-honest-liveness.md` | webhook bind fatal at startup; `daemon_is_running` → `DaemonLiveness` 4-case enum; `ping`/`status` say "wedged" vs "dead" |
| 10 lifecycle-observability | `phase-10-lifecycle-observability.md` | `pid` on every event record; logger-init failure surfaced; startup identity line |
| 11 fork-readiness-handshake | `phase-11-fork-readiness-handshake.md` | parent reports the child's real startup outcome instead of unconditional success |

**Dependencies:** 08 → none. 09 → 08 (reads the PID file it creates). 10 → 08
(reports the lock outcome). 11 → 08 **and** 09 (relays the errors both add).
So 08 can go at any time, and 09/10 are independent of each other.

**Why this landed in M5 rather than a new milestone (PE decision, 2026-07-26):**
it composes with the milestone's existing subject. A `SessionStore` deadlock —
the confirmed defect phase 02 fixed — puts the daemon in exactly the state the
old liveness probe misread: threads `futex`-parked, socket still listening,
nothing answered. A stall invites a duplicate; the duplicate then shares the
session store, `schedules.json`, and the memory index. Phase 08 is the
blast-radius limiter for a failure mode M5 has already confirmed in production.

**Three things found while drafting that the specs now pin explicitly:**

1. **The existing guard fires too late to matter.** It sits at
   `src/daemon/mod.rs:739`, but a duplicate reaching it has already deleted the
   live daemon's `de-pipe-*.log` files, repointed all four global tmux hooks at
   its own `current_exe()`, run a memory migration, emitted `daemon_start`, and
   spawned three pollers — and `anyhow::bail!` restores none of it. **A duplicate
   launch was destructive whether the guard worked or not.** Phase 08's central
   task is therefore task 4 (ordering), not the lock itself. § 2.3 of the design
   doc tabulates all seven side effects with line numbers.
2. **A second duplicate signal was already there and swallowed.** The webhook
   `TcpListener::bind` returns `EADDRINUSE` for a duplicate, but it sits inside
   `supervise(...)`, which retries forever with backoff. Phase 09 task 6 splits
   `webhook::start` into `bind` (eager, fatal) + `serve` (supervised), and the
   spec spells out the `Option`-in-a-`Mutex` dance needed because `axum::serve`
   consumes the listener — a supervisor that silently re-binds would recreate the
   retry loop the task exists to delete.
3. **`flock` rather than a bare PID file, and the reason is the same bug one
   layer down.** The kernel releases a `flock` on process death including
   `SIGKILL`, so there is no stale-lock recovery path. A PID file alone needs "is
   that PID alive, is it really ours, was it recycled" guesswork — the same
   inference-instead-of-invariant mistake that caused the incident. The PID is
   written into the file as *diagnostic payload only*; § 2.1 forbids branching on
   it, and phase 08's spec repeats the prohibition.

**Two authorizations were granted at draft time**, both narrow and both stated in
the phase docs' Authorizations sections rather than left to the executor:

- Phase 08 may edit `Cargo.toml` **solely** to add `features = ["fs"]` to the
  existing `nix = "0.31.1"` dependency. That enables `nix::fcntl::Flock`, a safe
  RAII wrapper, which is what keeps phase 08 free of `unsafe`. The exact 0.31.1
  API is quoted into the phase doc from the vendored source
  (`nix-0.31.1/src/fcntl.rs:1038-1100`) because the executor cannot fetch docs —
  including the non-obvious bits: `lock()` returns the `File` *back* on failure
  (so the incumbent's PID is still readable for the error message), and matching
  both `EAGAIN` and `EWOULDBLOCK` will not compile on Linux since they are the
  same value.
- Phase 11 may write `unsafe`, **only** the two blocks inside
  `ready::create_pipe` wrapping `libc::pipe` and `OwnedFd::from_raw_fd`. Its
  acceptance criteria pin `grep -c "unsafe" src/daemon/ready.rs` at exactly `2`.

**Sizing note for review:** 08 ~260 lines, 09 ~210, 10 ~130, 11 ~220 — all
mechanical against a complete design, the shape this executor handles cleanly
(per the M4 retrospective). None involves a compaction-path rewire or a large
additive block, the two documented self-sabotage triggers.

**Also verified while drafting, so nobody re-derives it:** after phase 08 deletes
the only call site, `daemon_is_running()` has zero callers but does **not** trip
`dead_code` under `-D warnings`, because `src/lib.rs:10` has `pub mod daemon;`
and the function is `pub`. Phase 08 says so explicitly so the executor does not
add an `#[allow(dead_code)]` or invent a caller.

**Deliberately out of scope of all four phases,** recorded in
`daemon-instance.md` § 3 and § 5: file locking for `schedules.json`, the memory
FTS5 index, and the session JSONL stores (single-instance enforcement is the
fix); auto-restarting a wedged daemon (detection yes, action no); cross-host
exclusion over NFS; and any conclusion about where the 2026-07-25 SIGTERM came
from — it was almost certainly a human cleaning up the duplicate, and the bug is
that the duplicate was possible at all.

---

## M5 lock-conversion sequence (04x) — unchanged

Converts the 13 `sessions.lock()` sites in `src/daemon/server/ask.rs`. Unlike
`handlers.rs` these are not uniform — seven are `sessions.lock().ok()?` chains
inside `.and_then(…)` closures, so the spec spells out each conversion instead
of leaving it to pattern matching. The three consecutive ghost-config reads at
579/590/600 collapse into one acquisition (exact target code in the spec), so
the finish condition is **11** `with_sessions` calls for 13 former sites, and
`cargo test --lib` still at **914** — this phase adds no tests.

**⚠ Hazard found while drafting, now recorded as `docs/design/daemon-stalls.md`
§ 3.5.** The re-entrancy assertion only catches `with_sessions` nested inside
`with_sessions`. A converted closure that encloses a call still using **raw**
`.lock()` deadlocks silently — no panic, no log, just a hung test run.
`ask.rs:571` calls `build_memory_namespaces`, which locks at
`executor/mod.rs:88` and is not converted until 04d, so no 04c closure may span
lines 571–575. The spec forbids it explicitly.

That generalises: during the 04b–04d window the guard is weakest exactly at the
converted/unconverted boundary, and **a conversion phase's failure mode is a
hang, not a red gate**. Every remaining conversion phase must name the
store-touching calls inside its region. The hazard disappears once 04e's newtype
makes raw `.lock()` stop compiling — an argument for finishing the sequence
rather than stopping after the useful-looking middle.

Also noted for 04d: `build_memory_namespaces` calls `crate::agents::load_agent`
(a config-file read) **while holding the lock** — mechanism A living in
`executor/mod.rs`. 04d should hoist that I/O out, not merely convert the call.

**M5 phase-04b — convert-handlers is `done`** (2026-07-25,
`approved_first_try`, 95 turns). All 15 `sessions.lock()` sites in
`server/handlers.rs` now go through `with_sessions`; the two adjacent
acquisitions at 166/173 collapsed into one, so the file has 14 calls for 15
former sites. `ask.rs` untouched at 13, `SessionStore` still a plain alias.

The 04a follow-up is closed: `with_sessions_sets_depth_inside_closure` fails in
**0.00s** under a `let _depth` → `let _` mutation (verified by the reviewer),
where the older re-entrancy test hangs under the same change.

**Sizing data for 04d — read before drafting it.** 95 turns for 15 conversions
plus one test, against 46/50/70 for the three preceding `size=s` phases. That is
~6 turns per site plus fixed overhead. The 04d tail (`background.rs`,
`ghost.rs`, `executor/mod.rs`, `stream.rs`) is **~60 sites** — roughly 360 turns
at this rate, which fits under the 600-turn cap but leaves no margin for a stall
and is well past what one review can cover carefully. **Split 04d into at least
three phases of ~15–20 sites, one file group each.**

**Remaining M5 phases:** 04c (`ask.rs`, 13 sites with `lock().ok()?` chains
needing per-site care), 04d×3 (the tail), 04e (newtype + enforce, converts the
13 `Arc::clone` sites), 05 (`webhook/process.rs` — mechanism A), 06 (tmux-call
hardening — mechanism B), 07 (stall-instrumentation, rescoped).

**M5 phase-04a — with-sessions-accessor is `done`** (2026-07-25,
`approved_first_try`, 50 turns). `with_sessions(&store, |map| …)` now exists in
`src/daemon/session.rs` with an always-on re-entrancy assertion behind an RAII
depth guard. Two sites converted (`cleanup_pass`; the shutdown pipe-pane sweep,
which also hoists a blocking `stop_pipe_pane` subprocess out of the critical
section). `SessionStore` is still the `Arc<Mutex<…>>` alias, so the other 98
sites and all 13 `Arc::clone` sites compile untouched.

Both guard mutations were checked by the reviewer: `let _depth` → `let _`
disables the guard and the re-entrancy test then deadlocks (proving the binding
is load-bearing), and emptying the `Drop` impl makes the panic-reset test fail
fast. Shutdown path verified against the real binary — `Received SIGTERM` →
`Daemon stopped cleanly.`, socket removed.

**Carry into phase 04b** (recorded in the 04a verdict): add a fast-failing
companion test asserting the thread-local depth reads 1 inside a `with_sessions`
closure. The current re-entrancy test catches its regression by *hanging*, which
stalls CI instead of failing it. **This is the second such test in this
milestone** — a third would justify a `STANDARDS.md` line requiring lock-invariant
regression tests to fail fast rather than block.

**Remaining M5 phases:** 04b (convert `handlers.rs` + `ask.rs`), 04c
(`background.rs` + `ghost.rs` + tail), 04d (newtype + enforce, converts the 13
`Arc::clone` sites), 05 (`webhook/process.rs` — mechanism A), 06 (tmux-call
hardening — mechanism B), 07 (stall-instrumentation, rescoped). Plan and
rationale in `docs/design/daemon-stalls.md` § 3.4.

**M5 phase-03 — echo-user-input is `done`** (2026-07-25, `approved_first_try`,
46 turns — the shortest run of the milestone). The user's prose queries now
commit into scrollback as a `you`-titled panel, the same element tool output
uses, so a finished conversation reads as a transcript. Verified end-to-end
against the real binary: the startup greeting produces no panel, a typed query
does, and a slash command does not.

Also carried the phase-02 follow-up: `cleanup_pass_evicts_idle_and_keeps_active`
now ends with `try_lock().expect(...)`, so a future re-entrancy regression fails
fast instead of hanging CI beside the sibling that reports it correctly.

**Three M5 phases remain undrafted:** 04 unlock-blocking-paths, 05
tmux-call-hardening, 06 stall-instrumentation (rescoped). Phases 04 and 05 come
straight from the design doc's mechanisms A and B — both confirmed by code
reading, neither implicated in the hang that phase-02 fixed.

**Open question for the PE, raised at phase-02 and still unanswered:** this
codebase has now produced **two** re-entrant `sessions`-lock defects (the
phase-02 one, and one fixed during the M4 phase-08 takeover). Neither was
catchable by `clippy::await_holding_lock`. Before drafting phase 04, decide
whether it should carry a structural answer — a `with_sessions(|store| …)`
accessor that makes the guard's lifetime explicit and nesting hard to write, or
a debug-build re-entrancy assertion — or stay a set of point fixes. It touches
~180 lock sites, so it is your call, not mine.

**M5 phase-02 — cleanup-deadlock is `done`** (2026-07-25, `approved_first_try`,
70 turns, no bounce). **The daemon hang is fixed.** The re-entrant
`SessionStore` acquisition in the `session-cleanup` supervisor is gone:
`cleanup_pass()` in `src/daemon/session.rs` locks exactly once, returns evicted
entries by value plus an active-id snapshot, and the supervisor runs the tmux
teardown and both filesystem sweeps outside the lock.

Verified by an accelerated before/after soak (cleanup interval temporarily 60 s
→ 1 s so the sweep branch fires at ~60 s instead of ~60 min, both trees soaked
identically):

- **pre-fix, 1 m 32 s:** 0 threads in `epoll_wait`, 33/33 `futex_wait`, accept
  backlog 2 and climbing — the production wedge reproduced.
- **fixed, 3 m 01 s (3 sweeps):** 1 thread in `epoll_wait`, backlog 0 — healthy.

The mutation check was also re-run by the reviewer rather than trusted:
stranding the guard makes `cleanup_pass_releases_the_lock` fail immediately.

**A daemon built from `master` is now safe to leave running.** Any binary built
before commit `435382e` still wedges about an hour in.

**One-line follow-up, deliberately not dispatched:**
`cleanup_pass_evicts_idle_and_keeps_active` ends with
`sessions.lock().unwrap()`, which would *hang* rather than fail if re-entrancy
regressed. Switch it to `try_lock`; fold into whichever phase next touches
`session.rs`.

**M5 phase-01 — spinner-gutter is `done`** (2026-07-25, `approved_after_2`,
commit `2753c93`). Spinner moved out of the input box onto a reserved one-row
line above the top border, carrying frame + verb + dots together; the row stays
reserved when idle so the box never moves. Two bounces (bug-01-1 E2E not
performed, bug-01-2 prompt lost at height 4), both closed; E2E performed by the
architect with real `tmux capture-pane` snapshots.

**Architect calibration from phase-01:** two spec contradictions in one phase —
pinning an implementation that could not satisfy the behavior stated elsewhere
in the same doc. Third occurrence warrants raising it with the PE. Also:
verification needing a live daemon or a human eye belongs to the architect, not
the executor. Phase-02 applied both lessons and landed `approved_first_try`.

**M5 — UX & Stability is scoped** (2026-07-24, PE sign-off). Milestone README:
`docs/dev/milestones/M5-ux-stability/README.md`. Design + hang evidence log:
`docs/design/daemon-stalls.md`.

Phase order (**revised 2026-07-25** once the hang was root-caused):

01 spinner-gutter (**done**) → 02 cleanup-deadlock (**drafted**) →
03 echo-user-input → 04 unlock-blocking-paths → 05 tmux-call-hardening →
06 stall-instrumentation (rescoped, draft only if 04–05 leave a gap)

**Hang status: ROOT-CAUSED and drafted as phase 02.** A re-entrant acquisition
of the global `SessionStore` mutex in the `session-cleanup` supervisor
(`src/daemon/mod.rs:693` and `:709`) strands the lock ≈60 minutes after every
daemon start. Confirmed by a live capture (33/33 threads futex-parked, reactor
gone, zero CPU over 12 h, 9 connections queued unaccepted) plus PE-captured gdb
stacks showing one task in `lock_contended` with **no thread holding the mutex**
— the holder was the same task, one frame up. `docs/design/daemon-stalls.md`
§ 1.5b–1.5c.

The two mechanisms found earlier by code reading are still real and still worth
fixing — `webhook/process.rs:148,161` (disk writes and a timeout-free tmux
subprocess under the global lock) and the 49 blocking `std::process::Command`
tmux calls on tokio workers — they are simply not what fired. They are phases
04 and 05.

**⚠ This is the second re-entrant `sessions` lock found in this codebase.** The
first was fixed during the M4 phase-08 takeover ("held the `sessions` lock
across `spawn_compaction`, which re-locks" — see the M4 entry below). Two
independent occurrences of the same defect class, neither catchable by
`clippy::await_holding_lock`, means the codebase needs a structural answer, not
just two point fixes. Candidates worth weighing when phase 04 is drafted: a
`with_sessions(|store| …)` accessor that makes the guard's lifetime explicit and
un-nestable, or a debug-build re-entrancy assertion. Flagging for PE decision —
not folding into a phase unilaterally.

- **Calibration:** the M4 candidate fold (large additive blocks → executor
  self-sabotage, from phase-10b) remains **held for recurrence** per PE. If an
  M5 phase reproduces it, that is occurrence three and the fold lands in
  `WORKFLOW.md`.

---

**M4 — Context Management Overhaul is complete** (2026-07-16, all ten phases
`done`, retrospective in
`docs/dev/milestones/M4-context-management/README.md` § Retrospective). Gates
green at close: 901 lib-unit + 27 integration passing, clippy clean.

**M4 phase-10b — memory-extraction is `done`** (2026-07-16, escalated → architect
takeover; the LAST M4 phase). Opt-in (off-by-default) memory extraction from the
interactive **async** epoch build (`extract_memories_from_epoch` in
`context/epochs.rs`, wired into `run_compaction` after `append_epoch`; category
`knowledge`, `source: "compaction"` stamped as a raw frontmatter line — no schema
change). Executor `hard_fail`ed on `LowNoveltyStall` (rexyMCP#3 governor, in the
wild) after corrupting adjacent existing code while adding the +309-line
`epochs.rs` block — the documented large-addition self-sabotage pathology. Its
production code (extraction fn, `apply_extraction`, config flag, call site) was
correct as written; takeover restored a deleted test-fn signature, removed a
stray `#[test]`/`}` pair + a duplicate `append_epoch` line, and fixed 3 test bugs
(`env::var::var` typo, `Config::load_default`, private-fn round-trip). 913 unit +
27 integration green. Every M4 epoch/compaction-path phase (03, 05a, 05b, 06, 07,
08, 10b) except 04 and 10a needed takeover.

**M4 phase-10a — ghost-coverage is `done`** (2026-07-16, approved_first_try,
commit `06389f6`). Synchronous, model-call-free ghost working-set guard
(`enforce_ghost_working_set` in new `context/ghost_ws.rs`, wired into the
`ghost.rs` turn loop). **First M4 compaction/epoch-path phase to reach `done`
WITHOUT architect takeover** — executor completed clean in 109 turns, no
git-thrash, no verify-loop. Structured-only epochs (`narrative == None`), skips
`maybe_rollup` (the one deliberate divergence from the interactive ladder, since
rollup can make a model call). 909 unit + 27 integration tests green.

**M4 phase-09 — session-meta-persistence is `done`** (2026-07-16,
approved_after_1, commit `f7e4df2`). `<id>.meta.json` continuity + boundary-safe
reload. First M4 compaction/session-path phase to reach done WITHOUT takeover
(resume+spec-fix, then one review bounce on vacuous boundary tests, bug-09-1) —
after the rexyMCP#2 governor fix. Filed rexyMCP#3 (novelty-aware stall
detection) as the follow-up.

**M4 phase-08 — async-compaction is `done`** (2026-07-15, escalated → architect
takeover after **two** `hard_fail`s, both `NoProgressStall` on the `ask.rs`
step-2 rewire — the documented Qwen git-thrash/orient-paralysis pathology).
Run 1 self-reverted `ask.rs` and thrashed; run 2 (dispatched on the partial
tree, per PE choice) burned 40 turns reading with zero edits. The executor's
scaffold was near-complete (`background.rs` +408, the `SessionEntry` fields,
the ctx thread, the narrative-default flip all correct — tree was one struct
field from building). Architect finished the last mile: reconstructed the
`ask.rs` threshold ladder (fixing a defeated safety-cap net, a dropped 50 %
elide branch, and a persistence-flag regression), fixed a **stream.rs
self-deadlock** (held the `sessions` lock across `spawn_compaction`, which
re-locks), corrected the `background.rs` idempotency guard (compared against
the whole snapshot's last turn → never fired), converted lock sites to
`.unwrap_or_log()`, gated the narrative call on `narrative_enabled`, and wrote
the 4 missing tests (executor shipped 3/7). Also fixed a pre-existing recall
test HOME-isolation gap the new tests exposed. Gates green (900 unit + 27
integration, 3× deterministic). Every epoch/compaction-path phase (03, 05a,
05b, 06, 07, 08) has now needed architect takeover — the compaction-path
rewire shape reliably defeats this executor (04, a pure archive add, was the
lone `approved_first_try` in this stretch).

**M4 phase-07 — recall-context is `done`** (2026-07-14, escalated → architect
takeover after 2 no-progress stalls). New `recall_context` tool over the phase-04
archive (query/range, char-safe excerpts, masked+truncated). The new rexyMCP
`NoProgressStall` governor **validated in the wild** — caught both stalls (20,
then 40 turns) instead of 167-529-turn runaways; threshold raised 20→40 for this
project. Executor wrote a near-complete impl on the 2nd run; takeover finished §3
wording + sre.toml, fixed the should_emit arm, a build_excerpt byte/char bug, and
the recall tests' HOME isolation. Gates green (893 unit + 27 integration).

**M4 phase-06 — ledger-rollups is `done`** (2026-07-14, escalated → architect
takeover). Executor stopped by the human (`rexymcp stop`) at 167 turns
verify-looping — its implementation was complete and compiled
(`maybe_rollup`/`uncovered_epochs`/`EpochTally::merge`/`summarize_once` extract/
ledger render/`rollup_after` config); takeover fixed test-only defects (HOME-leak
→ RAII guard, wrong turn_end assertion, poison-resilient `TEST_HOME_LOCK`, clippy
nits) and restored a README the executor's edit tool corrupted. Gates green
(883 unit + 27 integration). **3rd consecutive human-stopped verify-loop on an
epoch phase** — reinforces filed FR-2.

**M4 phase-05b — epoch-head is `done`** (2026-07-14, escalated → architect
takeover). Executor was **stopped by the human (`rexymcp stop`) after 529 turns
of verify-looping**; its `epochs.rs` (`compact_with_epochs` regenerated head +
`render_context_block`) and `ask.rs` (should_digest epoch-build rewire) were
correct, but `digest.rs` was left garbled mid-edit. Architect reconstructed
digest.rs from HEAD + reapplied the intended deletions (retired
`build_session_digest`/`compact_with_digest`/`tally_events`/`scan_artifacts`;
kept the narrative summarizer + budget planner; keep-newest narrative), fixed
executor clippy/test bugs. Gates green (874 unit + 27 integration).

**M4 phase-05a — epoch-persistence is `done`** (2026-07-14, escalated → minimal
architect takeover). Executor authored `src/daemon/context/epochs.rs` (+514,
all functions + tests, correct) but verify-looped (`IdenticalToolCallRepetition`,
6 bash calls) on a 1-line test-fixture bug (event ts 15:00 vs window
`[00:00,01:00)`). Notably it did **not** git-revert this time — the split's
additive 05a left the code intact for a trivial takeover (fixed the window; gates
green, 884 unit + 27 integration). digest.rs untouched — additive contract held.

**Phase-05 was re-split (2026-07-14, PE decision) into 05a + 05b.** At ~500
lines it sat at the one-session limit and it deletes/replaces the phase-03
compaction path — the exact digest-heavy shape the executor git-thrashed on
twice. **05a** (`phase-05a-epoch-persistence.md`) is purely additive:
`context/epochs.rs` types + append-only persistence + span-windowed
`tally_span`/`scan_artifacts_span`, deleting/rewiring nothing (build stays green
throughout). **05b** (`phase-05b-epoch-head.md`) does the risky rewire:
`compact_with_epochs` regenerated head, `render_context_block`, keep-newest
narrative, and retirement of `compact_with_digest`/`build_session_digest`. 05b's
Current-state quotes the phase-03 takeover `should_digest` block verbatim so the
executor rewires in place. The old `phase-05-epoch-records.md` is now a redirect
stub.

**M4 phase-04 — append-only-archive is `done`** (2026-07-14, approved_first_try,
commit `0c02961`). Archive folded into `append_session_message` (all 7 callers
automatic, archive-first ordering to avoid seed-duplicate); honest elision
placeholders; `sweep_session_archives` retention.

**M4 phase-03 — budget-compaction is `done`** (2026-07-14, escalated → architect
takeover). Executor hard_failed after 352 turns: it wrote the `digest.rs` core
then **reverted it via `git checkout`/`git stash`** (despite a runtime guard),
leaving a non-compiling tree with only the plumbing. Architect implemented
`digest.rs` (§2 budget planner + `raw_budget_cut`, §4 `synthesized_tail_start` +
`repair_tail_head`, §5 graduated UTF-8-safe elision, 3-arg pure-cutter
`compact_with_digest`), fixed 3 executor plumbing deviations (`validate_compaction`
fallback, hardcoded `token_scale=1.0` → real per-session scale, dead
`_history_pct`), and verified E2E (real binary emits the `[compaction]` fallback
warning). Gates green (875 unit + 27 integration). 2nd occurrence of the Qwen
git-thrash pathology (phase-01 was 1st) — one more warrants a WORKFLOW fold.

**M4 phase-02 — token-estimation is `done`** (2026-07-14, approved_after_1).
Delivered `src/daemon/context/estimate.rs` (deterministic per-message estimate
`chars/4 + 8 + 12·items`, `estimate_history_tokens`, EMA `update_token_scale`
clamped to [0.5, 4.0]), `token_scale: f64` on `SessionEntry` (all 7 construction
sites), calibration at both `stream.rs` write-back sites, and the post-restart
blind-spot fix in `server/ask.rs`. Bounced once (bug-02-1, major
`masked_diagnostic`): the first run computed `effective_prompt_tokens` but bound
it to a `_`-suppressed variable and never consumed it, so the blind-spot fix was
a no-op that passed clippy. Fix `cb92cd3` wired it into `token_pct` +
`PromptCtx`; bug `verified`, gates green (867 unit + 27 integration). Consumer is
phase 03.

**M4 phase-01 — events-rotation is `done`** (2026-07-09, escalated → session
takeover after 1 bounce; committed `3d74880`). Executor implemented the phase +
bug fixes but looped on the bug-01-3 test verification (120+ turns grepping test
stdout); the architect finished it in the main loop — extracted
`aggregate_over_range()` for a real cost-sort test (bug-01-1), corrected the
search-tail test query (bug-01-3), and ran both real-binary E2E scenarios
(bug-01-2). All three bugs `verified`; gates green (862 unit + 27 integration).

**M4 — Context Management Overhaul is scoped** (2026-07-07, PE sign-off). The
design is `docs/design/context-management.md` (failure catalog D1–D15 + target
architecture); the milestone README with all ten phase rows is
`docs/dev/milestones/M4-context-management/README.md`. All ten phase docs were
drafted at kick-off by explicit PE request — **re-verify each doc's Current
state section against the working tree before dispatching it** (earlier phases
move its anchors; each doc carries a Pre-flight step for this).

Phase order: 01 events-rotation → 02 token-estimation → 03 budget-compaction →
04 append-only-archive → 05 epoch-records → 06 ledger-rollups →
07 recall-context → 08 async-compaction → 09 session-meta-persistence →
10 ghost-and-memory.

---

**M3 — Polish & Maintenance is complete** (2026-06-28; all 10 phases `done`,
all `approved_first_try`, zero bounces, zero bug reports). Retrospective in
`docs/dev/milestones/M3-polish-maintenance/README.md` § Retrospective. All seven
M3 exit criteria met; no STANDARDS.md / WORKFLOW.md folds this milestone (M3 was
all maintenance-shaped work that confirmed existing folds rather than revealing
new patterns). The two M3 survey holdovers (error-result/response-builder
helper ~74 sites; executor approval-gate extraction) remain deferred.

---

**M3 phase-09 — consolidate-loop-ctx is `done`** (approved_first_try, 2026-06-28).
Consolidated the two remaining high-arity orchestration signatures via borrow-structs
(`AskRequest`/`AskContext` for `handle_ask`, `ConversationLoopCtx` for
`run_conversation_loop`), deleting the last two `#[allow(clippy::too_many_arguments)]`
suppressions + two `TODO(M2)` markers — clearing the "7 `TODO(M2)` markers resolved" exit
criterion. Executor commit `7edabde`; review approval `67a4d78`.

**M3 phase-08 — help-and-truncation is `done`** (approved_first_try, 2026-06-28). Added
ellipsis truncation markers on silent truncation (status bar / panel / committed text) and
completed the `/help` text (aliases, document redirect + tool-output cap). Executor commit
`66b6654`.

**M3 phase-07 — split-webhook is `done`** (approved_first_try, 2026-06-28). Split the
1210-line `webhook.rs` grab-bag into a `webhook/` directory module with three cohesive
submodules (`parse` / `process` / `server`) via the M2 C5-split idiom; glob re-exports keep
every `crate::webhook::<name>` path resolving, zero consumer edits. Only non-move edit:
`AlertStatus::as_str` `fn` → `pub(crate) fn`. Executor commit `d8aba17`; review approval `e125eae`.

**M3 phase-06 — error-hardening is `done`** (approved_first_try, 2026-06-28). Three
behavior-preserving hardening edits: `memory_prompt.rs` double-lookup → single Entry-API
expression; four `ai/mod.rs` circuit-breaker lock sites → documented `.unwrap_or_log()`
invariant (ERROR-on-poison logging); five `daemon/scheduled.rs` swallowed `notify_tx` sends →
`log::debug!` on dropped receiver. Executor commit `e7a1658`; review approval `b040651`.

**M3 phase-05 — consolidate-leaf-params is `done`** (approved_first_try, 2026-06-28).
Introduced per-function borrow-structs (`UpdateMemoryArgs`, `SaveSessionArgs`, `RunEditArgs`,
`UpdateMemoryRequest`, `CreateAgentArgs`) resolving 5 of the 7 `TODO(M2)` markers and deleting
their `#[allow(clippy::too_many_arguments)]` suppressions. Executor commit `822ba7f`; review
approval `e89255e`.

**M3 phase-04 — error-message-quality is `done`** (approved_first_try, 2026-06-28). Killed the
`render_error` `{:?}` debug-dump leak via an exhaustive `Response::kind()` label method + a pure
`error_line()` formatter (`unexpected reply from daemon (<Kind>)`), and normalized the
`/session list` + `/prompt` empty-state strings. Executor commit `77ee226`; review approval `1b9d22f`.

**M3 phase-03 — split-utils is `done`** (approved_first_try, 2026-06-28). Split the 1007-line
`src/daemon/utils.rs` grab-bag into a `daemon/utils/` directory of cohesive submodules with
`pub use <submod>::*;` re-exports preserving every `crate::daemon::utils::<name>` path. Executor
commit `bc4b76f`; review approval `4a69f1e`.

**M3 phase-02 — approval-prompt-consistency is `done`** (approved_first_try, 2026-06-27).
Unified the three interactive approval prompts through a shared `build_approval_prompt()`
builder, canonicalizing on `[Y]es [A]pprove for <label> [N]o`. Executor commit `d4097a6`;
review approval `5726f15`.

**M3 phase-01 — fix-test-hermeticity is `done`** (approved_first_try, 2026-06-27). Converted the
racy `webhook_alert_to_event_log` to a sync `#[test]` driving its one async call via `rt.block_on`
(holds `TEST_HOME_LOCK` for the whole body, restores `HOME`), and added `HOME` capture/restore to
the five leak tests. Executor commit `c52608f`; review approval `ce7c650`. 15× concurrency soak clean.

**M2 — TUI Renderer Overhaul is complete** (2026-06-27; all 16 phases `done`). Retrospective in
`docs/dev/milestones/M2-tui-renderer/README.md`. The M2 calibration fold (front-loading made
task-shape-conditional + milestone-gate clarification) landed in WORKFLOW.md (commit `70e9712`).

**M1 — Agent Tooling Improvements is complete** — all eleven phases `done`; retrospective in
`docs/dev/milestones/M1-agent-tooling/README.md`.
