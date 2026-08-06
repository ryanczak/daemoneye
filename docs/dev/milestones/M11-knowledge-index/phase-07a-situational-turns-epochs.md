# Phase 07a: situational injections — turns and epochs in the dynamic block

**Milestone:** M11 — Unified Knowledge Index
**Status:** in-progress
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
- [ ] `cargo test --lib` reports **more than 1128** passed, 0 failed. 1128 is
      the baseline measured at drafting; no existing test may be removed.
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
