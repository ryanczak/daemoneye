# Phase 02 — gate-blind-spots

**Milestone:** M41 — Correctness & reliability
**Status:** todo
**Drafted:** 2026-11-27
**Depends on:** 01 (done)
**Estimated diff:** ~250 lines (tests only)
**Tags:** language=rust, kind=test-gate, size=s

## Goal

Close audit rows **Q4** and **Q5**. Both source gates miss code. The
architect probed both on 2026-11-27 and reverted the scratch probes.

1. **Q4: both gates stop at the first `#[cfg(test)]`.**
   `tests/lock_invariant.rs` (`production_lines`) and
   `tests/tmux_read_only.rs` (`source_issues_no_tmux_write_verbs`) keep
   only the text before a file's first `#[cfg(test)]`.
   - **Probed:** a brace-counting scan found about 290 production items
     below an early `#[cfg(test)]`, for example a
     `#[cfg(test)] mod tests;` near the top. Affected files include
     `src/shell/client.rs`, `src/search.rs`, `src/memory/index/mod.rs`,
     `src/cli/viewer.rs` and `src/daemon/notice_sinks.rs`.
   - Neither gate ever sees that code.
2. **Q5: the lock gate knows only `.lock()`.** A `RwLock`'s
   `.read().unwrap()` / `.write().expect(…)`, and the `try_lock`,
   `try_read` and `try_write` forms, are not checked.

**No live violation hides there.** The architect ran a scratch `syn`
visitor over `src/`. It skipped test-only items and validated itself by
finding 176 `.lock()/.read()/.write().unwrap_or_log()` sites. It found
**no** production guard that is `unwrap`ped or `expect`ed, and no
`"tmux"` literal in production code below a first `#[cfg(test)]`. So both
widened gates must pass on today's tree. Their red tests are fixture
tests.

## Statement of work

1. **Find test code by parsing (both gates).**
   - Replace the `#[cfg(test)]` string split with a function that parses
     the file with `syn` and returns its production lines. Production
     lines are every line outside the line span of a **test-only item**.
   - Test-only items are found at any depth: a module, a free fn, an
     impl's fn, or anything else that carries an attribute.
   - Use `tests/hygiene.rs`'s `is_test_only` rule: `#[test]`,
     `#[…::test]`, or a `cfg` whose predicate is `test` or an
     `all`/`any` holding `test`. Copy the rule into each gate, or share
     it. Do not edit `hygiene.rs`.
   - Keep the gates' existing whole-file exemptions: `tests.rs` /
     `*_tests.rs` and `TEST_CODE_FILES`.
   - A file that fails to parse **fails the gate**, naming the file.
     Never skip it.
2. **Keep the scanners textual.** Each gate keeps scanning the production
   lines as text. `lock_unwrap_hits` and `tmux_write_hits` run on those
   lines with their line numbers unchanged.
   - Do not replace the scanners with an expression visitor. `syn` does
     not parse inside macro invocations (`tokio::select!`, `format!`), so
     a visitor would go blind there.
3. **Widen the lock scan (Q5).**
   - A guard is `.lock()`, `.read()`, `.write()`, `.try_lock()`,
     `.try_read()` or `.try_write()` **with empty parentheses**,
     followed by `.unwrap()` or `.expect(`.
   - The split form also counts: the guard call ends one line, and
     `.unwrap()` / `.expect(` starts the next.

**Closed file list:** `tests/lock_invariant.rs` and
`tests/tmux_read_only.rs`. No production code. If a widened gate flags a
production site, stop and record it in the Update Log instead of editing
`src/`, because the probe says there are none.

**Gotchas:**
- Line numbers come from `Spanned::span().start().line` /
  `.end().line`. The `span-locations` feature that makes them real is
  already on `proc-macro2` in `[dev-dependencies]`. Do not add a
  dependency.
- An item's span covers its outer attributes. A test-only item's span
  therefore starts at its `#[cfg(test)]` line.

**Edit order:** each step ends with
`cargo test --test lock_invariant --test tmux_read_only --no-run`.
1. **Your first tool call is an edit adding the red test**
   `the_gate_scans_code_below_a_cfg_test_item` to `tests/lock_invariant.rs`.
2. The lock gate's parse-based production lines, and the Q5 widening with
   its test.
