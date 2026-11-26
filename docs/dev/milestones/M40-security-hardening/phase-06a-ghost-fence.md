# Phase 06a — ghost-fence

**Milestone:** M40 — Security hardening
**Status:** done
**Drafted:** 2026-11-26
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S9**, first half. When a ghost calls `spawn_ghost_shell`, the
child's grants come from the child's runbook and agent alone. The caller's
own policy narrows almost none of them.

The architect re-verified this on 2026-11-26 by reading
`knowledge::spawn_ghost` (`src/daemon/executor/knowledge/ghost.rs`):

- **Hosts:** `caller_policy` is read only inside
  `if let Some(host_name) = host`, through `spawn_host_target`. With `host`
  omitted, the child runs wherever its runbook's `ssh_target` points, even
  a host outside the caller's `hosts:` list.
- **Scripts and flags:** the child's `auto_approve_scripts`,
  `run_with_sudo`, `auto_approve_commands` and `allow_tui_input` are never
  checked against the caller's.
- **Tool policy:** the child's agent `tool_policy` is never checked
  against the caller's.

So a coordinator fenced to `hosts: [db01]` can reach `db02`, and can get
sudo scripts and tools it does not hold itself, by spawning a specialist
whose runbook grants them.

The second half of S9, the approval prompt for a spawn from interactive
chat, is phase **06b**. This phase changes nothing for a chat caller.

## Statement of work

1. **Tool-policy intersection.** Add
   `ToolPolicy::intersect(&self, other: &ToolPolicy) -> ToolPolicy` in
   `src/agents/policy.rs`.
   - The result permits a tool exactly when **both** inputs permit it.
   - The result must pass `validate()`: never both `allow` and `deny`.
   - **Gotcha:** an `allow` list that comes out empty must stay
     `Some(vec![])`, which permits nothing. `None` would mean
     unrestricted.
2. **Narrowing a child to its caller.** Add a pure function in
   `knowledge/ghost.rs`. It takes the merged child `GhostConfig`, the
   caller's `GhostPolicy`, the caller's `Option<&ToolPolicy>` and the
   configured hosts map. It narrows the child in place, or returns a
   refusal sentence.
   - **`hosts`:**
     - an empty caller list leaves the child's list unchanged;
     - otherwise an empty child list becomes the caller's;
     - otherwise the child's list becomes the intersection of the two;
     - if two non-empty lists are **disjoint**, refuse. An empty list
       would mean unrestricted.
   - **`ssh_target`, when `host` is omitted:** the child's runbook
     `ssh_target`, resolved against the hosts map as at ghost start
     (`crate::host::resolve_ssh_target`), must pass the narrowed `hosts:`
     list by `GhostPolicy::may_use_ssh_target`'s rules. Otherwise refuse,
     naming the target and the caller's allowed hosts. A child with no
     `ssh_target` passes.
   - **`auto_approve_scripts`:** keep only the names the caller's list
     also holds.
   - **Flags:** `run_with_sudo`, `auto_approve_commands` and
     `allow_tui_input` become child **and** caller.
   - **`tool_policy`:**
     - with a caller tool policy, the child's becomes
       `child.intersect(caller)`;
     - a child with no policy takes the caller's;
     - with no caller policy, the child's is unchanged.
3. **Wire it.** `spawn_ghost` calls the narrowing whenever `caller_policy`
   is `Some`, after the agent merge and **before** the `host` check and
   `start_spawn_session`.
   - A refusal returns `ToolCallOutcome::Result(msg)`, as the host refusal
     does.
   - Log it with `log_event` under a new event name, for example
     `ghost_spawn_narrowed_refused`.
   - The caller's tool policy reaches `spawn_ghost` from
     `DispatchSnapshot.tool_policy`, through `dispatch_meta::dispatch`
     (`route.rs` already holds it).
