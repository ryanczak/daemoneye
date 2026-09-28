# Phase 08d: spawn-ghost-host — a coordinator names the host a specialist runs on, and a ghost's own command honours `host`

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** phase-08a (`ssh_target` resolution + the `hosts:` fence),
phase-08c (a resolved host runs on a ghost-owned PTY shell)
**Estimated diff:** ~350 lines across ~12 files, plus 6 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Three things, all on the ghost side:

1. **`spawn_ghost_shell { host? }`** — a caller names the host the specialist
   ghost's commands run on. It overrides the child runbook's `ssh_target`,
   and it is **refused before any ghost starts** unless: it is a configured
   `[hosts]` entry (or `"local"`), the **child** runbook's `hosts:` list
   grants it, and — when the caller is itself a ghost — the **caller's**
   `hosts:` list grants it too (PE, 2026-09-28).
2. **A ghost's own `run_terminal_command { host? }`** is honoured instead of
   silently dropped: `"local"` runs on the daemon host, a configured host runs
   there through 08c's PTY path, and the ghost's `hosts:` list fences it —
   exactly as if the runbook's `ssh_target` had named that host (PE: same row).
3. **Fix a measured 08a defect** in the path both of the above use: a ghost
   with **no** `ssh_target` but a non-empty `hosts:` list has every
   background command refused with a garbled message.

Tool count is unchanged — `host` is a parameter (`doc_truth` guards counts).

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — delegation to a named host.
- `docs/dev/milestones/M21-host-model/README.md` § Notes — the 08 split and
  its PE decisions; 06d (the `hosts:` fence).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-28** at commit `6ac74e45`, on a clean tree.

Baseline: `cargo test --lib` → **2058 passed / 0 failed / 11 ignored**.

### 1. The 08a defect (measured)

`GhostPolicy::may_use_ssh_target` (`src/daemon/policy.rs:62-67`):

```rust
    pub fn may_use_ssh_target(&self) -> bool {
        let Some(host) = self.remote_host.as_ref() else {
            return self.hosts.is_empty();
        };
        self.may_use_host(host)
    }
```

`run_background`'s fence (`src/daemon/executor/foreground.rs:958-960`) calls
it for **every** ghost command, with no check that an `ssh_target` exists.
Measured by driving `run_background` through `refusal_gate_tests`'
`drive_run_background_refusal` with a policy of `hosts: ["db01"]`,
`ssh_target: None`, `remote_host: None`, command `echo hi`:

```
Error: this runbook's ssh_target  is not a configured host, so its `hosts:` list cannot grant it. Hosts this runbook allows: db01 (1). Configure a [hosts.<name>] entry whose ssh is , or work on the daemon host.
```

A ghost that works on the daemon host is refused for having a `hosts:` list.
Both new features below produce exactly this policy shape (`host: "local"`),
so the fix is part of this row: **with no `ssh_target` there is nothing to
fence** — the fence applies only when `ssh_target` is `Some`. An
`ssh_target` that did not resolve keeps 08a's rule (only an empty list grants
it).

### 2. How a spawn builds the child today

`knowledge::spawn_ghost` (`src/daemon/executor/knowledge/ghost.rs:8-58`)
loads the runbook, merges the agent into a `GhostConfig`, bumps
`spawn_depth`, and calls `GhostManager::start_session_with_config`, which
runs 08a's `apply_ssh_target_resolution` on the config's `ssh_target`. So
**setting `ghost_config.ssh_target` before that call** is all a spawn host
needs: 08a resolves a `[hosts]` key by key match, 08c then routes the child's
commands onto a PTY shell there. The executor arm
(`src/daemon/executor/mod.rs:1278-1297`) has the **caller's** policy in
scope as `ghost_policy` (`Option<GhostPolicy>`, `None` for a chat caller).

