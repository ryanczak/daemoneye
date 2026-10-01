# NEXT

**Active phase:** **M23 phase-03b — shell-screen-snapshot**, drafted
2026-10-01, status `todo`. Dispatch with `/rexymcp:dispatch phase-03b`.

**What it does.** A new host request, `SubscribeScreen`, is answered first
with `Screen { bytes }`: escape codes that redraw the host's current screen
exactly. Live `Chunk`s follow. The snapshot and the subscription are taken
under one screen lock, and the pump now feeds and sends each chunk under that
lock, so no byte is lost or duplicated between them. Host side only. Five
tests; lib 2105 → 2110.

**Measured at drafting.** vt100 0.16.2's `state_formatted` reproduces text,
colour and cursor exactly. Alt-screen mode needs a `?1049h` prefix. The
whole change was dry-run: tests 6/6, lib 2110 ×3, clippy clean, three
mutations red 6/6. The race the lock closes cannot be shown by a test (12/12
green on the old order), so review checks it by reading the code.

**Split.** The daemon `ShellTail` relay is now row 03c.

**After 03b:** `/rexymcp:architect next` drafts 03c (shell-tail-relay).
