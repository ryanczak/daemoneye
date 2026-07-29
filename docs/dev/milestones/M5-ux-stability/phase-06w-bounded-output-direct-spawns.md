# Phase 06w: `bounded_output` — the direct spawns that bypass `src/tmux/`

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-06v — `done`
**Estimated diff:** ~70 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Bound the **9 raw `std::process::Command::new("tmux")` spawns that never call a
`src/tmux/` helper** — the two `Drop` impls and six sites in `src/cli/` — by
calling `crate::tmux::bounded_output` directly.

**This closes the milestone's fifth exit criterion.** Stage A (06s–06v) bounded
the 44 helper spawns; these nine are the ones no helper-side timeout can reach.

**Finish condition: 9 new `bounded_output(` calls across 5 files, and the only
raw tmux spawn left in `src/cli/` is the `.exec()` site** — see Hazard 2.

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
grep -c "bounded_output(" src/cli/local_cmds.rs                        # expect 0
grep -c "bounded_output(" src/cli/commands/pane.rs                     # expect 0
grep -c "bounded_output(" src/cli/commands/chat.rs                     # expect 0
grep -c "bounded_output(" src/daemon/executor/foreground.rs            # expect 0
grep -c "bounded_output(" src/daemon/executor/knowledge/pane.rs        # expect 0
grep -c 'Command::new("tmux")' src/daemon/executor/foreground.rs       # expect 5
grep -c 'Command::new("tmux")' src/daemon/executor/knowledge/pane.rs   # expect 3
grep -c "off_runtime(" src/daemon/executor/foreground.rs               # expect 30
grep -c "off_runtime(" src/daemon/executor/knowledge/pane.rs           # expect 7
cargo test 2>&1 | grep "^test result" | head -3   # expect 921 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

## Current state

### The conversion, unchanged from 06s–06v

```
<builder>.output()   →   crate::tmux::bounded_output(<builder>)
```

`bounded_output` returns the **same** `std::io::Result<std::process::Output>`, so
**every surrounding expression stays exactly as it is**. **There is no collapse.
Do not add one.**

The `std::process::` prefix stays **inside** the wrapper — the whole expression is
wrapped, prefix and all. Worked example already landed in this tree
(`src/tmux/pane.rs`, from 06u):

```rust
    let out = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
        "pipe-pane",
        "-O",
        "-t",
        pane_id,
        &cmd,
    ]))?;
```

Wrapping only `Command::new("tmux")` and leaving `std::process::` outside gives
`std::process::crate::tmux::bounded_output(…)` — **`error[E0433]: `crate` in
paths can only be used in start position`**. **Every site in this phase is
fully qualified**, so this hazard applies to all nine.

### Why these nine and not the other 26

`grep -rn 'Command::new("tmux")' src/ | grep -v '^src/tmux/'` reports **36** raw
spawns. **27 of them are already bounded** and are **not** in scope:

- **26 sit inside a `tmux::off_runtime(…)` closure** or in a helper body whose
  every call site is wrapped — the daemon's async surface, finished by 06r. The
  milestone README's fourth exit criterion records the verification.
- **1 is `.exec()`** — Hazard 2.

The nine in scope are the ones wrapped at **no** level, because they never route
through `src/tmux/` and their enclosing code cannot be made `async`.

### ⚠ Hazard 1 — three files contain BOTH in-scope and out-of-scope sites

This is the trap that makes this phase not a blind sweep. **A whole-file
find-and-replace converts already-bounded code.**

`src/daemon/executor/foreground.rs` has **5** raw spawns — only the **2 inside
`impl Drop for FgHookGuard`** are targets:

```rust
// IN SCOPE — src/daemon/executor/foreground.rs, impl Drop for FgHookGuard
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

```rust
// OUT OF SCOPE — already inside off_runtime. DO NOT TOUCH.
    let _ = tmux::off_runtime("set-hook", move || {
        std::process::Command::new("tmux")
            .args([…])
            .output()
    })
    .await;
