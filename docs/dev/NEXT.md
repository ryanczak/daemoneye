# NEXT

**Active phase: none — phase 01 is `done`; phase 02 is named but not drafted.**

Phase 01 (test-isolation-harness) was approved 2026-07-30 as `approved_after_1`.
Draft the next phase with `/rexymcp:architect next`.

## What phase 01 shipped

`tests/harness/mod.rs` (`IsolatedEnv`) and `tests/isolation.rs` (three scenarios).
No `src/` changes across either round. Every remaining M6 phase can now run a real
`daemoneye` daemon against a throwaway `HOME` and a private tmux server without
touching the operator's `~/.daemoneye/` or their default tmux server.

The mechanism, for phases 02–12 that will build on it: isolation is **per-`Command`
environment construction**, not argument plumbing. `HOME` → throwaway root under
`/tmp`, `TMUX_TMPDIR` → same root, `TMUX`/`TMUX_PANE` removed. That covers all 82
`Command::new("tmux")` sites with no source changes. The harness never calls
`std::env::set_var`, so these tests need no `test_home_guard()` serialization and
run in parallel with everything else.

Two things later phases should know:

- **Teardown is pinned to an explicit socket path** (`tmux -S <root>/tmux-<uid>/default
  kill-server`), deliberately *not* routed through the env helper. It fails closed:
  a broken `apply_env` makes teardown a no-op rather than killing the operator's
  server. Do not "simplify" it back.
- **`start_daemon` writes `config.toml` after `daemoneye setup`**, because setup's
  `ensure_dirs()` overwrites any pre-existing config with the bundled default.

## Cost of the round trip, and what it says

One bounce, four bugs, all closed. **Two of the four were spec bugs charged to the
architect**, not executor faults:

- The spec named `pane-died` + `show-hooks -g` for an assertion that could not
  fail — `tmux show-hooks -g <name>` echoes the hook's *name* whether or not it is
  set, so the test that was supposed to prove the daemon reached the private server
  passed on any running tmux server.
- The spec specified a `Drop` teardown whose safety argument was circular ("safe
  precisely because `TMUX_TMPDIR` scopes it"), which made the phase's own required
  mutation destructive. It destroyed a live session on the operator's default
  server during review round 1.

Both are the fold the phase doc itself cited at the executor — `WORKFLOW.md`
§ "Confirm the property is observable before pinning it". **The architect quoted
the rule and broke it in the same document.** That is the third architect-authored
unobservable property in this project, so the fold is not the missing piece; a
mechanical pre-dispatch criteria check is. See `docs/dev/TODO.md` § 1, and note
that M6 phase 02 is a narrower instance of the same idea — worth designing the two
together.

## Where things stand

- `cargo clippy --all-targets --all-features -- -D warnings` clean; 947 lib + 27
  integration (2 ignored, pre-existing) + **3 isolation**, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 02–12 are
  named, not drafted. Re-verify each phase's "Current state" against the tree
  before dispatching it — phase 01 changed what `tests/` looks like.
- Standing backlog: `docs/dev/TODO.md`.
