# NEXT

**Active phase:** **M21 phase-08c — ghost-remote-pty**, drafted 2026-09-28,
status `todo`. Dispatch with `/rexymcp:dispatch phase-08c`.

**What it does.** A ghost whose runbook `ssh_target` resolved to a configured
host (08a) runs its `run_terminal_command` in a **PTY shell on that host**,
owned by the ghost and reused for its later commands, opened with
`-o BatchMode=yes` (so ssh never prompts), synchronous with a **300 s** cap,
and **closed when the ghost ends** — both PE decisions of 2026-09-27. An
unresolved `ssh_target` and `backend = "tmux"` keep the 1.x `ssh <target>
'<cmd>'` tmux wrap byte for byte. The branch sits in `run_background` after
the 08a gates, `resolve_command`, the approval and the remote-script build,
so all of those are unchanged.

**Carries four leftovers:** `ghost_host_refusal` back to private and
`refusal_gate_tests`' leaking `HomeGuard` replaced (08a review), the
self-contradictory `run_in_shell_with_prompts` doc comment (08b review), and
a stale "row 08b" comment in `ghost.rs` (now 08d).

**Measured at drafting (pinky, 2026-09-27/28).** BatchMode + ControlMaster:
470 ms cold, 12 ms reused; `remote_stream_cmd` (incl. a 25 KB typed command)
runs with the right exit code in the interactive remote shell; TERM ends an
in-process **ssh** host in ~100 ms and `sh -c 'sleep 1000'` / `cat` in < 1 ms,
but an interactive **bash ignores TERM** — so the close test uses a sleep
host. 08f supplies the fail-fast cancel for an unanswerable password prompt.

**Pre-dispatch check run.** The § End-to-end block was run verbatim against
`b837bef4`: every non-preservation line reads its stated current value
(`new_tests=0p-0f`, `ghost_argv=0`, `ghost_spawn=0`, `timeout=` — file
absent, `routed=0`, `closed_at_end=0`, `leftovers=0-1-1-1`, `readme=0`,
`ignore_total=10`); every preservation line holds (`argv_pins=4p-0f`,
`refusal_tests=2p-0f`, `tmux_wrap=1`); lib green on all three runs.

**Next after 08c:** 08d (spawn-ghost-host), 08e (remote-connect-handshake,
probe first), 09 (live close).

Twenty-three rows in M21; live host `pinky` (sudo-rs 0.2.13, password
required; ssh is key-auth); `max_turns` 1000.
