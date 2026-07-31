# NEXT

**Active phase: none — M7 is scoped but no phase is drafted.**

M7 — Memory Search & Maintenance was scoped 2026-07-31. Milestone README:
`docs/dev/milestones/M7-memory-search-and-maintenance/README.md` (nine phases
named, none drafted).

Draft phase 01 with `/rexymcp:architect next`.

## What M7 covers

One capability and one maintenance axis:

- **Working memory search.** `fts5_search()` is an eight-line stub returning an
  empty `Vec`, and it is one of three candidate sources in memory recall — so a
  memory whose *text* matches what the user said surfaces only if its *tags*
  happen to overlap. Degraded silently today.
- **Maintenance:** dependency currency, the path-audit gate's blindness to fenced
  code blocks, a generated runtime-layout tree, a bug-tracker truth gate, and the
  four test sleeps `STANDARDS.md` §3.3 forbids.

## One decision needed before phase 06

**Un-stubbing FTS5 requires adding a SQLite dependency** — there is none today.
Adding one is on `WORKFLOW.md`'s "What Executors Never Decide" list, so it needs
your sign-off, and the choice includes *how it links*: `rusqlite` with `bundled`
compiles SQLite from source (no host dependency, larger binary, FTS5 via feature
flag) versus linking the system SQLite (smaller, but FTS5 availability becomes a
property of the operator's machine). For a daemon shipping to operator machines
bundling is the safer default, but it is a real build-time and size cost.

**Phases 01–05 do not depend on this** and are ordered ahead of it, so drafting
can start immediately.

## Where the tree stands

- M6 closed: 13 phase docs `done`, retrospective in its README.
- 991 lib + 30 integration (2 ignored) + 8 isolation (1 ignored); clippy clean;
  20 consecutive `cargo test --lib` runs clean.
- Working tree clean. No daemon running; no tmux server running.
- No live bugs: the five bug docs still marked `open` across M2/M4 were each
  verified fixed against the code — M7 phase 02 closes them and lands a gate so
  the tracker cannot drift again.