```

`src/daemon/executor/knowledge/pane.rs` has **3** raw spawns — only the **1
inside `impl Drop for WatchHookGuard`** is a target. The other two:

- one in `watch_pane`'s **prologue** — `watch_pane` is wrapped in `off_runtime`
  at its call site in `executor/mod.rs`, so it is already bounded;
- one inside an `off_runtime` closure.

**Both are out of scope. Do not touch either.**

`src/cli/commands/chat.rs` has **4** raw spawns — **3** are targets; the fourth
is Hazard 2.

**The reliable discriminator is the terminator plus the enclosing context**, not
the file. Convert a site only when it (a) ends in `.output()` and (b) is *not*
lexically inside a `tmux::off_runtime(…)` closure.

### ⚠ Hazard 2 — the `.exec()` site is NOT a target and never will be

```rust
// src/cli/commands/chat.rs — DO NOT CONVERT
                let err = std::process::Command::new("tmux")
                    .args(["attach-session", "-t", sname])
                    .exec();
                // exec() only returns on error.
```

`CommandExt::exec()` **replaces the current process image**. It does not return
on success, there is no child to time out, and it returns `std::io::Error`, not
`Output` — `bounded_output` would not compile against it. It is the one raw tmux
spawn that must remain in `src/cli/` after this phase.

**There is a second non-`.output()` tmux spawn in the tree**, at
`src/daemon/background/respawn.rs`: a `.status()` call already inside
`off_runtime`. Not a target either — `bounded_output` returns `Output`, not
`ExitStatus`. **Stated so you do not go hunting for a tenth site.**

### The 9 sites

Line numbers are current-as-of-drafting; re-derive with
`grep -n 'Command::new("tmux")' <file>`.

| File | Site | Enclosing | Shape |
|---|---|---|---|
| `daemon/executor/foreground.rs` | `:73` | `impl Drop for FgHookGuard` | `let _ = …;` |
| `daemon/executor/foreground.rs` | `:78` | `impl Drop for FgHookGuard` | `let _ = …;` |
| `daemon/executor/knowledge/pane.rs` | `:182` | `impl Drop for WatchHookGuard` | `let _ = …;` |
| `cli/local_cmds.rs` | `:203` | `list_de_windows` | `let output = …;` then `match output` |
| `cli/commands/pane.rs` | `:107` | the `"" \| "s"` match arm | `.output().ok()?` |
| `cli/commands/pane.rs` | `:143` | `pick_sibling_pane` | `.output().map(…).unwrap_or_default()` |
| `cli/commands/chat.rs` | `:57` | `run_chat_inner` | `let _ = …;` |
| `cli/commands/chat.rs` | `:145` | `run_chat_inner` | `let _ = …;` |
| `cli/commands/chat.rs` | `:268` | `run_chat_ratatui` attach loop | `.output().ok().and_then(…).unwrap_or(1)` |

**The shapes are listed only so you can confirm you disturbed nothing.** The
substitution is type-preserving; all of them stay byte-identical. **Adding a
collapse is the first trap.**

### Why the `Drop` sites matter more than the CLI ones

The three `Drop` sites are the load-bearing half. `FgHookGuard` and
`WatchHookGuard` are dropped **on tokio worker threads** — `Drop::drop` cannot be
`async`, so `off_runtime` structurally cannot reach them, and a wedged tmux
server blocks a worker for as long as it stays wedged. That is mechanism B, in
the one place the rest of this milestone could not fix.

The six `src/cli/` sites are the cheaper half: `src/cli/` has **no concurrency**
— no `tokio::spawn`, no threads — so a blocking call there stalls only the
process that made it. What they gain is the 5 s bound, so a wedged tmux server
means `daemoneye chat` reports a failure instead of hanging the user's terminal
forever.

A timeout at each site produces what that site already produces when tmux fails,
because `.output()` already returns `Err` on spawn failure: the `let _ =` sites →
nothing (best-effort by design), `local_cmds.rs:203` → its existing `Err` match
arm, `pane.rs:107` → `None`, `pane.rs:143` → `String::default()`, `chat.rs:268` →
`1` (which breaks the attach loop — the same thing it does today when the call
fails).

### ⚠ `cargo fmt` reflows these call sites heavily

Converting changes the expression's nesting depth, so `fmt` re-wraps the
`.args([…])` arrays and sometimes de-indents a trailing chain. **Expected and
correct.** Apply the substitution, run `cargo fmt --all`, accept its output.
**Do not hand-format.**

Two post-`fmt` forms from the checked run:

```rust
// foreground.rs — impl Drop for FgHookGuard
            let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
                "set-hook",
                "-u",
                "-t",
                &self.target,
                hook,
            ]));