`spawn_ghost_shell`'s `ParamDef`s (`src/ai/tools/defs.rs:1078-1101`):
`runbook`, `message`, `agent`. The value travels
`SpawnGhostArgs` (`src/ai/tools/args.rs:249-253`, `to_event` at `:821`) →
`AiEvent::SpawnGhost` (`src/ai/types/events.rs:236`) → `PendingCall::SpawnGhost`
(`src/ai/types/pending.rs:175`), converted in `src/daemon/stream.rs:788-800`
(chat) and `src/daemon/ghost.rs:1147-1160` (ghost).

### 3. The ghost's dropped `host`

`src/daemon/ghost.rs:912-930` turns every ghost `run_terminal_command` into
`PendingCall::Background` and drops `host`:

```rust
                        shell: _shell,
                        // Ghost Shell host routing is row 08d, not this row;
                        // every ghost command stays on the daemon host.
                        host: _host,
                        timeout_secs: _timeout_secs,
                    } => {
                        pending_calls.push(PendingCall::Background {
                            id,
                            cmd: command,
                            thought_signature,
                            _credential: None,
                        });
```

`PendingCall::Background` (`pending.rs`) has no `host` field. Its sites:
`pending.rs:359` (`to_tool_call`), `:900`, `:1141`, and tests `:1322`,
`:1535`, `:1560`; `ghost.rs:923`, `:1242`, `:1261`; `stream.rs:333` (chat —
see Out of scope); `executor/mod.rs:670`.

`run_background`'s ghost gates read the policy's `ssh_target`,
`remote_host` and `remote_sudo_scripts`; `resolve_command`,
`remote_script_call` and `wrap_remote` read `ssh_target`; 08c's
`ghost_remote_host` reads `remote_host`. So **a per-command host is exactly a
per-command policy with those three fields rebound** — every gate and route
then works unchanged.

### 4. What exists to reuse

- `HostId::parse` (`src/host.rs:28`): `"local"` (any case) → `Local`; a bad
  name → `Err` with a sentence naming the grammar.
- `GhostPolicy::may_use_host` (`policy.rs:129-136`): `Local` always; a named
  host when `hosts` is empty or lists it. `GhostPolicy` derives `Clone`.
- `ghost_host_refusal(policy, host)` (`executor/mod.rs:181`, private) — the
  refusal sentence naming the host and the allowed set.
  `knowledge/ghost.rs` is a descendant of `executor`, so it may call it as
  `super::super::ghost_host_refusal`.
- The `ghost_host_refused` event shape (`foreground.rs`, after the fence):
  `log_event("ghost_host_refused", json!({"session": …, "tool": …,
  "requested_host": …, "allowed_hosts": …}))`.

## Spec

### Task 1 — the fence only fences an `ssh_target` (`src/daemon/policy.rs`)

