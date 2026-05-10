# Phase A.5 — Finish the Integration Test Story
# Implementation Plan
#
# Drafted: 2026-05-09
# Status: A1–A7 complete (A.7 cleanup landed 2026-05-10)
#         C5 (large files) and C8 (thiserror) deferred — see end of doc

---

## Goal

Close the structural gaps in `tests/integration.rs` so the integration suite
actually catches production regressions. Current state: 10 tests, all shallow
serde round-trips and hand-rolled JSON. Three defects (C6):
  1. Production types re-declared locally — stale copies of `ipc.rs`
  2. Persistence tests hand-write JSON instead of calling real APIs
  3. No daemon-loop or webhook-pipeline test

**Exit criteria:** zero local re-declarations in `tests/`; integration suite
imports `daemoneye::*`; ≥ 1 daemon-process test; ≥ 1 webhook-pipeline test;
total test count ≥ 600.

---

## A1. Convert `daemoneye` to library + binary

### Motivation
Unblocks every subsequent item. Also precondition for Phase E plugin work (R3/I8).

### Changes

**New file: `src/lib.rs`**
```rust
// Library crate — modules needed by integration tests and future plugins.
pub mod ai;
pub mod config;
pub mod daemon;
pub mod ipc;
pub mod scheduler;
pub mod session_store;
pub mod webhook;

// Internal-only modules (not needed by tests or plugins).
pub(crate) mod cli;
pub(crate) mod header;
pub(crate) mod manifest;
pub(crate) mod memory;
pub(crate) mod pane_prefs;
pub(crate) mod runbook;
pub(crate) mod scripts;
pub(crate) mod search;
pub(crate) mod sys_context;
pub(crate) mod tmux;
pub(crate) mod util;
```

**Modified: `src/main.rs`**
Remove all `mod *` declarations. Replace with:
```rust
use daemoneye::{ai, cli, config, daemon, ipc, scheduler, session_store, util};
// (and any other modules main.rs references)
```
The `#[cfg(test)]` `TEST_HOME_LOCK` static moves to `lib.rs` so all test
modules (including `_tests.rs` siblings) can share it.

**Modified: `Cargo.toml`** — no changes. `lib.rs` and `main.rs` are both
implicit crate roots; no `[lib]` or `[[bin]]` section needed.

### Visibility audit
Most types/functions in the target modules are already `pub`. Items that need
promotion to `pub` for test access:

| Module | Item | Current | Needed |
|--------|------|---------|--------|
| `daemon/utils.rs` | `log_event()` | `pub` | — (already ok) |
| `scheduler.rs` | `ScheduleStore` | `pub` | — (already ok) |
| `scheduler.rs` | `ScheduleStore::load_or_create()` | `pub` | — |
| `scheduler.rs` | `ScheduleStore::add()` | `pub` | — |
| `session_store.rs` | `save_session()` | `pub` | — |
| `session_store.rs` | `load_session()` | `pub` | — |
| `ai/filter.rs` | `init_masking()` | `pub` | — |
| `ai/filter.rs` | `mask_sensitive()` | `pub` | — |
| `webhook.rs` | `process_alert()` | `pub(crate)` | `pub` (for A5) |
| `webhook.rs` | `parse_payload()` | private | `pub` (for A5) |
| `webhook.rs` | `InternalAlert` | `pub` | — |
| `config.rs` | `events_path()` | `pub` | — |
| `config.rs` | `Config::ensure_dirs()` | `pub` | — |

**Risk:** `daemon` module is large. Making it `pub` exposes internal types to
the test crate. Mitigation: only sub-modules that tests actually need will be
imported (`daemon::utils`, `daemon::session`). The `pub mod daemon;` in
`lib.rs` is fine — tests choose what to import.

---

## A2. Replace local IPC enums with `daemoneye::ipc::*`

### Changes

**Modified: `tests/integration.rs`**
- Delete lines 28–118 (local `Request`/`Response` enum definitions, ~90 lines)
- Replace with:
  ```rust
  use daemoneye::ipc::{Request, Response};
  ```
- Update 3 existing round-trip tests to construct `Request`/`Response` from
  the production types. Tests: `ipc_ask_round_trip`,
  `ipc_tool_call_response_round_trip`, `ipc_session_info_round_trip`.

### Effect
If `ipc.rs` adds a field, renames a variant, or changes a type, these tests
will now fail to compile — the opposite of current behaviour where they
silently pass against stale local copies.

---

