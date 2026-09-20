# NEXT

**Active phase:** none — **M21 phase-04b is `done`** (2026-09-20,
`approved_after_1`).

**DaemonEye's AI can now ask for a shell on a configured host.**
`open_shell { host: "pinky" }` returns an id, and the existing
`run_terminal_command { shell: "<id>" }` runs there with a real remote exit
code. Re-verified at review against the live host through the tool path:
`record_host=pinky listed_host=pinky remote_hostname=pinky
remote_exit_code=42`, in 1.98 s, leaving no control socket behind.

All gates green on an independent re-run: lib **1935 passed / 0 failed / 5
ignored**, doc_truth 14, integration 32, isolation 9, bug_tracker 6. Tool
counts held at 42 / 28. Both mutations re-applied by the reviewer and both
went red, including the one that proves an unknown host never reaches the
model dressed as shell-limit advice.

**The executor was right and my arithmetic was wrong.** I pinned 1934 tests;
it delivered 1935, adding `open_shell_rejects_a_malformed_host_name_before_any_spawn`
because Task 3b arms the `HostId::parse` path and none of my seven named
tests touched it. It stated the number and the reason instead of trimming a
test to hit the pin — which is what the criterion asked for.

**The hard_fail was my spec's fault.** The first dispatch died on
`VerifierFailurePersistent` partway through the enum wiring; resume, not
re-dispatch, finished it. Second occurrence of a shape whose remedy I already
had in hand and did not write into Task 4. Filed upstream as
`ryanczak/rexyMCP#10` with a replay of all 435 session logs: seven of that
detector's eight fires were wiring sweeps that completed on the next
dispatch, and raising the threshold to 10 would fire on nothing. Threshold
held at 6 by PE decision.

**Row 04c must fix one thing before it adds tests.** The accepted `cfg(test)`
`open_shell_test_seam` never restores the registry it swaps — `swap_in`
returns the previous value, not a guard, and the doc comment above it says
otherwise. Harmless today because all four consumers swap and serialize. It
becomes an order-dependent flake the moment a test calls `open_shell` without
swapping, which 04c will do.

**Next row: 04c — run-command-host.** `run_terminal_command { host? }`, the
one-shot remote form, plus host-keyed reuse in `shell_for_owner` (04b left it
local-only because nothing could ask for a remote shell yet). It has to decide
what `shell` and `host` together mean. Counts stay at 42 / 28.

Run `/rexymcp:architect next` to draft it. Thirteen rows in M21; live host
`pinky`; `max_turns` 1000.
