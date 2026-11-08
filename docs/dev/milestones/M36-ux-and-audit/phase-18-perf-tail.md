# Phase 18 — perf-tail

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-08
**Depends on:** phase 17c
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=perf, size=m

## Goal

This phase closes six small audit findings (`FINDINGS.md`): P2, P3, B17,
P4, P8 and L2. Each one is a hot path doing avoidable work, or a doc
sentence that disagrees with the code. No user-visible behaviour changes,
except that the daemon stops stalling workers on them.

## Behaviour

1. **P2: `read_shell_log` walks once, off the runtime**
   (`src/daemon/server/shells.rs`).
   - The handler `handle_shell_log_read` runs `read_shell_log` through
     `tokio::task::spawn_blocking`. Today the file read and the walk block
     a tokio worker.
   - The walk is linear in lines plus commands. Today every cast line
     loops over every command record, which is lines × commands. The meta
     records are ordered by byte position, so a cursor over them can
     advance with the line offset.
   - The result is unchanged: the same events, the same command starts
     (typed input or first output byte), and the same `cap` trimming.
2. **P3 and P4: one index connection per index path**
   (`src/memory/index/`).
   - `open_index` and its callers (`index_event`, the `search.rs` queries
     through `open_and_reconcile_if_empty`, `reconcile.rs`) share one
     process-wide connection for the current `memory_index_path()`.
     `ensure_schema` runs once for that path, not on every open. Today
     every logged event and every search opens a new connection and runs
     nine `CREATE … IF NOT EXISTS` plus a `PRAGMA user_version` write.
   - The cache is **keyed by path**. When `memory_index_path()` returns a
     different path, the next call opens that path's database. Tests rely
     on this, because each test points `HOME` at its own tempdir. A
     single slot is enough, and it never grows.
   - `open_and_reconcile_if_empty` reconciles an empty corpus **once per
     path**. A corpus that is still empty after its reconcile, such as
     `epochs` for a user who has never compacted, is not rebuilt on every
     turn.
3. **B17: the event offset comes from the append**
   (`src/daemon/utils/event_log.rs`). `log_event` records the line's
   offset from the file handle it appended with: the position after the
   write, minus the line's length. Today it reads `metadata().len()`
   before the append, so two writers can record the same offset and one
   `events_map` row points at the wrong line.
4. **P8: `ip_limits` is pruned** (`src/webhook/server.rs`).
   `admit_request` drops entries from windows other than the current one,
   so the map holds at most one window's worth of source IPs. Admission
   behaviour is unchanged.
5. **L2: the CLAUDE.md sentence follows the code.** The `src/memory/index.rs`
   row says `build_match_expr()` "quotes each user term". The code splits
   on non-alphanumerics, lowercases, deduplicates, keeps at most 32 terms
   and joins them with `OR`, unquoted. That is safe because every token is
   alphanumeric. Rewrite the sentence to say that. Leave the code as it is.

## Constraints

- **The shared connection lives behind a `Mutex`.** `rusqlite::Connection`
  is not `Sync`. Hold the lock for one whole index operation, and never
  across an `.await`. All these functions are synchronous today, so keep
  them that way.
- **Index writes stay best-effort.** An index failure logs a warning and
  never fails the caller. The existing tests that make the index
  directory unwritable must still pass unchanged.
- **Gotcha: `log_event` asserts `HOLDS_HOME_GUARD` in tests**, and that
  flag is thread-local (`src/lib.rs`). A test that calls `log_event` from
  spawned threads must set the flag in each one, under the same
  `crate::test_home()` its parent holds.
- **Tests do not touch the real `~/.daemoneye`.** Every test holds
  `crate::test_home()`.
- Do not change `build_match_expr`, `build_phrase_expr`, the FTS5 schema,
  `SCHEMA_VERSION`, `LOG_READ_CAP` or the webhook's window constants.

## Spec

1. **Red tests first.** Run each one and paste the real output into an
   Update Log entry `(red runs)`. A test that names a new seam is red as a
   compile error. The concurrent-offset test fails by chance: run it up to
   five times, then paste its first failing run, or say all five passed.
   - `shell_log_read_matches_each_command_to_its_own_start_across_many_commands`
     in `src/daemon/server/shells_tests.rs`. This is a preservation test,
     green today: a recording with a few hundred commands, some typed and
     some not, maps each one to its own start.
   - `index_connection_is_opened_once_per_path`
   - `index_connection_follows_a_home_change`: an event logged under one
     test HOME is never found in another HOME's index.
   - `an_empty_corpus_is_reconciled_once_per_path`
   - `log_event_offsets_point_at_their_own_lines_under_concurrent_writers`:
     every `events_map` row's offset reads back the line of its own event.
   - `admit_request_drops_entries_from_past_windows`
