# Phase 06j: tmux Calls Off the Runtime — the `daemon/` Tail

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06h — `done`
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to the **6** remaining convertible tmux calls under
`src/daemon/`, finishing the daemon's async surface:

| File | Sites | Enclosing fn |
|---|---|---|
| `background/gc.rs` | 2 | `notify_job_completion` (`:22`, async) |
| `ghost.rs` | 1 | `trigger_ghost_turn` region (async) |
| `hook.rs` | 1 | `handle_notify_session_closed` (`:78`, async) |
| `server/ask.rs` | 2 | `handle_ask` (`:38`, async) |

**Finish condition: the per-file scan reports `gc.rs: 3`, `ghost.rs: 1`,
`hook.rs: 0`, `ask.rs: 0` — and every remaining hit is on the do-not-convert
list below.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs:29` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime" src/daemon/background/gc.rs   # expect 0
grep -c "off_runtime" src/daemon/ghost.rs           # expect 0
grep -c "off_runtime" src/daemon/hook.rs            # expect 0
grep -c "off_runtime" src/daemon/server/ask.rs      # expect 0
cargo test 2>&1 | grep "^test result" | head -3     # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### 🛑 Do not convert — 4 hits, three different reasons

| Hit | Reason |
|---|---|
| `gc.rs:174`, `:273`, `:290` | all inside `pub fn gc_bg_windows` (`:170`) — **synchronous**. `off_runtime` is `async`; converting is `E0728`. A restructure, deferred. |
| `ghost.rs:18` | `use crate::tmux::ensure_incident_session;` — **an import, not a call.** The scan flags it because the regex matches `tmux::`; that is expected. |

**Two more hits elsewhere are deliberately out of this phase**, so do not go
looking for them: `server/handlers.rs:186` sits inside a synchronous
`.filter(|(id, _)| …)` closure in an iterator chain — `.await` is illegal
there and converting it means rewriting the chain into a loop. That is a
restructure and belongs with the other deferred work.

### ⭐ Worked examples — every shape you need is already in the tree

```rust
// bool gate, negated — a timeout must not read as "yes" — foreground.rs:1038
let pid = pane_id.to_string();
let pane_alive = tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&pid))
    .await
    .unwrap_or(false);
if !pane_alive { /* refuse */ }

// Result whose Err is USED — collapse Option<Result<T>> to Result<T> — scheduled.rs:216
let created = tmux::off_runtime("create-job-window", move || tmux::create_job_window(&s, &t))
    .await
    .unwrap_or_else(|| Err(anyhow::anyhow!("timed out creating window")));
if let Err(e) = created { log::warn!("…: {}", e); }

// inline Command with a 3-arm match, plus a None arm — daemon/mod.rs:458
let n = name.clone();
let created = crate::tmux::off_runtime("new-session", move || {
    std::process::Command::new("tmux")
        .args(["new-session", "-d", "-s", &n])
        .output()
})
.await;
match created {
    Some(Ok(o)) if o.status.success() => { … }
    Some(Ok(o)) => { … }
    Some(Err(e)) => { … }
    None => { … }
}
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**.

### This phase's 6 sites

Line numbers are current-as-of-drafting; re-derive with the Acceptance-criteria
script.

