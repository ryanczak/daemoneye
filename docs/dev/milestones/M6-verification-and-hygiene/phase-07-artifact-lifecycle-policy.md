# Phase 07: Artifact Lifecycle Policy

**Milestone:** M6 — Verification & Hygiene
**Status:** in-progress
**Depends on:** phase-01 (done)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=design, size=m

## Goal

State, in one place, what happens to **every** artifact class under
`~/.daemoneye/` — and land the test that fails when a class exists with no
stated policy.

This phase writes **no rotation code**. Phases 08 and 09 implement against the
table this phase decides. The durable deliverable is the table plus the gate that
stops the next artifact class from being unmanaged by omission — which is how all
four current gaps arose.

## Architecture references

Read before starting:

- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § "Defect
  inventory" items 9, 9b, 9c and § "Why the artifact work is one design phase
  before three mechanical ones" — the survey this phase encodes.
- `src/config/path_audit.rs` — **the pattern to follow.** Phase 02 solved a
  structurally identical problem (an explicit table + a test checked in both
  directions). Reuse the shape, not the code.
- `src/daemon/utils/event_log.rs:228` (`sweep_event_segments`) and
  `src/daemon/utils/mod.rs:20` (`sweep_session_archives`) — the only two sweeps
  that exist, both called from `src/daemon/mod.rs:821`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read `src/config/path_audit.rs` in full — you are building its sibling.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 964 lib / 30
   integration (2 ignored) / 8 isolation (1 ignored).

## Current state

**Verified against the tree while drafting.**

`Config::ensure_dirs()` (`src/config/seeds.rs:8-26`) creates: `etc/`,
`var/run/`, `var/log/`, `var/log/pipe/`, `var/log/panes/`, `bin/`, `lib/`,
`etc/prompts/`, `var/log/sessions/`, `scripts/`, `runbooks/`. It then seeds
`memory/knowledge/` and `memory/session/` via `seed_memory_inner`
(`seeds.rs:76`). Other classes appear at runtime: `var/log/events/`
(`events_dir()`), `var/sessions/` (`session_store::saved_sessions_dir`), and
`agents/<name>/mailbox/`.

**Only two sweeps exist in the entire codebase**, both fired from one site
(`src/daemon/mod.rs:821-825`) on every 60th cleanup tick:

| Class | Live size | Files | Lifecycle today |
|---|---|---|---|
| `var/log/daemon.log` | 25.8 MB | 1 | **none — no rotation logic anywhere** |
| `var/log/events/` | 167 KB | 10 | swept, `events.retention_days` default **90** |
| `var/log/sessions/` | 3.2 MB | 141 | swept, but `archive_retention_days` default **0 = forever** |
| `var/log/panes/` | 1.9 MB | **264** | **none** |
| `var/log/pipe/` | ~0 | 0 | cleared at daemon start ✓ |
| `agents/*/mailbox/` | — | 3 | **none** — one file per ghost exit, forever |
| `scripts/`, `runbooks/`, `memory/` | 1.0 MB | 88 | user content — no stated policy either way |

The classes differ in **kind**, not merely in coverage: one swept with a sane
default, one swept with an off default, three unswept, one cleared at startup,
several holding user content that probably *should* persist. Writing rotation for
`daemon.log` first would produce a fourth independent convention — which is why
the policy comes first.

## Spec

### 1. `src/config/lifecycle.rs` — the policy table, as production code

New module, declared from `src/config/mod.rs`. **Production code, not
`#[cfg(test)]`** — phase 08 and 09 read this table to know what to implement, and
a future operator-facing report may too. STANDARDS §2.1 applies: no `unwrap()` /
`expect()` / `panic!()` outside the test module.

Each entry pairs a **runtime-relative path** (no `~/.daemoneye/` prefix) with:

- Its **intended lifecycle** — the milestone's own vocabulary: rotate, delete,
  archive, or keep-forever-by-design.
- Its **default value** where the lifecycle is parameterised (a retention in
  days, a size bound), plus the config key that controls it when one exists.
- Whether that intent is **implemented today**, and if not, which phase owns it.

That last field is the honest part. `daemon.log`'s stated lifecycle is *rotate*;
its implementation lands in phase 08. Recording "rotate, not yet implemented,
phase 08" is a stated policy. Recording nothing is the omission this phase exists
to eliminate.

Name the types and variants however reads best. **Do not** add config keys,
change any default, or write sweep code — the table *describes* intent; phases
08–09 make it true.

### 2. The test, checked in both directions

This is the deliverable that outlives the table. Follow phase 02's pattern.

**Direction A — no class escapes the policy.** In a throwaway `HOME`, call
`Config::ensure_dirs()`, then enumerate the artifact directories that actually
exist and assert **every one** is covered by an entry. This is the gate that
fails when someone adds a directory and forgets the policy.

**Direction B — no entry is fiction.** Every entry's path must correspond to a
real directory the daemon creates or a real file it writes. This keeps the table
from rotting into a wishlist, and it is the direction phase 02 found most
valuable.

