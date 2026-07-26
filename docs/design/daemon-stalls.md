# Design: Daemon Stalls & Chat UX

**Status:** scoped 2026-07-24 (M5 kick-off)
**Milestone:** M5 — UX & Stability

This document captures the architect's diagnosis of the reported daemon hang
and the two chat-TUI defects that open M5. The hang section is written as an
evidence log: what was observed, what the code actually does, and which
mechanisms can produce the observed symptom. It is deliberately explicit about
what is **confirmed by reading code** versus what is **hypothesis awaiting
reproduction**, so a later phase does not mistake one for the other.

---

## 1. The hang

### 1.1 Reported symptom

The PE reports all three of the following, from the same defect:

1. Chat freezes mid-turn — spinner keeps animating, no tokens arrive.
2. The daemon stops answering **new** clients — a fresh `daemoneye chat` or
   `daemoneye status` gets no reply.
3. Hangs occur around tool calls / command execution, not only during
   token streaming.

Symptom (2) is the diagnostically important one. A stalled LLM stream would
freeze *one* session; it would not stop an unrelated `daemoneye status` from
being served. **Something global is wedging** — a shared lock, or the tokio
runtime itself.

### 1.2 Observed state

`daemon.log` (2026-07-24) shows two daemon restarts in one day (04:00:35Z and
06:20:26Z). In both cases the final line before the restart is:

```
DEBUG starting new connection 'Some("brain")'
```

`brain` is the configured ollama host. The line is emitted by the HTTP
connection pool, so it proves an AI request was in flight when the daemon
went quiet — but it does **not** prove the stream is the cause. It is equally
consistent with "an AI turn was running, and something else wedged."

The currently-running daemon (pid 205685) is idle-healthy: 33 threads, one in
`epoll_wait` (the tokio reactor), the rest in `futex_wait` (worker + blocking
pool parked). This is the normal shape of an idle tokio runtime and is **not**
evidence of a hang. No live wedge was captured.

### 1.3 Mechanism A — global `SessionStore` lock held across blocking work

`SessionStore` is a single lock over every session:

```rust
// src/daemon/session.rs:116
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;
```

Every IPC handler acquires it. Therefore **any** code path that holds this
guard while doing unbounded blocking work stalls the entire daemon, including
clients that have nothing to do with the session being worked on. That is an
exact match for symptom (2).

Two such paths are confirmed by reading the code. Both are in
`src/webhook/process.rs` and both are reachable from webhooks, scheduled jobs,
ghost lifecycle events, and the `spawn_ghost_shell` tool:

```rust
// src/webhook/process.rs:148 — file I/O under the global lock
pub(crate) fn inject_into_sessions(sessions: &SessionStore, msg: &Message) {
    let guard = sessions.lock().unwrap_or_log();
    for (sid, entry) in guard.iter() {
        append_session_message(sid, msg);   // ← synchronous disk write per session
        let _ = entry;
    }
}

// src/webhook/process.rs:161 — subprocess spawn under the global lock
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let guard = sessions.lock().unwrap_or_log();
    for entry in guard.values() {
        if let Some(ref pane) = entry.chat_pane {
            let _ = std::process::Command::new("tmux")
                .args(["display-message", "-d", "8000", "-t", pane, msg])
                .output();          // ← blocking, no timeout, once per chat pane
        }
    }
}
```

`inject_ghost_event()` calls **both** in sequence, so a single ghost
lifecycle event takes the global lock twice, once around N disk writes and
once around N tmux subprocess spawns.

`append_session_message` is not cheap as of M4 — it also writes the
append-only archive. So mechanism A got *worse* in M4, which is consistent
with the hang becoming noticeable now.

The failure needs only one slow `tmux display-message`. tmux is a single
server process with its own global lock; a busy or wedged tmux server, or a
pane belonging to a suspended client, blocks the call indefinitely. There is
no timeout and no `kill` on the child. The lock is then held forever and every
IPC handler queues behind it.

### 1.4 Mechanism B — blocking subprocess calls on tokio worker threads

Independent of any lock: the tmux layer makes **49** calls through blocking
`std::process::Command`, essentially all without a timeout:

