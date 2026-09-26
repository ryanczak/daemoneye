# NEXT

**Active phase:** **none** — M21 phase-06d is `done` (2026-09-26,
`approved_first_try`). **All four 06 rows are complete**, so the approval and
ghost-boundary half of the milestone is finished. **Row 07 —
remote-file-ops-over-pty is next**; run `/rexymcp:architect next` to draft it.
Four rows remain: 07, 08, 08b, 09.

## RESOLVED 2026-09-26 — the fold already existed; occurrences recorded, not duplicated

The PE said "fold it". Grepping `WORKFLOW.md` first (per the standing rule
from M24 close) showed that **every one of the four M21 occurrences is already
covered** by § "Run every count criterion; never derive it" — its "text-based
greps count prose" corollary, its scoping table (row 1: `sed -n
'1,/^#\[cfg(test)\]/p'` for production-only; row 4: expect a new identifier's
own test names and `use` lines to match), and the M24 paragraph on validating
against the *post-phase* corpus. A fifth rule would have been a duplicate.

What landed instead: an **M21 occurrence trailer** on that section, naming the
four rows against the clauses they each violated and the real defect —
**non-application**. 06d slipped because its criterion was validated against
the pre-phase tree only, which the M24 paragraph explicitly warns about. This
is the second milestone (after M24) in which an existing rule was re-proposed
rather than applied.

## Still open — the persist-to-`[hosts]` offer

Deferred from 06c and unchanged: no config-write path exists, the dependency
is `toml` not `toml_edit`, and a round-trip would discard 96 of the config's
193 comment lines. Needs either a new dependency or an append-only writer.
Nothing blocks 07–09 on it.

## Standing constraints for 07's drafting

- **No `su`+`do`-shaped token** in any command a spec asks the executor to
  run — the shell gate refuses the line. Audit the *runnable blocks*, not the
  whole document (my first audit of 06d made exactly that mistake).
- **§ Authorizations must name** the re-export site for a new `pub fn`, and
  the construction sites a field addition compile-forces. 06d's explicit
  authorization for the latter worked — `foreground.rs` was fixed minimally
  and listed.

**What it closes.** A runbook can already declare `hosts: [web01, db01]` and
the daemon already parses it — into a field whose own doc says **"enforced in
a later phase."** This is that phase, and the same row deletes that sentence.
It is the milestone's *"Ghost `hosts:` is enforced"* exit criterion.

**The compatibility trap, and the mutation that guards it.** `hosts: []` means
**unrestricted**, not "nothing allowed" — every runbook on disk omits the
line, and `runbook.rs:570` already pins that meaning. If this row got it
backwards it would silently break all of them. Mutation A drops exactly that
clause and must turn `an_empty_hosts_list_is_unrestricted` red.

**Two doors, both gated.** `PendingCall::Foreground` (via 06c's
`resolve_foreground_host`) and `PendingCall::OpenShell` (`mod.rs:1040`) can
each reach a host, and neither consults the policy today. A criterion pins
**two** `may_use_host` call sites, and Mutation B removes the `open_shell`
one — with an explicit instruction that if the test stays green, that is a
*finding* about test coverage rather than a pass.

**Pre-dispatch check run; five criteria confirmed failing** — `policy_field=0`→1,
`from_config_copies=0`→1, `predicate=0`→1, `both_gates=0`→2,
`stale_claim_gone=1`→0 — with `lib_counts=1994p-0f-6i`→2004,
`ignore_total=6` preserved, and the ten named tests reading
`0 passed; 2000 filtered out`.

**All three standing calibrations applied, and the last one caught me again.**
Every structural grep is `awk`-scoped and each range was verified to anchor on
real text (19, 8 and 6 lines). § Authorizations pre-emptively addresses the
re-export question (none needed — `executor/mod.rs:10` already imports
`GhostPolicy` directly) and the field-addition construction sites. And on the
`su`+`do`-token audit: my **first audit grep was itself over-broad**, matching
the whole document and flagging two hits that turned out to be a quoted
`rust` block and one prose line. Re-audited against the runnable blocks only
— **all three `sh` blocks clean, zero runnable commands carry the token.**
That is the third time in three rows that an over-broad match has bitten,
twice now in my own verification rather than in a criterion.

**Next after 06d: 07 — remote-file-ops-over-pty**, then 08, 08b, 09. When 06d
lands, every 06 row is done and the approval/ghost half of the milestone is
complete.

Eighteen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