2. **Implement** Behaviour 1–5, running `cargo build` after each step.
3. **Mutation check:** put the per-path reconcile guard back to
   reconciling every call, run
   `cargo test --lib an_empty_corpus_is_reconciled_once_per_path`, and
   confirm that it fails. Restore the guard and confirm that it passes.
   **Paste both real results** into an Update Log entry `(mutation)`.
4. **Findings:** add a `### Findings — <date>` entry of two or three
   lines, or `none`.
5. **End-to-end:** run § End-to-end verification once, and paste its
   output in an `(end-to-end verification)` entry.
6. **Commit** as
   `perf: one index connection per path, a single-pass shell log read off the runtime, append-derived event offsets and a pruned webhook limiter (M36 phase-18)`.

## Acceptance criteria

- [ ] All six named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `blocking_read=1` or more: `src/daemon/server/shells.rs` calls
      `spawn_blocking`. Today it is 0.
- [ ] `metadata_offset=0`: `src/daemon/utils/event_log.rs` no longer
      reads `metadata(&path)`. Today it is 1.
- [ ] `stale_doc=0`: CLAUDE.md no longer says "quotes each user term".
      Today it is 1.
- [ ] Existing tests pass unchanged, including `shell_log_read_*` and the
      unwritable-index tests.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in shell_log_read_matches_each_command_to_its_own_start_across_many_commands index_connection_is_opened_once_per_path index_connection_follows_a_home_change an_empty_corpus_is_reconciled_once_per_path log_event_offsets_point_at_their_own_lines_under_concurrent_writers admit_request_drops_entries_from_past_windows; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'spawn_blocking' src/daemon/server/shells.rs | sed 's/^/blocking_read=/'
grep -c 'metadata(&path)' src/daemon/utils/event_log.rs | sed 's/^/metadata_offset=/'
grep -c 'quotes each user term' CLAUDE.md | sed 's/^/stale_doc=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:**
  - `src/daemon/server/shells.rs` and `shells_tests.rs`;
  - `src/memory/index/` (`mod.rs`, `search.rs`, `reconcile.rs` and their
    tests);
  - `src/daemon/utils/event_log.rs` and its tests;
  - `src/webhook/server.rs`;
  - the `src/memory/index.rs` row of CLAUDE.md.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, add or raise its entry and give the reason in Findings. Do not split
  code to fit.
- **Process cleanup:** a test or probe that starts a process ends it by the
  pid it recorded (`$!`, a `Child`). Never find processes by name with
  `pgrep`, `pidof` or `ps`. The executor's bash tool refuses that.

## Out of scope

- Moving `log_event` itself off the async runtime (the rest of P3). It is
  synchronous and called from hundreds of sites.
- Reading memory files once per session instead of once per turn (the
  rest of P4).
- A size bound on the cast read itself. `LOG_READ_CAP` still trims after
  the read.
- The index's busy timeout, and `daemoneye reindex`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-08 14:11 (started)

Status flipped to `in-progress`; milestone README row matches. Executor: Claude
(Sonnet 4.5, headless). Starting with the red tests per the Spec.
### Update — 2026-11-08 14:11 (progress)