```
$ grep -rn "std::process::Command\|Command::new" src/tmux/*.rs | wc -l
49
$ grep -rn "tokio::process\|spawn_blocking" src/ --include=*.rs | wc -l
4
```

Only four sites use `tokio::process` or `spawn_blocking` (`tmux/pane.rs:516`,
`tmux/pane.rs:528`, `daemon/utils/sudo.rs:48`). Every other tmux call made
from inside an `async fn` blocks a tokio **worker thread** for the duration of
the subprocess. With the default multi-thread runtime the daemon has one
worker per core; enough concurrent blocked tmux calls starve the runtime and
no task — including the IPC accept loop — makes progress. Same observable
symptom as mechanism A, different cause.

The 2-second cache poller (`tmux/cache.rs`) makes several such calls on every
tick, so the daemon is continuously exposed to this path even when idle.

### 1.5 Mechanism C — AI stream stall (partially mitigated, not excluded)

`ai::http()` sets a single 300 s total-request timeout
(`src/ai/mod.rs:119`). There is no per-chunk / idle timeout on the SSE
stream, so a provider that accepts the connection and then goes quiet stalls
the turn until the total timeout expires. This explains symptom (1) on its
own but **cannot** explain symptom (2), so it is at most a contributing
factor. It is worth fixing (an idle-read timeout gives a much better error
than a 5-minute freeze) but it is not the root cause.

### 1.5b LIVE CAPTURE — 2026-07-25, pid 205685

A wedged daemon was caught in the act during the phase-01 review, while trying
to run the chat client for an end-to-end check. This is the first live capture;
§1.2's "no live wedge was captured" no longer holds.

Observed state, daemon pid 205685, **12 h 25 m** after start:

| Probe | Value | Reading |
|---|---|---|
| `connect()` to the socket | `ECONNREFUSED` | new clients cannot attach |
| `ss -lx` | `u_str LISTEN 9 4096` | **9 connections queued, never accepted** |
| established connections | 0 | nothing is being served |
| threads in `epoll_wait` | **0** | the tokio reactor is not polling |
| threads in `futex_wait` | **33 of 33** | every thread blocked on a lock |
| CPU time consumed | **00:00:00** | fully blocked — not a livelock, not a spin |
| last `daemon.log` line | 7 s after startup | wedged almost immediately |

The same daemon had **one** thread in `epoll_wait` when probed earlier in the
session (§1.2). By the time of this capture that thread was gone. A tokio
runtime whose reactor thread is itself parked on a futex cannot accept, cannot
time out, and cannot log — which is exactly what the queue depth of 9 and the
12-hour logging silence show.

**Zero CPU seconds over 12 hours is the decisive number.** It rules out a busy
loop, a retry storm, and a slow-but-progressing operation. Every thread is
waiting on a lock that will never be released. This is a deadlock, not a stall.

The last line written before the silence was:

```
2026-07-25T06:20:33Z DEBUG starting new connection 'Some("brain")'
```

That is the **third consecutive restart** ending on that same line (§1.2 records
the other two). The wedge therefore correlates with the first outbound AI HTTP
connection, roughly 7 seconds after startup — not with sustained load, not with
a rare alert, and not with anything the user did.

### 1.5c ROOT CAUSE — re-entrant lock in the session-cleanup sweep

The PE captured stacks with `sudo gdb -p 205685 -batch -ex "thread apply all
bt 15"` (592 lines). Across all 33 threads there are exactly **two** frames of
daemoneye code:

```
Thread 1  "daemoneye"      #5 daemoneye::main            → Runtime::block_on (normal)
Thread 20 "tokio-rt-worker" #1 <std::sys::sync::mutex::futex::Mutex>::lock_contended
                            #2 daemoneye::daemon::run_daemon::{{closure}}::{{closure}}::{{closure}}
                            #3 tokio::runtime::task::harness::Harness<T,S>::poll
```

Every other thread is a parked worker in `Condvar::wait`. **No thread holds the
mutex** — which is the tell. The holder is not another thread; it is the *same
task*, one frame up its own stack.

`src/daemon/mod.rs:683-723`, the `session-cleanup` supervisor:

```rust
loop {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let now = Instant::now();
    let mut store = sessions_cleanup.lock().unwrap_or_log();   // ← 693: guard bound
    store.retain(|_, v| { … });

    sweep_counter = sweep_counter.wrapping_add(1);
    if sweep_counter.is_multiple_of(60) {
        …
        let active_ids: std::collections::HashSet<String> = sessions_cleanup
            .lock()                                            // ← 709: SAME mutex,
            .unwrap_or_log()                                   //   SAME thread,
            .keys().cloned().collect();                        //   guard still alive
        …
    }
}                                                              // ← 720: `store` drops here
```

`store` is a `let` binding, not a temporary, and `MutexGuard` implements `Drop`,
so it lives to the end of the loop body at line 720. Line 709 re-locks the same
mutex while line 693's guard is still held. **`std::sync::Mutex` is not
reentrant** — the second lock blocks forever, on a lock the same task will never
release.

Confirmed by inspection:

- `sessions_cleanup_sup = Arc::clone(&sessions)` (`mod.rs:682`) — this is the
  global `SessionStore`, the one every IPC handler locks.
- There is **no `.await`** between the two locks. This is why
  `clippy::await_holding_lock` never fired and the lint gate stayed green: that
  lint targets guards held across suspension points, not a plain double-lock.

**The trigger is uptime, not load, and not the AI.** `sweep_counter` increments
once per 60-second iteration; the inner block runs when
`sweep_counter % 60 == 0`, i.e. the 60th iteration — **≈60 minutes after daemon
start, deterministically, every time.** Nothing the user does affects it. The
earlier suspicion (§1.5b, since corrected) that the wedge correlated with the
first outbound AI request was wrong: `starting new connection 'brain'` is simply
the last thing an otherwise-idle daemon logs before the one-hour mark, and it
appears in all three restart logs for that reason alone.

Once the guard is stranded, **every** path that locks `sessions` blocks forever
— which is precisely the three reported symptoms at once: a chat turn freezes
mid-stream, new clients get nothing, and anything touching a tool call hangs.
The 9 unaccepted connections and the 12 hours of zero CPU follow directly.

**This is a mechanism-A defect** (§1.3) — a `SessionStore` critical section
doing more than it should — but a purer form than the ones found by code
reading: no blocking I/O, no subprocess, just the same lock taken twice. The
milestone's exit criterion "no `SessionStore` critical section performs blocking
work" must therefore be widened to include **re-entrant acquisition**, which no
existing lint catches.

**Consequence for the phase plan.** Mechanism C (SSE stall, §1.5) is excluded as
the primary cause. Phase 03's original purpose — instrument so the next wedge
identifies itself — is now partly spent: this wedge has been identified. The fix
is a few lines (reuse the `store` guard already held, or scope it so it drops
before the sweep). That should lead M5, ahead of the broader hardening work.

### 1.6 Conclusion and confidence

- **Confirmed by code reading:** mechanisms A and B are both present and both
  can produce a daemon-wide wedge. Neither is speculative — the lock is held
  across blocking work at named lines, and the blocking-subprocess-in-async
  count is measured.
- **ROOT CAUSE IDENTIFIED (§1.5c, 2026-07-25):** a re-entrant acquisition of the
  global `SessionStore` mutex in the `session-cleanup` supervisor
  (`src/daemon/mod.rs:693` and `:709`). The guard from 693 is still alive at 709;
  `std::sync::Mutex` is not reentrant, so the task self-deadlocks and strands the
  lock for every other path. Fires deterministically ≈60 minutes after daemon
  start. Proven by gdb stacks (one task in `lock_contended`, no thread holding
  the mutex) plus code inspection.
- **Corrected:** the earlier reading that the wedge correlated with the first AI
  request was wrong — that log line is coincidence, not cause.
- **Unchanged:** mechanisms A and B remain real defects worth fixing (blocking
  I/O and subprocess spawns under the same lock); they are simply not what fired
  here. Mechanism C is excluded.

This ordering drives the phase plan: **instrument first** (phase 03) so the
next wedge identifies itself, then fix the confirmed hazards (phases 04–05)
regardless of which one the instrumentation catches. Both are real defects
worth removing on their own merits — the instrumentation is what tells us
whether anything *else* remains.

