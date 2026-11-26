# Phase 06a — ghost-fence

**Milestone:** M40 — Security hardening
**Status:** in-progress
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
- [ ] **Gates:** all four pass, run once each.

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