## A3. Persistence tests via real APIs

### Changes

**`schedule_store_persistence`** — rewrite:
```rust
use daemoneye::scheduler::{ScheduleStore, ScheduledJob, ScheduleKind, ActionOn, JobStatus};

let tmp = temp_daemoneye_home();
let path = tmp.join("var").join("schedules.json");
std::fs::create_dir_all(&path.parent().unwrap()).unwrap();

let store = ScheduleStore::load_or_create(path.clone()).unwrap();
store.add(ScheduledJob::new(
    "disk check".into(),
    ScheduleKind::Every { interval_secs: 300, next_run: /* future */ },
    ActionOn::Script { name: "check-disk.sh".into() },
    /* ghost_config */ &Default::default(),
)).unwrap();

// Load fresh and assert.
let store2 = ScheduleStore::load_or_create(path).unwrap();
let jobs = store2.list();
assert_eq!(jobs.len(), 1);
assert_eq!(jobs[0].name, "disk check");
assert_eq!(jobs[0].status, JobStatus::Active);
```

**`session_jsonl_round_trip`** — rewrite:
```rust
use daemoneye::session_store::{save_session, load_session};
use daemoneye::ai::Message;

let msgs = vec![
    Message::new("user", "hello"),
    Message::new("assistant", "hi there"),
    Message::new("user", "bye"),
];
save_session("test-sess", None, "test", &msgs, 2, "default", &[], false).unwrap();
let (loaded_msgs, meta) = load_session("test-sess").unwrap();
assert_eq!(loaded_msgs.len(), 3);
assert_eq!(meta.name, "test-sess");
```

**`session_index_persistence`** — rewrite:
```rust
use daemoneye::session_store::list_sessions;

// After save_session above, verify index entry.
let sessions = list_sessions().unwrap();
assert!(sessions.iter().any(|s| s.name == "test-sess"));
```

**`event_log_entry_format`** — rewrite:
```rust
use daemoneye::daemon::utils::log_event;

let fields = serde_json::json!({
    "type": "webhook_alert",
    "alert_name": "HighCPU",
    "severity": "critical"
});
log_event("webhook_alert", fields);

// Read events.jsonl and assert last line has expected fields.
let path = daemoneye::config::events_path();
let lines: Vec<&str> = std::fs::read_to_string(&path).unwrap().lines().collect();
let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
assert_eq!(last["event"], "webhook_alert");
assert_eq!(last["alert_name"], "HighCPU");
assert!(last["ts"].is_string());
```

**`event_log_append_read`** — rewrite similarly with multiple `log_event()` calls.

**`minimal_config_parsing`** and **`ghost_config_parsing`** — these are fine
as-is. They test TOML parsing of config snippets, not the production
`Config::load()` path. Could optionally add a test that calls
`Config::load()` with a temp config file, but the existing tests cover the
deser contract adequately.

### HOME isolation
`save_session()` and `log_event()` call `config::default_*_path()` which
resolves `~/.daemoneye/`. Tests must set `HOME` to a tempdir. Use the
existing `TEST_HOME_LOCK` pattern:
```rust
let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
std::env::set_var("HOME", tmp_dir.to_str().unwrap());
```

---

## A4. One real daemon-loop test

### Design

Spawn a daemon process bound to a tempdir socket. Connect via Unix socket,
send newline-delimited JSON, verify responses.

```rust
#[tokio::test(flavor = "current_thread")]
async fn daemon_ping_status_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("de-test.sock");
    let socket_str = socket.to_string_lossy().to_string();

    // Spawn daemon with --console so it doesn't fork.
    let mut child = std::process::Command::new("cargo")
        .args(["run", "--", "daemon", "--console"])
        .env("DE_SOCKET_PATH", &socket_str)
        .env("HOME", tmp.path().to_str().unwrap())
        .spawn()
        .expect("spawn daemon");

    // Wait for socket to appear (daemon needs to initialise).
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !socket.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }).await.expect("daemon socket did not appear in time");

    // Connect and ping.
    let mut stream = tokio::net::UnixStream::connect(&socket)
        .await.expect("connect to socket");

    // Send Ping request.
    let ping = serde_json::to_string(&daemoneye::ipc::Request::Ping).unwrap();
    stream.writable().await.unwrap();
    stream.try_write_all(format!("{}\n", ping).as_bytes()).unwrap();

    // Read response.
    let mut buf = Vec::new();
    stream.readable().await.unwrap();
    stream.try_read_to_end(&mut buf).unwrap();
    let resp: daemoneye::ipc::Response =
        serde_json::from_str(str::from_utf8(&buf).unwrap().trim()).unwrap();
    assert!(matches!(resp, daemoneye::ipc::Response::Ok));

    // Send Status request.
    buf.clear();
    let status = serde_json::to_string(&daemoneye::ipc::Request::Status).unwrap();
    stream.writable().await.unwrap();
    stream.try_write_all(format!("{}\n", status).as_bytes()).unwrap();

    stream.readable().await.unwrap();
    stream.try_read_to_end(&mut buf).unwrap();
    let resp: daemoneye::ipc::Response =
        serde_json::from_str(str::from_utf8(&buf).unwrap().trim()).unwrap();
    match resp {
        daemoneye::ipc::Response::DaemonStatus { uptime_secs, pid, .. } => {
            assert!(uptime_secs >= 0);
            assert!(pid > 0);
        }
        _ => panic!("expected DaemonStatus, got {:?}", resp),
    }

    // Cleanup.
    child.kill().unwrap();
}
```

