# Phase 06b: Get `respawn.rs`'s tmux Calls Off the Async Runtime

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06a (the `off_runtime` adapter) — `done`
**Estimated diff:** ~110 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply the `tmux::off_runtime` adapter — landed by 06a — to the **11** tmux
subprocess calls in `src/daemon/background/respawn.rs`.

The whole file is one `pub async fn respawn_background_in_pane` plus nested
`tokio::spawn(async move { … })` blocks, so **every** tmux call in it runs on a
runtime worker and blocks it until tmux answers.

**Finish condition: 0 unwrapped tmux subprocess calls in `respawn.rs`, verified
with the span-matching script in Acceptance criteria.**

**The adapter already exists.** This phase adds no new machinery — it applies an
established pattern. 06c–06e do the same for `executor/`, the `daemon/` core and
`cli/`.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `src/tmux/mod.rs` — the `off_runtime` adapter and `TMUX_TIMEOUT`, both landed
  by 06a. Read the doc comment; it explains the `Option<T>` return.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "tmux::" src/daemon/background/respawn.rs                 # expect 10
grep -c 'Command::new("tmux")' src/daemon/background/respawn.rs   # expect 3
grep -c "off_runtime" src/tmux/mod.rs                             # expect 1
grep -c "off_runtime" src/daemon/background/run.rs                # expect 23
cargo test 2>&1 | grep "^test result" | head -2                   # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

Note the arithmetic: 10 + 3 = 13 hits, but only **11** are sites. See below.

## Current state

### ⭐ The worked example is now in-tree — `background/run.rs`

06a converted 16 sites in that file. **Read it and copy the shapes**; every form
you need already exists there. Two representative extracts:

```rust
// value used, both failure modes collapse to a default
let p2 = pane_id.clone();
let shell_name = tmux::off_runtime("pane-current-command", move || {
    tmux::pane_current_command(&p2)
})
.await
.and_then(|r| r.ok())
.unwrap_or_default();

// error inspected; the timeout arm is a no-op because off_runtime already logged it
let (s_gc, wn_gc) = (session.to_string(), win_name.clone());
match tmux::off_runtime("kill-job-window", move || {
    tmux::kill_job_window(&s_gc, &wn_gc)
})
.await
{
    Some(Err(e)) => log::error!("Failed to GC dead bg window {}: {}", win_name, e),
    None => {} // already logged by off_runtime
    Some(Ok(_)) => {}
}
```

**The `Option<Result<…>>` shape is the point.** `None` is *"we do not know"*
(timeout or panic, already logged); `Some(Err(e))` is *"tmux said no"*. Do not
collapse them — that would hide a wedged server as an ordinary failure.

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes owned
before the closure**. That is the per-site work.

### ⚠ Two `tmux::` hits that are NOT sites — do not wrap them

`grep -c "tmux::"` returns **10**, but only **8** are subprocess calls:

- **`respawn.rs:23`** is a **doc comment**: `/// … (caller verifies via
  \`tmux::pane_exists\`)`. Prose, not code.
- **`respawn.rs:85`** is **`tmux::pipe_log_path(pane_id)`**, which is a **pure
  path builder** — no subprocess at all:

  ```rust
  // src/tmux/pane.rs:244
  pub fn pipe_log_path(pane_id: &str) -> std::path::PathBuf {
      let safe = pane_id.trim_start_matches('%');
      crate::config::pipe_log_dir().join(format!("de-pipe-{}.log", safe))
  }
  ```

  **Wrapping it would be wrong** — it spawns nothing, so `off_runtime` would add
  a thread hop and a spurious timeout log for a string concatenation.

(The `std::fs::remove_file` on that same line *is* blocking I/O, but it is not a
tmux call and is out of scope — see Out of scope.)

### The 11 sites

| Line | Call | Shape |
|---|---|---|
| 41 | inline `Command` — `respawn-pane`, `.status()` | **E** — result used, early return |
| 57 | `pane_current_command` | B — `.unwrap_or_default()` |
| 86 | `start_pipe_pane` | B — `.map_err(…).ok()` |
| 90 | `send_keys` | C — **early return** on failure |
| 92 | `stop_pipe_pane` | A — ignored |
| 132 | `pane_dead_status` | B — needs `.flatten()` |
| 145 | inline `Command` — `pipe-pane` | D — ignored |
| 188 | `kill_job_window` | C |
| 239 | `pane_dead_status` | B — needs `.flatten()` |
| 249 | inline `Command` — `pipe-pane` | D — ignored |
| 285 | `kill_job_window` | C |

