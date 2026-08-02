# M7 — Memory Search & Maintenance

**Goal:** Make memory recall actually match what the user said — and bring the
dependency tree, the path-audit gate, and the bug tracker back to a state that
does not silently lie.

**Status:** planning

**Depends on:** M6 (Verification & Hygiene) — closed 2026-07-31.

**Scoped:** 2026-07-31, PE decision, from M6's carried-forward list plus a survey
run at close. One capability (working memory search) and one maintenance axis.

**Exit criteria:**

- [ ] **`fts5_search()` is real.** `src/memory/index.rs` maintains a SQLite FTS5
      index and returns BM25-ranked hits. Verified by a test that stores a memory
      whose *text* matches a query but whose *tags* do not, and asserts recall
      surfaces it — the case that cannot work today.
- [ ] **The index survives edits.** Adding, updating and deleting a memory keeps
      the index consistent with the files on disk, verified by a reconciliation
      test rather than by construction order.
- [ ] **`CLAUDE.md` and `docs/architecture.md` describe the index as it is** once
      it is real — including removing architecture.md § 5's "currently a stub"
      note, which is accurate today and must not become the next stale claim.
- [ ] **Every direct dependency is on its latest stable release**, or carries a
      one-line note in `Cargo.toml` saying why it is pinned back. `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test`
      all green afterwards.
- [ ] **The path audit sees fenced code blocks.** A stale path inside a fenced
      shell command fails the gate. Demonstrated by the mutation, not asserted.
- [ ] **`agent-runtime-layout.md`'s directory tree is generated, not
      hand-maintained** — derived from `POLICY_TABLE` / `ensure_dirs()` so it
      cannot drift. Two of M6's three gate escapes were hand-edited tree lines.
- [ ] **No bug doc is marked `open` while its phase is `done`.** Enforced by a
      test, not by discipline; the five currently-stale docs are closed as part of
      landing it.
