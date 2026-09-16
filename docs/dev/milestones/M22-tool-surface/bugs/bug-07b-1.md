# Bug 1 on phase-07b: the ghost test duplicates another test and its message says the opposite of its assertion

**Severity:** minor
**Status:** open
**Filed:** 2026-09-15

## What's wrong

`ghost_tool_call_preserves_its_execution_mode` (`src/daemon/stream.rs`) was
specified to guard the phase's one silent-failure trap: that the
`background` → `wait` inversion was not applied backwards in the ghost loop.
It does not guard that, and it says something false while not guarding it.

```rust
        let AiEvent::ToolCall(_, _, ghost_wait, ..) = ghost_event else {
            panic!("test only constructs ToolCall")
        };
        assert!(
            !routes_to_background(ghost_wait),
            "the ghost loop's own ToolCall events must keep taking the background path"
        );
```

**Three separate problems:**

1. **The assertion contradicts its own message.** It asserts
   `!routes_to_background(...)` — that the event routes to the **foreground** —
   while the message says it "must keep taking the **background** path". A
   future reader trusting the message will read the test backwards.
2. **It is a duplicate of `omitting_wait_runs_in_the_foreground`.** The
   constructed event's `wait` is `None`, so after the destructuring theatre the
   call is `routes_to_background(None)` — byte-for-byte what test 1 already
   asserts. Demonstrated at review: mutating the routing to background
   everything reddens **both**, together, for the same reason:

   ```
   test daemon::stream::tests::omitting_wait_runs_in_the_foreground ... FAILED
   test daemon::stream::tests::ghost_tool_call_preserves_its_execution_mode ... FAILED
   test daemon::stream::tests::wait_true_runs_in_the_foreground ... FAILED
   ```
3. **Ghost routing does not go through `routes_to_background` at all**, so no
   assertion about that function can guard it. `src/daemon/ghost.rs` binds
   position 3 as `_wait` — deliberately unused — and pushes
   `PendingCall::Background` **unconditionally**. The executor's own Update Log
   says exactly this. Verified at review by trying to mutate ghost's real
   routing: swapping its unconditional `Background` for `Foreground` **does not
   even compile**, because the two variants have different fields
   (`_credential`/`retry_pane` versus `target`/`shell`/`timeout_secs`). The
   paths are structurally separate.

**No production defect.** Ghost behaviour is genuinely unchanged — unconditional
`Background` before and after, with the received value ignored both times. The
phase's code is right; the test that was supposed to prove it is not.

## What should happen

Either the test guards the property its name claims, or it goes away and the
reason is recorded. Both are acceptable; what is not acceptable is a test whose
message states the opposite of its assertion.

The property worth pinning, if you keep a test: **the ghost loop takes the
background path regardless of the `wait` value it receives** — that is what
makes the inversion safe, and it is true by construction today.

## Root cause

Architect-side in part. The Test plan said *"Assert on the **variant**, not on
the field value: the point is that the inversion was not applied backwards, and
a field assertion would pass against a double inversion."* It did not say that
ghost's routing is unconditional and therefore unreachable from
`routes_to_background`, because I had not measured that — the § Pre-flight
note only told the executor to read ghost's value before editing. Given a
pure routing function and an instruction to assert on routing, asserting on
`routes_to_background` was the available move.

## Definition of done

- [ ] **The message and the assertion agree.** Whatever the test asserts, its
      failure message describes that same thing. Against the tree today they
      are opposites.
- [ ] **Either** the test discriminates ghost routing — prove it with a
      mutation that changes ghost's behaviour and reddens **only** this test,
      captured red-then-green, 6 of 6 — **or** the test is removed as
      redundant and the Update Log records why (ghost pushes `Background`
      unconditionally and never calls `routes_to_background`). If you remove
      it, `cargo test --lib 'daemon::stream'` → **11**, and say so.
- [ ] **No duplicate remains:** mutating the routing to background everything
      must not redden two tests that assert the identical call. Against the
      tree today `omitting_wait_runs_in_the_foreground` and
      `ghost_tool_call_preserves_its_execution_mode` both fail for the same
      reason.
- [ ] `cargo test --lib 'daemon::stream'` → **11 or 12**, `0 failed`, matching
      whichever route you took.
- [ ] `cargo test --lib` → **1895 or 1896**, `0 failed`, matching the same.
- [ ] The three routing tests still pass and still discriminate: the
      background-everything mutant reddens the foreground pair, and a
      foreground-everything mutant reddens `wait_false_runs_in_the_background`.
- [ ] All four gates pass; no `#[allow]` added:
      `git diff d3121674..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
