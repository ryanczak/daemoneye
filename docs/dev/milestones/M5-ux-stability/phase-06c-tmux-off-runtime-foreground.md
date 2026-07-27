# Phase 06c: Get `foreground.rs`'s tmux Calls Off the Async Runtime

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-06b — `done`
**Estimated diff:** ~230 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Apply `tmux::off_runtime` to the **29** tmux subprocess calls that run in async
context in `src/daemon/executor/foreground.rs`.

This is the interactive execution path — the one that injects a command into the
user's own pane and waits for it. Every tmux call in it currently blocks a tokio
worker until tmux answers.

**Finish condition: the span-matching script reports `UNWRAPPED: 0` for the async
sites, and the two `Drop` sites are left exactly as they are.**

Largest single-file phase of the 06 series. The pattern is established by 06a/06b
— **read `background/run.rs` and `background/respawn.rs` first**; every shape you
need is already there.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `src/tmux/mod.rs` — the `off_runtime` adapter and `TMUX_TIMEOUT`. Read the doc
  comment; it explains the `Option<T>` return.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "tmux::" src/daemon/executor/foreground.rs                 # expect 27
grep -c 'Command::new("tmux")' src/daemon/executor/foreground.rs   # expect 5
grep -c "off_runtime" src/daemon/executor/foreground.rs            # expect 0
grep -c "off_runtime" src/daemon/background/respawn.rs             # expect 15
cargo test 2>&1 | grep "^test result" | head -2                    # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

## Current state

### ⭐ Two in-tree worked examples — read them, do not invent

`background/run.rs` (06a) and `background/respawn.rs` (06b) contain every shape
this phase needs. The canonical forms:

```rust
// value used, both failure modes collapse to a default
let p = pane_id.to_string();
let out = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&p, 10))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();

// error inspected; timeout arm is a no-op because off_runtime already logged it
let (s, w) = (session.to_string(), win_name.to_string());
match tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s, &w)).await {
    Some(Err(e)) => log::error!("…: {e}"),
    None => {} // already logged by off_runtime
    Some(Ok(_)) => {}
}

// helper returning Option<T> needs .flatten()
let p = pane_id.to_string();
let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p))
    .await
    .flatten()
    .unwrap_or(-1);
```

**The `Option<Result<…>>` shape is the point.** `None` = timeout or panic
(already logged); `Some(Err(e))` = tmux refused. Do not collapse them.

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes owned
before the closure**. That is the per-site work.

### ⚠ The two `Drop` sites are NOT convertible — leave them exactly as they are

`foreground.rs:71-84`:

```rust
impl Drop for FgHookGuard {
    fn drop(&mut self) {
        for hook in &self.hooks {
            let _ = std::process::Command::new("tmux")
                .args(["set-hook", "-u", "-t", &self.target, hook])
                .output();
        }
        if self.monitor_silence {
            let _ = std::process::Command::new("tmux")
                .args(["set-option", "-u", "-t", &self.target, "monitor-silence"])
                .output();
        }
    }
}
```

**`Drop::drop` cannot be `async`**, so `off_runtime` — an `async fn` — cannot be
called there. There is no way to `.await` inside a destructor. These two calls
stay synchronous.

That is a real, acknowledged gap: hook teardown still blocks whatever thread drops
the guard. **Bounding it belongs to the later sync-helper stage**, which puts a
timeout inside `src/tmux/` itself. **Do not** attempt a workaround here — no
`block_on`, no detached `tokio::spawn` in `drop`, no `futures::executor`. Any of
those would be worse than the blocking call.

### ⚠ `tmux::cache::SessionCache` (line 23) is a type import, not a call

```rust
use crate::tmux::cache::SessionCache;
```

It matches `tmux::` but spawns nothing. The Acceptance-criteria script excludes
it via its `PURE` set; **do not wrap it**, and do not "tidy" the import.

### ⚠ `wait_for` in this file is not `tmux::wait_for`

