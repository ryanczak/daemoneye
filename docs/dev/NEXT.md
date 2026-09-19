# NEXT

**Active phase:** none — **M21 phase-01a is `done`** (2026-09-19, `escalated`
via session takeover after three `budget_exceeded` runs).

**The defect it existed to fix is fixed.** A `sudo` password prompt on the
default PTY backend now reaches the user through `Response::CredentialPrompt`,
the answer is written into the shell mid-command, and the command completes.
The end-to-end capture is the exact inverse of the drafting probe.

All four gates green: lib **1893 passed / 4 ignored**, doc_truth 14,
integration 32, isolation 9, bug_tracker 6.

**Division of labour, for the record:** the executor wrote every production
line and all seven tests across three runs; the architect finished only the
evidence — the two mutation checks, the real-host capture, and cleanup.

**Config change during the takeover:** `max_turns` raised to **1000** (PE,
2026-09-19). Three 600-turn exhaustions on one phase was the signal.

**Next row: 01b — input-scrub-on-prompt.** The credential must stop reaching
the cast log. It is host-process work (`shell/backend.rs`, `shell/log.rs`),
deliberately kept out of 01a and pinned there by must-survive criteria:
`grep -rc 'scrub\|redact' src/shell/log.rs` is still **0** and the `"i"` event
writer is untouched, so 01b's mutation is satisfiable.

Run `/rexymcp:architect next` to draft it. Ten rows in M21; live host `pinky`.

**Carried to M21 close:** three calibration items in 01a's review verdict —
most importantly that the M22 pre-dispatch trigger failed on its first outing,
in the first phase drafted after it landed. Recommendation on the table: make
acceptance criteria anchored by default so they cannot prefix-match the test
names the phase itself creates.
