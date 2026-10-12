# Bug 2 on phase-07c: the tmux-write gate misses chained calls whose `.args(` starts on the next line

**Severity:** major
**Status:** fixed (08c74b98)
**Filed:** 2026-10-11

## What's wrong

`tmux_write_hits` (`tests/tmux_read_only.rs`) looks for `.args([` /
`.arg(` only on the `Command::new("tmux")` line itself (`n == 0`). If the
opener is not on that line, the span ends immediately. rustfmt's chain
layout puts it on the next line:

```rust
let _ = std::process::Command::new("tmux")
    .args(["send-keys", "-t", "x"])
    .output();
```

4 of the 26 production `Command::new("tmux")` calls use this layout:
`src/webhook/process.rs:216`, `src/daemon/mod.rs:208`,
`src/daemon/utils/host.rs:12` and `src/cli/viewer.rs:143`. **Reviewer
probe:** adding exactly that `send-keys` block to `src/tmux/pane.rs` left
`source_issues_no_tmux_write_verbs` **passing**. The probe was reverted.

(bug-07c-1, the multi-line array on the `Command::new` line, is fixed and
verified by probe.)

## What should happen

The spec, row 5: check the `.args([...])` / `.arg(...)` chain that
*follows* `Command::new("tmux")`, possibly across lines. That includes an
opener on a later line of the same expression.

## Root cause

The opener search runs on the first line only. It should look for the
first `.args(` / `.arg(` that follows `Command::new("tmux")` within the
span window, on any line, and then balance brackets from there.

## Definition of done

- [ ] `cargo test --test tmux_read_only gate_catches_a_chained_write`
      reports `1 passed`. It runs `tmux_write_hits` over the three-line
      `send-keys` block above and expects exactly one hit, `send-keys`.
      Today the test does not exist.
- [ ] `source_issues_no_tmux_write_verbs` still passes on the real tree.
      If it now names one of the four real call sites, report the
      file:line and verb, and do not edit the file.
- [ ] `cargo test` reports `passed=2319 failed=0`.