`may_use_ssh_target`: with `remote_host: None`, return `true` when
`ssh_target` is `None` (nothing to fence) and `self.hosts.is_empty()` when
it is `Some` (08a's unresolved rule, unchanged). The resolved branch is
unchanged. Update the doc comment to say so.

### Task 2 — a policy rebound to a host (`src/daemon/policy.rs`)

```rust
/// This policy with its remote binding replaced by `host`, as if the
/// runbook's `ssh_target` had named it. Every other field is kept.
pub fn for_host(
    &self,
    host: &crate::host::HostId,
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
) -> Result<GhostPolicy, String>
```

- `Local` → a clone with `ssh_target: None`, `remote_host: None`,
  `remote_sudo_scripts: vec![]`.
- `Named(k)` present in `hosts` → a clone with `ssh_target: Some(k)`,
  `remote_host: Some(Named(k))`, `remote_sudo_scripts:
  hosts[k].sudo_scripts.clone()`.
- `Named(k)` absent → `Err` naming `k` and listing the configured hosts
  (sorted, comma-separated; say "no hosts are configured" when empty).

It does **not** check the `hosts:` list — `run_background`'s fence does that
next, with its existing wording and event.

### Task 3 — a ghost's command honours `host` (`pending.rs`, `ghost.rs`, `stream.rs`, `executor/mod.rs`, `foreground.rs`)

1. `PendingCall::Background` gains `host: Option<String>`. `to_tool_call`
   adds `"host": host` to its JSON. Every construction site sets it; the
   tests at `pending.rs:1322/1535/1560` use `host: None`.
2. `ghost.rs:912-930`: bind `host` and pass it; delete the
   "row 08d, not this row" comment.
3. `stream.rs:333` (chat, `wait: false`): `host: None` — chat keeps its
   documented behaviour.
4. `executor/mod.rs:670`: destructure `host` and pass `host.as_deref()` to
   `foreground::run_background` as a new `host: Option<&str>` parameter
   (after `cmd`). The chat PTY branch above it is unchanged.
5. In `run_background`, **before** `remote_script` is computed (`:944`),
   rebind the policy for a ghost that named a host:

   ```rust
   let rebound;
   let ghost_policy = match (is_ghost, host, ghost_policy) {
       (true, Some(name), Some(policy)) => {
           let refusal = match crate::host::HostId::parse(name) {
               Err(e) => Err(format!("Error: {e}")),
               Ok(h) => policy
                   .for_host(&h, &crate::config::Config::load().unwrap_or_default().hosts)
                   .map_err(|e| format!("Error: {e}")),
           };
           match refusal {
               Ok(p) => {
                   rebound = p;
                   Some(&rebound)
               }
               Err(msg) => { /* send ToolResult(msg), log_command, return Ok(Result(msg)) */ }
           }
       }
       _ => ghost_policy,
   };
   ```

   Send the refusal exactly as the fence below does (`send_response_split`
   → `Response::ToolResult`, then `return Ok(ToolCallOutcome::Result(msg))`).
   From there on every existing line runs against the rebound policy: the
   `hosts:` fence refuses an unlisted host with `ghost_host_refusal` and the
   `ghost_host_refused` event; 08c routes a resolved host to its PTY path;
   `"local"` runs on the daemon host.
6. Update `refusal_gate_tests::drive_run_background_refusal` to take
   `host: Option<&str>` and pass it; its two existing callers pass `None`.

### Task 4 — `spawn_ghost_shell { host? }` (`defs.rs`, `args.rs`, `events.rs`, `pending.rs`, `stream.rs`, `ghost.rs`, `executor/mod.rs`, `knowledge/ghost.rs`)

1. `defs.rs`: a fourth `ParamDef` on `spawn_ghost_shell`:

   ```rust
   ParamDef {
       name: "host",
       ty: ParamTy::Str,
       required: false,
       description: "Run the specialist's commands on this configured \
                     [hosts.<name>] host instead of its runbook's ssh_target; \
                     \"local\" means the daemon host. Refused before the ghost \
                     starts unless the host is configured and the specialist's \
                     runbook hosts: list grants it — and, when you are a ghost, \
                     your own hosts: list too.",
   },
   ```

2. Thread `host: Option<String>` through `SpawnGhostArgs` + its `to_event`,
   `AiEvent::SpawnGhost`, `PendingCall::SpawnGhost` (+ `to_tool_call`'s JSON,
   `"host": host`), both conversions (`stream.rs:788`, `ghost.rs:1147`), and
   the tests that spell the variant out (`pending.rs:1299`, `:1439`,
   `:1709` → `host: None`).
3. In `knowledge/ghost.rs`, a pure decision:

   ```rust
   /// The `ssh_target` a spawn with `host` gives the child: `Ok(None)` for
   /// "local", `Ok(Some(key))` for a granted configured host, `Err(sentence)`
   /// otherwise. Checked, in order: the name parses; it is configured; the
   /// child's `hosts:` grants it; the caller's (a ghost) grants it.
   pub(super) fn spawn_host_target(
       host: &str,
       child: &crate::ipc::GhostConfig,
       caller: Option<&crate::daemon::policy::GhostPolicy>,
       hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
   ) -> Result<Option<String>, String>
   ```

   The unconfigured sentence names the host and the configured set (as
   Task 2); the two fence sentences are `ghost_host_refusal(&policy, &host)`
   for `GhostPolicy::from_config(child)` and for the caller respectively.
4. `spawn_ghost` gains `host: Option<&str>` and `caller: Option<&GhostPolicy>`
   (after `agent_name`). After the `spawn_depth` / `parent_job_id` lines and
   **before** `start_session_with_config`: when `host` is `Some`, call
   `spawn_host_target` with `Config::load().unwrap_or_default().hosts`
   (`spawn_config` is already loaded there); on `Ok(t)` set
   `ghost_config.ssh_target = t`; on `Err(msg)` log
   `ghost_host_refused` with `"tool": "spawn_ghost_shell"` and return
   `Ok(ToolCallOutcome::Result(msg))` — **no session is started**.
5. The executor arm (`mod.rs:1278`) passes `host.as_deref()` and
   `ghost_policy.as_ref()`.

### Task 5 — docs

1. `assets/prompts/sre.toml`, § Delegation (`:350`): the entry becomes
   `spawn_ghost_shell(runbook, message, agent?, host?)` and gains one
   sentence on `host` (the specialist's machine; configured hosts or
   "local"; refused unless both `hosts:` lists grant it).
2. `assets/prompts/sre.toml:85`: after "`wait=false` ignores `host`: a
   background command always runs on the daemon host." add: "A Ghost
   Shell's command is the exception: it honours `host`, fenced by its
   runbook's `hosts:` list."
3. `CLAUDE.md`, Current AI tools table, the `spawn_ghost_shell` row: add a
   clause containing the literal `host?` (the specialist's host, fenced by
   both `hosts:` lists). The literal matters: "Ghost" already contains the
   substring `host`, so the criterion greps for `host?`.
4. `README.md`, the `spawn_ghost_shell` row of the tool table (`:528`):
   the same clause, with the literal `host?`.

### Task 6 — tests

Six tests; names pinned.

In `src/daemon/policy.rs` tests:

1. `a_ghost_without_an_ssh_target_is_not_fenced_by_its_hosts_list` — a
   policy with `hosts: ["db01"]`: with `ssh_target: None` →
   `may_use_ssh_target()` is **true**; with `ssh_target: Some("user@far")`
   and `remote_host: None` → **false** (08a unchanged); with `hosts: []` and
   `ssh_target: Some("user@far")` → **true**.
2. `for_host_rebinds_a_ghost_policy_to_the_named_host` — `hosts` map with
   `db01` (`sudo_scripts: ["a.sh"]`): `Named("db01")` → `ssh_target
   Some("db01")`, `remote_host Some(Named("db01"))`, `remote_sudo_scripts
   ["a.sh"]`, `hosts` field unchanged; `Local` → all three cleared;
   `Named("nope")` → `Err` containing `nope`.

In `src/daemon/executor/knowledge/ghost.rs` tests:

3. `spawn_host_target_is_granted_only_by_both_hosts_lists` — `hosts` map
   `db01`, `db02`; child `GhostConfig { hosts: ["db01"], ..Default::default() }`:
   `"db01"`, no caller → `Ok(Some("db01"))`; `"db02"`, no caller → `Err`
   containing `db02`; `"db01"` with a caller policy whose `hosts` is
   `["db02"]` → `Err` containing `db01`; `"local"` → `Ok(None)`; `"nope"` →
   `Err` containing `nope`; `"a b"` → `Err`; child `hosts: []` and caller
   `hosts: []`, `"db02"` → `Ok(Some("db02"))`.
4. `spawn_ghost_refuses_an_unlisted_host_before_any_ghost_starts` — door test.
   Hold `crate::test_home_guard()` with `HOME` at a temp dir and
   `Config::ensure_dirs()`; write `crate::config::etc_dir().join("config.toml")`
   with `[hosts.db01]\nssh = "db01"\n\n[hosts.db02]\nssh = "db02"\n`; write
   `crate::config::runbooks_dir().join("rb08d.md")` with
   `"---\nenabled: true\nhosts: [db01]\n---\n# Runbook: rb08d\n\n## Alert Criteria\n- x\n"`
   (the fixture shape at `src/runbook.rs:585`). Call
   `spawn_ghost("rb08d", "task", None, Some("db02"), None, &sessions, 0, None)`
   → `ToolCallOutcome::Result(text)` with `text` containing `db02`, and the
   `SessionStore` still has **no** entry.

In `src/daemon/executor/foreground.rs` `refusal_gate_tests`:

5. `a_ghost_command_host_outside_its_hosts_list_is_refused` — config file
   with `[hosts.db01]` and `[hosts.db02]` (as test 4); policy `hosts:
   ["db01"]`, no `ssh_target`; `drive_run_background_refusal(…, "echo hi",
   Some("db02"))` → text contains `db02` and `db01`.
6. `a_ghost_command_host_that_is_not_configured_is_refused` — same policy,
   `Some("nope")` → text contains `nope`.

**Test count:** six new, none ignored → at least **2064** passed, **11**
ignored.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today — checked at drafting.

- [ ] `cargo test --lib` → **at least 2064 passed / 0 failed / 11 ignored**,
      on three consecutive runs (current: 2058/0/11). **The ignored count is
      exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation* — `host` is a parameter,
      so the 42-tool count and the doc tables stay in sync).
