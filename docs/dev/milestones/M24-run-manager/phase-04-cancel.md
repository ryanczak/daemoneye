# Phase 04: cooperative cancellation — the `stop-<run>` sentinel

**Milestone:** M24 — Run Manager
**Status:** todo
**Depends on:** phase-01 (`RunId`) and phase-03 (`CancelReason`, `Cancellation`).
Both **done**.
**Estimated diff:** ~520 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Make a run cancellable **from any terminal**, not just from the chat client that
started it. Five pieces, all in `src/daemon/run/cancel.rs` except the last two:

1. a run-keyed cancel registry whose signal carries a `CancelReason`
   (phase-03 shipped the type; nothing sets it yet),
2. the filesystem sentinel `var/run/stop-<run-id>` and its read/write/clear
   helpers,
3. an async watcher that polls the sentinel and flips the run's signal,
4. a startup sweep for sentinels left behind by a dead daemon, and
5. `daemoneye stop <run>` — which must keep working as
   `daemoneye stop` (stop the daemon) when given no argument.

**No run loop consumes the signal yet.** Nothing calls `register_run` from a
live turn; phases 05–08 do that when they build runs. This phase ships the
mechanism and proves it end to end through the CLI door.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3, table row 3: *"**Cooperative
  cancellation**: checked at turn top and inside the model-call `select!`;
  result is a real `Cancelled{reason, stage, turns_done}`; filesystem sentinel
  `var/run/stop-<run>` polled at 500 ms so `daemoneye stop <run>` works from any
  terminal"* — anchors `mcp/src/stop_watcher.rs:20-45`. **The "checked at turn
  top and inside the model-call `select!`" half is phases 05–08**, not this one.
- `docs/dev/milestones/M24-run-manager/README.md` § Exit criteria, bullet 4.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib` → **1705 passed; 0 failed; 4 ignored** before you start.
- `cargo test --lib daemon::run::` → **39 passed**.
- `cargo test --lib daemon::cancel::` → **6 passed** (the chat-turn module this
  phase does **not** touch — F2).
- `test -f src/daemon/run/cancel.rs` → **false**.

## Current state

`src/daemon/run/` holds `mod.rs`, `turn_log.rs`, `log_read.rs`, `result.rs`.
`grep -rc 'stop_sentinel\|fn register_run\|watch_stop' src/` matches **nothing**.
`Commands::Stop` in `src/main.rs:69` is a **unit variant with no fields**
(`grep -c 'Stop,' src/main.rs` → 1).

## Measured facts — executed 2026-09-10, not reasoned about

### F1 — the cancel machinery already exists, keyed by session

`src/daemon/cancel.rs` (160 lines) is a `tokio::sync::watch` cancel signal with
a global registry, **ported from the same rexyMCP source this phase ports from**.
Quoted so you build the run-keyed version in its image rather than inventing one:

```rust
static REGISTRY: OnceLock<Mutex<HashMap<String, CancelHandle>>> = OnceLock::new();

/// Register a turn's cancel handle. Returns a guard that deregisters on
/// drop, so every exit path of the turn cleans up.
pub fn register_turn(session_id: &str) -> (TurnCancelGuard, CancelSignal) {
    let (handle, signal) = CancelSignal::new();
    registry().lock().unwrap_or_log().insert(session_id.to_string(), handle);
    (TurnCancelGuard { session_id: session_id.to_string() }, signal)
}

/// Flip the cancel signal for a session's in-flight turn, if any.
/// Returns whether a turn was found.
pub fn cancel_turn(session_id: &str) -> bool { … }

impl Drop for TurnCancelGuard {
    fn drop(&mut self) {
        registry().lock().unwrap_or_log().remove(&self.session_id);
    }
}

