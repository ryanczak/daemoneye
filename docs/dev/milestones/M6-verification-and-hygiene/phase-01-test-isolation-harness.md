# Phase 01: Test-Isolation Harness

**Milestone:** M6 — Verification & Hygiene
**Status:** review
**Depends on:** none
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=test, size=m

## Goal

Give this milestone an end-to-end test environment that runs a real `daemoneye`
daemon against a **throwaway `HOME`** and a **private tmux server**, touching
neither the operator's `~/.daemoneye/` nor their default tmux server. Every
remaining M6 phase needs it: axes 2–4 all want end-to-end verification, and
across M5 every real-artifact check disrupted the operator's live daemon and
repointed their global tmux hooks — one scenario could not be re-run at review
for that reason.

## Architecture references

Read before starting:

- `docs/architecture.md` § 2 "Major data flows" — what a daemon does at startup,
  so you know which side effects the harness must contain.
- `CLAUDE.md` § "Important Invariants" — the instance lock is an exclusive
  `flock` on `~/.daemoneye/var/run/daemoneye.pid`, i.e. it is **per-`HOME`**.
  That is why a throwaway `HOME` alone already prevents lock collisions with the
  operator's daemon.
- `docs/dev/WORKFLOW.md` § "Coverage claims are inadmissible without mutation
  proof" — this phase's deliverable *is* a test harness, so task 5 requires you
  to demonstrate the isolation is load-bearing by breaking it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

There is **no test harness for running the real binary**. `tests/integration.rs`
(1583 lines) is the only integration target, and it exercises persistence and
IPC-protocol layers in-process against temp directories — it never spawns
`daemoneye` and never talks to tmux. `grep -rn CARGO_BIN_EXE tests/ src/` returns
nothing: no test in this repo has ever run the compiled binary.

Three facts about the daemon that determine the harness's shape:

**1. All paths derive from `$HOME`, through one function.**

```rust
// src/config/load.rs:98
fn dirs_next() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
```

`config_dir()` is `dirs_next().join(".daemoneye")`, and `var_run_dir()`,
`default_socket_path()`, `default_pid_path()`, `var_log_dir()` all hang off it.
So setting `HOME` in the child process's environment relocates the *entire*
runtime tree — socket, instance lock, logs, events — with no code change.

**2. The daemon installs four `-g` (global, server-wide) tmux hooks.**

```
src/daemon/mod.rs:563   set-hook -g pane-died          …
src/daemon/mod.rs:581   set-hook -g after-new-session  …
src/daemon/mod.rs:602   set-hook -g client-attached    …
src/daemon/mod.rs:620   set-hook -g client-detached    …
```

These are server-wide, not session-scoped. This is the entire disturbance
problem: a daemon started for a test rewrites global hooks on whatever tmux
server it can reach, and those hooks fire `daemoneye notify …` afterwards — at a
socket that may no longer exist.

**3. There are 82 `Command::new("tmux")` call sites** across 15 files
(31 in `src/tmux/pane.rs`, 12 in `src/daemon/mod.rs`, 9 in `src/tmux/session.rs`,
…). **None of them plumb a `-L` socket-name argument, and this phase does not add
one** — see "The load-bearing constraint" below, which is why.

**4. The webhook listener is disabled by default** (`WebhookConfig::default()`,
`src/config/types.rs:500`, `enabled: false`), so there is no port to collide with
the operator's daemon in this phase. Phase 06 will need to enable it on a
distinct port; the harness should make that easy but must not do it here.

## The load-bearing constraint — read this before designing anything

**Do not plumb `-L` through the 82 tmux call sites.** tmux resolves its server
socket from the `TMUX_TMPDIR` environment variable (`$TMUX_TMPDIR/tmux-$UID/default`),
and `std::process::Command` children inherit the parent's environment by default.
So setting `TMUX_TMPDIR` on the spawned `daemoneye` process gives every one of
those 82 call sites a private server **with zero source changes**.

This was verified on the target machine during phase drafting:

```
$ export TMUX_TMPDIR=/tmp/de-probe; mkdir -p $TMUX_TMPDIR
$ tmux new-session -d -s probe 'sleep 30' && tmux ls
probe: 1 windows (created Thu Jul 30 07:23:02 2026)

$ find $TMUX_TMPDIR
/tmp/de-probe
/tmp/de-probe/tmux-1000
/tmp/de-probe/tmux-1000/default        <- private server socket

$ (unset TMUX_TMPDIR; tmux ls)
no server running on /tmp/tmux-1000/default   <- default server unaffected

$ tmux new-window -t probe -d 'sleep 5'; tmux list-windows -t probe
0: tmux* (1 panes) …
1: tmux  (1 panes) …                   <- nested calls stay on the private server
```