- [ ] `spawn_ghost_shell` has a `host` parameter:
      ``awk '/name: "spawn_ghost_shell"/,/deferred_group/' src/ai/tools/defs.rs | grep -c 'name: "host"'``
      → **1** (current: 0).
- [ ] The rebinding exists: ``grep -c 'pub fn for_host' src/daemon/policy.rs``
      → **1** (current: 0); ``grep -c 'fn spawn_host_target' src/daemon/executor/knowledge/ghost.rs``
      → **1** (current: 0).
- [ ] The ghost no longer drops `host`:
      ``grep -c 'host: _host' src/daemon/ghost.rs`` → **0** (current: 1).
- [ ] Docs: ``grep -c 'spawn_ghost_shell(runbook, message, agent?, host?)' assets/prompts/sre.toml``
      → **1** (current: 0); ``grep -c "A Ghost Shell's command is the exception" assets/prompts/sre.toml``
      → **1** (current: 0); ``grep '^| `spawn_ghost_shell`' CLAUDE.md | grep -c 'host?'``
      → **1** (current: 0); ``grep '^| `spawn_ghost_shell`' README.md | grep -c 'host?'``
      → **1** (current: 0).
- [ ] All six new tests pass:

      ```sh
      cargo test --lib -- \
        a_ghost_without_an_ssh_target_is_not_fenced_by_its_hosts_list \
        for_host_rebinds_a_ghost_policy_to_the_named_host \
        spawn_host_target_is_granted_only_by_both_hosts_lists \
        spawn_ghost_refuses_an_unlisted_host_before_any_ghost_starts \
        a_ghost_command_host_outside_its_hosts_list_is_refused \
        a_ghost_command_host_that_is_not_configured_is_refused
      ```

      → **6 passed** (current: `0 passed; 0 failed; … filtered out`).
