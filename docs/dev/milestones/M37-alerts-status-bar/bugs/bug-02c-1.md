# Bug 1 on phase-02c: the grants segment is no longer flush right; hosts double-count; detail order

**Severity:** major
**Status:** fixed (phase-02c re-dispatch, 2026-11-14)
**Filed:** 2026-11-14

> **Green gates and a clean tree are EXPECTED here and are NOT evidence the
> phase is done.** fmt, build, clippy and `cargo test` (2947 passed) are all
> green today. Already approved, do not redo: the `GrantSummary` types in
> `src/cli/status_grants.rs`, the glyph fields in all three dialects, the
> `approval_hint` → `grants` switch, `hint()`'s deletion, the lead colour,
> the active-kind count, the rank renumbering, the docs, and the seven
> existing grants tests. **There are exactly three defects left**, listed
> below.

## What's wrong

1. **Not flush right (major).** The spec (§ Spec 4: "The segment stays
   flush right, as `join_slots` places it today") and the PE's decision both
   require the grants segment at the pane's right edge. `status_bar_line`
   (`src/cli/status_bar.rs`) now pushes `grants_parts(...)` as an ordinary
   last slot and calls `join_slots(&slots, &mut None, sep, width)`, so the
   segment trails the session slot behind a ` · ` separator. Observed at 160
   columns with gated cmds, one named agent and schedules:

   ```
   width=104 text="💤 · 🌓20% 📈3.1k · … · ⟡ sonnet-4-6 · ⚡3 🔒 🤖1 📅"
   ```

   The line is 104 columns in a 160-column pane, and the segment sits
   mid-line after a `·`.

2. **Hosts double-count (minor).** `SessionApproval::summary()`
   (`src/cli/commands/approval.rs`) sets
   `hosts: (self.hosts_regular.len() + self.hosts_sudo.len()) as u32`. The
   spec says "distinct hosts in `hosts_regular ∪ hosts_sudo`": a host
   granted both regular and sudo commands counts 2, not 1.

3. **Detail order (minor).** The spec's table order is gated, sudo,
   scripts, runbooks, files, **schedules, agents**, deletes, hosts.
   `grants_parts` (`src/cli/status_grants.rs`) emits agents (inside the
   scope loop) before schedules. The probe above shows `🤖1 📅`; it must be
   `📅 🤖1`.

## What should happen

1. At any width where the grants core shows, the line measures exactly the
   pane width, and the grants segment's last glyph is the last visible
   character, with padding before it, as the old `hint` was placed. The
   drop order is unchanged: detail at rank 8, core at rank 13.
2. `hosts` counts the distinct names across both sets.
3. Detail parts follow the table's order.

## Root cause

1. The old code passed the hint to `join_slots` as its `hint` argument
   (`src/cli/status_bar_fit.rs`, `join_slots`, the `if let Some(h) = hint`
   branch that pads the gap) and measured it through `line_fits`'s
   `hint_width`. The switch to `grants_parts` made the segment a `Vec<Part>`,
   which that single-`Part` path cannot carry, so it went into `slots` and
   the flush-right branch became dead (`&mut None`). No test measures the
   line's width with grants present, so nothing caught it.
2. `len() + len()` sums the two sets instead of taking their union.
3. Agents sits inside the scope loop with scripts, runbooks and files, and
   schedules is pushed after the loop.

## Definition of done

Each of these FAILS against the current tree (architect-verified with a
throwaway probe, 2026-11-14):

- [x] A new test `grants_sit_flush_right` in `src/cli/status_bar_tests.rs`:
      with a non-default `GrantSummary` and a line shorter than the pane,
      `line_width(&m, now, 160) == 160`, and the line text, trimmed of
      trailing whitespace, ends with the grants segment's last detail glyph.
      Also check 120 columns. Today the width is 104.
- [x] `grants_detail_drops_before_the_core` and
      `status_line_drops_tier_three_before_tier_two_before_the_grants_core`
      still pass unchanged: flush right must not change the drop order.
- [x] `summary_counts_each_kind` (or a sibling) pins that a host present in
      both `hosts_regular` and `hosts_sudo` gives `hosts == 1`. Today: 2.
- [x] A test pins `📅` before `🤖` when both schedules and agents are
      granted. Today 🤖 comes first.
- [x] Mutation check, pasted with its `test result:` line: make the join
      drop the flush-right padding again and confirm `grants_sit_flush_right`
      fails, then revert.
- [x] fmt, build (0 warnings), clippy clean; `cargo test` reports
      `failed=0` with **more than 2947** passed (the new tests add to it).
</content>
</invoke>