Line numbers shift as you edit; re-derive with the Acceptance-criteria script
rather than working from this table.

## Spec

### 1. Convert the eight `tmux::` calls

Shapes A–D are exactly as `run.rs` does them. Two need specific care:

**`pane_dead_status` (132, 239) needs `.flatten()`.** It returns `Option<i32>`, so
`off_runtime` yields `Option<Option<i32>>`:

```rust
let p_dead = pane_id_str.clone();
let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p_dead))
    .await
    .flatten()
    .unwrap_or(-1);
```

Both "timed out" and "tmux reported no status" become `-1`, which is what the
current `.unwrap_or(-1)` already means.

**`send_keys` (90) returns early** — and must also return on timeout:

```rust
let p = pane_id.to_string();
let w = wrapped.clone();
match tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &w)).await {
    Some(Err(e)) => {
        if pipe_log.is_some() { /* stop_pipe_pane, via off_runtime */ }
        return format!("Error: failed to send retry command to pane {}: {}", pane_id, e);
    }
    None => {
        if pipe_log.is_some() { /* stop_pipe_pane, via off_runtime */ }
        return format!(
            "Error: failed to send retry command to pane {}: tmux timed out \
             (server may be wedged)",
            pane_id
        );
    }
    Some(Ok(_)) => {}
}
```

The cleanup in the `None` arm is **not optional** — leaving pipe-pane running
after a failed send leaks a log writer, exactly as the `Err` arm already avoids.

### 2. Convert the three inline `Command::new("tmux")` sites

**145 and 249 are Shape D** — identical `pipe-pane` stops, result ignored. Copy
`run.rs`'s treatment verbatim.

**41 is Shape E and is new.** It uses `.status()`, not `.output()`, and its
failure path returns:

```rust
// before
let respawn_status = std::process::Command::new("tmux")
    .args(["respawn-pane", "-k", "-t", pane_id])
    .status();
if !respawn_status.map(|s| s.success()).unwrap_or(false) {
    return format!("Error: failed to respawn pane {} (pane may no longer exist)", pane_id);
}

// after
let p = pane_id.to_string();
let respawn_ok = tmux::off_runtime("respawn-pane", move || {
    std::process::Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", &p])
        .status()
})
.await
.and_then(|r| r.ok())
.map(|s| s.success())
.unwrap_or(false);
if !respawn_ok {
    return format!("Error: failed to respawn pane {} (pane may no longer exist)", pane_id);
}
```

**A timeout must be a failure here**, not a success — the pane was not respawned,
so proceeding would send a command into an unknown shell. `.unwrap_or(false)`
gives that, and the existing message stays accurate for both causes. **Do not add
a separate timeout message**; `off_runtime` already logged the cause with the
operation name.

### 3. Change nothing else

`respawn.rs:85`'s `std::fs::remove_file` and `tmux::pipe_log_path` stay exactly
as they are.

## Acceptance criteria

- [ ] **Span-matching check reports 0 unwrapped sites.** Use this, not a
      line-oriented grep — rustfmt puts closure bodies on their own line, and a
      `move ||` heuristic produces false positives:

```bash
python3 - <<'PY'
import re, pathlib
src = pathlib.Path("src/daemon/background/respawn.rs").read_text()
spans = []
for m in re.finditer(r'off_runtime\s*\(', src):
    i = m.end()-1; d = 0
    while i < len(src):
        if src[i] == '(': d += 1
        elif src[i] == ')':
            d -= 1
            if d == 0: break
        i += 1
    spans.append((m.start(), i))
inside = lambda p: any(a <= p <= b for a, b in spans)
PURE = {"pipe_log_path", "off_runtime", "TMUX_TIMEOUT", "pane_exists"}
bad = [(src[:m.start()].count("\n")+1, m.group(1))
       for m in re.finditer(r'\btmux::(\w+)', src)
       if m.group(1) not in PURE and not inside(m.start())]
bad += [(src[:m.start()].count("\n")+1, 'Command::new("tmux")')
        for m in re.finditer(r'Command::new\("tmux"\)', src) if not inside(m.start())]
print("UNWRAPPED:", len(bad))
for l, n in bad: print(f"  {l}: {n}")
PY
#   UNWRAPPED: 0
```

- [ ] `grep -c "off_runtime" src/daemon/background/respawn.rs` returns **≥ 11** —
      one per site, plus any extra introduced by cleanup in the `send_keys`
      timeout arm. **A number below 11 means a site was missed**; the span check
      above is what proves the exact set.