3. The tmux gate's tests, then its parse-based production source.

**Read limit:** read at most **3** files before your first edit:
`tests/lock_invariant.rs`, `tests/tmux_read_only.rs` and
`tests/hygiene.rs`.

**Budget:** you have **1200 turns**; use them efficiently.

## Tests

The fixtures are Rust source strings, passed to the same production-lines
function and scanner the gate's walk uses.

**`tests/lock_invariant.rs`:**
- `the_gate_scans_code_below_a_cfg_test_item`
  - **Fixture:** `#[cfg(test)] mod t {}` and then
    `fn f() { let g = m.lock().unwrap(); }`.
  - **Asserts:** one hit, on the `lock` line.
  - **Today:** red.
- `the_gate_sees_rwlock_and_try_forms`
  - **Fixture:** one line each for `read().unwrap()`, `write().expect(`,
    `try_lock().unwrap()`, `try_read().unwrap()` and
    `try_write().expect(`, plus a split `.read()` / `.unwrap()` pair.
  - **Asserts:** every one of them is a hit.
  - **Today:** red.
- `the_gate_skips_test_only_items_and_non_guards`
  - **Fixture:** a `#[test] fn` (no `cfg(test)` above it) and a
    `#[tokio::test] async fn`, each with `m.lock().unwrap()`, and a
    `#[cfg(all(test, unix))] mod x { … m.lock().unwrap() … }`.
  - **Production lines that must not hit:** `m.lock().unwrap_or_log()`
    and `file.read(&mut buf).unwrap()`, which has arguments and so is
    not a guard.
  - **Asserts:** no hits.
  - **Today:** red. The leading `#[test] fn` sits above any
    `#[cfg(test)]`, so today's split keeps it.

**`tests/tmux_read_only.rs`:**
- `gate_scans_code_below_a_cfg_test_item`
  - **Fixture:** `#[cfg(test)] mod t {}` and then a production fn whose
    `Command::new("tmux").args(["send-keys", "-t", "x"])` is a write.
  - **Asserts:** one hit.
  - **Today:** red.
- `gate_skips_test_only_items`
  - **Fixture:** the same write inside a `#[test] fn` that sits above
    any `#[cfg(test)]`.
  - **Asserts:** no hits.
  - **Today:** red.

## Acceptance criteria

- [ ] The five named tests pass. Paste the red runs of
      `the_gate_scans_code_below_a_cfg_test_item`, of
      `the_gate_sees_rwlock_and_try_forms` and of the tmux
      `gate_scans_code_below_a_cfg_test_item`, **as output**.
- [ ] Both gates' tree walks pass on today's `src/`:
      `cargo test --test lock_invariant --test tmux_read_only` is green.
- [ ] **No split left:** this prints **0**:
      `grep -c 'split("#\[cfg(test)\]")' tests/lock_invariant.rs tests/tmux_read_only.rs | awk -F: '{s+=$2} END{print s}'`
- [ ] **Mutation run, pasted as output:**
      1. Remove `read` from the lock gate's guard-method list.
      2. Run `cargo test --test lock_invariant` once. It must fail.
      3. Undo that one edit, never by copying a backup file.
- [ ] **No test removed (preservation):** run this once. It prints
      nothing:
      `for f in tests/lock_invariant.rs tests/tmux_read_only.rs; do LC_ALL=C comm -23 <(git show e971ea6f:$f | grep -A1 '^#\[test\]' | grep -oE 'fn [a-z_0-9]+' | sort) <(grep -A1 '^#\[test\]' $f | grep -oE 'fn [a-z_0-9]+' | sort); done`
- [ ] **Counts:**
      - `lock_invariant` reports **5 passed** (2 + 3).
      - `tmux_read_only` reports **6 passed** (4 + 2).
      - The lib suite stays at **3211 passed**.
- [ ] **Gates:** all four pass, each run once.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when the `--no-run` build is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the three red runs and the mutation run, **as output, not
     summarized**;
   - the grep and the comm loop output.

   Then commit as one `test:` commit.

## Authorizations

- You may touch the two files in the closed file list, and no others.

## Out of scope

- `tests/hygiene.rs`, which already parses.
- Any production code, including a site a widened gate flags (record it
  instead).
- New dependencies.

## Update Log

<!-- entries appended below this line -->
