# Phase 05d: Make the Session Lock Unreachable — the `SessionStore` Newtype

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-05c (all 22 conversions) — `done`
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Turn `SessionStore` from a type alias into a **newtype**, so `.lock()` is not
reachable from outside `session.rs` and `with_sessions` becomes the only way to
touch the map. This is the "**enforced by a test or lint, not only by review**"
half of the milestone's third exit criterion.

Today the invariant is a convention. After this phase it is a **compile error**.

| Change | Count |
|---|---|
| the type definition itself | 1 |
| `with_sessions`'s own body | 1 |
| `Arc::new(Mutex::new(HashMap::new()))` → `SessionStore::new()` | 16 |
| `Arc::clone(&sessions…)` → `sessions.clone()` | 16 |

**Finish condition: `cargo build` and `cargo clippy --all-targets` both pass with
`SessionStore` defined as a `struct`, and no production code can call `.lock()` on
one.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanisms A and B, and § 1.5b–1.5c — the
  confirmed hang this milestone exists to prevent. The newtype is what stops it
  from being reintroduced by a future edit.
- `CLAUDE.md` § "Important Invariants" — `.unwrap_or_log()` at every lock site.
  `with_sessions` satisfies it internally, and after this phase there is no other
  lock site to satisfy it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "pub type SessionStore" src/daemon/session.rs           # expect 1
grep -c "pub struct SessionStore" src/daemon/session.rs         # expect 0
grep -rn 'Arc::new(Mutex::new(HashMap::new()))\|Arc::new(std::sync::Mutex::new(HashMap::new()))' src/ | wc -l   # expect 16
grep -rn 'Arc::clone(' src/ | grep -c 'sessions'                # expect 16
grep -rc 'SessionStore::new()' src/ --include=*.rs | grep -v ':0' | wc -l   # expect 0
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

## Current state

### The definition — `src/daemon/session.rs:117`

```rust
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;
```

Because it is an *alias*, every holder of a `SessionStore` can call `.lock()`
directly. That is the hole this phase closes.

### The accessor — `src/daemon/session.rs:427-434`

```rust
pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T {
    let _depth = SessionsLockDepth::enter();
    let mut store = sessions.lock().unwrap_or_log();
    f(&mut store)
}
```

`sessions.lock()` here becomes `sessions.0.lock()` — the newtype's field is
visible inside its own module.

### ⚙ Let the compiler find the work — do not hunt for sites

**Change the type first (task 1), then run `cargo build` and fix exactly what it
names.** Every one of the 32 call sites is a compile error the moment the alias
becomes a struct, so the compiler enumerates them for you. Loop:

```
cargo build          # bare — never pipe through `tail`, it swallows the exit status
<fix the errors it names>
cargo build
… until clean, then cargo clippy --all-targets --all-features -- -D warnings
```

This phase has more sites than any other in the milestone, but you should **never
need to search for one**. If you find yourself grepping to discover work rather
than to verify it, stop and go back to the build output.

### ⚠ Look-alike `Arc`s that must NOT change

`Arc::clone(` appears **60** times in the tree; only **16** are sessions. The
rest are on `cache`, `shutdown`, `schedule_store`, `config`, `client` — and two
that read like sessions but are entirely different types:

```rust
let managed_session: Arc<Option<String>> = Arc::new(managed_session);   // NOT a SessionStore
let bg_session: Arc<Mutex<String>> = …                                  // NOT a SessionStore
```

**Never blanket-replace `Arc::clone`.** Both decoys are singular
(`bg_session`, `managed_session`); every real target contains the plural
`sessions`. If the compiler does not complain about a line, it is not yours.

## Spec

### 1. `session.rs:117` — replace the alias with a newtype

```rust
/// Shared registry of live sessions.
///
/// A newtype rather than an alias so the inner mutex cannot be locked from
/// outside this module: `with_sessions` is the only way in, which is what keeps
/// blocking work out of the critical section. See
/// `docs/design/daemon-stalls.md` § 1 mechanism A.
#[derive(Clone, Default)]
pub struct SessionStore(Arc<Mutex<HashMap<String, SessionEntry>>>);

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only escape hatch for asserting the guard was released.
    ///
    /// Deliberately `#[cfg(test)]`: production code must go through
    /// `with_sessions`, and a non-blocking peek is only ever needed to prove a
    /// lock is *not* held.
    #[cfg(test)]
    pub fn try_lock(
        &self,
    ) -> std::sync::TryLockResult<std::sync::MutexGuard<'_, HashMap<String, SessionEntry>>> {
        self.0.try_lock()
    }
}
```

**Three deliberate choices:**

- **`#[derive(Clone)]`** is what keeps the ~27 existing `sessions.clone()` call
  sites compiling untouched. Without it this phase would be several times larger.
