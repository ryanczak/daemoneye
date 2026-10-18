# Bug 1 on phase-03e: a dead `#[allow(clippy::too_many_arguments)]`

**Severity:** minor
**Status:** fixed (02ced43c)
**Filed:** 2026-10-18

## What's wrong

`run_interactive_in_shell` in `src/daemon/executor/shell_exec.rs` (added in
`d71d13c2`) carries `#[allow(clippy::too_many_arguments)]`. It has 7
parameters. Clippy's `too_many_arguments` fires above 7, and the repo has no
`clippy.toml` raising or lowering that limit. The reviewer removed the
attribute in a scratch worktree and
`cargo clippy --all-targets --all-features -- -D warnings` exited 0. The
attribute suppresses nothing.

## What should happen

`STANDARDS.md` § Definition of Done: no `#[allow(...)]` lint-silencing
shims. Existing `too_many_arguments` allows in the repo sit on functions
that do exceed the limit; this one does not.

## Root cause

The attribute was copied from `run_in_shell_with_prompts` (11 parameters,
which needs it) onto the new wrapper, which does not.

## Definition of done

- [ ] `grep -B1 'pub(crate) async fn run_interactive_in_shell' src/daemon/executor/shell_exec.rs | grep -c allow`
      ⇒ `0` (1 today).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` ⇒ `passed=2395 failed=0`, unchanged. **Not** 2396: this
      fix adds no test.
