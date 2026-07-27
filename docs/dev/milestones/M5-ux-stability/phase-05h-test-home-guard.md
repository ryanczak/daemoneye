# Phase 05h: Stop One Failing Test From Failing Forty-Seven Others

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-05g (which measured the cascade) — `done`
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`TEST_HOME_LOCK` serialises every test that mutates `HOME`. **41 of its 62
acquisition sites use `.lock().unwrap()`**, so when a test panics while holding
the lock, the lock is poisoned and *every subsequent HOME-dependent test in the
same binary fails with it.*

Measured during 05g's review, by deleting one production line:

| Mutation | Target test holds `TestHome`? | Failures |
|---|---|---|
| `background.rs:119` | no | **1** |
| `background.rs:136` | yes | **48** |
| `background.rs:232` | yes | **48** |
| `background.rs:240` | yes | **48** |

One real failure, forty-seven fictional ones. Every future mutation, bisect, or
flaky-test hunt in this repo reads through that noise.

**Fix: one accessor that recovers from poison, and every site through it.**

**Finish condition: `TEST_HOME_LOCK.lock()` appears exactly once in the tree —
inside the accessor.**

## Architecture references

Read before starting:

- `CLAUDE.md` § "Important Invariants" — the `.unwrap_or_log()` convention exists
  because a poisoned lock should degrade, not abort. This phase applies the same
  reasoning to the test lock.
- `docs/dev/WORKFLOW.md` § "A phase that exhausts a trait's uses must say what
  happens to its import" — directly relevant; see task 4.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state. **Use this census, not `grep -c`** — nine sites
   split the acquisition across lines and a single-line grep cannot see them:

```bash
python3 - <<'PY'
import pathlib, re
tot = {"unwrap":0, "unwrap_or_log":0, "other":0}
for f in sorted(list(pathlib.Path("src").rglob("*.rs")) + list(pathlib.Path("tests").rglob("*.rs"))):
    src = f.read_text()
    for m in re.finditer(r'TEST_HOME_LOCK', src):
        seg = src[m.end():m.end()+140]
        if not seg.lstrip().startswith(('.lock', '\n')) and '.lock()' not in seg[:60]:
            continue
        k = "unwrap_or_log" if '.unwrap_or_log()' in seg[:120] else \
            "unwrap" if '.unwrap()' in seg[:120] else "other"
        tot[k] += 1
print(tot, "=> acquisitions:", sum(tot.values()))
PY
#   {'unwrap': 41, 'unwrap_or_log': 12, 'other': 9} => acquisitions: 62
cargo test 2>&1 | grep "^test result" | head -2   # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If the census differs, **stop and
report a blocker.**

## Current state

### Three idioms for the same intent — 21 sites already fix this, inconsistently

The codebase has independently discovered the fix twice and applied it two
different ways:

| Idiom | Sites | Poison behavior |
|---|---|---|
| `crate::TEST_HOME_LOCK.lock().unwrap()` | **41** | **panics — this is the cascade** |
| `crate::TEST_HOME_LOCK.lock().unwrap_or_log()` | 12 | recovers, logs an ERROR |
| `crate::TEST_HOME_LOCK`<br>`    .lock()`<br>`    .unwrap_or_else(std::sync::PoisonError::into_inner)` | 9 | recovers silently |

This is a convention no one can enforce — the same shape this milestone already
solved for the session store by routing every caller through one accessor.

### ⚠ The accessor must NOT be `#[cfg(test)]`

`src/lib.rs:27-32` says why, and it is load-bearing:

```rust
/// Single global lock used by tests that mutate the HOME environment variable.
/// All test modules that call `env::set_var("HOME", ...)` must hold this lock.
///
/// This is unconditionally `pub` so integration tests (which are a separate
/// crate and do not get `#[cfg(test)]` items from the library) can access it.
pub static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

**`tests/integration.rs` holds 11 of the 62 sites.** A `#[cfg(test)]` accessor
would be invisible to it and the phase would not compile. The accessor is plain
`pub`, for exactly the reason the static is.

## Spec

### 1. Add the accessor to `src/lib.rs`

Immediately **below** the existing `TEST_HOME_LOCK` static (do not insert between
the static and its doc comment):