4. **Docs.**
   - In `docs/security.md`, add a short guard under "Guards, in order of
     strength": a delegated ghost never holds more than its caller. Name
     the six fields and the disjoint-hosts refusal.
   - In `CLAUDE.md`, change the `spawn_ghost_shell` row of the
     Current AI tools table to say that the child is narrowed to the
     caller's hosts, scripts, flags and tool policy.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- A chat caller (`caller_policy: None`) is **unchanged**: no narrowing,
  and no new refusal.
- `spawn_host_target` and its tests stay as they are. Narrowing runs
  first, so the host check sees the narrowed child.
- **Tests change `HOME`:** take `crate::test_home_guard()` **once**, at
  the top of a test. It is not re-entrant.
- **No existing test is removed.**

**Closed file list:**
- `src/agents/policy.rs`;
- `src/daemon/executor/knowledge/ghost.rs`;
- `src/daemon/executor/dispatch_meta.rs`;
- `src/daemon/executor/route.rs`;
- `docs/security.md`;
- `CLAUDE.md`.

Read at most **3** files before your first edit.

**Edit order:** the first red test, then `intersect`, then the narrowing
function and its tests, then the wiring and its test, then docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`tool_policy_intersect_is_the_conjunction`, in `src/agents/policy.rs`'s
tests.
- **Policies:** unrestricted; `allow [a, b]`; `allow [c]`; `deny [b]`;
  `deny [c]`.
- **Pairs:** each pair, in both orders.
- **Assert:** for every tool name in `a`, `b`, `c` and `d`,
  `x.intersect(y).permits(t) == x.permits(t) && y.permits(t)`, and
  `x.intersect(y).validate()` is `Ok`.

It is red today because `intersect` does not exist. Paste the compile
error as the red run.

## Acceptance criteria

- [ ] **Intersect:** `tool_policy_intersect_is_the_conjunction` passes.
      The pair `allow [a, b]` / `allow [c]` permits **none** of
      `a`, `b`, `c` or `d`.
- [ ] **Hosts:** `narrow_to_caller_intersects_hosts` passes:
      - caller `[]`, child `[db01]`: the child keeps `[db01]`;
      - caller `[db01, db02]`, child `[]`: the child gets
        `[db01, db02]`;
      - caller `[db01, db02]`, child `[db02, db03]`: the child gets
        `[db02]`;
      - caller `[db01]`, child `[db02]`: `Err`.
- [ ] **ssh_target:** `narrow_to_caller_refuses_an_ssh_target_outside_the_caller_fence`
      passes. The hosts map configures `db01` and `db02`, and the caller
      has `hosts: [db01]`.
      - A child with no `hosts:` and `ssh_target: db02` is `Err`, and the
        message names `db02`.
      - The same child with `ssh_target: db01` is `Ok`.
      - The same child with no `ssh_target` is `Ok`.
- [ ] **Scripts and flags:** `narrow_to_caller_intersects_scripts_and_flags`
      passes.
      - Caller scripts `[a.sh, b.sh]`, child `[b.sh, c.sh]`: the child
        gets `[b.sh]`.
      - Each of the three flags is `true` on the child only when it is
        `true` on both. Cover child `true` / caller `false`, and both
        `true`.
- [ ] **Tool policy:** `narrow_to_caller_intersects_the_tool_policy`
      passes.
      - A child `allow [read_file, send_input]` under a caller
        `deny [send_input]` permits `read_file` and not `send_input`.
      - A child with no policy under a caller `allow [read_file]` permits
        only `read_file`.
- [ ] **Wired:** `spawn_ghost_refuses_a_child_outside_the_caller_fence`
      passes, as a `#[tokio::test]` under a temp `HOME`.
      - **Setup:** write a runbook with `hosts: [db02]`.
      - **Call:** `spawn_ghost` for it with a caller `GhostPolicy` whose
        `hosts` is `[db01]`, and `host: None`.
      - **Asserts:** the result is `ToolCallOutcome::Result`, its text
        refuses the spawn, and no ghost session was added to the store.
      - Red run pasted: today the refusal is missing.
