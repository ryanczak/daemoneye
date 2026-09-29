# NEXT

**Active phase:** **M21 phase-08e — remote-connect-handshake**, drafted
2026-09-29, status `todo`. Dispatch with `/rexymcp:dispatch phase-08e`.

**What it does.** Before the first shell to a configured host spawns, the
daemon connects first: if `ssh -O check` finds no ControlMaster, it runs
`ssh -M -N -f` in a PTY it drives, answers a password / passphrase prompt
through the caller's transport, refuses an unknown host key, and only then
lets the shell spawn as a multiplexed client that can never prompt. All four
remote callers do it — `run_terminal_command`, `read_file`, `edit_file`,
`open_shell` — reversing 07a's "a read must not prompt" rule (PE,
2026-09-29); a ghost connects with `BatchMode=yes`. Also fixes 08d's doubled
`Error: Error:` prefix.

**Measured at drafting (pinky).** `-M -N -f` key auth: child exits in 81 ms,
the master survives the PTY being dropped, a later shell muxes in 6 ms;
`ssh -O check` 2–3 ms (0 warm / 255 cold); a `LocalCommand` marker was
tried and rejected — it never fires through a warm master. Password retries
arrive as one chunk `Permission denied, please try again.` + a fresh prompt,
so failure is decided by the process exiting, never by that text. A fake-ssh
`sh -c` fixture driven by `PtyShell` works 3/3.

**Pre-dispatch check run.** The § End-to-end block was run verbatim against
`66bb7e50`: every non-preservation line reads its current value
(`new_tests=0p-0f`, `pieces=0-0-0-0`, `callers=0-0-0-0`, `read_comment=1`,
`double_error=1`, `readme=0`, `ignore_total=11`); neighbours hold
(`0 failed` ×3); lib green on all three runs. One criterion was wrong at first
draft (the helper's definition does not match `connect_named_host(`) and now
pins the definition separately.

**Next after 08e:** 09 (live close) — the milestone's exit criterion end to
end.

Twenty-three rows in M21; live host `pinky` (sudo-rs 0.2.13, password
required; ssh is key-auth, password auth also accepted); `max_turns` 1000.