### Concerns & mitigations

| Concern | Mitigation |
|---------|-----------|
| Daemon needs tmux running | `--console` mode may still try tmux setup. If so, mock tmux or use a config that skips tmux init. Alternative: test only Ping/Status before tmux init. |
| `cargo run` is slow in CI | Build binary once in `build.rs` or use `trycmd` crate. Or accept 15s overhead — it's one test. |
| Socket path not configurable | May need `DE_SOCKET_PATH` env override or `--socket` CLI flag. Check `config::default_socket_path()` for override mechanism. |
| Poisoned lock / fork safety | `--console` avoids fork. Test runs in single-threaded tokio runtime. |

**If tmux dependency is too heavy:** reduce scope to just verifying the
socket binds and accepts connections with a Ping → Ok round-trip. The Status
test can be a separate item if needed.

---

## A5. One webhook → audit-log test

### Approach: in-process via axum `Service`

The roadmap specifies: "POST a synthetic Alertmanager payload to an
in-process axum router (no socket bind required, axum is testable as a
`Service`)".

However, `webhook::start()` requires a full `WebhookState` with `SessionStore`,
`SessionCache`, and `ScheduleStore` — all heavy types that depend on tmux and
config dirs. The highest-value part of the pipeline is the
parse → dedup → mask → log path, not the HTTP transport.

### Revised approach: direct pipeline test

Make `process_alert` and `parse_payload` pub (visibility change noted in A1
audit). Test the pipeline directly:

```rust
#[tokio::test(flavor = "current_thread")]
async fn webhook_alert_to_event_log() {
    use daemoneye::webhook::{parse_payload, process_alert, WebhookState, InternalAlert};
    use daemoneye::ai::filter;

    // Init masking (safe — OnceLock, only sets once per process).
    filter::init_masking(&[]);

    // Set HOME to tempdir for events_path resolution.
    let tmp = tempfile::tempdir().unwrap();
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    std::env::set_var("HOME", tmp.path().to_str().unwrap());
    daemoneye::config::Config::ensure_dirs().unwrap();

    // Parse a synthetic Alertmanager payload.
    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "HighCPU",
                "severity": "critical",
                "instance": "web-01"
            },
            "annotations": {
                "summary": "CPU usage above 90%"
            }
        }]
    });
    let alerts = parse_payload(&body);
    assert_eq!(alerts.len(), 1);
    let alert = alerts[0].clone();
    assert_eq!(alert.alert_name, "HighCPU");
    assert_eq!(alert.source, "alertmanager");

    // Process alert through the pipeline (dedup → mask → log).
    // We need a minimal WebhookState — sessions/cache/schedule_store can be
    // empty since we only care about the log_event side effect.
    let state = WebhookState {
        config: daemoneye::config::Config::load().unwrap(),
        sessions: daemoneye::daemon::session::SessionStore::default(),
        cache: Arc::new(daemoneye::tmux::cache::SessionCache::default()),
        schedule_store: Arc::new(daemoneye::scheduler::ScheduleStore::new_empty()),
        dedup: Mutex::new(HashMap::new()),
        rate_limit: Mutex::new(HashMap::new()),
    };
    process_alert(alert, Arc::new(state)).await;

    // Assert events.jsonl contains the webhook_alert entry.
    let path = daemoneye::config::events_path();
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["event"], "webhook_alert");
    assert_eq!(last["alert_name"], "HighCPU");

    // Verify masking ran (no sensitive data leaked — our test payload has none,
    // but the path is exercised).
}
```

