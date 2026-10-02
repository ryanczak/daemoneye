# NEXT

**Active phase:** **M23 phase-04a — shells-command**, drafted 2026-10-02,
status `todo`. Dispatch with `/rexymcp:dispatch phase-04a`.

**What it does.** `/shells` in `daemoneye chat` lists every PTY shell the
daemon runs: id, host, owner (`this session` for the chat's own), state
(`live` or `exited N ago`, plus `interactive` / `dedicated`), age and last
command. It is built on one `ShellList` request and a pure renderer. It also
adds a help line and clears 03d's stray banner. Five tests; lib 2120 → 2125.

**Split.** Row 04 became 04a `/shells`, 04b `/hosts` (needs a ControlMaster
probe) and 04c the `[HOSTS]` context block.

**Measured at drafting.** The whole change was dry-run green, with three
mutations red. Chat in a clientless private tmux hangs before drawing, so the
chat-UI live check stays with row 10's architect-run close, as the PE decided.

**After 04a:** `/rexymcp:architect next` drafts 04b (hosts-command).
