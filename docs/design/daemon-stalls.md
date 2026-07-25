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

### 1.6 Conclusion and confidence

- **Confirmed by code reading:** mechanisms A and B are both present and both
  can produce a daemon-wide wedge. Neither is speculative — the lock is held
  across blocking work at named lines, and the blocking-subprocess-in-async
  count is measured.
- **Not confirmed:** which one actually fired in the two observed restarts.
  No live wedge was captured, so attributing the specific incident would be a
  guess.

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
