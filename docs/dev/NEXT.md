# NEXT

**Active phase:** **M23 phase-05c — tail-search**, drafted 2026-10-02, status
`todo`. Dispatch with `/rexymcp:dispatch phase-05c`.

**What it does.** In `/tail`'s view, `/` opens a search prompt and `Enter`
finds the newest case-insensitive match across scrollback and the live
screen. `n` steps to older matches and `N` to newer ones. No match keeps the
view, and a jump pauses follow. `Ctrl-C` now closes the view (the 05b nit).
One file, `src/cli/tail.rs`, given whole in the spec. Five new tests, one
updated; lib 2145 → 2150.

**Measured at drafting.** Rebuilding every line through vt100's scrollback
offsets takes 1.6 ms at 5 000 lines, so search needs no index. A
case-insensitivity mutant survived a lowercase-only test query, and the test
now uses `"Error"` vs `ERROR`. Dry-run green: filter printed 6, lib 2150 ×3,
mutations A–E red.

**After 05c:** `/rexymcp:architect next` drafts 06 (logs-and-replay).
