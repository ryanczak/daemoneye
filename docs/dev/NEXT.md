# NEXT

**Active phase:** **M23 phase-05b — tail-viewer**, drafted 2026-10-02, status
`todo`. Dispatch with `/rexymcp:dispatch phase-05b`.

**What it does.** `/tail s3` (alias `/watch`) in chat opens a live, full-screen
view of a shell on 05a's model: `f` follow, ↑↓ PgUp/PgDn Home/End scroll, and
`Esc`/`q`/`Ctrl-O` close. An unknown id prints the daemon's error without
entering the alt screen; a closed stream shows `exited` and keeps the view.
The help lists `/tail` and `/watch`. Five tests; lib 2140 → 2145.

**Split.** Search (`/`, `n`/`N`) is row 05c.

**Measured at drafting.** The loop is generic over backend, frames, keys and
resizes, so a test drives the real loop with a `TestBackend`, a duplex daemon
and a key channel. The whole change was dry-run green: the test filter
printed 5 on the dry run, lib 2145 ×3, mutations A–D red.

**After 05b:** `/rexymcp:architect next` drafts 05c (tail-search).
