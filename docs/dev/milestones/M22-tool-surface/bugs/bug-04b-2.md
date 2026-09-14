# Bug 2 on phase-04b: the pattern-match test's assertion does not discriminate

**Severity:** minor
**Status:** open
**Filed:** 2026-09-13

## What's wrong

`watch_shell_returns_when_the_pattern_appears` is one of the two load-bearing
positive tests for this phase's headline behaviour, and its second assertion
cannot fail for the reason it claims to check.

`src/daemon/executor/knowledge/shell.rs`, in that test:

```rust
        assert!(out.contains("READY"), "the matching line is shown: {out}");
```

The appended output line is `"READY to go\n"` and the watched pattern is
`Some("READY")`. The outcome sentence interpolates the **pattern** as well as
the rendered match:

```rust
                "watch_shell {shell}: matched {} after {} s. …\n{}",
                pattern.unwrap_or_default(),
                capped_secs,
                rendered_match
```

So `out.contains("READY")` is satisfied by the echoed pattern alone. The
assertion passes whether or not the matching line is rendered at all.

**Demonstrated at review.** Replacing the rendering with an empty string:

```rust
                let rendered = { let _ = matches; String::new() };
```

leaves the test **green** (`1 passed; 0 failed`). The phase's Test plan item 6
asks for two properties — *"the call returns naming the pattern **and showing
the matching line**"* — and only the first is actually tested.

This matters more after bug-04b-1's fix than before it: that fix **moved the
rendering** into the match branch (`WatchOutcome::Pattern(String)`), so this is
now the only test standing over the relocated code.

## What should happen

The assertion must fail when the rendered match is dropped. Assert on
something the echoed pattern cannot produce — the rest of the matched line.

## Root cause

The phase doc's acceptance criterion for this test pins only its **name**:

> `grep -cE '^    (async )?fn (… watch_shell_returns_when_the_pattern_appears …)'` → **10**

A name criterion cannot see an assertion's discriminating power, and the Test
plan's two-property sentence was never turned into a criterion. Architect-side:
the spec pinned that the test exists, not that it tests both properties.

## How to fix

Verified at review — this exact change was applied, run green against the
intact tree, and run **red** against the dropped-rendering mutation above:

```rust
        assert!(out.contains("READY to go"), "the matching line is shown: {out}");
```

`"READY to go"` is the full appended line; the pattern echo contributes only
`READY`, so the assertion now depends on the rendering.

Apply the same scrutiny to the other new `watch_shell` assertions while you are
here: any assertion whose substring also appears in the sentence's non-payload
parts (the pattern, the shell id, the elapsed seconds) is not testing what its
message claims. Fix any you find, and say in the Update Log which ones you
checked and which you changed.

## Definition of done

- [ ] `grep -c 'out.contains("READY"), "the matching line is shown' src/daemon/executor/knowledge/shell.rs`
      → **0** (currently **1**).
- [ ] The test asserts on the full matched line:
      `grep -c 'READY to go", "the matching line is shown' src/daemon/executor/knowledge/shell.rs`
      → **1** (currently **0**).
- [ ] **Mutation check, captured red-then-green with the `patch` tool.** Replace
      the rendering in `watch_shell_with` with an empty string
      (`let rendered = { let _ = matches; String::new() };`);
      `watch_shell_returns_when_the_pattern_appears` must go **red**. Revert and
      show it green. Quote the `test result:` line from the run whose command you
      are showing.
- [ ] `cargo test --lib 'knowledge::shell'` → **37**, `0 failed` — unchanged.
      This is an assertion change, not a new test.
- [ ] `cargo test --lib` → **1861 passed, 0 failed** — **not 1862**.
- [ ] All four gates pass.
