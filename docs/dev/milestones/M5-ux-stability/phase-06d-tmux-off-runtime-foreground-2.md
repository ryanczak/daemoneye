# Phase 06d: `foreground.rs` tmux Calls Off the Runtime — Slice 2 (poll & capture)

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-06c (slice 1) — `done`
**Estimated diff:** ~110 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to **10** more tmux calls in
`src/daemon/executor/foreground.rs` — the poll-and-capture stage of
`run_foreground`.

06c converted the first 10 (setup & send). **19 convertible sites remain**; this
phase takes 10, leaving 9 for 06e.

**Finish condition: the span-matching script reports `UNWRAPPED: 11` — the 9
slice-3 sites plus the 2 `Drop` sites — and every convertible one left is on
06e's list below.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime" src/daemon/executor/foreground.rs   # expect 10
cargo test 2>&1 | grep "^test result" | head -2           # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If a count differs, **stop and
report a blocker.**

## Current state

### ⭐ Three in-tree worked examples

`background/run.rs`, `background/respawn.rs`, and **`foreground.rs` itself** —
06c converted 10 sites in this very file. **Read those first**; they are the
closest possible analogue, same file, same function.

```rust
// value used, both failure modes collapse to a default
let target_str_cur = target_str.to_string();
let cur = tmux::off_runtime("pane-current-command", move || {
    tmux::pane_current_command(&target_str_cur)
})
.await
.and_then(|r| r.ok())
.unwrap_or_default();

// bool gate — a timeout must not read as "yes"
let tp_owned = tp.to_string();
let pane_exists = tmux::off_runtime("pane-exists", move || tmux::pane_exists(&tp_owned))
    .await
    .unwrap_or(false);
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes owned
before the closure**.

### ⚠ Scope is defined by SITE, not by line number

06c's review found that a site's expression can extend far past any line
boundary: the `send_keys` match at `:374` had its arms at `:890+`, so converting
it necessarily edited lines outside the nominal slice. **That was correct, not
scope creep.**

So this phase owns **the ten sites named below and whatever their expressions
require, wherever those extend.** It does not own any other site. Line numbers
shift as you edit — re-derive with the Acceptance-criteria script.

### This slice's 10 sites

All inside `async fn run_foreground`. Line numbers are current-as-of-drafting:

| Line | Call | Shape |
|---|---|---|
| 525 | `pane_pid` | B — `Result`, `.unwrap_or(0)` today |
| 567 | `select_pane` | A / B — follow what the site does today |
| 639 | `send_cancel` | A — `let _ =` |
| 660 | `unhighlight_pane` | A — **returns `()`**, see below |
| 691 | `capture_pane` | B |
| 698 | `capture_pane` | B |
| 715 | `capture_pane` | B |
| 737 | `capture_pane` | B |
| 751 | inline `Command` — `set-hook` (alert-silence) | D — `let _ =` |
| 760 | inline `Command` — `set-option` (monitor-silence) | D — `let _ =` |

### ⚠ `unhighlight_pane` returns `()`, not `Result`

```rust
// src/tmux/pane.rs:467
pub fn unhighlight_pane(pane_id: &str, restore_focus_to: Option<&str>) {
```

So `off_runtime` yields `Option<()>`, **not** `Option<Result<…>>`. Discard it
directly — **do not** write `.and_then(|r| r.ok())`, which will not compile:

```rust
let t = target_str.to_string();
let cp = chat_pane.map(|s| s.to_string());
let _ = tmux::off_runtime("unhighlight-pane", move || {
    tmux::unhighlight_pane(&t, cp.as_deref())
})
.await;
```

`highlight_pane` (converted in 06c) has the same signature; copy that site's
form. Note the `Option<&str>` argument must become an owned `Option<String>`
before the closure and be re-borrowed with `.as_deref()` inside.

### ⚠ The highlight/unhighlight pair spans slices — do not "fix" it

`highlight_pane` was converted in 06c. **This slice converts one
`unhighlight_pane` (`:660`); the other (`:831`) is 06e's.** That is fine: every
conversion is behaviour-preserving, so the pair stays balanced across the split.

An uncleared highlight leaves the user's pane tinted until they restart tmux, so
**the unhighlight must still run on exactly the paths it runs on today.** Convert
it as Shape A and add no early return that could skip it.

### ⚠ Three non-sites — unchanged from 06c

| Hit | Why not a site |
|---|---|
| `:74`, `:79` — `Command::new("tmux")` in `impl Drop for FgHookGuard` | `Drop::drop` cannot be `async` |
| `:23` — `use crate::tmux::cache::SessionCache;` | a type import |
| `wait_for_sudo_prompt_and_inject` | a local helper, not `tmux::wait_for` |

**No** `block_on`, `futures::executor`, or detached `tokio::spawn` as a
workaround for the `Drop` limit.

## Spec

### 1. Convert the 10 sites

Match each to a shape from 06c's conversions in this same file. Preserve each
site's existing failure default exactly — if it is `.unwrap_or(0)` today, the
timeout arm collapses to `0` too; do not invent a new sentinel.

### 2. Build after every site

Not a suggestion. `cargo build` after each converted site. 06c's predecessor run
died because one conversion's type error surfaced 470 lines away and could not be
localised. **If a converted form would change the type of a binding used later,
keep the binding's type identical** — collapse the `off_runtime` result at the
site rather than letting `Option<…>` leak downstream.

### 3. Convert nothing on 06e's list

These 9 stay untouched: `read_pane_exit_status` (×2), `pane_pid` (×3),
`unhighlight_pane` (`:831`), `capture_pane` (`:833`), `select_pane` (`:868`),
`pane_exists` (`:954`, in `run_background`).

## Acceptance criteria

- [ ] **Span-matching check reports `UNWRAPPED: 11`:**

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
for l, n in sorted(bad): print(f"   {l}: {n}")
PY
#   UNWRAPPED: 11
#   the 2 Drop sites (~74, ~79) + exactly these 9 names:
#     read_pane_exit_status x2, pane_pid x3, unhighlight_pane, capture_pane,
#     select_pane, pane_exists
```

      **Read the list and confirm the 9 convertible names match 06e's set
      exactly.** A different name means the wrong site was converted.

- [ ] `grep -c "off_runtime" src/daemon/executor/foreground.rs` returns **≥ 20**
      (10 from 06c + at least 10 here). The span check above is what proves the
      exact set; this is a floor, not an identity.
- [ ] `grep -c "and_then(|r| r.ok())" src/daemon/executor/foreground.rs` — every
      occurrence is on a site whose helper returns `Result`. **`unhighlight_pane`
      must not have one.** Verify by reading its converted site.
- [ ] The `impl Drop for FgHookGuard` block is **byte-identical**. Verify with
      `diff`, not by eye, and quote the result.
- [ ] `grep -c "block_on\|futures::executor" src/daemon/executor/foreground.rs`
      returns **0**. (Scoped to this file — six pre-existing hits elsewhere under
      `executor/knowledge/` are unrelated and must not be touched.)
- [ ] `grep -c "spawn_blocking" src/daemon/executor/foreground.rs` returns **0**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_foreground` needs a live tmux server and pane; it has **no unit coverage**.
Pre-existing gap, neither widened nor closed here.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. Do not claim any test guards these sites.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **`unhighlight_pane`.** Paste your converted site and show it has no
   `.and_then(|r| r.ok())`, explaining in one sentence why that would not
   compile.
2. **Failure defaults.** For `pane_pid` (`:525`) and one `capture_pane`, quote
   the before and after, showing the timeout arm collapses to the same default
   the original used.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/executor/foreground.rs` — **the ten named sites and
      whatever their expressions require.**
- [x] May add owned bindings at call sites.
- [ ] **No** edit to `impl Drop for FgHookGuard`.
- [ ] **No** conversion of any site on 06e's list.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/mod.rs` or any other file.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **06e's 9 sites** — the exit-status and cleanup stage.
- **The 2 `Drop` calls** — structurally unconvertible; the sync-helper stage.
- **`executor/knowledge/pane.rs`, `file_ops/`, `daemon/` core, `cli/`** — 06f–06h.

### ⚠ Traps

1. **Build after every site.** A type error 470 lines from its cause killed a
   previous run of this file.
2. **Use the span script, not a `move ||` line grep** — rustfmt puts closure
   bodies on the next line, which produces false positives.
3. **`unhighlight_pane` returns `()`** — `.and_then(|r| r.ok())` will not compile
   there.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