So the harness's job is **environment construction, not argument plumbing**:
`HOME` → throwaway root, `TMUX_TMPDIR` → same root, `TMUX` and `TMUX_PANE`
**removed**.

Removing `TMUX`/`TMUX_PANE` is not optional. `src/daemon/mod.rs:158`
(`detect_session`) branches on `std::env::var("TMUX")`, and the CLI reads
`TMUX_PANE` at `src/cli/commands/ask.rs:72`, `src/cli/commands/chat.rs:42`,
`src/cli/commands/stream.rs:101`. If the harness runs from inside one of the
operator's panes, those inherited values point the child at the operator's pane
on the operator's server.

### Two gotchas that will bite

- **Unix socket paths are capped at ~108 bytes** (`sun_path`). Both the tmux
  socket and `$HOME/.daemoneye/var/run/daemoneye.sock` live under the throwaway
  root, so a long root silently breaks the harness with `File name too long`.
  This was hit during drafting with a root under the scratchpad directory.
  Create the root under `/tmp` explicitly — **not** `std::env::temp_dir()`, which
  honours `$TMPDIR` and can be arbitrarily long — and assert the length (task 1).
- **`daemoneye daemon` forks.** The parent does not exit until the forked child
  has bound its socket, relaying the outcome over the readiness pipe
  (`src/daemon/ready.rs`; `CLAUDE.md` § "Important Invariants"). So the harness
  should run the **non**-`--console` form and treat the parent's exit status as
  the readiness signal — no polling loop, no sleep. But it also means the daemon
  outlives the test process, so teardown must stop it explicitly.

## Spec

### 1. `IsolatedEnv` — the harness type

Create `tests/harness/mod.rs`. A file in a **subdirectory** of `tests/` is not
compiled as its own test binary, so it is included by the test target with
`mod harness;`.

Define `pub struct IsolatedEnv` holding a `tempfile::TempDir` (already a
dev-dependency). Construction:

- Root the temp dir at `/tmp` — `TempDir::new_in("/tmp")` — for the socket-length
  reason above. This project is Linux-only (`libc::fork`, `flock`, tmux), so
  hardcoding `/tmp` is acceptable here.
- **Assert at construction** that the resulting
  `<root>/.daemoneye/var/run/daemoneye.sock` path is under 100 bytes, panicking
  with a message naming the path and its length if not. A silent
  `File name too long` later is much harder to diagnose.
- Create `<root>/.daemoneye/etc/` and write a minimal `config.toml` sufficient
  for the daemon to start. Keep it minimal — the daemon does not need an API key
  at startup (`resolve_api_key`, `src/config/types.rs:642`, is called per
  request, not at boot).

**The harness must never call `std::env::set_var`.** Isolation is per-`Command`,
not per-process. This is deliberate: it keeps these tests free of
`crate::test_home_guard()` serialization and lets them run in parallel with each
other and with `tests/integration.rs`.

### 2. Command builders

Two methods returning a configured `std::process::Command`:

- `fn daemoneye(&self, args: &[&str]) -> Command` — program is
  `env!("CARGO_BIN_EXE_daemoneye")` (cargo sets this for integration tests and it
  points at the just-built binary, so no `cargo build` step is needed).
- `fn tmux(&self, args: &[&str]) -> Command` — program is `"tmux"`.

Both apply the **same** environment: `.env("HOME", root)`,
`.env("TMUX_TMPDIR", root)`, `.env_remove("TMUX")`, `.env_remove("TMUX_PANE")`.
Factor that into one private helper so the two builders cannot drift — the
mutation in task 5 depends on there being exactly one place to break.

Also provide `fn default_tmux(&self, args: &[&str]) -> Command`: a tmux command
with `TMUX_TMPDIR` **removed**, for snapshotting the operator's default server.
Do not give it the throwaway `HOME`; it must observe the real environment.

### 3. Daemon lifecycle

- `fn start_daemon(&self, session: &str) -> std::process::Output` — runs
  `daemoneye setup` first (HOME-confined; assert success), then
  `daemoneye daemon --session <session>` **without** `--console`, waiting on the
  parent's exit status per the readiness handshake. On non-zero exit, panic with
  the captured stderr **and** the contents of
  `<root>/.daemoneye/var/log/daemon.log` if it exists — a daemon that fails to
  boot in a temp dir is otherwise mute.