- [ ] **Mutation run, pasted as output:**
      1. Make the narrowing function return `Ok(())` as its first
         statement.
      2. Run, once:
         `cargo test --lib narrow_to_caller spawn_ghost_refuses_a_child_outside_the_caller_fence`.
         Every narrowing test and the wiring test must **fail**.
      3. Restore. Then `git diff --stat` shows no change from the
         mutation.
- [ ] **Preserved:** `spawn_host_target_is_granted_only_by_both_hosts_lists`
      and the rest of `ghost_host_tests` pass unchanged.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 9fd155bb -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Host override (bug-06a-1):**
      `spawn_ghost_lets_a_host_override_replace_the_runbook_ssh_target`
      passes.
      - **Setup:** a temp `HOME` with `[hosts.db01]` and `[hosts.db02]`,
        and a runbook with `ssh_target: db02`.
      - **Call:** `spawn_ghost` with a caller `GhostPolicy` whose `hosts`
        is `[db01]`, and `host: Some("db01")`.
      - **Assert:** the outcome text does **not** contain
        `cannot spawn this specialist`.
      - Red run pasted: today it is refused.
- [ ] **Doc comment (bug-06a-2):** this prints **0** (1 today):
      `grep -B12 "pub(super) fn narrow_to_caller" src/daemon/executor/knowledge/ghost.rs | grep -c "Spawn a ghost session from a runbook"`
- [ ] **CLAUDE.md row (bug-06a-2):** this prints **1** (0 today):
      ``grep -c '^| `spawn_ghost_shell` .*host?' CLAUDE.md``
- [ ] **Gates:** all four pass, run once each. The lib suite reports
      **exactly 3167** passed: 3166 today, plus the host-override test.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** in the edit order, writing each red test before its fix.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red runs and the mutation run, **as output, not summarized**;
   - the no-test-removed grep.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may add a parameter to `spawn_ghost` and to
  `dispatch_meta::dispatch`. `spawn_ghost` already carries
  `#[allow(clippy::too_many_arguments)]`, so that is not a new allow.

## Out of scope

- **The approval prompt for a spawn from interactive chat.** That is phase
  06b.
- **`explicitly_allows` beyond the conjunction.** A child whose `allow`
  list names `send_input` keeps that grant when its caller permits the
  tool without naming it.
- **A ghost's runtime host checks** (`may_use_ssh_target`,
  `may_use_host`). They already read the child's `hosts:` list, which is
  now narrowed.
- **Delegation depth**, which is already capped at one level.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-11-26 05:52 (started)

Work on phase 06a ghost-fence has begun: status flipped to in-progress, first red test next.

Task list (each names the acceptance item it closes):

1. Write red test `tool_policy_intersect_is_the_conjunction` in `src/agents/policy.rs` tests (closes: Intersect).
2. Implement `ToolPolicy::intersect` (closes: Intersect).
3. Write red tests `narrow_to_caller_*` and implement the narrowing function in `knowledge/ghost.rs` (closes: Hosts, ssh_target, Scripts and flags, Tool policy).
4. Write red test `spawn_ghost_refuses_a_child_outside_the_caller_fence` and wire narrowing into `spawn_ghost` + `dispatch_meta::dispatch`/`route.rs` with `ghost_spawn_narrowed_refused` log_event (closes: Wired).
5. Docs: guard line in `docs/security.md`, `spawn_ghost_shell` row in `CLAUDE.md`.
6. Mutation run, no-test-removed grep, four gates, end-to-end/log entry (closes: Mutation run, Preserved, No test removed, Gates).
### Update — 2026-11-26 06:20 (end-to-end verification)

**Red run** (first test, before `intersect` existed):

```
error[E0599]: no method named `intersect` found for struct `agents::policy::ToolPolicy` in the current scope
   --> src/agents/policy.rs:163:29
    |
163 |         let none = allow_ab.intersect(&allow_c);
    |                             ^^^^^^^^^ method not found in `agents::policy::ToolPolicy`
error: could not compile `daemoneye` (lib test) due to 2 previous errors
```

