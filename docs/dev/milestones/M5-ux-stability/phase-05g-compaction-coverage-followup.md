# Phase 05g: Make the `compaction_in_flight` Assertions Real

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-04f (which found the gap) — `done`
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=test, size=m

## Goal

`run_compaction` clears `compaction_in_flight` on **four** distinct paths. Only
**one** of them is genuinely guarded by a test. The other three are covered by
assertions that are **tautological** — they assert a field is `false` when the
test fixture already defaults it to `false` and nothing ever set it `true`.

| Site | Path | Guarded today? |
|---|---|---|
| `background.rs:119` | "no viable cut" discard | **yes** |
| `background.rs:136` | idempotency-guard discard | no |
| `background.rs:232` | stale-branch discard ("a turn ran while we worked") | **no test reaches it at all** |
| `background.rs:240` | swap success | no — assertion is vacuous |

**Finish condition: each of the four clearing sites has a test that fails when
that specific line is deleted, demonstrated by mutation and quoted in the Update
Log.**

**This phase ADDS a test.** `cargo test` must report **916**, not 915 — the
inverse of every other phase in this milestone. See Acceptance criteria.

## Architecture references

Read before starting:

- `docs/design/context-management.md` § 3.1 — the archive invariant and the
  discard paths. The flag is what prevents two compactions racing on one session;
  a path that fails to clear it wedges compaction for that session **for the rest
  of its life**, silently.
- `CLAUDE.md` § "Important Invariants".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "compaction_in_flight = false" src/daemon/context/background.rs   # expect 5
grep -c "compaction_in_flight = true"  src/daemon/context/background.rs   # expect 2
grep -c "#\[tokio::test"               src/daemon/context/background.rs   # expect 6
cargo test 2>&1 | grep "^test result" | head -1                           # expect 915 passed
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

## Current state

### Why three assertions are vacuous — the fixture default

`make_test_entry()` (`background.rs:342`) builds a `SessionEntry` with:

```rust
            compaction_in_flight: false,
```

In production the flag is set `true` by `try_snapshot` (`background.rs:71`)
*before* `run_compaction` runs. **Three of the four tests hand-build their
`CompactionSnapshot` instead of calling `try_snapshot`**, so the entry's flag is
never set — and `assert!(!entry.compaction_in_flight)` passes no matter what the
code does.

| Test | Snapshot from | Flag ever `true`? |
|---|---|---|
| `background_swap_discards_on_new_turn` | `try_snapshot` (`:401`) | **yes** — its assertion is real |
| `background_swap_applies_when_unchanged` | hand-built (`:502`) | no — `:517` is tautological |
| `epoch_build_idempotent_after_discard` | hand-built (`:550`) | no — and it asserts nothing about the flag |
| `swap_discards_on_evicted_entry` | hand-built (`:595`) | n/a — entry is evicted |

The hand-built snapshots exist for a good reason: they let a test create a
`turn_count` / `msg_len` mismatch that `try_snapshot` cannot produce. **Keep
them.** The fix is to set the flag explicitly, mirroring what `try_snapshot`
would have done.

### ⭐ The worked example — the one test that already does it right

`background_swap_discards_on_new_turn` (`background.rs:377-427`):

```rust
        let snapshot = try_snapshot(&session_id, &sessions).unwrap();
        …
        let result = run_compaction(&snapshot, &sessions, &config).await;
        …
        with_sessions(&sessions, |store| {
            let entry = store.get(&session_id).unwrap();
            assert!(!entry.compaction_in_flight);
```

`try_snapshot` sets the flag `true` at `:71`, so asserting it is `false`
afterwards genuinely discriminates. **That is the property every other test
needs** — reached either through `try_snapshot` or by setting the flag directly.

### The four production clearing sites, verbatim

```rust
    // :117-121 — no viable cut
    let Some(tail_start) = tail_start else {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(&snapshot.session_id) {
                entry.compaction_in_flight = false;
            }
        });
        return Ok(());
    };

    // :134-138 — idempotency guard
    if let Some(last_prior) = prior.last() && dropped_last_turn > 0
        && last_prior.turn_end >= dropped_last_turn
    { … entry.compaction_in_flight = false; … return Ok(()); }

    // :229-234 — stale branch, inside the swap closure
        if entry.turn_count != snapshot.turn_count || entry.messages.len() != snapshot.msg_len {
            entry.compaction_in_flight = false;
            return None;
        }

    // :237-240 — swap success
        entry.messages = compacted.clone();
        entry.compaction_in_flight = false;
```

## Spec

### 1. Make `background_swap_applies_when_unchanged` guard the swap path (`:240`)