- [ ] `grep -c "pipe_log_path" src/daemon/background/respawn.rs` returns **1**,
      and it is **not** inside an `off_runtime` closure — it is a pure path
      builder. Verify by reading.
- [ ] `grep -c "spawn_blocking" src/daemon/background/respawn.rs` returns **0** —
      call sites use `off_runtime`; only `src/tmux/mod.rs` names `spawn_blocking`.
- [ ] `git diff --name-only` lists exactly **one** `src/` file:
      `src/daemon/background/respawn.rs`.
- [ ] `grep -c "off_runtime" src/daemon/background/run.rs` returns **23**,
      unchanged — 06a's work is not this phase's to revisit.
- [ ] `grep -c "tmux::" src/daemon/executor/foreground.rs` returns **27**,
      unchanged — phase 06c's, and a lower number means you swept out of scope.
      (27 is the `grep -c` **line** count. A separate survey counted 15 *calls
      reachable from async context* there — different instruments, different
      numbers. This criterion is the line count, measured.)
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests —
      both unchanged. This phase adds no tests.

**Run every gate bare** — piping through `tail` exits with `tail`'s status.

## Test plan

`respawn.rs` respawns a shell in a live tmux pane and has **no unit coverage** —
it needs a real tmux server, a pane, and a pane-death hook. That is a pre-existing
gap this phase neither widens nor closes, and it is why the spec gives exact
target code for the two non-obvious shapes.

**Write no new tests.** The 916 + 27 existing tests are the regression net for
compilation and unrelated behavior; they cannot exercise this file.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test guards these sites — that would
be false, and a coverage claim is admissible in this project only when
demonstrated by mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **The two non-sites.** Confirm `pipe_log_path` (line 85) and the doc comment
   (line 23) were left unwrapped, and say in one sentence why wrapping
   `pipe_log_path` would be wrong.
2. **Early returns.** Name every site that returns on failure and confirm each
   also returns on timeout. State what would go wrong at line 41 if a timeout
   were treated as success.
3. **Cleanup on the timeout path.** Confirm the `send_keys` timeout arm still
   stops pipe-pane when `pipe_log.is_some()`, as its error arm does.

## End-to-end verification

None required beyond the gates. 06a already demonstrated the timeout arm fires
(`TMUX_TIMEOUT` lowered to 1 ms, `None` returned, log line observed); this phase
adds no new machinery, only call sites. **Do not repeat that demonstration** and
do not add a test to make it repeatable.

## Authorizations

- [x] May edit `src/daemon/background/respawn.rs` — the 11 sites.
- [x] May add owned bindings (`let p = pane_id.to_string();`) at call sites —
      required by `spawn_blocking`'s `'static` bound.
- [ ] **No** edits to `src/tmux/mod.rs`. The adapter is complete; if it seems to
      need a change, report a blocker instead.
- [ ] **No** edits to `background/run.rs` or any 06c–06e file.
- [ ] **No** wrapping of `tmux::pipe_log_path` or `std::fs::remove_file`.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`std::fs::remove_file` at line 85** and other non-tmux blocking I/O in async
  context. Real, but a different criterion; this phase is mechanism B for **tmux**
  calls.
- **The remaining ~59 async tmux sites** — `executor/foreground.rs` (15),
  `daemon/mod.rs` (8), `scheduled.rs` (7), `cli/commands/chat.rs` (10) and the
  rest. Phases **06c–06e**.
- **Hardening the sync helpers themselves** (a timeout inside `src/tmux/`). The
  agreed second stage, after all async sites are off the runtime.

### ⚠ Three traps, two of them from this phase family

1. **A `move ||` line-heuristic gives false positives.** 06a's acceptance script
   flagged 7 correctly-wrapped calls because rustfmt puts the closure body on the
   next line. The span-matching script above replaces it — **use it, do not
   re-derive a grep.**
2. **Not every `tmux::` hit is a subprocess.** Two of this file's ten are a doc
   comment and a pure path builder. Wrapping a pure function is a defect, not
   over-caution.
3. **Do not insert an item between a doc comment and the item it documents.** This
   phase adds no items, but if you insert anything at item scope, read the lines
   directly above the insertion point first.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 15:40 (progress)

Converting all 11 tmux subprocess call sites in `respawn.rs` to use `tmux::off_runtime`. Left `tmux::pipe_log_path` (pure path builder) and the doc comment referencing `tmux::pane_exists` unwrapped as required.
