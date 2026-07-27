# Phase 06g: tmux Calls Off the Runtime — `scheduled.rs` + `utils/sudo.rs`

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-06f — `done` (`executor/` is finished)
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to **11** tmux calls in the scheduled-job and sudo
paths:

| File | Sites | Enclosing fn(s) |
|---|---|---|
| `src/daemon/scheduled.rs` | 7 | `run_scheduled_job` (`:27`, async) |
| `src/daemon/utils/sudo.rs` | 4 | 3 async fns (`:47`, `:65`, `:95`) |

**Every site in both files is inside an `async fn`** — unlike 06f, there is no
sync-boundary carve-out here.

**Finish condition: the per-file scan reports `0` for both files.**

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
grep -c "off_runtime" src/daemon/scheduled.rs      # expect 0
grep -c "off_runtime" src/daemon/utils/sudo.rs     # expect 0
cargo test 2>&1 | grep "^test result" | head -3    # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### ⭐ Worked examples — three shapes already in the tree

`src/daemon/executor/foreground.rs` is fully converted (29 sites) and carries
the three shapes you already need:

```rust
// Result<T>, value used, failure collapses to a default — foreground.rs:749
let t = target_str.to_string();
let snap = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t, 20))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();

// Option<T> — .flatten(), NOT .ok() — foreground.rs:833
let t = target_str.to_string();
let latch = tmux::off_runtime("read-pane-exit-status", move || {
    tmux::read_pane_exit_status(&t)
})
.await
.flatten();

// discard — foreground.rs:951
let cp2 = cp.to_string();
let _ = tmux::off_runtime("select-pane", move || tmux::select_pane(&cp2)).await;
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**. In `scheduled.rs`, `pane_id` is a `String`, so each
closure needs its **own `.clone()`**.

### ⚠ Hazard 1 — four sites keep their `Err(e)` arm, which needs a *fourth* shape

`scheduled.rs` sites `:216`, `:247`, `:261` and `:265` all bind the error and
use it — in a log line, in a user-facing message, and in
`store.mark_done(&job.id, false, Some(e.to_string()))`. Collapsing to a default
would throw that away, and there is no `e` on a timeout because the call never
returned one. **You must synthesise one.**

Collapse `Option<Result<T>>` down to `Result<T>` and leave the existing
`match` / `if let Err(e)` **completely untouched**:

```rust
let s = session.to_string();
let t = temp_win_name.to_string();
let created = tmux::off_runtime("create-job-window", move || {
    tmux::create_job_window(&s, &t)
})
.await
.unwrap_or_else(|| Err(anyhow::anyhow!("timed out creating window")));

