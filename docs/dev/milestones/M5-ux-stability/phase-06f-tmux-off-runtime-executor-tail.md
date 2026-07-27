# Phase 06f: tmux Calls Off the Runtime — the `executor/` Tail

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06e — `done` (`foreground.rs` is finished)
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to the **12** convertible tmux calls remaining under
`src/daemon/executor/` outside `foreground.rs`:

| File | Convertible sites |
|---|---|
| `knowledge/pane.rs` | 7 |
| `file_ops/mod.rs` | 2 |
| `file_ops/read.rs` | 3 |

**Finish condition: the per-file scan reports `pane.rs: 4`, `file_ops/mod.rs:
0`, `read.rs: 1` — and every remaining hit is on the do-not-convert list
below.**

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
grep -c "off_runtime" src/daemon/executor/knowledge/pane.rs   # expect 0
grep -c "off_runtime" src/daemon/executor/file_ops/mod.rs     # expect 0
grep -c "off_runtime" src/daemon/executor/file_ops/read.rs    # expect 0
cargo test 2>&1 | grep "^test result" | head -3               # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### 🛑 Read this first — 3 hits are in *synchronous* functions and must NOT be converted

This is the difference between this phase and 06c–06e, where every hit lived in
an `async fn`. `off_runtime` is `async`; **you cannot `.await` inside a
synchronous function.** Attempting it produces `error[E0728]: await is only
allowed inside async functions and blocks`, and the fix is *not* to sprinkle
`async` — it changes the function's signature, every call site, and its tests.

| Hit | Enclosing fn | Why it is not this phase's |
|---|---|---|
| `pane.rs:46` — `kill_job_window` | `pub fn close_bg_window` (`:13`) — **sync** | needs the fn to become `async`; called from `executor/mod.rs:561`, and 2 unit tests at `pane.rs:420`/`:429` call it directly |
| `pane.rs:196` — `pane_current_command` | `pub fn watch_pane` (`:188`) — **sync prologue** | same; called from `executor/mod.rs:388` |
| `pane.rs:209` — inline `Command` `set-hook` | `pub fn watch_pane` — **sync prologue** | same |

Making those two functions `async` is a **restructure**, deferred to its own
phase — the same rule that moved the `background/` restructures out of the
conversion sweep and into phase 05.

**`watch_pane` is only sync down to `:235`.** At that line it calls
`tokio::spawn(async move { … })`, and **everything inside that spawned task is
async and IS in scope** — that is where 6 of this phase's 7 `pane.rs` sites
live.

### ⚠ One more non-site — `tmux::wait_for` is already `async`

```rust
// src/tmux/pane.rs:515
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
```

`read.rs:63` calls it as `tmux::wait_for(&buf_name, …).await`. It is **already
off the blocking path** and must be left exactly as it is. Wrapping an `async
fn` in `off_runtime` would hand `spawn_blocking` a future that is never polled.
The scan still prints it because the regex matches `tmux::`; that is expected.

### ⭐ Worked examples — `foreground.rs` now carries every shape you need

06c–06e converted all 29 sites in `src/daemon/executor/foreground.rs`. Read it.

