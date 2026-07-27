# Phase 05e: Get `watch_pane`'s Completion Callback Out of the Session Lock

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-05d (the newtype) — `done`
**Estimated diff:** ~40 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`executor/knowledge/pane.rs:329` performs **two file writes and a tmux subprocess
spawn while holding the global session lock**. Restructure it into the
collect-under-the-lock / act-outside-it shape.

This is mechanism A + B — the same defect 05a and 05b removed from five other
sites. It survived because it was **invisible to every scan in this milestone**;
05d's newtype is what exposed it.

**Finish condition: `pane.rs` has 4 `with_sessions` calls, and the closure-audit
script in the Pre-flight reports `pane.rs` clean.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism A (lock held across blocking work)
  and mechanism B (blocking subprocess spawns on tokio workers). This site is
  both.
- `CLAUDE.md` § "Important Invariants" — `with_sessions` satisfies the
  `.unwrap_or_log()` invariant internally.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**A new instrument.** Counting `.lock()` is obsolete — 05d's newtype made raw
acquisition a compile error, so the only question left is *what runs inside a
`with_sessions` closure*. Save this as `/tmp/audit_closures.py`:

```python
import pathlib, re

BLOCKING = [
    ("append_session_message", "file write"),
    ("write_session_file", "file write"),
    ("write_session_meta", "file write"),
    ("log_event(", "file append"),
    ("std::process::Command", "subprocess"),
    ("tmux::", "subprocess"),
    ("related_knowledge_hints", "fs scan"),
    ("read_session_meta", "file read"),
]

for f in sorted(pathlib.Path("src").rglob("*.rs")):
    src = f.read_text()
    for m in re.finditer(r'with_sessions\s*\(', src):
        i = m.end() - 1
        depth = 0
        start = i
        while i < len(src):
            if src[i] == '(':
                depth += 1
            elif src[i] == ')':
                depth -= 1
                if depth == 0:
                    break
            i += 1
        body = src[start:i + 1]
        line = src[:m.start()].count("\n") + 1
        hits = [(p, w) for p, w in BLOCKING if p in body]
        if hits:
            print(f"{f}:{line}  ->  {', '.join(f'{p} ({w})' for p, w in hits)}")
```

Then:

```bash
python3 /tmp/audit_closures.py
#   src/daemon/executor/knowledge/pane.rs:329  ->  append_session_message (file write), std::process::Command (subprocess)
#   src/daemon/server/ask.rs:97  ->  tmux:: (subprocess), read_session_meta (file read)
grep -c "with_sessions(" src/daemon/executor/knowledge/pane.rs   # expect 3
```

**`ask.rs:97` is phase 05f's, not yours.** It must still be reported when you
finish. A clean report for `ask.rs` means you went out of scope.

**Verified against the tree while drafting.** If the `pane.rs` line differs,
**stop and report a blocker.**

## Current state

### The site — `src/daemon/executor/knowledge/pane.rs:329`

Inside `watch_pane`'s completion callback, which runs on a spawned thread when a
watched pane finishes or times out:

```rust
        with_sessions(&sessions_clone, |store| {
            if let Some(entry) = store.get_mut(&session_id_owned) {
                append_session_message(&session_id_owned, &watch_msg);   // TWO file writes
                entry.messages.push(watch_msg);

                let alert = if completed {
                    format!("Watched pane {} command completed", pane_id_owned)
                } else {
                    format!("Watched pane {} timed out", pane_id_owned)
                };
                if let Some(ref cp) = entry.chat_pane {
                    let _ = std::process::Command::new("tmux")           // subprocess spawn
                        .args(["display-message", "-d", "5000", "-t", cp, &alert])
                        .output();
                }
            }
        });
```

Everything blocking runs while the global lock is held, stalling every other
session's IPC handler.

### ⭐ The worked example — `notify_session`, same shape, same file family

`src/daemon/background/helpers.rs` (landed by 05a). It carries the exact
four-phase structure this task needs, including the two subtleties:

```rust
    // Phase 1 (locked): update the registry and take what the rest needs.
    // Returns None when the session entry is gone.
    let Some(chat_pane) = with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;
        …
        Some(entry.chat_pane.clone())
    }) else {
        return;
    };

    // Phase 2 (unlocked): the file write.
    append_session_message(session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.messages.push(completion_msg);
        }
    });

    // Phase 4 (unlocked): the tmux notification.
    if let Some(ref cp) = chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
```

