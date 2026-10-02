# NEXT

**Active phase:** **M23 phase-03d — shell-tail-relay**, drafted 2026-10-02,
status `todo`. Dispatch with `/rexymcp:dispatch phase-03d`.

**What it does.** `Request::ShellTail { id, follow }` over the daemon socket
answers in three stages:
- `ShellScreen` first (the host's exact screen, or empty for an old host,
  using a `Subscribe` fallback);
- with `follow`, live `ShellChunk`s;
- `ShellExited` when the shell ends, seen on host EOF or a `Status` poll.

The tail stops when the client hangs up. It also clears the two stale
comments carried from 03c. Six tests; lib 2114 → 2120.

**Measured at drafting.** The whole change was dry-run: tests 6/6, lib 2120
×3, clippy clean, three mutations red 6/6. The end-to-end block's live part
runs a real daemon that adopts a real shell-host, tails it, types into it
and exits it. On the dry run it printed `ShellScreen` → output chunks →
`ShellExited,EOF`, host exit 0; on the current tree, `live_first=Error`.

**Lag** is decided, not built: a viewer re-issues `ShellTail`; row 05 owns
resync.

**After 03d:** `/rexymcp:architect next` drafts 04 (shells-and-hosts).