The entry is inserted with the fixture default, so `:517`'s assertion cannot
fail. Set the flag in the setup, immediately after `entry.turn_count = 5;`
(`background.rs:497`):

```rust
        let mut entry = make_test_entry();
        entry.messages = msgs.clone();
        entry.turn_count = 5;
        // `try_snapshot` sets this in production. The hand-built snapshot below
        // bypasses it, so set it here or the flag assertion is tautological.
        entry.compaction_in_flight = true;
```

**Change nothing else in this test.** Its existing assertion at `:517` becomes
real as a result.

### 2. Make `epoch_build_idempotent_after_discard` guard the idempotency discard (`:136`)

This test runs `run_compaction` twice over the same snapshot and asserts no
duplicate epoch. It restores the entry between runs so the staleness check
passes, isolating the idempotency guard — but it sets the flag to **`false`**,
which is the opposite of the in-flight state the second run models, and it
asserts nothing about the flag.

**Two edits.** In the restore block (`background.rs:565-570`), change the flag to
`true`:

```rust
        with_sessions(&sessions, |store| {
            let e = store.get_mut(&session_id).unwrap();
            e.messages = msgs.clone();
            e.turn_count = 5;
            // In-flight, as `try_snapshot` would have left it before the build.
            e.compaction_in_flight = true;
        });
```

Then **after** the second `run_compaction` and its existing
`assert_eq!(epochs::read_epochs(&session_id).len(), 1, …)`, add:

```rust
        with_sessions(&sessions, |store| {
            let e = store.get(&session_id).unwrap();
            assert!(
                !e.compaction_in_flight,
                "the idempotency-guard discard must clear the in-flight flag"
            );
        });
```

**Do not remove or weaken the existing epoch assertion** — it is what proves the
guard fired at all. The flag assertion is additional, not a replacement.

### 3. Add a test for the stale branch (`:232`) — no test reaches it today

The stale branch fires when a turn lands *while the build is running*, so the
entry's `turn_count` or `messages.len()` no longer matches the snapshot. Add this
test immediately after `background_swap_applies_when_unchanged`:

```rust
    #[tokio::test(start_paused = true)]
    async fn swap_discards_when_turn_ran_during_build() {
        let _home = TestHome::new();
        let sessions: SessionStore = SessionStore::new();
        let session_id = "swap-stale".to_string();

        let msgs = make_turn_msgs(32);
        let mut entry = make_test_entry();
        entry.messages = msgs.clone();
        entry.turn_count = 5;
        entry.compaction_in_flight = true;
        with_sessions(&sessions, |store| {
            store.insert(session_id.clone(), entry);
        });

        let snapshot = CompactionSnapshot {
            session_id: session_id.clone(),
            messages: msgs.clone(),
            turn_count: 5,
            msg_len: msgs.len(),
            // Huge scale forces a budget cut, so the build reaches the swap.
            token_scale: 1e9,
        };

        // A turn lands while the build is in flight — the snapshot is now stale.
        with_sessions(&sessions, |store| {
            store.get_mut(&session_id).unwrap().turn_count = 6;
        });

        let result = run_compaction(&snapshot, &sessions, &hermetic_config()).await;
        assert!(result.is_ok());

        with_sessions(&sessions, |store| {
            let e = store.get(&session_id).unwrap();
            assert!(
                !e.compaction_in_flight,
                "the stale-branch discard must clear the in-flight flag"
            );
            assert_eq!(
                e.messages.len(),
                32,
                "a stale discard must leave the history untouched"
            );
        });
    }
```

**The `token_scale: 1e9` matters.** It forces a budget cut so the build gets past
"no viable cut" and reaches the swap — without it the test would exit through
`:119` and prove nothing about `:232`. The second assertion (history untouched)
is what distinguishes a *discard* from a *swap*.

**If this test passes before you touch the production code, that is expected** —
it is a new test of existing correct behavior. Task 4 is what proves it
discriminates.

### 4. Prove all four sites by mutation — this is the phase's real deliverable

For **each** of the four production lines (`:119`, `:136`, `:232`, `:240` — line
numbers will shift as you edit; find them by the surrounding comments quoted in
§ "The four production clearing sites"):

1. Delete that single `entry.compaction_in_flight = false;` line.
2. Run `cargo test`.
3. Record **which test fails and its assertion message.**
4. Restore the line. Confirm `cargo test` passes again.

**Quote the fail/pass pair for all four in the Update Log**, in a table:

| Site | Mutation | Test that failed | Restored → passes |
|---|---|---|---|
| no viable cut | deleted `:119` clear | `<name>` — `<message>` | ✓ |
| … | … | … | … |

**If any site's mutation does not make a test fail, the phase is not done** —
report which one and stop. That is the whole point: a claim about what a test
guards is only admissible in this project when demonstrated by mutation.

