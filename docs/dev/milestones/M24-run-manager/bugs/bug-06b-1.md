# Bug 1 on phase-06b: the recorded `stage` cannot name the door it came through

**Severity:** major
**Status:** resolved — `b727013` (architect takeover after `stop_phase` at turn 90)
**Filed:** 2026-09-11

## What's wrong

Phase-06b's fourth deliverable is "A cancelled turn records
`RunState::Cancelled { stage, turns }` **naming the door it came through**",
and § Spec task 4 spells out the mechanism: "**Both doors use
`CancelReason::UserStop`** — do not add a `CancelReason` variant […] The
`stage` string is what distinguishes them."

The shipped code derives the stage *from the reason*:

```rust
// src/daemon/stream.rs:30-40
/// The `stage` string recorded in `RunState::Cancelled`, naming the door the
/// cancellation came through. Both wired doors fire `CancelReason::UserStop`
fn cancel_stage(reason: &crate::daemon::run::CancelReason) -> &'static str {
    match reason {
        crate::daemon::run::CancelReason::UserStop => "user_stop",
        crate::daemon::run::CancelReason::ClaudeStop => "claude_stop",
    }
}
```

Both production cancel doors fire `CancelReason::UserStop`:

- chat client Ctrl-C — `src/daemon/server/mod.rs:176-179`,
  `cancel_session(&session_id, CancelReason::UserStop)`;
- `daemoneye stop <run>` sentinel — `src/daemon/run/cancel.rs:191`,
  `cancel_run(&id, CancelReason::UserStop)`.

So `cancel_stage` returns `"user_stop"` for **both**, and the recorded stage
names nothing. Verified on the current tree:

```
$ grep -rn 'CancelReason::' src/ --include='*.rs' | grep -v '/run/result.rs' | grep -v tests
src/daemon/stream.rs:36:  CancelReason::UserStop => "user_stop",
src/daemon/stream.rs:37:  CancelReason::ClaudeStop => "claude_stop",
src/daemon/server/mod.rs:178: crate::daemon::run::CancelReason::UserStop,
src/daemon/run/cancel.rs:191: cancel_run(&id, CancelReason::UserStop);
```

`CancelReason::ClaudeStop` is fired by **no** production call site anywhere in
the tree, so its match arm is dead — a branch for a case that cannot happen,
which `STANDARDS.md` § 2.2 forbids. The doc comment above it ("naming the door
the cancellation came through") and the commit message of `ec521b2`
("`claude_stop` from the sentinel path's twin reason") both assert a behaviour
the code does not have: the sentinel path fires `UserStop`, not `ClaudeStop`.

No test covers the stage string, so the whole mechanism is uncovered as well as
inert — `daemon::stream` is still at its pre-phase 7 tests.

## What should happen

The `stage` recorded in `RunState::Cancelled` must **differ** between the two
doors — `Request::Cancel` from the chat client versus the `stop-<run>` sentinel
— so a reader of the run log can tell which one fired, and a test must pin that
difference.

Constraints the solution must satisfy (the phase doc's § Out of scope and
§ Authorizations still bind):

- **Do not add a `CancelReason` variant.** That type shipped in phase-03 and is
  explicitly out of scope for this phase; the distinction is carried by the
  `stage` string.
- **Do not leave a production match arm for `CancelReason::ClaudeStop`** while
  no door fires it.
- The ghost path must stay uncancellable by either door (unchanged; all the
  phase's other acceptance criteria still hold).
- Authorized files are unchanged from the phase doc — in particular
  `src/daemon/run/result.rs` stays untouched.

## Root cause

`cancel_stage` (`src/daemon/stream.rs:35`) takes the only value the
`select!` arm has in hand — the `CancelReason` returned by
`cancel_signal.cancelled()` — and treats it as the door identity. It is not:
phase-04 gave both doors the same reason deliberately, and § Spec task 4 said so
in as many words. The door identity is not carried on the signal at all, so no
function of the reason can recover it.

## Definition of done

Each was run against the current tree (commit `80ddd3e`) and returns the "now"
value shown, so each fails today.

- [x] `grep -c 'CancelReason::ClaudeStop' src/daemon/stream.rs` → **0**
      (now **1**).
- [x] A test pinning the difference exists:
      `grep -c 'fn cancel_stage_distinguishes_the_two_doors' src/daemon/stream.rs`
      → **1** (now **0**). It must assert that the stage recorded for the chat
      door and the stage recorded for the sentinel door are **different
      strings**, and fail if they are made equal.
- [x] `cargo test --lib daemon::stream` → **8 or more** passing, `0 failed`
      (now **7**).
- [x] Full lib suite **1745 or more** passing, `0 failed` (now **1744**).
- [x] Preservation, each unchanged: `daemon::run::active` **6**,
      `daemon::run::cancel` **13**, `daemon::server` **21**,
      `daemon::run::guard` **7**, `daemon::run::turn_log` **7**,
      `daemon::run::log_read` **11**, `daemon::executor` **235**.
- [x] A mutation check recorded red-then-green in the Update Log: make both
      doors record the same stage string → the new test goes **red** on its
      named assertion.
- [x] All four gates pass.