### Visibility changes needed for A5

| Item | File | Change |
|------|------|--------|
| `process_alert()` | `webhook.rs:362` | `async fn` → `pub async fn` |
| `parse_payload()` | `webhook.rs` | private → `pub fn` |
| `WebhookState` | `webhook.rs:67` | already `pub struct` |
| `WebhookState` fields | `webhook.rs:68-76` | already `pub` |

### Alternative: full HTTP test

If the direct-function approach is deemed insufficient, the full HTTP path
can be tested by:
1. Binding `webhook::start()` to a random port (`port: 0`)
2. Using `reqwest::Client` to POST the payload
3. Asserting `events.jsonl`

This requires `webhook::start()` to accept `port: 0` for random port
assignment. Currently it reads `config.webhook.port` — a one-line change to
support `0` → random port.

---

## A6. Mark C6 fully closed

### Changes

**Modified: `docs/ROADMAP.md`**
- §2.2 table: strike through C6 row, add "**Fixed**" label
- Phase A.5 section: add "✅ COMPLETE" marker
- Update test count in §1 metrics table (596 → new count)

---

## Execution Order

```
A1 (lib.rs + visibility changes)
  ├──→ A2 (replace IPC local types)
  ├──→ A3 (persistence tests via real APIs)
  └──→ A5 (webhook pipeline test)
A2 + A3 + A5 pass
  └──→ A4 (daemon-loop test) — independent, can run in parallel with A3/A5
A4 + A5 pass
  └──→ A6 (mark C6 closed, update ROADMAP.md)
```

A1 is the critical path blocker. A2–A5 can proceed in parallel once A1 lands.
A4 has the highest flakiness risk (process spawn, tmux dependency) and should
be implemented last.

---

## Expected Test Count

| Item | Before | After | Notes |
|------|--------|-------|-------|
| IPC round-trip | 3 | 3 | Same tests, different types |
| Schedule persistence | 1 | 1 | Rewritten, same count |
| Session persistence | 2 | 2 | Rewritten, same count |
| Event log | 2 | 2 | Rewritten, same count |
| Config parsing | 2 | 2 | Unchanged |
| Daemon loop | 0 | 1 | New |
| Webhook pipeline | 0 | 1 | New |
| **Integration total** | **10** | **11** | +2 new tests |
| **Grand total** | **596** | **607** | 586 unit + 11+10 integration |

Exit criteria of ≥ 600 is met. If desired, we can add 1-2 more tests:
  - `daemon_shutdown_loop` — Ping → Shutdown → socket gone
  - `webhook_dedup_suppression` — same fingerprint within window → no second log entry
  - `schedule_cancel_persistence` — add → cancel → load → assert cancelled status

---

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|-----------|
| `lib.rs` breaks `pub(crate)` invariants | Low | Audit each module; only promote what tests need |
| `log_event()` file lock contention in parallel tests | Medium | Tests run sequentially in `#[test]`; `log_event` uses `OpenOptions::append` which is atomic on Linux |
| `init_masking()` OnceLock — can only set once | Low | Call once in first webhook test; subsequent tests use already-initialized patterns |
| A4 daemon spawn flakes in CI (tmux not available) | Medium | Skip if tmux not found (`#[ignore]` or conditional); or reduce scope to socket bind + Ping only |
| `config::events_path()` writes to real `~/.daemoneye/` | High | Must set `HOME` before calling; `TEST_HOME_LOCK` serializes access |

---

## Files Modified Summary

| File | Action |
|------|--------|
| `src/lib.rs` | **Create** — module re-exports |
| `src/main.rs` | **Modify** — remove `mod *`, add `use daemoneye::*` |
| `src/webhook.rs` | **Modify** — `pub` on `process_alert`, `parse_payload` |
| `tests/integration.rs` | **Rewrite** — delete local types, use real APIs, add 2 new tests |
| `docs/ROADMAP.md` | **Modify** — mark C6 closed, update test count |

---

## Post-Implementation Audit (2026-05-10)

A re-read of A1–A6 against the actual tree turned up four issues the original
plan did not anticipate. They do not invalidate the structural fix to C6 —
production drift will now break compilation as intended — but they do mean
the "CI green, clippy clean" claim in ROADMAP §C1 is no longer accurate
post-A.5.

### Findings