```rust
// Result-returning, value used, failure collapses to a default — foreground.rs:749
let t = target_str.to_string();
let snap = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t, 20))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();

// ()-returning helper — no .ok() — foreground.rs:906
let t = target_str.to_string();
let cp = chat_pane.map(|s| s.to_string());
let _ = tmux::off_runtime("unhighlight-pane", move || {
    tmux::unhighlight_pane(&t, cp.as_deref())
})
.await;

// inline std::process::Command — foreground.rs:797
let th = target_str.to_string();
let shn = silence_hook_name.clone();
let nh = notify_cmd.clone();
let _ = tmux::off_runtime("set-hook", move || {
    std::process::Command::new("tmux")
        .args(["set-hook", "-t", &th, &shn, &nh])
        .output()
})
.await;
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**.

### This phase's 12 sites

Line numbers are current-as-of-drafting; re-derive with the Acceptance-criteria
script.

**`knowledge/pane.rs` — all 7 inside the `tokio::spawn` at `:235`:**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 255 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |
| 260 | `capture_pane` | `Result<String>` | same |
| 270 | `pane_current_command` | `Result<String>` | same |
| 281 | `pane_current_command` | `Result<String>` | same |
| 286 | `pane_current_command` | `Result<String>` | same |
| 294 | `capture_pane` | `Result<String>` | same |
| 360 | inline `Command` — `display-message` | — | `let _ = …` |

All six of the first group read `…(&pane_id_owned, …).unwrap_or_default()`
today. `pane_id_owned` is a `String` already in scope, so each closure needs its
**own clone** — `let p = pane_id_owned.clone();` — because the previous
conversion moved the last one.

**`file_ops/mod.rs` — inside `async fn remote_run_and_capture` (`:40`):**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 45 | `send_keys` | `Result<()>`, propagated with `?` | **see Hazard below** |
| 52 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |

**`file_ops/read.rs` — inside `async fn local_read_via_buffer` (`:49`):**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 60 | `send_keys` | `Result<()>`, propagated with `?` | **see Hazard below** |
| 67 | `save_buffer` | `Result<Vec<u8>>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |
| 68 | `delete_buffer` | `()` | `let _ = …` |

### ⚠ Hazard — the two `send_keys` sites propagate with `?`, and there is no precedent in the tree

Both read like this today:

```rust
tmux::send_keys(pane_id, cmd)?;
```

`off_runtime` yields `Option<Result<()>>`, so **a timeout has no error to
propagate — you must create one.** A timeout must become an `Err`, not a silent
success: if `send_keys` never ran, `remote_run_and_capture` would poll for a
`__DE_DONE__` marker that can never appear, and `local_read_via_buffer` would
wait on a buffer that was never loaded.

**This exact form was compile-checked while drafting. Use it:**

```rust
let p = pane_id.to_string();
let c = cmd.to_string();
tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &c))
    .await
    .ok_or_else(|| anyhow::anyhow!("timed out sending keys to pane {pane_id}"))??;
```

Three things about it that are easy to get wrong:

1. **`??`, not `?`.** `.ok_or_else(…)` turns `Option<Result<()>>` into
   `Result<Result<()>, anyhow::Error>`. The first `?` unwraps the outer, the
   second the inner. A single `?` leaves an unused `Result` and fails the lint
   gate.
2. **`pane_id` is still usable in the message.** Only the *clone* `p` was moved
   into the closure; the original `&str` parameter is untouched.
3. `read.rs:60` passes `&cmd` where `cmd` is a local `String` — clone it the
   same way; do not move the original if it is used later.

Both enclosing functions already return `anyhow::Result<…>`, so `?` works
unchanged.

### ⚠ `capture_pane` depth arguments differ — copy each site's own

`mod.rs:52` uses depth **600**; the `pane.rs` sites use **200**. Carry each
site's existing number through. They are not interchangeable.

## Spec

### 1. Convert the 12 sites

Match each to its collapse from the tables above. **Preserve every existing
failure default exactly** — `.unwrap_or_default()` stays, the `?` sites become
`Err` on timeout, the discards stay discards.

### 2. Convert nothing on the do-not-convert list

`pane.rs:46`, `:182` (the `Drop` impl), `:196`, `:209`, and `read.rs:63`
(`wait_for`). Five hits, all deliberate.

### 3. Build after every site

Not a suggestion. `cargo build` after each converted site. An earlier run on
`foreground.rs` died because one conversion's type error surfaced 470 lines from
its cause and could not be localised.

## Acceptance criteria

- [ ] **Per-file scan reports `4 / 0 / 1`:**

```bash
python3 - <<'PY'
import re, pathlib
for f in ["src/daemon/executor/knowledge/pane.rs",
          "src/daemon/executor/file_ops/mod.rs",
          "src/daemon/executor/file_ops/read.rs"]:
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
#   knowledge/pane.rs: 4   -> kill_job_window, Command::new (Drop),
#                            pane_current_command, Command::new  (~46, 182, 196, 209)
#   file_ops/mod.rs:   0
#   file_ops/read.rs:  1   -> wait_for  (~63)
```

      **Read the names, not just the counts.** A different name means the wrong
      site was converted.

