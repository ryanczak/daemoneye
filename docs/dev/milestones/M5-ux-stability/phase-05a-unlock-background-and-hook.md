# Phase 05a: Get tmux Subprocesses Out of the Session Lock

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-04j (conversion sweep complete) — `done`
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Fix the **3** remaining sites that spawn **tmux subprocesses while holding the
global session lock**. These are not conversions — each needs a restructure into a
*collect under the lock* phase and an *act outside it* phase.

| Site | Blocking work currently under the guard |
|---|---|
| `hook.rs:92` | `cleanup_bg_windows()` → a `kill_job_window` **per window** + `stop_pipe_pane` |
| `background/gc.rs:201` | `kill_job_window` per window, looped over **every session**, plus `log_event` file appends — **and the orphan sweep too** |
| `background/helpers.rs:155` | a filesystem scan, **two file writes**, and a `tmux display-message` — guard held ~50 lines to the end of the function |

**Finish condition: 0 raw `sessions.lock()` in all three files, with
`with_sessions(` at 3 in `hook.rs`, 1 in `gc.rs`, and 2 in `helpers.rs`.**

`helpers.rs` needs **two** acquisitions for one former site — that is the point of
the restructure, not an accident. See task 3.

**This is a bugfix phase, not a refactor.** `hook.rs:92` is the *same defect* that
caused the confirmed production hang, in a place the original fix never reached.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism A — lock held across blocking work —
  and § 1.5b–1.5c, the confirmed hang: every thread `futex`-parked, the reactor
  gone, zero CPU over 12 h. That hang was a `SessionStore` critical section doing
  blocking work. All three sites here are the same shape.
- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers. Every site here spawns at least one.
- `CLAUDE.md` § "Important Invariants" — `.unwrap_or_log()` at every lock site is a
  project invariant; `with_sessions` satisfies it internally.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**Use this scan, not `grep -c`.** `grep -c "sessions\.lock()"` cannot see an
acquisition split across lines, and that blindness caused a bounce earlier in this
milestone. Save as `/tmp/scan_locks.py`:

```python
import pathlib, re, sys
for f in sys.argv[1:]:
    L = pathlib.Path(f).read_text().splitlines()
    tb = next((i for i, l in enumerate(L, 1) if l.strip().startswith("#[cfg(test)]")), None)
    prod = 0
    for i, l in enumerate(L, 1):
        if tb and i >= tb:
            break
        if "sessions.lock()" in l:
            prod += 1
        elif re.search(r'\bsessions\s*$', l) and i < len(L) and L[i].strip().startswith(".lock()"):
            prod += 1
    print(f"{f}: {prod}")
```

Then:

```bash
python3 /tmp/scan_locks.py src/daemon/hook.rs src/daemon/background/gc.rs src/daemon/background/helpers.rs
#   src/daemon/hook.rs: 1
#   src/daemon/background/gc.rs: 1
#   src/daemon/background/helpers.rs: 1
grep -c "with_sessions(" src/daemon/hook.rs                  # expect 2 (from the prior phase)
grep -c "with_sessions(" src/daemon/background/gc.rs         # expect 0
grep -c "with_sessions(" src/daemon/background/helpers.rs    # expect 0
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker** — the per-site code below is stale.

## Current state

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

Generic over `T` — a closure may return a `Vec`, a tuple, an `Option`, anything.
That return channel **is** the collect-then-act mechanism this phase uses.

### ⭐ The worked example — `cleanup_pass`, the fix that resolved the confirmed hang

`src/daemon/session.rs`:

```rust
pub fn cleanup_pass(
    sessions: &SessionStore,
    now: std::time::Instant,
    idle_after: std::time::Duration,
) -> (Vec<SessionEntry>, std::collections::HashSet<String>) {
    with_sessions(sessions, |store| {
        let expired: Vec<String> = store
            .iter()
            .filter(|(_, v)| now.duration_since(v.last_accessed()) >= idle_after)
            .map(|(k, _)| k.clone())
            .collect();

        let mut evicted = Vec::with_capacity(expired.len());
        for key in expired {
            if let Some(entry) = store.remove(&key) {
                evicted.push(entry);
            }
        }

        let active: std::collections::HashSet<String> = store.keys().cloned().collect();
        (evicted, active)
    })
}
```

and its caller in `src/daemon/mod.rs`:

```rust
                    let (evicted, active_ids) = crate::daemon::session::cleanup_pass(
                        &sessions_cleanup,
                        Instant::now(),
                        Duration::from_secs(1800),
                    );

                    // Unlocked phase: everything blocking happens out here.
                    for entry in &evicted {
                        entry.cleanup_bg_windows();
                    }
