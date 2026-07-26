# Phase 05b: Get the Last Blocking Work Out of the Session Lock

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-05a (subprocess-under-lock restructures) — `done`
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix the **3** remaining sites that do blocking work while holding the global
session lock. Like 05a these are restructures, not conversions: collect under the
lock, release, then act.

| Site | Blocking work currently under the guard |
|---|---|
| `webhook/process.rs:162` | a `tmux display-message` subprocess **per session**, inside `for entry in guard.values()` |
| `webhook/process.rs:149` | `append_session_message` **per session** — two file writes each |
| `stream.rs:722` | `write_session_meta` — a file write inside a `let`-chain guard |

**Finish condition: 0 raw `sessions.lock()` in both files, with `with_sessions(`
at 2 in `webhook/process.rs` and 9 in `stream.rs`.**

**This is a bugfix phase, not a refactor.** After it lands, no `SessionStore`
critical section anywhere in the daemon performs blocking work — that is the
milestone's third exit criterion, and 05b is the phase that closes it.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism A — lock held across blocking work —
  and § 1.5b–1.5c, the confirmed hang: every thread `futex`-parked, the reactor
  gone, zero CPU over 12 h. That hang was a `SessionStore` critical section doing
  blocking work.
- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers. Task 1 spawns one per session.
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
python3 /tmp/scan_locks.py src/webhook/process.rs src/daemon/stream.rs
#   src/webhook/process.rs: 2
#   src/daemon/stream.rs: 1
grep -c "with_sessions(" src/webhook/process.rs   # expect 0
grep -c "with_sessions(" src/daemon/stream.rs     # expect 8
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker** — the per-site code below is stale.

## Current state

### ⭐ The worked example — `notify_session`, landed by 05a in the file next door

`src/daemon/background/helpers.rs`. This is the closest analogue in the codebase
and it carries **both** shapes this phase needs — a value cloned out under the
lock and used to spawn a subprocess after it, and a file write moved out
entirely:

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

    // … formatting and the file write happen here, unlocked …

    // Phase 4 (unlocked): the tmux notification.
    if let Some(ref cp) = chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