| File:line | Call | Returns | Collapse |
|---|---|---|---|
| `gc.rs:53` | `tmux::pane::capture_pane_to_file` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| `gc.rs:79` | `kill_job_window` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` |
| `ghost.rs:400` | `session_exists` | `bool` | `.unwrap_or(false)` — Hazard 2 |
| `hook.rs:115` | inline `Command` — `new-session` | 3-arm `match` | add a `None` arm — Hazard 3 |
| `ask.rs:246` | `pane_exists` | `bool` | `.unwrap_or(false)` — Hazard 4 |
| `ask.rs:247` | `start_pipe_pane` | `Result<PathBuf>` | keep the `match` — Hazard 4 |

Note `gc.rs:53` is called as `tmux::pane::capture_pane_to_file(…)` — through
the `pane` submodule. **The scan prints its module segment (`pane`), not the
function name.** That is a quirk of the regex, not a different site.

### ⚠ Hazard 1 — `gc.rs:53` is an `else if let`, and the short-circuit matters

```rust
if let Err(e) = std::fs::create_dir_all(&logs_dir) {
    log::error!("Failed to create pane_logs directory: {}", e);
} else if let Err(e) =
    tmux::pane::capture_pane_to_file(&pane_id, &logs_dir.join(format!("{}.log", win_name)))
{
    log::error!("Failed to archive pane logs for {}: {}", win_name, e);
}
```

The capture **only runs when the directory was created**. An awaited value
cannot stay in an `else if let`, so convert the `else if` into an `else` block
with the await inside it — **not** by hoisting the capture above the `if`,
which would archive into a directory that does not exist:

```rust
if let Err(e) = std::fs::create_dir_all(&logs_dir) {
    log::error!("Failed to create pane_logs directory: {}", e);
} else {
    let p = pane_id.clone();
    let out = logs_dir.join(format!("{}.log", win_name));
    let archived = tmux::off_runtime("capture-pane-to-file", move || {
        tmux::pane::capture_pane_to_file(&p, &out)
    })
    .await
    .unwrap_or_else(|| Err(anyhow::anyhow!("timed out archiving pane logs")));
    if let Err(e) = archived {
        log::error!("Failed to archive pane logs for {}: {}", win_name, e);
    }
}
```

Both `log::error!` messages stay exactly as they are.

### ⚠ Hazard 2 — `ghost.rs:400` is a negated bool gate that bails

```rust
if !tmux::session_exists(&tmux_session) {
    anyhow::bail!(
        "Ghost Shell {}: tmux session '{}' no longer exists", session_id, tmux_session
    );
}
```

`.unwrap_or(false)` → `!false` is `true` → the ghost turn **aborts with the
existing error**. That is the fail-safe direction: a ghost shell that cannot
confirm its session must not run autonomous remediation against it.
`.unwrap_or(true)` would let it proceed against a session it never verified.
**Write `.unwrap_or(false)`**, and leave the `bail!` string byte-identical.

### ⚠ Hazard 3 — `hook.rs:115` looks like `mod.rs:458` but must NOT bail

The shape is the same three-arm `new-session` match, and `mod.rs:458` (just
converted) is the worked example for the **structure**. The `None` arm is
different:

- **`mod.rs`'s** failure arms `anyhow::bail!` — a daemon with no session cannot
  start.
- **`hook.rs`'s** failure arms `log::warn!` and **continue**;
  `handle_notify_session_closed` goes on to
  `send_response_split(tx, Response::Ok).await?`.

`handle_notify_session_closed` returns `Result<()>`, so a `bail!` **would
compile** — and would change behaviour, turning a logged warning into a failed
hook response. **The `None` arm must `log::warn!` and fall through**, matching
its two neighbours:

```rust
None => {
    log::warn!(
        "tmux new-session for managed session '{}' timed out",
        session_name
    );
}
```

All three existing arms stay byte-identical, including the
`*bg_session.lock().unwrap_or_log() = session_name.clone();` and
`cache.set_session(&session_name);` pair in the success arm, and including the
guard `if o.status.success()`.

### ⚠ Hazard 4 — `ask.rs:246`/`:247` are nested, and sit in a deliberately unlocked region

```rust
// Unlocked phase: the two blocking tmux calls, then a short write-back.
if let (Some(id), Some(ref pane_id)) = (session_id.as_deref(), pipe_candidate.as_deref()) {
    let resolved = if crate::tmux::pane_exists(pane_id) {
        match crate::tmux::start_pipe_pane(pane_id) {
            Ok(_) => pane_id.to_string(),
            Err(e) => {
                log::debug!("R1: could not start pipe-pane for {}: {}", pane_id, e);
                String::new() // don't retry
            }
        }
    } else {
        log::debug!("R1: skipping pipe-pane for {} — pane no longer exists", pane_id);
        String::new() // don't retry
    };
    with_sessions(sessions, |store| { … entry.pipe_source_pane = Some(resolved); });
}
```

Three things to preserve:

1. **`start_pipe_pane` only runs when `pane_exists` was true.** Convert
   `pane_exists` to a `let` binding above the `if`, then keep the `if`
   structure — do not merge the two calls.
2. **Both `String::new()` "don't retry" paths and both `log::debug!` messages
   stay exactly as they are.** The `Err` arm's comment about the TOCTOU race is
   institutional memory; keep it.
3. **Both `.await`s stay OUTSIDE the `with_sessions` closure.** This whole block
   exists *because* these two calls were hoisted out of that closure earlier in
   the milestone — the comment "Unlocked phase: the two blocking tmux calls"
   says so. Putting an `.await` back inside would not compile and would
   re-create a fixed defect.

`start_pipe_pane` returns `Result<PathBuf>` and its `Err` is used, so it takes
the `.unwrap_or_else(|| Err(anyhow::anyhow!(…)))` collapse; the timeout then
flows into the existing `Err(e)` arm and yields the same `String::new()`
"don't retry" outcome.

## Spec

### 1. Convert the 6 sites

Match each to its collapse from the table above. **Preserve every existing
match arm, guard, log message, comment and failure default exactly.**

### 2. Convert nothing on the do-not-convert list

`gc.rs:174`, `:273`, `:290` and `ghost.rs:18`. Four hits, all deliberate.

### 3. Build after every site

Not a suggestion. `cargo build` after each converted site.

## Acceptance criteria

- [ ] **Per-file scan reports `3 / 1 / 0 / 0`:**

```bash
python3 - <<'PY'
import re, pathlib
for f in ["src/daemon/background/gc.rs", "src/daemon/ghost.rs",
          "src/daemon/hook.rs", "src/daemon/server/ask.rs"]:
    src = pathlib.Path(f).read_text()
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
    PURE = {"off_runtime", "TMUX_TIMEOUT", "cache"}
    bad = [(src[:m.start()].count("\n")+1, m.group(1))
           for m in re.finditer(r'\btmux::(\w+)', src)
           if m.group(1) not in PURE and not inside(m.start())]
    bad += [(src[:m.start()].count("\n")+1, 'Command::new("tmux")')
            for m in re.finditer(r'Command::new\("tmux"\)', src) if not inside(m.start())]
    print(f"{f}: {len(bad)}")
    for l, n in sorted(bad): print(f"      {l}: {n}")
