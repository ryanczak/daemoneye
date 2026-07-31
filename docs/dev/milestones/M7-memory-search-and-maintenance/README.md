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
- [ ] **No `sleep` in a non-`#[ignore]`d test.** `STANDARDS.md` §3.3 already
      forbids it; four sites predate the rule.
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

Phase 01 is drafted; the rest are named only. Draft each with `/rexymcp:architect next` when its
predecessor is `done`. Ordering is deliberate — see Notes § "Why this order".

| #  | Phase | Status |
|----|-------|--------|
| 01 | [dependency-currency](phase-01-dependency-currency.md) — update every direct dependency to latest stable; hold `libc` back from its alpha | in-progress |
| 02 | bug-tracker-truth — a gate failing when a bug doc is `open` while its phase is `done`; close the five stale M2/M4 docs it catches | todo |
| 03 | test-sleep-removal — the four `sleep` sites `STANDARDS.md` §3.3 forbids | todo |
| 04 | path-audit-fenced-blocks — extend extraction to fenced code blocks (Part A of M6 item 5) | todo |
| 05 | generated-runtime-tree — derive `agent-runtime-layout.md`'s tree from the policy table (Part B of M6 item 5) | todo |
| 06 | fts5-index-schema — add `rusqlite` (`bundled`, authorized — see Notes); FTS5 schema, creation, and the `var/index/memory.db` lifecycle entry | todo |
| 07 | fts5-write-path — index maintained on add/update/delete, with reconciliation | todo |
| 08 | fts5-search — BM25 ranking wired into `ftsearch_memories()`, with the tag-miss/text-hit test | todo |
| 09 | index-doc-correction — `CLAUDE.md` and architecture.md § 5 describe the index as built | todo |

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
lifecycle policy table, and the runtime-layout tree. Landing the *generated* tree
first means phase 06 adds one inventory entry and the tree updates itself; landing
it after means hand-editing the tree again, which is the drift this milestone is
removing.

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

**Four `sleep` sites in tests.** `src/session_store_tests.rs:254`,
`tests/integration.rs:615`, `:1746`, `:1770`. Two are inside `#[ignore]`d tests
where §3.3 permits them with a justification comment; the other two need real
synchronisation or an ignore justification of their own.

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
