# Phase 06t: `bounded_output` — Stage A Slice 2, `src/tmux/session.rs`

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06s — `done` (introduced `bounded_output`)
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Convert the **9 `.output()` call sites in `src/tmux/session.rs`** to
`crate::tmux::bounded_output`, so every one is bounded by `TMUX_TIMEOUT` instead
of hanging indefinitely on a wedged tmux server.

**Finish condition: `src/tmux/session.rs` has zero `.output()` calls and nine
`bounded_output(` calls, and every surrounding error-handling expression is
byte-identical to what it is today.**

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
grep -c "\.output()"      src/tmux/session.rs   # expect 9
grep -c "bounded_output(" src/tmux/session.rs   # expect 0
grep -c "\.output()"      src/tmux/window.rs    # expect 0
grep -c "\.output()"      src/tmux/pane.rs      # expect 30
cargo test 2>&1 | grep "^test result" | head -3   # expect 921 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

`window.rs` is already **0** (slice 1 converted it) and `pane.rs` stays at **30**
(slice 3). Both are pinned so this phase can prove it stayed in its lane.

## Current state

### The conversion is a pure substitution — the type does not change

`bounded_output` returns the **same** `std::io::Result<std::process::Output>` that
`.output()` returns. So the transformation is:

```
<builder>.output()   →   crate::tmux::bounded_output(<builder>)
```

and **every surrounding expression stays exactly as it is** — the `?`, the
`match`, the `.ok()?`, the `.map(…).unwrap_or(…)`. This is unlike the
`off_runtime` conversions elsewhere in this milestone, which changed
`Result<T>` into `Option<Result<T>>` and needed a collapse at each site. **Here
there is no collapse. Do not add one.**

Worked example, already landed in this tree by slice 1
(`src/tmux/window.rs:106`):

```rust
// before
    let output = Command::new("tmux")
        .args(["rename-window", "-t", &target, new_name])
        .output()?;

// after
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "rename-window",
        "-t",
        &target,
        new_name,
    ]))?;
```

The `?` is untouched; only the terminator moved.

### The 9 sites carry **five** different surrounding shapes

Line numbers are current-as-of-drafting; re-derive before editing. All nine take
the identical substitution — the table is here so you can confirm you have not
disturbed the handling, **not** because any of them needs different treatment.

| Site | Enclosing fn | Surrounding shape | On timeout |
|---|---|---|---|
| `:24` | `list_sessions` | `match … { Ok(o) => o, Err(_) => return Vec::new() }` | empty list |
| `:63` | `list_session_flags` | `match … { Ok(o) if success => o, _ => return HashMap::new() }` | empty map |
| `:206` | `session_environment` | `?` | `Err` propagates |
| `:234` | `get_active_pane` | `?` | `Err` propagates |
| `:247` | `current_session_name` | `.ok()?` | `None` |
| `:261` | `client_dimensions` | `match … { Ok(o) if success => o, _ => return (0, 0) }` | `(0, 0)` |
| `:312` | `ensure_incident_session` | `?` | `Err` propagates |
| `:326` | `session_exists` | `.map(\|o\| o.status.success()).unwrap_or(false)` | `false` |
| `:335` | `list_pane_ids_in_session` | `?` | `Err` propagates |

**Every one of those timeout outcomes is what the site already produces when the
tmux call fails today** — `.output()` already returns `Err` on spawn failure, and
each shape already handles it. A timeout is just one more `Err`. So this
conversion is behaviour-preserving; it only replaces "hang forever" with "fail
the way this site already fails".

Two worth stating explicitly because they look risky and are not:

- **`session_exists` → `false` on timeout.** The daemon then tries to create a
  session that may already exist; `tmux new-session -d -s <name>` fails on a
  duplicate, and the caller's existing error arm handles that. Same as any tmux
  failure today.
- **`client_dimensions` → `(0, 0)` on timeout.** Its callers already guard with
  `if w > 0 && h > 0`.

### ⚠ `cargo fmt` reflows these call sites heavily

Converting changes the expression's nesting depth, so `fmt` re-wraps the
`.args([…])` arrays — a one-line array may explode to one element per line, and a
`let` may gain a line break. **This is expected and correct.** Apply the
substitution, then run `cargo fmt --all` and accept its output. Do not
hand-format, and do not treat the reflow as a mistake.

Two post-`fmt` examples from the checked run, showing that the handling survives
verbatim:

```rust
    let out = match crate::tmux::bounded_output(Command::new("tmux").args([
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_windows}\t#{session_activity}\t#{session_attached}",
    ])) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
```

```rust
pub fn current_session_name() -> Option<String> {
    let out =
        crate::tmux::bounded_output(Command::new("tmux").args(["display-message", "-p", "#S"]))
            .ok()?;
```

## Spec

