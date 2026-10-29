# Phase 03 — `recall_context` honours the configured `[limits]`

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Finding:** M31 `FINDINGS.md` #74

## Goal

The `RecallContext` arm of `dispatch` in
`src/daemon/executor/dispatch_memory.rs` hands
`crate::daemon::context::recall::recall` a hard-coded
`LimitsConfig::default()`. `recall` reads one field from it,
`tool_result_chars`, which caps its output; the default is 16 000. A
user who sets `[limits] tool_result_chars` in `config.toml` therefore
gets every other tool capped at their value and `recall_context` capped
at 16 000.

After this phase, the arm passes the configured `[limits]` to `recall`.
It uses the executor's usual idiom,
`crate::config::Config::load().unwrap_or_default().limits`, so a missing
or unreadable config still means the defaults, as everywhere else in
`src/daemon/executor/`.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Write the test first, and run it red.** Add a `#[cfg(test)]`
   module at the end of `dispatch_memory.rs` with the test
   `recall_context_uses_the_configured_tool_result_chars`. Run it
   against the unfixed arm and paste the failing output into the Update
   Log. The test:
   - points `HOME` at a fresh temp dir, holding `crate::test_home_guard()`
     for the whole test;
   - writes `<HOME>/.daemoneye/etc/config.toml` containing only
     `[limits]` and `tool_result_chars = 200`. Use
     `crate::config::etc_dir()` for the path;
   - writes a session archive at
     `crate::daemon::session::archive_file(id)` holding one turn of about
     4 000 characters. `src/daemon/context/recall.rs`'s tests show the
     archive format (`write_archive`, `make_msg`);
   - calls `dispatch` with a `PendingCall::RecallContext` for that turn
     range, a `Vec<u8>` as `tx`, `Some(id)` as the session, and an
     `ArtifactCtx` built the way `knowledge/memory.rs`'s tests build one;
   - asserts the result contains
     `[…truncated — narrow the turn range or refine the query…]` and is
     under 400 characters. Under the default 16 000 cap the 4 000-char
     turn is returned whole, so the unfixed arm fails both assertions.
2. **The fix.** In the `RecallContext` arm, replace
   `LimitsConfig::default()` with the configured limits, and run the
   test green. `dispatch` is 108 of 150 lines, so this is a one-line
   edit and nothing else moves.
3. **Findings** go in their own `### Findings — <date>` entry, from
   `dispatch_memory.rs` only, within five tool calls, or the single line
   `none`.
4. **Commit** as `fix:`.

## Acceptance criteria

- [ ] `grep -c 'LimitsConfig::default()' src/daemon/executor/dispatch_memory.rs`
      prints `0` (today: `1`).
- [ ] `recall_context_uses_the_configured_tool_result_chars` exists and
      passes, and the Update Log shows it failing before the fix.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2594 failed=0` (2593 + 1).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/daemon/executor/dispatch_memory.rs
echo "default_limits=$(grep -c 'LimitsConfig::default()' $F)"
cargo test recall_context_uses_the_configured_tool_result_chars 2>&1 | grep -E '^test result: ok\. 1 passed'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `default_limits=0`, `test result: ok. 1 passed`, hygiene
`ok. 16 passed`, clippy clean, `passed=2594 failed=0`. Paste the output
into an `### Update — <date> (end-to-end verification)` entry. **Known
flake:** `read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If
the block shows `passed=2593 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/daemon/executor/dispatch_memory.rs`. No other source file.

## Out of scope

- `recall` itself, and the other `Config::load().unwrap_or_default()`
  sites.
- The ghost loop's own `RecallContext` handling in
  `src/daemon/ghost/turn.rs`.

## Update Log

<!-- entries appended below this line -->
