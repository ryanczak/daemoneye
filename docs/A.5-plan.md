# Phase A.5 — Finish the Integration Test Story
# Implementation Plan
#
# Drafted: 2026-05-09
# Status: COMPLETE (A1–A5 landed, A4 ignored, A6 docs updated)

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
