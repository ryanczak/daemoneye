# Phase 06c: `foreground.rs` tmux Calls Off the Runtime — Slice 1 (setup & send)

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06b — `done`
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to the **10** tmux calls in the first region of
`src/daemon/executor/foreground.rs` — **lines ≤ 460**, the setup-and-send stage
of `run_foreground`.

**This phase was re-scoped after a `hard_fail`.** The first attempt tried all 29
sites in this 1228-line file at once, converted 5, hit a type error whose symptom
surfaced 470 lines from its cause, and stalled re-reading the file for 60
consecutive turns. The file is now split into three slices of ~10 sites each —
the size that succeeded in the two preceding phases.

**Finish condition: the span-matching script reports `UNWRAPPED: 21` — 19
convertible sites in later slices, plus the 2 `Drop` sites — and every remaining
one is at line > 460 or inside `impl Drop`.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `src/tmux/mod.rs` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime" src/daemon/executor/foreground.rs   # expect 0
grep -c "off_runtime" src/daemon/background/respawn.rs    # expect 15
cargo test 2>&1 | grep "^test result" | head -2           # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

## Current state

### ⭐ Two in-tree worked examples — read them, do not invent

`background/run.rs` (06a) and `background/respawn.rs` (06b). The canonical forms:

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
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes owned
before the closure**. That is the per-site work.

### ⚠ The type error that stalled the first attempt

The first run converted `send_keys` (`:374`) as
`let result = match tmux::off_runtime("send-keys", …)` and produced a type
mismatch that the compiler reported **470 lines later**, at `:860`, where
`result` was consumed.

**Convert one site, then run `cargo build`, before moving to the next.** Ten
small builds cost seconds each; one big build at the end reports an error whose
cause you then have to find in a 1228-line function. That is exactly what
consumed the previous run.

If a site's converted form changes the type of a binding used later, **that is a
signal to keep the binding's type identical** — collapse the `off_runtime` result
back to whatever the original expression produced, at the site, rather than
letting `Option<Result<…>>` leak downstream.

### ⚠ Three non-sites — do not wrap them

| Hit | Why not a site |
|---|---|
| `:74`, `:79` — `Command::new("tmux")` in `impl Drop for FgHookGuard` | **`Drop::drop` cannot be `async`.** No `.await` in a destructor. Structurally impossible. |
| `:23` — `use crate::tmux::cache::SessionCache;` | a type import; spawns nothing |
| `:12`, `:563` — `wait_for_sudo_prompt_and_inject` | a local async helper, not `tmux::wait_for` |

**Do not** work around the `Drop` limit with `block_on`, `futures::executor`, or a
detached `tokio::spawn` — all are worse than the blocking call. Bounding those two
belongs to the later sync-helper stage.

### This slice's 10 sites

| Line | Call |
|---|---|
| 200 | `pane_exists` |
| 303 | `pane_pid` |
| 362 | inline `Command::new("tmux")` |
| 372 | `clear_pane_exit_status` |
| 374 | `send_keys` ← the one that broke the first attempt |
| 376 | `highlight_pane` |
| 420 | `pane_current_command` |
| 427 | `capture_pane` |
| 452 | `pane_current_command` |
| 457 | `capture_pane` |

Line numbers shift as you edit; re-derive with the Acceptance-criteria script.

## Spec

### 1. Convert the 10 sites at lines ≤ 460

Ordinary A/B/C shapes from `run.rs`/`respawn.rs`, except:

**`pane_exists` (`:200`) gates behaviour and returns `bool`.** A timeout must not
read as "the pane exists":

```rust
let p = target.to_string();
let exists = tmux::off_runtime("pane-exists", move || tmux::pane_exists(&p))
    .await
    .unwrap_or(false);
```

Treating a wedged tmux as "pane present" would send keys into a pane that may be
gone.

**`send_keys` (`:374`) is where the first attempt broke.** Keep the surrounding
binding's type exactly as it is today. Convert it, then `cargo build`, before
touching anything else.

**`highlight_pane` (`:376`) has its `unhighlight_pane` partners in later slices**
(`:602`, `:773`). Convert the highlight as Shape A (`let _ = …`) so it still runs
on exactly the paths it runs on today. **Do not touch the unhighlight calls** —
they are 06d's and 06e's. The pair stays balanced because every conversion is
behaviour-preserving; an early return that skipped one would break it.