**F1. Two real clippy errors in the A4 test.**
`tests/integration.rs:442` and `:460` call `stream.read(&mut buf).await.unwrap()`
where `buf` is `Vec::new()`. `read()` into a zero-capacity buffer returns
`Ok(0)` immediately, so as written the test would deserialize an empty byte
slice. The test is `#[ignore]`'d, which masks the bug from CI test runs but
not from `cargo clippy --all-targets -- -D warnings` (which now fails). If A4
is ever un-ignored it cannot pass.

**F2. `MutexGuard` held across `.await` in A5 test.**
`tests/integration.rs:497` and `:542` hold `daemoneye::TEST_HOME_LOCK.lock().unwrap()`
across `process_alert(...).await`. Clippy flags this; in practice it is
unlikely to deadlock because the test is single-threaded and there is only
one consumer of the lock per test, but the lint should be honoured: drop the
guard before the `.await`, or reach for `tokio::sync::Mutex` if cross-await
hold is genuinely required.

**F3. Pre-existing clippy lints in `src/`.**
Independent of A.5, `cargo clippy --all-targets` reports:
- 4 × `items after a test module` — items added below `mod tests { ... }` in
  `src/daemon/ghost.rs:34`, `src/daemon/session.rs:272`, `src/daemon/utils.rs:483`,
  `src/tmux/session.rs:283`. Mechanical fix: move the `#[cfg(test)] mod tests`
  block to the bottom of each file.
- 3 × `too many arguments` — `src/daemon/server.rs:1003`, `src/daemon/stream.rs:41`,
  `src/session_store.rs:173`. Sometimes worth a struct-of-args refactor;
  sometimes worth `#[allow(clippy::too_many_arguments)]` with a comment.
- 5 × `field assignment outside of initializer for an instance created with Default::default()` —
  cosmetic, mechanical fix.
- 3 × `Default impl missing` (`MarkdownRenderer`, `InputState`, `InputLine`) —
  cosmetic, mechanical fix.
- 2 × `assertion has a constant value`, 2 × collapsible-if, 1 × literal-bool
  assert, 1 × empty-line-after-doc-comment, 1 × module-same-name — all
  mechanical.

These are not new in A.5 but the ROADMAP §C1 "Fixed" claim implied a clean
clippy run that does not exist with `--all-targets`.

**F4. ROADMAP test count is off by one.**
ROADMAP §1 reports "598 passing + 1 ignored (587 unit + 11 integration + 1 ignored)".
Actual `cargo test` output: **587 unit + 11 integration + 1 ignored = 599 passing
+ 1 ignored = 600 total**. ROADMAP §A.5 exit-criteria text already says
"599 passing + 1 ignored" — §1 metrics table is the stale copy.

### What was *not* a problem

- **Production `unwrap()` calls.** A grep that excludes inline `mod tests { ... }`
  blocks finds **6** production `unwrap()` calls, all defensible (post-validation
  guarantees, statically-checked regex compilation). The earlier "124" number
  conflated test-only unwraps in non-`_tests.rs` files with production code.
  C4 is genuinely closed.
- **Library/binary split.** `src/lib.rs` and `src/main.rs` are clean. Visibility
  audit from §A1 was accurate; no over-exposure.
- **Integration test contract.** `daemoneye::ipc::{Request, Response}` is now
  the only source of truth — verified at `tests/integration.rs:8`. C6 is
  structurally closed.

---

## Phase A.7 — Post-implementation Cleanup

Small, mechanical follow-ups. Each item lists the file(s), the fix, and the
acceptance check. Estimated total effort: half a day.

### A.7.1 Fix A4 test `read()` bug and un-ignore if feasible

**Files:** `tests/integration.rs:442, 460`
**Change:** Replace `let mut buf = Vec::new(); stream.read(&mut buf).await.unwrap();`
with a framed read that respects the newline-delimited JSON wire format. The
daemon writes each `Response` followed by `\n`; use `tokio::io::BufReader` +
`read_line()`:
```rust
use tokio::io::{AsyncBufReadExt, BufReader};

let (rd, mut wr) = stream.into_split();
let mut rd = BufReader::new(rd);
let mut line = String::new();

wr.write_all(format!("{}\n", ping).as_bytes()).await.unwrap();
line.clear();
rd.read_line(&mut line).await.unwrap();
let resp: Response = serde_json::from_str(line.trim()).unwrap();
```
**Decision point:** if the daemon binary can be located reliably without
tmux running, drop `#[ignore]`. If tmux is required, keep `#[ignore]` but
document the requirement in a comment so the test is run-able locally.
**Accept:** `cargo clippy --all-targets -- -D warnings` no longer reports
`read amount is not handled` in the integration test crate.

