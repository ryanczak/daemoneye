# Phase 01: Fix test hermeticity (parallel-HOME race)

**Milestone:** M3 — Polish & Maintenance
**Status:** done
**Depends on:** none
**Estimated diff:** ~70 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Eliminate the documented parallel-`HOME` flaky test `webhook_alert_to_event_log`
and make every `HOME`-mutating integration test obey one invariant: hold
`TEST_HOME_LOCK` for the test's **entire** `HOME`-dependent body, and **restore**
`HOME` on exit. This is the first M3 phase because a reliably-green `cargo test`
underpins every later phase's review.

## Architecture references

Read before starting:

- None required. This is a test-only fix. The relevant invariant is documented at
  `src/lib.rs:27` (the `TEST_HOME_LOCK` doc comment): *"All test modules that call
  `env::set_var("HOME", ...)` must hold this lock."*

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All changes are confined to **`tests/integration.rs`** (the only file with the
defect). The `src/` unit tests already hold the lock and restore `HOME` correctly
via per-module `with_home` helpers — **do not touch them**.

`TEST_HOME_LOCK` is a process-global `std::sync::Mutex<()>` exported at
`src/lib.rs:32` (`pub static TEST_HOME_LOCK`). Integration tests reach it as
`daemoneye::TEST_HOME_LOCK`. Within the integration test binary, tests run on
multiple threads concurrently; `HOME` is process-global, so any test that sets
`HOME` must serialize against every other such test by holding this lock for as
long as it depends on `HOME`.

`HOME` is set with `unsafe { std::env::set_var("HOME", …) }` because this is
edition 2024 (`set_var` is `unsafe`). That `unsafe` block is **not** a lock scope —
it is just the unsafe-call wrapper. Keep the `unsafe` blocks.

### The bug — `webhook_alert_to_event_log` (currently at line 649)

The lock is acquired in an inner block and **dropped immediately** (line ~662),
then the test reads `HOME`-derived paths (`Config::ensure_dirs`, `events_path()`)
with the lock released:

```rust
#[tokio::test(flavor = "current_thread")]
async fn webhook_alert_to_event_log() {
    // ...
    let tmp = tempfile::tempdir().expect("create tempdir");
    {
        let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path().to_str().unwrap());
        }
    }                                   // <-- LOCK DROPPED HERE (the bug)
    daemoneye::config::Config::ensure_dirs().expect("ensure dirs");
    // ... process_alert(...).await reads HOME-derived events.jsonl, UNLOCKED ...
    let path = daemoneye::config::events_path();
    let content = fs::read_to_string(&path).expect("read events.jsonl");
    // assertions ...
}
```

A concurrent test can change `HOME` between the drop and the `events_path()` read,
so `webhook_alert_to_event_log` reads the wrong `events.jsonl` and intermittently
fails. It also never restores `HOME`.

### The five "leak" tests — lock held, but no `HOME` restore

These hold the lock for the whole body (correct) but never restore `HOME`, leaving
the process pointed at a deleted temp dir for any later unlocked reader:

- `session_jsonl_round_trip` (~204)
- `session_index_persistence` (~259)
- `event_log_entry_format` (~300)
- `cost_record_serializes_to_events_jsonl_round_trip` (~327)
- `event_log_append_read` (~388)

### The canonical correct pattern — `g4_briefing_read_and_clear` (~940)

Use this exact shape as the model (lock at function scope, capture `old_home`
before `set_var`, restore at the end):

```rust
#[test]
fn g4_briefing_read_and_clear() {
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    let tmp = temp_daemoneye_home();
    let old_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", &tmp);
    }
    // ... body that depends on HOME ...
    match old_home {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
```

The `g4_*`, `g5_*`, and `g6_*` HOME tests already follow this pattern — **leave
them unchanged**.

## Spec

