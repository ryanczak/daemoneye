# Bug 1 on phase-07c: the tmux-write gate never scans a multi-line arg list

**Severity:** major
**Status:** fixed (0db1440b)
**Filed:** 2026-10-11

## What's wrong

`tmux_write_hits` (`tests/tmux_read_only.rs`) collects the span after a
`Command::new("tmux")` line, and stops at the first line that contains
`)` or `])`. The `Command::new("tmux")` line itself contains `)`, so the
span always ends on that first line. For the usual layout:

```rust
Command::new("tmux").args([
    "split-window",
    "-h",
]);
```

the literals on the following lines are never checked. In production
source, 20 `Command::new("tmux").args([` calls put their args on later
lines. **Reviewer probe:** adding exactly that `split-window` call to
`src/tmux/pane.rs` left `source_issues_no_tmux_write_verbs` **passing**.
The probe was reverted. `gate_catches_an_abbreviated_write` tests only the
single-line form, so it cannot catch this.

## What should happen

Row 5 of the spec: the gate checks the literals in the `.args([...])` /
`.arg(...)` chain that follows `Command::new("tmux")`, **possibly across
lines**.

## Root cause

The span terminator is tested against the opening line before any arg
line is read. It should scan from the `.args([` / `.arg(` opener to the
**matching** close: the `])` that ends the array, or the `)` that ends
`.arg(`. That close can be on a later line.

## Definition of done

- [ ] `cargo test --test tmux_read_only gate_catches_a_multi_line_write`
      reports `1 passed`. The test runs `tmux_write_hits` over the
      four-line `split-window` block above and expects exactly one hit,
      `split-window`. Today the test does not exist.
- [ ] `source_issues_no_tmux_write_verbs` still passes on the real tree.
      If the fixed scanner now names a real production hit, report it;
      do not edit that file.
- [ ] `cargo test` reports `passed=2318 failed=0`: today's 2317 plus this
      one test.