```rust
/// Acquire [`TEST_HOME_LOCK`], recovering if a previous holder panicked.
///
/// A test that panics while holding the lock poisons it. Every later
/// `.lock().unwrap()` on a poisoned mutex then panics too, so one real failure
/// becomes a failure in every HOME-dependent test in the same binary — 48
/// instead of 1, measured. Recovering keeps the count honest: the test that
/// actually broke is the only one that fails.
///
/// Unconditionally `pub`, not `#[cfg(test)]`, for the same reason the lock is:
/// integration tests are a separate crate and do not receive `#[cfg(test)]`
/// items from the library.
pub fn test_home_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
```

### 2. Route all 62 sites through it

Three mechanical substitutions. The binding name (`_lock`, `lock`) is whatever
the site already uses — **keep it**, so the guard's lifetime is unchanged:

```rust
// in src/ (52 sites), all three idioms collapse to:
let _lock = crate::test_home_guard();

// in tests/integration.rs (11 sites):
let _lock = daemoneye::test_home_guard();
```

**Two things not to change:**

- **A site binding `lock` (not `_lock`) keeps that name** — `background.rs:293`,
  `epochs.rs:1166`, `ghost_ws.rs:131`, `recall.rs:259` bind a named guard and use
  it later. Renaming to `_lock` would drop the guard immediately and break the
  serialisation these tests depend on.
- **Nothing else in any test.** This is a substitution of the acquisition
  expression only.

### 3. Delete `TEST_HOME_LOCK`'s now-unused imports

`ghost_ws.rs:107` has `use crate::TEST_HOME_LOCK;` and uses the bare name. After
the substitution that import is unused — **delete it**.

### 4. Delete the ten `UnpoisonExt` imports that die — and keep the one that lives

`unwrap_or_log` comes from `UnpoisonExt`. In **ten** files, the only
`unwrap_or_log` call is the `TEST_HOME_LOCK` one this phase converts, so the
import becomes unused and **`cargo clippy --all-targets` will error on it**
(`cargo build` will not — it reports zero warnings for an unused test-module
import; clippy is authoritative):

| File | `unwrap_or_log` total | on `TEST_HOME_LOCK` | Import |
|---|---|---|---|
| `src/search.rs` | 1 | 1 | **delete** |
| `src/scripts.rs` | 1 | 1 | **delete** |
| `src/runbook.rs` | 1 | 1 | **delete** |
| `src/memory_tests.rs` | 1 | 1 | **delete** |
| `src/manifest_tests.rs` | 1 | 1 | **delete** |
| `src/agents/mod.rs` | 1 | 1 | **delete** |
| `src/agents/mailbox.rs` | 1 | 1 | **delete** |
| `src/daemon/briefing.rs` | 1 | 1 | **delete** |
| `src/daemon/executor/file_ops/read.rs` | 1 | 1 | **delete** |
| `src/daemon/executor/knowledge/mod.rs` | 1 | 1 | **delete** |
| `src/daemon/executor/mod.rs` | **5** | 2 | **KEEP** — three uses survive |

**`src/daemon/executor/mod.rs` is the exception.** Deleting its import breaks the
build. Let clippy confirm each deletion rather than deleting on the pattern.

## Acceptance criteria

- [ ] The Pre-flight census reports
      `{'unwrap': 0, 'unwrap_or_log': 0, 'other': 1}` — **exactly one acquisition
      left, and it is the accessor's own body.** Not zero: the census counts any
      `TEST_HOME_LOCK` followed by `.lock()`, and `test_home_guard` is one by
      construction. **`'unwrap': 0` is the criterion that matters** — it is the
      cascade, and it must be gone.
- [ ] `grep -rn "TEST_HOME_LOCK" src/ tests/ | grep -c "\.lock()"` returns **1** —
      the accessor body in `src/lib.rs`, and nothing else. Verify by reading that
      the one hit is inside `test_home_guard`.
- [ ] `grep -rc "test_home_guard()" src/ tests/ --include=*.rs | grep -v ':0' |
      awk -F: '{s+=$2} END {print s}'` returns **63** — 62 call sites plus the
      definition.
- [ ] `grep -c "UnpoisonExt" src/daemon/executor/mod.rs` returns **3** — unchanged;
      three `unwrap_or_log` calls survive there.
- [ ] `grep -rl "UnpoisonExt" src/ | wc -l` returns **15**, down from **25** —
      exactly the ten files in task 4's table. Quote both numbers.
- [ ] `grep -c "use crate::TEST_HOME_LOCK" src/daemon/context/ghost_ws.rs` returns
      **0**.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit tests and **27** integration tests —
      both unchanged. This phase adds and deletes no tests.
- [ ] `python3 /tmp/audit_closures.py` still prints nothing (unchanged from 05f).

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status, so a failing gate reads as passing.

## Test plan

Behavior-preserving for every passing test: the accessor acquires the same lock
with the same lifetime. What changes is only what happens **after a panic**.

**Write no new tests.** The 916 + 27 existing tests are the regression net, and
the End-to-end verification below is the real proof.

## End-to-end verification

**Reproduce the measurement, before and after.** This is the phase's whole claim,
and a green suite cannot demonstrate it — the suite was green before.

Pick any test that holds the HOME guard (e.g.
`daemon::context::background::tests::background_swap_applies_when_unchanged`)
and make it panic, by temporarily inserting `panic!("cascade probe");` as its
first statement.

1. **Before your changes** (stash them, or check out the parent commit): run
   `cargo test --lib` and record the failure count. Expect **many** — 05g measured
   48 for this class.
2. **After your changes**: same probe, same command. Expect **1**.
3. **Remove the probe** and confirm `cargo test` is back to 916 / 27.

Quote all three numbers in the Update Log. **If the after-count is not 1, the
phase is not done** — report what it was.

Restore the probe by reverting the file, not by retyping the line, and confirm
`git status` is clean when you finish.

## Authorizations

- [x] May edit `src/lib.rs` (the accessor) and every file holding a
      `TEST_HOME_LOCK` acquisition — **23 files**, listed by the Pre-flight census.
- [x] May edit `tests/integration.rs` — 11 of the sites live there.
- [x] **Must delete** the ten `UnpoisonExt` imports in task 4's table, and the
      `use crate::TEST_HOME_LOCK;` in `ghost_ws.rs`.
- [x] May temporarily insert a `panic!` probe for the End-to-end verification,
      provided it is reverted and `git status` is clean at the end.
- [ ] **No** deletion of `UnpoisonExt` from `src/daemon/executor/mod.rs` — three
      unrelated uses survive there.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** change to any test's logic, only to how it acquires the lock.
- [ ] **No** renaming of a named guard binding (`lock` → `_lock`) — that changes
      the guard's drop point.
- [ ] **No** new dependency. A non-poisoning mutex from `parking_lot` would also
      solve this; adding a dependency is a design decision and is not authorised.
- [ ] **No** `#[allow(...)]` anywhere.

