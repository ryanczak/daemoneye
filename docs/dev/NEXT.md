# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-08a-shell-context-block.md`
— 08a, shell-context-block. **Bounced 2026-09-16** (bug-08a-1), status
`in-progress`.

The shipped behaviour is correct: all twelve original acceptance criteria pass,
four gates are green, and both mutation checks reproduce independently (3/3 red
each, on exactly the named tests). The bounce is for test-quality and evidence
defects — two assertions in
`format_shell_context_lists_every_shell_with_its_flags` that cannot fail, and an
E2E note that states a mechanism which is not the one at work.

The refreshed acceptance criteria at the bottom of the phase doc's criteria list
express the outstanding work; each was run and fails against HEAD `6f876d0d`.

Re-dispatch with `/rexymcp:dispatch phase-08a`.