```

```rust
// chat.rs:268 — the chain de-indents to the statement's own level
        let attached = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
            "display-message",
            "-p",
            "#{session_attached}",
        ]))
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1);
```

The second reads wrong at a glance — the `.ok()`/`.and_then` lines drop back to
the statement's indent level. **That is `rustfmt`'s output, not a mistake. Leave
it.**

## Spec

1. **Convert the two `Drop` sites in `src/daemon/executor/foreground.rs`** —
   inside `impl Drop for FgHookGuard` only. The other three raw spawns in that
   file are already inside `off_runtime`; leave them.
2. **Convert the one `Drop` site in `src/daemon/executor/knowledge/pane.rs`** —
   inside `impl Drop for WatchHookGuard` only. The other two are already bounded;
   leave them.
3. **Convert the one site in `src/cli/local_cmds.rs`.**
4. **Convert the two sites in `src/cli/commands/pane.rs`.**
5. **Convert the three `.output()` sites in `src/cli/commands/chat.rs`.** Leave
   the `.exec()` site exactly as it is.
6. **Run `cargo fmt --all`** — mandatory; this project has no `format_fix` hook.
7. `cargo build` after each file.

## Acceptance criteria

- [ ] `grep -c "bounded_output(" src/daemon/executor/foreground.rs` returns **2**.
- [ ] `grep -c "bounded_output(" src/daemon/executor/knowledge/pane.rs` returns
      **1**.
- [ ] `grep -c "bounded_output(" src/cli/local_cmds.rs` returns **1**.
- [ ] `grep -c "bounded_output(" src/cli/commands/pane.rs` returns **2**.
- [ ] `grep -c "bounded_output(" src/cli/commands/chat.rs` returns **3**.
- [ ] `sed -n '/^impl Drop for FgHookGuard/,/^}/p' src/daemon/executor/foreground.rs | grep -c "\.output()"`
      returns **0** — both `Drop` sites converted.
- [ ] `sed -n '/^impl Drop for WatchHookGuard/,/^}/p' src/daemon/executor/knowledge/pane.rs | grep -c "\.output()"`
      returns **0**.
- [ ] `grep -rn 'Command::new("tmux")' src/cli/ | grep -v bounded_output | wc -l`
      returns **1**, and that line is the `.exec()` site in `commands/chat.rs`.
      *(This is the criterion that closes the fifth exit criterion.)*
- [ ] `grep -c "off_runtime(" src/daemon/executor/foreground.rs` returns **30**
      and `grep -c "off_runtime(" src/daemon/executor/knowledge/pane.rs` returns
      **7** — both **unchanged**. No already-bounded site was rewritten.
- [ ] `grep -c 'Command::new("tmux")' src/daemon/executor/foreground.rs` returns
      **5** and `… src/daemon/executor/knowledge/pane.rs` returns **3** — both
      **unchanged**. This phase wraps calls; it neither adds nor removes one.
- [ ] `grep -rcF "std::process::crate::" src/ | grep -v ":0" | wc -l` returns
      **0** — the `E0433` malformation is absent everywhere.
- [ ] `grep -c "\.output()" src/tmux/pane.rs` returns **1** and
      `grep -c "bounded_output(" src/tmux/pane.rs` returns **29** — `src/tmux/`
      untouched by this phase.
- [ ] `git diff --name-only -- src/` lists exactly **five** files: the three
      `src/cli/` files and the two `src/daemon/executor/` files.
- [ ] `git diff --name-only | grep -c Cargo` returns **0**.
- [ ] `git diff -U0 -- src/ | grep '^+' | grep -cE 'unwrap\(\)|expect\(|panic!'`
      returns **0** — no collapse smuggled in via a new unwrap.
- [ ] `git diff -U0 -- src/ | grep '^+' | grep -cE '\basync\b|\.await'` returns
      **0** — this phase adds no async. `Drop::drop` cannot be `async`; that is
      the whole reason this phase exists.
- [ ] `grep -rcE '#\[allow|unsafe' src/cli/local_cmds.rs src/cli/commands/pane.rs src/cli/commands/chat.rs`
      shows **0** for each.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **921** lib-unit and **27** integration tests —
      **unchanged**. This phase adds no tests.

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status. Every number above was produced by running that exact command against a
tree with this change applied.

## Test plan

`bounded_output` is covered by the five tests in `src/tmux/mod.rs`, including the
1 MiB pipe-buffer regression test and the timeout-kills-the-child test. **This
phase adds no tests**: it changes which function these 9 sites call, not what
they compute, and every one needs a live tmux server. The two `Drop` sites
additionally need a live tmux *pane* with a hook installed.

**The suite must stay at 921 lib tests.** If any test needs editing, **stop and
report a blocker**.

Three reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **The `.exec()` carve-out.** Quote the site you left alone and state in one
   sentence why `bounded_output` cannot be applied to it.
2. **The in-file discrimination.** `src/daemon/executor/foreground.rs` has five
   raw tmux spawns and you converted two. Quote one you converted and one you
   did not, and state in one sentence what distinguishes them.
3. **No collapse.** Quote `src/cli/commands/pane.rs`'s `pick_sibling_pane` site
   as you left it and state in one sentence why
   `.map(…).unwrap_or_default()` needed no change.

## End-to-end verification

Not applicable — this phase ships no new runtime-loadable artifact. It redirects
9 existing call sites to a helper whose timeout behaviour is covered by its own
tests. **Do not attempt a live-tmux demonstration**, and in particular do not try
to exercise a `Drop` impl by hand.

## Authorizations

- [x] May edit `src/daemon/executor/foreground.rs` — **inside `impl Drop for
      FgHookGuard` only.**
- [x] May edit `src/daemon/executor/knowledge/pane.rs` — **inside `impl Drop for
      WatchHookGuard` only.**
- [x] May edit `src/cli/local_cmds.rs`, `src/cli/commands/pane.rs`,
      `src/cli/commands/chat.rs` — **the `.output()` sites only.**
- [x] May let `cargo fmt --all` reflow those call sites.
- [ ] **No** change to any surrounding `?`, `.ok()?`, `let _ =`, `match`, or
      `.map(…).unwrap_or…`.
- [ ] **No** change to any site already inside a `tmux::off_runtime(…)` closure.
- [ ] **No** change to the `.exec()` site.
- [ ] **No** change to `src/tmux/` — stage A is finished.
- [ ] **No** new dependency, no new tests, no `#[allow(...)]`.
- [ ] **No** signature change to any function, and **no** `async` added anywhere.

