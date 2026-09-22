# NEXT

**Active phase:** **M21 phase-04c — host-keyed-reuse**, drafted 2026-09-22,
status `todo`. Dispatch with `/rexymcp:dispatch phase-04c`.

**04c split at drafting, and the split is what the measurement bought.** The
queued row was "`run_terminal_command { host? }` plus host-keyed reuse".
Measuring the tree turned that into two rows: **04c is plumbing with no tool
surface**, **04d is the tool surface on top of it**. This is the third time an
04-row has split on a measured seam, and the same shape row 03 used before
04a spent it.

**The measurement found a live defect, not just a seam.**
`default_shell_for` (`src/daemon/executor/knowledge/shell.rs:210`) documents
itself as a *"pure mirror of `shell_for_owner`'s step-(1) reuse predicate"*.
It is not: `shell_for_owner` filters `record.host == HostId::Local`, and the
mirror never reads `row.host` at all, though `ShellRow` carries it. A
throwaway probe (added, run, reverted) returned:

```
PROBE default_shell_for(remote-only rows) = Some("s1")
```

so a session whose only shell is remote gets this line in every prompt:

```
[DEFAULT SHELL] s1 — run_terminal_command with no shell argument lands here
```

which is **false** — a bare `run_terminal_command` skips `s1` and spawns a new
local shell. 04b shipped the surface that makes remote shells reachable; this
is the prompt lying about them. Task 4 fixes it, and a mutation pins it.

**Two hazards repaired before the surface lands on them.** `AiEvent::ToolCall`
is a six-field positional tuple; 04d would add `host: Option<String>` directly
beside `shell: Option<String>`, which transposes silently and compiles. 04c
converts it to a struct variant first (4 sites). And 04b's review recorded the
`open_shell_test_seam` flaw — `swap_in` returns the previous value rather than
an RAII guard, so the global is never restored, and the swapped registry is
`Box::leak`ed — with the instruction *"Fix it there, as a real guard, before
adding `run_terminal_command { host }` tests."* 04c adds exactly those tests,
so Task 1 is that fix.

**Pre-dispatch check run, every non-preservation criterion confirmed failing:**
`lib_counts=1935p-0f-5i` (target 1943), `positional_toolcall=4` (target 0),
`hardcoded_local=1` (target 0), `static_return=2` (target 0), and the
eight named tests `0 passed; 1940 filtered out`. Two criteria were
**corrected** during that check — a `Box::leak` grep that would have dragged
three out-of-scope `AtomicBool` helpers into the phase, and an eight-name
`cargo test` invocation that fails with `error: unexpected argument` because
cargo takes a single `TESTNAME` (the filters need the `--` separator).

**Next after 04c: 04d — run-command-host.** The `ParamDef`, the `PendingCall`
field, `ShellTarget`'s host, the dispatch arm, the three doc rows, and the
live `pinky` capture. 42 tools unchanged — `host?` is a parameter, not a tool.

Fifteen rows in M21; live host `pinky`; `max_turns` 1000.