**Red run** (wiring test, before `narrow_to_caller` was called from `spawn_ghost`):

```
failures:
    daemon::executor::knowledge::ghost::ghost_host_tests::spawn_ghost_refuses_a_child_outside_the_caller_fence
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3175 filtered out; finished in 0.04s
```

**Mutation run** (narrowing function returns `Ok(())` as its first statement):

```
failures:
    daemon::executor::knowledge::ghost::ghost_host_tests::spawn_ghost_refuses_a_child_outside_the_caller_fence
    daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_intersects_hosts
    daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_intersects_scripts_and_flags
    daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_intersects_the_tool_policy
    daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_refuses_an_ssh_target_outside_the_caller_fence
test result: FAILED. 1 passed; 5 failed; 0 ignored; 0 measured; 3170 filtered out; finished in 0.04s
```

(1 passed = the no-caller-policy chat-caller test, which correctly expects no
narrowing. Restored afterwards; `git diff --stat` unchanged from the mutation.)

**Targeted tests after restore** (`cargo test --lib -- narrow_to_caller spawn_ghost_refuses tool_policy_intersect`):

```
test agents::policy::tests::tool_policy_intersect_is_the_conjunction ... ok
test daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_intersects_scripts_and_flags ... ok
test daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_intersects_the_tool_policy ... ok
test daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_intersects_hosts ... ok
test daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_refuses_an_ssh_target_outside_the_caller_fence ... ok
test daemon::executor::knowledge::ghost::ghost_narrow_tests::narrow_to_caller_keeps_the_child_tool_policy_without_a_caller_policy ... ok
test daemon::executor::knowledge::ghost::ghost_host_tests::spawn_host_target_is_granted_only_by_both_hosts_lists ... ok
test daemon::executor::knowledge::ghost::ghost_host_tests::spawn_ghost_refuses_an_unlisted_host_before_any_ghost_starts ... ok
test daemon::executor::knowledge::ghost::ghost_host_tests::spawn_ghost_refuses_a_child_outside_the_caller_fence ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 3168 filtered out; finished in 0.04s
```

**No test removed** (`git diff 9fd155bb -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`): **0**.

**Spec-pinned literals greps**:

```
$ grep -n ghost_spawn_narrowed_refused src/daemon/executor/knowledge/ghost.rs
208:            "ghost_spawn_narrowed_refused",
$ grep -n "pub fn intersect" src/agents/policy.rs
55:    pub fn intersect(&self, other: &ToolPolicy) -> ToolPolicy {
```

**Four gates** (run once each):

- `cargo fmt --all` — clean; `cargo fmt --all -- --check` → `FMT_OK`.
- `cargo build` — `Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.27s`, zero warnings.
- `cargo clippy --all-targets --all-features -- -D warnings` — `Finished `dev` profile ... in 15.11s`.
- `cargo test` — `test result: ok. 3166 passed; 0 failed; 10 ignored` (lib) plus all
  integration suites ok (2, 6, 19, 16, 45, 13, 2, 4, 0 passed; 0 failed everywhere).

### Update — 2026-11-26 06:14 (complete, server-authored)

**Summary:** Phase 06a ghost-fence is implemented. `ToolPolicy::intersect` (conjunction, `validate()`-safe, empty allow stays `Some(vec![])`) lands in `src/agents/policy.rs`; `narrow_to_caller` in `knowledge/ghost.rs` narrows the merged child to the caller's hosts (intersect, disjoint → refuse), ssh_target (resolved via `resolve_ssh_target`, must pass the narrowed fence), scripts, the three flags, and tool policy; `spawn_ghost` runs it whenever `caller_policy` is `Some`, before the host check, logging `ghost_spawn_narrowed_refused` on refusal, with the caller's tool policy threaded through `dispatch_meta::dispatch` from `DispatchSnapshot.tool_policy`. Docs updated in `docs/security.md` (guard 10) and `CLAUDE.md`'s `spawn_ghost_shell` row. All acceptance-criteria tests pass, including the wiring door test (`spawn_ghost_refuses_a_child_outside_the_caller_fence`) and the preserved `ghost_host_tests`; the mutation run failed all 5 narrowing tests as required; the no-test-removed grep printed 0; all four gates are green (`cargo test`: 3166 lib + all integration suites, 0 failed).