Mutate **one line at a time** and restore before the next, so each failure
attributes to exactly one site. Leave the tree clean when you finish —
`git status` must show only your intended test edits.

## Acceptance criteria

- [ ] `cargo test` passes with **916** lib-unit tests — **not 915.** This phase
      adds exactly one test (task 3). **915 means task 3 was not added; 917+ means
      scope crept.**
- [ ] `grep -c "#\[tokio::test" src/daemon/context/background.rs` returns **7**
      (6 pre-existing + 1).
- [ ] `grep -c "compaction_in_flight = false" src/daemon/context/background.rs`
      returns **4** — down from 5, and **all four are the production clearing
      sites.** No test may set the flag `false` any more; that is the fixture
      trap being closed. Verify by reading that none of the four is in
      `mod tests`.
- [ ] `grep -c "compaction_in_flight = true" src/daemon/context/background.rs`
      returns **5** — the production set at `:71`, the pre-existing test setup,
      and the three added by tasks 1–3.
- [ ] The Update Log contains the **four-row mutation table** from task 4, each
      row naming a real test and its assertion message.
- [ ] `git diff --stat` shows **exactly one** `src/` file changed.
- [ ] `git diff src/daemon/context/background.rs` touches **no line outside
      `mod tests`** — the four production clearing sites must be byte-identical
      to their current state when you finish. **Mutations are temporary; restore
      every one.**
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `python3 /tmp/audit_closures.py` prints **nothing** — unchanged from 05f;
      this phase must not put blocking work into a `with_sessions` closure.

**Run every gate bare.** `cargo clippy … | tail -20` exits with `tail`'s status,
so a failing gate reads as passing.

## Test plan

This phase **is** the test plan. It adds one test and repairs two, and its
deliverable is the mutation evidence in task 4.

**State no coverage claim you have not mutated.** The four-row table is the only
admissible form of "this test guards that line" in this project. If a row cannot
be filled honestly, say so rather than filling it — a false coverage claim is
what created this phase in the first place.

Two reasoning checks to state in the Update Log alongside the table:

1. **Why the fixture default made three assertions vacuous.** In one sentence,
   name the mechanism (`make_test_entry` defaults the field to the asserted-for
   value) and say which of the three tests it affected.
2. **Why task 3's `token_scale: 1e9` is load-bearing.** Say which branch the test
   would exit through without it, and therefore which site it would fail to
   cover.

## End-to-end verification

The mutation table **is** the end-to-end verification. A green `cargo test` proves
nothing here — the tests were green before this phase and three of them were
decorative.

## Authorizations

- [x] May edit `src/daemon/context/background.rs` — **`mod tests` only** in the
      final state.
- [x] May **temporarily** delete production lines for the task-4 mutations,
      provided every one is restored and the final diff touches no production
      line.
- [x] May add exactly **one** test (task 3).
- [ ] **No** production behavior changes. The four clearing sites are correct as
      they stand; this phase proves that, it does not alter it.
- [ ] **No** changes to `make_test_entry`'s defaults. Changing the fixture would
      fix these three assertions and silently break others that rely on the
      default. Set the flag per-test instead.
- [ ] **No** other new tests, no deleted tests, no renamed tests.
- [ ] **No** import additions or deletions.
- [ ] **No** `#[allow(...)]` anywhere.

## Out of scope

- **The `Lock/HOME` test-hygiene thread.** `background.rs`'s tests use
  `TestHome::new()` and `TEST_HOME_LOCK`; that pattern is unchanged here.
- **Coverage for `try_snapshot`'s guard** (`:68`, the `is_ghost` /
  already-in-flight early return). `spawn_is_noop_when_in_flight` covers the
  in-flight half; the ghost half is untested and stays that way — note it if you
  like, but do not add a test.
- **Phase 06** (tmux-call-hardening) and **07** (stall-instrumentation).

### ⚠ Two traps from earlier phases in this milestone

1. **Do not leave a mutation in place.** Task 4 deletes production lines
   temporarily. The acceptance criteria require the final production code to be
   byte-identical — check `git diff` before you finish, not just `cargo test`.
2. **Do not insert an item between a doc comment and the item it documents.**
   Phase 05a cost two extra runs that way. Task 3 adds a test function with an
   attribute; read the lines directly above your insertion point and confirm you
   are not splitting an existing `///` block or `#[tokio::test]` from its `fn`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 03:38 (started)

**Executor:** Claude

**Work in progress:** Implementing tasks 1–4: making three vacuous `compaction_in_flight` assertions real, adding one new test for the stale-branch discard, and proving all four clearing sites by mutation.