```

**The property to copy:** what crosses the lock boundary is **owned data**
(`entry.chat_pane.clone()`), never a borrow into the map. Tasks 1 and 2 are this
same move, plural — collect a `Vec` instead of a single `Option`.

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

Generic over `T` — the closure may return a `Vec`, an `Option`, anything. That
return channel **is** the collect-then-act mechanism.

### Receiver forms differ per file — check each

- `process.rs`: both functions take `sessions: &SessionStore` →
  `with_sessions(sessions, …)`, **no** `&`.
- `stream.rs`: destructures `sessions` by value out of `ConversationLoopCtx` and
  all 8 existing calls are `with_sessions(&sessions, …)` → **with** the `&`.

Getting this wrong is a type error, not a silent bug — but do not guess it.

### Imports

**`stream.rs` already imports `with_sessions`** (it has 8 calls). Do not touch its
imports.

**`process.rs` does not.** This is the actual current line, read from the tree:

```rust
// before
use crate::daemon::session::{SessionStore, append_session_message};
// after
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};
```

### ⚠ `UnpoisonExt` **stays** in `process.rs` — deleting it breaks the build

The last five phases all ended in "delete the now-unused import," which makes this
the easy mistake. `process.rs` imports it on line 6, from
`crate::daemon::utils` (**not** `crate::util`):

```rust
use crate::daemon::utils::{UnpoisonExt, fire_notification, log_event};
```

It has **four** `unwrap_or_log` calls. Two are the sites this phase converts
(`:149`, `:162`); the other two are on **entirely different mutexes** and are out
of scope:

- `process.rs:45` — `state.dedup.lock().unwrap_or_log()`
- `process.rs:275` — `state.rate_limit.lock().unwrap_or_log()`

So after this phase `UnpoisonExt` is still used twice and the import **must
remain**. `grep -c "UnpoisonExt" src/webhook/process.rs` stays at **1**.

`stream.rs` has no `UnpoisonExt` import — 04j removed it — and must not gain one.
Its site at `:722` uses `let Ok(store) = sessions.lock()`, with no `unwrap_or_log`.

## Spec

### 1. `process.rs:162` — collect the panes, then spawn outside

Current:

```rust
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let guard = sessions.lock().unwrap_or_log();
    for entry in guard.values() {
        if let Some(ref pane) = entry.chat_pane {
            let _ = std::process::Command::new("tmux")
                .args(["display-message", "-d", "8000", "-t", pane, msg])
                .output();
        }
    }
}
```

One `tmux` subprocess **per active session**, every one of them under the global
lock. Target:

```rust
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let panes: Vec<String> = with_sessions(sessions, |store| {
        store.values().filter_map(|e| e.chat_pane.clone()).collect()
    });

    // Unlocked phase: everything blocking happens out here.
    for pane in &panes {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "8000", "-t", pane, msg])
            .output();
    }
}
```

`filter_map` + `clone` collapses the `if let Some(ref pane)` into the collect
step, so sessions with no chat pane are dropped under the lock exactly as before.

### 2. `process.rs:149` — collect the ids, then write outside

Current:

```rust
pub(crate) fn inject_into_sessions(sessions: &SessionStore, msg: &Message) {
    let guard = sessions.lock().unwrap_or_log();
    for (sid, entry) in guard.iter() {
        append_session_message(sid, msg);
        // In-memory is intentionally NOT updated here — the next Ask request
        // will re-read from disk when the in-memory history is stale.
        // For sessions currently in flight this means the alert appears in the
        // turn after the one already in progress, which is acceptable.
        let _ = entry; // suppress unused-variable warning
    }
}
```

`append_session_message` writes **two files** per session, all under the lock.
Target:

```rust
pub(crate) fn inject_into_sessions(sessions: &SessionStore, msg: &Message) {
    let ids: Vec<String> = with_sessions(sessions, |store| store.keys().cloned().collect());

    // In-memory is intentionally NOT updated here — the next Ask request
    // will re-read from disk when the in-memory history is stale.
    // For sessions currently in flight this means the alert appears in the
    // turn after the one already in progress, which is acceptable.
    for sid in &ids {
        append_session_message(sid, msg);
    }
}
```

**Two details, both deliberate:**

- **`let _ = entry;` disappears.** It existed only to suppress an
  unused-variable warning from binding `entry` in the `for` pattern. Collecting
  `keys()` never binds it, so the suppression has nothing to suppress. **Do not
  carry it forward** — a `let _ = …` with no unused variable is dead code.
- **Keep the four-line comment**, moved to sit above the new loop. It documents a
  deliberate design decision about staleness, not the mechanics of the loop, and
  it is the only record of that decision. Reword nothing.

### 3. `stream.rs:722` — build the meta inside, write it outside

Current:

```rust
                            if !is_ghost_session
                                && let Ok(store) = sessions.lock()
                                && let Some(entry) = store.get(id)
                            {
                                let meta = crate::daemon::session::SessionMeta {
                                    started_at: entry.started_at,
                                    turn_count: entry.turn_count,
                                    last_prompt_tokens: entry.last_prompt_tokens,
                                    token_scale: entry.token_scale,
                                    tool_calls_this_session: entry.tool_calls_this_session,
                                    saved_name: entry.saved_name.clone(),
                                };
                                crate::daemon::session::write_session_meta(id, &meta);
                            }
```

Target:

```rust
                            if !is_ghost_session {
                                let meta = with_sessions(&sessions, |store| {
                                    store.get(id).map(|entry| {
                                        crate::daemon::session::SessionMeta {
                                            started_at: entry.started_at,
                                            turn_count: entry.turn_count,
                                            last_prompt_tokens: entry.last_prompt_tokens,
                                            token_scale: entry.token_scale,
                                            tool_calls_this_session: entry.tool_calls_this_session,
                                            saved_name: entry.saved_name.clone(),
                                        }
                                    })
                                });
                                if let Some(meta) = meta {
                                    crate::daemon::session::write_session_meta(id, &meta);
                                }
                            }
