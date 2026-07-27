# Phase 06a: Get tmux Subprocess Calls Off the Async Runtime

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-05h — `done`
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Establish the **mechanism-B adapter** and apply it to `background/run.rs`.

Every `tmux` call is a blocking subprocess spawn. **88 of them run inside `async
fn`s across 16 files**, so a wedged tmux server stalls a tokio worker thread —
the milestone's fourth exit criterion:

> Every tmux subprocess call made from an async context is either non-blocking
> (`tokio::process`) or off the runtime (`spawn_blocking`), and carries a
> timeout. A wedged tmux server degrades one operation instead of the whole
> daemon.

**This phase does two things:** adds one adapter (`tmux::off_runtime`), and
converts the **16** call sites in `background/run.rs` as the worked example the
remaining phases copy.

**Finish condition: `background/run.rs` has 0 unwrapped `tmux::` calls in async
context, and the adapter exists with a timeout.**

**This is the first of ~5 phases.** 06b–06e apply the same adapter to
`background/respawn.rs`, `executor/`, the `daemon/` core, and `cli/`. Do not
touch them here.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `CLAUDE.md` § "Important Invariants" — `main()` is synchronous so `libc::fork()`
  precedes the runtime. Nothing in this phase moves the fork.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "tmux::" src/daemon/background/run.rs                    # expect 14
grep -c 'Command::new("tmux")' src/daemon/background/run.rs      # expect 2
grep -rc "spawn_blocking" src/ --include=*.rs | grep -v ':0' | wc -l   # expect 0
grep -c "off_runtime" src/tmux/mod.rs                # expect 0
cargo test 2>&1 | grep "^test result" | head -2      # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** `spawn_blocking` appears **nowhere**
in this codebase — this phase introduces it. If any count differs, **stop and
report a blocker.**

## Current state

### The problem, concretely

`src/tmux/` helpers are **synchronous** and call
`std::process::Command::new("tmux") … .output()`, which blocks the calling thread
until tmux answers. Called from an `async fn`, that thread is a tokio worker.

The helpers stay sync — they are also called from sync CLI code, and making them
async would force a duplicate API. **The adapter goes at the async call site.**

### ⭐ The one existing async tmux call — `pane::wait_for` (`src/tmux/pane.rs:515`)

The codebase already does this correctly in exactly one place. Quote it for the
shape of "bound a tmux operation in time":

```rust
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
    let mut child = match tokio::process::Command::new("tmux")
        .args(["wait-for", channel])
        .spawn()
    { Ok(c) => c, Err(_) => return false };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => true,
        Err(_) => { let _ = child.start_kill(); /* … */ false }
    }
}
```

**Do not convert `wait_for`.** It is already non-blocking and already bounded;
it is here as the reference, not as work.

### The 16 sites in `background/run.rs` — two populations

| Population | Count | Lines |
|---|---|---|
| `tmux::<helper>(…)` calls | **14** | 66, 75, 98, 103, 153, 157, 159, 161, 180, 187, 240, 297, 350, 409 |
| inline `std::process::Command::new("tmux")` | **2** | 254, 360 |

`use crate::tmux;` (line 11) does **not** match `tmux::` and is not a site.

**There are no `tmux::wait_for` calls in this file.** The two `wait_for` hits are
`wait_for_sudo_prompt_and_inject`, which is not a tmux call at all — do not
convert it, and do not mistake it for the async helper quoted above.

Re-derive both lists with the script in Acceptance criteria rather than working
from the table; the line numbers will shift as you edit.

## Spec

### 1. Add the adapter to `src/tmux/mod.rs`

`src/tmux/mod.rs` is currently a 9-line module declaration file. Append:

```rust
/// Ceiling for a single tmux subprocess call made from async code.
///
/// tmux normally answers in milliseconds; five seconds means the server is
/// wedged, and waiting longer cannot help the caller.
pub const TMUX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run a blocking `tmux` helper off the async runtime, bounded by [`TMUX_TIMEOUT`].
///
/// The `src/tmux/` helpers are synchronous `std::process::Command` calls: invoked
/// directly from an `async fn` they block a tokio worker until tmux answers, and
/// a wedged tmux server therefore stalls the whole daemon. This moves the call to
/// the blocking pool and gives up on it after the timeout, so a wedge degrades
/// one operation instead of the reactor. See `docs/design/daemon-stalls.md`
/// § 1 mechanism B.
///
/// Returns `None` if the call timed out or the blocking task panicked — both are
/// logged. `Some(v)` carries whatever the helper returned, including its own
/// `Err`.
pub async fn off_runtime<T, F>(what: &'static str, f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(TMUX_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            log::error!("tmux {what}: blocking task panicked: {e}");
            None
        }
        Err(_) => {
            log::error!("tmux {what}: timed out after {TMUX_TIMEOUT:?} — tmux server may be wedged");
            None
        }
    }
}
```

