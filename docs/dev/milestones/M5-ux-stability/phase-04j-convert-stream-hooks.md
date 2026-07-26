# Phase 04j: Convert `stream.rs` + `hook.rs` — the Conversion Sites

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-04i (`background/run.rs` + `respawn.rs`) — `done`
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert **10** `sessions.lock()` sites — 8 in `src/daemon/stream.rs` and 2 in
`src/daemon/hook.rs` — to `with_sessions`.

**Finish condition: 8 `with_sessions` calls in `stream.rs`, 2 in `hook.rs`, and
each file left with exactly 1 raw acquisition** — the two sites that are *not*
conversions and belong to phase 05.

**Two sites are deliberately excluded** because they are mechanism-A/B defects
needing restructures, not wraps (see Out of scope):

- `stream.rs:719` holds the guard across `write_session_meta` (a file write).
- `hook.rs:91` holds it across `cleanup_bg_windows()`, which spawns **one tmux
  subprocess per background window plus `stop_pipe_pane`** — the same defect class
  as the confirmed production hang this milestone was opened to fix.

Nine of your ten are mechanical. **One is not** — task 4 (`stream.rs:751`) is a
six-element `let`-chain that mutates an entry and yields a `bool`.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard: a converted closure
  enclosing a call that still uses raw `.lock()` deadlocks silently. Task 11 names
  this phase's exposure, which is unusually important here.
- `docs/design/daemon-stalls.md` § 1.5b–1.5c — the confirmed hang. Its shape is
  why `stream.rs:689`'s closure must close before the compaction block.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**`grep -c` is not sufficient for this phase.** `stream.rs` contains an
acquisition that splits `sessions` and `.lock()` across lines (task 8), which
`grep -c "sessions\.lock()"` **cannot see** — it is one of the five such sites that
caused a bounce earlier in this milestone. Save as `/tmp/scan_locks.py`:

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
python3 /tmp/scan_locks.py src/daemon/stream.rs src/daemon/hook.rs
#   src/daemon/stream.rs: 9      <- the scan sees 9; plain grep -c sees only 8
#   src/daemon/hook.rs: 3
grep -c "sessions\.lock()" src/daemon/stream.rs   # 8 — the discrepancy is task 8
grep -c "with_sessions(" src/daemon/stream.rs     # expect 0
grep -c "with_sessions(" src/daemon/hook.rs       # expect 0
```

**Verified against the tree while drafting.** If the scan does not print 9 and 3,
**stop and report a blocker** — the per-site line numbers below are stale.

## Current state

`SessionStore` is still the bare type alias:

```rust
// src/daemon/session.rs:117
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;
```

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

### Pass `&sessions` in both files

In `stream.rs`, `sessions` is destructured **by value** out of
`ConversationLoopCtx` (field `pub sessions: SessionStore`, destructured at the top
of `run_conversation_loop`). In `hook.rs` all three handlers take
`sessions: SessionStore` **by value**. So **every call in this phase is
`with_sessions(&sessions, |store| …)`** — with the `&`.

### `hook.rs` defines its own local `SessionStore` alias — leave it alone

```rust
// src/daemon/hook.rs:11-12
type SessionStore =
    Arc<std::sync::Mutex<std::collections::HashMap<String, crate::daemon::session::SessionEntry>>>;
```

This is a *local* alias, not the canonical `crate::daemon::session::SessionStore`.
It expands to the identical type, and Rust type aliases are transparent, so
`with_sessions(&sessions, …)` type-checks against it without any change.

**Do not replace the local alias with the canonical import.** That is a separate
cleanup, it is not needed to make this phase compile, and touching it widens the
diff for no behavioral gain.

### Imports

```rust
// src/daemon/stream.rs:6
use crate::daemon::session::{SessionStore, append_session_message, write_session_file};
```

Add `with_sessions` to that list.

**`hook.rs` imports nothing from `crate::daemon::session` today** (it has the local
alias instead). Add a new line:

```rust
use crate::daemon::session::with_sessions;
```

### `UnpoisonExt`: delete from `stream.rs`, **keep** in `hook.rs`

This differs between the two files and getting it backwards breaks the build.

- **`stream.rs`** has exactly one `unwrap_or_log` — task 1's site — and its
  `#[cfg(test)]` module (line 1141+) does not use it. After task 1,
  `use crate::util::UnpoisonExt;` is unused. **Delete it.** (`stream.rs:719`, which
  you are *not* converting, uses `if let Ok(store) = sessions.lock()` and needs no
  trait.)
