# NEXT

**Active phase:** **M21 phase-08d — spawn-ghost-host**, drafted 2026-09-28,
status `todo`. Dispatch with `/rexymcp:dispatch phase-08d`.

**What it does.** Three ghost-side changes:
1. `spawn_ghost_shell { host? }` — the caller names the specialist's host,
   overriding its runbook `ssh_target`; refused **before any ghost starts**
   unless configured (or `"local"`), granted by the child runbook's `hosts:`,
   and — for a ghost caller — by the caller's `hosts:` too (PE 2026-09-28).
2. A ghost's own `run_terminal_command { host? }` is honoured instead of
   dropped, by rebinding the ghost's policy to that host (`GhostPolicy::for_host`)
   so 08a's fence and 08c's PTY routing apply unchanged (PE: same row).
3. **A measured 08a defect**: a ghost with no `ssh_target` but a non-empty
   `hosts:` list has every background command refused ("ssh_target  is not a
   configured host … whose ssh is , or work on the daemon host"). The fence
   now applies only when there is an `ssh_target`.

**Pre-dispatch check run.** The § End-to-end block was run verbatim against
`6ac74e45`: every non-preservation line reads its current value
(`new_tests=0p-0f`, `spawn_host_param=0`, `for_host=0`, `spawn_target=0`,
`dropped_host=1`, `docs=0-0-0-0`); preservation holds (`refusal_tests=2p-0f`,
`ghost_remote_tests=4p-0f`, `ignore_total=11`). One criterion was vacuous at
first draft — "Ghost" contains the substring `host` — and now pins `host?`.

**Next after 08d:** 08e (remote-connect-handshake, probe first), 09 (live
close).

Twenty-three rows in M21; live host `pinky` (sudo-rs 0.2.13, password
required; ssh is key-auth); `max_turns` 1000.