PY
#   gc.rs:    3  -> pane, kill_job_window, kill_job_window  (all in gc_bg_windows)
#   ghost.rs: 1  -> ensure_incident_session  (the `use` import at ~:18)
#   hook.rs:  0
#   ask.rs:   0
```

      **Read the names.** `gc.rs` must keep exactly its three
      `gc_bg_windows` hits, and `ghost.rs` only its import.

- [ ] `grep -c "off_runtime"` returns **≥ 2** in `gc.rs`, **≥ 1** in
      `ghost.rs`, **≥ 1** in `hook.rs`, **≥ 2** in `server/ask.rs`. Each printed
      **0** before this phase.
- [ ] `grep -cF 'anyhow::bail!' src/daemon/hook.rs` returns **0** — the `None`
      arm logs, it does not bail.
- [ ] `grep -cF 'String::new() // don'"'"'t retry' src/daemon/server/ask.rs`
      returns **2** — both no-retry paths survive.
- [ ] The `with_sessions(sessions, |store| …)` closure in `handle_ask` near
      `:263` contains **no `.await`**. Verify by reading and quote it.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking"` returns **0** in
      all four files.
- [ ] `git diff --name-only` lists exactly **four** `src/` files.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

All four enclosing functions need a live tmux server, and `handle_ask`
additionally needs an IPC peer and an AI client. **None of the six sites has
unit coverage.** Pre-existing gap, neither widened nor closed here. `gc.rs`'s
`mod tests` (`:311`) covers only `plan_gc_actions`, a pure planner this phase
does not touch.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The `else if` short-circuit.** Paste the converted `gc.rs:53` and state, in
   one sentence, whether the pane can be archived when `create_dir_all` failed.
2. **`hook.rs`'s `None` arm.** Paste it and say why it logs rather than bails,
   naming what the function does after the match.
3. **The lock boundary.** Quote the `with_sessions` closure in `handle_ask` and
   confirm both new `.await`s are outside it.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/background/gc.rs`, `src/daemon/ghost.rs`,
      `src/daemon/hook.rs`, `src/daemon/server/ask.rs` — **the six named sites
      and whatever their expressions require.**
- [x] May add owned bindings and `.clone()` calls at call sites.
- [x] May convert `gc.rs`'s `else if let` into an `else` block.
- [x] May add a `None` arm to the `hook.rs` match.
- [ ] **No** change to any function's signature.
- [ ] **No** change to any existing match arm, guard, log message, or comment.
- [ ] **No** `anyhow::bail!` added to `hook.rs`.
- [ ] **No** `.await` inside any `with_sessions` closure.
- [ ] **No** conversion of `gc_bg_windows`' three hits or `ghost.rs`'s import.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the four named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`gc_bg_windows` (3), `helpers.rs` (4), `session.rs::cleanup_bg_windows`
  (2), `webhook/process.rs` (1), `utils/host.rs` (1)** — all in synchronous
  functions; the restructure phase owns them.
- **`server/handlers.rs:186`** — inside a synchronous `.filter()` closure;
  needs the iterator chain rewritten into a loop. Restructure.
- **`cli/`** — surveyed: `commands/ask.rs` (2) and `commands/chat.rs` (10) are
  async and convertible; `commands/pane.rs` (6) and `local_cmds.rs` (1) are
  sync. Its own phase.
- **`src/tmux/{pane,session,window,cache}.rs`** — those `Command::new("tmux")`
  calls **are** the sync helper bodies, not call sites. Stage A hardens them.
  **Do not touch them.**

### ⚠ Traps

1. **`hook.rs`'s `None` arm logs; it does not bail** — unlike the otherwise
   identical match in `daemon/mod.rs`. A `bail!` there compiles and is wrong.
2. **Do not hoist `gc.rs`'s capture above the `create_dir_all` check.**
3. **`.unwrap_or(false)` under a `!`** at `ghost.rs:400` and `ask.rs:246`.
4. **Keep `start_pipe_pane` nested inside the `pane_exists` branch.**
5. **Both `ask.rs` awaits stay outside the `with_sessions` closure.**
6. **`gc.rs:53` is `tmux::pane::capture_pane_to_file`** — the scan prints the
   module segment `pane`, not the function name.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 22:54 (started)

**Executor:** Claude (Sonnet 4.5)

Converting the remaining 6 tmux calls under `src/daemon/` to `off_runtime`: `gc.rs` (2 sites in `notify_job_completion`), `ghost.rs` (1 site in `trigger_ghost_turn`), `hook.rs` (1 site in `handle_notify_session_closed`), `ask.rs` (2 sites in `handle_ask`).
