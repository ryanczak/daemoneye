# Phase 09: Error-Suppress Audit

**Milestone:** M1 — Agent Tooling Improvements
**Status:** todo
**Depends on:** none (standalone; can follow any of 05–08 — touches different code paths)
**Estimated diff:** ~150–300 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Audit every `unwrap()`, `expect()`, `panic!()`, `unsafe` block, and
`#[allow(...)]` attribute in production code paths and eliminate or justify
each one. The current codebase has ~312 raw hits across those patterns in
non-`_tests.rs` source files, but most are inside embedded `#[cfg(test)]`
sections and are exempt. This phase resolves every hit in a genuine production
path — either by converting it to a proper error-propagation form, by adding a
`// SAFETY:` comment on a necessary `unsafe` block, by removing dead code, or
by recording a one-line justification comment on the rare allows that are
genuinely load-bearing.

## Architecture references

Read before starting:

- `src/util.rs` — the `UnpoisonExt` trait (`unwrap_or_log()`) is the established
  pattern for mutex lock sites; **mutex locks must use `.unwrap_or_log()`**, not
  `.unwrap()` or `.expect()`.
- `docs/dev/STANDARDS.md §2.1` — error-handling rules (typed results, no silent
  swallow, propagation operator is the default).
- `docs/dev/STANDARDS.md §1` — the DoD forbids error-suppressing idioms in
  production paths; test code is explicitly exempt.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read `src/util.rs` (`UnpoisonExt` + `unwrap_or_log`).
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Run the classification grep in §"Current state" to produce your working
   inventory before making any edits.

## Current state

The raw grep (all non-`_tests.rs` source files, non-comment lines):

```
grep -rn --include='*.rs' -E '\.(unwrap|expect)\(|panic!\(' \
  $(find src -name '*.rs' | grep -v '_tests.rs') | grep -v '^\s*//'
```

reports ~312 hits. The same command for `unsafe`:

```
grep -rn --include='*.rs' 'unsafe' \
  $(find src -name '*.rs' | grep -v '_tests.rs') | grep -v '^\s*//'
```

reports ~62 hits. `#[allow(...)` `:

```
grep -rn --include='*.rs' '#\[allow(' \
  $(find src -name '*.rs' | grep -v '_tests.rs')
```

reports ~30 hits.

**However, the large majority are in embedded `#[cfg(test)]` blocks inside
production files** — not genuine production paths. When you encounter a hit,
confirm whether the surrounding function or block is gated by `#[cfg(test)]`,
`#[test]`, or is otherwise exclusively exercised in test contexts. Test-embedded
occurrences are **exempt and must not be changed**.

### Known production-path hits

The following have been pre-verified to be in production code paths (not test
sections). Re-verify line numbers before editing — the tree moves.

#### `unsafe` blocks — justified FFI (document only, do NOT remove)

| File | Line approx | What | Why |
|---|---|---|---|
| `src/main.rs` | ~257 | `libc::fork()` + `libc::setsid()` | Must run before the tokio runtime starts; forking a live multi-threaded runtime is unsound. Documented in CLAUDE.md invariant. |
| `src/cli/render.rs` | ~202, ~219 | `libc::ioctl(TIOCGWINSZ)` | Terminal dimension query; no safe alternative for live values from `TIOCGWINSZ`. |
| `src/daemon/server.rs` | ~384 | `libc::kill(libc::getpid(), libc::SIGTERM)` | Graceful self-signal; no safe wrapper in the Rust stdlib. |

These three blocks are **correct and must stay**. The fix is to add a `// SAFETY:`
comment above each one explaining *why* the invariants required by the unsafe code
hold. Use the "Why" column above as the basis.

The remaining ~59 `unsafe` hits are `std::env::set_var("HOME", ...)` calls inside
`#[cfg(test)]` sections in `server.rs`, `runbook.rs`, `search.rs`, `briefing.rs`,
`mailbox.rs`, `scripts.rs`, `cli/commands/costs.rs`, and others. These are **test
code — do not touch them**.

#### `#[allow(...)]` — classified

