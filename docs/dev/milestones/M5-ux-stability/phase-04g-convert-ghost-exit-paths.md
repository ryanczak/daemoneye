# Phase 04g: Convert the Ghost Exit Paths — `write_mailbox_on_exit` + `briefing.rs`

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-04f (`context/background.rs` converted) — `done`
**Estimated diff:** ~70 lines
**Tags:** language=rust, kind=refactor, size=s

## Goal

Convert the **4** `sessions.lock()` sites on the ghost-shell exit path —
three in `write_mailbox_on_exit` (`src/daemon/ghost.rs`) and one in
`generate_and_save_briefing` (`src/daemon/briefing.rs`) — to `with_sessions`.

**Finish condition: 3 `with_sessions` calls in `ghost.rs`, 1 in `briefing.rs`,
and `ghost.rs` down to exactly 8 remaining raw acquisitions** (the turn loop,
which is the next phase).

**This phase deliberately does NOT touch the ghost turn loop.** `ghost.rs` has
**11** production sites (plus `briefing.rs`'s 1 = 12 for the group); the 8 in
`GhostManager::start_session*` and
`do_ghost_turn` are split into the following phase because three of them are
individually hard (an `anyhow::bail!` inside the guard, blocking file I/O under
the guard, and a `break` out of the enclosing loop). This phase is the clean
four.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard: a converted
  closure enclosing a call that still uses raw `.lock()` deadlocks silently.
  **This phase's region has no such callees** — see Spec task 5.
- `CLAUDE.md` § "Ghost Shell conventions" — `write_mailbox_on_exit` is what
  writes `~/.daemoneye/agents/<agent>/mailbox/<job_id>.json` when a ghost exits;
  the coordinator reads it via `await_agent_result`. Do not change that contract.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**Use this scan, not `grep -c`.** A plain `grep -c "sessions\.lock()"` **cannot
see** an acquisition that splits `sessions` and `.lock()` across lines, and that
blindness already caused one bounce in this milestone (`bugs/bug-04f-1.md`) and
one phase approved on a false count. Save it as `/tmp/scan_locks.py`:

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
python3 /tmp/scan_locks.py src/daemon/ghost.rs src/daemon/briefing.rs
#   src/daemon/ghost.rs: 11
#   src/daemon/briefing.rs: 1
grep -c "with_sessions(" src/daemon/ghost.rs      # expect 0
grep -c "with_sessions(" src/daemon/briefing.rs   # expect 0
```

These are the **verified** starting values — the architect ran this exact script
against the tree while drafting, rather than deriving them by arithmetic. For
`ghost.rs` the plain `grep -c` happens to agree (11), because none of its
acquisitions are multi-line; that agreement is a coincidence of this file and not
a reason to trust `grep -c` elsewhere.

If either count differs, **stop and report a blocker** — the per-site code below
is stale.

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

Generic over `T`, synchronous `FnOnce`, `&mut HashMap`.

### Imports to extend

```rust
// src/daemon/ghost.rs — find the existing `use crate::daemon::session::…` line
// src/daemon/briefing.rs — same
```

Add `with_sessions` to whatever `crate::daemon::session::{…}` list each file
already has. **Do not remove `UnpoisonExt`** from either file without checking:
after this phase `ghost.rs` still has 8 raw acquisitions using `unwrap_or_log`,
so its import stays. For `briefing.rs`, its only `unwrap_or_log` is the site you
are converting — see task 4.

### Site inventory — 4 sites

| # | File:line | Shape |
|---|---|---|
| 1 | `ghost.rs:32` | scoped block, **`return` from the enclosing fn** inside the guard |
| 2 | `ghost.rs:45` | scoped block, pure read, no early exit |
| 3 | `ghost.rs:85` | scoped block, pure read with a fallback closure over outer locals |
| 4 | `briefing.rs:22` | scoped block, **`log::warn!` + `return`** inside the guard |

## Spec

### 1. `ghost.rs:32` — first read in `write_mailbox_on_exit`

Current — `src/daemon/ghost.rs:31-39`:

```rust
    let (agent_name, ghost_config) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(session_id) else {
            return;
        };
        let gc = entry.ghost_config.clone();
        let agent = gc.as_ref().and_then(|g| g.agent.clone());
        (agent, gc)
    };
```

The `return` exits `write_mailbox_on_exit`, not the block. Moving it inside a
`with_sessions` closure would return from the *closure* — a type error at best.
Have the closure return an `Option` and let the caller exit:

```rust
    let Some((agent_name, ghost_config)) = with_sessions(sessions, |store| {
        let entry = store.get(session_id)?;
        let gc = entry.ghost_config.clone();
        let agent = gc.as_ref().and_then(|g| g.agent.clone());
        Some((agent, gc))
    }) else {
        return;
    };
