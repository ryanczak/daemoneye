# NEXT

**Active phase:** **M21 phase-08b — shell-exit-detection**, drafted
2026-09-27, status `todo`. Dispatch with `/rexymcp:dispatch phase-08b`.

**What it does.** A command sent to a shell whose program has exited fails
in seconds, saying the shell exited — instead of waiting out its whole
timeout, being reported as a timeout, and telling the model it "may still be
running". One shared loop (`read_to_marker`) gains a 500 ms `Status` poll; a
typed `ShellExited` error carries the fact to both model-facing texts.

**Why it jumped the queue.** Drafting the ghost PTY move (now 08c) meant
measuring what happens when a ghost's ssh cannot connect. A throwaway test
drove a real `ssh -o BatchMode=yes` through the daemon's real `run_command`:
ssh died in 17 ms, the client waited the full 20 s. The same probe, without
`BatchMode`, showed a second defect — a plain command's bytes typed into
ssh's `password:` prompt — which became row 08e. PE decisions (2026-09-27):
exit detection first, ghosts use `BatchMode=yes`, the connection-time
handshake gets its own probed row.

**Pre-dispatch check run.** Every grep criterion matches its stated current
value; the six pinned tests filter to 0 (2045 filtered out); two cited line
numbers were wrong and are fixed. The lib count is pinned as a **floor**
(≥ 2043) and the ignored count exactly (9) — the 08a calibration. The E2E
block now records **the names** of any failing test, and says to run with no
other `cargo test` in parallel — both from the 08a review.

**Renumbering.** 08a's doc says "08b" for the ghost PTY move and "08c" for
spawn-ghost-host; those are now **08c** and **08d**. 08c carries two 08a
review residuals.

**Next after 08b:** 08c (ghost-remote-pty), 08d (spawn-ghost-host), 08e
(remote-connect-handshake, probe first), 09 (live close).

Twenty-two rows in M21; live host `pinky` (sudo requires a password since
2026-09-22; ssh is key-auth); `max_turns` 1000.
