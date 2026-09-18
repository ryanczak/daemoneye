# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09d-pane-param-retirement.md`
— 09d, pane-param-retirement. **Bounced 2026-09-17** (bug-09d-1), status
`in-progress`.

Everything the phase set out to do is done and verified: the milestone's second
exit criterion is met (`%N` in the tool surface is **0**), tool counts are
unchanged at 42 / 28, both `is_none_or` escape-hatch conjuncts are gone, and
`read_file` / `edit_file` correctly keep `target_pane` for M21. All four gates
are green.

**The bounce is a regression in a path the phase does not own.**
`GhostPolicy::resolve_command` now has **zero** production callers.
`run_background` had two — one inside the `retry_pane` block (correctly removed
with it) and one in the main path (should have stayed). Ghost and scheduled
background jobs no longer resolve bare script names to absolute paths, and
`run_with_sudo` policies no longer get their sudo prefix.

**No gate catches it**: `resolve_command` is `pub`, so `dead_code` does not
fire; its 25 unit tests call it directly and still pass; and the call site has
no test at all. That is why the DoD requires a regression test pinning the
**call site** plus a mutation check proving the deletion can no longer be
silent.

Re-dispatch with `/rexymcp:dispatch phase-09d`.