- **`hook.rs`** uses `unwrap_or_log` at line 116 on a **different mutex**:
  `*bg_session.lock().unwrap_or_log() = session_name.clone();`. That is
  `bg_session`, not `sessions`. **Keep `hook.rs`'s `UnpoisonExt` import** — deleting
  it breaks the build.

### Site inventory — 10 sites in scope

| # | File:line | Shape |
|---|---|---|
| 1 | `stream.rs:107` | scoped read inside `.and_then`; the only `unwrap_or_log` |
| 2 | `stream.rs:662` | 3-element chain → cost accumulation |
| 3 | `stream.rs:689` | `if let Ok(..) && let Some(..)` → the persist block. **Closure must close before the compaction region** |
| 4 | `stream.rs:751` | **6-element chain that mutates and yields a `bool`** — the one non-mechanical site |
| 5 | `stream.rs:828` | 3-element chain → cost accumulation (same shape as #2) |
| 6 | `stream.rs:856` | 3-element chain → token tracking |
| 7 | `stream.rs:1012` | 4-element chain, `!APPROVAL_GATED.contains(..)` first |
| 8 | `stream.rs:896` | **multi-line** `.ok()?` chain inside `.and_then` — invisible to `grep -c`. Listed last deliberately, so the site `grep` misses gets its own task |
| 9 | `hook.rs:163` | `if let Ok(mut store)` → `for entry in store.values_mut()` |
| 10 | `hook.rs:186` | same shape as #9 |

**Not in scope:** `stream.rs:719` and `hook.rs:91`. Phase 05.

### Worked example — the 3-element chain, already converted in this codebase

`src/daemon/ghost.rs:850` is the shape for tasks 2, 5, 6, and 7:

```rust
                        with_sessions(sessions, |store| {
                            if let Some(entry) = store.get_mut(session_id) {
                                entry.cost_usd += record.cost.total_cost_usd;
                                …
                            }
                        });
```

Here the receiver is `&sessions`, and the outer `if let Some(ref id) = session_id`
stays **outside** the closure — it decides whether to acquire at all.

## Spec

### 1. `stream.rs:107` — loaded-tools read

```rust
        let loaded_tools: Vec<String> = session_id
            .as_deref()
            .and_then(|sid| {
                let store = sessions.lock().unwrap_or_log();
                store
                    .get(sid)
                    .map(|e| e.loaded_tools.iter().cloned().collect())
            })
            .unwrap_or_default();
```

becomes:

```rust
        let loaded_tools: Vec<String> = session_id
            .as_deref()
            .and_then(|sid| {
                with_sessions(&sessions, |store| {
                    store
                        .get(sid)
                        .map(|e| e.loaded_tools.iter().cloned().collect())
                })
            })
            .unwrap_or_default();
```

### 2. `stream.rs:662` — cost accumulation

```rust
                        // Accumulate cost on the session entry.
                        if let Some(ref id) = session_id
                            && let Ok(mut store) = sessions.lock()
                            && let Some(entry) = store.get_mut(id)
                        {
                            entry.cost_usd += record.cost.total_cost_usd;
                            *entry
                                .cost_by_agent
                                .entry(record.agent_name.clone())
                                .or_insert(0.0) += record.cost.total_cost_usd;
                            if record.pricing_source == PricingSource::Unknown {
                                entry.has_untracked_cost = true;
                            }
                        }
```

becomes:

```rust
                        // Accumulate cost on the session entry.
                        if let Some(ref id) = session_id {
                            with_sessions(&sessions, |store| {
                                if let Some(entry) = store.get_mut(id) {
                                    entry.cost_usd += record.cost.total_cost_usd;
                                    *entry
                                        .cost_by_agent
                                        .entry(record.agent_name.clone())
                                        .or_insert(0.0) += record.cost.total_cost_usd;
                                    if record.pricing_source == PricingSource::Unknown {
                                        entry.has_untracked_cost = true;
                                    }
                                }
                            });
                        }
```

`.cost_by_agent.entry(..)` is `HashMap::entry` on a **field** — unrelated to the
store's entry. Do not rename anything.

### 3. `stream.rs:689` — the persist block, and where its closure must end

**This is the most consequential boundary in the phase.** Current code:

```rust
                        // Persist the conversation for the next turn.
                        if let Some(ref id) = session_id {
                            if let Ok(mut store) = sessions.lock()
                                && let Some(entry) = store.get_mut(id)
                            {
                                entry.messages = messages.clone();
                                entry.last_accessed = Instant::now();
                                entry.last_prompt_tokens = usage.input_tokens
                                    + usage.cache_read_tokens
                                    + usage.cache_write_tokens;
                                crate::daemon::context::estimate::update_token_scale(
                                    entry, &messages,
                                );
                                entry.dirty = true;
                                if chat_pane.is_some() {
                                    entry.chat_pane = chat_pane.clone();
                                }
                            }
                            if needs_compaction {
                                …
```

Convert **only the inner `if let Ok(..)` block**:

```rust
                        // Persist the conversation for the next turn.
                        if let Some(ref id) = session_id {
                            with_sessions(&sessions, |store| {
                                if let Some(entry) = store.get_mut(id) {
                                    entry.messages = messages.clone();
                                    entry.last_accessed = Instant::now();
                                    entry.last_prompt_tokens = usage.input_tokens
                                        + usage.cache_read_tokens
                                        + usage.cache_write_tokens;
                                    crate::daemon::context::estimate::update_token_scale(
                                        entry, &messages,
                                    );
                                    entry.dirty = true;
                                    if chat_pane.is_some() {
                                        entry.chat_pane = chat_pane.clone();
                                    }
                                }
                            });
                            if needs_compaction {
                                …
```

**The closure must close with `});` before `if needs_compaction`.** Everything from
there on stays exactly where it is and outside the closure:

- `write_session_file(id, &messages)` / `append_session_message(id, msg)` — file
  writes;
- `stream.rs:719`'s block — **not yours** (phase 05);
- `spawn_compaction(...)`, which **re-locks the store internally**.

That last one matters most. The code already carries the warning:

```rust
                            // Spawn background compaction if the turn signaled it.
                            // spawn_compaction takes the lock itself (snapshot) and
                            // no-ops on ghost / already-in-flight sessions, so we
                            // must NOT hold the lock here — std::sync::Mutex is not
                            // reentrant and re-locking would deadlock.
```

**Keep that comment verbatim.** A `with_sessions` closure enclosing
`spawn_compaction` would now trip the re-entrancy assertion — a loud panic rather
than the silent hang it used to be, because `context/background.rs` was converted
earlier — but it is still a bug. Do not widen the closure toward it.

### 4. `stream.rs:751` — the six-element chain that mutates and yields a `bool`

The one non-mechanical site. Current code:

```rust
                            let should_suggest = if let Ok(mut store) = sessions.lock()
                                && let Some(ref id) = session_id
                                && let Some(entry) = store.get_mut(id)
                                && entry.saved_name.is_none()
                                && !entry.auto_name_suggested
                                && entry.turn_count == config.sessions.auto_name_turn_threshold
                            {
                                entry.auto_name_suggested = true;
                                true
                            } else {
                                false
                            };
```

Note what this does: it is a **guard and a mutation at once** — it decides whether
to suggest a name *and* sets `auto_name_suggested = true` so the suggestion fires
exactly once. The `if let Ok(mut store) = sessions.lock()` is the **first** element,
so the lock is acquired before `session_id` is even checked.

Target — closure returns the `bool`, and the acquisition moves inside:

```rust
                            let should_suggest = with_sessions(&sessions, |store| {
                                if let Some(ref id) = session_id
                                    && let Some(entry) = store.get_mut(id)
                                    && entry.saved_name.is_none()
                                    && !entry.auto_name_suggested
                                    && entry.turn_count == config.sessions.auto_name_turn_threshold
                                {
                                    entry.auto_name_suggested = true;
                                    true
                                } else {
                                    false
                                }
                            });
```

Three things to get right:

- **All five remaining conditions stay, in this order, inside the closure.** They
  are a short-circuit chain: `saved_name.is_none()` and `!auto_name_suggested` and
  the exact `==` on the threshold. Reordering or loosening any of them changes when
  the one-shot suggestion fires.
- **`entry.auto_name_suggested = true` must stay inside the closure**, before the
  `true`. Setting it after `with_sessions` returns would let two turns both pass
  the check and suggest twice.
- The `== config.sessions.auto_name_turn_threshold` is an **equality**, not `>=`.
  Leave it. That is what makes the suggestion fire exactly once rather than on
  every subsequent turn.

The `if should_suggest && let Some((name, desc)) = auto_name::suggest_session_name(&messages, config).await`
that follows is an `.await` and stays **outside** the closure. `with_sessions`
takes a synchronous `FnOnce`, so putting it inside would not compile.

### 5. `stream.rs:828` — cost accumulation, second occurrence

Byte-identical in shape to task 2, in a different match arm. Apply the same
rewrite. **Do not attempt to factor the two into a helper** — three similar lines
are better than a premature abstraction (`STANDARDS.md` § 2.2), and the two arms
may diverge later.

### 6. `stream.rs:856` — token tracking

```rust
                    if let Some(ref id) = session_id
                        && let Ok(mut store) = sessions.lock()
                        && let Some(entry) = store.get_mut(id)
                    {
                        entry.last_prompt_tokens =
                            usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
                        crate::daemon::context::estimate::update_token_scale(entry, &messages);
                    }
```

becomes:

```rust
                    if let Some(ref id) = session_id {
                        with_sessions(&sessions, |store| {
                            if let Some(entry) = store.get_mut(id) {
                                entry.last_prompt_tokens = usage.input_tokens
                                    + usage.cache_read_tokens
                                    + usage.cache_write_tokens;
                                crate::daemon::context::estimate::update_token_scale(
                                    entry, &messages,
                                );
                            }
                        });
                    }
```

`update_token_scale` is pure arithmetic on the entry — safe inside the closure.

### 7. `stream.rs:1012` — per-session tool counter

```rust
                                if !APPROVAL_GATED.contains(&tool_name)
                                    && let Some(id) = &session_id
                                    && let Ok(mut store) = sessions.lock()
                                    && let Some(entry) = store.get_mut(id)
                                {
                                    entry.tool_calls_this_session += 1;
                                }
```

becomes:

```rust
                                if !APPROVAL_GATED.contains(&tool_name)
                                    && let Some(id) = &session_id
                                {
                                    with_sessions(&sessions, |store| {
                                        if let Some(entry) = store.get_mut(id) {
                                            entry.tool_calls_this_session += 1;
                                        }
                                    });
                                }
```

The `APPROVAL_GATED` check must stay **outside** — it is the cheap test and gates
whether to acquire at all.

### 8. `stream.rs:896` — the multi-line acquisition

**This is the site `grep -c` cannot see.** Current code:

```rust
                                let session_tool_count = session_id
                                    .as_ref()
                                    .and_then(|id| {
                                        sessions
                                            .lock()
                                            .ok()?
                                            .get(id)
                                            .map(|e| e.tool_calls_this_session)
                                    })
                                    .unwrap_or(0);
```

becomes:

```rust
                                let session_tool_count = session_id
                                    .as_ref()
                                    .and_then(|id| {
                                        with_sessions(&sessions, |store| {
                                            store.get(id).map(|e| e.tool_calls_this_session)
                                        })
                                    })
                                    .unwrap_or(0);
```

The `.ok()?` disappears — `with_sessions` recovers from poison via
`unwrap_or_log()` instead of bailing to `None`, which is the intended direction for
this whole sequence and matches the `CLAUDE.md` invariant. `.unwrap_or(0)` still
handles "no session id" and "entry absent".

### 9. `hook.rs:163` — clear detach state on client attach

```rust
    if let Ok(mut store) = sessions.lock() {
        for entry in store.values_mut() {
            if entry.tmux_session == session_name {
                entry.last_detach = None;
                entry.detach_time_utc = None;
            }
        }
    }
```

becomes:

```rust
    with_sessions(&sessions, |store| {
        for entry in store.values_mut() {
            if entry.tmux_session == session_name {
                entry.last_detach = None;
                entry.detach_time_utc = None;
            }
        }
    });
```

The `send_response_split(tx, Response::Ok).await?` that follows stays outside — it
is an `.await` and would not compile inside.

### 10. `hook.rs:186` — record detach state on client detach

```rust
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    if let Ok(mut store) = sessions.lock() {
        for entry in store.values_mut() {
            if entry.tmux_session == session_name {
                entry.last_detach = Some(now);
                entry.detach_time_utc = Some(now_utc);
                entry.messages_at_detach = entry.messages.len();
            }
        }
    }
```

becomes:

```rust
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    with_sessions(&sessions, |store| {
        for entry in store.values_mut() {
            if entry.tmux_session == session_name {
                entry.last_detach = Some(now);
                entry.detach_time_utc = Some(now_utc);
                entry.messages_at_detach = entry.messages.len();
            }
        }
    });
```

`now` / `now_utc` are computed **before** the closure and must stay there — taking
the timestamps inside would measure lock-acquisition time rather than detach time.

### 11. Do not widen any closure — this phase's hazard surface is the largest yet

`run_conversation_loop` dispatches tool calls and spawns compaction, so the calls a
closure must **not** enclose are dense here:

| Callee | Why it must stay outside |
|---|---|
| `spawn_compaction` (`stream.rs:738`) | **re-locks the store**; would trip the re-entrancy assertion |
| `stream.rs:719`'s block | still a raw acquisition — phase 05 |
| `hook.rs:91`'s block | still a raw acquisition — phase 05 |
| `write_session_file`, `append_session_message`, `write_session_meta` | file writes |
| `executor::execute_tool_call` and everything under it | reaches `webhook/process.rs`'s 2 raw acquisitions via `inject_ghost_event` |
| `auto_name::suggest_session_name(..).await`, `send_response_split(..).await` | `.await` — will not compile inside a closure |

Every closure this phase introduces reads or writes one entry (or iterates
`values_mut`) and returns immediately. **Keep it that way.**

### 12. Delete `stream.rs`'s `UnpoisonExt` import; keep `hook.rs`'s

After task 1, run:

```bash
grep -n "unwrap_or_log" src/daemon/stream.rs   # expect nothing → delete the import
grep -n "unwrap_or_log" src/daemon/hook.rs     # expect line 116 (bg_session) → KEEP the import
```

`hook.rs:116` is `*bg_session.lock().unwrap_or_log() = session_name.clone();` — a
**different mutex**, untouched by this phase. Deleting `hook.rs`'s import breaks
the build.

Verify with **both** `cargo build` **and**
`cargo clippy --all-targets --all-features -- -D warnings`. They disagree about
whether a test-only import counts as used, and that disagreement caused a
`hard_fail` earlier in this milestone.

## Acceptance criteria

**Two criteria are deliberately non-zero.** Each file keeps exactly 1 raw
acquisition — phase 05's restructure sites. A **0** means you converted out of
scope.

- [ ] `python3 /tmp/scan_locks.py src/daemon/stream.rs` prints **1**.
- [ ] `python3 /tmp/scan_locks.py src/daemon/hook.rs` prints **1**.
- [ ] `grep -c "with_sessions(" src/daemon/stream.rs` returns **8**.
- [ ] `grep -c "with_sessions(" src/daemon/hook.rs` returns **2**.
- [ ] `grep -c "UnpoisonExt" src/daemon/stream.rs` returns **0**.
- [ ] `grep -c "UnpoisonExt" src/daemon/hook.rs` returns **1** — still needed for
      `bg_session`.
- [ ] `grep -c "spawn_compaction" src/daemon/stream.rs` returns **2** (the comment
      and the call), and the call is **not** inside a `with_sessions` closure —
      verify by reading; the count cannot prove it.
- [ ] `python3 /tmp/scan_locks.py src/daemon/ghost.rs src/daemon/background/run.rs src/daemon/background/respawn.rs src/daemon/context/background.rs src/daemon/executor/mod.rs`
      prints **0** for all five (earlier phases untouched).
- [ ] `python3 /tmp/scan_locks.py src/daemon/background/helpers.rs src/daemon/background/gc.rs`
      still prints **1** each — also phase 05's, also untouched.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. **916 means
      scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text including comments. **Do not write the
literal `sessions.lock()`, `with_sessions(`, or `UnpoisonExt` in a new comment** in
either file.

## Test plan

Behavior-preserving refactor: the existing **915** tests are the regression net and
must all still pass, unchanged. **Write no new tests.**

`stream.rs`'s `#[cfg(test)]` module (line 1141+) does not exercise
`run_conversation_loop` — it is a ~1000-line async function requiring a live AI
client, a tmux session, and an IPC peer. `hook.rs` has **no** test module at all.
So **none of the ten sites in this phase is covered by the unit suite.** That is a
pre-existing gap this phase neither widens nor closes.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do **not** claim any test "guards" or "covers" one of
these sites — here such a claim would be plainly false, and in this project a
statement about what a test would catch is admissible only when demonstrated by
mutation. This phase requires no mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Task 3 boundary.** Confirm the persist closure closes with `});` **before**
   `if needs_compaction`, and that `spawn_compaction` is outside it. Quote the
   line the closure closes on.
2. **Task 4 one-shot semantics.** Confirm `entry.auto_name_suggested = true` is
   still inside the closure and the threshold comparison is still `==`, not `>=`.
3. **`UnpoisonExt` asymmetry.** Confirm it was deleted from `stream.rs` and
   **kept** in `hook.rs`, and state why (`hook.rs:116` uses it on `bg_session`).

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key, no
> file the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon, or
the chat client. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

- [x] May delete `use crate::util::UnpoisonExt;` from `src/daemon/stream.rs`
      (task 12). **Not** from `hook.rs`.
- [x] May add `use crate::daemon::session::with_sessions;` to `src/daemon/hook.rs`,
      which currently imports nothing from that module.

This phase adds no tests, so it needs no `HOME` redirection and no `unsafe`. If you
think you need `unsafe` or a new dependency, **stop and report a blocker**.

## Out of scope

- **Do not touch `stream.rs:719`.** It holds the guard across `write_session_meta`
  (a file write) and needs a hoist — build the `SessionMeta` inside, write outside.
  **Phase 05.** A criterion pins `stream.rs` at 1 so an over-eager conversion is
  caught.
- **Do not touch `hook.rs:91`.** Its `store.retain(|_, entry| { … entry.cleanup_bg_windows(); … })`
  calls a function that spawns **one tmux subprocess per background window plus
  `stop_pipe_pane`**, all under the global lock — the same defect class as the
  confirmed production hang. It needs the collect-then-act restructure that
  `cleanup_pass` (`src/daemon/session.rs`) already models. **Phase 05.**
- **Do not replace `hook.rs`'s local `SessionStore` type alias** with the canonical
  import. Not needed, and out of scope.
- **Do not delete `hook.rs`'s `UnpoisonExt` import.** Task 12 explains.
- **Do not factor tasks 2 and 5 into a shared helper.**
- **Do not convert `webhook/process.rs`.** Phase 05.
- **Do not re-touch `ghost.rs`, `background/run.rs`, `background/respawn.rs`,
  `context/background.rs`, `briefing.rs`, or `executor/`.** Done; pinned by
  criteria.
- **Do not change `SessionStore` into a newtype** and do not touch the 13
  `Arc::clone` sites.
- **Do not widen a closure over `spawn_compaction`, a file write, a tool dispatch,
  or an `.await`.** Task 11 tabulates them.
- **Do not reword the `spawn_compaction` warning comment.** It records why the
  lock must not be held there.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to a `let`-chain
  inside a closure, report a blocker rather than suppressing.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 18:40 (started)

**Executor:** model (phase-04j)

Converted all 10 `sessions.lock()` sites to `with_sessions`: 8 in `stream.rs`, 2 in `hook.rs`. Each file retains exactly 1 raw acquisition (phase 05's restructure sites).

### Update — ts=1785091863342 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- `scan_locks.py` on `stream.rs`: **1**, `hook.rs`: **1** ✓
- `with_sessions(` count: `stream.rs` **8**, `hook.rs` **2** ✓
- `UnpoisonExt`: `stream.rs` **0**, `hook.rs` **1** ✓
- `spawn_compaction` count: **2** (comment + call), call is outside closure ✓
- Prior-phase files all at **0**; phase-05 files at **1** each ✓
- `SessionStore` type alias unchanged at line 117 ✓
- `cargo build`: clean ✓
- `cargo clippy`: clean ✓
- `cargo fmt --all`: clean ✓
- `cargo test`: **915** passed, 0 failed ✓
- Working tree clean, one conventional commit ✓

**Summary + Notes for review:**

Converted all 10 `sessions.lock()` sites to `with_sessions` across `stream.rs` (8) and `hook.rs` (2), with each file retaining exactly 1 raw acquisition for phase 05's restructures. Task 3's persist closure closes with `});` before `if needs_compaction`, keeping `spawn_compaction` outside — the re-entrancy hazard is preserved. Task 4's one-shot auto-name semantics are intact: `entry.auto_name_suggested = true` remains inside the closure and the threshold comparison is still `==`. `UnpoisonExt` was deleted from `stream.rs` (no longer used after task 1) and kept in `hook.rs` (still needed for `bg_session` at line 116). All 915 tests pass unchanged.

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
:cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test memory::tests::session_memory_block_respects_cap ... ok
test memory::tests::update_memory_append_mode ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test memory::tests::update_memory_replace_body ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.24s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ghost_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

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
- `docs/dev/milestones/M5-ux-stability/phase-04j-convert-stream-hooks.md` — +7 -1
- `src/daemon/hook.rs` — +5 -4
- `src/daemon/stream.rs` — +81 -72

**Commit:** 3e8466e553526a2d3fd73e32ce067766ec08596b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key.

### Review verdict — 2026-07-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (89 turns)
- **Scope deviations:** none. Only `stream.rs` and `hook.rs` touched, and each
  retained exactly the one raw acquisition phase 05 owns.
- **Calibration:** none.

**Independent re-run at review** (separate invocations, not chained):

```
cargo fmt --all --check                                    → exit 0
cargo build                                                → exit 0, no warnings
cargo clippy --all-targets --all-features -- -D warnings   → exit 0
cargo test  → 915 lib-unit / 0 failed (unchanged); 27 integration / 2 ignored
```

**Acceptance criteria — all four non-zero ones held:**

| Check | Result |
|---|---|
| `scan_locks.py` `stream.rs` / `hook.rs` | **1 / 1** ✓ — phase 05's sites left in place |
| `with_sessions(` `stream.rs` / `hook.rs` | **8 / 2** ✓ |
| `UnpoisonExt` `stream.rs` / `hook.rs` | **0 / 1** ✓ — the asymmetry, correct in both directions |
| `helpers.rs` / `gc.rs` | **1 / 1** ✓ — also phase 05's, untouched |
| `ghost.rs`, `background/run.rs`, `background/respawn.rs`, `context/background.rs`, `executor/mod.rs` | **0** each ✓ |
| `spawn_compaction` occurrences | **2** ✓ (comment + call) |
| `pub type SessionStore` still an alias | ✓ |
| lib-unit tests | **915**, unchanged ✓ |

**The remaining raw acquisitions are precisely the intended two:**
`stream.rs:722` (the `write_session_meta` guard) and `hook.rs:92` (the
`cleanup_bg_windows` retain). Both phase 05.

**All twelve spec tasks implemented as written.** The four things a count cannot
prove, each verified by reading:

- **Task 3's boundary is correct.** The persist closure closes with `});` on the
  line immediately before `if needs_compaction {` (`stream.rs:707-708`). Everything
  downstream — `write_session_file`, `append_session_message`, `stream.rs:722`, and
  `spawn_compaction` — is outside it.
- **`spawn_compaction` is not inside any closure.** It sits at `stream.rs:740-746`
  under `if wants_background_compaction {`, well after the closure closed. Had it
  been enclosed, the re-entrancy assertion would now panic rather than hang
  silently — but it was not enclosed at all.
- **The `spawn_compaction` warning comment survives verbatim**, including the
  "`std::sync::Mutex` is not reentrant and re-locking would deadlock" line. That
  comment is the institutional memory of a confirmed production defect.
- **Task 4's one-shot semantics are intact.**
  `entry.auto_name_suggested = true;` is inside the closure before the `true`; the
  threshold test is still `==`, not `>=`; all five conditions remain in order; and
  `auto_name::suggest_session_name(..).await` is outside the closure. Moving the
  flag out would let two turns both suggest; loosening `==` would suggest every
  turn thereafter. Neither would fail a test.

No forbidden idioms in the added lines.

**Fifth consecutive phase where the corrected drafting practices held.** Criteria
were validated against the tree before pinning (third draft running with no
correction needed), the Pre-flight stated the `grep -c` 8 vs scan 9 discrepancy so
it read as expected rather than stale, and the Test plan named no discriminating
test — it stated that **none** of the ten sites is covered by the unit suite
(`run_conversation_loop` needs a live AI client, tmux session and IPC peer;
`hook.rs` has no test module), which made a coverage claim impossible rather than
merely unproven. The Update Log made none.

**Milestone position:** the 04x conversion sweep is now **complete except for
phase 05's six restructures**. Converted: `handlers.rs`, `ask.rs` (bar two
multi-line stragglers), the whole `executor/` subtree, `context/background.rs`,
`briefing.rs`, `ghost.rs`, `background/run.rs`, `background/respawn.rs`,
`stream.rs`, `hook.rs`.
