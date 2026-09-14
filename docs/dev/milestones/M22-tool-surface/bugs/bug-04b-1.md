# Bug 1 on phase-04b: a `#[allow(dead_code)]` shim and two new production `expect()`s

**Severity:** minor
**Status:** closed (fixed 2026-09-13, commit `dfe43b60`)
**Filed:** 2026-09-13

## What's wrong

Two Definition-of-Done boxes are unchecked, and the phase doc's own acceptance
criterion for the second **is recorded as failing in the phase's own end-to-end
transcript**:

```
prod unwrap/panic knowledge  (0): 2
```

### 1. A lint-silencing shim on genuinely dead code

`src/daemon/executor/knowledge/shell.rs:630-634`:

```rust
/// `watch_shell`'s default timeout, when `timeout_secs` is omitted. The
/// seeded `WatchShellArgs` default and the `ToolDef` description carry the
/// same number; changing this means changing both.
#[allow(dead_code)]
pub(super) const WATCH_DEFAULT_SECS: u64 = 300;
```

`grep -rn 'WATCH_DEFAULT_SECS' --include=*.rs src/` returns **one** line — the
definition itself. The constant has no reader: the default 300 is supplied by
`#[serde(default = "default_300")]` on `WatchShellArgs` and restated in the
`ToolDef` description. The `#[allow(dead_code)]` exists only to stop
`cargo clippy --all-targets --all-features -- -D warnings` reporting that.

`STANDARDS.md` § 2.2 says to delete an unused symbol rather than keep it
speculatively, and the DoD lists "No `#[allow(...)]`, `#[ignore]`, or
lint-silencing shims to mask diagnostics". This is the whole diff's only
`#[allow]`: `git diff fa10352a..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'`
→ 1.

### 2. Two new `expect()` calls in production paths

The module had **0** before this phase and has **2** now.

`src/daemon/executor/knowledge/shell.rs:780`:

```rust
            let matches = search_shell_buffer(&buf, re.as_ref().expect("pattern checked"), 1);
```

`src/daemon/executor/knowledge/shell.rs:938`:

```rust
        .expect("ANSI-strip regex is valid")
```

**These two are not the same kind of thing, and only the first is a real
fragility.** The invariant behind `:780` is *control flow* — `WatchOutcome::
Pattern` is returned only from inside `if re.as_ref().is_some_and(…)` at
`:747`. That holds today (verified at review; the tool does not panic), but it
is an invariant the type system is not carrying, so a later arm returning
`Pattern` from anywhere else turns a tool call into a daemon-side panic.

`:938` is the crate's established lazy-regex idiom over a `concat!` literal,
and three production sites already do exactly this — `src/daemon/utils/sudo.rs:36`
(`unwrap`), `src/tmux/ansi.rs:59` (`unwrap`), `src/ai/backends/gemini.rs:115`
(`expect`). **All three carry an `// INVARIANT: literal is a valid regex`
comment; this one does not**, and the module has no `INVARIANT` marker at all
(`grep -c 'INVARIANT' src/daemon/executor/knowledge/shell.rs` → 0).

## What should happen

- `WATCH_DEFAULT_SECS` is gone, and with it the `#[allow(dead_code)]`. Nothing
  in the phase needs the constant; the 300 already lives in `args.rs`'s
  `default_300` and in the `ToolDef` description, which is what the doc comment
  itself says.
- The `:780` `expect` no longer exists — the regex reaches the rendering site
  without a runtime assertion that it is `Some`. Any shape that makes the
  compiler carry the invariant is fine; the phase doc does not prescribe one.
- The `:938` site either loses its `expect` or joins the crate convention it is
  already following, with the same `// INVARIANT:` comment the other three
  carry. If you keep it, the production-`expect` count above must still reach
  **0**, so a kept `expect` has to stop matching that grep — say in the Update
  Log which route you took and why.

**This fix adds no behaviour and must add no tests.** The full lib suite must
still report **1861**, not 1862 — a rising count here means scope crept.

## Root cause

Both are the shape a phase takes on when its last edits are made against a tree
that will not compile. The previous run halted on the enum-variant wall
(`VerifierFailurePersistent`), so the resumed run's first green
`cargo clippy` came *after* all the code was written — at which point a dead
constant and an `Option` unwrap are cheapest to silence rather than to
restructure. Neither is a misunderstanding of the spec: the spec's criterion
pinned production `expect` at 0, the executor's own § D transcript prints `2`
against it, and the executor reported that honestly rather than massaging it.

## Definition of done

- [ ] `grep -c '#\[allow' src/daemon/executor/knowledge/shell.rs` → **0**
      (currently **1**).
- [ ] `grep -rn 'WATCH_DEFAULT_SECS' --include=*.rs src/ | wc -l` → **0**
      (currently **1**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/knowledge/shell.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **0** (currently **2**).
- [ ] If a lazy-regex `expect` is kept anywhere in the module, the module
      carries the crate's `// INVARIANT:` comment for it:
      `grep -c 'INVARIANT' src/daemon/executor/knowledge/shell.rs` → **1 or
      more** (currently **0**).
- [ ] `cargo test --lib 'knowledge::shell'` → **37**, `0 failed` — unchanged.
- [ ] `cargo test --lib` → **1861 passed, 0 failed** — **not 1862**. This fix
      adds no tests.
- [ ] `cargo test --test doc_truth` → **10**, unchanged.
- [ ] All four gates pass, `cargo clippy --all-targets --all-features -- -D
      warnings` with **no** new `#[allow]` anywhere in the diff:
      `git diff fa10352a..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- [ ] `watch_shell_returns_when_the_pattern_appears` and
      `watch_shell_ignores_output_written_before_the_call` still pass — the
      `:780` restructure must not change which outcome is produced.


## Resolution — 2026-09-13

Both items fixed, verified at review against the tree:

- `WATCH_DEFAULT_SECS` and its `#[allow(dead_code)]` are gone.
  `grep -rn 'WATCH_DEFAULT_SECS' --include=*.rs src/ | wc -l` → **0**;
  `grep -c '#\[allow' src/daemon/executor/knowledge/shell.rs` → **0**;
  `git diff fa10352a..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- The `:780` `expect("pattern checked")` is gone **by restructuring**, which is
  a better fix than the one this doc's constraint implied.
  `WatchOutcome::Pattern` now carries the rendered match as
  `Pattern(String)`, produced inside `if let Some(r) = re.as_ref() && …`, so
  the regex is bound where it is used and the compiler carries the invariant
  that the `expect` used to assert. `grep -c 'pattern checked'` → **0**.
- The `:938` lazy-regex `expect` was kept under route two and now carries
  `// INVARIANT: literal is a valid regex`, matching `src/daemon/utils/sudo.rs:36`,
  `src/tmux/ansi.rs:59` and `src/ai/backends/gemini.rs:115`.
  `grep -c 'INVARIANT'` → **1**.
- `cargo test --lib` → **1861**, not 1862. The fix added no tests, as pinned.

**One box in this doc was self-contradictory and is the architect's error.**
Box 3 pinned the module-head `unwrap|expect|panic|unsafe` count at **0** while
box 4 authorised *keeping* a marker-carrying lazy-regex `expect`. Both cannot
hold: the count's regex matches the kept `expect`. The executor took route
two, reached **1**, and **disclosed the divergence in its own Update Log
rather than deleting the expect to satisfy a number or reporting 0**. That is
the right call and the right disclosure.

The property the box was reaching for — *no production `expect` outside the
crate's compile-time-provable lazy-regex idiom, and that one carrying the
crate's marker* — **is met**. The count is corrected to **1** in the phase
doc's acceptance criteria.
