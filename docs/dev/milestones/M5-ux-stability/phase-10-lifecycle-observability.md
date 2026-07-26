# Phase 10: Lifecycle Observability — Attribute Every Event to a Process

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-08 (instance lock) — the startup identity line reports the
lock outcome
**Estimated diff:** ~130 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Make a repeat of the 2026-07-25 incident diagnosable in minutes instead of hours:
stamp the emitting PID on every event record, surface a logger-init failure
instead of discarding it, and log one startup identity line saying which binary,
PID, and log destination this daemon is using.

## Architecture references

Read before starting:

- `docs/design/daemon-instance.md` § 1 — the incident timeline. The two
  `daemon_stop` records 1.3 ms apart were indistinguishable because neither
  carried a PID; that is the specific gap task 1 closes.
- `docs/design/daemon-instance.md` § 4.3 — the event-attribution gap.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### `log_event` — `src/daemon/utils/event_log.rs:10-45`

```rust
pub fn log_event(event: &str, mut fields: serde_json::Value) {
    use std::io::Write;

    let path = crate::config::current_event_segment_path();
    let ts = chrono::Utc::now().to_rfc3339();

    if let Some(obj) = fields.as_object_mut() {
        // Prepend ts + event so they appear first in the line.
        let mut record = serde_json::Map::new();
        record.insert("ts".to_string(), serde_json::Value::String(ts));
        record.insert(
            "event".to_string(),
            serde_json::Value::String(event.to_string()),
        );

        // Take ownership of the fields from the caller's object
        let drained = std::mem::take(obj);
        for (k, v) in drained {
            record.insert(k, v);
        }
        …
```

Two things to notice. `ts` and `event` are inserted first so they lead the line,
then caller fields are merged over the top — so a caller field named `ts` would
win. And the whole body is inside `if let Some(obj) = fields.as_object_mut()`,
meaning a non-object `fields` value silently writes **nothing**.

### The only producer of a `pid` field — `src/daemon/mod.rs:478-486`

```rust
    log_event(
        "daemon_start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "session": initial_session.as_deref().unwrap_or(""),
            "pid":     std::process::id(),
            "socket":  default_socket_path().display().to_string(),
        }),
    );
```

`grep -rn '"pid"' --include=*.rs src/` returns this line and nothing else — no
code anywhere *reads* a `pid` field out of an event record, so adding one
globally breaks no consumer.

### The discarded logger init — `src/daemon/mod.rs:334-344`

```rust
    let _ =
        env_logger::Builder::from_env(env_logger::Env::new().filter_or("DAEMONEYE_LOG", "info"))
            .write_style(env_logger::WriteStyle::Never)
            .format(|buf, record| {
                use std::io::Write;
                let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
                writeln!(buf, "{} {:5} {}", ts, record.level(), record.args())
            })
            .try_init();
```

## Spec

### 1. Stamp the PID on every event record

In `log_event` (`src/daemon/utils/event_log.rs`), insert a `pid` field
immediately after `event`, before the caller's fields are drained in:

```rust
        record.insert(
            "pid".to_string(),
            serde_json::Value::from(std::process::id()),
        );
```

Placement matters twice over. Leading position keeps `ts` / `event` / `pid` as a
stable prefix on every line, which is what makes `grep` and `jq` over a segment
readable. And inserting *before* the drain means an explicit caller-supplied
`pid` still overrides it rather than producing a duplicate key — which is what
makes task 2 a cleanup rather than a bug fix.

Update the doc comment's second paragraph to name the new always-present field:

```rust
/// Each call appends one JSON object per line.  The top-level fields
/// `ts` (ISO-8601 UTC), `event` (event type name), and `pid` (the emitting
/// process) are always present.  Additional fields are provided by the caller
/// as a `serde_json::Value` object and merged in.
```

### 2. Drop the now-redundant explicit `pid` from `daemon_start`

In `src/daemon/mod.rs:478-486`, delete the `"pid": std::process::id(),` line from
the `json!` block. Task 1 supplies it for every event including this one, and
STANDARDS § 2.2 does not want the duplicate. The emitted record is unchanged.

### 3. Surface a logger-init failure

Replace the `let _ = …try_init();` at `src/daemon/mod.rs:334` with a bound result
and an `eprintln!` on failure:

```rust
    if let Err(e) =
        env_logger::Builder::from_env(env_logger::Env::new().filter_or("DAEMONEYE_LOG", "info"))
            .write_style(env_logger::WriteStyle::Never)
            .format(|buf, record| {
                use std::io::Write;
                let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
                writeln!(buf, "{} {:5} {}", ts, record.level(), record.args())
            })
            .try_init()
    {
        eprintln!("daemoneye: logger already initialised: {e} — continuing with the existing logger");
    }
```

`eprintln!` and not `log::warn!`, deliberately: the thing that failed *is* the
logger, so a `log::` call is not guaranteed to go anywhere. This must not become
a `bail!` — `try_init` failing means a logger already exists (the normal case
when a test binary initialises one), and that is not fatal.

### 4. One startup identity line

In `run_daemon`, immediately after the `InstanceLock` acquisition added by phase
08 (so it is the first thing a successful start writes to `daemon.log`), log:

```rust
    log::info!(
        "daemoneye {} starting — PID {}, exe {}, log {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string()),
        log_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<stdout>".to_string()),
    );
```

Each field answers a question the incident forensics could not. **PID** ties the
log to the event records from task 1. **exe** distinguishes a
`~/.cargo/bin/daemoneye` instance from a `./target/release/daemoneye` one — the
two builds involved on 2026-07-25. **log** makes it explicit when output is going
to a terminal rather than `daemon.log`, which is precisely why the second
instance left no trace in the log file.

