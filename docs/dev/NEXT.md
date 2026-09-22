# NEXT

**Active phase:** **none** — M21 phase-04d is `done` (2026-09-22,
`approved_first_try`), and it was verified **live against `pinky`**:

```
record_host=pinky reused=s0 remote_hostname=pinky remote_exit_code=42
```

`run_terminal_command { host? }` ships. **Row 05 — ssh-prompt-detection is
next**; run `/rexymcp:architect next` to draft it.

**Carry into 05's drafting — two spec-side calibrations from 04d's review,
both this architect's, not the executor's.** Pinning a test count *and*
permitting a justified extra, without also requiring the § End-to-end
verification block to be **re-captured as the final act before commit**, let
04d's capture ship one count stale (`1950` recorded, `1951` actual; every
other value correct, and the executor's own notes stated the true number). And
"no new `#[allow]` anywhere else" needed a carve-out for a mirror **test**
helper — scope that ban to production paths.

The account below is the drafting-time record for 04d. Its pre-phase
measurements describe the tree at `117cffe2`, not today's.

**What it adds.** `run_terminal_command { host? }` — the one-shot remote form,
on the plumbing 04c landed. `shell_for_owner` is already host-keyed and
`AiEvent::ToolCall` is already a struct variant, so this row carries a `host`
from the tool schema down to them and answers the question the milestone
reserved for it: **what `shell` and `host` mean together.**

**The semantics are an eight-row table in Task 1**, and its load-bearing line
is the last one: a `shell` id whose shell is on a *different* host than the
one named is **refused, naming both**, never relocated. Silently relocating
would run a command somewhere the model did not ask for — the same failure the
existing ownership check on `ShellTarget::Existing` exists to prevent.
Mutation A deletes that bail and must turn the test red.

**Measured at drafting: the spawn closure is monomorphic.**
`run_in_shell_with_prompts` takes one `spawn: F`, and the local
(`spawn_detached_host`) and remote (`spawn_ssh_host`) closures have different
types, so a single generic call cannot serve both. `open_shell`
(`knowledge/shell.rs:1662-1705`) already solved this with a two-arm `match`,
each arm calling the generic function with its own closure — that code is
quoted in § Current state 2 as the pattern to copy, with an explicit "do not
try to box the closure."

**Host refusals resolve before any spawn.** `HostId::parse` rejects a
malformed name (and maps `"local"` to `HostId::Local`, which is legal);
`host_entry` rejects an unconfigured one with `unknown host "nope"; configured
hosts: pinky`. Both must reach the model as their own sentence rather than
being dressed as shell-limit or timeout advice — `open_shell`'s doc comment
states that rule and the spec quotes it. Mutation B deletes the `host_entry`
call and must turn the unknown-host test red.

**Pre-dispatch check run; every non-preservation criterion confirmed failing:**
`host_params=1` (→2), `event_host=0` (→1), `pending_host=0` (→1),
`reuse_hardcoded=1` (→0), `ignore_total=5` (→6), and the eight named tests
`0 passed; 1948 filtered out`. **One criterion was corrected by that check** —
`grep -c 'host: Option<String>' src/ai/types/events.rs` already read **1**
today, because `AiEvent::OpenShell` has such a field at `events.rs:186`. It
was a vacuous guard; it is now scoped to the `ToolCall` variant with `awk`,
and the spec says why an unscoped grep proves nothing.

**42 tools, 28 core, 14 deferred — unchanged.** `host?` is a parameter, not a
tool, and `tests/doc_truth.rs` is the gate that catches it if the counts move.

**Next after 04d: 05 — ssh-prompt-detection**, then 06 (new-host approval,
which is where the approval panel learns to name the host — deliberately *not*
04d's job), 07, 08, 08b, 09.

Fifteen rows in M21; live host `pinky`; `max_turns` 1000.
