# NEXT

**Active phase:** **M21 phase-06c — new-host-allowed**, drafted 2026-09-23,
status `todo`. Dispatch with `/rexymcp:dispatch phase-06c`.

**What it changes.** A well-formed but unconfigured host is **gated, not
refused**. The two cases separate:

| Case | Today | After 06c |
|---|---|---|
| malformed (`"bad host!"`) | refused **after** approval | refused **before** the prompt |
| well-formed, unconfigured (`"nope"`) | refused after approval | classified `new_host`, asked, and on approval connected to |

On approval the bare name becomes its own ssh destination — not a hack:
`HostEntry.ssh`'s own doc says a bare `Host` alias is valid, and the live
`[hosts.pinky] ssh = "pinky"` proves it.

**`open_shell { host }` keeps the strict lookup**, deliberately, with its own
criterion. It is an explicit "give me a shell there"; requiring configuration
for that is the boundary this row draws.

**The persist-to-`[hosts]` offer is deferred, and needs a PE decision.**
Measured at drafting: the daemon has **no config-write path** — `grep` finds
exactly one write, `seeds.rs:50`, which lays down the whole bundled template
on first run. The dependency is `toml = "1.1"`, **not `toml_edit`**, and a
deserialize→serialize round-trip discards comments:

```
$ wc -l ~/.daemoneye/etc/config.toml    → 193
$ grep -c '^#' ~/.daemoneye/etc/config.toml → 96
```

Half the user's config is comments. Persisting a host therefore needs either a
new `toml_edit` dependency or an **append-only text writer** (a
`[hosts.<name>]` table at EOF is valid TOML and preserves everything above).
Both are defensible; picking one is not this row's call.

**Pre-dispatch check run; three criteria confirmed failing** —
`lib_counts=1984p-0f-6i`→1994, `permissive_exists=0`→1,
`run_path_permissive=0`→1 — with three preservation lines correctly passing
(`open_shell_stays_strict=0`, `strict_lookup_intact=1`, `ignore_total=6`), and
the ten named tests reading `0 passed; 1990 filtered out`.

**The check caught two of my own defects again.** A criterion counting
`host_entry` inside `run_in_shell_core` read **2**, because one hit is a
*comment* — the over-broad-match class one level in, this time inside a
correctly-scoped range. And with it wrong, only **one** criterion actually
failed, so the structural criteria proved nothing about the row's real work.
Both replaced: the greps now match the **call** (`host_entry_or_bare(name)`)
rather than the word, and a new `permissive_exists` criterion pins the new
function. **Scope the range *and* match a token that cannot appear in prose.**

**The `su`+`do`-token constraint from 06b is applied and audited.** No command
in this spec contains the blocked token — verified by grep against the
finished doc, not assumed.

**Next after 06c: 06d — ghost `hosts:` enforcement**, then 07, 08, 08b, 09.

Eighteen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