Deliberately **not** proposed: switching `SessionStore` to an async
`tokio::sync::Mutex`. That would make every lock site `.await` and invites the
lock to be held across await points — a worse failure mode than the one being
fixed. The fix is to shrink the critical sections so no blocking work happens
under the lock at all.

---

## 2. Chat TUI defects

### 2.1 Spinner is drawn inside the input box

`render_spinner_region()` (`src/cli/render_ratatui.rs:545`) renders the
spinner line *as the content of the bordered input block*, replacing the input
text:

```rust
let input_block = Block::default().borders(Borders::ALL) /* … */;
let input_para = Paragraph::new(spinner_line).block(input_block);
frame.render_widget(input_para, chunks[0]);
```

**Target:** the spinner moves to a dedicated one-row line immediately **above**
the input box's top border. The row is reserved in every live-region draw mode
— blank when idle — so the box does not shift vertically when streaming starts
or stops.

```
  (◉) scrying...
┌────────────────────────────┐
│                            │
└────────────────────────────┘
 session:a1b2… · opus · up 3m
```

The animated frame, the verb, and the dot animation stay together on that row
as a single unit. This is why the row is full-width rather than a narrow
left-hand gutter: the longest verb (`"discerning"`) plus frame, spacing, and
dots needs roughly 20 columns, which no gutter narrow enough to leave a usable
input box could provide.

The row is taken out of the existing six-row inline viewport
(`VIEWPORT_ROWS = 6`) rather than added to it, so the live region occupies the
same amount of terminal as before; the input box goes from three content rows
to two and scrolls for longer input.

This affects all three live-region renderers — `render_live_region()`,
`render_spinner_region()`, and `render_prompt_region()` — because they must all
reserve the row or the box will jump between states.

### 2.2 User input is never committed to scrollback

`run_chat_ratatui()` (`src/cli/commands/chat.rs`) reads a line, trims it,
pushes it to input history, and passes it straight to
`ask_with_session_ratatui()`. Nothing commits the query to the terminal
scrollback, so once the input box clears, what the user typed is gone from the
transcript — the conversation reads as a series of unattributed answers.

**Target:** commit the submitted query above the live region using the same
committed-panel element as tool output, `commit_panel()`
(`src/cli/render_ratatui.rs:318`) — the renderer already used for
`▸ tool(args)` lines and command output. This keeps one visual grammar for
everything in the transcript and inherits the existing width/truncation
handling.

Applies to prose queries. Slash commands and the synthetic `"Hello!"` greeting
are excluded — see the phase doc for the exact boundary.


---

## 3. Structural answer to re-entrant `SessionStore` locking

**Decided by the PE, 2026-07-25: adopt a `with_sessions(…)` accessor.** This
section records the decision, the survey that backs it, and the sizing — it is
the input to phase 04's draft, not a phase spec itself.

### 3.1 Why structural rather than another point fix

Two independent re-entrant `sessions`-lock defects have now shipped in this
codebase:

1. `stream.rs` — the `sessions` guard held across `spawn_compaction`, which
   re-locks. Found and fixed during the M4 phase-08 architect takeover.
2. `mod.rs:693`/`:709` — the session-cleanup double-lock. Root-caused in § 1.5c
   and fixed in M5 phase-02, after wedging the daemon hourly.

Neither was catchable by any lint. `clippy::await_holding_lock` targets guards
held across suspension points; both bugs were plain double-acquires with no
`.await` between them, so the gate stayed green for the entire life of each
defect. Two occurrences with zero tooling coverage is the argument for changing
the shape of the API rather than fixing the third one later.

### 3.2 Survey

- **100** `sessions.lock()` call sites outside tests, concentrated in
  `server/handlers.rs` (15), `server/ask.rs` (13), `context/background.rs` (13),
  `ghost.rs` (11), `executor/mod.rs` (10), `stream.rs` (8).
- **13** `Arc::clone(&sessions…)` sites — a newtype must derive `Clone`.
- **0** guards held across `.await` (confirmed: `clippy -D warnings` is clean and
  `await_holding_lock` is warn-by-default). Every existing site is therefore
  convertible to a synchronous closure without restructuring async control flow.