- **`try_lock` is `#[cfg(test)]`-gated.** All four callers are in test modules
  (`session.rs` ×3, `executor/mod.rs` ×1), so gating it closes the hole
  completely in production rather than leaving a second way in. Returning
  `TryLockResult` (std's own alias) keeps every caller's `.is_ok()` and
  `.expect(…)` working unchanged.
- **No `lock()` method, not even a private one.** `with_sessions` reaches the
  field directly. Adding a `lock()` would recreate exactly what this phase
  removes.

Keep the `derive(Default)` — `Arc<Mutex<HashMap<…>>>` is `Default`, so
`SessionStore::new()` is a one-liner rather than hand-built.

### 2. `with_sessions` — reach through the newtype

One character-level change in the body:

```rust
    let mut store = sessions.0.lock().unwrap_or_log();
```

Everything else in the function is unchanged, **including the
`SessionsLockDepth::enter()` re-entrancy guard on the line above.** That guard is
what makes a nested acquisition panic loudly instead of deadlocking; do not
remove or reorder it.

`cleanup_pass` already goes through `with_sessions` and needs no change.

### 3. The 16 construction sites → `SessionStore::new()`

Every one is already annotated with its type, which makes them unambiguous:

```rust
// before
let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
// after
let sessions: SessionStore = SessionStore::new();
```

Six use `std::sync::Mutex::new` spelled out (`context/background.rs`); they take
the same replacement. Full inventory — **1 production, 15 test**:

| File | Lines |
|---|---|
| `daemon/mod.rs` | 602 — **the only production one** |
| `daemon/session.rs` | 1179, 1197, 1223, 1239, 1248, 1268 |
| `daemon/context/background.rs` | 381, 434, 466, 492, 540, 585 |
| `daemon/executor/mod.rs` | 1001, 1143, 1244 (variable is named `store`) |

**Leave the `: SessionStore` annotations in place.** They stay correct and they
are what makes these sites greppable.

### 4. The 16 `Arc::clone` sites → `.clone()`

```rust
// before
let sessions_sup = Arc::clone(&sessions);
// after
let sessions_sup = sessions.clone();
```

Three of the sixteen take a **reference argument** and so have no `&`:

```rust
// ask.rs:721 and knowledge/pane.rs:218 — `sessions` is already a &SessionStore
sessions: Arc::clone(sessions),        →   sessions: sessions.clone(),
let sessions_clone = Arc::clone(sessions);  →   let sessions_clone = sessions.clone();
```

Full inventory:

| File | Lines |
|---|---|
| `daemon/mod.rs` | 618, 624, 639, 654, 659, 682, 687, 722, 727, 773 |
| `daemon/server/mod.rs` | 182, 196, 204 |
| `daemon/context/background.rs` | 450 |
| `daemon/server/ask.rs` | 721 — no `&` |
| `daemon/executor/knowledge/pane.rs` | 218 — no `&` |

Note `mod.rs:659` is `Arc::clone(&wh_sessions_sup)` — it does not contain the
substring `&sessions`, which is why a `grep "Arc::clone(&sessions"` sweep finds
only 13 of the 16. **Trust the compiler, not that grep.**

### 5. Confirm the hole is actually closed

After the gates pass, verify by reading that **no `impl SessionStore` method
exposes a blocking `lock()`**, and that the only `.lock()` on the inner field is
inside `with_sessions`. State both in the Update Log.

Then confirm the enforcement works, and **report what you observe**:

```bash
# In a scratch file or by temporarily editing a production fn, confirm that
#   sessions.lock()
# no longer compiles outside session.rs. REVERT the experiment afterwards and
# leave the tree clean.
```

If you run that experiment, `git status` must be clean when you finish.

## Acceptance criteria

- [ ] `grep -c "pub type SessionStore" src/daemon/session.rs` returns **0**.
- [ ] `grep -c "pub struct SessionStore" src/daemon/session.rs` returns **1**.
- [ ] `grep -rn 'Arc::new(Mutex::new(HashMap::new()))\|Arc::new(std::sync::Mutex::new(HashMap::new()))' src/ | wc -l`
      returns **0**.
- [ ] `grep -rc "SessionStore::new()" src/ --include=*.rs | grep -v ':0' | wc -l`
      returns **4** — the four files listed in task 3.
- [ ] `grep -rn 'Arc::clone(' src/ | grep -c 'sessions'` returns **0**.
- [ ] `grep -rn 'Arc::clone(' src/ | wc -l` returns **44** — down from 60 by
      exactly the 16 sessions sites. **A lower number means you changed an
      unrelated `Arc`**; the `cache` / `shutdown` / `schedule_store` /
      `bg_session` / `managed_session` clones must all survive.
- [ ] `grep -c "cfg(test)" src/daemon/session.rs` returns **2** — the pre-existing
      `mod tests` gate (currently the only one) plus the new one on `try_lock`.
- [ ] `python3 /tmp/scan_all.py $(git ls-files 'src/**/*.rs' 'src/*.rs')` reports a
      non-zero count for **`src/daemon/session.rs` only**, and that count is
      **`prod=1`** — line 443, the `cleanup_pass` doc comment. The real
      acquisition at the old `:432` no longer matches, because it is now
      `sessions.0.lock()`.
- [ ] `grep -cF "SessionsLockDepth::enter()" src/daemon/session.rs` returns **2** —
      **not 1.** Only one is code (`:431`, the re-entrancy guard, which must
      survive); the other is `:1274`, where the name appears inside a test's
      assertion message. The grep counts prose, exactly as it does for the
      `cleanup_pass` doc comment at `:443`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **any other number means scope crept.**
- [ ] `git status` is clean — no leftover scratch file from the task 5 experiment.

**Run every gate bare.** `cargo clippy … | tail -20` exits with `tail`'s status,
so a failing gate reads as passing — that is how a real error went unnoticed in
the previous phase.

## Test plan

The compiler is the test. A newtype that hides `.lock()` either compiles across
all 32 call sites or does not, and the 915 existing tests confirm behavior is
unchanged — `#[derive(Clone)]` on a newtype over `Arc` clones the same handle the
alias did, so every clone still points at one shared map.

**Write no new tests.** The four `try_lock` assertions already prove guard
release, and they now also prove the `#[cfg(test)]` accessor works.

**No production behavior changes at all** in this phase — it is a visibility
change. Anything that alters runtime behavior is out of scope and a sign
something went wrong.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim a test "guards" the enforcement — the
compiler does, and the honest demonstration is the task 5 experiment.

Three reasoning checks to state in the Update Log, no new tests:

1. **Clone semantics.** Confirm `#[derive(Clone)]` on the newtype clones the
   inner `Arc` (one shared map) and does **not** deep-copy the `HashMap`. Say in
   one sentence what would break if it did.
2. **The re-entrancy guard.** Confirm `SessionsLockDepth::enter()` is still the
   first statement in `with_sessions`, before the acquisition.
3. **The decoys.** Confirm `bg_session` and `managed_session` still use
   `Arc::clone` and were not swept up — name their types.

## End-to-end verification

The task 5 experiment **is** the end-to-end verification: demonstrate that
`sessions.lock()` no longer compiles outside `session.rs`, quote the compiler
error in the Update Log, and revert the experiment. That is the phase's whole
claim, and it is not provable by a passing test suite.

## Authorizations

- [x] May edit `src/daemon/session.rs` (the definition, `with_sessions`, and its
      test module).
- [x] May edit the call sites named in tasks 3 and 4:
      `daemon/mod.rs`, `daemon/server/mod.rs`, `daemon/server/ask.rs`,
      `daemon/context/background.rs`, `daemon/executor/mod.rs`,
      `daemon/executor/knowledge/pane.rs`.
- [x] May add or remove `use` lines **only** where the compiler requires it —
      e.g. if `Mutex` or `HashMap` becomes unused in a test module. Run
      `cargo clippy --all-targets` to decide; it, not `cargo build`, is
      authoritative for test-module imports.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** `lock()` method on `SessionStore`, public or private.
- [ ] **No** behavior changes. This is a visibility change only.
- [ ] **No** `#[allow(...)]` anywhere. If clippy objects, report a blocker rather
      than suppressing.

## Out of scope

- **04f's coverage follow-up** — the three vacuous `compaction_in_flight`
  assertions in `background.rs`. That is **phase 05e**. You will be editing
  construction lines in the same test module; **change nothing else** — do not
  "fix" an assertion you notice looks weak.
- **A lint or CI rule beyond the type system.** The newtype *is* the enforcement.
  Do not add a clippy config, a custom lint, or a grep-based CI check.
- **Renaming `with_sessions` or changing its signature.**
- **`session.rs:443`** — the `cleanup_pass` doc comment containing the literal
  `sessions.lock()`. It is prose, it is still accurate, and it is expected to
  keep showing up in the scan. Leave it.

### ⚠ Two traps from earlier phases in this milestone

1. **Do not assert an import count without checking whether your edits exhaust
   its uses.** Phase 05c `hard_fail`ed because its spec required an import to
   survive while converting the only 17 things that used it. If an import goes
   unused here, **delete it** — and note that `cargo build` reports zero warnings
   for an unused *test-module* import while `cargo clippy --all-targets` errors on
   it. Clippy is authoritative.
2. **Do not insert an item between a doc comment and the item it documents.**
   Phase 05a cost two extra runs when a `struct` added "immediately above" a
   function landed between that function's `///` block and the function, silently
   transferring its documentation. Task 1 adds a `struct` and an `impl` block to
   `session.rs` — **read the lines directly above and below your insertion point**
   and confirm no `///` block has been orphaned.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