1. **Convert all 9 `.output()` sites** in `src/tmux/session.rs` per the
   substitution above. Change nothing else in the file.
2. **Run `cargo fmt --all`** — mandatory; this project has no `format_fix` hook.
3. `cargo build` after the file.

## Acceptance criteria

- [ ] `grep -c "\.output()" src/tmux/session.rs` returns **0** (printed **9**
      before).
- [ ] `grep -c "bounded_output(" src/tmux/session.rs` returns **9**.
- [ ] `grep -c "\.output()" src/tmux/pane.rs` returns **30** — **unchanged**.
      It is slice 3; a lower number means this phase over-reached.
- [ ] `grep -c "\.output()" src/tmux/window.rs` returns **0** — unchanged from
      slice 1.
- [ ] `grep -cF "pub fn bounded_output_with(" src/tmux/mod.rs` returns **1** and
      `grep -cF "pub fn bounded_output(" src/tmux/mod.rs` returns **1** — the
      helper was not modified.
- [ ] **All five surrounding shapes survive verbatim.** Each of these returns
      **1**:

```bash
grep -cF "Err(_) => return Vec::new()," src/tmux/session.rs
grep -cF "_ => return HashMap::new()," src/tmux/session.rs
grep -cF ".ok()?;" src/tmux/session.rs
grep -cF "_ => return (0, 0)," src/tmux/session.rs
grep -cF ".map(|o| o.status.success())" src/tmux/session.rs
```

- [ ] `git diff --name-only | grep -c Cargo` returns **0** — no dependency
      change.
- [ ] `grep -cE '#\[allow|unsafe' src/tmux/session.rs` returns **0**.
- [ ] `git diff --name-only -- src/` lists exactly **one** file:
      `src/tmux/session.rs`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **921** lib-unit and **27** integration tests —
      **unchanged**. This phase adds no tests.

**Run every gate bare.** Every number above was produced by running that exact
command against a tree with this change applied.

## Test plan

`bounded_output` itself is covered by the five tests slice 1 landed
(`src/tmux/mod.rs`), including the pipe-buffer regression test. **This phase adds
no tests**: it changes which function these 9 sites call, not what they compute,
and every one needs a live tmux server to exercise.

**The suite must stay at 921 lib tests.** If any test needs editing, **stop and
report a blocker** — it would mean a signature or behaviour changed, which this
phase forbids.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **Why no collapse.** Quote one converted site with its surrounding `match` or
   `?`, and state in one sentence why the error handling did not need to change.
2. **The riskiest-looking timeout.** Quote `session_exists` as you left it and
   state in one sentence what a timeout makes it return, and why that is the same
   thing it already does when tmux fails.

## End-to-end verification

Not applicable — this phase ships no new runtime-loadable artifact. It redirects
9 existing call sites to a helper whose timeout behaviour was demonstrated by its
own tests in slice 1. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/tmux/session.rs` — **the 9 `.output()` call sites only.**
- [x] May let `cargo fmt --all` reflow those call sites.
- [ ] **No** change to any surrounding `match`, `?`, `.ok()?`, or
      `.map(…).unwrap_or(…)`.
- [ ] **No** change to `src/tmux/mod.rs` — the helper is finished.
- [ ] **No** edits to `src/tmux/pane.rs` (slice 3) or `src/tmux/window.rs`
      (slice 1, done).
- [ ] **No** new dependency, no new tests, no `#[allow(...)]`.
- [ ] **No** signature change to any function in `session.rs`.

## Out of scope

- **`src/tmux/pane.rs` (30 sites)** — slice 3, and it will need splitting.
- **`src/tmux/cache.rs`** — it holds no direct `Command::new("tmux")` calls.
- **The `Drop` impls and `src/cli/`** — they call tmux directly rather than
  through `src/tmux/`; bounding them is a later decision, not this slice.

### ⚠ Traps

1. **No collapse.** The return type is unchanged, so `?` stays `?` and every
   `match` arm stays as written. Adding `.ok()`, `.flatten()` or an extra arm is
   wrong here.
2. **`pane.rs` stays at 30.** Converting it here is over-reach.
3. **Let `fmt` reflow** — do not hand-format, and run `cargo fmt --all` before
   finishing.
4. **The suite stays at 921.** No new tests.
5. **`session_exists` keeps `.unwrap_or(false)`** — do not "improve" it to
   `true`; false is what it already returns when tmux fails.
6. **Only the terminator moves.** `X.output()` → `bounded_output(X)`, nothing
   else on the line changes.

## Update Log

### Update — 2026-07-29 01:55 (started)

**Executor:** Claude (Sonnet 4.5)
**Action:** Converting all 9 `.output()` call sites in `src/tmux/session.rs` to `crate::tmux::bounded_output`.

<!-- entries appended below this line -->
