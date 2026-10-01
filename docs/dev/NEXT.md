# NEXT

**Active phase:** **M23 phase-03a — shell-ipc-relay**, drafted 2026-10-01,
status `todo`. Dispatch with `/rexymcp:dispatch phase-03a`.

**What it does.** Three request/response IPC messages let a client see and
touch the daemon's PTY shells: `ShellList` (every shell, with host, owner,
state and last recorded command), `ShellInput { id, bytes }` and
`ShellResize { id, rows, cols }`. Unknown or exited ids get a `Response::Error`
naming them. The handlers are in a new `src/daemon/server/shells.rs` and take an
injected registry. Five tests: a wire round trip, the list, input reaching a
real in-process shell, resize changing a real shell's size, and refusal.
Lib 2099 → 2104.

**Measured at drafting.** A raw `Resize { rows: 30, cols: 100 }` frame against
an in-process host was acked, and `status()` then said 30×100 (it had been
24×80). No resize client function exists; `send_input` does. Every symbol the
spec names was checked against the tree.

**Split.** `ShellTail` (live streaming) became row 03b. The host streams only
from `Subscribe` and has no backlog, so a correct starting screen needs its
own design.

**After 03a:** `/rexymcp:architect next` drafts 03b (shell-tail).
