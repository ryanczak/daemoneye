# NEXT

**Active phase:** **M21 phase-06d — ghost-hosts-enforced**, drafted
2026-09-26, status `todo`. Dispatch with `/rexymcp:dispatch phase-06d`.
**This is the last of the 06 rows.**

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
