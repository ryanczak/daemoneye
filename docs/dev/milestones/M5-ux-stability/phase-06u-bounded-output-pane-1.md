# Phase 06u: `bounded_output` — Stage A Slice 3a, `src/tmux/pane.rs` (first 15)

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06t — `done`
**Estimated diff:** ~100 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Convert the **first 15 `.output()` call sites in `src/tmux/pane.rs`** — every one
from the top of the file through `select_pane` — to
`crate::tmux::bounded_output`.

`pane.rs` is the largest of the three `src/tmux/` files (30 `.output()` calls) and
is split across two phases. **This is 3a; `read_pane_exit_status` onward is 3b.**

**Finish condition: `src/tmux/pane.rs` has exactly 15 `.output()` calls left and
15 `bounded_output(` calls, and both fully-qualified sites converted correctly.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs` — `bounded_output`, `bounded_output_with`, `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "\.output()"      src/tmux/pane.rs                       # expect 30
grep -c "bounded_output(" src/tmux/pane.rs                       # expect 0
grep -c 'std::process::Command::new("tmux")' src/tmux/pane.rs    # expect 2
grep -c "tokio::process::Command" src/tmux/pane.rs               # expect 2
grep -c "bounded_output(" src/tmux/session.rs                    # expect 9
grep -c "bounded_output(" src/tmux/window.rs                     # expect 6
cargo test 2>&1 | grep "^test result" | head -3   # expect 921 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

## Current state

### The conversion, unchanged from slices 1 and 2

```
<builder>.output()   →   crate::tmux::bounded_output(<builder>)
```

`bounded_output` returns the **same** `std::io::Result<std::process::Output>`, so
**every surrounding expression stays exactly as it is** — the `?`, the `match`,
the `.ok()`, the `.map(…).unwrap_or(…)`. **There is no collapse. Do not add one.**

Worked example already landed in this tree (`src/tmux/session.rs`):

```rust
pub fn session_exists(name: &str) -> bool {
    crate::tmux::bounded_output(Command::new("tmux").args(["has-session", "-t", name]))
        .map(|o| o.status.success())
        .unwrap_or(false)
}
```

### ⚠ Hazard 1 — two sites are **fully qualified**, and a naive replace breaks them

`pane.rs` is the first file in stage A where this appears: `window.rs` and
`session.rs` had **zero** fully-qualified sites, so slices 1 and 2 never hit it.
**Both of these are in this slice.**

```rust
// src/tmux/pane.rs:257 — start_pipe_pane
    let out = std::process::Command::new("tmux")
        .args(["pipe-pane", "-O", "-t", pane_id, &cmd])
        .output()?;

// src/tmux/pane.rs:274 — stop_pipe_pane
    let _ = std::process::Command::new("tmux")
        .args(["pipe-pane", "-t", pane_id])
        .output();
```

Replacing the substring `Command::new("tmux")` produces
`std::process::crate::tmux::bounded_output(…)` — **`error[E0433]: `crate` in
paths can only be used in start position`**. That is a real compile error hit
while drafting.

**The wrapper goes around the *whole* expression, including the
`std::process::` prefix.** Post-`fmt` form, from the checked run:

```rust
    let out = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
        "pipe-pane",
        "-O",
        "-t",
        pane_id,
        &cmd,
    ]))?;
```

```rust
    let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
        "pipe-pane",
        "-t",
        pane_id,
    ]));
```

**Do not "tidy" these to the bare `Command::new`** — leave the `std::process::`
prefix where it is. `grep -c 'std::process::Command::new("tmux")'` must still
return **2** afterwards.

### ⚠ Hazard 2 — `wait_for` uses `tokio::process` and must NOT be touched

At the **bottom of the file** (outside this slice, but stated so you do not go
looking for a 30th conversion):

```rust
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
    let mut child = match tokio::process::Command::new("tmux")
        .args(["wait-for", channel])
        .spawn()
    { … };
    match tokio::time::timeout(timeout, child.wait()).await {
        …
            let _ = tokio::process::Command::new("tmux")
                .args(["wait-for", "-S", channel])
                .output()
                .await;
```

Two reasons it is not a target: it is **already async and already bounded** by
`tokio::time::timeout`, and `bounded_output` takes `&mut std::process::Command`,
so it would not compile anyway. **`pane.rs` therefore has 29 convertible sites,
not 30** — the file's final residue after slice 3b will be **1**, not 0.

### This slice's 15 sites

From the top of the file through `select_pane`. Line numbers are
current-as-of-drafting.

| Fn | Site(s) |
|---|---|
| `list_panes_detailed` | `:51` |
| `pane_dead_status` | `:123` |
| `capture_pane` | `:145` |
| `capture_pane_with_escapes` | `:170` |
| `capture_pane_at_scroll_with_escapes` | `:197` |
| `capture_pane_to_file` | `:215`, `:223`, `:232` |
| `start_pipe_pane` | `:259` — **fully qualified** |
| `stop_pipe_pane` | `:276` — **fully qualified** |
| `pane_current_command` | `:290` |
| `pane_pid` | `:305` |
| `query_pane_width` | `:320` |
| `resize_pane_width` | `:334` |
| `select_pane` | `:345` |

**The slice boundary is the doc comment `/// Read the last exit status recorded
by the shell hook`** (immediately above `read_pane_exit_status`). Everything from
there to the end of the file is slice 3b — **do not touch it.**

