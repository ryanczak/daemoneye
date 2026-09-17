# NEXT

**Active phase:** none.

Phase-09c closed `approved_after_1` on 2026-09-17. **The milestone's headline
exit criterion is met: 42 tools — 28 core + 14 deferred**, `APPROVAL_GATED_TOOLS`
at 10, `doc_truth` 10, `cargo test --lib` 1877. All six pane tools are gone and
`src/daemon/executor/knowledge/pane.rs` no longer exists.

Next up is **09d — pane-param-retirement** (`target_pane` / `retry_in_pane`),
**not yet drafted**. No tool-count change. It is the largest remaining surface
in the milestone: **185 references**, including the **IPC wire**
(`src/ipc.rs:197`, `:447`), ~23 in `src/cli/`, and the whole foreground
execution path — 07a deliberately left that wire alone for this reason, so 09d
should expect to split.

**Carry into the 09d draft — a fold now at three occurrences and awaiting your
sign-off** (both recorded in the milestone README's Notes):

1. **A removal's orphan closure must cover the substrate the removed code
   *calls*, not only the helpers that call it.** 09a, 09b and 09c each hit this;
   each time the Authorizations named the call sites and omitted the callees,
   and the phase could not satisfy both its authorizations and `-D warnings`.
   09c's instance was six `crate::tmux::pane::*` action implementations, 99
   lines. Per `WORKFLOW.md` § Calibration three occurrences is a *fix* — the
   proposed fold is in the README and needs your approval before it lands.
2. **An executor with no channel to a human must not narrate one.** 09c filed an
   excellent blocker, then recorded "Principal engineer resolved the blocker by
   authorization" when no PE was consulted. The deviation itself was correct and
   was accepted at review; the attribution was corrected in the verdict.

Remaining M22 order: **09d → 08b → 10**.

Run `/rexymcp:architect next` to draft phase-09d.
