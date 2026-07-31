# Phase 11: Runtime-Tree Hygiene

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-02 (done), phase-07 (done), phase-10 (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=fix, size=m

## Goal

Make `~/.daemoneye/` contain nothing the code does not deliberately produce, and
nothing the docs describe that the code does not create.

Three concrete items, all verified in the tree while drafting:

1. **Decide `lib/`** — created on every install, empty since 26 March, documented
   as something that was never built.
2. **Correct the stale CLI help strings** that still name a pre-`var/` path.
3. **Stop a test-created runtime tree from being committable.**

## Architecture references

Read before starting:

- `src/config/lifecycle.rs:179` — the `lib` policy entry, whose own note says
  "defect-8 decides whether this lives". **This phase is defect 8.**
- `src/config/path_audit.rs:122` — the `lib` inventory entry
  (`source: "config::lib_dir()"`).
- `assets/memory/knowledge/agent-runtime-layout.md:30` — the ASCII tree line
  describing `lib/` as "shared SDK modules (de_sdk, Python helpers)".
- `src/config/seeds.rs:18` and `src/config/load.rs:45` — where `lib/` is created
  and where its path helper lives.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is clean and `cargo test` is green at 989 lib / 30
   integration (2 ignored) / 8 isolation (1 ignored).

## Current state

**Verified against the maintainer's live tree and the repo while drafting.**

**`lib/` is empty and always has been.** `ls -A ~/.daemoneye/lib/` returns
nothing; the directory's mtime is its creation date, 26 March. It is created
unconditionally by `Config::ensure_dirs()` (`seeds.rs:18`), has a path helper
(`load.rs:45`), a lifecycle entry (`lifecycle.rs:179`), a path-audit inventory
entry (`path_audit.rs:122`), and an asset line promising "shared SDK modules
(de_sdk, Python helpers)" that do not exist anywhere in the tree.

**The CLI help still names a pre-`var/` path.** `src/main.rs:17` and `:30` both
say the daemon log defaults to `~/.daemoneye/daemon.log`. It is
`var/log/daemon.log` (`config::default_log_path()`). This is the same drift class
phase 03 fixed in the assets — but the phase-02 gate only audits assets, so CLI
help text was never covered.

**A test-created runtime tree is committable.** `.gitignore` has no
`.daemoneye/` entry. During phase 04 a full 168 KB seeded tree appeared untracked
in the repo root and had to be moved out before a `git add -A` swept it in. Two
reviews recommended the entry; both correctly declined to add it as out of scope.

**One orphan is NOT this phase's to delete.** `~/.daemoneye/pane_prefs.json`
(12 bytes, 25 June) is dead — `pane_prefs::prefs_path()` returns
`var/run/pane_prefs.json` — but it lives in the operator's own tree. See "Out of
scope".

## Spec

### 1. Decide `lib/` — and the decision is: drop it

Recorded by the architect so this phase is determinate. `lib/` has been created
on every install for four months, is empty in the only live tree available, and
describes a feature (`de_sdk`, Python helpers) with no code anywhere in the
repository. Keeping a directory because it was once planned is how the drift this
milestone exists to remove got started.

**If you find evidence that something writes to `lib/`, stop and report a
blocker** rather than removing it — that would falsify the premise.

Dropping it means removing it from **every** place it is currently asserted.
These are interlocking, and phase 02's and phase 07's gates will catch a partial
job:

- `Config::ensure_dirs()` — stop creating it.
- `config::lib_dir()` — remove it if nothing else calls it. Check first;
  if something does, say what in the Update Log.
- `path_audit.rs`'s `lib` inventory entry — remove it. **This is load-bearing:**
  once removed, any surviving `lib/` mention in an audited asset becomes an
  `Unknown` finding and turns the phase-02 gate red. That is the gate working.
- `lifecycle.rs`'s `lib` policy entry — remove it. Phase 07's Direction B test
  asserts every entry corresponds to a real path.
- `assets/memory/knowledge/agent-runtime-layout.md` — remove the ASCII-tree line
  and any prose referring to it.

**Do not** remove `lib/` from anyone's disk. Ceasing to create it is the change;
an existing empty directory is inert.

### 2. Correct the CLI help strings

`src/main.rs:17` and `:30` must name `var/log/daemon.log`. Check whether any
other help text in `main.rs` names a pre-`var/` path and fix those too — say in
the Update Log how many you found.

### 3. Add `.gitignore` coverage

Add an entry so a `.daemoneye/` directory created in the repo root by a test run
cannot be committed. Keep it minimal and put it with the existing ignore rules.

### 4. A gate for the whole tree, not just three items

The durable deliverable. A test that asserts **the directories `ensure_dirs()`
creates are exactly the set the policy table documents** — no directory created
without a policy entry, and no non-lazy policy entry that `ensure_dirs()` fails to
create.

Phase 07's Direction A already checks one half of this (every existing directory
has a policy entry). The missing half is the reverse: a non-lazy entry naming a
directory that is never created. Adding `lib`-shaped drift back in should fail
this test, which is what stops the next `lib/` from accumulating.

