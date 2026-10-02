# NEXT

**Active phase:** **M23 phase-04b — hosts-command**, drafted 2026-10-02,
status `todo`. Dispatch with `/rexymcp:dispatch phase-04b`.

**What it does.** `/hosts` in chat: `local` first, then each configured host
with its ssh destination, ControlMaster state (`master:up/down`, using the
existing `master_running`) and live-shell count.

**Plus two 03d defects found at the 04a review (required):**
- **Relay:** a client hanging up mid-stream made the tail return
  `Err(broken pipe)` (3 of 8 full runs). Now it ends `Ok`, pinned by a
  deterministic test.
- **Tests:** the two shell-tail tests asserted on the first screen and used
  markers that appear in the typed command's echo. They now poll, and build
  their markers at run time.

Five new tests; lib 2125 → 2130.

**Measured at drafting.** The whole change was dry-run green, lib 2130 on 5/5
full runs. Mutations: A and B red, C (delayed output) green 3/3, D red 6/6
with the exact production error.

**After 04b:** `/rexymcp:architect next` drafts 04c (the `[HOSTS]` context
block).