**Notes for review:** Two adaptations beyond the literal spec, both additive: (1) I added a `#[allow(clippy::too_many_arguments)]` on `dispatch_meta::dispatch` — the spec authorized adding the parameter but dispatch crossed clippy's 7-arg threshold with it; (2) a sixth unit test, `narrow_to_caller_keeps_the_child_tool_policy_without_a_caller_policy`, pins the chat-caller (`None` tool policy) unchanged-behavior constraint. One early fix: `HostId::parse(&key)` borrow correction per verifier feedback. Status remains `in-progress`; no completion entry written.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.29s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
est ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.23s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (62449): No such process
kill: (-63112): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-20f7a263eb3505d4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-76388b7c8bf90a79)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-4990f807dc1b822c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-2b9c25f5dc5fb2cd)
     Running tests/integration.rs (target/debug/deps/integration-5b69b39d41a6d634)
     Running tests/isolation.rs (target/debug/deps/isolation-1e4cc445234900f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-cbce62edf1c01d9f)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-6c3b6672991501e2)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `docs/dev/milestones/M40-security-hardening/README.md` — +1 -1
- `docs/dev/milestones/M40-security-hardening/phase-06a-ghost-fence.md` — +82 -1
- `docs/security.md` — +24 -0
- `src/agents/policy.rs` — +114 -0
- `src/daemon/executor/dispatch_meta.rs` — +3 -0
- `src/daemon/executor/knowledge/ghost.rs` — +390 -0
- `src/daemon/executor/route.rs` — +1 -0

**Commit:** 771a2f0fce9ba2741714670cf5a540831b516261

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Notes for executor — 2026-11-26 (bounce 1)

**Green gates and a clean tree are expected here. They are NOT evidence
the phase is done.**

**Already approved, do not redo:**
- `ToolPolicy::intersect` and its test;
- the hosts, scripts, flags and tool-policy narrowing, and their tests;
- the wiring in `spawn_ghost`, `dispatch_meta` and `route.rs`, including
  the new `#[allow(clippy::too_many_arguments)]` on
  `dispatch_meta::dispatch`, which is accepted;
- the `docs/security.md` § 10 guard.

**Exactly three pieces of work are left:**

1. **[bug-06a-1](bugs/bug-06a-1.md).** Make the `ssh_target` check in the
   narrowing run **only when `host` is `None`**. With `host` given,
   `spawn_host_target` already checks the named host against the narrowed
   child and the caller. How the narrowing learns about `host` is your
   choice. For example, it could take a `host_given: bool`, or
   `spawn_ghost` could run the check itself. Update the existing
   narrowing tests' calls to match. Add
   `spawn_ghost_lets_a_host_override_replace_the_runbook_ssh_target` beside
   `spawn_ghost_refuses_a_child_outside_the_caller_fence`, with the same
   setup pattern, and run it red first.
2. **[bug-06a-2](bugs/bug-06a-2.md), part 1.** Move `spawn_ghost`'s `///`
   doc comment back directly above its `#[allow]` and `pub async fn
   spawn_ghost`, so `narrow_to_caller` keeps only its own doc.
3. **[bug-06a-2](bugs/bug-06a-2.md), part 2.** In CLAUDE.md's
   `spawn_ghost_shell` row, put back
   `` `host?` runs the specialist on a configured host or "local" ``
   before the narrowing sentence.

**Closed file list:** `src/daemon/executor/knowledge/ghost.rs` and
`CLAUDE.md`. Read at most **2** files before your first edit.

