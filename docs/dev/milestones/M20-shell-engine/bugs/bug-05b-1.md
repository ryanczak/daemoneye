# Bug 1 on phase-05b: three of the seven pinned tests do not exercise the case they are named for

**Severity:** major
**Status:** resolved 2026-09-04 (verified at review by three independent mutation checks)
**Filed:** 2026-09-04

## What's wrong

`src/shell/backend.rs` is correct production code — all four gates pass, there
is no `unsafe`, no `set_size`, no `unwrap`/`expect`/`panic!` outside tests, no
`#[allow]`, no `TODO`. **The defect is entirely in the tests.** Three of the
seven carry the right name and assert the wrong thing, and a fourth races.

### 1. The signal test is vacuous — proven by mutation (major)

`backend_signal_targets_the_foreground_group_not_the_shell` **passes with
`signal()` doing nothing at all.** Verified at review by replacing the body of
`ShellBackend::signal` (`src/shell/backend.rs:~115`):

```rust
.signal_foreground(signal_name(sig))      // replaced with:
.is_alive().map(|_| ())
```

→ `test result: ok. 1 passed; 0 failed` (20.61s).

Both its assertions survive a no-op signal:

- `ShellState::Running` is true whether or not the signal fired — the *shell*
  was never the thing at risk.
- `cast.contains("post-signal-probe")` is satisfied by the **PTY echo** of the
  typed line. The probe text reaches the cast as echoed input even when
  `sleep 30` is still holding the foreground and the command never runs. This
  is the same echo hazard phase-02 already measured and defended against with
  the split-nonce marker; it is undefended here.

The spec (§ Test plan) asked for a negative case: *"assert the command stops
**and** the shell is still alive."* Only the second half is tested, and the
first half is unguarded.

### 2. The simultaneity test writes no input during streaming (major)

`backend_streams_while_input_is_written` calls `backend.input(...)` **exactly
once**, at the start, then passively reads until `second-part` arrives:

```rust
let command = "printf 'first-part\\n'; sleep 0.15; printf 'second-part\\n'";
tokio::runtime::Runtime::new()...block_on(backend.input(command.as_bytes()))
// ...no further input() call anywhere in the test...
let got = recv_until(&mut rx, deadline, |c| ...contains("second-part"));
```

Nothing is written *while* output streams. The test's own comment claims it is
"interleaving the 'while streaming' writes", but there are no writes to
interleave. As written it asserts "output arrives", which
`backend_fans_one_chunk_out_to_subscriber_log_and_screen` already covers.

The spec asked for: *"write input **while** output is streaming, and assert the
input reached the shell (its echo or effect appears downstream) and no streamed
chunk was lost."* Neither the concurrent write nor the input-reached-the-shell
assertion exists.

This is the milestone's headline lesson — *name the two things that can happen
at once and test them happening at once* — unmet by the very test written to
embody it.

### 3. The lagged-subscriber test drops the slow subscriber before saturating (major)

`backend_lagged_subscriber_does_not_break_the_pump` drops `_rx_slow` **before**
the 2000-write saturation loop:

```rust
let mut _rx_slow = backend.subscribe();
let mut rx_fast  = backend.subscribe();
quiet(&mut rx_fast, deadline);
drop(_rx_slow);                        // <-- the slow subscriber is gone
rt.block_on(async { for _ in 0..2000 { let _ = backend.input(b"x\n").await; } });
```

There is never a lagging subscriber coexisting with a live one, so the stated
guarantee — *"a subscriber that stops reading must not stall the pump for
others"* — is never exercised. What remains is "one subscriber recovers from
its own `Lagged`", which is a different property.

### 4. The fan-out test's cast assertion races (minor)

`backend_fans_one_chunk_out_to_subscriber_log_and_screen` polls for the screen
(step 3) but reads the cast file with no barrier (step 2). The pump order is
`out_tx.send(chunk)` → `log.write_output(...)` → `screen.feed(...)`
(`src/shell/backend.rs:74-83`), so `recv_until` returns at the instant the log
write has *not yet happened*. `cast_output()` acquires and drops the **`shell`**
mutex, which the pump does not hold while logging — it synchronizes with
nothing.

Verified at review by inserting a 50 ms sleep between the send and the log
write: the test fails deterministically with
`cast: expected an "o" event containing "FANOUT_51_99"`. It passes today only
because the test thread happens to be slower than the pump.

## What should happen

Each of the four tests must fail when the behaviour it names is broken.

1. The signal test must fail when `signal_foreground` is neutered. It must
   assert the foreground command **stopped**, by a signal that PTY echo cannot
   forge — the echo of a typed line is not evidence the line ran. Note the
   existing tests already have the tool for this: a unique marker printed by
   the command's *output*, distinguished from its echo.
2. The simultaneity test must issue a second `input()` **while** the first
   command is still streaming, and assert both that the second input's effect
   reaches the shell and that no chunk of the first command's output was lost.
3. The lagged test must keep the slow subscriber alive and unread across the
   saturation, then assert the fast one still receives current chunks.
4. The fan-out test's cast assertion must poll to a bounded deadline, the way
   its own step 3 already polls for the screen — that loop is the in-file
   pattern to copy.

## Root cause

All four share one mechanism: **the assertion is satisfiable by something other
than the behaviour under test.** Echo satisfies the signal probe; passive
reading satisfies "streams while input is written"; a single subscriber
satisfies "lagged subscriber does not break the pump"; and the pump usually
wins a race the test never forces.

## Definition of done

Confirmed FAILING against the current tree at filing time — each value below is
what the tree returns **now**.

- [ ] The signal test is no longer vacuous. With `.signal_foreground(signal_name(sig))`
      in `ShellBackend::signal` replaced by `.is_alive().map(|_| ())`,
      `cargo test --lib shell::backend::tests::backend_signal` must report
      **FAILED**. It currently reports `ok. 1 passed`. Restore the line afterwards.
- [ ] The fan-out test is no longer racy. With
      `std::thread::sleep(std::time::Duration::from_millis(50));` inserted
      between `out_tx.send(chunk.clone())` and the `log` write in the pump,
      `cargo test --lib shell::backend::tests::backend_fans_one_chunk` must
      report **ok. 1 passed**. It currently FAILS. Remove the sleep afterwards.
- [ ] `awk '/fn backend_streams_while_input_is_written/,/^    }$/' src/shell/backend.rs | grep -c 'backend\.input('`
      → **2 or more** (now `1`).
- [ ] `awk '/fn backend_lagged_subscriber_does_not_break_the_pump/,/^    }$/' src/shell/backend.rs | grep -c 'drop(_rx_slow)'`
      → **0** (now `1`).
- [ ] `awk '/fn backend_lagged_subscriber_does_not_break_the_pump/,/^    }$/' src/shell/backend.rs | grep -c 'rx_slow, '`
      → **1 or more** (now `0`) — the slow subscriber must actually be read
      from after the saturation, not merely held.
- [ ] `cargo test --lib shell::backend::` reports **7 passed, 0 failed** —
      **7, not 8.** This bug adds no tests; it strengthens four existing ones.
      A rising count means scope creep.
- [ ] `shell::pty::` **13**, `shell::host::` **8**, `shell::log::` **12**,
      `shell::screen::` **11**, `shell::proto::` **5** — all unchanged.
- [ ] All four gates pass.