- [ ] `grep -c "off_runtime" src/daemon/executor/knowledge/pane.rs` returns
      **≥ 7**; `…/file_ops/mod.rs` **≥ 2**; `…/file_ops/read.rs` **≥ 3**. Each
      command printed **0** before this phase. Floors, not identities — the scan
      above proves the exact set.
- [ ] `grep -c "??;" src/daemon/executor/file_ops/mod.rs` and the same for
      `read.rs` each return **≥ 1** — the two `send_keys` propagation sites.
- [ ] `impl Drop for WatchHookGuard` (`pane.rs`, ~`:180`–`:186`) is
      **byte-identical**. Verify with `diff` against the parent commit, not by
      eye, and quote the result.
- [ ] `pub fn close_bg_window` and `pub fn watch_pane` still have **`pub fn`**
      signatures, not `pub async fn`. Quote both lines.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking"` returns **0** in
      all three files.
- [ ] `git diff --name-only` lists exactly **three** `src/` files.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`watch_pane`, `remote_run_and_capture` and `local_read_via_buffer` all need a
live tmux server and pane; **none has unit coverage**. Pre-existing gap, neither
widened nor closed here. `pane.rs`'s `mod tests` (`:389`) covers only
`close_bg_window`'s store bookkeeping — a function this phase does not touch —
and `read.rs`'s (`:236`) covers only the pure command-builder helpers.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The sync boundary.** Quote the `pub fn watch_pane` signature line and the
   `tokio::spawn(async move {` line, and state which of your converted `pane.rs`
   sites are after the second one. Explain in one sentence why `:196` could not
   be converted.
2. **`send_keys` propagation.** Paste one converted site. Show it has `??` and
   say what the function returns to its caller when the tmux call times out.
3. **`wait_for`.** Quote the line and state in one sentence why wrapping it in
   `off_runtime` would be wrong.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/executor/knowledge/pane.rs`,
      `src/daemon/executor/file_ops/mod.rs`,
      `src/daemon/executor/file_ops/read.rs` — **the twelve named sites and
      whatever their expressions require.**
- [x] May add owned bindings and `.clone()` calls at call sites.
- [x] May add an `anyhow::anyhow!` error for the two `send_keys` timeouts.
- [ ] **No** change to any function's signature — in particular, **do not make
      `close_bg_window` or `watch_pane` `async`.**
- [ ] **No** edit to `impl Drop for WatchHookGuard`.
- [ ] **No** wrapping of `tmux::wait_for`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the three named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **Making `close_bg_window` / `watch_pane` async** — a restructure, its own
  phase.
- **The `Drop` impls** — structurally unconvertible; bounded by stage A.
- **`daemon/` core, `cli/`, `scheduled.rs`, `webhook/`** — 06g–06h.
- **`src/tmux/{pane,session,window,cache}.rs`** — those `Command::new("tmux")`
  calls **are** the sync helpers themselves, not call sites. Hardening them with
  timeouts is stage A. **Do not touch them.**

### ⚠ Traps

1. **Three hits are in sync fns.** `pane.rs:46`, `:196`, `:209`. Converting one
   yields `E0728`; do not try to fix it by adding `async`.
2. **`??` not `?`** at the two `send_keys` sites.
3. **`tmux::wait_for` is already async** — leave it.
4. **Clone `pane_id_owned` per closure** — six closures in `pane.rs` need it,
   and each move consumes it.
5. **`capture_pane` depths differ** — 600 in `mod.rs`, 200 in `pane.rs`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 20:56 (started)

**Executor:** Claude executor
**Action:** Started phase 06f — converting 12 tmux calls in `knowledge/pane.rs` (7), `file_ops/mod.rs` (2), `file_ops/read.rs` (3) to `off_runtime`.