```

**Two properties to copy:**

1. The closure **removes entries and returns them by value**, so the caller owns
   them with no borrow into the map.
2. The blocking teardown (`cleanup_bg_windows`, which spawns subprocesses) runs
   **after** `cleanup_pass` returns — the comment "Unlocked phase: everything
   blocking happens out here" marks the boundary.

Task 1 is this pattern almost verbatim. Tasks 2 and 3 are the same idea with a
richer payload.

### Receiver forms differ per file — check each

- `hook.rs:handle_notify_session_closed` takes `sessions: SessionStore` **by
  value** → `with_sessions(&sessions, …)`.
- `gc.rs:gc_bg_windows` takes `sessions: &crate::daemon::session::SessionStore` →
  `with_sessions(sessions, …)`, **no** `&`.
- `helpers.rs:notify_session` takes `sessions: &SessionStore` →
  `with_sessions(sessions, …)`, **no** `&`.

Getting this wrong is a type error, not a silent bug — but do not guess it.

### Imports

`hook.rs` already imports `with_sessions`. **`gc.rs` and `helpers.rs` do not.**
The two need different edits — these are the actual current lines, read from the
tree:

**`helpers.rs:3`** has a brace list; extend it:

```rust
// before
use crate::daemon::session::{SessionStore, append_session_message};
// after
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};
```

**`gc.rs` imports nothing from `crate::daemon::session`** — its first three lines
are `crate::daemon::utils::log_event`, `crate::ipc::Response`, `crate::tmux`, and
its signature spells the store out in full as
`sessions: &crate::daemon::session::SessionStore`. So it needs a **new** line:

```rust
use crate::daemon::session::with_sessions;
```

Leave the fully-qualified `SessionStore` in the signature alone — narrowing it to
an imported alias is a separate cleanup and out of scope.

### `UnpoisonExt` — check each file after its conversion

`with_sessions` handles poison internally, so a converted site stops needing the
trait. **The answer differs per file**, and getting it backwards breaks the build:

```bash
grep -n "unwrap_or_log" src/daemon/hook.rs                 # line 116 is bg_session — a DIFFERENT mutex
grep -n "unwrap_or_log" src/daemon/background/gc.rs
grep -n "unwrap_or_log" src/daemon/background/helpers.rs
```

- **`hook.rs`** — **keep** the import. `hook.rs:116` uses `unwrap_or_log` on
  `bg_session`, which this phase does not touch.
- **`gc.rs` and `helpers.rs` have no `UnpoisonExt` import and no `unwrap_or_log`
  call at all** — verified while drafting. Both sites use
  `let Ok(mut store) = sessions.lock() else { … }`, which needs no trait. So there
  is **nothing to delete in either file, and nothing to add.** If you find yourself
  editing an import in `gc.rs` or `helpers.rs`, stop — you have gone off-spec.

Verify with **both** `cargo build` **and**
`cargo clippy --all-targets --all-features -- -D warnings`. They disagree about
whether a test-only import counts as used, and that disagreement produced a
`hard_fail` earlier in this milestone. **A green `cargo build` alone has already
misled once.**

## Spec

### 1. `hook.rs:92` — collect the closed sessions, tear down outside

Current — `src/daemon/hook.rs:92-105`:

```rust
    if let Ok(mut store) = sessions.lock() {
        store.retain(|_, entry| {
            if entry.tmux_session == session_name {
                entry.cleanup_bg_windows();
                log::info!(
                    "Cleaned up session '{}' on tmux session-closed.",
                    session_name
                );
                false
            } else {
                true
            }
        });
    }
