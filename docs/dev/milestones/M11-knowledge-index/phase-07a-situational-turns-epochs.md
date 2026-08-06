# Phase 07a: situational injections — turns and epochs in the dynamic block

**Milestone:** M11 — Unified Knowledge Index
**Status:** done
**Depends on:** phase-06 (done)
**Estimated diff:** ~365 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Every corpus in the index is now populated and searchable, but the per-turn
prompt still only reads `memories`. This phase adds a small, budget-capped
`[SITUATIONAL]` block that surfaces at most one past turn and one past epoch
matching the user's current turn — "this error appeared at turn 214 of session
X" — from **other** sessions. It also collapses the two duplicate
`read_line_at_offset` helpers into one before adding a third caller.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Read surfaces", item 4, first bullet —
  the one bullet this phase implements. The other two bullets (ghost cold-start
  seeding, incident `relates_to` auto-linking) are **phase 07b**, not this phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Line numbers are current as of drafting (2026-08-06). Re-derive with
`grep -rn "read_line_at_offset" --include=*.rs src`.

**`read_line_at_offset` exists twice, and this phase would make it three.** The
two copies are semantically identical — they differ only in import style:

`src/search.rs:530`
```rust
fn read_line_at_offset(path: &std::path::Path, offset: u64) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let reader = std::io::BufReader::new(file);
    let mut current_offset: u64 = 0;
    for line in std::io::BufRead::lines(reader) {
        let Ok(l) = line else {
            break;
        };
        if current_offset == offset {
            return l;
        }
        current_offset += l.len() as u64 + 1; // +1 for newline
    }
    String::new()
}
```

`src/daemon/context/recall.rs:186` is the same function with `File::open`,
`BufReader::new` and `reader.lines()` written against the module's `use`
statements.

There are exactly **three** call sites: `src/search.rs:425`, `src/search.rs:464`,
`src/daemon/context/recall.rs:157`.

**Deleting the `recall.rs` copy does not orphan its imports.** `recall.rs:7-8`
imports `File`, `BufRead` and `BufReader`; counted across the file there are 3
uses of `File::`, 4 of `BufReader` and 4 of `.lines()`, so removing one use of
each leaves 2/3/3 live. **Do not delete those `use` lines.** (`search.rs` writes
fully-qualified paths, so it has no import to consider.)

**The search API this phase consumes already exists** (`src/memory/index.rs`):

```rust
pub struct TurnHit {
    pub session_id: String,
    pub turn: i64,
    pub offset: i64,
    pub score: f64,
}
pub fn search_turns(query: &str, limit: usize, session_id: Option<&str>) -> Vec<TurnHit>

pub struct EpochHit {
    pub session_id: String,
    pub seq: i64,
    #[allow(dead_code)]
    pub kind: String,
    pub body: String,
    #[allow(dead_code)]
    pub score: f64,
}
pub fn search_epochs(query: &str, limit: usize) -> Vec<EpochHit>
```

Both are best-effort: any failure logs a warning and returns an empty `Vec`.
`epochs` is a stored-content table, so `EpochHit.body` is the text itself — no
file round-trip. `turns` is contentless, so a `TurnHit` must be resolved by
reading its archive line at `offset`.

**`PromptCtx` has no `session_id`, and the dynamic-memory call site passes
`None`** (`src/daemon/prompt.rs:224-231`). There is exactly **one** `PromptCtx`
construction site — `src/daemon/server/ask.rs:645` — and `session_id:
Option<String>` is already in scope there (it is used at `ask.rs:608` as
`session_id.as_deref()`). So adding the field is a one-site change.

**How a hit is rendered from an archive line today** (`src/search.rs:466-495`),
which is the shape to reuse — note the `tool_results` concatenation, without
which a match that exists only in a tool result renders as an empty excerpt:

```rust
let msg: Message = match serde_json::from_str(line.trim_end()) { ... };

let mut matched_line = msg.content.clone();
if let Some(tool_results) = &msg.tool_results {
    for tr in tool_results {
        if !matched_line.is_empty() {
            matched_line.push('\n');
        }
        matched_line.push_str(&tr.content);
    }
}
```

**The noise risk, measured not assumed.** `build_match_expr`
(`src/memory/index.rs:123-141`) splits the query on non-alphanumeric characters,
lowercases, de-duplicates, caps at 32 terms, and joins them with **`OR`**. A
whole user turn therefore becomes a disjunction of all its words, and *something*
will match almost any non-trivial corpus. BM25 ranking decides which hit leads,
but it cannot make a junk corpus produce a relevant hit. Two consequences this
phase pins: a minimum-signal guard on the query (task 3), and a hard cap of two
rendered lines.