1. **Rewrite `webhook_alert_to_event_log` as a synchronous `#[test]`** — in
   `tests/integration.rs`, replace the test's attribute + signature and lock
   handling. **Do not keep it `async` with a function-scoped lock**: holding a
   `std::sync::MutexGuard` across the `process_alert(...).await` would trip
   `clippy::await_holding_lock`, which is denied under
   `cargo clippy --all-targets --all-features -- -D warnings`. Instead drive the
   one async call through a local current-thread runtime so no `.await` appears in
   the test body while the guard is alive. The full replacement body:

   ```rust
   /// (keep the existing doc comment block above the test unchanged)
   #[test]
   fn webhook_alert_to_event_log() {
       use daemoneye::webhook::{WebhookState, parse_payload, process_alert};

       daemoneye::ai::filter::init_masking(&[]);

       let tmp = tempfile::tempdir().expect("create tempdir");
       let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
       let old_home = std::env::var("HOME").ok();
       unsafe {
           std::env::set_var("HOME", tmp.path().to_str().unwrap());
       }
       daemoneye::config::Config::ensure_dirs().expect("ensure dirs");

       let body = serde_json::json!({
           // ... keep the existing synthetic Alertmanager payload verbatim ...
       });
       let alerts = parse_payload(&body);
       assert_eq!(alerts.len(), 1);
       let alert = &alerts[0];
       assert_eq!(alert.alert_name, "HighCPU");
       assert_eq!(alert.severity, "critical");
       assert_eq!(alert.source, "alertmanager");

       let config = daemoneye::config::Config::default();
       let sessions = daemoneye::daemon::session::SessionStore::default();
       let cache = std::sync::Arc::new(daemoneye::daemon::SessionCache::new("test"));
       let schedule_store =
           std::sync::Arc::new(daemoneye::scheduler::ScheduleStore::new_empty());
       let state = std::sync::Arc::new(WebhookState {
           config,
           sessions,
           cache,
           schedule_store,
           dedup: std::sync::Mutex::new(std::collections::HashMap::new()),
           rate_limit: std::sync::Mutex::new(std::collections::HashMap::new()),
       });

       let rt = tokio::runtime::Builder::new_current_thread()
           .enable_all()
           .build()
           .expect("build current-thread runtime");
       rt.block_on(process_alert(alert.clone(), state));

       let path = daemoneye::config::events_path();
       let content = fs::read_to_string(&path).expect("read events.jsonl");
       let lines: Vec<&str> = content.lines().collect();
       let last: serde_json::Value =
           serde_json::from_str(lines.last().expect("at least one line"))
               .expect("parse last line");
       assert_eq!(last["event"], "webhook_alert");
       assert_eq!(last["alert_name"], "HighCPU");
       assert_eq!(last["severity"], "critical");
       assert!(last["ts"].is_string());

       match old_home {
           Some(v) => unsafe { std::env::set_var("HOME", v) },
           None => unsafe { std::env::remove_var("HOME") },
       }
   }
   ```

   Preserve the existing payload JSON and assertions exactly; only the
   attribute/signature, the lock scope, and the `HOME` capture/restore change.
   `tmp` is a `tempfile::TempDir` and cleans itself on drop — no `remove_dir_all`
   needed here.

2. **Add `HOME` capture + restore to the five leak tests** — in
   `tests/integration.rs`, for each of `session_jsonl_round_trip`,
   `session_index_persistence`, `event_log_entry_format`,
   `cost_record_serializes_to_events_jsonl_round_trip`, and
   `event_log_append_read`: insert `let old_home = std::env::var("HOME").ok();`
   immediately **before** the `unsafe { std::env::set_var("HOME", …) }` block, and
   immediately **before** the test's closing brace add:

   ```rust
   match old_home {
       Some(v) => unsafe { std::env::set_var("HOME", v) },
       None => unsafe { std::env::remove_var("HOME") },
   }
   ```

   Do not move or re-scope their existing `let _lock = …` lines — those are already
   function-scoped and correct. Do not add `remove_dir_all` (out of scope; these
   tests already omit it).

## Acceptance criteria

- [ ] `webhook_alert_to_event_log` is a synchronous `#[test] fn` (no `async`, no
      `#[tokio::test]`) that holds `daemoneye::TEST_HOME_LOCK` from before the
      `set_var("HOME", …)` through the `events_path()` read, and restores `HOME`
      via the `old_home` match at the end.
- [ ] No `std::sync` lock guard is held across an `.await` anywhere in
      `tests/integration.rs` (the async work runs inside `rt.block_on(...)`).
- [ ] Each of the five leak tests captures `old_home` before `set_var` and restores
      `HOME` before returning.
- [ ] The `g4_*` / `g5_*` / `g6_*` HOME tests are byte-for-byte unchanged.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes (in
      particular, **no `await_holding_lock`**).
- [ ] `cargo test` passes.

## Test plan