Read the tree: `shells.rs` + tests, `memory/index/{mod,search,reconcile}.rs`,
`event_log.rs` + tests, `webhook/server.rs`, the CLAUDE.md `index.rs` row and
`src/lib.rs`'s HOME-guard plumbing. `read_shell_log`'s walk is
lines × commands today; `open_index()` opens a fresh `rusqlite::Connection`
and runs `ensure_schema` at every call site (every event, every search, every
turn's prompt); `log_event` reads `metadata(&path)` before the append;
`admit_request` never prunes; CLAUDE.md line 114 says "quotes each user term".

Now writing the six named tests red-first (the P2 preservation test already
passes; the other five name new seams and are red as compile errors).

### Update — 2026-11-08 15:40 (escalation)

**Chosen lever:** resume
**Rationale:** The spec held. Behaviours 1–5 are committed and the full
suite is green at 2796, so the run hit the 512-turn cap with a bounded list
left. A fresh context seeded with the diff finishes it more cheaply than a
re-dispatch would re-derive it.

### Notes for executor — 2026-11-08

Green gates are **expected** here and are **not** evidence the phase is
done. Already done, do not redo: P2 (`ede006bb`), P3/P4/B17/P8/L2
(`e9d1d27b`), the three index tests in `perf_tests.rs`, the per-path
reconcile guard, and its mutation. The red and green output is saved in
`/tmp/mutation_red.txt` and `/tmp/mutation_green.txt`. What is left:

1. **Write `log_event_offsets_point_at_their_own_lines_under_concurrent_writers`.**
   It is missing from every commit. Several threads each log events; every
   `events_map` row's offset reads back the line of its own event
   (`read_line_at_offset`). Each spawned thread sets `HOLDS_HOME_GUARD`
   under the parent's `crate::test_home()`.
2. **Show it is real:** put the pre-append `metadata(&path).len()` offset
   back, run the test up to five times, paste the first failing run or say
   all five passed, then restore and paste the passing run.
3. **Lock sites:** every production `.lock()` added this phase uses
   `.unwrap_or_else(std::sync::PoisonError::into_inner)`. CLAUDE.md
   requires `.unwrap_or_log()` (`crate::util::UnpoisonExt`) at every lock
   site, because it logs the poison. Change them all.
4. **No `.expect()` in production:** `IndexConn`'s `Deref`/`DerefMut` and
   `open_index`'s `guard.take().expect(…)` panic on a broken invariant.
   Restructure them so they cannot panic (STANDARDS §2).
5. `cargo fmt --all`.
6. **Update Log:** a `(red runs)` entry (compile-error red for the seam
   tests is enough, plus the item-2 red), a `(mutation)` entry pasting the
   two `/tmp/mutation_*.txt` files, `### Findings`, and the
   `(end-to-end verification)` entry from § End-to-end verification.
7. **Commit** the rest as one `perf:` commit. The test count after it is
   **2797**: 2796 today plus the one new test.

### Update — 2026-11-08 15:49 (red runs)

Continuing the resumed run. Item 1–2 of the escalation notes:

```
$ for i in 1 2 3 4 5; do cargo test --lib log_event_offsets_point_at_their_own_lines_under_concurrent_writers | grep 'test result'; done
run1: test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.07s
run2: test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.09s
run3: test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.07s
run4: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.07s
run5: test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.09s
```
First failing run, over the mutated offset (pre-append `metadata(&path).len()`):

```
thread 'daemon::utils::event_log::tests::log_event_offsets_point_at_their_own_lines_under_concurrent_writers'
(305271) panicked at src/daemon/utils/event_log_tests.rs:790:5:
assertion `left == right` failed: every row's offset must be a distinct line start:
rows [0, 196, 385, 798, 995, 1185, 1389, 1600, 1791, 1996, 2194, 2598, ...] vs
starts [0, 196, 385, 588, 798, 995, 1185, 1389, 1600, ...]
  left: 46
 right: 48
```
Two lines lost a row to a stale pre-append offset; 4 of 5 runs failed. offset
restored to the append-derived value:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.07s
```

The five seam tests' red was compile-error red in the first half of the run
(d3f91df1 stubs failing on the named seams) and is recorded there; the
preservation test `shell_log_read_matches_each_command_to_its_own_start_across_many_commands`
passed on arrival.

### Update — 2026-11-08 15:49 (mutation)

Enforcing item 3 of the escalation notes: every lock site this phase added or
touched now uses `.unwrap_or_log()` (`crate::util::UnpoisonExt`) —
`src/memory/index/mod.rs` (slot, `Drop`, `reconciled_empty()`), and
`shell_life.rs`'s `open_shell_test_seam`. Item 4: the
`guard.take().expect(...)` in `open_index` is now a match that reopens on the
impossible empty slot rather than panicking; `IndexConn`'s derefs route an
empty slot (inert after `Drop` is the only taker) through a leaked fallback
connection instead of `.expect()`. `grep -c 'expect(' src/memory/index/mod.rs`
= 0 in production paths.

Per-corpus reconcile guard mutation (from the prior half of the run, recorded
in `/tmp/mutation_red.txt` / `/tmp/mutation_green.txt`):

```
test memory::index::perf_tests::an_empty_corpus_is_reconciled_once_per_path ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2716 filtered out; finished in 0.01s
```
Green again with the guard restored:

```
test memory::index::perf_tests::an_empty_corpus_is_reconciled_once_per_path ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2716 filtered out; finished in 0.01s
```

### Update — 2026-11-08 15:49 (end-to-end verification)

```
shell_log_read_matches_each_command_to_its_own_start_across_many_commands passed=1 failed=0
index_connection_is_opened_once_per_path passed=1 failed=0
index_connection_follows_a_home_change passed=1 failed=0
an_empty_corpus_is_reconciled_once_per_path passed=1 failed=0
log_event_offsets_point_at_their_own_lines_under_concurrent_writers passed=1 failed=0
admit_request_drops_entries_from_past_windows passed=1 failed=0
blocking_read=1
metadata_offset=0
stale_doc=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.95s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.00s
fmt_exit=0
passed=2797 failed=0
```

### Findings — 2026-11-08

- Two new files needed no allowlist beyond default caps; hygiene stayed at 16
  passes with no adjustment.
- The B17 race needed the test to assert *distinctness of offsets against the
  file's actual line starts* (`dedup` against `line_starts`) — a
  `read_line_at_offset`-only check was too coarse, because a stale offset can
  still read back an event's line when lines are same-length. That stronger
  assertion is what makes the mutation fail 4 of 5 runs.
