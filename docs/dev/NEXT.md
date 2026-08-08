# NEXT

## Active phase: [M12 phase-02 — pane-status-classification](milestones/M12-tmux-integration/phase-02-pane-status-classification.md) (`in-progress` — **bounced 2026-08-08**, see [bug-02-1](milestones/M12-tmux-integration/bugs/bug-02-1.md))

**Start at the `ROUND 2` block at the top of the phase doc's § Acceptance
criteria.** It holds the only unfinished work: five criteria, each run and
confirmed to fail against the current tree at bounce time.

**This is a green bounce.** Round 1 shipped correct code: all four gates green,
tree clean, 1158 tests passing, all nine round-1 progress markers passing, and
both mutation pairs re-run by the architect in both directions — all of it
held. The defect is the missing `(end-to-end verification)` Update Log entry
the spec required, plus one duplicated section comment in
`src/tmux/cache_tests.rs`. `cargo test` must still report **1158, not 1159**.

Round 1 delivered D2 in full: `src/tmux/status.rs` with the `PaneStatus` enum,
pure `classify()` (Dead > Bell > shell/non-shell × activity age), `format_age`,
`Display`, and the new `summarize()`; a `PaneState.status` field stamped every
refresh; the old heuristic `SessionCache::summarize()` and its 8 tests deleted;
`is_shell_prompt` moved to the new module behind a `pub(super) use` re-export.

**Next action:** `/rexymcp:dispatch phase-02` for round 2.

### Calibration carried out of phase-02

1. **The missing-E2E-entry bounce is at two consecutive occurrences** (phase-01,
   phase-02) — a trend worth folding. The sharp point: phase-02's spec
   **already carried both M6 countermeasures** (a literal copy-pasteable block,
   and an explicit "the server-authored `(complete)` entry does not count") and
   bounced anyway. Both remedies are necessary, neither is sufficient.
2. **A runnable block with a manual step in its middle is prose wearing a code
   fence.** Phase-02's E2E block said "Make the edit manually (delete the 3
   lines), then:" for mutation pair 2 — so it could not be pasted and run end
   to end, and the executor had to reconstruct a transcript around the gap.
   Fixed in round 2 with a `perl -0pi -e` deletion and a scoped `git checkout`
   restore, **both executed at review before being written into the spec**.
   Candidate fold: every command in an E2E block must be a command, mutations
   included.
3. **The taxonomy gap is unchanged and now hit twice.** `rexymcp review`
   again warned that `missing_e2e_verification` is not a known failure class;
   the nearest, `false_completion`, is defined as self-reporting complete on a
   *red* gate, and this is green gates plus correct code with the evidence
   artifact missing. A **rexyMCP-repo** change, out of bounds here.

**M12 — Full-View tmux Integration scoped 2026-08-07** (PE decision). Eight
phases planned; settled design (D1–D7) in `docs/design/tmux-integration.md`;
milestone README at `docs/dev/milestones/M12-tmux-integration/README.md`.
Headline: multi-session pane cache, `PaneStatus` classification, `read_pane` /
`find_in_panes` / `tmux_control` tools, `/panes` inspector, one shared
targetable-panes filter.

**[phase-01 — multi-session-cache](milestones/M12-tmux-integration/phase-01-multi-session-cache.md)
approved 2026-08-08** (`approved_after_1`; one bounce,
[bug-01-1](milestones/M12-tmux-integration/bugs/bug-01-1.md), verified fixed).
`SessionCache` now retains foreign-session panes as metadata-only, all five
iteration surfaces and four target-validation sites filter to the home session
via `is_home_pane`, and closed panes are evicted by a guarded `evict_missing`.
Behavior at every existing surface is unchanged, as intended — the foreign
panes this phase admits are exposed deliberately by phases 03–05.

**Next action:** `/rexymcp:architect next` to draft phase-02
(pane-status-classification).

### Two calibration items carried out of phase-01

1. **Lock ordering across the five filter sites is inconsistent** — three clone
   `session_name` before taking `panes`, three hold `panes` while taking it. No
   deadlock is possible today (verified at review: every `session_name` guard is
   a statement-temporary, so no cycle exists). **Not bounced** — Task 4 pinned
   the ordering for `is_home_pane` and the executor followed it exactly; Task 5
   never pinned it, so this is an architect spec gap. **Phase 08's spec must pin
   session-before-panes ordering at every site it touches.**