## Out of scope

- **The 26 raw spawns already inside `off_runtime`** or in helper bodies whose
  call sites are wrapped — the daemon's async surface, finished by 06r.
- **`src/cli/commands/chat.rs`'s `.exec()` site** — never a target.
- **`src/daemon/background/respawn.rs`'s `.status()` site** — already inside
  `off_runtime`, and returns `ExitStatus`, not `Output`.
- **`src/tmux/`** — all 44 helper spawns bounded by 06s–06v.
- **Changing the timeout.** Every site uses `bounded_output` at the standard
  5 s `TMUX_TIMEOUT`. Do not reach for `bounded_output_with`.

### ⚠ Traps

1. **Three of the five files contain out-of-scope sites.** A whole-file
   find-and-replace converts already-bounded `off_runtime` code. Discriminate by
   enclosing context, not by file.
2. **The `.exec()` site.** Not `.output()`, not convertible, must survive.
3. **Every site is fully qualified.** Wrap the whole expression including
   `std::process::`; wrapping only `Command::new("tmux")` is `error[E0433]`.
4. **No collapse.** The return type is unchanged; `?` stays `?`, `.ok()?` stays
   `.ok()?`, the `match` stays a `match`.
5. **No `async`.** `Drop::drop` cannot be `async` — that is why these sites need
   a synchronous timeout rather than `off_runtime`.
6. **Let `fmt` reflow** — run `cargo fmt --all`; do not hand-format.
7. **The suite stays at 921.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