```

`cleanup_bg_windows()` (`src/daemon/session.rs:363`) runs
`tmux::kill_job_window` **once per background window** and then
`tmux::stop_pipe_pane` — every one a subprocess spawn, all inside `retain`, all
under the guard. **This is the `cleanup_pass` defect, unfixed here.**

Target — mirror `cleanup_pass`:

```rust
    let closed: Vec<crate::daemon::session::SessionEntry> = with_sessions(&sessions, |store| {
        let matching: Vec<String> = store
            .iter()
            .filter(|(_, entry)| entry.tmux_session == session_name)
            .map(|(k, _)| k.clone())
            .collect();

        let mut closed = Vec::with_capacity(matching.len());
        for key in matching {
            if let Some(entry) = store.remove(&key) {
                closed.push(entry);
            }
        }
        closed
    });

    // Unlocked phase: everything blocking happens out here.
    for entry in &closed {
        entry.cleanup_bg_windows();
        log::info!(
            "Cleaned up session '{}' on tmux session-closed.",
            session_name
        );
    }
```

Three points:

- **Keep the `log::info!` inside the loop**, not before or after it. The original
  logged once per removed session, and a session-closed hook can match more than
  one entry. Hoisting it out of the loop changes the log volume.
- The message text must stay byte-identical.
- **Do not** try to keep `retain` and merely move `cleanup_bg_windows` out —
  `retain`'s closure cannot hand entries back by value, which is exactly why
  `cleanup_pass` uses collect-then-remove.

### 2. `gc.rs:201` — collect the kill list, then log and kill outside

**The guard currently lives to the end of the function**, because
`let Ok(mut store) = sessions.lock() else { return; }` binds at function scope. So
it covers the per-session loop *and* the orphan sweep after it — every `log_event`
(a file append) and every `kill_job_window` (a subprocess) in both.

Add a private struct immediately above `gc_bg_windows`:

```rust
/// One window the GC has decided to kill, captured under the lock so the kill
/// itself can happen outside it.
struct GcKill {
    session_id: String,
    window_name: String,
    tmux_session: String,
    pane_id: String,
    reason: &'static str,
}
```

Then replace the guard and the per-session loop. Current:

```rust
    let Ok(mut store) = sessions.lock() else {
        return;
    };

    // Track all pane IDs referenced by any session (for orphan detection).
    let mut tracked_pane_ids: HashSet<String> = HashSet::new();

    for (session_id, entry) in store.iter_mut() {
        let to_kill = plan_gc_actions(&entry.bg_windows, &pane_map, now_unix);
        if to_kill.is_empty() {
            for w in &entry.bg_windows {
                tracked_pane_ids.insert(w.pane_id.clone());
            }
            continue;
        }

        for pane_id in &to_kill {
            // Look up window info before removing.
            if let Some(win) = entry.bg_windows.iter().find(|w| &w.pane_id == pane_id) {
                let reason = if pane_map.contains_key(pane_id) { … } else { "pane_gone" };
                log_event("gc_window", serde_json::json!({ … }));
                if let Err(e) = tmux::kill_job_window(&win.tmux_session, &win.window_name) {
                    log::warn!("gc_bg_windows: failed to kill {}: {}", win.window_name, e);
                }
            }
        }

        entry.bg_windows.retain(|w| { … });
    }
```

Target:

```rust
    // Locked phase: decide what to kill and which panes stay tracked. No
    // subprocess spawn and no file write happens while the guard is alive.
    let (kills, tracked_pane_ids): (Vec<GcKill>, HashSet<String>) =
        with_sessions(sessions, |store| {
            let mut kills: Vec<GcKill> = Vec::new();
            let mut tracked: HashSet<String> = HashSet::new();

            for (session_id, entry) in store.iter_mut() {
                let to_kill = plan_gc_actions(&entry.bg_windows, &pane_map, now_unix);
                if to_kill.is_empty() {
                    for w in &entry.bg_windows {
                        tracked.insert(w.pane_id.clone());
                    }
                    continue;
                }

                for pane_id in &to_kill {
                    // Look up window info before removing.
                    if let Some(win) = entry.bg_windows.iter().find(|w| &w.pane_id == pane_id) {
                        let reason = if pane_map.contains_key(pane_id) {
                            if pane_map[pane_id].dead {
                                "pane_dead"
                            } else {
                                "idle_completed"
                            }
                        } else {
                            "pane_gone"
                        };
                        kills.push(GcKill {
                            session_id: session_id.clone(),
                            window_name: win.window_name.clone(),
                            tmux_session: win.tmux_session.clone(),
                            pane_id: pane_id.clone(),
                            reason,
                        });
                    }
                }

                entry.bg_windows.retain(|w| {
                    let keep = !to_kill.contains(&w.pane_id);
                    if keep {
                        tracked.insert(w.pane_id.clone());
                    }
                    keep
                });
            }

            (kills, tracked)
        });

    // Unlocked phase: everything blocking happens out here.
    for k in &kills {
        log_event(
            "gc_window",
            serde_json::json!({
                "session": k.session_id,
                "win_name": k.window_name,
                "pane_id": k.pane_id,
                "reason": k.reason,
            }),
        );
        if let Err(e) = tmux::kill_job_window(&k.tmux_session, &k.window_name) {
            log::warn!("gc_bg_windows: failed to kill {}: {}", k.window_name, e);
        }
    }