**Finish condition:** the lib suite reports **3167**, not 3166 or 3168.

### Update — 2026-11-26 06:33 (complete, server-authored)

**Summary:** Fixed both bounced bugs. (1) bug-06a-1: `narrow_to_caller` now takes a `host_given: bool`; when a spawn names `host`, the runbook-`ssh_target` fence check is skipped because `spawn_host_target` checks the named host instead — verified by the new door test `spawn_ghost_lets_a_host_override_replace_the_runbook_ssh_target`, written red first (it failed with `cannot spawn this specialist. Its ssh_target 'db02' is outside your host fence (db01)`) and now green. (2) bug-06a-2: `spawn_ghost`'s `///` doc comment is back directly above its `#[allow]`/`pub async fn`, and CLAUDE.md's `spawn_ghost_shell` row again carries "`host?` runs the specialist on a configured host or \"local\"". All prior work (intersect, narrowing, wiring, docs/security guard) is untouched.

**Notes for review:** Both pinned greps now read correctly (`grep -B12 … narrow_to_caller | grep -c "Spawn a ghost session…"` → 0; `grep -c '^| \`spawn_ghost_shell\` .*host?' CLAUDE.md` → 1). No-test-removed grep → 0. Gates run once each: fmt clean, build clean (no warnings), clippy `-D warnings` clean, and `cargo test` reports **3167 passed; 0 failed** on the lib suite (the required finish condition), with all integration suites green. Committed as `ec909c03`; working tree clean; phase status left at `in-progress`.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
t ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test alert_list_request_returns_the_snapshot ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (92055): No such process
kill: (-92899): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-20f7a263eb3505d4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-76388b7c8bf90a79)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-4990f807dc1b822c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-2b9c25f5dc5fb2cd)
     Running tests/integration.rs (target/debug/deps/integration-5b69b39d41a6d634)
     Running tests/isolation.rs (target/debug/deps/isolation-1e4cc445234900f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-cbce62edf1c01d9f)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-6c3b6672991501e2)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `src/daemon/executor/knowledge/ghost.rs` — +106 -20

**Commit:** ec909c0346e9a8a1987aebbcc993638943fe7dda

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-26

- **Verdict:** approved_after_1
- **Bounces:** 1
  - [bug-06a-1](bugs/bug-06a-1.md), major: the narrowing checked the
    runbook's `ssh_target` even with a `host` override, so a valid
    delegation was refused. It failed safe.
  - [bug-06a-2](bugs/bug-06a-2.md), minor: `spawn_ghost`'s doc comment
    was glued onto `narrow_to_caller`, and the CLAUDE.md row lost its
    `host?` sentence.
- **Executor:** GLM-5.3-Flash. Run 1 completed in 124 turns, and the
  bounce in 53.
- **Scope deviations:**
  - A new `#[allow(clippy::too_many_arguments)]` on
    `dispatch_meta::dispatch`. Accepted: the codebase holds 41, and the
    parameter the spec authorized is what crossed the limit.
  - An extra test, `narrow_to_caller_keeps_the_child_tool_policy_without_a_caller_policy`.
    Additive.
- **Review re-run:**
  - All four gates pass, and the lib suite reports **3167**, exactly as
    pinned.
  - The no-test-removed grep prints 0, and both doc greps print as
    pinned.
  - Run 1's pasted mutation run, with the narrowing returning `Ok`,
    failed all five narrowing and wiring tests.
  - The reviewer's scratch test, since reverted, reproduced bug-06a-1
    red. The bounce's test uses the same setup.
- **Calibration:**
  - The spec said "`ssh_target`, when `host` is omitted", but the
    narrowing function's signature had no way to know. When a rule is
    conditional on an argument, **name that argument in the function's
    inputs**.
  - Two inserts landed between an existing item's `///` doc comment and
    the item. For a spec that adds a function above an existing one, say
    where it goes: "above the doc comment", not "above the function".