Lines 12 and 563 reference **`wait_for_sudo_prompt_and_inject`**, a local async
helper — not the bounded `tmux::wait_for`. It is not a tmux call and is not a
site. (Same false friend as in `background/run.rs`.)

### The site population

| Population | Count |
|---|---|
| in `async fn run_foreground` | 28 |
| in `async fn run_background` | 1 |
| **convertible total** | **29** |
| in `impl Drop` (not convertible) | 2 |
| non-call `tmux::` hits (the `SessionCache` import) | 1 |

`grep -c "tmux::"` returns 27 and `grep -c 'Command::new("tmux")'` returns 5 —
**line** counts, which do not equal the site count. Re-derive the sites with the
Acceptance-criteria script; do not work from these numbers.

## Spec

### 1. Convert all 29 async-context sites

Match each to a shape from `run.rs`/`respawn.rs`. Most are `capture_pane`,
`pane_current_command`, `pane_pid`, `select_pane`, `send_keys`,
`highlight_pane`/`unhighlight_pane`, `read_pane_exit_status`, `pane_exists` — all
ordinary A/B/C shapes.

Three need specific care:

**`pane_exists` (`:200`, `:883`) gates behaviour.** It returns `bool`. A timeout
must **not** be read as "the pane exists":

```rust
let p = target.to_string();
let exists = tmux::off_runtime("pane-exists", move || tmux::pane_exists(&p))
    .await
    .unwrap_or(false);
```

Treating a wedged tmux as "pane present" would send keys into a pane that may be
gone. `.unwrap_or(false)` is the safe direction; make sure the surrounding
control flow still does the right thing when it is `false`.

**The highlight/unhighlight pair must stay balanced.** `highlight_pane` (`:376`)
sets a background colour that `unhighlight_pane` (`:602`, `:773`) clears. If a
highlight succeeds and the unhighlight is skipped, **the user's pane stays tinted
until they restart tmux**. So the unhighlight calls are Shape A (`let _ = …`) and
must run on every path they run on today — including error and timeout paths.
**Do not add an early return that skips one.**

**`read_pane_exit_status` (`:727`, `:750`) decides the reported exit code.** Follow
whatever default the current code uses on failure, and apply it to the timeout arm
too — do not invent a new sentinel.

### 2. Leave the `Drop` impl untouched

Two `std::process::Command::new("tmux")` calls at `:74` and `:79`. **No edit at
all** — not a comment, not a reformat.

### 3. Change nothing outside this file

`knowledge/pane.rs` (11 sites) and `file_ops/` (6) are phase 06d.

## Acceptance criteria

- [ ] **Span-matching check reports `UNWRAPPED: 2`, and both are the `Drop`
      sites.** Use this script — a `move ||` line heuristic gives false positives
      because rustfmt puts closure bodies on their own line:

```bash
python3 - <<'PY'
import re, pathlib
src = pathlib.Path("src/daemon/executor/foreground.rs").read_text()
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
PURE = {"off_runtime", "TMUX_TIMEOUT", "cache"}   # cache = the SessionCache type import, not a call
bad = [(src[:m.start()].count("\n")+1, m.group(1))
       for m in re.finditer(r'\btmux::(\w+)', src)
       if m.group(1) not in PURE and not inside(m.start())]
bad += [(src[:m.start()].count("\n")+1, 'Command::new("tmux")')
        for m in re.finditer(r'Command::new\("tmux"\)', src) if not inside(m.start())]
print("UNWRAPPED:", len(bad))
for l, n in sorted(bad): print(f"  {l}: {n}")
PY
#   UNWRAPPED: 2
#     <line>: Command::new("tmux")     <- both inside impl Drop
#     <line>: Command::new("tmux")
```

      **Read the two reported lines and confirm both are inside `impl Drop for
      FgHookGuard`.** Any other line means a site was missed.