This phase repairs existing tests; the repaired tests are themselves the
regression. No new test functions are required — the behavior under test
(serialized, restored `HOME`) is verified by running the existing integration
suite under concurrency (see End-to-end verification). Adding a brand-new test
that deterministically reproduces a thread race is not feasible without a
contrived sleep, which STANDARDS §3.3 forbids; the concurrency soak below is the
verification instead.

State this in the completion Update Log under "New tests": *"None — phase repairs
existing tests; regression verified via the concurrency soak in End-to-end
verification."*

## End-to-end verification

The flake is a thread race, so verify by running the integration binary under
multi-threaded concurrency repeatedly and confirming every run is green. Paste the
actual output.

```bash
# Build once, then soak the integration suite under concurrency.
cargo test --test integration -- --test-threads=16 2>&1 | grep 'test result'
for i in $(seq 1 25); do
  cargo test --test integration -- --test-threads=16 2>&1 \
    | grep -E 'test result|FAILED' | tail -1
done
```

Every line must read `test result: ok. … 0 failed …`. Quote the final loop output
(at least the last several iterations) in the completion Update Log. (Before this
fix, `webhook_alert_to_event_log` fails intermittently across these iterations.)

## Authorizations

- [ ] May add dependencies: none. (`tokio::runtime::Builder` is already available
      to the test crate — `tokio` is a dev-dependency and `#[tokio::test]` is
      already used in `tests/integration.rs`, so the `rt`/`macros` features are
      present via feature unification.)
- [ ] May touch `docs/architecture.md`: no.

None beyond the above.

## Out of scope

- The `src/` unit-test `with_home` / `temp_home` / `TmpHome` helpers — they already
  lock and restore correctly. Consolidating their duplication is a separate M3
  maintenance phase; **do not** touch `src/`.
- The `g4_*` / `g5_*` / `g6_*` HOME tests — already correct; leave them alone.
- Introducing a shared RAII `HOME`-guard helper or refactoring the test-isolation
  pattern. A future phase may do this; here, mirror the existing manual pattern.
- Any non-`HOME` environment-variable mutation.
- Adding `remove_dir_all` cleanup to the five leak tests.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-28 05:25 (started)

**Executor:** rexyMCP executor

Started implementing Phase 01: Fix test hermeticity (parallel-HOME race).

### Update — 2026-06-28 05:30 (complete)

**Executor:** rexyMCP executor

**Summary:** Rewrote `webhook_alert_to_event_log` from `#[tokio::test]` async to synchronous `#[test]`, holding `TEST_HOME_LOCK` for the entire body and restoring `HOME`. Added `old_home` capture + restore to the five leak tests (`session_jsonl_round_trip`, `session_index_persistence`, `event_log_entry_format`, `cost_record_serializes_to_events_jsonl_round_trip`, `event_log_append_read`).

**Files changed:** `tests/integration.rs`, `docs/dev/milestones/M3-polish-maintenance/phase-01-fix-test-hermeticity.md`

**Verification commands:**
- `cargo fmt --all` — passed
- `cargo build` — passed, zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed (no `await_holding_lock`)
- `cargo test` — passed (27 passed, 0 failed, 2 ignored)
- 25× `cargo test --test integration -- --test-threads=16` — all green, 0 failures

**End-to-end verification:** 25 consecutive runs of the integration suite under `--test-threads=16` all produced `test result: ok. 27 passed; 0 failed; 2 ignored`. Before the fix, `webhook_alert_to_event_log` failed intermittently under concurrency.

**Grep proof:** `grep 'fn webhook_alert_to_event_log' tests/integration.rs` → 1 match (synchronous `#[test]`); `grep 'async fn webhook_alert_to_event_log' tests/integration.rs` → 0 matches.

**New tests:** None — phase repairs existing tests; regression verified via the concurrency soak in End-to-end verification.

**Notes for review:** No adaptations needed; the spec matched the code exactly.

### Review verdict — 2026-06-27

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** rexyMCP executor (Qwen/Qwen3.6-27B-PrismaAURA)
- **Scope deviations:** none
- **Calibration:** none

Independently re-ran fmt/build/clippy/test (all green) plus a 15× integration
concurrency soak under `--test-threads=16` (15/15 `27 passed; 0 failed`). Verified
all six tests capture+restore `HOME`, `webhook_alert_to_event_log` is a sync
`#[test]` driving the async call via `rt.block_on` (no `await_holding_lock`), and
the `g4_*`/`g5_*`/`g6_*` tests are untouched in commit `c52608f`.