- [ ] 08a's and 08c's gates still pass (*preservation*):
      ``cargo test --lib refusal_gate_tests`` → **0 failed** (4 tests after
      this phase); ``cargo test --lib ghost_remote`` → **0 failed**.
- [ ] Mutations A–C were applied, run, and their observed results recorded.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.** Reasoned against code this phase writes —
record what you actually observe.

- **Mutation A — fence with no `ssh_target`.** Restore the old
  `return self.hosts.is_empty();` in `may_use_ssh_target`. Run test 1 —
  expected **red**.
- **Mutation B — ignore the caller.** In `spawn_host_target`, skip the
  caller check. Run test 3 — expected **red** (the caller-`db02` row).
- **Mutation C — keep `ssh_target`.** In `for_host`, leave `ssh_target`
  unchanged for a named host. Run test 2 — expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Six tests (Task 6): the fence fix and the rebinding (pure), the spawn
decision's truth table (pure), a spawn door test that proves nothing starts,
and two `run_background` door tests on the ghost command path. There is **no
live test** in this row — the substrate a named host reaches is 08c's, live-
verified there; row 09 closes the milestone live end to end.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- a_ghost_without_an_ssh_target_is_not_fenced_by_its_hosts_list for_host_rebinds_a_ghost_policy_to_the_named_host spawn_host_target_is_granted_only_by_both_hosts_lists spawn_ghost_refuses_an_unlisted_host_before_any_ghost_starts a_ghost_command_host_outside_its_hosts_list_is_refused a_ghost_command_host_that_is_not_configured_is_refused 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'refusal_tests=%s\n' "$(cargo test --lib refusal_gate_tests 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'ghost_remote_tests=%s\n' "$(cargo test --lib ghost_remote 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'spawn_host_param=%s\n' "$(awk '/name: "spawn_ghost_shell"/,/deferred_group/' src/ai/tools/defs.rs | grep -c 'name: "host"')"
  printf 'for_host=%s\n' "$(grep -c 'pub fn for_host' src/daemon/policy.rs)"
  printf 'spawn_target=%s\n' "$(grep -c 'fn spawn_host_target' src/daemon/executor/knowledge/ghost.rs)"
  printf 'dropped_host=%s\n' "$(grep -c 'host: _host' src/daemon/ghost.rs)"
  printf 'docs=%s-%s-%s-%s\n' "$(grep -c 'spawn_ghost_shell(runbook, message, agent?, host?)' assets/prompts/sre.toml)" "$(grep -c "A Ghost Shell's command is the exception" assets/prompts/sre.toml)" "$(grep '^| `spawn_ghost_shell`' CLAUDE.md | grep -c 'host?')" "$(grep '^| `spawn_ghost_shell`' README.md | grep -c 'host?')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-08d.txt
cat /tmp/e2e-08d.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=` at least
`2064p-0f-11i`, `new_tests=6p-0f`, `refusal_tests=4p-0f`,
`ghost_remote_tests=` with `0f`, `spawn_host_param=1`, `for_host=1`,
`spawn_target=1`, `dropped_host=0`, `docs=1-1-1-1`, `ignore_total=11`.

**Known pre-existing flake.** `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
fails intermittently under full-suite load. If it fails, report its name —
do not edit it — and re-run.

**Run this block with nothing else running `cargo test`.**

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-08d-spawn-ghost-host.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08d.txt
diff /tmp/pasted-08d.txt /tmp/e2e-08d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/daemon/policy.rs` — Tasks 1–2, tests 1–2.
- `src/ai/tools/defs.rs`, `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
  `src/ai/types/pending.rs` — Tasks 3.1 and 4.1–4.2.
- `src/daemon/stream.rs`, `src/daemon/ghost.rs` — the conversions (Tasks 3.2,
  3.3, 4.2).
- `src/daemon/executor/mod.rs` — the two arms (Tasks 3.4, 4.5).
- `src/daemon/executor/foreground.rs` — Task 3.5–3.6, tests 5–6.
- `src/daemon/executor/knowledge/ghost.rs` — Tasks 4.3–4.4, tests 3–4.
- `assets/prompts/sre.toml`, `CLAUDE.md`, `README.md` — Task 5 only.
- This phase doc's Update Log.

## Out of scope

- **Chat's `run_terminal_command { host, wait: false }`** — still drops
  `host` (`stream.rs:331`, documented at `sre.toml:85`). Ghost only here.
- **The approval request's `host`** — unchanged, as in 08c.
- **A spawn host for a child that is not a ghost** — `spawn_ghost_shell`
  always spawns a ghost; nothing else takes `host` here.
- **Connection-time ssh prompts** — row 08e.
- **The backend redaction-marker flake** — recorded, not fixed here.

## Update Log

<!-- entries appended below this line -->