**Three deliberate choices:**

- **`what: &'static str`** — a timeout log with no operation name is unactionable.
  Pass the tmux verb (`"kill-job-window"`, `"capture-pane"`).
- **`Option<T>`, not `Result`** — timeout-or-panic is *"we do not know"*, which is
  distinct from the helper's own `Err` (*"tmux said no"*). Collapsing them would
  hide a wedge as an ordinary failure.
- **No retry.** A wedged tmux does not get better in 5 s; retrying multiplies the
  stall. Callers degrade instead.

### 2. Convert the 16 sites in `background/run.rs`

`spawn_blocking` requires `F: 'static`, so **every borrowed argument must become
owned** before the closure. That is the per-site work; it is not a textual
substitution.

There are exactly **four shapes** in this file. Match each site to one.

**Shape A — result ignored (`let _ = …` or a bare statement):**

```rust
// before
let _ = tmux::kill_job_window(session, &win_name);
// after
let (s, w) = (session.to_string(), win_name.clone());
let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s, &w)).await;
```

**Shape B — result used with a default:**

```rust
// before
let snap = crate::tmux::capture_pane(&pane_id, 10).unwrap_or_default();
// after
let p = pane_id.clone();
let snap = tmux::off_runtime("capture-pane", move || crate::tmux::capture_pane(&p, 10))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();
```

`.and_then(|r| r.ok())` is the load-bearing part: the outer `Option` is
timeout-or-panic, the inner `Result` is tmux's own answer. **Both** collapse to
the default, and that is correct here — a snapshot we could not take is an empty
snapshot either way.

**Shape C — the error is inspected:**

```rust
// before
if let Err(e) = tmux::set_remain_on_exit(&pane_id, true) {
    log::warn!("…: {e}");
}
// after
let p = pane_id.clone();
match tmux::off_runtime("set-remain-on-exit", move || tmux::set_remain_on_exit(&p, true)).await {
    Some(Err(e)) => log::warn!("…: {e}"),
    None => {}          // already logged by off_runtime
    Some(Ok(_)) => {}
}
```

**Never write `None => log::warn!(…)` in shape C** — `off_runtime` already logged
the timeout with the operation name. A second line per timeout is noise.

**Shape D — an inline `std::process::Command::new("tmux")`** (lines 254 and 360,
both identical in form):

```rust
// before
let _ = std::process::Command::new("tmux")
    .args(["pipe-pane", "-t", &pane_id])
    .output();
// after
let p = pane_id.clone();
let _ = tmux::off_runtime("pipe-pane", move || {
    std::process::Command::new("tmux")
        .args(["pipe-pane", "-t", &p])
        .output()
})
.await;
```

Both sites stop a pipe-pane and are best-effort — Shape A's `let _ =` treatment is
correct for them. **Do not "improve" them into a `tmux::` helper call**; adding a
helper is a separate change and would widen the diff.

**Two things not to change:**

- **Early returns keep their meaning.** A site that does
  `match tmux::create_job_window(..) { Ok(v) => v, Err(e) => return Err(e) }` must
  still return on failure — and a **timeout must also return**, because the caller
  cannot proceed without a window. Map `None` to the same failure path, with a
  message naming the timeout.
- **`tmux::wait_for` is already correct** — leave every call to it alone.

### 3. Do not convert anything outside `background/run.rs`

`respawn.rs`, `gc.rs`, `executor/`, `daemon/mod.rs`, `cli/` all have async tmux
calls. **They are phases 06b–06e.** A criterion below pins their counts so an
over-eager sweep fails.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/tmux/mod.rs` returns **1** — the `pub async fn`
      signature. (The doc comment references `TMUX_TIMEOUT`, not the function's own
      name, so there is no second hit.) Verify by **reading** that `TMUX_TIMEOUT`
      is applied via `tokio::time::timeout` wrapping `spawn_blocking` — the count
      cannot show that.
- [ ] Every `tmux::` call in an async context in `background/run.rs` is inside an
      `off_runtime` closure. Check with:

```bash
python3 - <<'PY'
import re, pathlib
src = pathlib.Path("src/daemon/background/run.rs").read_text()
lines = src.splitlines()
bad = []
for i, l in enumerate(lines, 1):
    if not re.search(r'\btmux::', l): continue
    if 'off_runtime' in l or 'wait_for' in l or l.strip().startswith('use '): continue
    # a helper named inside an off_runtime closure appears on the same line as `move ||`
    if 'move ||' in l: continue
    bad.append((i, l.strip()))
