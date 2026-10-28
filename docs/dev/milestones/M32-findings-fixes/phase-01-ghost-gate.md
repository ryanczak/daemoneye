# Phase 01 — The ghost gate fails closed

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-28
**Finding:** M31 `FINDINGS.md` #84

## Goal

`run_terminal_command`'s foreground arm (`src/daemon/executor/dispatch_foreground.rs`)
calls `Config::load().unwrap_or_default()` twice per call: once in
`dispatch` for the ghost gate's `sandbox.enabled`, and once in
`run_approved` for `shells.max_per_owner`. `Config::load` returns `Ok`
with the defaults when `config.toml` is missing; it is `Err` only when the
file exists and cannot be read or parsed. In that case the gate sees
"sandbox off", and `ghost_may_run_foreground(true, false)` is `true`, so
a ghost with a broken config is let through. Nothing reaches it today (a
ghost session never takes the PTY path and is refused further on), but
the gate itself fails open.

After this phase:
- The config is loaded **once** per foreground call, in `dispatch`, and
  `run_approved` reads `max_per_owner` from that same load.
- A **ghost** with an unreadable config is refused at the gate, and the
  refusal names the load error.
- A **non-ghost** call keeps today's behaviour: an unreadable config
  falls back to the defaults.

## Architecture references

- `CLAUDE.md` § "Sandbox (Docker Sandboxes / sbx)": the sandbox fails
  closed. This brings the foreground gate in line.
- M32 README, decisions 1 and 6.

## Spec

1. **Write the gate as a pure function, and test it first.** Beside
   `ghost_may_run_foreground` in `src/daemon/executor/mod.rs`, add
   `ghost_foreground_refusal(is_ghost: bool, config: &anyhow::Result<Config>) -> Option<String>`.
   It returns the refusal text to send, or `None` when the call may go
   on:
   - not a ghost → `None`, whatever the config;
   - a ghost, config `Err(e)` → `Some`, a refusal naming `e`;
   - a ghost, config `Ok` with `sandbox.enabled` → `Some`, today's refusal
     text, unchanged;
   - a ghost, config `Ok` with the sandbox off → `None`.

   Add the four tests below to the `mod.rs` test module that holds the
   `ghost_may_run_foreground_*` tests. Run them before wiring the
   function in, and confirm the `Err` test fails or does not compile;
   paste that output.
2. **Load once and wire it.** In `dispatch`, call `Config::load()` once,
   keep the `Result`, and use `ghost_foreground_refusal` for the gate.
   Give `ApprovedRun` a `max_per_owner: u32` field, set from the same
   load (`Ok` → its value, `Err` → `Config::default()`'s), and delete
   `run_approved`'s own load. `ApprovedRun` must stay `Copy`.
3. **Findings** go in their own `### Findings — <date>` entry, written
   from `src/daemon/executor/dispatch_foreground.rs` and the new function
   only, within five tool calls, or the single line `none`.
4. **Commit** as `fix:`.

## Acceptance criteria

- [ ] `grep -c 'Config::load' src/daemon/executor/dispatch_foreground.rs`
      prints `1` (today: `2`).
- [ ] These four tests exist and pass:
      `ghost_foreground_refusal_refuses_a_ghost_when_the_config_is_unreadable`
      (the text contains the error's message),
      `ghost_foreground_refusal_refuses_a_ghost_when_the_sandbox_is_on`,
      `ghost_foreground_refusal_allows_a_ghost_when_the_sandbox_is_off`,
      `ghost_foreground_refusal_allows_a_non_ghost_when_the_config_is_unreadable`.
- [ ] The Update Log shows the `Err` test failing before the fix.
- [ ] `cargo test` reports **exactly** `passed=2591 failed=0` (2587 +
      the four tests). `tests/hygiene.rs` passes with no allowlist edit.
- [ ] fmt, build and clippy are clean.

## Test plan

The four unit tests above. An unreadable config is
`Err(anyhow::anyhow!("…"))` built in the test. The tests must not read
or write a real `config.toml`.

## End-to-end verification

```sh
bash <<'E2E'
F=src/daemon/executor/dispatch_foreground.rs
echo "loads=$(grep -c 'Config::load' $F)"
cargo test ghost_foreground_refusal 2>&1 | grep -E '^test result: ok\. [1-9]'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `loads=1`, a `test result: ok. 4 passed` line, clippy clean,
`passed=2591 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the run
shows `passed=2590 failed=1`, run it once more and paste both outputs.

## Authorizations

- Edit `src/daemon/executor/dispatch_foreground.rs` and
  `src/daemon/executor/mod.rs`.

## Out of scope

- Changing what a non-ghost does with an unreadable config.
- `ghost_may_run_foreground` itself: keep it, and its four tests.
- The other `Config::load().unwrap_or_default()` sites in the tree.

## Update Log

<!-- entries appended below this line -->