| Kind | Count | Action |
|---|---|---|
| `dead_code` | ~10 | Remove the unused symbol; if actively kept for future use, add a one-sentence `// Kept for <reason>` doc comment and drop the `#[allow]` (let it fail clippy as a reminder) — or keep the `#[allow]` with a justification comment. Prefer removal. |
| `deprecated` | ~6 | Fix the underlying deprecated API call. Sites are in `src/scheduler.rs` (≈ lines 152, 587, 745) and `src/daemon/executor/schedule.rs` (≈ line 44) and `src/daemon/scheduled.rs` (≈ lines 39, 162). Identify the deprecated call, find the replacement, update. |
| `too_many_arguments` | ~8 | These mask large function signatures. **Do not refactor the function signatures** in this phase (that is wide blast radius and out of scope). Instead, add a one-line justification comment directly above the `#[allow]` explaining why the function can't be split or take a struct at this time: e.g. `// TODO(M2): consolidate into a params struct`. |
| `clippy::large_enum_variant` on `ipc.rs` | 1 | The existing inline comment already justifies it (`DaemonStatus is large by design`). Leave it — it is correct. |

#### `unwrap()` / `expect()` / `panic!()` in production paths

Do **not** attempt to fix every unwrap in the codebase. The goal is to eliminate
the ones that can actually panic at runtime in a production path with no recovery.
After excluding test-section hits, work through the remaining production-path
occurrences file by file using the following classification:

**Class A — mutex locks → convert to `.unwrap_or_log()`**
Any `.lock().unwrap()` or `.lock().expect(...)` in production code must use
`.unwrap_or_log()` from `crate::util::UnpoisonExt`. This is the established
project pattern (CLAUDE.md: "All mutex lock sites use `.unwrap_or_log()`").

**Class B — `Option` dereference where `None` would panic → convert to `?` or
explicit error**
`.unwrap()` on an `Option` or `Result` where `None`/`Err` is a plausible runtime
condition. Convert to `?` propagation (preferred), `.ok_or_else(|| ...)`, or
`.unwrap_or(default)` where a default is semantically correct.

**Class C — `unwrap()` / `expect()` on a value that is provably `Some`/`Ok` by
construction → add a justification comment**
If the value cannot be `None`/`Err` due to an upstream invariant already proven
in the code, add an inline comment: `// INVARIANT: <reason it cannot fail>` and
leave the `.unwrap()`. Do not add a panic-on-failure path for provably-unreachable
branches.

**Class D — `panic!()` in unreachable arms → convert to `unreachable!()`**
Any `panic!("...")` used as an exhaustiveness guard for an `else` / `match` arm
that the type system should already rule out should be `unreachable!()` (more
accurate semantics). If the type system does *not* rule it out, the branch needs
proper error handling.

For each production-path hit, classify it (A/B/C/D) and apply the corresponding
fix or comment. Skip all hits inside `#[cfg(test)]` or `#[test]` sections.

## Spec

### 1. **Add `// SAFETY:` comments to the three production `unsafe` blocks** — in
   `src/main.rs` (~line 257), `src/cli/render.rs` (~lines 202 and 219),
   and `src/daemon/server.rs` (~line 384). Each comment goes on the line
   immediately above the `unsafe {` keyword. Base the text on the "Why" column
   in §"Current state" above; two sentences max.

### 2. **Resolve `#[allow(dead_code)]` instances** — in each file listed above,
   either delete the unused symbol and remove the attribute, or (if the symbol
   is kept intentionally) replace the bare `#[allow(dead_code)]` with a doc
   comment explaining why. Files: `src/header.rs` (~lines 53, 239, 295),
   `src/runbook.rs` (~line 17), `src/daemon/policy.rs` (~line 26),
   `src/search.rs` (~line 20), `src/webhook.rs` (~line 43),
   `src/daemon/session.rs` (~line 106), `src/daemon/memory_prompt.rs`
   (~lines 26, 32, 39, 44, 67), `src/memory/index.rs` (~line 6).

### 3. **Fix `#[allow(deprecated)]` instances** — in `src/scheduler.rs`
   (~lines 152, 587, 745), `src/daemon/executor/schedule.rs` (~line 44),
   and `src/daemon/scheduled.rs` (~lines 39, 162). For each: identify the
   deprecated API call, find the non-deprecated replacement (check Rust stdlib
   or chrono docs as appropriate), update the call site, and remove the
   `#[allow(deprecated)]`. If the replacement is not straightforward, file a
   blocker rather than leaving a silent `deprecated` suppressor.