- [ ] `grep -c "off_runtime" src/daemon/executor/foreground.rs` returns **≥ 29** —
      one per converted site, plus any duplicated in a timeout arm. Below 29 means
      a site was missed; the span check proves the exact set.
- [ ] `grep -n "impl Drop for FgHookGuard" -A 14 src/daemon/executor/foreground.rs`
      is **byte-identical** to its current form. Quote it in the Update Log.
- [ ] `grep -c "spawn_blocking" src/daemon/executor/foreground.rs` returns **0**,
      and `grep -rn "block_on\|futures::executor" src/daemon/executor/` returns
      **nothing** — no workaround was attempted in `drop`.
- [ ] `git diff --name-only` lists exactly **one** `src/` file:
      `src/daemon/executor/foreground.rs`.
- [ ] `grep -c "tmux::" src/daemon/executor/knowledge/pane.rs` returns **10**,
      unchanged — phase 06d's, and a lower number means you swept out of scope.
- [ ] `grep -c "off_runtime" src/daemon/background/respawn.rs` returns **15**,
      unchanged.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests —
      both unchanged. This phase adds no tests.

**Run every gate bare** — piping through `tail` exits with `tail`'s status.

## Test plan

`run_foreground` injects a command into a live user pane and polls it to
completion; it has **no unit coverage** and cannot have any without a real tmux
server and a live pane. That is a pre-existing gap this phase neither widens nor
closes, and it is why the spec gives exact target code for the three
behaviour-sensitive sites.

**Write no new tests.** The 916 + 27 existing tests are the regression net for
compilation and unrelated behavior.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test guards these sites.

Three reasoning checks to state in the Update Log. **For each, quote the code —
a claim without a quotation is not an answer:**

1. **The `Drop` sites.** Paste the `impl Drop for FgHookGuard` block as it stands
   after your changes, and state in one sentence why `off_runtime` cannot be used
   there.
2. **Highlight/unhighlight balance.** Give the **line number and the surrounding
   match arm** for every `unhighlight_pane` call, and show that each still runs on
   the same paths it did before — including timeout arms.
3. **`pane_exists` on timeout.** Quote both converted `pane_exists` sites with
   their `.unwrap_or(…)`, and say what would go wrong if a timeout were read as
   `true`.

*(These ask for quoted evidence rather than a summary because the last two phases'
reasoning checks each over-counted early-return sites. Quote the code and the
answer checks itself.)*

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no new
machinery, only call sites. **Do not repeat that demonstration** and do not add a
test to make it repeatable.

## Authorizations

- [x] May edit `src/daemon/executor/foreground.rs` — the 29 async sites.
- [x] May add owned bindings (`let p = target.to_string();`) at call sites.
- [ ] **No** edit of any kind to `impl Drop for FgHookGuard`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn` anywhere —
      in `drop` or elsewhere.
- [ ] **No** edits to `src/tmux/mod.rs`; the adapter is complete.
- [ ] **No** edits to `knowledge/pane.rs`, `file_ops/`, or any other file.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **The two `Drop`-impl calls.** Structurally unconvertible; bounding them belongs
  to the sync-helper stage.
- **`executor/knowledge/pane.rs` (11 sites) and `file_ops/` (6)** — phase 06d.
- **`daemon/` core and `cli/`** — phases 06e/06f.
- **Hardening the sync helpers themselves** — the agreed stage after all async
  sites are off the runtime.

### ⚠ Three traps from this phase family

1. **A `move ||` line-heuristic gives false positives** — rustfmt puts closure
   bodies on the next line. Use the span-matching script above; do not re-derive
   a grep.
2. **Not every `tmux::` hit is a convertible site.** This file has two that are
   structurally impossible (`Drop`) and one false friend
   (`wait_for_sudo_prompt_and_inject`). Wrapping a non-site is a defect.
3. **Do not insert an item between a doc comment and the item it documents.** This
   phase adds no items, but if you insert anything at item scope, read the lines
   directly above the insertion point first.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