`log_file` is the `Option<PathBuf>` parameter of `run_daemon`; it is still in
scope at this point. Read it by reference — do not move or consume it, the
`dup2` block above already borrowed it and later code does not, but a move here
would still be a needless change.

### 5. Document the guarantee

In `CLAUDE.md`, in the `## Important Invariants` list:

```markdown
- Every `events.jsonl` record carries `ts`, `event`, and `pid` as a leading
  prefix; `log_event` stamps `pid` itself, so call sites must not pass one.
```

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes: existing tests plus the 3 new ones below.
- [ ] `grep -rn '"pid"' --include=*.rs src/` shows the insert in
      `event_log.rs` and **no** occurrence in `src/daemon/mod.rs`.
- [ ] `grep -n "let _ =" src/daemon/mod.rs` does not match the `try_init` call.
- [ ] A real daemon start emits a `daemon_start` record containing `pid`, and its
      first `daemon.log` line is the identity line (End-to-end verification).

## Test plan

In `src/daemon/utils/event_log.rs`'s existing `#[cfg(test)] mod tests`. There is
already a `log_event_writes_today_segment` test at `event_log.rs:506` using a
`with_test_home(...)` helper — use that same helper, and note that anything
touching `HOME` must hold `crate::TEST_HOME_LOCK` (`with_test_home` already
does; do not re-acquire it or you will deadlock).

- `log_event_stamps_emitting_pid` — writes an event, reads the segment back,
  asserts the parsed record's `pid` equals `std::process::id()`.
- `log_event_prefix_order_is_ts_event_pid` — asserts the raw serialized line's
  first three keys are `ts`, `event`, `pid` in that order. Assert on key order in
  the string (e.g. the byte offsets of `"ts"`, `"event"`, `"pid"` are strictly
  increasing), not on the parsed map — `serde_json::Map` preserves insertion
  order only with the `preserve_order` feature, which this crate does not enable,
  so a parsed-map assertion would be testing nothing.
- `log_event_caller_pid_overrides_stamp` — passing an explicit
  `{"pid": 999_999}` yields a record whose `pid` is `999999`, not the process's.
  This pins the insert-before-drain ordering from task 1; a mutation that moves
  the insert after the drain must fail this test.

No test for tasks 3 or 4: `try_init`'s failure branch cannot be triggered
deterministically from inside a test binary that already has a logger, and a
single `log::info!` line is not testable behavior (STANDARDS § 3.2, plumbing).
Both are covered by the E2E below.

## End-to-end verification

The event log and `daemon.log` are real artifacts the running binary writes.
Quote actual output in the Update Log.

```bash
cargo build --release
./target/release/daemoneye daemon
sleep 3

# 1. The identity line is the first thing this start wrote.
grep -n "starting — PID" ~/.daemoneye/var/log/daemon.log | tail -1
# expect e.g.:
#   2026-07-26T.. INFO  daemoneye 0.9.9 starting — PID 12345,
#   exe /home/matt/src/daemoneye/target/release/daemoneye,
#   log /home/matt/.daemoneye/var/log/daemon.log

# 2. Every record in today's segment carries a pid.
SEG=~/.daemoneye/var/log/events/events-$(date -u +%Y%m%d).jsonl
python3 -c "
import json,sys
n=miss=0
for line in open('$SEG'):
    line=line.strip()
    if not line: continue
    n+=1
    if 'pid' not in json.loads(line): miss+=1
print(f'records={n} missing_pid={miss}')
"
# expect missing_pid=0

# 3. daemon_start has exactly one pid field and it matches the PID file.
grep '"event":"daemon_start"' "$SEG" | tail -1
cat ~/.daemoneye/var/run/daemoneye.pid

# 4. Stop and confirm daemon_stop is now attributable — the record that was
#    ambiguous during the 2026-07-25 incident.
./target/release/daemoneye stop
grep '"event":"daemon_stop"' "$SEG" | tail -1
# expect a pid field naming the process that just stopped
```

Step 4 is the acceptance test for `daemon-instance.md` § 4.3. Under the old code
that record was `{"ts":…,"event":"daemon_stop","reason":"SIGTERM"}` with no way
to tell which of two processes emitted it.

## Authorizations

- [x] May edit `CLAUDE.md` § "Important Invariants" (task 5).
- [ ] No new dependencies.
- [ ] No `unsafe`.

## Out of scope

- **Do not add a `pid` to `Response::DaemonStatus` or any IPC type.** It already
  carries one; the wire protocol is not this phase's business.
- **Do not change where `daemon.log` is written, or tee lifecycle output to it
  when `--console` is set.** The identity line (task 4) reports the destination;
  changing the destination is a larger behavior change and is not scoped here.
- **Do not add PIDs to the per-session JSONL logs** under
  `var/log/sessions/`. Events only.
- **Do not rotate, reformat, or migrate existing event segments.** Old records
  keep having no `pid`; that is expected and needs no backfill.
- **Do not touch `sweep_event_segments` or the retention logic.**
- **Do not change `log_event`'s signature** or its silent-failure contract — the
  doc comment's "Errors are silently discarded — logging must never crash the
  daemon" stays true and stays written.
- **Do not "fix" the `if let Some(obj) = fields.as_object_mut()` guard** that
  makes a non-object `fields` write nothing. It is pre-existing, no caller passes
  a non-object, and changing it is not in scope.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