let pane_id = match created {
    Ok(p) => p,
    Err(e) => {
        // ... every line of the existing arm, unchanged ...
    }
};
```

**This exact shape was compile-checked while drafting.** `.unwrap_or_else(||
Err(…))` is the whole trick: the `Some(Ok(_))` / `Some(Err(_))` cases pass
through unchanged and only the `None` case gains a new error.

Apply the same pattern to the other three, adapting the message:

| Site | Call | Existing form | Timeout message |
|---|---|---|---|
| 216 | `create_job_window` | `match … { Ok(p) => …, Err(e) => … }` | `"timed out creating window"` |
| 247 | `rename_window` | `match … { Ok(()) => …, Err(e) => … }` | `"timed out renaming window"` |
| 261 | `set_remain_on_exit` | `if let Err(e) = …` | `"timed out setting remain-on-exit"` |
| 265 | `send_keys` | `if let Err(e) = …` | `"timed out sending keys"` |

**Why an error and not a silent success:** a scheduled job whose window was
never created, or whose command was never sent, must be **marked failed** and
reported — the existing `Err` arms already do exactly that. Swallowing the
timeout would leave the job looking successful while nothing ran.

All four helpers return `anyhow::Result<…>` (`src/tmux/window.rs:72`, `:104`,
`src/tmux/pane.rs:478`, `:387`), so `anyhow::anyhow!` matches the error type.

### ⚠ Hazard 2 — `sudo.rs:86` is inside a short-circuited `||`

```rust
if waited >= TIMEOUT || crate::tmux::pane_dead_status(pane_id).is_some() {
    return false;
}
```

Today the tmux call **only runs when `waited < TIMEOUT`**. That must stay true —
otherwise every loop iteration past the deadline pays an extra subprocess. Use a
block expression on the right of the `||`, the same way `foreground.rs:525`
handles its `pane_pid` gate. **This shape was compile-checked while drafting:**

```rust
if waited >= TIMEOUT || {
    let p = pane_id.to_string();
    crate::tmux::off_runtime("pane-dead-status", move || {
        crate::tmux::pane_dead_status(&p)
    })
    .await
    .flatten()
    .is_some()
} {
    return false;
}
```

`pane_dead_status` returns `Option<i32>` (`src/tmux/pane.rs:114`), so the
collapse is **`.flatten()`**, not `.and_then(|r| r.ok())` — `Option` has no
`.ok()` and that will not compile.

A timeout yields `None` → `.is_some()` is `false` → the loop keeps polling until
`waited >= TIMEOUT`. That is the same thing a live-but-not-dead pane does today.

### ⚠ Hazard 3 — `scheduled.rs:296` is a `let`-chain inside `tokio::select!`

```rust
result = rx.recv() => {
    if let Ok(notified_pane) = result
        && notified_pane == pane_id
            && let Some(code) = tmux::pane_dead_status(&pane_id) {
                break code;
            }
}
```

`let Some(code) = <awaited value>` cannot stay in the chain. Split it, exactly
as the `read_pane_exit_status` sites in `foreground.rs:867` were split:

```rust
result = rx.recv() => {
    if let Ok(notified_pane) = result
        && notified_pane == pane_id
    {
        let p = pane_id.clone();
        let dead = tmux::off_runtime("pane-dead-status", move || {
            tmux::pane_dead_status(&p)
        })
        .await
        .flatten();
        if let Some(code) = dead {
            break code;
        }
    }
}
```

**`break code` must stay a `break`, not a `return`** — it exits the enclosing
`let exit_code = loop { … }` **with the exit code as its value**. A `select!`
arm is a plain block in the enclosing scope, so `break` works; turning it into
anything else changes what `exit_code` binds to.

### This phase's 11 sites

Line numbers are current-as-of-drafting; re-derive with the Acceptance-criteria
script.

**`src/daemon/scheduled.rs` — all inside `pub async fn run_scheduled_job`:**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 216 | `create_job_window` | `Result<String>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 247 | `rename_window` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 261 | `set_remain_on_exit` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 265 | `send_keys` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 286 | `pane_dead_status` | `Option<i32>` | `.flatten()` |
| 296 | `pane_dead_status` | `Option<i32>` | `.flatten()` + Hazard 3 restructure |
| 308 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |

**`src/daemon/utils/sudo.rs`:**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 72 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |
| 83 | `send_keys` | `Result<()>` | `let _ = …` (already discarded today) |
| 86 | `pane_dead_status` | `Option<i32>` | `.flatten()` — Hazard 2 |
| 102 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |

### ⚠ `capture_pane` depths differ — copy each site's own

`scheduled.rs:308` uses **5000**; both `sudo.rs` sites use **20**. Carry each
site's existing number through unchanged.

### Not a site — `tokio::process::Command`

`sudo.rs:48` runs `tokio::process::Command::new("sudo")` with `.status().await`.
That is **already non-blocking** and is not a tmux call at all. Leave it.

## Spec

### 1. Convert the 11 sites

Match each to its collapse from the tables above. **Preserve every existing
failure default and every existing `Err` arm exactly.**

### 2. Preserve short-circuiting and control flow

`sudo.rs:86`'s `||` must still skip the tmux call when `waited >= TIMEOUT`, and
`scheduled.rs:296`'s `break code` must still break the `exit_code` loop.

### 3. Build after every site

Not a suggestion. `cargo build` after each converted site.

## Acceptance criteria

- [ ] **Per-file scan reports `0` for both files:**

