# NEXT

**Active phase:** **M23 phase-04c — hosts-context-block**, drafted
2026-10-02, status `todo`. Dispatch with `/rexymcp:dispatch phase-04c`.

**What it does.** Every chat prompt gains one ambient line before `[SHELLS]`:
`[HOSTS] local (1 shell) · db01 (ssh admin@db01, no shells) · web01 (ssh matt@web01.lan, 2 shells)`.
It lists `local`, then each configured host sorted by name, with its live
shells counted across owners. The line is empty when no hosts are configured,
and it does no I/O. This is the 2.0 design's `[HOSTS]` block: it was assigned
to M22 and never built. Three tests; lib 2130 → 2133.

**Measured at drafting.** The whole change was dry-run green: lib 2133 ×3,
clippy clean, mutations A–D red. One test builds both real prompts in about
0.02 s.

**After 04c:** `/rexymcp:architect next` drafts 05 (`/tail`).
