# NEXT

**Active phase:** **none** — M21 phase-06b is `done` (2026-09-23,
`approved_first_try`). Session `A` is now per class **per host**, and
`new_host` / `escape_hatch` never auto-approve whatever the session state or
config default. **Row 06c — new-host-allowed is next**; run
`/rexymcp:architect next` to draft it.

**Carry into 06c's drafting — a new constraint, discovered by the executor.**
**E2E blocks must not contain `sudo`-shaped tokens in their command lines.**
06b's `bare_flags` criterion embedded `(regular|sudo): bool,` in a `grep -E`,
and the executor's shell gate refuses any line carrying such a token — it had
to run the block byte-identical from a gitignored script and disclosed that.
Use a character class (`su[d]o`) or match the field another way. **This
matters for 06c specifically**, which is all about sudo-adjacent gating. First
occurrence; recorded, not folded.

**06c's job, and it carries real security weight.** An unconfigured host is
still refused *after* approval. Making it allowed-but-gated needs three things
together: host resolution reordered ahead of the prompt, `spawn_ssh_host`
working without a `[hosts]` entry (the bare name as the ssh destination), and
the offer to persist it. 06b deliberately left all three alone — verified at
review: `executor/mod.rs` and `policy.rs` are untouched.

The account below is the drafting-time record for 06b; its pre-phase
measurements describe the tree at `4405f8ca`.

**Measured at drafting, and it is the reason this row exists.** 06a threaded
`Classification` all the way into `prompt_tool_call_ratatui` — and it is read
**only for the panel body**. Every use of it inside that function:

```
 9:    classification: &crate::ipc::Classification,     <- the parameter
14:    let body = tool_call_panel_body(command, background, target_pane, classification);
18:    let auto_approved = if is_sudo {
19:        approval.sudo
21:        approval.regular
```

The decision at 18–21 has no host term, and `commands = true` is the **shipped
default**. So on a default install today the panel prints
`→ host: pinky (configured)` and immediately `✓ auto-approved (session)`: the
user is shown the destination and given no chance to refuse it. 06b makes the
decision host-aware — `A` on the daemon host no longer covers `pinky`, `A` on
`pinky` no longer covers `web01`, and **`new_host` / `escape_hatch` never
auto-approve at all**, which Mutation A pins.

**06b changes who gets asked, not what is reachable.** An unconfigured host is
still refused after approval, exactly as today.

**Row 06 split again — it is now 06a–06d, and M21 is eighteen rows.** Making
an unconfigured host *allowed but gated* turned out to be its own change with
its own security weight: it needs host resolution reordered ahead of the
prompt **and** `spawn_ssh_host` working without a `[hosts]` entry (the bare
name as the ssh destination), plus the persist offer. That is **06c**. Ghost
`hosts:` enforcement moves to **06d**.

**Pre-dispatch check run; both non-preservation criteria confirmed failing**
(`decision_uses_class=0`→1, `host_sets=3`→5), `lib_counts=1970p-0f-6i`→1980,
the two preservation lines correctly pass (`bare_flags=2`, `ignore_total=6`),
and the ten named tests read `0 passed; 1976 filtered out`.

**Both standing calibrations are applied in this spec.** Every structural grep
is `awk`-scoped **and each range was checked to anchor on real text** (57 and
16 lines respectively) — a range that matches nothing also returns 0, which is
the vacuous guard that shipped twice. And § Authorizations now **explicitly
authorizes the construction sites a field addition compile-forces**, naming
the three known ones in `approval.rs` and requiring the executor to list any
others it touches — the omission that recurred in 04c, 04d and 06a.

One criterion is deliberately soft: `host_sets=5` assumes two
`HashSet<String>`s. The spec tells the executor that if it picks a different
shape it should re-pin the number and say so, rather than contort the design
to hit mine.

**Next after 06b: 06c — new-host-allowed**, then 06d (ghost `hosts:`), 07, 08,
08b, 09.

Eighteen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