```bash
python3 - <<'PY'
import re, pathlib
for f in ["src/daemon/scheduled.rs", "src/daemon/utils/sudo.rs"]:
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
#   src/daemon/scheduled.rs: 0
#   src/daemon/utils/sudo.rs: 0
```

- [ ] `grep -c "off_runtime" src/daemon/scheduled.rs` returns **≥ 7** and
      `src/daemon/utils/sudo.rs` **≥ 4**. Both commands printed **0** before
      this phase. Floors, not identities — the scan proves the exact set.
- [ ] `grep -c "unwrap_or_else(|| Err(" src/daemon/scheduled.rs` returns
      **≥ 4** — the four `Err`-arm-preserving sites.
- [ ] `grep -c "flatten()" src/daemon/scheduled.rs` returns **≥ 2** and
      `src/daemon/utils/sudo.rs` **≥ 1** — the three `pane_dead_status` sites.
      Both printed **0** before this phase.
- [ ] `grep -c "and_then(|r| r.ok())"` — every occurrence in both files is on a
      helper returning `Result`. **No `pane_dead_status` site may have one.**
      Verify by reading.
- [ ] `grep -cF 'break code;' src/daemon/scheduled.rs` returns **≥ 2** — both
      loop exits still `break`, neither became a `return`.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking"` returns **0** in
      both files.
- [ ] `git diff --name-only` lists exactly **two** `src/` files.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_scheduled_job` needs a live tmux server, a scheduler store and a job
window; the three `sudo.rs` async fns need a live pane with a real sudo prompt.
**None has unit coverage.** Pre-existing gap, neither widened nor closed here.
`sudo.rs`'s `mod tests` (`:116`) covers only `command_has_sudo`, a pure string
function this phase does not touch, and `scheduled.rs` has no test module.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The `Err`-arm shape.** Paste one converted `scheduled.rs` site. Show the
   existing `Err(e)` arm is unchanged, and say in one sentence what the job's
   recorded outcome is when the tmux call times out.
2. **Short-circuiting.** Paste the converted `sudo.rs:86` and state whether the
   tmux subprocess runs when `waited >= TIMEOUT`.
3. **The `select!` restructure.** Paste the converted `scheduled.rs:296` and
   confirm `break code` is still a `break`, saying what value `exit_code` takes.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/scheduled.rs` and `src/daemon/utils/sudo.rs` — **the
      eleven named sites and whatever their expressions require.**
- [x] May add owned bindings and `.clone()` calls at call sites.
- [x] May add `anyhow::anyhow!` errors for the four timeout paths.
- [x] May split `scheduled.rs:296`'s `let`-chain into a block.
- [ ] **No** change to any function's signature.
- [ ] **No** change to any existing `Err(e)` arm's body.
- [ ] **No** touching `tokio::process::Command` at `sudo.rs:48`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the two named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`daemon/mod.rs`** — 9 convertible sites in `run_daemon`, plus 6 in the
  **synchronous** `detect_session` / `install_session_hooks`. Its own phase.
- **`background/`, `session.rs`, `ghost.rs`, `hook.rs`, `server/`** — later.
- **`cli/`** — largely synchronous; needs its own survey first.
- **`src/tmux/{pane,session,window,cache}.rs`** — those `Command::new("tmux")`
  calls **are** the sync helper bodies, not call sites. Stage A hardens them.
  **Do not touch them.**

### ⚠ Traps

1. **Four shapes now.** `Result` → `.and_then(|r| r.ok())`; `Option` →
   `.flatten()`; discard → `let _ =`; **`Result` whose `Err` is used** →
   `.unwrap_or_else(|| Err(anyhow::anyhow!(…)))`. Picking the wrong neighbour's
   form is the likeliest failure.
2. **`pane_dead_status` returns `Option<i32>`** — `.ok()` will not compile.
3. **Keep the `||` short-circuit** at `sudo.rs:86`.
4. **Keep `break code` a `break`** at `scheduled.rs:296`.
5. **`capture_pane` depths differ** — 5000 in `scheduled.rs`, 20 in `sudo.rs`.
6. **`pane_id` is a `String` in `scheduled.rs`** — clone per closure.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