Worth knowing while you work: `capture_pane_at_scroll_with_escapes` (`:197`) is
the `capture-pane -S -` call that dumps the **entire scrollback**. It is the
reason `bounded_output` drains its pipes on threads, and it is in this slice.
Nothing special to do — just do not be surprised that a capture can be megabytes.

### ⚠ `cargo fmt` reflows these call sites heavily

Converting changes the expression's nesting depth, so `fmt` re-wraps the
`.args([…])` arrays. **Expected and correct.** Apply the substitution, run
`cargo fmt --all`, accept its output. Do not hand-format.

## Spec

1. **Convert the 15 `.output()` sites** from the top of `src/tmux/pane.rs`
   through `select_pane`, per the substitution above.
2. **Both fully-qualified sites keep their `std::process::` prefix**, wrapped
   whole — per Hazard 1.
3. **Stop at the slice boundary.** `read_pane_exit_status` onward is untouched.
4. **Run `cargo fmt --all`** — mandatory; this project has no `format_fix` hook.
5. `cargo build` after the file.

## Acceptance criteria

- [ ] `grep -c "\.output()" src/tmux/pane.rs` returns **15** (printed **30**
      before; 15 converted). **Not 0** — the rest of the file is slice 3b.
- [ ] `grep -c "bounded_output(" src/tmux/pane.rs` returns **15**.
- [ ] `grep -c 'std::process::Command::new("tmux")' src/tmux/pane.rs` returns
      **2** — **unchanged**. Both fully-qualified sites were wrapped, not
      rewritten to the bare form.
- [ ] `grep -c "tokio::process::Command" src/tmux/pane.rs` returns **2** —
      `wait_for` untouched.
- [ ] `grep -cF "std::process::crate::" src/tmux/pane.rs` returns **0** — the
      Hazard-1 malformation is absent.
- [ ] `grep -cF "pub fn read_pane_exit_status(pane_id: &str) -> Option<i32> {" src/tmux/pane.rs`
      returns **1** and the function still contains a `.output()` — the slice
      boundary held.
- [ ] `grep -c "bounded_output(" src/tmux/session.rs` returns **9** and
      `grep -c "bounded_output(" src/tmux/window.rs` returns **6** — both
      unchanged.
- [ ] `grep -cF "pub fn bounded_output_with(" src/tmux/mod.rs` returns **1** —
      the helper was not modified.
- [ ] `git diff --name-only | grep -c Cargo` returns **0**.
- [ ] `grep -cE '#\[allow|unsafe' src/tmux/pane.rs` returns **0**.
- [ ] `git diff --name-only -- src/` lists exactly **one** file:
      `src/tmux/pane.rs`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **921** lib-unit and **27** integration tests —
      **unchanged**. This phase adds no tests.

**Run every gate bare.** Every number above was produced by running that exact
command against a tree with this change applied.

## Test plan

`bounded_output` is covered by the five tests in `src/tmux/mod.rs`, including the
pipe-buffer regression test. **This phase adds no tests**: it changes which
function these 15 sites call, not what they compute, and every one needs a live
tmux server.

**The suite must stay at 921 lib tests.** If any test needs editing, **stop and
report a blocker**.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **The fully-qualified pair.** Quote `start_pipe_pane` as you left it and state
   in one sentence why wrapping only `Command::new("tmux")` — leaving
   `std::process::` outside the wrapper — does not compile.
2. **The residue.** State in one sentence why `.output()` should read **15** and
   not **0** at the end of this phase, and name the one call in this file that
   will never be converted.

## End-to-end verification

Not applicable — this phase ships no new runtime-loadable artifact. It redirects
15 existing call sites to a helper whose timeout behaviour is covered by its own
tests. **Do not attempt a live-tmux demonstration.**

## Authorizations

- [x] May edit `src/tmux/pane.rs` — **the 15 `.output()` sites from the top of
      the file through `select_pane` only.**
- [x] May let `cargo fmt --all` reflow those call sites.
- [ ] **No** change to any surrounding `match`, `?`, `.ok()`, or
      `.map(…).unwrap_or(…)`.
- [ ] **No** edit at or after `/// Read the last exit status recorded by the
      shell hook` — that is slice 3b.
- [ ] **No** change to `wait_for` or either `tokio::process::Command` call.
- [ ] **No** rewriting of the two `std::process::Command::new("tmux")` sites to
      the bare form.
- [ ] **No** change to `src/tmux/mod.rs`, `src/tmux/session.rs`, or
      `src/tmux/window.rs`.
- [ ] **No** new dependency, no new tests, no `#[allow(...)]`.
- [ ] **No** signature change to any function.

## Out of scope

- **`read_pane_exit_status` → end of file (14 sites)** — slice 3b.
- **`wait_for`** — already async and already bounded; never a target.
- **The `Drop` impls and `src/cli/`** — they call tmux directly rather than
  through `src/tmux/`; a later decision.

### ⚠ Traps

1. **The two fully-qualified sites.** Wrap the whole expression including
   `std::process::`; wrapping only `Command::new("tmux")` is `error[E0433]`.
   That error was hit for real while drafting this spec.
2. **Residue is 15, not 0.** Converting past the boundary is over-reach.
3. **`wait_for` is `tokio::process`** — not a target, and would not compile.
4. **No collapse.** The return type is unchanged; `?` stays `?`.
5. **Let `fmt` reflow** — do not hand-format, and run `cargo fmt --all`.
6. **The suite stays at 921.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-29 04:17 (started)

**Executor:** Claude (headless)

Converting the first 15 `.output()` call sites in `src/tmux/pane.rs` to `crate::tmux::bounded_output`.