- [ ] **No real-clock `sleep` in a non-`#[ignore]`d test.** `STANDARDS.md` §3.3
      already forbids it. **Three** live sites predate the rule (corrected from
      "four" during phase-03 drafting — the original count came from a text grep
      that both over- and under-counted; see phase-03 § Current state).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
      test` green; no regression against the 991 lib + 30 integration + 8
      isolation baseline M6 closed at.

## Architecture references

- `docs/architecture.md` § 1.4 "tmux integration & persistence layer" and
  § 5's FTS5 stub note — the claim this milestone makes true.
- `docs/architecture.md` § 2.3 "Knowledge flow" — where memory recall sits.
- `CLAUDE.md` § "Key files" — `src/memory.rs`, `src/memory/index.rs`,
  `src/daemon/memory_prompt.rs`.
- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § "M6
  retrospective" — the carried-forward items this milestone drains.

## Phases

**All ten phases are drafted.** Nine are `done`; phase 09 is the last in-scope
phase. Ordering is deliberate — see Notes § "Why this order".

**Phase 10 is numbered last but is independent of the FTS5 chain and may be
dispatched at any point** — it touches `src/session_store.rs`, the runtime tree,
the policy table and `CLAUDE.md`, none of which phases 07–08 go near. It is
numbered 10 rather than inserted at 08 so that phase 07's spec, which already
refers to "phase 08" and "phase 09" by number, stays accurate.

| #  | Phase | Status |
|----|-------|--------|
| 01 | [dependency-currency](phase-01-dependency-currency.md) — update every direct dependency to latest stable; hold `libc` back from its alpha | done |
| 02 | [bug-tracker-truth](phase-02-bug-tracker-truth.md) — a gate failing when a bug doc is `open` while its phase is `done`; close the five stale M2/M4 docs it catches | done |
| 03 | [test-sleep-removal](phase-03-test-sleep-removal.md) — the three live-test `sleep` sites `STANDARDS.md` §3.3 forbids | done |
| 04 | [path-audit-fenced-blocks](phase-04-path-audit-fenced-blocks.md) — extend extraction to fenced code blocks, multi-segment rule (Part A of M6 item 5) | done |
| 05 | [generated-runtime-tree](phase-05-generated-runtime-tree.md) — render `agent-runtime-layout.md`'s tree from a table in Rust, with an equality test against the shipped asset (Part B of M6 item 5) | done |
| 06 | [fts5-index-schema](phase-06-fts5-index-schema.md) — add `rusqlite` (`bundled`, authorized — see Notes); FTS5 schema, creation, and registering `var/index/memory.db` in all four gates | done |
| 07 | [fts5-write-path](phase-07-fts5-write-path.md) — index maintained on add/update/delete, with reconciliation | done |
| 08 | [fts5-search](phase-08-fts5-search.md) — BM25 ranking wired into `ftsearch_memories()`, per-term query building, reconcile-on-empty, and the tag-miss/text-hit test | done |
| 09 | [index-doc-correction](phase-09-index-doc-correction.md) — correct five stale/never-true claims in `CLAUDE.md` and architecture.md, plus a tripwire so the retired ones cannot return | done |
| 10 | [tree-and-doc-truth](phase-10-tree-and-doc-truth.md) — `memory/incident` → `incidents` (incl. a live stamping bug), the per-category `POLICY_TABLE` entries that close the gate gap, `agents/*/memory/`, and two false `CLAUDE.md` rows | done |

Phases 06–08 may be re-split once 06 lands; the FTS5 work is the least-known part
of this milestone and the phase boundaries are a guess until the schema exists.

## Notes

### Dependency decision — settled 2026-07-31 (PE)

**`rusqlite` with the `bundled` feature.** SQLite is compiled from source into the
binary, so FTS5 availability is a property of *our build*, not of the operator's
machine. For a daemon that ships to arbitrary operator hosts, a search index that
silently does not exist on some of them is the failure mode this milestone is
removing — paying build time to eliminate it is the right trade.

Verified empirically before recording, not assumed:

```
$ cargo add rusqlite --features bundled     # in a scratch crate
$ cargo run
sqlite_version=3.53.2
fts5=AVAILABLE
compile_options_fts=["ENABLE_FTS3", "ENABLE_FTS3_PARENTHESIS", "ENABLE_FTS5"]
```

Three facts that follow, and that phase 06 must not re-litigate:

- **`bundled` alone is sufficient for FTS5** — `ENABLE_FTS5` is in the bundled
  build's compile options. Do **not** reach for `bundled-full`; it enables a large
  set of extensions we do not use.
- **Latest stable is `0.40.1`**, bundling SQLite 3.53.2. Phase 01 sweeps
  dependencies *before* this lands, so 06 adds it already-current.
- **The transitive cost is six small crates** — `bitflags`,
  `fallible-iterator`, `fallible-streaming-iterator`, `hashlink` (→ `hashbrown`
  → `foldhash`), `libsqlite3-sys`, `smallvec`. The `ffi-sqlite-wasm-rs` default
  feature is **target-gated to wasm** and compiles nothing on native targets;
  confirmed by inspecting the built dep directory. No need to disable default
  features.

The build-time cost is real — compiling SQLite adds roughly a minute to a cold
build — and is accepted.

### Why this order

**01 first, because everything else builds on it.** A dependency sweep changes the
base every later phase compiles against; doing it last would mean re-validating
finished work. Doing it first also surfaces any breaking API change while the tree
is otherwise quiet.

**02 and 03 early, because they are small and they clean the instruments.** M6's
review gate is only as trustworthy as the tracker it writes into; five bug docs
asserting `open` for fixed defects is exactly the false signal that gate exists to
prevent. Both phases are hygiene, both are cheap, and neither blocks anything.

**04 and 05 before the FTS5 work, deliberately.** Phase 06 adds
`var/index/memory.db` — a path that must appear in the path-audit inventory, the
lifecycle policy table, and the runtime-layout tree.

*Corrected after phase 05 landed (2026-08-01).* This originally read "phase 06
adds one inventory entry and the tree updates itself." That is not what phase 05
built, and phase 06's spec should not inherit the wrong expectation. The tree
carries files, the `memory/{session,knowledge,incident}` split, and purpose
annotations that `POLICY_TABLE` does not have, so it has its own table
(`RUNTIME_TREE`) rather than being derived from the policy table. Phase 06
therefore makes **four** one-entry edits — `POLICY_TABLE`, `RUNTIME_TREE`, the
asset the tree renders to, and the path-audit `INVENTORY`.

The ordering still earns its place, just for a sharper reason: **none of those
four edits can be silently skipped.** `every_policy_path_appears_in_tree` fails
until the tree entry exists, `render_matches_shipped_asset` fails until the asset
matches, `every_existing_directory_has_a_policy_entry` fails until the policy
entry exists, and `audit-prompts` reports the path `Unknown` until it is
inventoried. Landing 04 and 05 after 06 would mean hand-editing the tree with no
gate watching — the drift this milestone is removing.

**06 → 07 → 08 as schema → write → read.** The schema is the load-bearing
decision; the write path is where reconciliation bugs live; the search path is
where the user-visible behaviour finally appears. Splitting them keeps the
executor's blast radius small on the least-understood work in the milestone.

**09 last**, because it documents what was built, and M6 phase 12 demonstrated
that documenting a milestone before it closes produces claims that are not yet
true.

### Defect inventory (2026-07-31 survey, verified against the tree)

**The stub is a live degradation, not just an inaccuracy.** Memory recall merges
three candidate sources in `daemon/memory_prompt.rs:64-76` — tag overlap,
`relates_to` expansion, and FTS5 against the user's turn. The third calls
`index::fts5_search()`, which returns an empty `Vec` unconditionally. So a memory
whose *text* matches what the user said is surfaced only if its *tags* happen to
overlap. The feature is degraded silently and invisibly; nothing logs that the
FTS5 arm contributed nothing.

**Five bug docs are stale, not open.** Each was verified against the code at M6
close and each is fixed:

| Doc | Claim | Verified reality |
|---|---|---|
| M2 `bug-phase-01-1` | banned `unsafe` / `#[allow]` in the ratatui wiring | `render_ratatui.rs` contains neither |
| M2 `bug-phase-01-2` | ratatui path never enters raw mode | `enable_raw_mode()` at `render_ratatui.rs:170` |
| M2 `bug-phase-02b-1` | approval line-editing inert | uses `InputLine` + `draw_prompt`, as the fix instruction specified |
| M2 `bug-phase-02b-2` | credential prompt returns masking bullets | `cred_real` holds characters, `cred_display` holds bullets |
| M4 `bug-09-1` | boundary-reload fixtures vacuous | fixtures carry `tool_calls` / `tool_results` |

Two of these are marked **blocker**. A tracker that reports two open blockers
against a shipped milestone is worse than one that reports none, because it
trains everyone to ignore it — hence phase 02 lands a gate rather than only a
cleanup.

**Three live `sleep` sites in tests** — recounted during phase-03 drafting by
walking each `sleep(` back to its enclosing function and reading that function's
attributes, rather than by grepping text. The original "four" was wrong in both
directions: it listed `tests/integration.rs` sites that are already compliant,
and missed two live ones in `src/`.

| Site | Test | Sleep |
|---|---|---|
| `src/session_store_tests.rs:254` | `list_returns_newest_first` | 10 ms real clock |
| `src/daemon/mod.rs:1151` | `liveness_is_unresponsive_when_peer_never_replies` | **3 s real clock** |
| `src/daemon/context/background.rs:450` | `spawn_is_noop_when_in_flight` | 10 ms virtual clock |

Five further sleeps (`tests/integration.rs:615`/`:1746`/`:1770`/`:1778`,
`tests/isolation.rs:591`) are all inside `#[ignore]`d tests that already carry
the justification comment §3.3 requires — compliant, and out of scope.

**No `TODO` / `FIXME` / `XXX` anywhere in `src/`** — checked, clean.

### Carried in from M6

- **The E2E-capture fold works and is now contract.** Give phase specs their
  end-to-end commands as runnable blocks with `exit=` markers, never as prose,
  and never let the server-authored `(complete)` entry stand as the evidence.
  Ten of M6's fourteen bounces were this one requirement.
- **`test_home_guard()` now restores `HOME` on drop.** New tests do not need
  manual restore blocks; taking the guard is sufficient.
- **On a `NoProgressStall`, run the gates against the partial tree before
  choosing a lever** (`WORKFLOW.md`). Four occurrences in M6; the partial work was
  correct every time.

### Runtime-tree defects found mid-milestone (2026-08-01)

Two defects in the runtime-layout tree surfaced while drafting phases 06 and 07.
Both are in scope for M7's *spirit* — this milestone exists partly to stop the
tree from lying — but neither belongs to a drafted phase, and both were
explicitly held out of 06 and 07 so those phases stayed focused. **They need a
phase of their own before the milestone closes.**

1. **`memory/incident/` does not exist; the real directory is
   `memory/incidents/`.** `MemoryCategory::Incident.dir_name()` returns
   `"incidents"` (`src/memory.rs:18`) while `canonical_name()` returns
   `"incident"` (`:31`). `RUNTIME_TREE` and the shipped asset document the
   singular. Verified empirically: after `daemoneye setup`, `memory/` contains
   only `knowledge` and `session`, and `incidents/` is created lazily on first
   write — the singular form is never created by anything. So an agent reading
   this knowledge memory is told a path that cannot exist. This is the exact
   defect class M6 item 5 was about, and phase 05's gates did not catch it
   because `POLICY_TABLE` carries only `memory`, not the per-category
   subdirectories.
2. **`agents/*/memory/` is in neither `POLICY_TABLE` nor `RUNTIME_TREE`.**
   `memory_dir_for_namespace()` (`src/memory.rs:240`) creates
   `agents/<ns>/memory/<category>/` for every non-global namespace, and no table
   lists it.

The two share a fix shape — add the missing entries, correct the singular, and
consider whether `POLICY_TABLE` should carry the per-category paths so the
cross-check test can see them at all. Doing it in one phase also means one asset
regeneration instead of two.

**Drafted as phase 10 (2026-08-01), and it grew on contact.** Tracing the
singular through the tree turned up a **third** site that is a live bug, not doc
drift: `stamp_artifact_origin` (`src/session_store.rs:374`) builds
`memory/incident/<name>.md`, a path that never exists, so **an incident memory
created inside a named session never gets its `session_origin` stamped.** No
test covered it — the existing backfill test uses a knowledge memory, which
works.

The phase also answers the "should `POLICY_TABLE` carry the per-category paths"
question above with **yes**, and that is its centre of gravity rather than the
spelling fix. `is_covered()` treats a directory as covered if it is a
*subdirectory of* a table entry, so `memory/incidents` on disk was "covered" by
the bare `memory` entry without ever being named — and phase 05's tree
cross-check had nothing per-category to compare against. Adding the six entries
is what makes the gate able to catch this class at all, and the phase requires a
quoted red run proving it does.

Two `CLAUDE.md` rows were folded in for the same reason (they assert machinery
`src/memory.rs` does not have — verified claim by claim), on the grounds that
one doc-truth phase beats three.