```

Five things to preserve exactly:

- **The `reason` strings** `"pane_dead"`, `"idle_completed"`, `"pane_gone"`, and
  (in the orphan sweep) `"orphan"` all land in `events.jsonl` and are queried
  later. Byte-identical.
- **The `log_event` field names and order** — `session`, `win_name`, `pane_id`,
  `reason`. The original's `"session": session_id` becomes `k.session_id`; the key
  stays `session`.
- **`plan_gc_actions` stays inside the closure.** It is pure (it reads
  `bg_windows`, `pane_map`, `now_unix`) — see `gc.rs`; keeping it inside is what
  lets the `retain` use its result in the same acquisition.
- **The `retain` stays inside**, and must still insert kept pane ids into
  `tracked`. That set drives orphan detection; dropping an insert would make the
  orphan sweep kill a live window.
- **The `log::warn!` text** `"gc_bg_windows: failed to kill {}: {}"` is unchanged.

**The orphan sweep after this block does not move** — it is already positioned
after the loop, and once the guard no longer spans the function it is
automatically outside the critical section. Do not restructure it; just leave it
where it is. Its `log_event` and `kill_job_window` are then free of the lock as a
consequence of this task.

### 3. `helpers.rs:155` — split `notify_session` into four phases

The guard currently spans from acquisition to the **end of the function**, ~50
lines, covering:

- `crate::manifest::related_knowledge_hints(body)` — which calls
  `load_all_entries()` (`src/manifest.rs:411`), a **filesystem scan** of runbooks,
  scripts and memory;
- `append_session_message(session_id, &completion_msg)` — **two file writes**;
- a `tmux display-message` **subprocess spawn**.

Current — `src/daemon/background/helpers.rs:155-208`:

```rust
    let Ok(mut store) = sessions.lock() else {
        return;
    };
    let Some(entry) = store.get_mut(session_id) else {
        return;
    };

    // Update exit_code in the bg_windows registry.
    if let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id) {
        w.exit_code = Some(exit_code);
    }

    let persist_note = …;
    let hints = crate::manifest::related_knowledge_hints(body);
    …
    let completion_msg = Message { … };
    append_session_message(session_id, &completion_msg);
    entry.messages.push(completion_msg);

    let status_word = …;
    let alert = format!("`{cmd}` {status_word} in pane {pane_id}");
    if let Some(ref cp) = entry.chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
}
```

Restructure into **locked → unlocked → locked → unlocked**:

```rust
    // Phase 1 (locked): update the registry and take what the rest needs.
    // Returns None when the session entry is gone.
    let Some(chat_pane) = with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;

        // Update exit_code in the bg_windows registry.
        if let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id) {
            w.exit_code = Some(exit_code);
        }

        Some(entry.chat_pane.clone())
    }) else {
        return;
    };

    // Phase 2 (unlocked): the filesystem scan, the formatting, and the file write.
    let persist_note = if pane_persists {
        format!(
            "The window is still open (pane {pane_id}). \
             Use target=\"{pane_id}\" to run follow-up commands in the same shell. \
             Call close_background_window(\"{pane_id}\") when you are done with this window."
        )
    } else {
        format!("The window was closed. Full log: ~/.daemoneye/var/log/panes/{win_name}.log")
    };

    let hints = crate::manifest::related_knowledge_hints(body);
    let hints_section = if !hints.is_empty() {
        format!("\n{}", hints)
    } else {
        String::new()
    };
    let history_content = format!(
        "Background command `{cmd}` in window {win_name} finished with exit code {exit_code}.\n\
         {persist_note}\n<output>\n{body}\n</output>{hints_section}"
    );
    let completion_msg = Message {
        role: "user".to_string(),
        content: format!("[Background Task Completed]\n{}", history_content),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    append_session_message(session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.messages.push(completion_msg);
        }
    });

    // Phase 4 (unlocked): the tmux notification.
    let status_word = if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
    let alert = format!("`{cmd}` {status_word} in pane {pane_id}");
    if let Some(ref cp) = chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
}
```

Six points, several of them subtle:

- **Two acquisitions for one former site is correct here** and is why the finish
  condition says `helpers.rs` → **2**. Do not try to do it in one; the whole point
  is that the work between them must not hold the lock.
- **`chat_pane` is cloned out in phase 1** (`Option<String>`), because phase 4
  needs it after the guard is gone. `if let Some(ref cp) = chat_pane` then borrows
  the local. The original borrowed `entry.chat_pane` — that borrow is what pinned
  the guard open to the end of the function.
- **Phase 1's `?` returns from the closure**, and the outer `let … else { return; }`
  turns `None` into a return from `notify_session`. This preserves the original's
  two early returns (poison → return, entry absent → return) as one, since
  `with_sessions` recovers from poison rather than bailing.
- **Phase 3 re-checks `get_mut`** rather than assuming the entry still exists. It
  can legitimately vanish between phases — the session may be evicted while the
  filesystem scan runs. `if let Some(entry)` handles it; a missing entry means the
  message is on disk but not in memory, which is exactly what the pre-existing
  comment in `webhook/process.rs::inject_into_sessions` describes as acceptable
  ("the next Ask request will re-read from disk").
- **`append_session_message` stays before phase 3**, as in the original
  (append-then-push). Do not reorder.
- **Every string must stay byte-identical** — `persist_note`'s two arms, the
  `history_content` format, `"[Background Task Completed]\n"`, and the `alert`.
  The first two reach the AI as session history; the last reaches the user's tmux
  status line.

### 4. Do not widen any closure

The whole point of this phase is narrowing critical sections. Nothing that
blocks may end up inside a `with_sessions` closure:

| Must stay outside | Why |
|---|---|
| `cleanup_bg_windows()` (task 1) | `kill_job_window` per window + `stop_pipe_pane` |
| `log_event` (task 2) | file append |
| `tmux::kill_job_window` (task 2, both loops) | subprocess |
| `related_knowledge_hints` (task 3) | filesystem scan via `load_all_entries` |
| `append_session_message` (task 3) | two file writes |
| `std::process::Command::new("tmux")` (task 3) | subprocess |
| the orphan sweep (task 2) | `log_event` + `kill_job_window` |

`with_sessions` takes a synchronous `FnOnce`, so an `.await` inside one will not
compile — a guardrail, not an obstacle.

## Acceptance criteria

- [ ] `python3 /tmp/scan_locks.py src/daemon/hook.rs src/daemon/background/gc.rs src/daemon/background/helpers.rs`
      prints **0** for all three.
- [ ] `grep -c "with_sessions(" src/daemon/hook.rs` returns **3** (2 pre-existing + 1).
- [ ] `grep -c "with_sessions(" src/daemon/background/gc.rs` returns **1**.
- [ ] `grep -c "with_sessions(" src/daemon/background/helpers.rs` returns **2** —
      **not 1.** Task 3 needs two acquisitions.
- [ ] `python3 /tmp/scan_locks.py src/webhook/process.rs src/daemon/stream.rs`
      still prints **2** and **1** — **phase 05b's sites, deliberately untouched.**
      A lower number means you converted out of scope.
- [ ] `python3 /tmp/scan_locks.py src/daemon/ghost.rs src/daemon/background/run.rs src/daemon/background/respawn.rs src/daemon/context/background.rs src/daemon/executor/mod.rs`
      prints **0** for all five (earlier phases untouched).
- [ ] `grep -c "UnpoisonExt" src/daemon/hook.rs` returns **1** — still needed for
      `bg_session`.
- [ ] `grep -c "cleanup_bg_windows" src/daemon/hook.rs` returns **1**, and it is
      **not** inside a `with_sessions` closure — verify by reading; the count
      cannot prove it.
- [ ] `grep -c "kill_job_window" src/daemon/background/gc.rs` returns **3**,
      unchanged from before this phase. Two are yours — the per-session loop and
      the orphan sweep — and **neither** may be inside a closure. The third is at
      `gc.rs:78` in `notify_job_completion`, which **holds no session lock and is
      not in scope**. Verify placement by reading; the count cannot prove it.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **916 means scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text including comments. **Do not write the
literal `sessions.lock()`, `with_sessions(`, `cleanup_bg_windows`, or
`kill_job_window` in a new comment** in these files.

## Test plan

Behavior-preserving restructure: the observable behavior — which windows get
killed, what lands in `events.jsonl`, what the AI sees in history, what the user
sees in tmux — is unchanged. Only *when the lock is held* changes. The existing
**915** tests are the regression net. **Write no new tests.**

`gc.rs` has a `#[cfg(test)]` module covering `plan_gc_actions`, which this phase
does not modify — it moves the call, not the logic. `helpers.rs`'s test module does
not cover `notify_session`, and `hook.rs` has no test module.

**So none of the three restructured functions is covered by the unit suite.** That
is a pre-existing gap this phase neither widens nor closes, and it is why the spec
gives exact target code rather than relying on tests to catch a slip.

Run the suite and report what you observe. **Report only which commands you ran and
whether they passed.** Do **not** claim any test "guards" or "covers" one of these
sites — here that would be plainly false, and in this project a claim about what a
test would catch is admissible only when demonstrated by mutation. This phase
requires no mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Task 1 log volume.** Confirm `log::info!` is still inside the teardown loop,
   so a hook matching N sessions logs N times as before — not once.
2. **Task 2 tracking.** Confirm the `retain` still inserts kept pane ids into the
   tracked set, and say why it matters (the orphan sweep kills anything untracked,
   so a dropped insert would kill a live window).
3. **Task 3 phase boundaries.** List which of the four phases holds the lock, and
   confirm `related_knowledge_hints`, `append_session_message`, and the
   `tmux display-message` are all in unlocked phases.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal restructuring
> of lock scope inside existing code paths; no CLI surface, no config key, no file
> the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon, or
a background job. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

- [x] May add a private `GcKill` struct to `src/daemon/background/gc.rs` (task 2).
      A struct rather than a 5-tuple is the intent; five positional fields at the
      use site would be worse.
- [ ] **No import deletions are authorized.** `gc.rs` and `helpers.rs` have no
      `UnpoisonExt` import to remove, and `hook.rs` still needs its. Unlike the
      preceding phases, this one ends with every import exactly as it started.

This phase adds no tests, so it needs no `HOME` redirection and no `unsafe`. If you
think you need `unsafe` or a new dependency, **stop and report a blocker**.

## Out of scope

- **Do not touch `webhook/process.rs` (2 sites) or `stream.rs:722`.** They are
  **phase 05b**. Criteria pin them at 2 and 1 so an over-eager fix is caught.
  `webhook/process.rs::notify_chat_panes` is the same subprocess-under-lock shape
  and will be tempting; leave it.
- **Do not re-touch `ghost.rs`, `background/run.rs`, `background/respawn.rs`,
  `context/background.rs`, `briefing.rs`, or `executor/`.** Converted; pinned by a
  criterion.
- **Do not change `SessionStore` into a newtype** and do not touch the 13
  `Arc::clone` sites — that is **05c**, which must run after this phase.
- **Do not modify `plan_gc_actions`** or its tests. Task 2 moves its call site, not
  its logic.
- **Do not modify `cleanup_bg_windows`** (`session.rs:363`). It is correct; the bug
  is *where it is called from*.
- **Do not reword any string** — the three GC `reason` values, the orphan
  `"orphan"`, both `log::warn!` texts, `persist_note`'s two arms, the
  `history_content` format, `"[Background Task Completed]"`, and the `alert`.
- **Do not restructure the orphan sweep** in `gc.rs`. It ends up outside the lock
  as a consequence of task 2; it needs no edit of its own.
- **Do not collapse task 3's two acquisitions into one.**
- **Do not reorder `append_session_message` and the phase-3 push.**
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to the `GcKill` field
  count or a `let … else` shape, report a blocker rather than suppressing.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 19:36 (started)

**Executor:** model (phase-05a)

Implemented all three restructures: `hook.rs:92`, `gc.rs:201`, `helpers.rs:155`.