## Spec

### Task 1 — Extract `read_line_at_offset` to one home

Add to `src/memory/index.rs`, public:

```rust
/// Read the single line beginning at `offset` bytes into `path`.
///
/// The inverse of the byte offsets this module stores in `turns_map` and
/// `events_map`, which is why it lives here: the append-only invariant that
/// makes those offsets stable is documented in this module. Returns an empty
/// string when the file is missing, unreadable, or has no line at that offset —
/// callers treat that as "no excerpt", never as an error.
pub fn read_line_at_offset(path: &std::path::Path, offset: u64) -> String
```

Use the `src/search.rs:530` body verbatim (the fully-qualified form quoted in
§ Current state). Then:

- Delete the copy at `src/search.rs:530` and point its two call sites
  (`:425`, `:464`) at `crate::memory::index::read_line_at_offset`.
- Delete the copy at `src/daemon/context/recall.rs:186` and point its one call
  site (`:157`) at the same path. **Leave `recall.rs:7-8`'s `use` lines alone** —
  § Current state gives the surviving-use counts.

Run `cargo build` after this task, before starting task 2. Behavior must not
change: this is a pure de-duplication.

### Task 2 — New module `src/daemon/situational.rs`

Register it in `src/daemon/mod.rs` with the other `pub mod` lines (alphabetical
placement: between `session` and `stats`, matching the existing ordering).

The module's public surface is one function:

```rust
/// Assemble the `[SITUATIONAL]` block: at most one past turn and one past
/// epoch from **other** sessions matching the current user turn.
///
/// Returns `None` when the query carries too little signal, when nothing
/// matches, or when every hit belongs to `current_session`.
pub fn assemble_situational_block(
    user_turn: &str,
    current_session: Option<&str>,
) -> Option<String>
```

Constants, all module-private:

```rust
/// Minimum number of distinct terms of >= MIN_TERM_LEN characters before the
/// block is assembled at all. `build_match_expr` ORs every term, so a short or
/// filler turn ("yes", "run it") would otherwise match arbitrary history.
const MIN_QUERY_TERMS: usize = 3;
const MIN_TERM_LEN: usize = 4;
/// Per-line excerpt cap, in characters (not bytes — excerpts may be UTF-8).
const EXCERPT_CHARS: usize = 200;
```

Behavior, in order:

1. **Signal guard.** Split `user_turn` on non-alphanumeric characters, lowercase,
   de-duplicate, and count the terms with at least `MIN_TERM_LEN` characters. If
   that count is `< MIN_QUERY_TERMS`, return `None` immediately — do not touch
   the index.
2. **Turns.** Call `search_turns(user_turn, 8, None)` — `None` because the point
   is cross-session recall. Walk the hits in rank order and take the **first**
   whose `session_id != current_session`. Resolve it:
   `crate::daemon::session::archive_file(&hit.session_id)`, then
   `index::read_line_at_offset`, then deserialize to
   `crate::ai::types::Message` and build the excerpt text with the
   `content` + `tool_results` concatenation quoted in § Current state. Skip a hit
   whose line is empty, fails to deserialize, or renders to an empty excerpt, and
   try the next one.
3. **Epochs.** Call `search_epochs(user_turn, 8)` and take the first hit whose
   `session_id != current_session` and whose `body` is non-empty.
4. **Render.** Each excerpt is masked with `crate::ai::filter::mask_sensitive`,
   then flattened to a single line (replace every `\n` and `\r` with a space,
   collapse runs of whitespace), then truncated to `EXCERPT_CHARS` **characters**
   with a trailing `…` when truncated. Index in char space, never byte offsets —
   `src/daemon/context/recall.rs:252-262` is the existing example of that rule.
5. If both lookups came up empty, return `None`. Otherwise return the block:

```
[SITUATIONAL] Possibly-related history from other sessions
- past turn — session <session_id>, turn <n>: <excerpt>
- past epoch — session <session_id>, epoch <seq> (<kind>): <excerpt>
```

with the header line always present when the block exists and each `- ` line
present only when that lookup produced a hit.

Using `EpochHit.kind` makes that field live: remove the `#[allow(dead_code)]`
above `pub kind: String` in `src/memory/index.rs`. Leave the one above `score`
alone — this phase does not read it.

