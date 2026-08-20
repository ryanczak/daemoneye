# Bug 3 on phase-02: ctrl+o is swallowed mid-turn, so the footer advertises a key that does nothing

**Severity:** major
**Status:** resolved (round 4, `d24dba9`)
**Filed:** 2026-08-20
**Found by:** live check in a real tmux pane (isolated `tmux -L de-m17` server),
during M17 milestone close-out.

## What's wrong

`Key::CtrlO` is handled in exactly one place — the idle input loop
(`src/cli/commands/chat.rs:738`). While a turn is streaming, the client is
inside `select_stream` (`src/cli/commands/stream.rs:807`), whose key arm is:

```rust
                key = read_key(stdin) => {
                    if let Some(key) = key {
                        if let Some(outcome) = focus_outcome(&key) {
                            return outcome;
                        }
                        match interrupt_state.feed(&key) {
                            InterruptAction::Ignore => continue,
```

`focus_outcome` maps only `Key::FocusGained` (`stream.rs:873-878`), and
`InterruptState::feed` returns `Ignore` for ctrl+O. The keypress is therefore
**consumed and dropped** — not queued, not deferred.

Measured live, same session, same key, two moments:

| When ctrl+o is pressed | `#{alternate_on}` | Result |
|---|---|---|
| Mid-turn, as `… 54 more lines · ctrl+o` renders | `0` | nothing happens; keypress lost |
| At the idle prompt after the turn ends | `1` | viewer opens, full output present (98 rows) |

The second row confirms the viewer itself is sound. The defect is *when* it can
be reached.

## What should happen

`ctrl+o` opens the transcript viewer **during a turn as well as at the prompt**,
and returns to the still-streaming turn when the viewer closes. The transcript
is already populated up to that moment, which is exactly what the user wants to
look at.

Two properties the fix must hold:

1. **No daemon frames are lost across the viewer.** `select_stream`'s
   `recv_line` already accumulates into a **caller-owned** `line_buf`
   (`stream.rs`, the buffer declared before the loop) precisely so that a
   dropped read future does not strand bytes — the same mechanism that makes
   interrupt-during-stream non-destructive. Opening the viewer must not reset or
   bypass that buffer.
2. **Interrupt behaviour is unchanged.** Esc/ctrl+c must still warn on first
   press and abort on second; `select_stream_first_interrupt_press_warns`
   (`stream.rs:1581`) must keep passing.

## Root cause

The viewer was given exactly one entry point. Phase-02 listed "opening the
viewer mid-turn" as out of scope — defensible on its own — and phase-03 then
added the ` · ctrl+o` suffix to the elided-output footer. That footer renders
*while the turn is still streaming*, so the affordance advertises itself at the
one moment it cannot work. A deliberate limitation plus an unconditional
advertisement is a broken promise.

This is an **architect-side scoping miss**, not an executor error: both phases
implemented their specs exactly, and no acceptance criterion in either could
observe "the key does nothing right now" — it is only visible with a terminal
and a stopwatch.

## Definition of done

Each command below **fails against the current tree** (verified 2026-08-20) and
must pass:

- [ ] `grep -c "Key::CtrlO" src/cli/commands/stream.rs` prints at least `1`.
      (Currently `0`.)
- [ ] Test `stream_key_ctrl_o_opens_viewer` passes — the pure key classifier
      that today is `focus_outcome` (`stream.rs:873`) maps `Key::CtrlO` to a new
      `StreamOutcome::OpenViewer`, asserted by value. Extend that function (or
      rename it to something like `key_outcome`) rather than adding a second
      classifier: it is already the pure, tested precedent, with
      `focus_outcome_maps_focus_gained_to_reanchor` at `stream.rs:1664`.
      (Currently absent.)
- [ ] Test `stream_key_focus_gained_still_reanchors` passes — `Key::FocusGained`
      still maps to `StreamOutcome::Reanchor`. Renaming the function must not
      drop its existing behaviour. (May reuse/rename the existing test; the
      mapping must remain asserted.)
- [ ] Test `select_stream_first_interrupt_press_warns` still passes, unchanged.
- [ ] The turn survives the viewer: after `StreamOutcome::OpenViewer` is
      handled, `ask_with_session_ratatui` re-enters its read loop with the same
      `line_buf` and the same connection — it must **not** return, break the
      turn, or reconnect. State the mechanism in the Update Log; the live check
      at milestone close verifies it on screen.
- [ ] `renderer.reanchor()` runs after the viewer closes mid-turn, so the inline
      viewport is re-pinned before streaming resumes.
- [ ] The phase-02 guard contract is untouched: `grep -c "disarm" src/cli/viewer.rs`
      prints `0`, and `grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs`
      prints nothing and exits 1.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Out of scope for this fix

- Changing phase-03's footer text. The footer becomes true once the key works;
  do not retract ` · ctrl+o`.
- Opening the viewer from an approval prompt (`[Y]es/[A]pprove/[N]o`) or the
  credential prompt — those use their own readers and are a separate question.
- Any change to the viewer itself. `run_transcript_viewer` already does the
  right thing; this bug is about reaching it.
