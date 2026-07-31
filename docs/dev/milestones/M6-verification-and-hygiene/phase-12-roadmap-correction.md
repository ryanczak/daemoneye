# Phase 12: Roadmap Correction

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phases 01–11 (all done)
**Estimated diff:** ~120 lines
**Tags:** language=markdown, kind=docs, size=s

## Goal

Make `docs/architecture.md` § 5 factually true as of M6's close, and remove one
false belief the agent is still being told — found while drafting this phase,
and a textbook instance of the defect class this milestone exists to eliminate.

This is M6's last in-scope phase. **It does not write the milestone
retrospective and does not relabel M6 from "Active" to "Shipped"** — both are
milestone-close work and belong to the human gate. See "Out of scope".

## Architecture references

Read before starting:

- `docs/architecture.md` § 5 "Milestone roadmap", especially `### Active
  milestone — M6 Verification & Hygiene` (~line 376) and the FTS5 note that
  follows it.
- `docs/dev/milestones/M6-verification-and-hygiene/README.md` — the phase table
  is the source of truth for what actually shipped.
- `assets/memory/knowledge/agent-runtime-layout.md` — the ASCII tree, for task 2.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is clean and `cargo test` is green at 990 lib / 30
   integration (2 ignored) / 8 isolation (1 ignored).

## Current state

**Verified against the tree while drafting.**

**§ 5 no longer names M4 as active.** The milestone README's exit criterion says
architecture.md "no longer points at a superseded 'active milestone'" — that was
already corrected when M6 was scoped on 2026-07-30. The section now reads
`### Active milestone — M6 Verification & Hygiene`.

**What *is* stale is M6's own entry.** It says the milestone README has "twelve
phases named, none drafted". All twelve are now drafted and eleven are `done`;
phase 06 was split into 06a/06b, so there are thirteen phase docs. The entry also
describes the milestone's five axes in the future tense, as scope rather than as
delivered work.

**The FTS5 note is still accurate — verified, not assumed.** § 5 says the FTS5
memory index is "currently a **stub** (`src/memory/index.rs` returns empty; real
search is the grep scan in `src/search.rs`)". `src/memory/index.rs` is eight
lines:

```rust
//! G5: FTS5 memory index (stub — not yet implemented).
pub fn fts5_search(_query: &str, _limit: usize) -> Vec<(String, f64)> {
    Vec::new()
}
```

Leave that note alone. It is the doc being honest.

**But the agent is still told the index exists.**
`assets/memory/knowledge/agent-runtime-layout.md:40` shows, inside the ASCII
tree:

```
      memory.db            ← FTS5 full-text search index (SQLite)
```

Nothing under `src/` references `memory.db` or `var/index` at all — `grep -rn
"memory.db\|var/index" src/` returns nothing. **That file never exists**, so the
agent is being told to look for something that cannot be there. This is defect
class 4/5 exactly, and it survived phases 02 and 03 only because it lives in a
**code fence**, which the backtick-delimited extractor is structurally blind to —
the limitation phase 03 recorded deliberately.

## Spec

### 1. Bring § 5's M6 entry up to date

Rewrite the M6 entry so it describes what the milestone **delivered**, not what
was scoped. Take the facts from the milestone README's phase table — do not
re-derive them from memory. At minimum it must no longer claim "none drafted",
and it should note the 06a/06b split so the phase count reconciles.

Keep it proportionate to the neighbouring M4 and M5 entries — a paragraph, not a
phase-by-phase log. The detailed record lives in the milestone README.

**Do not** change the M1–M5 entries, the shipped baseline, or the FTS5 note.

### 2. Stop telling the agent about a file that does not exist

Remove the `memory.db` line from the ASCII tree in
`assets/memory/knowledge/agent-runtime-layout.md`, along with the `var/index/`
parent if that leaves it empty. Check the surrounding prose for any other mention
of an FTS5 index or `memory.db` and remove those too — say how many you found.

**Verify with the gate, not by eye.** After the edit, `cargo test --lib
path_audit` must still be green. If removing the line leaves a dangling parent
that the audit now flags, fix that too — that is the phase-02 gate doing its job.

**Do not** implement FTS5, un-stub `src/memory/index.rs`, or touch
`src/search.rs`. The stub is honest and documented; making it real is future work
the M4 design doc already records.

### 3. Leave the close to the human

Do **not** relabel `### Active milestone — M6` to `### Shipped — M6`, and do
**not** write a retrospective section. M6 is not closed until the human signs off,
and the retrospective is theirs to approve. Phase 12 makes the section *true*; the
close makes it *final*.

## Acceptance criteria

- [ ] § 5's M6 entry no longer says "none drafted" and reflects the delivered
      phase set, including the 06a/06b split.
- [ ] The M1–M5 entries, the shipped baseline and the FTS5 stub note are
      unchanged.
- [ ] No `memory.db` or FTS5-index reference remains in
      `assets/memory/knowledge/agent-runtime-layout.md`.
- [ ] `cargo test --lib path_audit` is green — the asset edit did not introduce
      an `Unknown` finding.
- [ ] `### Active milestone — M6` is still labelled active, with no retrospective
      section added.
- [ ] `src/memory/index.rs` and `src/search.rs` are untouched.
- [ ] All four gates green, test counts unchanged at 990 / 30 (2 ignored) /
      8 (1 ignored).

## Test plan

This phase ships documentation and one asset edit; the executable check is the
phase-02 path audit, which already covers the asset.

**No new tests are expected.** If you believe one is warranted, say why in the
Update Log rather than adding it silently.

**Do not pin a test count in advance** — but the expected delta is zero.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

**The server-authored `(complete)` entry's "Command output tails" block does NOT
satisfy this.** This requirement has cost ten bounces and two architect takeovers
on this milestone — more than any other single cause. Author your own entry.

```sh
cargo test --lib path_audit -- --nocapture \
  > /tmp/e2e-12-audit.txt 2>&1; echo "exit=$?" >> /tmp/e2e-12-audit.txt

grep -rn "memory\.db\|var/index" assets/ src/ \
  > /tmp/e2e-12-grep.txt 2>&1; echo "grep-exit=$?" >> /tmp/e2e-12-grep.txt
```

Paste both. `/tmp/e2e-12-grep.txt` should contain **no matches** and
`grep-exit=1` — grep exits 1 when it finds nothing, which is the success case
here, so the exit marker is what makes an empty file meaningful.

## Authorizations

- [ ] May modify `docs/architecture.md` § 5 — **this phase only**; every other
      M6 phase forbade it.
- [ ] May modify `assets/memory/knowledge/agent-runtime-layout.md`.

No new dependencies. No code changes.

## Out of scope

- **Do not write the M6 retrospective** or relabel the milestone as shipped —
  human gate.
- **Do not implement FTS5** or modify `src/memory/index.rs` / `src/search.rs`.
- **Do not touch `docs/dev/STANDARDS.md` or `docs/dev/WORKFLOW.md`** — contract
  docs, human gate.
- **Do not update `docs/dev/NEXT.md`** — that belongs to the close.
- **Do not widen the path-audit extractor to code fences.** Phase 03 recorded
  that limitation deliberately; this phase fixes the one instance it let through,
  not the extractor.
- **Do not delete anything from the operator's `~/.daemoneye/`.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