**Two properties to copy:**

1. **`chat_pane` is cloned** in phase 1. Borrowing `entry.chat_pane` is what
   would pin the guard open across the rest.
2. **Phase 3 re-checks `get_mut`.** The entry can legitimately vanish while the
   file write runs, and an unconditional push would panic or resurrect state.

### Receiver form

`sessions_clone` is an owned `SessionStore` (cloned before the thread spawn), so
every call is `with_sessions(&sessions_clone, …)` — **with** the `&`, matching
the existing call at `:329`.

### Imports need no change

`pane.rs:3` already imports `append_session_message` and `with_sessions`:

```rust
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe, with_sessions,
```

**`UnpoisonExt` stays.** `pane.rs` has 4 `unwrap_or_log` calls (`:76`, `:77`,
`:420`, `:434`) and **none of them is this phase's** — they are on
`cache.panes` / `cache.session_name`, which are `RwLock`s, not the session store.
`grep -c "UnpoisonExt"` stays at **2** and `grep -c "unwrap_or_log"` stays at
**4**.

## Spec

### 1. Restructure the callback into four phases

Replace the block quoted above with:

```rust
        // Phase 1 (locked): confirm the entry exists and take what the rest needs.
        let Some(chat_pane) = with_sessions(&sessions_clone, |store| {
            store
                .get_mut(&session_id_owned)
                .map(|entry| entry.chat_pane.clone())
        }) else {
            log::info!(
                "watch_pane {}: {}",
                pane_id_owned,
                if completed { "completed" } else { "timed out" }
            );
            return;
        };

        // Phase 2 (unlocked): the file write.
        append_session_message(&session_id_owned, &watch_msg);

        // Phase 3 (locked): push the message into the in-memory history.
        with_sessions(&sessions_clone, |store| {
            if let Some(entry) = store.get_mut(&session_id_owned) {
                entry.messages.push(watch_msg);
            }
        });

        // Phase 4 (unlocked): the tmux notification.
        let alert = if completed {
            format!("Watched pane {} command completed", pane_id_owned)
        } else {
            format!("Watched pane {} timed out", pane_id_owned)
        };
        if let Some(ref cp) = chat_pane {
            let _ = std::process::Command::new("tmux")
                .args(["display-message", "-d", "5000", "-t", cp, &alert])
                .output();
        }
```

**Four things this must preserve:**

1. **The whole block is still conditional on the entry existing.** Today nothing
   happens when `get_mut` misses — no file write, no tmux message. Phase 1's
   `else` branch must keep that true. An unconditional hoist would append to the
   JSONL of a session that no longer exists, and **no test would catch it.**
2. **`append_session_message` still precedes the in-memory push**, exactly as it
   does today and as `notify_session` does.
3. **Phase 3 re-checks `get_mut`** rather than assuming phase 1's hit still holds.
4. **The trailing `log::info!` still runs on every path.** It currently sits
   *after* the block and fires whether or not the entry was found — hence the
   duplicate in phase 1's `else` branch above. Do not drop it from either path,
   and do not move the surviving one before phase 4.

### 2. Keep exactly one `log::info!` per invocation

The callback currently ends with a single `log::info!("watch_pane {}: {}", …)`
(`pane.rs:347`) that fires on **every** path — entry found or not. The shape in
task 1 preserves that by duplicating it into phase 1's early-return, giving
**two** occurrences in the file that are mutually exclusive at runtime.

**Either arrangement is acceptable**, and a shape with a single occurrence is
preferable if you find one that also preserves the early-return semantics:

- `grep -c 'watch_pane {}: {}'` returning **2** → the duplicated form; verify by
  reading that exactly one branch can run.
- returning **1** → a single-exit form; verify by reading that it still fires when
  the entry is missing.

**The requirement is behavioral — one log line per invocation, on every path —
not a specific count.** Say which arrangement you chose in the Update Log and
show it.

## Acceptance criteria

- [ ] `python3 /tmp/audit_closures.py` no longer reports
      `src/daemon/executor/knowledge/pane.rs`.
- [ ] `python3 /tmp/audit_closures.py` **still reports `src/daemon/server/ask.rs:97`**
      — that is phase 05f's site. A clean report there means you went out of scope.