### 4. **Add justification comments to `#[allow(clippy::too_many_arguments)]`**
   — in `src/memory.rs`, `src/session_store.rs`, `src/daemon/stream.rs`,
   `src/daemon/server.rs`, `src/cli/input.rs` (×2),
   `src/daemon/executor/file_ops.rs`, `src/daemon/executor/knowledge.rs` (×2).
   Add a `// TODO(M2): consolidate params into a struct` comment on the line
   directly above each `#[allow(clippy::too_many_arguments)]`. Do **not**
   refactor the function signatures — that is out of scope for this phase.

### 5. **Fix production-path `unwrap()`/`expect()`/`panic!()` occurrences** —
   classify every non-test-section hit per the A/B/C/D taxonomy in §"Current
   state", then apply the corresponding fix or comment. Use `UnpoisonExt` for
   mutex sites (Class A); `?` propagation for fallible `Option`/`Result` in error
   contexts (Class B); `// INVARIANT:` inline comments where the value is
   provably non-null (Class C); `unreachable!()` for exhaustiveness guards that
   are already ruled out by the type system (Class D).

   Work file by file, starting with the highest-hit files in production paths.
   After each file, run `cargo build` to confirm zero new warnings before
   proceeding. Do **not** touch any occurrence inside a `#[cfg(test)]` block or
   `#[test]` function.

## Acceptance criteria

- [ ] `grep -rn 'unsafe {' src/main.rs src/cli/render.rs src/daemon/server.rs`
      shows a `// SAFETY:` comment on the line immediately preceding each of the
      three production `unsafe` blocks.
- [ ] `grep -rn '#\[allow(dead_code' $(find src -name '*.rs' | grep -v '_tests.rs')`
      returns zero hits (all dead-code allows resolved — symbols removed or
      converted to doc comments).
- [ ] `grep -rn '#\[allow(deprecated' $(find src -name '*.rs' | grep -v '_tests.rs')`
      returns zero hits (all deprecated allows resolved by fixing the underlying
      call).
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings), `cargo clippy
      --all-targets --all-features -- -D warnings`, and `cargo test` all pass.
- [ ] No new `unwrap()`/`expect()`/`panic!()` introduced in production paths (diff
      shows only removals or additions of `// INVARIANT:` comments, `unreachable!()`,
      `?` propagation, or `.unwrap_or_log()`).

## Test plan

This phase is a refactor + documentation pass — it does not add new behavior.
No new tests are required beyond confirming existing tests continue to pass.
The acceptance criteria are verified by the grep checks and `cargo test`.

If any Class B fix (Option `?` propagation) changes a function's return type from
`T` to `Result<T, _>` and call sites must be updated, confirm `cargo build` zero
warnings at each call-site update before proceeding to the next.

## End-to-end verification

Not applicable — this phase ships no new runtime-loadable artifact. It is a
mechanical cleanup of error-suppress idioms whose verification surface is the
grep checks and the zero-warning/zero-test-failure build, both verified by the
acceptance criteria above.

## Authorizations

None. No new dependencies. No architecture doc changes. The three justified
`unsafe` blocks are not removed — only documented.

## Out of scope

- **Refactoring `too_many_arguments` functions into parameter structs.** The
  `#[allow(clippy::too_many_arguments)]` sites receive a TODO comment; the actual
  struct refactor belongs in a dedicated phase (risk: wide blast radius on call
  sites across multiple files and trait boundaries). Do **not** rename or
  restructure any of the affected functions.
- **Removing `#[allow(clippy::large_enum_variant)]` in `ipc.rs`.** The existing
  comment justifies it; leave it.
- **Test-embedded `unsafe { std::env::set_var(...) }` blocks.** These are in
  `#[cfg(test)]` sections and are exempt. Do not touch them.
- **Refactoring mutex types away from `std::sync::Mutex`.** The project's
  established pattern (`UnpoisonExt`) handles mutex poison recovery; converting to
  async `tokio::sync::Mutex` or removing locks is a separate architectural decision.
- **Fixing `unwrap()` hits inside `#[cfg(test)]` or `#[test]` functions**, even
  when those functions live in non-`_tests.rs` source files. Test code is exempt
  per STANDARDS §1.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
