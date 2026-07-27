# Phase 05d: Make the Session Lock Unreachable — the `SessionStore` Newtype

**Milestone:** M5 — UX & Stability
**Status:** done
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

### Update — 2026-07-27 00:56 (started)

**Executor:** Claude (Sonnet 4.5)

Converted `SessionStore` from a `pub type` alias to a `pub struct` newtype wrapping `Arc<Mutex<HashMap<String, SessionEntry>>>`. Added `#[derive(Clone, Default)]`, `SessionStore::new()`, and a `#[cfg(test)]`-gated `try_lock()` method. Updated `with_sessions` to reach through the newtype via `sessions.0.lock()`. Converted all 16 construction sites to `SessionStore::new()` and all 16 `Arc::clone(&sessions)` sites to `sessions.clone()`. Also fixed 2 additional `.lock()` call sites in `respawn.rs` and `run.rs` that were not in the spec's inventory (they used direct `.lock()` instead of `with_sessions`), and 2 more in `pane.rs`'s `watch_pane` callback, converting them to use `with_sessions`. Removed a shadowing `type SessionStore` alias from `hook.rs` and updated 3 `Arc::clone(&sessions)` → `sessions.clone()` in `server/mod.rs`. Fixed 2 additional test construction sites in `memory.rs` and `pane.rs` that were not in the spec's inventory. Removed now-unused `HashMap`/`Mutex` imports from test modules.

**Build gates:** `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo fmt --all` clean, `cargo test` 915 passed.

**Task 5 experiment:** Created a scratch file `test_lock_experiment.rs` with `s.lock()` on a `SessionStore` — compiler error: `no method named 'lock' found for struct 'daemon::session::SessionStore'`. Reverted and deleted the scratch file; tree is clean.

**Reasoning checks:**
1. **Clone semantics:** `#[derive(Clone)]` on the newtype clones the inner `Arc`, so all clones point to the same shared `HashMap`. If it deep-copied the map, every handler would get an isolated copy and session state would never be shared.
2. **Re-entrancy guard:** `SessionsLockDepth::enter()` is the first statement in `with_sessions`, before the `sessions.0.lock()` acquisition — confirmed.
3. **Decoys:** `bg_session` is `Arc<Mutex<String>>` and `managed_session` is `Arc<Option<String>>`; both still use `Arc::clone` untouched.

### Update — ts=1785115482399 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted `SessionStore` from a `pub type` alias to a `pub struct` newtype, making `.lock()` unreachable from outside `session.rs`. The newtype derives `Clone` and `Default`, provides `SessionStore::new()`, and exposes a `#[cfg(test)]`-gated `try_lock()` for test assertions. Updated `with_sessions` to reach through via `sessions.0.lock()`. All 16 spec-listed construction sites and `Arc::clone` sites were converted, plus 4 additional `.lock()` call sites discovered by the compiler in `respawn.rs`, `run.rs`, and `pane.rs` (converted to use `with_sessions`). Removed a shadowing `type SessionStore` alias in `hook.rs`. Task 5 experiment confirmed: `sessions.lock()` produces `no method named 'lock' found for struct 'daemon::session::SessionStore'` outside `session.rs`. All gates pass: `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo fmt --all` clean, `cargo test` 915 passed. The spec expected 4 files with `SessionStore::new()`; the compiler found 2 additional test files (`memory.rs`, `pane.rs`) that also needed conversion — these were not listed in the spec's inventory but were necessary for a clean build.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ext_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.20s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test event_log_append_read ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-05d-sessionstore-newtype.md` — +16 -1
- `src/daemon/background/respawn.rs` — +6 -5
- `src/daemon/background/run.rs` — +6 -5
- `src/daemon/context/background.rs` — +7 -8
- `src/daemon/executor/knowledge/memory.rs` — +3 -3
- `src/daemon/executor/knowledge/pane.rs` — +18 -21
- `src/daemon/executor/mod.rs` — +9 -13
- `src/daemon/hook.rs` — +1 -4
- `src/daemon/mod.rs` — +11 -12
- `src/daemon/server/ask.rs` — +1 -1
- `src/daemon/server/mod.rs` — +3 -3
- `src/daemon/session.rs` — +33 -9

**Commit:** ca46ad4fecbad070de62276b487ca824fefec826

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none (189 turns)
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** four, **all of them corrections to an incomplete spec**,
  every one forced by the compiler and correctly scoped. See below.
- **Calibration:** one major instrument finding, one new mechanism-A site.

The newtype landed exactly as specified: `#[derive(Clone, Default)]`,
`SessionStore::new()`, a `#[cfg(test)]`-gated `try_lock` returning
`TryLockResult`, no `lock()` method, and `with_sessions` reaching the field via
`sessions.0.lock()`. Gates re-run bare with exit codes captured: fmt 0, build 0
(zero warnings), clippy 0, test 0 with **915** tests unchanged.

**`Arc::clone` landed at exactly 44** — 60 minus the 16 sessions sites, with no
unrelated `Arc` swept up. The decoys (`bg_session: Arc<Mutex<String>>`,
`managed_session: Arc<Option<String>>`) are untouched.

### ⚠ The scan instrument was blind for the entire milestone

**This is the finding that matters most.** The compiler found **four production
`.lock()` sites that every scan in phases 04a–05c missed**:

| Site | Variable |
|---|---|
| `background/run.rs:412` | `sessions_bg.lock()` |
| `background/respawn.rs:288` | `sessions_bg.lock()` |
| `executor/knowledge/pane.rs:330` | `sessions_clone.lock()` |

The scan matched the literal string `sessions.lock()` (plus its multi-line
split). **A `SessionStore` held in a differently-named variable was invisible to
it.** `sessions_bg.lock()` does not contain `sessions.lock()`.

Every "0 raw acquisitions" claim in this milestone was therefore made with an
instrument that could only see one variable name. This is the second and worse
instrument failure here — 04f found it could not see multi-line splits; this one
was never suspected, and unlike the multi-line gap it had no symptom.

**The good news is that it no longer matters.** After this phase, any `.lock()`
on a `SessionStore` outside `session.rs` is a **compile error**, so the end state
is now guaranteed by the type system rather than asserted by grep. That is
precisely why this phase existed, and it justified itself by finding three real
sites on the way in.

### ⚠ A live mechanism-A/B site, newly visible — `pane.rs`

`executor/knowledge/pane.rs`'s `watch_pane` callback now reads:

```rust
        with_sessions(&sessions_clone, |store| {
            if let Some(entry) = store.get_mut(&session_id_owned) {
                append_session_message(&session_id_owned, &watch_msg);   // 2 file writes
                entry.messages.push(watch_msg);
                …
                if let Some(ref cp) = entry.chat_pane {
                    let _ = std::process::Command::new("tmux")           // subprocess spawn
                        .args(["display-message", "-d", "5000", "-t", cp, &alert])
                        .output();
                }
            }
        });
```

**Two file writes and a tmux subprocess, under the global session lock.** This is
mechanism A + B — the exact shape 05a and 05b spent two phases removing.

**The executor did not introduce it.** The same work sat under
`sessions_clone.lock()` before; the conversion is behavior-preserving, which is
what the spec required ("No behavior changes. This is a visibility change only").
The site was simply never triaged into 05a/05b **because the scan could not see
it**. Restructuring it here would have been scope creep and a behavior change the
spec forbade — declining to do so was correct.

But it now sits inside a `with_sessions` closure, which is the construct this
milestone uses to mark *"this critical section is safe"*. It is not.

**Follow-up required — a new phase.** Same shape as 05a task 3: collect
`chat_pane` under the lock, release, then write and spawn. `respawn.rs` and
`run.rs` need no such follow-up — their closures hold only
`entry.bg_windows.retain(…)`, and `tmux::kill_job_window` is outside.

### ⛔ Do not tick the milestone's third exit criterion yet

Its enforcement half — "enforced by a test or lint, not only by review" — is
**done**, and done better than planned: the type system, not a lint.

Its first half — "no `SessionStore` critical section performs blocking work" — is
**now known to be false**, at `pane.rs`. It was believed true after 05b only
because the site was invisible. Tick the criterion when the `pane.rs` follow-up
lands.

### The four scope deviations, each necessary

1. **Three extra `.lock()` conversions** (`run.rs`, `respawn.rs`, `pane.rs`) —
   compiler-forced; the spec's inventory missed them.
2. **Two extra construction sites** (`knowledge/memory.rs`, `knowledge/pane.rs`)
   — my pre-flight grep matched `Arc::new(Mutex::new(HashMap::new()))` and the
   `std::sync::Mutex` variant, but **not** the fully-qualified
   `std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))`.
   So the criterion "**4** files with `SessionStore::new()`" is wrong; **6** is
   correct.
3. **`hook.rs`'s shadowing `type SessionStore` alias removed.** Under the alias
   regime 04j deliberately kept it (it expanded to the identical type). Under a
   newtype it expands to a *different* type and cannot stay. Correct call.
4. **Unused `HashMap` / `Mutex` / `Arc` imports removed** from test modules —
   explicitly authorized, and required by `cargo clippy --all-targets`.

### Verified by reading

- **Clone semantics.** `#[derive(Clone)]` on a newtype over `Arc` clones the
  handle, so every clone still points at one shared map. A deep copy would give
  each handler an isolated map and session state would silently stop being
  shared — no test would catch it, since each test builds its own store.
- **The re-entrancy guard is intact.** `SessionsLockDepth::enter()` is still the
  first statement in `with_sessions`, before the acquisition, and
  `with_sessions_rejects_reentrant_call` still passes.
- **`respawn.rs` / `run.rs` closures hold no blocking work** —
  `entry.bg_windows.retain(…)` only, with `kill_job_window` outside.
- **The task 5 experiment was performed and reverted**; the tree is clean and the
  quoted error is `no method named 'lock' found for struct SessionStore`.

### Two criteria of mine that were wrong, both from counting prose

- **`grep -c "cfg(test)" src/daemon/session.rs` returns 3, not 2** — the third is
  the phrase `#[cfg(test)]` inside the doc comment *my own spec told the executor
  to write*.
- **`scan_all` now reports `prod=0 test=1`**, not `prod=1`. The `#[cfg(test)]`
  attribute on `try_lock` sits near the top of the file, and the scan treats the
  first `#[cfg(test)]` as the start of the test module — so it misclassifies
  everything below. The one hit is still the `cleanup_pass` doc comment, now at
  `:467`.

That is the **fourth** phase running where a text-based grep counted prose rather
than code. The instrument is retired by this phase for `.lock()` detection; the
compiler replaces it.