impl CancelSignal {
    pub fn new() -> (CancelHandle, CancelSignal) {
        let (tx, rx) = watch::channel(false);
        (CancelHandle { tx }, CancelSignal { rx })
    }
    pub fn is_cancelled(&self) -> bool { *self.rx.borrow() }
    /// Resolve when the signal is flipped. If the sender is dropped before
    /// the flip, park forever.
    pub async fn cancelled(&mut self) {
        loop {
            if *self.rx.borrow() { return; }
            match self.rx.changed().await {
                Ok(_) => {}
                Err(_) => std::future::pending::<()>().await,
            }
        }
    }
}
```

Note `.unwrap_or_log()` on every lock — the project's poison-recovery idiom
(`src/util.rs`). Use it; do **not** use `.unwrap()` on a lock.

### F2 — why you build a second module instead of editing that one

`daemon::cancel`'s channel carries a **`bool`**. A run's cancellation must carry
**which** `CancelReason` fired, so its channel carries
`Option<CancelReason>` — a different type, not a widening. Changing
`daemon::cancel` would touch the live chat-turn path
(`src/daemon/stream.rs:88-94,178`) and its 6 tests for no benefit to this phase.

**So: create `src/daemon/run/cancel.rs` and leave `src/daemon/cancel.rs`
untouched.** `cargo test --lib daemon::cancel::` must still report **6 passed**,
and § Authorizations forbids editing it. This is a *deliberate*, recorded
duplication: when phase 06 makes a chat turn a `Run`, the session-keyed module
collapses into this one. Do not do that collapse now, and do not add a "unify
them" TODO.

### F3 — the sentinel-watcher anchor, verbatim

`rexyMCP:mcp/src/stop_watcher.rs:14-52`. Copy two things exactly: the **500 ms
constant** and the **injectable `poll` parameter** (it is what makes the tests
deterministic rather than sleepy):

```rust
/// How often the serve-side watcher checks for `.rexymcp/stop`. Stop latency is
/// bounded by this (a human waits at most this long after `rexymcp stop`).
pub const STOP_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// … `poll` is injectable for tests.
pub async fn watch_stop_sentinel_single(repo_path: PathBuf, cancel: CancelHandle, poll: Duration) {
    loop {
        tokio::time::sleep(poll).await;
        if stop::sentinel_present(&repo_path) {
            cancel.cancel();
            let _ = stop::clear_sentinel(&repo_path);
            return;
        }
    }
}
```

The daemoneye watcher differs in one way: it is **per-run**, not global. It
exits without firing once the run is no longer registered, so the task never
outlives its run — that is the `registry.is_running(&run_id)` check in the
anchor's other function.

### F4 — `var/run` is **not** actually cleared at startup, whatever the lifecycle table says

`src/config/lifecycle.rs:156-162` records `var/run` as
`intent: ClearAtStartup, implemented: Implemented`. **Nothing implements it.**
Measured: the only startup cleanup loop is for `var/log/pipe`
(`src/daemon/mod.rs:531-541`), and `var/run` holds files that must survive —
`pane_prefs.json`, `schedules.json`, `daemoneye.pid`, and the `shells/` subtree
that phase M20 explicitly exempted.

**Consequence for this phase: you must sweep your own sentinels.** Do not
assume the lifecycle row does it. The sweep has a worked example ten lines long
in the same file you will edit — copy its shape:

```rust
    // R1: clean up any pipe log files left behind by a previous daemon run so
    // stale content from a different session is never shown to the AI.
    if let Ok(entries) = std::fs::read_dir(crate::config::pipe_log_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("de-pipe-") && name_str.ends_with(".log") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
```

(The inaccurate lifecycle row is **not yours to fix** — phase-10 owns the
lifecycle work. Do not edit `src/config/lifecycle.rs`.)

### F5 — no gate fires for a new file inside `var/run`

The five-gate rule from 02a applies to a new **directory**. This phase adds
files *inside* an existing one, so:

- `src/config/lifecycle.rs` — `var/run` already has its row (F4). No new row.
- `src/config/runtime_tree.rs` — `run/` is already a leaf
  (`runtime_tree.rs:238`). No new leaf, so
  `render_matches_shipped_asset` and `every_policy_path_appears_in_tree` do not
  move.
- `src/config/path_audit.rs` — its completeness test
  (`inventory_contains_all_config_constructors`, `path_audit.rs:564`) walks a
  **hand-written `Vec<fn() -> PathBuf>`**. `stop_sentinel_path` takes a `&RunId`
  argument, so it is not that shape and cannot be added to the list. No
  `INVENTORY` entry, no edit.

Measured baselines that must not move: `config::lifecycle` **5**,
`config::path_audit` **24**.

### F6 — `daemoneye stop` already means "stop the daemon"

`src/main.rs:68-69`:

```rust
    /// Stop the background daemon
    Stop,
```

dispatched at `src/main.rs:417-419` to `cli::run_stop()`, which connects to the
socket and sends `Request::Shutdown`
(`src/cli/commands/lifecycle.rs:53-75`). The design asks for
`daemoneye stop <run>`, so the variant gains an **optional positional**:
no argument keeps today's behaviour byte for byte.

**The run arm must not connect to the socket.** The whole point is that it works
from any terminal, including when the daemon is wedged — it writes a file and
exits. `run_stop()`'s `connect().await` / `std::process::exit(1)` path is for
the no-argument case only.

### F7 — `RunId::parse` is the security boundary, and it is already written

The run id becomes a **path segment** (`var/run/stop-<id>`), so a caller-supplied
string must go through `RunId::parse` (`src/daemon/run/mod.rs:41`), which
`bail!`s on anything but 16 lowercase hex — it already rejects
`../../etc/passwd`, `a/b` and backslash forms, and phase-01 pins that in
`run_id_parse_rejects_path_traversal_and_separators`.

**So `stop_sentinel_path` takes `&RunId`, never `&str`** — the type is the
validation. The string-accepting entry point is one function (`request_stop`),
which parses first and propagates the error.

### F8 — deterministic async tests, no sleeps

`tokio` is a dev-dependency with **`test-util`** (`Cargo.toml:54`), and
`#[tokio::test(start_paused = true)]` is an established idiom here — 10 sites,
e.g. `src/daemon/utils/keepalive.rs:65` and `src/daemon/mod.rs:1267`.

Use it for every watcher test: start paused, `tokio::time::advance(...)` past
the poll interval, then assert. **Do not write a test that sleeps for real and
asserts afterwards** — bug-08a-1 in the last milestone was exactly that (an
unpolled assertion on an async condition) and it was flaky.

Tests that touch `~/.daemoneye/` must take `crate::test_home_guard()`
(`src/lib.rs:45`) — **not** the raw lock — and point `HOME` at a `tempfile`
tempdir. `src/daemon/run/turn_log.rs:110-120` has this exact `HomeGuard` RAII
shape already written for this module; reuse it.

## Spec

1. **Create `src/daemon/run/cancel.rs`** and declare it `mod cancel;` (private)
   in `src/daemon/run/mod.rs`, re-exporting the public items beside the existing
   `pub use` block at `mod.rs:485-492`.

2. **The reason-carrying signal.** Mirror F1's shape with
   `watch::channel::<Option<CancelReason>>(None)`:
   - `RunCancelHandle` with `pub fn cancel(&self, reason: CancelReason)`
     (ignores the all-receivers-dropped send error, as F1 does);
   - `RunCancelSignal` — `Clone`, with `pub fn reason(&self) ->
     Option<CancelReason>`, `pub fn is_cancelled(&self) -> bool`, and
     `pub async fn cancelled(&mut self) -> CancelReason` resolving to the reason
     that fired. **Keep F1's park-forever-on-dropped-sender branch**: a dropped
     handle must not read as a cancellation.
   - `pub fn new() -> (RunCancelHandle, RunCancelSignal)`.

3. **The run registry**, its own `static OnceLock<Mutex<HashMap<String,
   RunCancelHandle>>>` — separate from `daemon::cancel`'s (F2), keyed by
   `RunId::as_str()`:
   - `pub fn register_run(id: &RunId) -> (RunCancelGuard, RunCancelSignal)`;
   - `pub fn cancel_run(id: &RunId, reason: CancelReason) -> bool` — `false`
     when no such run is registered;
   - `RunCancelGuard` with a `Drop` that deregisters, exactly as F1's
     `TurnCancelGuard` does.

4. **The sentinel.**
   - `pub fn stop_sentinel_path(id: &RunId) -> PathBuf` →
     `config::var_run_dir().join(format!("{STOP_SENTINEL_PREFIX}{id}"))` with
     `pub const STOP_SENTINEL_PREFIX: &str = "stop-";`. Take `&RunId`, never
     `&str` (F7).
   - `pub fn sentinel_present(id: &RunId) -> bool`.
   - `pub fn clear_sentinel(id: &RunId)` — best-effort, warns rather than
     erroring, like `append_turn_event` does (`turn_log.rs:56-60`).
   - `pub fn request_stop(raw_id: &str) -> anyhow::Result<PathBuf>` — the one
     string-accepting entry point: `RunId::parse(raw_id)?`, create
     `var_run_dir()` if absent, write the sentinel (empty file is fine), return
     its path. This is what the CLI calls.

5. **The watcher.** `pub const STOP_POLL_INTERVAL: Duration =
   Duration::from_millis(500);` and

   ```rust
   pub async fn watch_stop_sentinel(id: RunId, handle: RunCancelHandle, poll: Duration)
   ```

   looping exactly as F3 does: `sleep(poll)`, then **first** return if the run is
   no longer registered (so the task never outlives its run), **then** if
   `sentinel_present(&id)` → `handle.cancel(CancelReason::UserStop)`,
   `clear_sentinel(&id)`, return. `poll` is a parameter, not the constant, so
   tests inject their own (F3, F8).

   **`UserStop` is the reason a sentinel fires** — a human at a terminal ran
   `daemoneye stop <run>`. `ClaudeStop` is for an in-process
   `cancel_run(...)` by the daemon itself and is not produced by this phase.

6. **Startup sweep.** In `src/daemon/mod.rs`, immediately after the existing
   pipe-log cleanup block (F4's quote, `mod.rs:531-541`), add the same shape for
   sentinels: read `config::var_run_dir()`, and remove only entries whose file
   name **starts with `STOP_SENTINEL_PREFIX`**. Extract the decision into a pure
   helper in `run/cancel.rs` — `pub fn is_stop_sentinel_name(name: &str) -> bool`
   — so it is unit-testable without a daemon, and call it from the loop.

   **The negative case is the point**: `daemoneye.pid`, `daemoneye.sock`,
   `schedules.json`, `pane_prefs.json` and the `shells` directory must all
   survive. A sweep that clears `var/run` wholesale would delete the schedule
   store and every shell socket.

7. **`daemoneye stop [<run>]`.** In `src/main.rs`, change `Stop` to carry an
   optional positional:

   ```rust
   /// Stop the background daemon, or cancel one run by id
   Stop {
       /// Run id to cancel. Omit to stop the daemon.
       run: Option<String>,
   },
   ```

   Dispatch: `None` → today's `cli::run_stop().await?` unchanged; `Some(id)` →
   a new `cli::run_stop_run(&id)` that calls `request_stop`, prints the run id
   and sentinel path on success, and on a parse error prints the error to
   stderr and exits non-zero. Put `run_stop_run` beside `run_stop` in
   `src/cli/commands/lifecycle.rs` and export it the way `run_stop` is exported.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `test -f src/daemon/run/cancel.rs && echo 1 || echo 0` → **1** (now `0`).
- [ ] `grep -c '^mod cancel;' src/daemon/run/mod.rs` → **1** (now `0`), and
      `grep -c '^pub mod ' src/daemon/run/mod.rs` → **0** (still).
- [ ] Declared exactly once each across `src/daemon/run/*.rs`:
      `pub fn register_run` → **1**, `pub fn cancel_run` → **1**,
      `pub fn stop_sentinel_path` → **1**, `pub fn request_stop` → **1**,
      `pub async fn watch_stop_sentinel` → **1**,
      `pub fn is_stop_sentinel_name` → **1** (each **0** now).
- [ ] **The signal carries a reason, not a bool:**
      `grep -c 'watch::channel::<Option<CancelReason>>\|watch::channel(None)' src/daemon/run/cancel.rs`
      → **1 or more**, and `grep -c 'watch::channel(false)' src/daemon/run/cancel.rs`
      → **0**.
- [ ] **The sentinel path is typed, not stringly (F7):**
      `grep -c 'fn stop_sentinel_path(id: &RunId)' src/daemon/run/cancel.rs` → **1**,
      and `grep -c 'fn stop_sentinel_path(.*&str' src/daemon/run/cancel.rs` → **0**.
- [ ] **`request_stop` parses before it writes:**
      `grep -c 'RunId::parse' src/daemon/run/cancel.rs` → **1 or more** (now: no
      such file).
- [ ] **The poll interval is injectable, not hardcoded in the loop:**
      `grep -c 'poll: Duration' src/daemon/run/cancel.rs` → **1 or more**, and
      `grep -c 'STOP_POLL_INTERVAL' src/daemon/run/cancel.rs` → **1 or more**.
- [ ] **`daemon/cancel.rs` is untouched (F2)** — against the pre-phase baseline
      commit `f8b49af`:
      `git diff --name-only f8b49af -- src/daemon/cancel.rs | wc -l` → **0**,
      and `cargo test --lib daemon::cancel::` → **6 passed** (now 6).
      *(Preservation criterion.)*
- [ ] **`Stop` gained its optional positional:** `grep -c 'Stop,' src/main.rs`
      → **0** (now `1`) and `grep -c 'run: Option<String>' src/main.rs` → **1 or
      more** (now `0`).
- [ ] **Sweeping is filtered, never wholesale:**
      `grep -c 'remove_dir_all' src/daemon/run/cancel.rs` → **0**, and the sweep
      site in `src/daemon/mod.rs` calls the predicate:
      `grep -c 'is_stop_sentinel_name' src/daemon/mod.rs` → **1 or more** (now `0`).
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/cancel.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0**. *(`.unwrap_or_log()` on locks is required and does not match this
      pattern — F1.)*
- [ ] **No real sleeps in the tests (F8):**
      `grep -c 'start_paused = true' src/daemon/run/cancel.rs` → **1 or more**,
      and `grep -cE 'thread::sleep|sleep\(Duration::from_secs' src/daemon/run/cancel.rs`
      → **0**.
- [ ] `cargo test --lib daemon::run::` reports **50 or more** passing,
      `0 failed` (now **39**). *Arithmetic: 39 existing + the 11 named tests in
      the Test plan = 50.*
- [ ] **Preservation, each must not move:** `config::lifecycle` **5**,
      `config::path_audit` **24**, `daemon::run::log_read` **11**,
      `daemon::run::result` **11**, `shell::registry` **27** (F5).
- [ ] **Nothing leaked into the developer's real HOME:**
      `ls ~/.daemoneye/var/run/ | grep -c '^stop-'` → **0** after the suite runs.
- [ ] Full lib suite `0 failed` on **three consecutive runs**.
- [ ] All four gates pass.

## Test plan

Eleven tests, names pinned; placement is yours (`cancel.rs`'s own
`#[cfg(test)] mod tests`). Every test touching `~/.daemoneye/` takes
`crate::test_home_guard()` + a tempdir `HOME` (F8); every watcher test is
`#[tokio::test(start_paused = true)]` with `tokio::time::advance`.

- `stop_sentinel_path_is_var_run_stop_plus_the_id` — the path ends in
  `var/run/stop-<the 16 hex chars>`; assert the **full tail**, not just that it
  is non-empty.
- `request_stop_rejects_an_id_that_is_not_a_run_id` — **the security negative**:
  `../../etc/passwd`, `a/b`, a backslash form and an uppercase id all return
  `Err`, and **no file is created anywhere under `var/run`** (assert the
  directory listing is unchanged — an error that still wrote a file is the bug
  this test exists for).
- `request_stop_writes_a_sentinel_the_watcher_can_see` — round-trip:
  `request_stop` then `sentinel_present` is `true`; `clear_sentinel` then it is
  `false`.
- `run_cancel_signal_reports_the_reason_that_fired` — `cancel(UserStop)` makes
  `reason()` `Some(UserStop)`, and `cancelled().await` resolves to `UserStop`.
  Assert the **variant**, not just `is_some()`.
- `run_cancel_dropped_handle_does_not_cancel` — F1's park-forever branch: with
  the handle dropped before any flip, `cancelled()` does not resolve inside a
  short `tokio::time::timeout`.
- `register_run_then_cancel_run_flips_that_runs_signal` — and
  `cancel_run` returns `true`.
- `cancel_run_on_an_unregistered_run_is_false` — no panic, no side effect.
- `run_cancel_guard_drop_deregisters` — after the guard drops, `cancel_run`
  returns `false`.
- `watcher_fires_user_stop_and_clears_the_sentinel` — sentinel written first,
  watcher spawned, time advanced past one poll: the signal reads
  `Some(UserStop)` **and** `sentinel_present` is now `false`.
- `watcher_exits_without_firing_when_the_run_is_gone` — no sentinel; drop the
  guard; advance time well past several polls; the signal is **not** cancelled
  and the task has returned.
- `sweep_predicate_matches_only_stop_sentinels` — `is_stop_sentinel_name` is
  `true` for `stop-<valid id>` and **false** for every real `var/run` neighbour:
  `daemoneye.pid`, `daemoneye.sock`, `schedules.json`, `pane_prefs.json`,
  `shells`. This is the negative case that keeps the sweep from eating the
  schedule store.

**Mutation-check three behaviors before reporting**, pasting the red **and**
green `test result:` lines for each. Each mutation below has been chosen so its
effect is reachable by the named test — break it, run *that* test, confirm red,
revert, confirm green:

1. **The security boundary.** In `request_stop`, drop the `RunId::parse` and
   build the path from the raw string →
   `request_stop_rejects_an_id_that_is_not_a_run_id` must go red.
2. **The sweep's negative case.** Make `is_stop_sentinel_name` return `true`
   unconditionally → `sweep_predicate_matches_only_stop_sentinels` must go red.
3. **The watcher's liveness exit.** Remove the "run no longer registered →
   return" check → `watcher_exits_without_firing_when_the_run_is_gone` must go
   red (the watcher keeps polling and, with a sentinel absent, at minimum the
   task must be observed still running; if your test asserts on the signal
   alone, also assert the task handle has finished).

Use the `patch` tool for every mutation and revert. **Do not use `sed -i`,
`perl -i` or `> file`** — the executor contract forbids them and `bash` refuses.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-m24-04.txt`.
**Section H drives the real CLI in a throwaway `HOME`** — that is the phase's
user-facing door.

```sh
{
echo "== A. build =="; cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test daemon::run::cancel::tests::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. run module total (>=50) =="
cargo test --lib daemon::run:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. preservation: cancel 6, lifecycle 5, path_audit 24, log_read 11, result 11, registry 27 =="
for m in daemon::cancel config::lifecycle config::path_audit daemon::run::log_read daemon::run::result shell::registry; do
  printf '%-26s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "cancel.rs exists           (1): "; test -f src/daemon/run/cancel.rs && echo 1 || echo 0
echo -n "mod cancel; private        (1): "; grep -c '^mod cancel;' src/daemon/run/mod.rs
echo -n "no pub mod in run/mod.rs   (0): "; grep -c '^pub mod ' src/daemon/run/mod.rs
echo -n "register_run             (>=1): "; grep -c 'pub fn register_run' src/daemon/run/cancel.rs
echo -n "cancel_run               (>=1): "; grep -c 'pub fn cancel_run' src/daemon/run/cancel.rs
echo -n "stop_sentinel_path takes RunId (1): "; grep -c 'fn stop_sentinel_path(id: &RunId)' src/daemon/run/cancel.rs
echo -n "stop_sentinel_path not &str(0): "; grep -c 'fn stop_sentinel_path(.*&str' src/daemon/run/cancel.rs
echo -n "request_stop parses      (>=1): "; grep -c 'RunId::parse' src/daemon/run/cancel.rs
echo -n "watcher declared           (1): "; grep -c 'pub async fn watch_stop_sentinel' src/daemon/run/cancel.rs
echo -n "poll injectable          (>=1): "; grep -c 'poll: Duration' src/daemon/run/cancel.rs
echo -n "STOP_POLL_INTERVAL       (>=1): "; grep -c 'STOP_POLL_INTERVAL' src/daemon/run/cancel.rs
echo -n "reason-carrying channel  (>=1): "; grep -cE 'watch::channel::<Option<CancelReason>>|watch::channel\(None\)' src/daemon/run/cancel.rs
echo -n "no bool channel            (0): "; grep -c 'watch::channel(false)' src/daemon/run/cancel.rs
echo -n "no remove_dir_all          (0): "; grep -c 'remove_dir_all' src/daemon/run/cancel.rs
echo -n "sweep uses predicate     (>=1): "; grep -c 'is_stop_sentinel_name' src/daemon/mod.rs
echo -n "Stop no longer unit        (0): "; grep -c 'Stop,' src/main.rs
echo -n "Stop has optional run    (>=1): "; grep -c 'run: Option<String>' src/main.rs
echo -n "start_paused in tests    (>=1): "; grep -c 'start_paused = true' src/daemon/run/cancel.rs
echo -n "no real sleeps             (0): "; grep -cE 'thread::sleep|sleep\(Duration::from_secs' src/daemon/run/cancel.rs
echo -n "no unwrap/panic pre-test   (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/cancel.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
echo "== G. daemon/cancel.rs untouched =="
echo -n "diff lines in daemon/cancel.rs (0): "; git diff --name-only f8b49af -- src/daemon/cancel.rs | wc -l
echo -n "stop- litter in real HOME      (0): "; ls ~/.daemoneye/var/run/ 2>/dev/null | grep -c '^stop-'
echo "== H. the CLI door, in a throwaway HOME =="
T=$(mktemp -d /tmp/de-m24-04-XXXX)
echo -n "h1 sentinel count BEFORE the stop (0): "
HOME="$T" ./target/debug/daemoneye setup >/dev/null 2>&1
ls "$T/.daemoneye/var/run/" 2>/dev/null | grep -c '^stop-'
HOME="$T" ./target/debug/daemoneye stop 0123456789abcdef; echo "stop_exit=$?"
echo -n "h2 sentinel now present      (1): "; ls "$T/.daemoneye/var/run/" | grep -c '^stop-0123456789abcdef'
echo -n "h3 traversal id is refused   (nonzero exit, 0 files): "
HOME="$T" ./target/debug/daemoneye stop ../../etc/passwd >/dev/null 2>&1; echo -n "stop_exit=$? files="
find "$T/.daemoneye" -name 'passwd*' | wc -l
echo -n "h4 nothing escaped the home  (0): "; find "$T/.daemoneye/var/run" -name 'stop-*' | grep -vc "$T"
echo -n "h5 no-arg stop still targets the daemon (nonzero, no daemon running): "
HOME="$T" ./target/debug/daemoneye stop >/dev/null 2>&1; echo "stop_exit=$?"
find "$T" -depth -delete
echo "== I. lifecycle/tree gates unmoved =="
cargo test --lib config:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
} > /tmp/e2e-m24-04.txt 2>&1
cat /tmp/e2e-m24-04.txt
```

Paste the contents of `/tmp/e2e-m24-04.txt` into your Update Log entry as a
fenced block. **The entry's header must be exactly this line**, or the
self-check below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M24-run-manager/phase-04-cancel.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-04.txt
diff /tmp/pasted-m24-04.txt /tmp/e2e-m24-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
`PASTE MATCH` is not optional and has no "by design" exemption: commentary
belongs in prose **outside** the fenced block.

**Your prose about sections F and H must state each value as it printed.**

**Section H is the acceptance evidence, not a formality.** `h3` is the security
case: a traversal id must exit non-zero and create **nothing**. `h5` is the
backward-compatibility case: bare `daemoneye stop` must still try to stop the
daemon (it exits non-zero here because no daemon is running in the throwaway
HOME — that is the expected result, and it proves the no-arg path still goes to
`run_stop()` rather than writing a sentinel).

**Section G's second line guards the developer's real `~/.daemoneye/`.** If the
suite left `stop-*` files there, a test wrote outside its tempdir HOME — fix the
test, do not delete the files and re-run.

## Authorizations

You may edit, and nothing else:

- **create** `src/daemon/run/cancel.rs`
- `src/daemon/run/mod.rs` — **only** the `mod cancel;` line and the
  `pub use cancel::{…}` re-export beside the block at `mod.rs:485-492`
- `src/daemon/mod.rs` — **only** the startup sweep block added immediately
  after the pipe-log cleanup at `mod.rs:531-541`
- `src/main.rs` — **only** the `Stop` variant's optional positional and its
  dispatch arm at `main.rs:417-419`
- `src/cli/commands/lifecycle.rs` — **only** the new `run_stop_run` function;
  `run_stop` itself is unchanged
- `src/cli/mod.rs` (or whichever module re-exports `run_stop`) — **only** the
  one line exporting `run_stop_run`
- this phase doc's Update Log

**Do not** touch `src/daemon/cancel.rs` (F2 — the session-keyed module and its 6
tests), `src/config/lifecycle.rs` (F4 — the inaccurate `var/run` row is
phase-10's), `src/config/runtime_tree.rs`, `src/config/path_audit.rs` (F5),
`src/daemon/stream.rs`, `src/ipc.rs`, `CLAUDE.md`, `README.md`, or
`tests/doc_truth.rs`.

**Do not add a dependency.** `tokio` (with `sync`, `time`), `anyhow` and
`tempfile` (dev) are already available; `test-util` is already a dev feature
(F8).

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any signal
to a process group or to PID -1.** Use `find <dir> -depth -delete` for cleanup,
never `rm -rf`. Note the E2E block ends with exactly that `find` form.

## Out of scope

- **Consuming the signal in a run loop.** No turn checks `is_cancelled()` at
  its top and no model call is wrapped in a `select!` here — that is phases
  05–08, when runs actually exist. This phase's watcher and registry have no
  live producer yet.
- **Building a `Cancellation`/`RunResult` from a fired signal.** Phase-03 ships
  those types; the code that maps a fired signal into
  `RunState::Cancelled { stage, turns }` belongs to whichever phase owns the
  loop that was cancelled.
- **Unifying `daemon::cancel` with `daemon::run::cancel`** — deliberate
  duplication (F2), collapsed in phase 06 when a chat turn becomes a `Run`.
- **`daemoneye runs` / `/runs` / `/run` / `/stop` chat commands** — phase 10.
- **Fixing the `var/run` lifecycle row** (F4) — phase 10 owns lifecycle.
- **`CancelReason::ClaudeStop` producers** — nothing in this phase emits it;
  the variant exists from phase-03 and gets its producer when the daemon gains
  a reason to cancel a run itself.
- **Cancelling a *shell* mid-command** — M20 phase-08a already shipped shell
  signalling; a run's cancellation does not reach into its shells here.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