**Masking is not optional.** Archive content is upstream-masked at production,
but epoch bodies reach this path from the index and the recall surface masks on
read as well; match that.

### Task 3 — Thread `session_id` through and wire the block in

1. `src/daemon/prompt.rs`: add to `PromptCtx` (after `memory_namespaces`, before
   `tool_policy`):

   ```rust
   /// Current session id, used to exclude this session's own history from the
   /// situational block. `None` for callers with no session.
   pub session_id: Option<&'a str>,
   ```

2. `src/daemon/server/ask.rs:645`: add `session_id: session_id.as_deref(),` to
   the single `PromptCtx` literal.

3. `src/daemon/prompt.rs`, immediately after the existing `dynamic_memory`
   binding (currently `:224-232`): add

   ```rust
   let situational = crate::daemon::situational::assemble_situational_block(
       ctx.safe_query,
       ctx.session_id,
   )
   .map(|s| format!("{}\n\n", s))
   .unwrap_or_default();
   ```

   and include `situational` in the assembled prompt **directly after**
   `dynamic_memory` in both the first-turn and subsequent-turn branches, using
   the same interpolation style as the surrounding blocks. The exact template
   position is yours; what is pinned is that it follows the dynamic memory block
   and appears in both branches.

   While you are here, the `None` passed as `session_id` to
   `assemble_turn_relevant_memory` (`:225`, with its "not available in
   PromptCtx" comment) can now be `ctx.session_id` — make that change and delete
   the stale comment.

### Task 4 — Tests

Add an inline `#[cfg(test)] mod tests` at the bottom of
`src/daemon/situational.rs`. Every test that touches the filesystem takes the
`HOME` guard, and **the guard must be bound in the test body and live for the
whole test** — if you write a setup helper, it must *return* the guard:

```rust
fn setup() -> (crate::TestHomeGuard, tempfile::TempDir) {
    let guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    (guard, tmp)
}
```

Binding it as `let _guard = crate::test_home_guard();` inside a helper that
returns only the `TempDir` drops the guard when the helper returns, releasing the
process-global lock — the tests then race over `HOME` and clobber each other's
fixtures. `src/daemon/memory_prompt.rs:294-299` is the corrected form.

To seed a turn: write a `Message` as one JSON line into
`crate::daemon::session::archive_file("<id>")`, then
`index::index_turn(session_id, turn, offset, body)`. To seed an epoch:
`index::index_epoch(session_id, seq, kind, body)`. Test names and behaviors are
pinned in § Test plan.

## Acceptance criteria

- [ ] Exactly one definition of `read_line_at_offset` exists in the tree:

      ```sh
      grep -rn "fn read_line_at_offset" --include=*.rs src | wc -l
      ```

      prints `1`, and the surviving one is in `src/memory/index.rs`.
- [ ] Neither former home still defines it, and the new caller exists:

      ```sh
      grep -c "fn read_line_at_offset" src/search.rs src/daemon/context/recall.rs
      grep -c "read_line_at_offset" src/daemon/situational.rs
      ```

      must print `0` for both files, then `1`. (Do **not** pin the tree-wide
      total: it is `5` before this phase and `5` after — two definitions plus
      three call sites becomes one definition plus four — so that number is
      satisfied without doing any of the work.)
- [ ] `grep -n "session_id" src/daemon/prompt.rs` shows the new `PromptCtx`
      field and both uses, and no `None, // session_id not available` comment
      survives.
- [ ] `cargo test --lib daemon::situational` passes and reports **more than 0**
      tests — the module does not exist today, so a filter matching nothing
      would also "pass".
- [ ] `cargo test --lib` reports **exactly 1136** passed, 0 failed — 1128 at
      drafting, plus the seven tests of the first dispatch, plus the one test
      [bug-07a-1](bugs/bug-07a-1.md) requires. Above 1136 means tests were added
      that nobody asked for; below means one was lost or the bug fix's test is
      missing. No existing test may be removed.
- [ ] **[bug-07a-1](bugs/bug-07a-1.md) is fixed.** These four currently **fail**
      and are the outstanding work of this phase:
      - [ ] `grep -n "find_map" src/daemon/situational.rs` finds the new call.
      - [ ] `grep -n "and_then(resolve_turn_hit)" src/daemon/situational.rs`
            finds nothing (exit 1).
      - [ ] `cargo test --lib daemon::situational` reports **8 passed**, 0
            failed.
      - [ ] Test `unresolvable_turn_hit_falls_through_to_the_next` exists and
            passes.
- [ ] `cargo fmt --all`, `cargo build`, and
      `cargo clippy --all-targets --all-features -- -D warnings` are clean.
- [ ] The mutation pair in § End-to-end verification is captured, and the
      restore is proven by the grep that follows it.

## Test plan

All in `src/daemon/situational.rs`'s `mod tests` unless stated.

- `short_turn_injects_nothing` — call with `"run it"` (two terms, both under
  `MIN_TERM_LEN`) against a corpus that **does** contain a matching turn, and
  assert `None`. The negative case: the guard must fire on the query's shape,
  not on an empty corpus, so the fixture has to be non-empty for this test to
  mean anything.
- `matching_turn_from_another_session_is_injected` — seed a turn in session
  `other` whose body contains a distinctive multi-word phrase; call with that
  phrase and `current_session = Some("current")`. Assert the block is `Some`,
  contains `session other`, contains the turn number, and contains text from the
  seeded body.
- `current_session_turn_is_excluded_and_the_guard_is_not_vacuous` — seed the
  *same* distinctive phrase into **two** sessions, `current` and `other`. Call
  with `current_session = Some("current")`. Assert the rendered block does
  **not** name session `current` **and does** name session `other`. The second
  assertion is the point: without it the test passes whenever nothing matched at
  all, for any reason.
- `only_current_session_matches_yields_none` — seed the phrase into `current`
  only; assert `None`. Distinct from the test above: this pins that exclusion
  produces no block rather than an empty-bodied one.
- `epoch_hit_renders_with_its_kind` — seed an epoch via `index_epoch` with a
  distinctive body and a known `kind`; assert the block's epoch line carries the
  session id, the seq, and the kind string.
- `excerpt_is_single_line_and_char_truncated` — seed a turn whose body contains
  embedded newlines and is far longer than `EXCERPT_CHARS`, including multi-byte
  characters. Assert the rendered block contains no `\n` inside the excerpt
  (each hit occupies exactly one `- ` line), that the excerpt ends with `…`, and
  that the function does not panic. A byte-slice truncation would panic on a
  multi-byte boundary — that is what the multi-byte content is there for.
- `tool_result_only_match_still_renders` — seed a turn whose `content` is empty
  and whose distinctive phrase appears **only** in a `tool_results` body. Assert
  the excerpt is non-empty and contains the phrase. Without the
  `content` + `tool_results` concatenation this renders empty and the hit is
  skipped.
- Existing `recall_context` and `search_repository` tests must keep passing
  unchanged after task 1 — that is the de-duplication's regression check. Do not
  modify them.

## End-to-end verification

The situational block reaches a real prompt only through a live LLM turn, so the
evidence here is structural plus a mutation proving the exclusion rule is
load-bearing. Run this block verbatim and paste the resulting file's contents
into an Update Log entry titled `### Update — <date> (end-to-end verification)`.
**The server-authored `(complete)` entry does not satisfy this**, however
accurately its summary describes what was run.

```sh
{
  echo "== exactly one definition, and it is in index.rs =="
  grep -rn "fn read_line_at_offset" --include=*.rs src; echo "exit=$?"
  echo "== former homes define it no longer (expect 0 and 0) =="
  grep -c "fn read_line_at_offset" src/search.rs src/daemon/context/recall.rs; echo "exit=$?"
  echo "== new caller exists (expect 1) =="
  grep -c "read_line_at_offset" src/daemon/situational.rs; echo "exit=$?"
  echo "== stale session_id comment must be gone (expect no output, exit=1) =="
  grep -rn "session_id not available" --include=*.rs src; echo "exit=$?"
  echo "== module registered =="
  grep -n "pub mod situational" src/daemon/mod.rs; echo "exit=$?"
  echo "== baseline: module tests green =="
  cargo test --lib daemon::situational 2>&1 | tail -5; echo "exit=$?"
} > /tmp/p07a-e2e.txt 2>&1
cat /tmp/p07a-e2e.txt
```

Then the mutation, appending to the same file:

```sh
# 1. MUTATE: delete the current-session exclusion. In assemble_situational_block,
#    change the turns filter so it accepts a hit regardless of session — i.e.
#    drop the `hit.session_id != current_session` condition.
{
  echo "== MUTATED: current-session exclusion removed =="
  cargo test --lib daemon::situational 2>&1 | tail -20; echo "exit=$?"
} >> /tmp/p07a-e2e.txt 2>&1

# 2. RESTORE the condition.
{
  echo "== RESTORED =="
  cargo test --lib daemon::situational 2>&1 | tail -5; echo "exit=$?"
  echo "== restore proof: the exclusion must be present =="
  grep -n "current_session" src/daemon/situational.rs; echo "exit=$?"
} >> /tmp/p07a-e2e.txt 2>&1
cat /tmp/p07a-e2e.txt
```

The mutated run **must show at least one failing test**, and you must name in
your Update Log which tests failed. A mutation that leaves every test green
means the exclusion is untested and the phase is not done.

**The restore is mandatory and is checked at review by grepping the shipped
source.** Phases in this milestone have shipped mutations that were never undone.

## Authorizations

- [ ] May add the `session_id` field to `PromptCtx` (`src/daemon/prompt.rs`) and
      update its single construction site (`src/daemon/server/ask.rs:645`).
- [ ] May remove the `#[allow(dead_code)]` above `EpochHit::kind`
      (`src/memory/index.rs`), which this phase makes live.
- [ ] May add `pub fn read_line_at_offset` to `src/memory/index.rs` and delete
      the two private copies.

No new dependencies. No `docs/architecture.md` changes.

## Out of scope

- **Ghost cold-start seeding and incident `relates_to` auto-linking.** Both are
  phase 07b. Do not touch `src/daemon/ghost.rs` or
  `src/daemon/executor/knowledge/memory.rs`.
- **A relevance threshold on BM25 scores.** The signal guard is on the *query*
  (`MIN_QUERY_TERMS`), not on hit scores. Scores are corpus-relative, so an
  absolute cutoff is not meaningful — do not invent one.
- **Changing `search_turns` / `search_epochs` signatures or SQL.** Consume them
  as they are; filter in the caller.
- **Making the block configurable.** No new `Config` fields; the three
  constants stay module-private literals.
- **More than two lines.** At most one turn and one epoch, always.
- **`recall_context` and `search_repository` behavior.** Task 1 is a pure
  de-duplication; if any of their tests change behavior, you have broken
  something.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-06 (escalation, round 3)

**Chosen lever:** session takeover
**Rationale:** three rounds, three failures, and every one traced to a defect in
my spec rather than the executor's capability — the decision table's takeover
trigger (repeated same-class failure after refinement), reached the hard way.

- **Round 1** shipped correct code with one spec deviation (`bug-07a-1`).
- **Round 2** returned `complete` with an empty diff because the phase doc's
  acceptance criteria were stale and still passed; the executor's report was
  honest against the inputs it had.
- **Round 3** hard-failed on `NoProgressStall` after ~45 consecutive runs of one
  test. The executor had added the test and was trying to satisfy the mutation
  requirement I imposed — but the test could not fail, because the fixture recipe
  in `bug-07a-1` was wrong about BM25 ranking. It was doing exactly the right
  thing against an impossible instruction.

**What I changed:** the one-line fix, and a rebuilt fixture for the new test
based on a measured BM25 ranking rather than an assumed one. Both corrections are
recorded in [bug-07a-1](bugs/bug-07a-1.md) § Resolution.

### Update — 2026-08-06 (end-to-end verification)

Captured mechanically to `/tmp/p07a-e2e.txt`, pasted verbatim:

```
== exactly one definition, and it is in index.rs ==
src/memory/index.rs:157:pub fn read_line_at_offset(path: &std::path::Path, offset: u64) -> String {
exit=0
== former homes define it no longer (expect 0 and 0) ==
src/search.rs:0
src/daemon/context/recall.rs:0
exit=1
== new caller exists (expect 1) ==
1
exit=0
== stale session_id comment must be gone (expect no output, exit=1) ==
exit=1
== module registered ==
43:pub mod situational;
exit=0
== bug-07a-1 fix present ==
46:        .find_map(resolve_turn_hit);
exit=0
exit=1
== baseline: module tests green ==
test daemon::situational::tests::tool_result_only_match_still_renders ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1128 filtered out; finished in 0.04s

exit=0
== MUTATED: fix reverted to .find(...).and_then(resolve_turn_hit) ==

---- daemon::situational::tests::unresolvable_turn_hit_falls_through_to_the_next stdout ----

thread 'daemon::situational::tests::unresolvable_turn_hit_falls_through_to_the_next' (3753441) panicked at src/daemon/situational.rs:440:28:
should fall through to the resolvable turn
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    daemon::situational::tests::unresolvable_turn_hit_falls_through_to_the_next

test result: FAILED. 7 passed; 1 failed; 0 ignored; 0 measured; 1128 filtered out; finished in 0.04s

error: test failed, to rerun pass `--lib`
exit=0
== RESTORED ==
test daemon::situational::tests::tool_result_only_match_still_renders ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1128 filtered out; finished in 0.04s

exit=0
== restore proof: find_map present ==
46:        .find_map(resolve_turn_hit);
exit=0
== restore proof: and_then(resolve_turn_hit) absent (expect exit=1) ==
exit=1
== full suite ==
test result: ok. 1136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.46s
exit=0
```

**Mutation result:** reverting the fix to `.find(...).and_then(resolve_turn_hit)`
fails `unresolvable_turn_hit_falls_through_to_the_next` at its `expect`. Restored,
and the two greps above prove it against the shipped source — `find_map` present,
`and_then(resolve_turn_hit)` gone. Note this is the mutation the *previous*
version of the test could not produce; it is the whole reason the fixture was
rebuilt.

### Update — 2026-08-06 (complete, architect takeover)

**Summary:** all four tasks are implemented and `bug-07a-1` is fixed.
`read_line_at_offset` lives once, in `src/memory/index.rs`, with four callers;
`src/daemon/situational.rs` assembles a `[SITUATIONAL]` block of at most one
cross-session turn and one epoch, guarded by a minimum-signal query check,
masked, flattened to one line and char-truncated; `session_id` is threaded
through `PromptCtx` and the block is wired in after `dynamic_memory` in both
prompt branches. The executor wrote all of the production code, the module, and
all eight tests across three dispatches; the architect supplied the one-line
`filter`/`find_map` fix and rebuilt the new test's fixture.

**Acceptance criteria:** all met, including the six added at the round-2 bounce.

**Commands** (each run bare, as separate invocations):

```
$ cargo fmt --all
fmt exit=0

$ cargo build
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.01s
build exit=0

$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.90s
lint exit=0

$ cargo test
test result: ok. 1136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 6 passed; 0 failed (bug_tracker)
test result: ok. 4 passed; 0 failed (doc_truth)
test result: ok. 30 passed; 0 failed; 2 ignored (integration)
test result: ok. 9 passed; 0 failed; 1 ignored (isolation)
test exit=0
```

`1136` lib tests — the pinned finish condition exactly.

**Files changed:** `src/memory/index.rs`, `src/search.rs`,
`src/daemon/context/recall.rs`, `src/daemon/mod.rs`, `src/daemon/prompt.rs`,
`src/daemon/server/ask.rs`, `src/daemon/situational.rs` (new),
`tests/integration.rs`.

**New tests:** the seven pinned in § Test plan plus
`unresolvable_turn_hit_falls_through_to_the_next`, all in
`src/daemon/situational.rs`.

**Notes for review:** no `unwrap`/`expect`/`panic!`/`unsafe` in production code;
no `#[allow]`, `TODO`, `dbg!` or `println!` in the new module. `EpochHit::kind`'s
`#[allow(dead_code)]` removed (now live); `score`'s retained.

### Review verdict — 2026-08-06

- **Verdict:** escalated
- **Bounces:** 2 (bug: [bug-07a-1](bugs/bug-07a-1.md) — minor, verified fixed),
  plus one `NoProgressStall` hard_fail
- **Executor:** Qwen/Qwen3.6-27B-FP8 (all production code, the module, all eight
  tests); Claude (direct) for the takeover — the one-line fix and the test
  fixture
- **Scope deviations:** none against the final spec
- **Calibration:** three items, below — all three are architect-side

**1. A bug doc's prescribed fix is a system fact and must be executed before it
is written — `spec_bug`, now at its third occurrence in M11 and past the fold
threshold.** `bug-07a-1` asserted two things I had not run: that a bare function
item would not coerce after `.filter()` (clippy rejects the closure I mandated),
and that repeating a term makes a document rank higher (BM25 length
normalization does the opposite). The second one cost an entire dispatch and a
`NoProgressStall`. `NEXT.md` already tracks this rule at two occurrences
(`bug-02b-1` Finding 1, `bug-03a-1` Finding 2); this is the third. **It should
now be folded into `WORKFLOW.md` as a bug-report clause, with PE sign-off.**

**2. A bounce must update the phase doc's acceptance criteria, not only file a
bug doc.** Round 2's `complete`-with-empty-diff happened because the phase doc
still certified itself as finished while the bug doc sat beside it unread-for-
purpose. The executor evaluates the phase doc to decide it is done; criteria that
still pass after a bounce are worse than none. First occurrence, noted at the
round-2 bounce.

**3. The vacuous-guard rule needs to cover fixtures whose *ordering* premise is
unverified.** `NEXT.md` records "verify the guard is not vacuous belongs inside
every exclusion criterion" at two occurrences. This is a third instance of the
same family with a new mechanism: not an empty fixture, but a fixture whose
assumed rank order was wrong, so the code path under test was never entered. The
remedy that worked is cheap and general — **assert the precondition in the test**
(`hits.first().map(|h| h.turn) == Some(100)`), so a premise that goes stale fails
loudly instead of passing silently.

### Notes for executor — 2026-08-06 (round 3)

**STOP: green gates and a clean tree are EXPECTED here and are NOT evidence
this phase is done.** Round 2 reported `complete` with an empty diff on exactly
that reasoning. The gates pass because the *first* dispatch's code is correct and
committed. One defect remains, and no gate can see it.

**Why round 2 was not your fault.** The acceptance criteria above were stale —
they described the state before the review, so they all passed and the phase
read as finished. They have now been corrected: four of them **fail right now**,
deliberately. Check them, and you will see the outstanding work.

**Already correct and committed — do not re-derive, re-read, or modify:**
`src/memory/index.rs` (the `read_line_at_offset` extraction, the
`EpochHit::kind` allow removal), `src/search.rs`, `src/daemon/context/recall.rs`,
`src/daemon/mod.rs`, `src/daemon/prompt.rs`, `src/daemon/server/ask.rs`,
`tests/integration.rs`, the epoch lookup in `src/daemon/situational.rs`, and all
seven existing tests.

**There is exactly one edit and one new test, both specified in full in
[bug-07a-1](bugs/bug-07a-1.md). Read that file.** In one sentence: line 44 of
`src/daemon/situational.rs` uses `.find(exclusion).and_then(resolve_turn_hit)`,
so if the first non-current hit fails to resolve, the turn line is dropped and
the other seven candidates are never tried; replace it with `.filter(exclusion)`
+ `.find_map(|hit| resolve_turn_hit(hit))`, and add
`unresolvable_turn_hit_falls_through_to_the_next`.

**Finish condition: `cargo test --lib` must report 1136, not 1135.** A run that
leaves the count at 1135 has done nothing.

**The mutation belongs to this round.** The end-to-end entry from the first
dispatch does not carry forward. Capture a new one: revert the fix, show
`unresolvable_turn_hit_falls_through_to_the_next` failing, restore, show it
passing, and paste both greps as restore proof.

### Update — 2026-08-06 18:25 (progress)

Executor: rexyMCP. Implemented all four tasks: (1) extracted `read_line_at_offset` to `src/memory/index.rs` and updated all three call sites in `search.rs` and `recall.rs`; (2) created `src/daemon/situational.rs` with `assemble_situational_block` implementing signal guard, turns lookup, epochs lookup, and rendering with masking/truncation; (3) threaded `session_id` through `PromptCtx` and wired the situational block into the prompt after `dynamic_memory` in both branches; (4) added 7 tests covering all test plan scenarios. Updated `tests/integration.rs` for the new `PromptCtx` field. Removed `#[allow(dead_code)]` from `EpochHit::kind`.

### Update — 2026-08-06 18:25 (end-to-end verification)

```
== exactly one definition, and it is in index.rs ==
src/memory/index.rs:157:pub fn read_line_at_offset(path: &std::path::Path, offset: u64) -> String {
exit=0
== former homes define it no longer (expect 0 and 0) ==
src/search.rs:0
src/daemon/context/recall.rs:0
exit=1
== new caller exists (expect 1) ==
1
exit=0
== stale session_id comment must be gone (expect no output, exit=1) ==
exit=1
== module registered ==
43:pub mod situational;
exit=0
== baseline: module tests green ==
test daemon::situational::tests::short_turn_injects_nothing ... ok
test daemon::situational::tests::tool_result_only_match_still_renders ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1128 filtered out; finished in 0.04s

exit=0
== MUTATED: current-session exclusion removed ==

---- daemon::situational::tests::current_session_turn_is_excluded_and_the_guard_is_not_vacuous stdout ----

thread 'daemon::situational::tests::current_session_turn_is_excluded_and_the_guard_is_not_vacuous' (3638032) panicked at src/daemon/situational.rs:249:9:
block must not name the current session
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- daemon::situational::tests::only_current_session_matches_yields_none stdout ----

thread 'daemon::situational::tests::only_current_session_matches_yields_none' (3638036) panicked at src/daemon/situational.rs:279:9:
should return None when only current session matches
 (x2)
failures:
    daemon::situational::tests::current_session_turn_is_excluded_and_the_guard_is_not_vacuous
    daemon::situational::tests::only_current_session_matches_yields_none

test result: FAILED. 5 passed; 2 failed; 0 ignored; 0 measured; 1128 filtered out; finished in 0.04s

error: test failed, to rerun pass `--lib`
exit=0
== RESTORED ==
test daemon::situational::tests::short_turn_injects_nothing ... ok
test daemon::situational::tests::tool_result_only_match_still_renders ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1128 filtered out; finished in 0.04s

exit=0
== restore proof: the exclusion must be present ==
18:/// matches, or when every hit belongs to `current_session`.
21:    current_session: Option<&str>,
43:        .find(|hit| current_session.is_none_or(|cs| hit.session_id != cs))
57:        .find(|hit| current_session.is_none_or(|cs| hit.session_id != cs) && !hit.body.is_empty())
213:    fn current_session_turn_is_excluded_and_the_guard_is_not_vacuous() {
261:    fn only_current_session_matches_yields_none() {
exit=0
```

Mutation results: `current_session_turn_is_excluded_and_the_guard_is_not_vacuous` and `only_current_session_matches_yields_none` both failed when the exclusion was removed, proving the `current_session` filter is load-bearing.

### Update — ts=1786041933578 (complete, server-authored)

**Summary:** All four tasks are complete. Here's the summary:

**What was built:** A new `src/daemon/situational.rs` module with `assemble_situational_block` that surfaces at most one cross-session turn and one cross-session epoch matching the user's current turn, gated by a minimum-signal query guard (3+ terms of 4+ chars). The block is wired into the prompt after `dynamic_memory` in both the snapshot and non-snapshot branches.

**Deviations from spec:** None. Used `is_none_or` for the session exclusion filter (Rust 1.82+ idiom) instead of `map_or(true, ...)` — functionally identical.

**E2E verification:** All acceptance criteria verified — exactly one `read_line_at_offset` definition in `index.rs`, zero in former homes, new caller present, stale comment removed, module registered. Mutation test proved the current-session exclusion is load-bearing: removing it caused `current_session_turn_is_excluded_and_the_guard_is_not_vacuous` and `only_current_session_matches_yields_none` to fail. All 7 situational tests pass, total 1135 tests pass (7 new), build/lint/format clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s


TEST
beled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1135 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.73s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 4 tests
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.18s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M11-knowledge-index/README.md` — +1 -1
- `docs/dev/milestones/M11-knowledge-index/phase-07a-situational-turns-epochs.md` — +70 -1
- `src/daemon/context/recall.rs` — +1 -20
- `src/daemon/mod.rs` — +1 -0
- `src/daemon/prompt.rs` — +11 -3
- `src/daemon/server/ask.rs` — +1 -0
- `src/daemon/situational.rs` — +381 -0
- `src/memory/index.rs` — +25 -1
- `src/search.rs` — +2 -21
- `tests/integration.rs` — +1 -0

**Commit:** 8f7f7795d7bad849816f36c1b0aef0c476832359

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — ts=1786047639884 (complete, server-authored)

**Summary:** All four tasks are already complete from the prior run. The code is committed, all gates are green, and the working tree is clean.

**Summary:** Phase 07a is complete. Task 1 extracted `read_line_at_offset` to `src/memory/index.rs` (1 definition, 4 call sites). Task 2 created `src/daemon/situational.rs` with `assemble_situational_block` implementing the signal guard, turns lookup, epochs lookup, and rendering with masking/truncation. Task 3 threaded `session_id` through `PromptCtx` and wired the situational block into the prompt after `dynamic_memory` in both branches. Task 4 added 7 tests covering all test plan scenarios. All 1135 lib tests pass, build/lint/format are clean, and the mutation test proved the current-session exclusion is load-bearing. The prior run's commit is already on disk with a clean tree — no further work needed.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


TEST
beled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1135 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.59s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 4 tests
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


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

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

(none)

**Commit:** 2e84b42b2d6aa08f7eab296988293c34f95862ad

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