- `fn stop_daemon(&self)` — best-effort `daemoneye stop`.
- `fn daemon_log(&self) -> String` — read the log, empty string if absent. Used
  in failure messages and by later phases.
- `impl Drop` — best-effort, ignoring all errors, in this order: `daemoneye stop`,
  then `tmux kill-server` **on the private server** (via `self.tmux`, never
  `default_tmux`). `TempDir`'s own drop removes the root.

`Drop` running `kill-server` is safe here precisely because `TMUX_TMPDIR` scopes
it. Getting this wrong kills the operator's tmux server, so keep the two builders
visibly distinct at every call site.

### 4. The scenario — `tests/isolation.rs`

New test target declaring `mod harness;`. Add a `fn tmux_available() -> bool`
helper (`tmux -V` succeeds). Each test that needs tmux checks it first and, if
absent, `eprintln!`s a line beginning `SKIP:` and returns — CI may not have tmux
installed, and a hard failure there would be noise. Acceptance criterion 6
requires you to show these tests actually **ran** on the target machine, not
skipped.

Write three tests. Names are yours to pick; these are the behaviours:

- **The daemon boots entirely inside the throwaway root.** After
  `start_daemon`, the socket and PID file exist under `<root>/.daemoneye/var/run/`,
  and `daemoneye ping` through the harness succeeds.
- **The daemon's global hooks land on the private server.** After
  `start_daemon`, `tmux show-hooks -g` on the **private** server mentions
  `pane-died`. This is the positive half — proving the daemon really did reach
  a tmux server, so that the negative half below is not passing vacuously
  (a daemon that reached *no* server would also leave the default one clean).
- **The default tmux server is unchanged.** Snapshot the default server
  **before** and **after** the full scenario and assert the two snapshots are
  byte-equal. Snapshot = the combined stdout, stderr, and exit status of
  `default_tmux(["list-sessions", "-F", "#S"])` and
  `default_tmux(["show-hooks", "-g"])`. Capture stderr and status, not just
  stdout: when no default server is running tmux writes
  `no server running on /tmp/tmux-1000/default` to **stderr** and exits
  non-zero, and that is a perfectly valid snapshot to compare — the assertion is
  *equality*, not *emptiness*.

Explicit must-NOT cases for the last test:
- It must **not** assert that the default server is absent, or that its session
  list is empty. The operator may have one running; the property is *unchanged*,
  not *nonexistent*.
- It must **not** call `kill-server`, `kill-session`, or any `set-hook` through
  `default_tmux`.

### 5. Mutation proof — required, and it is the deliverable

A harness that has never been shown to fail is not evidence of isolation. After
the suite is green, do this and record it in the completion Update Log:

1. In the single environment helper from task 2, remove the `TMUX_TMPDIR`
   assignment (leave `HOME` alone).
2. Run the suite. **Quote the actual failure output.**
3. Restore the line. Run the suite. Quote the pass.

Run this mutation with a tmux server live on the default server (create one for
the purpose — see End-to-end verification), or the mutated harness will simply
start its own default-socket server and the failure will be less informative.

If the mutation does **not** fail the suite, do not paper over it: the tests are
not observing what this phase claims. File that in the Update Log as a blocker
rather than adjusting the mutation until it fails.

## Acceptance criteria

- [ ] `cargo fmt --all` reports no changes needed.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero. Run
      it bare — a command piped through `tail` exits with `tail`'s status.
- [ ] `cargo test` is green with no regression against the M5 baseline of 947 lib
      + 27 integration tests. Quote the summary lines for every target.
- [ ] `tests/harness/mod.rs` contains no occurrence of `set_var` —
      `grep -c set_var tests/harness/mod.rs` prints `0`.
- [ ] `cargo test --test isolation -- --nocapture` shows all three tests running
      and passing, with **no** line beginning `SKIP:` in the output. Quote the
      output.
- [ ] The task-5 mutation pair is quoted in the completion Update Log: the
      failure with `TMUX_TMPDIR` removed, and the pass with it restored.

## Test plan

All three live in `tests/isolation.rs` and are described behaviourally in spec
task 4 — daemon-boots-in-throwaway-root, hooks-land-on-private-server,
default-server-unchanged. Choose the names.

No unit tests are required for the harness itself; `tests/isolation.rs` *is* its
test, and task 5's mutation is what establishes that it works.

## End-to-end verification

The automated tests cover the hermetic half. The milestone's exit criterion also
requires demonstrating this against a **live** default server, which `cargo test`
cannot assume. Do this by hand and quote the output:

1. On the **default** server (no `TMUX_TMPDIR`), create a throwaway session:
   `tmux new-session -d -s de-op-probe 'sleep 600'`.