- [ ] `grep -c "with_sessions(" src/daemon/executor/knowledge/pane.rs` returns
      **4** (3 pre-existing, with the third split into two).
- [ ] `grep -c "append_session_message" src/daemon/executor/knowledge/pane.rs`
      returns **2** — the import on line 3 and the single call, now unlocked.
- [ ] `grep -c "display-message" src/daemon/executor/knowledge/pane.rs` returns
      **1** — unchanged; the tmux notification is moved, not duplicated.
- [ ] `grep -c 'watch_pane {}: {}' src/daemon/executor/knowledge/pane.rs` returns
      **1 or 2** — see task 2. Whichever it is, **exactly one log line must fire
      per invocation on every path**; the count alone cannot prove that, so verify
      by reading and say which arrangement you chose.
- [ ] `grep -c "UnpoisonExt" src/daemon/executor/knowledge/pane.rs` returns **2**
      and `grep -c "unwrap_or_log" …` returns **4** — both unchanged. Those are
      `cache.panes` / `cache.session_name` `RwLock`s and are **not** this phase's.
- [ ] `git diff --stat` shows **exactly one** `src/` file changed.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **any other number means scope crept.**

**Run every gate bare.** `cargo clippy … | tail -20` exits with `tail`'s status,
so a failing gate reads as passing — that is how a real error went unnoticed
earlier in this milestone.

## Test plan

Behavior-preserving restructure: what lands in the session JSONL, what the AI
sees in history, and what the user sees in tmux are all unchanged. Only *when the
lock is held* changes. The existing **915** tests are the regression net.
**Write no new tests.**

`pane.rs` has a `#[cfg(test)]` module at `:370`, but it covers `close_bg_window`
and cache helpers. **`watch_pane` has no test coverage at all** — it needs a live
tmux pane, a spawned thread and a broadcast channel. That is a pre-existing gap
this phase neither widens nor closes, and it is why the spec gives exact target
code rather than relying on tests to catch a slip.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test "guards" this site — that would
be false, and in this project a claim about what a test would catch is admissible
only when demonstrated by mutation. This phase requires no mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Conditionality.** Confirm that a `get_mut` miss still produces **no** file
   write and **no** tmux message, and name the construct that enforces it.
2. **Ordering.** Confirm `append_session_message` still runs before the in-memory
   push.
3. **Log volume.** Confirm exactly one `log::info!` fires per invocation, on both
   the entry-found and entry-missing paths.

## End-to-end verification

None required. This phase ships no new artifact, no CLI behavior, and no config
surface. The gates, the closure audit, and the three reasoning checks are the
verification.

## Authorizations

- [x] May edit `src/daemon/executor/knowledge/pane.rs`.
- [ ] **No** import additions or deletions — everything needed is already in
      scope, and `UnpoisonExt` is still used by four unrelated `RwLock` sites.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** edits to `src/daemon/server/ask.rs` — that is phase 05f.
- [ ] **No** edits to `pane.rs`'s other two `with_sessions` sites (`:17`, `:54`).
      The audit reports them clean; leave them alone.
- [ ] **No** `#[allow(...)]` anywhere. If clippy objects to a shape, report a
      blocker rather than suppressing.

## Out of scope

- **`ask.rs:97`** — the other site the audit finds: `read_session_meta` (a file
  read) plus `tmux::pane_exists` and `tmux::start_pipe_pane` (two subprocesses),
  all inside one `with_sessions` closure. It is a **larger and more delicate**
  restructure in the daemon's busiest handler and gets its own phase, **05f**.
- **04f's coverage follow-up** — the three vacuous `compaction_in_flight`
  assertions. That is **05g**.
- **Adding `watch_pane` test coverage.** The gap is real and pre-existing; closing
  it needs a tmux fixture and is not this phase's job.

### ⚠ Two traps from earlier phases in this milestone

1. **Do not hoist blocking work unconditionally.** The 04h/05a precedent: the
   write must stay gated on the entry existing. This is the phase's one
   silent-failure risk — every gate stays green if you get it wrong.
2. **Do not insert an item between a doc comment and the item it documents.**
   Phase 05a cost two extra runs when a `struct` added "immediately above" a
   function landed between that function's `///` block and the function. This
   phase adds no items, but if you insert anything at item scope, read the lines
   directly above the insertion point first.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
