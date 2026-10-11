# Bug 1 on phase-06b: the rewrite claims host commands are sandboxed, and README describes a pane archive nothing writes

**Severity:** major
**Status:** fixed (25505d1d)
**Filed:** 2026-10-11

## What's wrong

1. **CLAUDE.md, Sandbox section, the "Not sandboxed" bullet** now reads:

   > Commands on a configured `[hosts.*]` host run there, in that host's
   > own sandbox when one is configured.

   No code path sandboxes a `[hosts.*]` command. `HostConfig::sandbox_profile`
   (`src/config/types.rs`) is parsed and defaulted, and nothing in the
   executor reads it. The pre-phase text was correct: `[hosts.*]` commands
   are **not** sandboxed. The rewrite turned a true statement into a false
   isolation claim, which a reader would rely on for security decisions.

2. **README.md, the runtime directory tree** still says:

   > `panes/              ← archived background-command output (one .log per job)`

   Since the background-window path was deleted, nothing writes
   `var/log/panes/`. `pane_logs_dir()` is only created at seed time
   (`config/seeds.rs`) and swept (`sweep_pane_logs` in
   `daemon/utils/mod.rs`). This is the same kind of stale line as the
   `pipes/` entry the phase removed.

## What should happen

Phase spec, § What is true now, and § Out of scope: "If the text and the
code disagree, the code is right." The docs state only what the code does.

## Root cause

The executor rewrote the sandbox bullet from inference instead of from
§ What is true now, which says nothing about host sandboxing. It reworded
the `panes/` line in passing without checking that anything still writes
there.

## Definition of done

- [ ] `grep -n "host's own sandbox" CLAUDE.md` prints nothing. Today it
      prints one line.
- [ ] `grep -n '`\[hosts\.\*\]` commands' CLAUDE.md` prints one line: the
      "Not sandboxed" bullet names `[hosts.*]` commands again. Today it
      prints nothing.
- [ ] `grep -n 'archived background-command output' README.md` prints
      nothing. Today it prints one line. The `panes/` line says it is a
      legacy directory swept at startup.
- [ ] `cargo test` reports exactly `passed=2308 failed=0`, the same as now.
      This fix adds no test.