### 2. Build after every site

Not a suggestion. `cargo build` after each converted site, so a type error is
attributed to the site that caused it.

### 3. Change nothing at line > 460, and nothing in `impl Drop`

## Acceptance criteria

- [ ] **Span-matching check reports `UNWRAPPED: 21`, and every one is either at
      line > 460 or inside `impl Drop`:**

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
PURE = {"off_runtime", "TMUX_TIMEOUT", "cache"}
bad = [(src[:m.start()].count("\n")+1, m.group(1))
       for m in re.finditer(r'\btmux::(\w+)', src)
       if m.group(1) not in PURE and not inside(m.start())]
bad += [(src[:m.start()].count("\n")+1, 'Command::new("tmux")')
        for m in re.finditer(r'Command::new\("tmux"\)', src) if not inside(m.start())]
print("UNWRAPPED:", len(bad))
early = [(l, n) for l, n in sorted(bad) if l < 100 or l <= 460]
print("of which at line <= 460 (must be ONLY the 2 Drop sites):")
for l, n in early: print(f"   {l}: {n}")
PY
#   UNWRAPPED: 21
#   of which at line <= 460 (must be ONLY the 2 Drop sites):
#      <~74>: Command::new("tmux")
#      <~79>: Command::new("tmux")
```

- [ ] `grep -c "off_runtime" src/daemon/executor/foreground.rs` returns **≥ 10**.
- [ ] The `impl Drop for FgHookGuard` block is **byte-identical** to its current
      form. Quote it in the Update Log.
- [ ] `grep -c "spawn_blocking" src/daemon/executor/foreground.rs` returns **0**,
      and `grep -rn "block_on\|futures::executor" src/daemon/executor/` returns
      **nothing**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file.
- [ ] `grep -c "off_runtime" src/daemon/background/respawn.rs` returns **15**,
      unchanged.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_foreground` needs a live tmux server and pane; it has **no unit coverage**
and cannot have any here. That is a pre-existing gap this phase neither widens nor
closes.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. Do not claim any test guards these sites.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **`pane_exists` on timeout.** Quote the converted site with its
   `.unwrap_or(false)` and say what would go wrong if a timeout read as `true`.
2. **The `Drop` block.** Paste it as it stands after your changes and state why
   `off_runtime` cannot be used there.

## End-to-end verification

None required. 06a already demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/executor/foreground.rs` — **lines ≤ 460 only**.
- [x] May add owned bindings at call sites.
- [ ] **No** edit to `impl Drop for FgHookGuard`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits at line > 460 — those are 06d and 06e.
- [ ] **No** edits to `src/tmux/mod.rs` or any other file.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **Lines 461–710 (10 sites)** — phase **06d**.
- **Lines > 710 (9 sites)** — phase **06e**.
- **The 2 `Drop` calls** — structurally unconvertible; the sync-helper stage.
- **`executor/knowledge/pane.rs`, `file_ops/`, `daemon/` core, `cli/`** — 06f–06h.

### ⚠ Traps

1. **Build after every site.** The previous attempt's type error surfaced 470
   lines from its cause and cost the run.
2. **A `move ||` line-heuristic gives false positives** — rustfmt puts closure
   bodies on the next line. Use the span script.
3. **Not every `tmux::` hit is a site** — see the three non-sites above.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 (escalation)

**Chosen lever:** re-split (not resume, not takeover)
**Rationale:** the first attempt's `hard_fail` was a scope problem, not a spec
gap — 29 sites in a 1228-line file, 5 converted, then 60 read-only turns chasing
a type error reported 470 lines from its cause; resuming into the same scope
would hit the same wall, and takeover would forfeit telemetry on a phase whose
only defect is size.

The partial work (5 sites, **non-compiling**) was stashed as
`stash@{0}` — "06c partial: 5/29 sites, broken build (type error at :860 from
:389)" — and the tree returned to a green baseline (916 tests) rather than
handing the next run a broken file to interpret. Those 5 sites are re-done as
part of this slice; redoing them mechanically is cheaper than resuming from a
broken state.

`foreground.rs` is now three phases of ~10 sites — the size that landed
`approved_first_try` in 06a (16) and 06b (11).

### Update — 2026-07-27 17:27 (started)

**Executor:** Claude (Sonnet 4.5)