- Dominant shape is short and closure-shaped already:

```rust
// src/daemon/server/handlers.rs:57
if let Ok(mut store) = sessions.lock()
    && let Some(entry) = store.get_mut(&session_id)
{
    entry.active_model = Some(model_name.clone());
}
```

The survey also turned up a live instance of the pattern the accessor removes —
two sequential acquisitions where one would do (not a deadlock; the first guard
drops at the end of its `if let`):

```rust
// src/daemon/server/handlers.rs:166 and :173
let current_target = if let Ok(store) = sessions.lock() { … } else { None };
let chat_pane_id: Option<String> = if let Ok(store) = sessions.lock() { … } else { None };
```

### 3.3 The shape

A closure accessor alone is only advisory — raw `.lock()` remains callable, so
nesting stays writable. The enforceable end state is a **newtype with a private
inner**, which makes the compiler find every site:

```rust
#[derive(Clone)]
pub struct SessionStore(Arc<Mutex<HashMap<String, SessionEntry>>>);   // inner is private

impl SessionStore {
    /// The only way to reach the map. The guard's lifetime is the closure body,
    /// so it cannot escape, cannot be held across an `.await`, and a nested
    /// `with_sessions` inside `f` is visible at the call site rather than
    /// hidden three frames down.
    pub fn with<T>(&self, f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T) -> T { … }
}
```

Add a **debug-build re-entrancy assertion** inside `with` — a thread-local depth
counter that panics in `cfg!(debug_assertions)` on a nested acquisition. That
converts the failure from "daemon wedges an hour later in production" into "test
run fails immediately with a stack trace." Detection and prevention together;
neither alone is sufficient, since the newtype prevents *accidental* nesting but
a determined caller can still nest two `with` calls.

### 3.4 Sizing and ordering — accessor first, newtype last

100 call sites in a single mechanical sweep is precisely the shape that has
defeated this executor before (M4's retrospective: large blocks and broad
rewires → self-sabotage; M5's clean runs were all small, single-purpose, fully
quoted). Split by file group, each phase independently green.

**The newtype must land last, not first.** A survey of the conversion found 13
`Arc::clone(&sessions…)` sites (9 of them in `daemon/mod.rs` alone). Under a
newtype, `Arc::clone(&sessions)` returns the inner `Arc`, not a `SessionStore`,
so every one of those sites becomes a type error the moment the newtype is
introduced — turning "phase 04a" into a 100-site sweep by accident. Introducing
the free accessor first keeps `pub type SessionStore = Arc<Mutex<…>>` unchanged,
so unconverted sites and all 13 `Arc::clone` calls keep compiling untouched.

Revised sequence:

- **04a** — add the free `with_sessions(&SessionStore, |store| …)` accessor plus
  the re-entrancy assertion, and convert only the two live sites
  (`session.rs` `cleanup_pass`, `mod.rs` shutdown sweep). Tiny; establishes the
  pattern and the guard with a worked example later phases quote.
- **04b/04c** — mechanical conversion of the remaining sites by file group,
  largest first (`handlers.rs` + `ask.rs`, then `background.rs` + `ghost.rs` +
  the tail). The type is unchanged throughout, so each group compiles alone.
- **04d** — flip `SessionStore` to the newtype with a private inner, convert the
  13 `Arc::clone` sites to `.clone()`, and delete the raw path. The compiler
  enumerates any straggler. Only at this point is the invariant *enforced* rather
  than merely available.
- Mechanisms A and B from § 1.3–1.4 (blocking I/O and tmux subprocesses under
  the lock, in `webhook/process.rs` and the tmux layer) stay their own phases —
  the accessor does not fix them, it only makes them easier to see.

**Assertion severity: always on, not `debug_assert`.** A re-entrant acquisition
on one thread is never legitimate here — it would deadlock. Panicking is strictly
better than wedging: the `supervise` wrapper restarts a panicked task, whereas the
phase-02 deadlock took the daemon down for 12 hours. A `debug_assert` would have
been compiled out of exactly the build where it mattered.