### A.7.2 Drop std `MutexGuard` before `.await` in A5 test

**Files:** `tests/integration.rs:497, 542`
**Change:** Acquire `TEST_HOME_LOCK`, set `HOME`, then drop the guard before
calling `process_alert(...).await`. Two options:
1. Scope the guard: `{ let _l = …lock().unwrap(); std::env::set_var(…); }` —
   the env var stays set; the lock is released.
2. Bind to `_` so it drops immediately: `let _ = …lock().unwrap();` (works
   here because we only need the lock long enough to mutate `HOME`).
**Accept:** `cargo clippy --all-targets` no longer reports `this MutexGuard
is held across an await point` in `tests/integration.rs`.

### A.7.3 Move `#[cfg(test)] mod tests` to bottom of file

**Files:** `src/daemon/ghost.rs`, `src/daemon/session.rs`, `src/daemon/utils.rs`,
`src/tmux/session.rs`
**Change:** Move the `mod tests { ... }` block (and any leading `#[cfg(test)]
use …` lines that belong with it) to the end of the file. No semantic change.
**Accept:** zero `items after a test module` warnings.

### A.7.4 Address `too_many_arguments` warnings

**Files:** `src/daemon/server.rs:1003`, `src/daemon/stream.rs:41`,
`src/session_store.rs:173`
**Change:** For each, choose between (a) `#[allow(clippy::too_many_arguments)]`
with a one-line comment explaining why a struct refactor is overkill, or
(b) introducing a small args struct. Default to (a) unless the function is
called from > 3 places — refactoring a single-call-site function for clippy
appeasement is not worth it.
**Accept:** zero `too many arguments` warnings.

### A.7.5 Mechanical clippy cleanup

**Change:** Run `cargo clippy --all-targets --fix --allow-dirty` and review
the diff before committing. Manually fix anything `--fix` does not handle:
- `MarkdownRenderer`/`InputState`/`InputLine` `Default` impls
- field-assignment-after-Default cases (collapse into struct-init form)
- collapsible-if, constant-assertion, literal-bool-assert, empty-line-after-doc
**Accept:** `cargo clippy --all-targets -- -D warnings` exits zero.

### A.7.6 Fix ROADMAP §1 test count

**Files:** `docs/ROADMAP.md` §1 metrics table
**Change:** "598 passing + 1 ignored (587 unit + 11 integration + 1 ignored)"
→ "599 passing + 1 ignored (587 unit + 12 integration including 1 ignored)".
Also update §C5 "586 unit + 11 integration" if it appears there.
**Accept:** numbers in §1, §A, and §A.5 all match `cargo test` output.

### A.7.7 Add a CI gate that catches this class of regression

**Optional but recommended.** Add a CI step (or a pre-push hook) that runs
`cargo clippy --all-targets -- -D warnings`. A.5 silently regressed clippy
because nothing in CI denies warnings on the test crates.
**Accept:** if the gate is added, document it in CLAUDE.md "Build & Test"
so future contributors see the contract.

---

## Deferred

### C5 — files trending past 1000 lines
Six files are > 1000 lines (`server.rs` 1632, `ai/tools.rs` 1479, `config.rs`
1381, `daemon/background.rs` 1369, `daemon/executor/file_ops.rs` 1328,
`cli/render.rs` 1245). Some have natural seams (config.rs has ~60 inline test
cases that could move to a `_tests.rs` sibling); others may be irreducible.

**Plan:** treat as an end-of-project sweep — audit each file, identify the
natural seams or document why none exist, then split or `#[allow]` per file
with explicit rationale. Not a hygiene-sprint candidate; the work is
file-specific and benefits from being done in one focused pass once the
feature roadmap is settled.

### C8 — `anyhow` everywhere; no `thiserror` at module boundaries
Confirmed via grep: zero `thiserror::Error` impls in `src/`. Recovery
decisions cannot be made by callers — every error is opaque.

**Plan:** scope a separate proposal that picks 2–3 module boundaries where
typed errors actually unlock recovery (likely candidates: `webhook` —
distinguish parse vs dedup vs IO; `ai` — distinguish provider error class;
`scheduler` — distinguish cron-parse vs persistence). Cosmetic conversion
across the whole tree is not worth it. Revisit after Phase B feature work
exposes which seams matter.
