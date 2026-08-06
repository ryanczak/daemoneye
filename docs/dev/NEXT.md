# NEXT

**Active phase:
[M11 phase-05b — search-repository-new-kinds](milestones/M11-knowledge-index/phase-05b-search-repository-new-kinds.md)
(`todo`, drafted 2026-08-05, running under `/rexymcp:auto`).**

[M11 phase-05a — search-repository-fts](milestones/M11-knowledge-index/phase-05a-search-repository-fts.md)
**done 2026-08-05 by architect takeover** after 3 `hard_fail`s (all
`NoProgressStall`) and 2 assists. The executor's design was sound and was kept;
three bugs were fixed at takeover, two of them one class — **an index hit
resolved to a path without its file extension**, so `read_to_string` failed and
every hit was silently skipped (runbooks are `<name>.md`, event segments are
`<stem>.jsonl`), plus a de-dup key comparing `file_name()` against bare stems.

**Two calibration items from 05a, both worth keeping:**

1. **Takeover was called one assist early, on purpose.** Assist 1 hit its stated
   goal (8 compile errors → 0). Assist 2 carried a complete written diagnosis and
   the executor stalled anyway — disproving the theory that it reliably applies a
   stated fix. A third identical attempt had no new information behind it.
2. **My assist-2 diagnosis was wrong and cost a run.** I named missing
   reconcile-on-empty as the root cause from *reading* code. It was a real bug
   and worth fixing, but it was not why the tests failed — the failing test calls
   `index_artifact` directly, so its corpus was never empty. Running the failing
   test first would have shown the missing `.md` immediately. **"Execute, don't
   assert" applies to diagnosis, not just to specs.**

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