```

Note the closure returns `Option<(Option<String>, Option<GhostConfig>)>` — the
outer `Option` is "entry present or not", and `agent_name` is *still* an
`Option<String>` inside it. That distinction is load-bearing: the very next
statement is

```rust
    let Some(agent_name) = agent_name else {
        return;
    };
```

which must **stay exactly as it is** and must **not** be merged into the closure.
It handles "entry exists but has no agent", which is a different case from
"entry missing" and both happen to return — collapsing them would work today but
destroys the distinction the code documents. Leave it.

### 2. `ghost.rs:45` — last assistant message

Current — `src/daemon/ghost.rs:44-52`:

```rust
    let last_content = {
        let store = sessions.lock().unwrap_or_log();
        store
            .get(session_id)
            .and_then(|e| e.messages.last())
            .filter(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default()
    };
```

Mechanical — no early exit, and the whole block's value is the local:

```rust
    let last_content = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.messages.last())
            .filter(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default()
    });
```

### 3. `ghost.rs:85` — task description, with a fallback over outer locals

Current — `src/daemon/ghost.rs:84-99`:

```rust
    let task_desc = {
        let store = sessions.lock().unwrap_or_log();
        store
            .get(session_id)
            .and_then(|e| e.ghost_task_message.clone())
            .unwrap_or_else(|| match &parent_job_id {
                Some(pid) => format!(
                    "ghost shell for session {} (depth {}, parent: {})",
                    session_id, spawn_depth, pid
                ),
                None => format!(
                    "ghost shell for session {} (depth {})",
                    session_id, spawn_depth
                ),
            })
    };
```

Mechanical — wrap the block body, keeping the `unwrap_or_else` fallback inside:

```rust
    let task_desc = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.ghost_task_message.clone())
            .unwrap_or_else(|| match &parent_job_id {
                Some(pid) => format!(
                    "ghost shell for session {} (depth {}, parent: {})",
                    session_id, spawn_depth, pid
                ),
                None => format!(
                    "ghost shell for session {} (depth {})",
                    session_id, spawn_depth
                ),
            })
    });
```

`parent_job_id`, `spawn_depth`, and `session_id` are all borrowed by the closure,
which is fine — they are read-only and outlive it. The fallback strings must stay
byte-identical; they end up in the mailbox JSON the coordinator reads.

### 4. `briefing.rs:22` — the only site in that file

Current — `src/daemon/briefing.rs:21-32`:

```rust
    let (messages, model_key) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(session_id) else {
            log::warn!(
                "Briefing: session '{}' not found for agent '{}'",
                session_id,
                agent_name
            );
            return;
        };
        (entry.messages.clone(), entry.active_model.clone())
    };
```

Same shape as task 1, plus a log line on the miss path. **Keep the `log::warn!`
outside the closure** — it is not store work, and putting it inside means logging
while holding the global session lock:

```rust
    let Some((messages, model_key)) = with_sessions(sessions, |store| {
        let entry = store.get(session_id)?;
        Some((entry.messages.clone(), entry.active_model.clone()))
    }) else {
        log::warn!(
            "Briefing: session '{}' not found for agent '{}'",
            session_id,
            agent_name
        );
        return;
    };
```

The warning text must stay byte-identical.

**Then check `briefing.rs`'s `UnpoisonExt` import.** This was the site's only
`unwrap_or_log`. Run this after the edit:

```bash
grep -n "unwrap_or_log" src/daemon/briefing.rs
```

- **If it returns nothing**, `use crate::util::UnpoisonExt;` is now unused and
  `cargo build` will fail on `-D warnings`. Delete that import line.
- **If it returns hits** (the `#[cfg(test)]` module at line 150 or later may use
  it), the import is needed only under `cfg(test)`, so **move** it inside
  `mod tests` rather than deleting it.

Either way, verify with **both** `cargo build` *and*
`cargo clippy --all-targets --all-features -- -D warnings`. They disagree about
whether a test-only import counts as used, and that exact disagreement caused a
`hard_fail` in the previous phase — do not skip the second command.

### 5. Deliberately no collapse, and no cross-region widening

Sites 1, 2, and 3 all read the same entry within ~60 lines. **Do not collapse
them.** Site 1's result gates an early return, and site 3's fallback depends on
`spawn_depth` / `parent_job_id`, which are derived from site 1's `ghost_config`
*after* site 2 runs. Collapsing would require hoisting that derivation, i.e.
restructuring the exit path — out of scope, and this is the code that reports a
ghost shell's outcome to its coordinator. **4 sites → 4 `with_sessions` calls.**