2. **A bounce criterion that quotes its own search string is vacuous.** The
   first draft of the round-2 criteria told the executor to grep the phase doc
   for `'<test> ... FAILED'` — which matched the criterion text itself and
   returned 1 before any work existed. Caught by *running* each criterion at
   bounce time (step 3 of the four-step bounce sequence) rather than reasoning
   about it; fixed by scoping every check to the Update Log section. Same family
   as the folded vacuous-guard rules but a new instance — the *criterion* is
   self-satisfying, not the fixture. First occurrence; hold for recurrence.

**The green-bounce treatment worked again.** Round 1 shipped correct code and
all criteria passed; the only defect was the missing end-to-end verification
entry. Applying the full treatment before re-dispatch (loud header that green
gates are not evidence, do-not-touch list on every `src/` file, one enumerated
task, inverted finish condition "1153, **not** 1154") landed it in 37 turns
with zero source files touched. Second local confirmation of that fold.

**The taxonomy gap is now visible from this repo.** The bounce was recorded as
`missing_e2e_verification`, which `rexymcp review` warned is not a known class.
The nearest existing class, `false_completion`, is defined as self-reporting
complete on a *red* gate; this was green gates plus correct code with the
evidence artifact missing. Same gap NEXT.md already tracks for fabricated
evidence under green gates — a **rexyMCP-repo** change, out of bounds here.

---

**M11 — Unified Knowledge Index closed 2026-08-07.** Twelve phases, all `done`;
all nine exit criteria verified at close against source. Verdicts: 1
`approved_first_try`, 6 `approved_after_1`, 5 `escalated`. Nine bug docs, all
resolved. Retrospective in
`docs/dev/milestones/M11-knowledge-index/README.md`.

### Two folds landed 2026-08-07 (PE sign-off)

Both are in `docs/dev/WORKFLOW.md`. **Neither is applied upstream.**

1. **The bounce sequence is now four numbered steps**, in
   § "Review and Bug-Report Cycle" — write the bug, flip the status, **refresh
   the acceptance criteria and confirm each fails against the current tree**,
   update the README and telemetry. The criteria-refresh requirement previously
   existed as a parenthetical inside a compound "rejects" step and was skipped by
   the architect who had folded it in the day before; a rule that must be
   remembered when writing a bug report has a failure rate, a rule that is step 3
   of 4 gets checked off. Second occurrence of the empty-diff failure (07a r2,
   07b r2); first occurrence of the original fold failing to be applied.
2. **A guard's premise must be demonstrated, not described** — appended to
   § "Coverage claims are inadmissible without mutation proof". Distinct from the
   three failures already there, which are about what the *assertion* can see;
   this is about the *fixture* being inert rather than near-miss, so the guard is
   never reached. Requires a both-directions mutation pair in the phase doc, and
   explicitly rejects an intent-stating comment as a substitute. Third occurrence
   of the vacuous-guard family (03a, 05b, 07b).

**Note on fold 1's scope.** The proposal was to make it a step in the *review
skill's* §8 sequence. That file lives in the rexyMCP repo and is out of bounds
here, so what landed is the in-repo half: `WORKFLOW.md` now carries the ordered
sequence. Mirroring it into the skill remains an upstream change, and until it
lands the skill's §8 and this file disagree on structure.

### Four upstream folds pulled in 2026-08-07

The sync gap was **bidirectional** — this repo was behind upstream on four
sections, discovered while inventorying what to push. All four are now in
`docs/dev/WORKFLOW.md`:

- **`Derive every spec fact from its source`** — folded upstream 2026-07-24
  after **ten** occurrences. This is the one that stings: M11's single largest
  failure class was architect-authored spec facts asserted rather than executed,
  and we re-derived that lesson across four bug docs and several dispatches
  while the fold sat in the template, unsynced.
- **`Governing a running phase`** — the `stop_phase` discipline and the
  don't-babysit-with-a-poll-loop rule.
- **`Pin the fixture that makes the row appear`**.
- **`Pre-inject compiler-error-driven recovery on oscillation-prone files`**.

