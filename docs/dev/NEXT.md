# NEXT

**Active phase:
[M11 phase-07a — situational-turns-epochs](milestones/M11-knowledge-index/phase-07a-situational-turns-epochs.md)
(`todo`, drafted 2026-08-06, not yet dispatched).**

The last read surface, and the milestone's one design-latitude phase. Every
corpus is populated and searchable, but the per-turn prompt still reads only
`memories`. 07a adds a budget-capped `[SITUATIONAL]` block carrying at most one
past turn and one past epoch **from other sessions** matching the current turn,
and de-duplicates `read_line_at_offset` — which exists twice today — before
adding its third caller.

**07 split into 07a/07b at drafting.** The design doc's situational bullet is
three features in three unrelated files; together they land well over 500 lines,
and oversizing the highest-latitude phase in the milestone is the wrong risk to
take. 07b holds the ghost cold-start seeding and the incident `relates_to`
auto-linking.

**Three things pre-injected because they are the traps here:**

1. **`build_match_expr` ORs every term** (`src/memory/index.rs:123-141`,
   verified), so a whole user turn matches almost anything. The phase guards on
   the *query's* shape (≥ 3 terms of ≥ 4 chars) and caps output at two lines. An
   absolute BM25 cutoff is explicitly out of scope — phase 06 established that
   scores are corpus-relative.
2. **`PromptCtx` has no `session_id`** and the dynamic-memory call site passes
   `None`. Without it there is no way to exclude the session's own history,
   which is already in context. There is exactly **one** construction site
   (`ask.rs:645`) and the id is already in scope there, so the field is a
   one-site addition — authorized in the phase doc.
3. **The `TestHomeGuard` must be bound in the test body**, and a setup helper
   must *return* it. Phase 06 lost a whole dispatch to a helper that bound the
   guard locally, dropping it on return and letting the tests race over `HOME`.

**Deliberately not pinned:** the tree-wide `read_line_at_offset` reference count.
It is 5 before and 5 after (two definitions plus three call sites becomes one
plus four), so it is satisfied without doing any of the work — the criterion
counts definitions per file instead. Phase 06's first stall was an acceptance
criterion that could not be satisfied; this is the mirror failure, one that
passes for free.

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