print("UNWRAPPED:", len(bad))
for i, l in bad: print(f"  {i}: {l}")
PY
#   UNWRAPPED: 0
```

- [ ] `grep -c "spawn_blocking" src/tmux/mod.rs` returns **1** — the only one in
      the tree at the end of this phase.
- [ ] `grep -rc "spawn_blocking" src/ --include=*.rs | grep -v ':0' | wc -l`
      returns **1** — one file (`src/tmux/mod.rs`). **Not more**: the adapter is
      the only place `spawn_blocking` appears; call sites use `off_runtime`.
- [ ] The out-of-scope files are untouched — `git diff --name-only` lists exactly
      **`src/tmux/mod.rs`** and **`src/daemon/background/run.rs`** under `src/`.
- [ ] `grep -c "tmux::" src/daemon/background/respawn.rs` returns **10** and
      `grep -c 'Command::new("tmux")' src/daemon/background/respawn.rs` returns
      **3** — both unchanged. Those 13 sites are phase 06b's; a lower number means
      you swept out of scope.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests —
      both unchanged. This phase adds no tests.
- [ ] `python3 /tmp/audit_closures.py` still prints nothing — no `tmux::` call may
      end up inside a `with_sessions` closure. (`off_runtime` is `async`, so a
      `with_sessions` closure **cannot** contain one; if you find yourself needing
      that, the restructure is wrong.)

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status.

## Test plan

`background/run.rs` runs background commands in real tmux windows and has **no
unit coverage** — it needs a live tmux server, a spawned window and a pane-death
hook. That is a pre-existing gap this phase neither widens nor closes, and it is
why the spec gives exact target code for all three shapes.

**Write no new tests.** The 916 + 27 existing tests are the regression net for
*compilation and unrelated behavior*; they cannot exercise this file.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test guards these sites — that would
be false, and in this project a coverage claim is admissible only when
demonstrated by mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Timeout vs error.** Explain in one sentence why `off_runtime` returns
   `Option<Result<…>>` rather than flattening to one `Result`, and name one site
   where the distinction changes behavior.
2. **Ownership.** Name one site where a borrowed argument had to become owned, and
   say what the compiler error would have been without it.
3. **Early returns.** Confirm every site that returned on `Err` also returns on
   timeout, and name them.

## End-to-end verification

**Demonstrate the timeout fires**, since no test can. Temporarily set
`TMUX_TIMEOUT` to `Duration::from_millis(1)`, run
`cargo test --lib` (or any code path that calls a converted site — a short
`#[tokio::test]` scratch harness is acceptable **if you delete it**), and quote
the `tmux …: timed out after …` log line. Then restore `TMUX_TIMEOUT` to 5 s.

If you cannot trigger it without adding a permanent test, say so and quote the
adapter code instead, explaining why the timeout arm is reachable. **Do not add a
permanent test to make this easier** — the phase adds none.

`git status` must be clean when you finish.

## Authorizations

- [x] May edit `src/tmux/mod.rs` (the adapter) and
      `src/daemon/background/run.rs` (the 16 sites).
- [x] May add owned bindings (`let p = pane_id.clone();`) at call sites — that is
      what `spawn_blocking`'s `'static` bound requires.
- [x] May temporarily lower `TMUX_TIMEOUT` for the end-to-end demonstration,
      provided it is restored and `git status` is clean.
- [ ] **No** new dependency. `tokio`'s `rt-multi-thread` and `time` features are
      already enabled; `spawn_blocking` and `timeout` need nothing further.
- [ ] **No** conversion of `src/tmux/` helpers to `async fn` — they are also
      called from sync CLI code and an async duplicate is not authorised.
- [ ] **No** edits to `tmux::wait_for` — already non-blocking and bounded.
- [ ] **No** edits to any file other than the two named above.
- [ ] **No** new tests, no retry loops, no `#[allow(...)]`.

## Out of scope

- **The other 72 async tmux sites** — `respawn.rs` (12), `executor/foreground.rs`
  (15), `daemon/mod.rs` (8), `scheduled.rs` (7), `cli/commands/chat.rs` (10) and
  the rest. Phases **06b–06e**.
- **Hardening the sync helpers themselves** (a timeout inside `src/tmux/`, so
  sync CLI callers are bounded too). That is the agreed **second** stage, after
  the async sites are off the runtime.
- **The 61 sync-only call sites.** Blocking a CLI process is not the defect this
  criterion describes.

### ⚠ Three traps from earlier phases in this milestone

1. **`grep` is line-oriented and blind to multi-line forms.** Several `tmux::`
   calls span lines. The Acceptance criteria script exists for that reason —
   this blindness has cost this milestone a bounce and three missed production
   sites.
2. **State what happens to imports.** If converting the last user of an import in
   either file leaves it unused, **delete it** — and note that `cargo build`
   reports zero warnings for an unused *test-module* import while
   `cargo clippy --all-targets` errors. Clippy is authoritative.
3. **Do not insert an item between a doc comment and the item it documents.**
   Task 1 appends two documented items to `src/tmux/mod.rs`; append **after** the
   existing module declarations, and re-read the lines above your insertion point.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
