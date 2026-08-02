# NEXT

**Phase 02 is `done` (approved_first_try). Both M8 phases are now `done`.**

**The milestone is at its boundary — a human gate.** Run `/rexymcp:architect` to
close it: write the retrospective, fold the calibration lessons, and set this
file's active phase to "none". **That has deliberately not been done here** —
the review step does not write retrospectives or close milestones, so this
pointer is left mid-state on purpose rather than by oversight.

**M7's exit criterion 8 is now satisfied**, which is worth carrying into the
close: it was the one criterion M7 shipped partly-met.

## Phase 02 — what it was

## Phase 02 — what it is

The four real-clock sleeps in non-`#[ignore]`d tests, which are two byte-identical
copies of one `write_bytes` helper. Finishes M7's single unticked exit criterion.

**Measured before drafting, not reasoned about:**

- **The 10 ms "give the reader time" wait is unnecessary.** Deleted both and ran
  each module 30 times: 0 failures, full lib suite green at 1032. `write()`
  returns once the bytes are in the pipe buffer and the caller reads the same fd
  — there was never anything to wait for.
- **The 1 ms EAGAIN backoff needs replacing, not deleting.** Replaced with
  `tokio::task::yield_now().await` plus an `ErrorKind::WouldBlock` assert:
  clippy clean, 0/30 per module. The assert matters because the current code
  says "EAGAIN is fine" and never checks, so *any* write error spins forever.

**Two calibration traps found while writing the acceptance criteria**, both of
which would have produced an unsatisfiable spec:

1. `stream.rs` has **four** `tokio::time::sleep` calls, and **three are
   production** (lines 681/705/727 — the streaming loop's timeout and tick). A
   blanket "remove sleeps from stream.rs" breaks streaming. The criterion pins
   the count at exactly 3 afterwards.
2. `tty.rs` has **six** `from_millis(10)` occurrences, and **five are
   production** — `timeout(Duration::from_millis(10), stdin.read_byte())` in the
   escape-sequence reader. Grepping that literal as a proxy for "the sleep is
   gone" would demand 0 and be impossible. The criterion greps
   `tokio::time::sleep` instead and records that `from_millis(10)` must end at 5.

**Deliberately no gate.** A test that forbids real-clock sleeps would be the
durable answer, but a correct scanner must separate production from
`#[cfg(test)]` and exempt `#[ignore]`d tests — the M7 close-out audit got that
wrong twice before getting it right by hand. A naive grep gate would fire on
`stream.rs:681` and on four legitimate `#[ignore]`d tests. Recorded as future
work rather than shipped wrong.

## Phase 01 — what landed

`cargo test --test isolation` no longer flakes. `alloc_free_port` is replaced by
`alloc_held_port`, which keeps the listener alive; the stub is handed its
pre-bound listener via `from_std` (no rebind at all), and the webhook listener is
released only immediately before the daemon spawn.

**Verified by the reviewer, not read from the transcript: 0 failures in 200
consecutive runs**, against a measured 5/100 baseline. If the old rate still
held, that outcome has ~0.003% probability. A single green run could not have
distinguished the fix from luck — at 5%, one run passes 95% of the time on the
unfixed code, which is how the bug survived two milestones of green gates.

`held_port_cannot_be_rebound` pins the invariant in one second; releasing the
listener again makes it fail immediately. The canary
`webhook_ports_differ_between_environments` survived — it was never flaky, it was
the detector.

## Phase 02 — named only

The four real-clock sleeps in non-`#[ignore]`d tests:
`src/cli/input/tty.rs:370,374` and `src/cli/commands/stream.rs:1265,1268`.
Draft with `/rexymcp:architect next` when 01 is `done`.

## Explicitly not in M8

**`hooks_land_on_private_server`** — the other flake, from phase-04 review. It
binds no ports at all and did **not** fail once in the 100-run baseline, so there
is no live evidence to work from. M7's retrospective originally claimed both
flakes shared a root cause; that was an over-claim and has been corrected there.
If it recurs, it is a separate bug wanting its own investigation.

## Still carried, unscheduled

1. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()`.
2. **`tree_block_of`'s loose error contract** — an unterminated fence returns
   `Some` where the spec said `None`. No reachable consequence.
3. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
4. **`reconcile_index()` has no operator entry point** — deferred twice; a
   `reindex` subcommand or startup hook is the obvious home.

## The rule M7 earned, still in force

> **Do not assert a fact about the system in a spec unless it was executed.**
>
> **Naming a false-success mode is worthless unless the guard is checked against
> it.**

Plus: a phase that deliberately lands code for a *later* phase must say how the
deny-warnings gate is satisfied, and **a green bounce always needs a refined
re-dispatch**.