If phase 07's existing tests already cover this exactly, say so in the Update Log
with the test name rather than adding a duplicate — but check carefully first,
because Direction B checks *paths are constructible*, which is weaker.

## Acceptance criteria

- [ ] `Config::ensure_dirs()` no longer creates `lib/`.
- [ ] No `lib` entry remains in `path_audit.rs`'s inventory or
      `lifecycle.rs`'s policy table.
- [ ] No `lib/` reference remains in `assets/memory/knowledge/agent-runtime-layout.md`.
- [ ] The phase-02 path audit is still green (no `Unknown` findings) —
      demonstrating the asset and the inventory were changed together.
- [ ] `src/main.rs`'s help text names `var/log/daemon.log`.
- [ ] `.gitignore` prevents committing a repo-root `.daemoneye/`.
- [ ] A test fails when a non-lazy policy entry names a directory
      `ensure_dirs()` does not create.
- [ ] Phase 07's three lifecycle tests and phase 02's path-audit tests still pass.
- [ ] All four gates green.

## Test plan

**Tests that touch `HOME` must take `crate::test_home_guard()`**
(`src/lib.rs:45`), hold it through all HOME-dependent work, **and restore `HOME`
at the end** — the idiom is in `src/pane_prefs.rs`'s tests. Phase 09 shipped five
tests that skipped the restore and caused a ~3-in-8 `cargo test --lib` flake that
cost an architect takeover.

**Mutation-check the new gate:** add a fake non-lazy policy entry for a directory
`ensure_dirs()` never creates, confirm the test **fails naming it**, remove it,
confirm it passes. Quote both runs. A tree-consistency gate that has never failed
is exactly the vacuous coverage this milestone exists to eliminate.

**Do not pin a test count in advance.** Report the resulting count and explain the
delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`. The
server-authored `(complete)` entry's "Command output tails" block does **not**
satisfy this — eight bounces on this milestone have turned on that distinction.

```sh
# The new gate must go red on a fake entry and green without it.
cargo test --lib lifecycle -- --nocapture \
  > /tmp/e2e-11-red.txt 2>&1; echo "exit=$?" >> /tmp/e2e-11-red.txt

git checkout -- src/

cargo test --lib lifecycle -- --nocapture \
  > /tmp/e2e-11-green.txt 2>&1; echo "exit=$?" >> /tmp/e2e-11-green.txt

# lib/ is gone from a freshly seeded tree.
export H=$(mktemp -d)
HOME=$H cargo run --quiet -- setup > /dev/null 2>&1
ls -A "$H/.daemoneye/" > /tmp/e2e-11-tree.txt 2>&1; echo "exit=$?" >> /tmp/e2e-11-tree.txt

for i in $(seq 1 12); do cargo test --lib >/dev/null 2>&1 || echo "FAIL run $i"; done \
  > /tmp/e2e-11-flake.txt 2>&1; echo "exit=$?" >> /tmp/e2e-11-flake.txt
```

Paste all four. `/tmp/e2e-11-tree.txt` must **not** list `lib`, and the flake file
must contain only `exit=0`.

## Authorizations

- [ ] May modify `src/config/seeds.rs`, `src/config/load.rs`,
      `src/config/path_audit.rs`, `src/config/lifecycle.rs`, `src/main.rs`,
      `.gitignore`, and `assets/memory/knowledge/agent-runtime-layout.md`.
- [ ] May add the tree-consistency test wherever it reads best.

No new dependencies. No changes to `docs/architecture.md` — that is phase 12.

## Out of scope

- **Do not delete anything from the operator's live `~/.daemoneye/`**, including
  the orphaned top-level `pane_prefs.json` and the now-unused `lib/` directory.
  Ceasing to *create* `lib/` is this phase's change; removing files from someone's
  real tree is an operator action, and this milestone has been careful about code
  that deletes user data. Note both in the Update Log so the operator can remove
  them deliberately.
- **Do not touch `src/pane_prefs.rs`** — phase 10 just rewrote it and its doc
  comment is already correct.
- **Do not change any retention default or sweep** — phases 08 and 09 own those.
- **Do not fix the pre-existing `tokio::time::sleep` at
  `tests/integration.rs:615`.** It predates M6 and is milestone housekeeping.
- **Do not touch `docs/architecture.md`.** Phase 12.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