**Reconciled rather than concatenated:** `Pin the fixture` and this milestone's
own `A guard's premise must be demonstrated` are two halves of one failure — a
fixture that does not exercise the path the test names. Inert fixture -> vacuous
pass; empty fixture -> spurious failure and a phantom bug hunt. Both now carry a
cross-reference and a shared draft-time question ("what in this fixture makes
the code take the branch I am asserting on?"). That synthesis is itself a
candidate to push upstream.

The only remaining divergence is `## How to fix` / `## Verification`, which the
2026-08-06 bug-report fold deliberately replaced with `## Root cause` +
`## Definition of done`. Do **not** pull those back.

### Still out of bounds from this repo

**Push direction, still outstanding.** ~503 lines across 12 local-only
`WORKFLOW.md` sections, ~13 lines of local-only `STANDARDS.md` DoD boxes (the
mechanical-capture and own-entry E2E requirements — exactly what phase-07b
violated three dispatches running), the bug-report template's breaking
`How to fix` -> `Root cause` + `Definition of done` change, and the
`false_completion` taxonomy gap in `executor/src/store/telemetry.rs:341-346`.
Plus mirroring the four-step bounce sequence into the review skill's §8. All are
**rexyMCP-repo** edits; a target-project architect session cannot make them.

---

## Historical record below (M11 and earlier)

## Three calibration folds landed 2026-08-06 (PE sign-off)

All three are in `docs/dev/WORKFLOW.md`. **None is applied upstream** — the same
clauses belong in rexyMCP's `plugin/templates/WORKFLOW.md`, which is out of
bounds from a target-project architect session and needs a separate change in
that repo. This is the second batch carrying that caveat.

1. **Bug reports state symptom / root cause / DoD, not the fix.** The template's
   `How to fix` section is gone, replaced by `Root cause` + `Definition of done`;
   a prescribed fix is now optional and admissible only when the architect has
   run it. **This was the PE's counter-proposal to the fold I suggested** — I had
   argued for "execute prescribed fixes before writing them", and giving the
   executor the diagnosis plus the finish line instead is the better remedy: it
   removes the class of error rather than adding a verification step to it. Four
   occurrences in M11, and in every one the executor's behavior was correct given
   what it was told.
2. **A bounce must refresh the phase doc's acceptance criteria**, and each new
   criterion must be confirmed to fail against the current tree. Folded on first
   occurrence at the PE's direction, because the failure is silent and
   self-certifying — green gates, clean tree, accurate summary, empty diff.
3. **A fixture's ordering premise must be asserted in the test.** Extends the
   vacuous-guard family: not an unobservable property or an empty fixture, but a
   false assumption about which candidate comes first, so the path under test is
   never entered.

**Active phase:
[M11 phase-07b — situational-knowledge-hooks](milestones/M11-knowledge-index/phase-07b-situational-knowledge-hooks.md)
(`in-progress` — **bounced ×2 on 2026-08-07**, see
[bug-07b-1](milestones/M11-knowledge-index/bugs/bug-07b-1.md)). **This is the
last phase of M11** — after it, the milestone hits its human gate.**

**Start at the `ROUND 2` block at the top of the phase doc's § Acceptance
criteria.** That block holds the only unfinished work: four criteria, each run
and confirmed to fail against the current tree.

**Round 2 returned `complete` with an empty diff and changed nothing** — the
documented green-bounce failure mode. The cause was architect-side: round 1's
bounce filed the bug but left all eight original criteria passing, so the phase
doc still certified itself as finished and the executor correctly concluded
there was no work. The criteria are refreshed as of round 3; the bug doc alone
was not enough.

**This is a green bounce.** Round 1 shipped correct production code and all
eight original acceptance criteria pass; four green gates and a clean tree are
*expected* and are not evidence the phase is done. Two edits remain — one test
fixture
(`incident_context_is_none_for_a_low_signal_alert`, whose query shares no term
with its seed, so deleting the guard it exists to protect leaves all 12
situational tests green) and one self-referential rustdoc link — plus the
`(end-to-end verification)` Update Log entry the spec required and round 1 never
wrote. `cargo test --lib` must report **1147, not 1148**. Full detail and a
both-directions-executed fixture recipe are in the bug doc.

The two remaining write/read choke points that ignore the index. An
incident-response ghost starts cold on every alert with no memory of the last
time the same one fired; and `add_memory` to `incidents` never fills
`relates_to`, so `expand_relates_to` — which already consumes that field on the
prompt path — has nothing to walk. Five tasks: an additive
`fts5_search_in_category` (leaving `fts5_search` a wrapper so its two callers are
untouched), `inject_yaml_relates_to` mirroring the `session_origin` injector,
auto-linking on incident writes, an `assemble_incident_context` block seeded into
the ghost's first user turn, and tests.

**The one trap that would silently void this phase:** the index stores the
category's **`canonical_name()`** — `"incident"`, singular — while
`dir_name()` is `"incidents"`. Filtering on the plural matches zero rows and
every test built on it passes vacuously. Verified at drafting
(`src/memory.rs:24-40`, `src/memory/index.rs:785`), pre-injected, and pinned as a
negative criterion.

**First phase drafted under the three folds.** Its acceptance criteria are split
into five that **fail against the current tree** (progress markers, each run at
drafting) and two that **already pass** (no-regression guards, labelled as such
so neither is mistaken for evidence of work). And where 07a's spec would have
prescribed a fix, the E2E block now says: if the mutation leaves every test
green, **report a blocker rather than adjusting a test to make it fail** — that
is precisely what cost 07a a dispatch.

[M11 phase-07a — situational-turns-epochs](milestones/M11-knowledge-index/phase-07a-situational-turns-epochs.md)
**completed 2026-08-06** (`escalated` — architect takeover after 2 bounces and a
`NoProgressStall`; [bug-07a-1](milestones/M11-knowledge-index/bugs/bug-07a-1.md)
minor, fixed). A `[SITUATIONAL]` block now carries one cross-session turn and one
epoch into the per-turn prompt, `read_line_at_offset` lives once in
`src/memory/index.rs`, and `PromptCtx` carries `session_id`. All three failures
traced to my specs, not the executor: a prescribed fix that was wrong twice, and
a stale acceptance-criteria set. Those are the three folds above.

[M11 phase-06 — prompt-scoring-fix](milestones/M11-knowledge-index/phase-06-prompt-scoring-fix.md)
**completed 2026-08-06** (`escalated` — architect takeover after two
`NoProgressStall` hard_fails; 0 bug docs, neither failure was a defect in
shipped work). FTS hits now score `FTS_WEIGHT * mag/mag_max * eff`, the merge is
keyed by `(namespace, key)` with max-wins, production code makes one memory-dir
listing per turn instead of four, and `memory_retrieved` logs what it emitted.
The executor wrote the production code and five of six test fixtures; the
architect fixed the sixth and captured the mutation pair. Two calibration items
recorded in the phase doc — an acceptance criterion invalidated by the spec's own
Test plan (`spec_bug`, second occurrence of the general class), and a worked
example that showed the correct shape without the failure mode it prevents
(first occurrence).

[M11 phase-05c — reconcile-scope-fix](milestones/M11-knowledge-index/phase-05c-reconcile-scope-fix.md)
**approved 2026-08-06** (`approved_after_1`; one bounce,
[bug-05c-2](milestones/M11-knowledge-index/bugs/bug-05c-2.md) — the transcript
was not mechanically captured, the code fix was correct first time). A search
over an empty corpus no longer wipes every other corpus:
`open_and_reconcile_if_empty(table)` rebuilds only that corpus, and phase 05b's
whole-index seeding workaround is gone.

## Calibration earned on 05c (superseded items are marked in each phase doc)

1. **A mutation check the executor performs on itself is not trustworthy.** On
   05b it applied the mutation and failed to restore it *twice* — once rewriting
   the guard test to assert the mutated behavior, once leaving the mutation in
   shipped code after an explicit two-line undo instruction. The phase-doc
   instruction is necessary but not sufficient; **the restore must be verified at
   review by grepping the shipped source.** Third occurrence of self-reported
   verification not surviving checking (03b fabricated transcript, 05a untested
   diagnosis, 05b unreverted mutation).
2. **"Verify the guard is not vacuous" belongs inside every exclusion criterion.**
   05b's `"all"`-excludes-turns test passed *with the mutation applied* — it
   proved nothing, because an unrelated empty corpus had silently wiped its
   fixture. Absence assertions pass trivially whenever the fixture is empty for
   any reason. This is the second occurrence (03a's `line.contains("turn")` was
   the first) — **one more and it folds into WORKFLOW.md.**
3. **"Execute, don't assert" applies to diagnosis.** My 05a assist-2 root cause
   was derived from reading code and was wrong; it cost a run. Running the failing
   test first would have shown the real cause immediately.
4. **The read-only stall is now at 5 occurrences** across 03a, 05a (×3) and 05b
   (×2). Well past the fold threshold, but the remedy is runtime-side in the
   rexyMCP governor and out of bounds from this repo.

## The rule 03b earned: re-run a pasted transcript, never read it

The bounce was a **fabricated** end-to-end transcript — 39 of 56 pasted
`memory::index` test names did not exist, describing ~25 error-injection tests
for `index_event_segment` in a diff that added none. **The totals were correct**
(52 and 94 are the true counts), so skimming would have passed it. Only diffing
the pasted names against a live run caught it, and that diff is now the standard
check at review:

```sh
cargo test --lib <module> 2>&1 | grep '^test ' | sed 's/ \.\.\..*//' | sort > /tmp/real
# extract the pasted block, sort, then: comm -13 /tmp/real /tmp/claimed
```

Scope the extraction to the entry's line range — a whole-file grep also picks up
the server-authored completion block's full `cargo test` output and produces
false positives.

**Taxonomy gap, still open.** Recorded as `false_completion`, but that class is
defined as self-reporting complete on a *red* gate. There is no class for
**fabricated evidence under green gates**, which is the more dangerous shape:
correct code plus passing gates make it look like a clean pass. Worth raising in
the rexyMCP repo.

**The green-bounce treatment works on this executor.** 03b's fix round was a
green bounce (four green gates, clean tree, documentation-only fix) — the shape
whose documented failure mode is `complete` with an empty diff. Applying the full
treatment before re-dispatch (loud header that green gates are *not* evidence of
doneness, explicit do-not-touch list, one enumerated task, inverted finish
condition "1084, **not** 1085") landed it in 31 turns with zero source files
touched. First local confirmation of that fold.

## Two lessons out of 03a, both at the fold threshold

**1. Identity criteria must pin distinctness — second occurrence, one from a
fold.** An acceptance criterion of the form "each X maps to *its own* Y" needs
the spec to name the discriminator *and* forbid the vacuous one. "each offset
seeks to its own line" was satisfied by `line.contains("turn")` — a JSON key
every record carries. The fix that worked was asserting the values are **pairwise
distinct**, not merely individually well-formed. If this recurs, fold it.

**2. A prescribed fix in a bug report is a system fact, and must be executed
before it is written — `spec_bug`, second occurrence in M11.** `bug-03a-1`
Finding 2 called `.or(Some(0))` a defect and prescribed removing it. Applying
that instruction at review breaks three tests: `metadata` fails principally
because the archive *does not exist yet* on a fresh append, where offset `0` is
correct. The executor restored the fallback and was right to; the finding was
withdrawn. The M7–M10 rule ("do not assert a fact about the system in a spec
unless it was executed") was written for phase specs and had not been applied to
bug reports, where the prescribed fix is exactly such an assertion. First
occurrence was `bug-02b-1` Finding 1's `read_line` recipe. **One more and this
folds into WORKFLOW.md as a bug-report clause.**

**3. The executor verify-loop pathology recurred (second occurrence).** The first
re-dispatch of 03a hard-failed on `NoProgressStall` after 60 consecutive
read-only turns grepping for the import path of `crate::ai::Message` — unrelated
to the remaining work. Resume with pointed guidance (name the stall, mark the
already-correct files do-not-touch, inline the fix, give an inverted test-count
finish condition) cleared it in 48 turns. Prefer resume over takeover here; the
prior note said prefer takeover, which would have cost the telemetry point
unnecessarily.

**Calibration fold landed 2026-08-04 (PE sign-off).** `docs/dev/WORKFLOW.md`
§ "End-to-end verification" now requires the entry **per dispatch**, not per
phase: a bounce-fix round needs its own, and an entry from an earlier round does
not carry forward. Folded after three occurrences (phase-01 r1, phase-02a r2,
phase-02b r2). **Not yet applied upstream** — the same clause belongs in
rexyMCP's `plugin/templates/WORKFLOW.md`, which is out of bounds from a
target-project architect session and needs a separate change in that repo.

[M11 phase-02b — contentless-corpora](milestones/M11-knowledge-index/phase-02b-contentless-corpora.md)
**approved 2026-08-04** (`approved_after_1`; one bounce,
[bug-02b-1](milestones/M11-knowledge-index/bugs/bug-02b-1.md), verified fixed).
`turns` and `events` are populated with byte-offset sidecar maps, masked on
index, and resilient to corrupt files. All five corpora now build from disk.

[M11 phase-02a — index-schema-v2](milestones/M11-knowledge-index/phase-02a-index-schema-v2.md)
**approved 2026-08-03** (`approved_after_1`; one bounce,
[bug-02a-1](milestones/M11-knowledge-index/bugs/bug-02a-1.md), verified fixed).
The index now carries all seven tables at SCHEMA_VERSION 2, with `artifacts` and
`epochs` populated and `daemoneye reindex` reporting per-corpus counts truthfully.

[M11 phase-01 — write-time-masking](milestones/M11-knowledge-index/phase-01-write-time-masking.md)
**approved 2026-08-03** (`approved_after_1`; one bounce,
[bug-01-1](milestones/M11-knowledge-index/bugs/bug-01-1.md), verified fixed).
`append_epoch` and `log_event` now mask at the write choke point, so the epoch
and event corpora are safe to index in phases 02–03.

**M11 — Unified Knowledge Index scoped 2026-08-03** (PE decision). Seven phases
planned; design settled in `docs/design/knowledge-index.md`; milestone README at
`docs/dev/milestones/M11-knowledge-index/README.md`.

**M10 — Residual Hygiene closed 2026-08-02** (three phases, all
`approved_first_try`, zero bugs, zero bounces). Retrospective:
`docs/dev/milestones/M10-residual-hygiene/README.md`.

## The carried list is empty except for one unreproducible item

1. **`hooks_land_on_private_server`** — the old phase-04-review flake. Binds no
   ports; **0 failures in 300 runs** across M8, M9 and M10. No evidence to work
   from. Only a bug if it recurs.

Everything else carried out of M7 and M8 is closed: the tty tests fail instead of
hanging, the memory category→directory mapping is derived in all three callers,
the last real-clock sleep is gone, and `daemoneye reindex` is documented and gated.

## One calibration item, resolved at close

The executor mislabelled its own model in its Update Log entry three times (M9
phase-01, M10 phase-01, M10 phase-03) — it is Qwen3.6-27B-FP8 every time. Three
occurrences hit the fold threshold, and the PE's decision was to drop the model
line from the executor's own entry.

**Applying it revealed the premise was wrong: no template asks for that line.**
`docs/dev/WORKFLOW.md` § "Update Log entries" defines progress, blocker and
completion entries, and none has an `**Executor:**` field. The only one in the
file is at `:347` in the **Review verdict** template — the architect's line, which
has been correct throughout. The embedded executor contract does not request it
either.

The executor adds it unprompted, so there is nothing to delete and no fold to
file. The operative consequence is for review: **an unrequested, self-reported
model name in an executor entry is not a defect against any spec, and should not
be corrected in place.** It was corrected three times on the assumption it was
contract-mandated.

Actively suppressing it would mean editing `executor/templates/executor_contract.md`
in the **rexyMCP** repo — out of bounds from a target-project architect session.

## The rules M7–M10 earned

> **Do not assert a fact about the system in a spec unless it was executed.**
> A *claimed failure mode* is such a fact — M9 justified a test with a
> compile-time impossibility one `cargo build` would have disproven.
>
> **A criterion about the tree the phase will produce must be validated against
> that tree**, not the one in front of you. Calibrating against the current tree
> catches unsatisfiable criteria; it does not catch criteria the phase's own work
> invalidates, or criteria that already pass without the work being done.
>
> **Prototype the change and mutate it before writing the spec.** M10 phase 02's
> real risk — two labels with no test, where a swap left 1036 tests green — was
> invisible until the prototype was mutated.
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.** A single green run is not evidence.
>
> **Measure through the same door the user will use.** M9's in-process probe of
> `reconcile_index()` recorded a bare-`$HOME` result the shipped binary never
> produces.

Corollaries, each earned more than once: naming a false-success mode is worthless
unless the guard is checked against it; a phase that lands code for a *later*
phase must say how the deny-warnings gate is satisfied; and **a green bounce
always needs a refined re-dispatch**, never a plain one.