Three acquisitions where one would do is a negligible cost: each is a short read,
and the milestone's goal is "no blocking work under the guard", not "fewest
acquisitions".

**No closure in this phase may be widened to enclose a neighbouring call.** For
the record, verified while drafting: `crate::agents::mailbox::write_mailbox`
(`src/agents/mailbox.rs:47`) does **not** touch `SessionStore` — `mailbox.rs` has
zero references to it — and `do_generate_briefing` (`briefing.rs:82`) does not
take `sessions`. So there is no deadlock hazard in this region. Both are
nonetheless **blocking I/O** (file write; AI call) and must stay outside every
closure, exactly where they already are.

## Acceptance criteria

**Two of these are deliberately non-zero.** `ghost.rs` keeps 8 raw acquisitions
for the next phase; a zero there means the turn loop was converted out of scope.

- [ ] `python3 /tmp/scan_locks.py src/daemon/ghost.rs` prints **8**.
- [ ] `python3 /tmp/scan_locks.py src/daemon/briefing.rs` prints **0**.
- [ ] `grep -c "with_sessions(" src/daemon/ghost.rs` returns **3**.
- [ ] `grep -c "with_sessions(" src/daemon/briefing.rs` returns **1**.
- [ ] `grep -c "sessions\.lock()" src/daemon/ghost.rs` returns **8** — consistent
      with the scan, because none of `ghost.rs`'s acquisitions are multi-line.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `python3 /tmp/scan_locks.py src/daemon/context/background.rs src/daemon/executor/mod.rs`
      prints **0** for both (04d–04f untouched).
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **916 means scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text including comments. **Do not write the
literal `sessions.lock()` or `with_sessions(` in a comment** in either file.

## Test plan

Behavior-preserving refactor: the existing **915** tests are the regression net
and must all still pass, unchanged. **Write no new tests.**

`ghost.rs`'s own `#[cfg(test)]` module (line 1049+) and the ghost integration
tests (`tests/integration.rs`: `g1_spawn_ghost_shell_with_agent_merge`,
`g5_mailbox_write_and_read`, `g4_briefing_*`, `g5_child_inherits_depth_and_parent`)
exercise the mailbox and briefing paths. Run them and name them in the Update Log:

```bash
cargo test g5_mailbox_write_and_read
cargo test g4_briefing
```

**Do not claim any of these "guards" a specific line.** State only what you
observed: which tests you ran and that they passed. A claim about *what a test
would catch* is only admissible in this project if you demonstrate it by
mutation, and no mutation is required by this phase.

One reasoning check to state in the Update Log, no new test: confirm that
`write_mailbox_on_exit` still returns early — writing no mailbox entry — in both
distinct miss cases, (a) the session entry is absent, and (b) the entry exists
but its `ghost_config.agent` is `None`. Task 1 explains why these must stay
separate.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key, no
> file the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon,
or a ghost shell. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

- [x] May delete or relocate `use crate::util::UnpoisonExt;` in
      `src/daemon/briefing.rs` **only**, per task 4's conditional. Leave
      `ghost.rs`'s import alone — it still has 8 raw acquisitions.

This phase adds no tests, so it needs no `HOME` redirection and no `unsafe`. If
you think you need `unsafe` or a new dependency, **stop and report a blocker**.

## Out of scope

- **Do not convert the 8 remaining sites in `ghost.rs`** — `GhostManager::start_session*`
  (1 site) and `do_ghost_turn` (7 sites). They are the next phase, and three of
  them need individual treatment (an `anyhow::bail!` inside the guard, an
  `append_session_message` file write under the guard, and a `break` that exits
  the enclosing loop and therefore **cannot** live inside a closure). An
  acceptance criterion pins `ghost.rs` at 8 so an over-eager conversion is caught.
- **Do not collapse sites 1–3.** Task 5 explains why.
- **Do not change `SessionStore` into a newtype** and do not touch the 13
  `Arc::clone` sites.
- **Do not touch `context/background.rs`, `executor/`, `background/`,
  `stream.rs`, `hook.rs`, or `webhook/process.rs`.** Separate phases; two are
  pinned by criteria.
- **Do not reword any string** — the two mailbox fallback descriptions and the
  briefing warning are all byte-identical requirements. The mailbox strings reach
  a coordinator ghost through `await_agent_result`.
- **Do not move `write_mailbox` or `do_generate_briefing` inside a closure.**
  Both are blocking I/O.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to a `let … else`
  shape, report a blocker rather than suppressing.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 16:29 (started)

**Executor:** Claude (sonnet-4-5-20250514)

Converting 4 `sessions.lock()` sites on ghost-shell exit path to `with_sessions`:
3 in `write_mailbox_on_exit` (ghost.rs) + 1 in `generate_and_save_briefing` (briefing.rs).