```

**Three things this must preserve, all of which the current `let`-chain gets
right by accident of its shape:**

1. **The ghost skip.** `!is_ghost_session` still gates everything — including the
   lock acquisition. Ghost sessions have one-shot UUID ids that are never
   recreated, so a ghost meta file is a write-only orphan. The existing comment
   above the block says this; leave it in place.
2. **The write is conditional on the entry existing.** `store.get(id)` returning
   `None` must still mean *no file is written*. That is why the closure returns
   `Option<SessionMeta>` and the write is gated on it. An unconditional write
   after the closure would create a meta file for a session that has been
   evicted — and every gate would stay green.
3. **All six fields, unchanged.** `saved_name` is the only one that clones.

### 4. Verify `spawn_compaction` is still outside any closure

Immediately below task 3's block is this, which **must not move and must not be
drawn into a closure**:

```rust
                            // Spawn background compaction if the turn signaled it.
                            // spawn_compaction takes the lock itself (snapshot) and
                            // no-ops on ghost / already-in-flight sessions, so we
                            // must NOT hold the lock here — std::sync::Mutex is not
                            // reentrant and re-locking would deadlock.
                            if wants_background_compaction {
                                crate::daemon::context::background::spawn_compaction(
```

That comment is the institutional memory of a confirmed production defect. **Keep
it verbatim**, and confirm by reading that `spawn_compaction` sits outside task
3's `with_sessions` closure. Since `context/background.rs` was converted in 04f,
a closure enclosing it would now trip the re-entrancy **assertion** — a loud panic
rather than the silent hang it used to be. Still a bug, just a louder one.

### 5. Confirm the import outcome in both files

Run `grep -c "UnpoisonExt" src/webhook/process.rs` — expect **1**, unchanged. If
you find yourself deleting it, re-read the § "`UnpoisonExt` **stays**" section
above: two of its four uses are on different mutexes and are not this phase's.

`stream.rs` must gain no import at all.

## Acceptance criteria

- [ ] `python3 /tmp/scan_locks.py src/webhook/process.rs src/daemon/stream.rs`
      prints **0** for both.
- [ ] `grep -c "with_sessions(" src/webhook/process.rs` returns **2**.
- [ ] `grep -c "with_sessions(" src/daemon/stream.rs` returns **9** (8 pre-existing + 1).
- [ ] `grep -c "UnpoisonExt" src/webhook/process.rs` returns **1** — **not 0.**
      Still needed for `state.dedup` and `state.rate_limit`.
- [ ] `grep -c "UnpoisonExt" src/daemon/stream.rs` returns **0** — unchanged; the
      file must not gain the import.
- [ ] `grep -c "unwrap_or_log" src/webhook/process.rs` returns **2** — down from 4.
      The two that remain are `:45` and `:275`, on different mutexes.
- [ ] `grep -c "let _ = entry" src/webhook/process.rs` returns **0** — the
      unused-variable suppression is gone with the loop that needed it.
- [ ] `grep -c "spawn_compaction" src/daemon/stream.rs` returns **2**, and neither
      is inside a `with_sessions` closure — **verify by reading; the count cannot
      prove it.**
- [ ] `grep -cF "reentrant and re-locking would deadlock" src/daemon/stream.rs`
      returns **1** — the re-entrancy warning comment survives byte-identical.
      (The comment wraps across two lines, so this literal is the tail half of the
      sentence. Matching the whole sentence returns **0** — it spans a line break,
      and `grep -F` is line-oriented.)
- [ ] `grep -cF "In-memory is intentionally NOT updated here" src/webhook/process.rs`
      returns **1** — the staleness comment survives byte-identical.
- [ ] `python3 /tmp/scan_locks.py src/daemon/hook.rs src/daemon/background/gc.rs src/daemon/background/helpers.rs src/daemon/ghost.rs src/daemon/context/background.rs`
      prints **0** for all five — earlier phases untouched.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **916 means scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text **including comments** — see the note below
on `session.rs:443`. **Do not write the literal `sessions.lock()`,
`with_sessions(`, `spawn_compaction`, or `unwrap_or_log` in a new comment** in
these files.

### The expected end state of the whole daemon

After this phase the scan over `src/` should report exactly **4** production
acquisitions, and every one of them is either legitimate or already assigned:

| File:line | What it is |
|---|---|
| `session.rs:432` | **`with_sessions` itself** — the one real acquisition, by design |
| `session.rs:443` | **not code** — a doc comment on `cleanup_pass` containing the literal `sessions.lock()`. The scan is text-based and counts it. |
| `ask.rs:519`, `:686` | the two known multi-line stragglers — **phase 05c's**, not yours |

**Do not touch any of these four.** Converting `session.rs:432` would make
`with_sessions` call itself; "fixing" `:443` means editing a comment that is
correct as written; and the `ask.rs` pair belongs to 05c, where the newtype makes
them fail to compile if missed.

## Test plan

Behavior-preserving restructure: which panes get notified, what lands in each
session's JSONL, and what gets written to the meta file are all unchanged. Only
*when the lock is held* changes. The existing **915** tests are the regression
net. **Write no new tests.**

`webhook/process.rs` has a `#[cfg(test)]` module at line 478, but it covers only
pure functions — `severity_rank`, `camel_to_kebab`, and `parse_ghost_trigger`.
None of them touches a `SessionStore`. `stream.rs`'s `run_conversation_loop` needs
a live AI client, a tmux session and an IPC peer, so it has no unit coverage
either.

**So none of the three restructured functions is covered by the unit suite.** That
is a pre-existing gap this phase neither widens nor closes, and it is why the spec
gives exact target code rather than relying on tests to catch a slip.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do **not** claim any test "guards" or "covers" one of
these sites — here that would be plainly false, and in this project a claim about
what a test would catch is admissible only when demonstrated by mutation. This
phase requires no mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Task 2's staleness comment.** Confirm the four-line "In-memory is
   intentionally NOT updated here" comment survives and still sits with the loop
   it describes — and that `let _ = entry;` is gone rather than carried forward.
2. **Task 3's conditionality.** Confirm that a `store.get(id)` miss still results
   in **no** meta file being written, and say which construct enforces it.
3. **Task 4's placement.** Confirm `spawn_compaction` is outside every closure and
   quote the line immediately above it.

## End-to-end verification

None required. This phase ships no new artifact, no CLI behavior, and no config
surface — it moves existing work out of three critical sections. The gates plus
the three reasoning checks above are the verification.

## Authorizations

- [x] May edit `src/webhook/process.rs` and `src/daemon/stream.rs`.
- [x] May extend the existing `use crate::daemon::session::{…}` list in
      `process.rs` to add `with_sessions` (task order: do this first or the file
      will not compile).
- [ ] **No** import deletions in either file. `UnpoisonExt` stays in `process.rs`;
      `stream.rs` needs nothing.
- [ ] **No** new tests, no test edits.
- [ ] **No** edits to `session.rs`, `ask.rs`, `hook.rs`, `gc.rs`, `helpers.rs`, or
      anything under `daemon/context/`.
- [ ] **No** `#[allow(...)]` anywhere. If clippy objects to a shape, report a
      blocker rather than suppressing.

## Out of scope

- **The two `ask.rs` multi-line stragglers** (`:519`, `:686`) — phase 05c's.
- **The `SessionStore` newtype and the enforcement lint** — phase 05c, which must
  run last because it makes raw `.lock()` stop compiling.
- **Test-module acquisitions** anywhere — 05c rewrites those.
- **Narrowing `process.rs`'s in-memory-staleness behavior.** Task 2 preserves it
  exactly, comment included. Whether alerts *should* update memory is a design
  question, not this phase's.

### ⚠ Do not insert an item between a doc comment and the item it documents

This cost phase 05a two extra runs. A `struct` added "immediately above" a
function landed **between** that function's `///` block and the function itself,
silently transferring a `pub fn`'s documentation onto the struct. It compiled, it
linted clean, and nothing in the gate set could see it.

This phase adds no new items, so the risk is lower — but if you find yourself
inserting anything at item scope, **read the lines directly above the insertion
point first** and confirm you are not splitting a doc comment from its item.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 22:05 (started)

**Executor:** Claude Sonnet 4.5 — implementing 3 restructures in `webhook/process.rs` and `daemon/stream.rs`.
