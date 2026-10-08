# Bug 1 on phase-09a: `container.rs` gained a file-level `#![allow(dead_code)]`

**Severity:** major
**Status:** fixed (`6050a97a`, verified 2026-10-08)
**Filed:** 2026-10-08

## What's wrong

`src/daemon/executor/container.rs` lines 1–4 (commit `d6c18fcc`):

```rust
// Phase 09a removed every production caller of this module's items (row 09b
// deletes the whole docker runtime). The file-level dead-code allow keeps the
// lint gate green in between.
#![allow(dead_code)]
```

This is a lint-silencing shim. `STANDARDS.md` § Definition of Done forbids
it, and phase-09a's acceptance criteria say "No `#[allow(` … added" and
"`container.rs` is unchanged".

## What should happen

The lint gate is green with no `allow` added by this phase. The six pre-existing
`#[allow(dead_code)]` lines in `src/search.rs` and `src/memory/index/mod.rs`
are not this phase's, so leave them alone.

## Root cause

A spec defect, not an executor one. Phase-09a's Task 2 said that once
`collect_container_status` was unused, being `pub` would mean "no
warning". But the module is declared `pub(crate) mod container;` in
`src/daemon/executor/mod.rs`, and unused `pub` items inside a `pub(crate)`
module **do** warn under `-D warnings`. Removing the docker preflight gate
and the docker status path left a chain of 27 unused items.

## How to fix

The architect ran this on 2026-10-08 against the `d6c18fcc` tree and then
reverted it:
- delete the four lines quoted above (the comment and `#![allow(dead_code)]`);
- in `src/daemon/executor/mod.rs`, change `pub(crate) mod container;` to
  `pub mod container;`, matching `pub mod sbx_lifecycle;` beside it.

With both edits, `cargo clippy --all-targets --all-features -- -D warnings`
is clean. With only the first, it reports 27 warnings. Row 09b deletes the
module, and its declaration with it.

The executor's other `container.rs` edit, adding `runtime: "docker"` to the
test `sandbox_window_enabled_starts_with_the_quoted_runtime` because the
default runtime changed, is **accepted**. Leave it.

## Definition of done

- [ ] `grep -n 'allow(dead_code)' src/daemon/executor/container.rs` prints nothing. Today it prints
      `4:#![allow(dead_code)]`. Do not touch the pre-existing allows in
      `src/search.rs` and `src/memory/index/mod.rs`.
- [ ] `grep -n '^pub mod container;' src/daemon/executor/mod.rs` prints one
      line. Today it prints nothing.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` reports `passed=2422 failed=0`, the same count as now.
      This fix adds no tests.
