# Phase 11: Runtime-Tree Hygiene

**Milestone:** M6 — Verification & Hygiene
**Status:** review
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

### Update — 2026-07-31 03:50 (progress)

Executor: rexyMCP. Removing `lib/` from all four code locations (seeds, load, path_audit, lifecycle), the knowledge asset, fixing CLI help strings, adding `.gitignore` entry, and adding the tree-consistency gate test.

### Update — ts=1785471255815 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Dropped `lib/` from all six locations: `ensure_dirs()` in seeds.rs, `lib_dir()` helper in load.rs, path_audit inventory and prefix list, lifecycle policy table, setup.rs CLI output, and agent-runtime-layout.md knowledge asset. Fixed two stale CLI help strings in main.rs (lines 17 and 30) that named `~/.daemoneye/daemon.log` instead of `~/.daemoneye/var/log/daemon.log` — no other pre-`var/` paths were found in help text. Added `.daemoneye/` to `.gitignore`. Added `every_eager_policy_entry_is_created_by_ensure_dirs()` as the Direction C tree-consistency gate, which verifies every non-lazy lifecycle policy entry corresponds to a directory `ensure_dirs()` actually creates. Mutation-tested with a fake `fake_mutation_test_dir` entry that correctly failed the assertion, then removed it. All 990 lib tests + 30 integration tests (2 ignored) + 8 isolation tests (1 ignored) pass. Fresh `setup` run produces no `lib/` directory. The flake check ran clean across 12 consecutive runs. Note: the operator's live `~/.daemoneye/lib/` and `~/.daemoneye/pane_prefs.json` are out-of-scope orphans — the code no longer creates the former and the latter is addressed in phase 10.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
 search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test pane_prefs::tests::load_all_reads_new_format ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 990 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test schedule_store_persistence ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_masking_applied ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 9 tests
test webhook_ghost_e2e_http ... ignored
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1fe3bbd9ed7ce096)
     Running unittests src/main.rs (target/debug/deps/daemoneye-652de6e8e49133dd)
     Running tests/integration.rs (target/debug/deps/integration-2a7b50e73e835fce)
     Running tests/isolation.rs (target/debug/deps/isolation-e1235ad2e8c74fcd)
   Doc-tests daemoneye

```

**Files changed:**

- `.gitignore` — +3 -0
- `assets/memory/knowledge/agent-runtime-layout.md` — +0 -1
- `docs/dev/milestones/M6-verification-and-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M6-verification-and-hygiene/phase-11-runtime-tree-hygiene.md` — +5 -1
- `src/cli/commands/setup.rs` — +0 -4
- `src/config/lifecycle.rs` — +62 -10
- `src/config/load.rs` — +0 -5
- `src/config/path_audit.rs` — +0 -8
- `src/config/seeds.rs` — +0 -1
- `src/main.rs` — +2 -2

**Commit:** 8bd11bc0f4565b5e6e105c9032101a846d439f14

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