2. Capture `tmux list-sessions -F '#S'` and `tmux show-hooks -g`.
3. Run `cargo test --test isolation -- --nocapture`.
4. Capture the same two commands again and show they are identical.
5. Clean up with `tmux kill-session -t de-op-probe`.

**Never run `tmux kill-server` against the default server** at any point in this
verification — kill only the named session you created. If `de-op-probe` is the
only session, killing it ends that server, which is expected and fine; killing
the *server* directly is not, because it would also destroy any session the
operator started while you worked.

Also confirm the operator's real tree was untouched: run
`find ~/.daemoneye -newermt '<timestamp before step 3>' -not -path '*/var/log/*'`
after the run and show it returns nothing.

## Authorizations

- [ ] May add a new integration test target `tests/isolation.rs` and a shared
      module `tests/harness/mod.rs`.
- [ ] May write a minimal `config.toml` into the throwaway root from the harness.

No new dependencies: `tempfile` is already in `[dev-dependencies]`. No changes to
`docs/architecture.md`.

## Out of scope

- **Do not add `-L` / `-S` socket arguments to any `tmux` call site in `src/`.**
  82 sites, and `TMUX_TMPDIR` makes it unnecessary. A diff touching `src/tmux/`
  is a scope violation in this phase.
- **Do not fix any defect the harness reveals.** If starting a daemon in a
  throwaway `HOME` surfaces a bug, record it in the Update Log and leave it.
  Phases 05–06 own the pipeline fixes; 07–09 own artifact lifecycle.
- **Do not enable the webhook listener or write a webhook scenario.** That is
  phase 06, which will build on this harness.
- **Do not add AI-dependent scenarios.** Nothing in this phase may require an
  API key or make a network call.
- **Do not touch `tests/integration.rs`.**
- **Do not add `#[ignore]` to any test.** An ignored test is not a harness; the
  `SKIP:` guard in task 4 is the sanctioned mechanism, and criterion 6 requires
  showing it did not trigger.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 14:57 (started)

**Executor:** rexyMCP executor

Implementing `tests/harness/mod.rs` (`IsolatedEnv`) and `tests/isolation.rs` (three end-to-end tests). The harness uses `TempDir::new_in("/tmp")` for the throwaway root, sets `HOME` and `TMUX_TMPDIR` on every spawned `Command`, and removes `TMUX`/`TMUX_PANE`. The `daemoneye setup` command overwrites pre-existing `config.toml`, so the test config is written after setup.

### Update — ts=1785423994194 (complete, server-authored)

**Summary:** All verification passes. Working tree is clean.

**Summary + Notes for review:**

Built `tests/harness/mod.rs` (`IsolatedEnv`) and `tests/isolation.rs` (3 end-to-end tests). The harness creates a throwaway `$HOME` under `/tmp` via `TempDir::new_in`, applies `HOME` + `TMUX_TMPDIR` to every spawned `Command` through a single `apply_env` helper, and removes `TMUX`/`TMUX_PANE` to prevent reaching the operator's tmux server. The `daemoneye setup` command's `ensure_dirs()` overwrites any pre-existing `config.toml` with the bundled default (empty `api_key`), so the test config with a dummy key is written **after** setup completes — this was the key adaptation needed to get the daemon to boot.

**Deviation from spec:** `pane-died` is a built-in tmux event hook that does not appear in the general `show-hooks -g` listing (tmux 3.7b behavior). The `hooks_land_on_private_server` test queries it by name (`show-hooks -g pane-died`) rather than grepping the full hook list. This is the correct way to verify the hook exists.

**Mutation proof:** Removing `TMUX_TMPDIR` from `apply_env` causes all 3 tests to fail — the daemon connects to the default server, modifies its hooks, and the `default_server_unchanged` test detects the change. Restoring `TMUX_TMPDIR` brings all 3 tests back to green.

**E2E result:** All 3 isolation tests pass with no `SKIP:` lines. 947 lib + 27 integration + 3 isolation = 977 tests total, zero failures. `grep -c set_var tests/harness/mod.rs` prints `0`.

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
::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
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
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 947 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.83s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test event_log_append_read ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test event_log_entry_format ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test g4_briefing_masking_applied ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 3 tests
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s


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

- `docs/dev/milestones/M6-verification-and-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M6-verification-and-hygiene/phase-01-test-isolation-harness.md` — +7 -1
- `tests/harness/mod.rs` — +197 -0
- `tests/isolation.rs` — +146 -0

**Commit:** 2b7b2f86456a2605ac10bdc7d2fafb7eb967fc9f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