## Out of scope

- **Removing the global HOME lock entirely.** Tests serialise on it because they
  mutate a process-global. Making them hermetic (per-test `HOME` without a global
  lock) is a much larger redesign and a separate milestone's work.
- **The `unsafe { std::env::set_var(...) }` blocks.** Rust 2024 requires `unsafe`
  there; that is not a defect and is not this phase's business.
- **Phases 06 and 07.**

### ⚠ Two traps from earlier phases in this milestone

1. **The single-line grep is blind.** Nine of the 62 sites split
   `TEST_HOME_LOCK` / `.lock()` / `.unwrap_or_else(...)` across three lines, and
   `grep -c "TEST_HOME_LOCK.lock()"` cannot see them. Use the Pre-flight census.
   This blindness cost this milestone a bounce and, later, three production sites
   that no scan had ever seen.
2. **Do not insert an item between a doc comment and the item it documents.**
   Task 1 adds a function directly below a documented `static`. Insert **after**
   the static's closing `;`, never between its `///` block and the `pub static`
   line — phase 05a cost two extra runs by doing exactly that.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 13:06 (started)

**Executor:** Claude Sonnet 4.5 (headless)

**Pre-flight census:** `{'unwrap': 41, 'unwrap_or_log': 12, 'other': 10} => acquisitions: 63`
(The +1 vs spec's 62 is the accessor's own `TEST_HOME_LOCK.lock()` call.)

Starting phase: adding `test_home_guard()` accessor and routing all 62 call sites through it.
