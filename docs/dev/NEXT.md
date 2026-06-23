# NEXT

**Active milestone:** none — M1 (Agent Tooling Improvements) is **complete**.

**Active phase:** none.

All eleven M1 phases (01–11) are `done`. phase-11-on-demand-tool-loading was the
last in-scope phase — approved 2026-06-23 with verdict `escalated` (the local-LLM
executor `hard_failed`; Claude Code took over and completed it directly, fixing two
inert `Vec::new()` call sites in the interactive and ghost loops that had made the
deferred-tool feature do nothing). See the phase doc's Review verdict and the M1
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.

M1 is at a **milestone boundary**: human sign-off is required before any M2 is
drafted or dispatched. The user kicks off the next milestone explicitly.
