# NEXT

**Active phase:** none.

Phase-09b closed `approved_first_try` on 2026-09-16 — `watch_pane` and
`close_background_window` are gone. **43 tools: 29 core + 14 deferred**;
`doc_truth` 10; `cargo test --lib` 1883. `knowledge/pane.rs` is down to 72
lines holding only the two `tmux_control` helpers.

Next up is **09c — tmux-control-removal**, **not yet drafted**. It deletes
`tmux_control` and reaches the milestone's exit criterion of **42 tools:
28 core + 14 deferred**. It is the largest single-tool footprint left: 38 refs,
22 of them in `executor/mod.rs` and 8 in `agents/policy.rs`, and it is
approval-gated — so `APPROVAL_GATED_TOOLS` shrinks and the README's `⚠`
markers move with it, both guarded by `doc_truth`.

**Carry into the 09c draft** — three things from 09a/09b's reviews, all in the
milestone README's Notes:

1. **Measure the orphan closure, including `use` statements.** 09b measured
   callee functions and still missed a `pub(super) use` that existed only for
   the removed caller and tripped `-D unused-imports`.
2. **Write criteria against identifiers, not tool-name strings.** 09b's
   `WatchPane` / `CloseBackgroundWindow` greps hit exactly; its `watch_pane` /
   `close_background_window` string greps read 5 and 2, because the phase's own
   E2E test must contain those literals. Third occurrence of that class.
3. **`tmux_control` will join the seeded-asset backlog** already carrying
   `watch_pane` / `close_background_window` in `ghost-shell.txt`, the tmux
   knowledge memory, `agents/sysadmin/config.toml` and `REQUIREMENTS.md`.
   Phase 10's row was widened at this review to cover all four.

Remaining M22 order: **09c → 09d → 08b → 10**.

Run `/rexymcp:architect next` to draft phase-09c.
