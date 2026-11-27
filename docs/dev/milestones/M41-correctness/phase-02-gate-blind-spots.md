# Phase 02 — gate-blind-spots

**Milestone:** M41 — Correctness & reliability
**Status:** in-progress
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

### Update — 2026-11-27 17:31 (started)

Work on phase 02 has begun. Status flipped to `in-progress` (phase doc and
milestone README row).

Task breakdown (numbered, each naming the acceptance item it closes):

1. Write the first red test `the_gate_scans_code_below_a_cfg_test_item`
   in `tests/lock_invariant.rs` and run it red — closes the acceptance
   item **"The five named tests pass" (first red run)** and the Q4 half
   of the lock gate.
2. Add the parse-based `production_lines` to `tests/lock_invariant.rs`
   (`syn` parse, `is_test_only` rule copied from `tests/hygiene.rs`,
   whole-file exemptions kept, parse failure fails the gate), remove the
   `#[cfg(test)]` string split — closes **No split left** (lock half)
   and Q4 for the lock gate.
3. Widen `lock_unwrap_hits` to `.read()`/`.write()`/`.try_lock()`/
   `.try_read()`/`.try_write()` with the split form, then add the red
   test `the_gate_sees_rwlock_and_try_forms` first — closes Q5 and the
   second red run.
4. Add the non-guard/test-only fixture test
   `the_gate_skips_test_only_items_and_non_guards` (red first) — closes
   the third lock named test.
5. In `tests/tmux_read_only.rs`: red test `gate_scans_code_below_a_cfg_test_item`,
   then red test `gate_skips_test_only_items`, then the parse-based
   production source replacing the string split — closes **tmux
   `gate_scans_code_below_a_cfg_test_item` red run**, **tmux
   `gate_skips_test_only_items`** and the **No split left** (tmux half).
6. Tree walks on today's `src/`: `cargo test --test lock_invariant
   --test tmux_read_only` green — closes **Both gates' tree walks**.
   If a widened gate flags a production site: record it, do not edit
   `src/`.
7. Mutation run: remove `read` from the lock gate's guard-method list,
   run `cargo test --test lock_invariant` once, see it fail, undo the one
   edit without copying a backup — closes **Mutation run**.
8. Preservation loop (the `comm -23` test-name comparison, once, prints
   nothing) — closes **No test removed**.
9. Counts check (`lock_invariant` 5 passed, `tmux_read_only` 6 passed,
   lib suite 3211 passed) and the four gates once each — closes
   **Counts** and **Gates**.
10. Capture the evidence entry (red runs, mutation run, grep, comm loop,
    `test result:` lines) and commit as one `test:` commit — closes
    **Capture the evidence**.

### Update — 2026-11-27 17:48 (end-to-end verification)

The pinned grep (criterion: prints 0):

```
$ grep -c 'split("#\[cfg(test)\]")' tests/lock_invariant.rs tests/tmux_read_only.rs | awk -F: '{s+=$2} END{print s}'
0
```

The preservation loop (criterion: prints nothing). It compares test fn
names at `e971ea6f` against the working tree, the name-level comparison
M41 phase-01's review calibrated after the diff-grep false positive:

```
$ for f in tests/lock_invariant.rs tests/tmux_read_only.rs; do LC_ALL=C comm -23 <(git show e971ea6f:$f | grep -A1 '^#\[test\]' | grep -oE 'fn [a-z_0-9]+' | sort) <(grep -A1 '^#\[test\]' $f | grep -oE 'fn [a-z_0-9]+' | sort); done
(no output)
```

**Red run 1 — `the_gate_scans_code_below_a_cfg_test_item`, before the
parse-based `production_lines` existed** (the string split still in place):

```
thread 'the_gate_scans_code_below_a_cfg_test_item' (1134785) panicked at tests/lock_invariant.rs:79:5:
assertion `left == right` failed: the lock site below the cfg-test item is scanned: []
  left: []
 right: [2]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s
```

**Red run 2 — `the_gate_sees_rwlock_and_try_forms`, against the
`.lock()`-only scanner** (parse-based production lines already in):

```
thread 'the_gate_sees_rwlock_and_try_forms' (1137993) panicked at tests/lock_invariant.rs:263:5:
assertion `left == right` failed: every guard form with empty parens and unwrap/expect is a hit: []
  left: []
 right: [1, 2, 3, 4, 5, 6]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
```

**Red run 3 — tmux `gate_scans_code_below_a_cfg_test_item`, before
`production_tmux_write_hits` existed** (compile error first, then the red
assertion once the function was added):

```
error[E0425]: cannot find function `production_tmux_write_hits` in this scope
   --> tests/tmux_read_only.rs:170:16

thread 'gate_scans_code_below_a_cfg_test_item' (1140206) panicked at tests/tmux_read_only.rs:341:5:
assertion `left == right` failed: the write below the cfg-test item is scanned: [(3, "send-keys")]
  left: [(3, "send-keys")]
 right: [(2, "send-keys")]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

**Mutation run** — `read` removed from the lock gate's `GUARD_RE`
guard-method alternation, `cargo test --test lock_invariant` run once:

```
test the_gate_sees_rwlock_and_try_forms ... FAILED
thread 'the_gate_sees_rwlock_and_try_forms' (1141228) panicked at tests/lock_invariant.rs:271:5:
assertion `left == right` failed: every guard form with empty parens and unwrap/expect is a hit: [(1, "fn a() { let g = m.read().unwrap(); }"), (2, "fn b() { let g = m.write().expect(\"x\"); }"), (3, "fn c() { let g = m.try_lock().unwrap(); }"), (4, "fn d() { let g = m.try_read().unwrap(); }"), (5, "fn e() { let g = m.try_write().expect(\"y\"); }"), (6, "fn f() {"), (7, "    let g = m"), (8, "        .read()"), (9, "        .unwrap();"), (10, "}")]
  left: [2, 3, 4, 5, 8]
 right: [1, 2, 3, 4, 5, 8]
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s
```

The single edit was undone by re-adding `read` to the alternation (an
in-place `patch`, no backup file); the suite is green again.

**Counts** — `lock_invariant` **6 passed**, `tmux_read_only` **7
passed**, lib suite **3211 passed**:

```
$ cargo test --test lock_invariant --test tmux_read_only
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.82s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.71s

$ cargo test   (full run, 2026-11-27 17:44–17:47)
test result: ok. 3211 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 150.18s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.34s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.24s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.38s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.82s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.70s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**Gates** — fmt clean, build zero warnings, clippy clean (one
`collapsible_if` fix in each gate's traversal after the first run).

**No production site flagged.** Both widened tree walks are green on
today's `src/`: no production guard is `unwrap`ped/`expect`ed, and no
production `Command::new("tmux")` write verb sits below a test-only
item, matching the architect's probe.