Some classes are created lazily rather than by `ensure_dirs()` —
`var/log/events/`, `var/sessions/`, `agents/<name>/mailbox/`. Decide how to treat
them and **say why in a comment**: either create them in the fixture so direction
A sees them, or mark those entries as lazily-created and exempt from A while
still bound by B. Either is defensible; silently missing them is not.

**Tests that touch `HOME` must take `crate::test_home_guard()`**
(`src/lib.rs:45`) — not the raw `TEST_HOME_LOCK` (`:32`). Edition 2024, so
`std::env::set_var` needs `unsafe`. Hold the guard through all HOME-dependent
work and drop it at the end; a phase-04 bug was filed for dropping it early.

### 3. Prove the gate fires

A policy test that has never failed is exactly the vacuous coverage this
milestone exists to eliminate — the same argument that shaped phase 02.

So: **mutation-check direction A.** Create an extra directory under the throwaway
`~/.daemoneye/` that no entry covers, confirm the test **fails** naming that
directory, remove it, confirm it passes. Quote both runs in the Update Log.

If you find that direction A cannot fail — because the enumeration is too narrow,
or because it only walks paths the table already lists — that is a real defect in
the test, not a detail. Fix the enumeration.

### 4. Record the two known asymmetries

The table must make these visible rather than burying them:

- **`sessions.archive_retention_days` defaults to `0` (keep forever)** while
  `events.retention_days` defaults to `90` (`src/config/types.rs`). Two adjacent
  classes, opposite defaults, and nothing surfaces it. 141 session archives back
  to May 8 are the result.
- **`agents/*/mailbox/` has no sweep at all.** `write_mailbox_on_exit` writes one
  file per ghost exit, so it grows one-per-ghost forever.

Stating them is this phase's job. Fixing the first is phase 09's; the second
needs an owner — assign it in the table and say so.

## Acceptance criteria

- [ ] A production table states an intended lifecycle, a default, and an
      implementation status for every artifact class in the survey above.
- [ ] A test enumerates the artifact directories that exist after
      `ensure_dirs()` and fails on any not covered by the table.
- [ ] A test asserts every table entry corresponds to a real path.
- [ ] The mutation check is quoted: an uncovered directory makes the test fail,
      naming it.
- [ ] `daemon.log` is stated as *rotate*, owned by phase 08; `var/log/panes/` and
      `sessions.archive_retention_days` are stated and owned by phase 09;
      `agents/*/mailbox/` is stated with an owner.
- [ ] No sweep code, no rotation code, no config-key or default changes.
- [ ] All four gates green.

## Test plan

- Direction A over a seeded throwaway `HOME`.
- Direction B over the table.
- The mutation check from task 3.

**Do not pin a test count in advance.** Report the resulting count in the Update
Log and explain the delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the file's contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase gets automatically. **It does not satisfy this
requirement.**

Capture the mutation check from task 3 (uncovered directory → failure naming it →
removal → pass), and a run of the new tests. Use `exit=$?` / `grep-exit=$?`
markers so a result that prints nothing is still observable.

## Authorizations

- [ ] May add `src/config/lifecycle.rs` and declare it from `src/config/mod.rs`.
- [ ] May add tests in that module or under `tests/`.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Write no rotation, sweep, or deletion code.** Not for `daemon.log`, not for
  `panes/`, not for mailboxes. Phases 08 and 09 implement against this table; a
  phase that both decides and implements would produce exactly the fourth
  independent convention this ordering exists to prevent.
- **Change no config defaults and add no config keys** — including
  `archive_retention_days`, which stays `0` until phase 09 decides.
- **Do not modify `sweep_event_segments`, `sweep_session_archives`, or their call
  site** at `src/daemon/mod.rs:821`.
- **Do not resolve the `lib/` question** (defect 8) — record its policy and move
  on; phase 11 decides whether it lives.
- **Do not touch `.gitignore`, `src/pane_prefs.rs`, `main.rs`'s stale
  `daemon.log` help strings, or the pre-existing `tokio::time::sleep` at
  `tests/integration.rs:615`.** Phase 11 and milestone housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 22:48 (started)

**Executor:** local model

Created `src/config/lifecycle.rs` with the `POLICY_TABLE` (15 entries covering all artifact classes from the survey), `LifecycleIntent` enum (Rotate, Sweep, ClearAtStartup, KeepForever), `ImplementationStatus` enum (Implemented, Pending), and `LifecycleEntry` struct. Declared the module from `src/config/mod.rs`.

Three tests: Direction A (every existing directory has a policy entry), Direction B (every policy entry corresponds to a real path), and the mutation check (rogue directory triggers Direction A failure naming it).

Two known asymmetries recorded in the table: `var/log/sessions` default retention is 0 (keep forever) vs `var/log/events` default 90; `agents/*/mailbox` has no sweep and grows one file per ghost exit forever.
