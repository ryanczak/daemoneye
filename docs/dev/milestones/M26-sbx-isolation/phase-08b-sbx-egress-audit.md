# Phase 08b: sbx-egress-audit — record each sandbox's allowed and blocked hosts

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** done
**Depends on:** phase-08a
**Estimated diff:** ~300 lines (about 110 code, 190 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Just before a daemon sandbox is torn down, read its sbx egress log (`sbx
policy log <name> --json`) and record the hosts it reached and the hosts the
proxy blocked:
- **ghost run:** as a `Note` in the run's turn log, which `run_log_tail` and
  `run_log_search` already show, and as a `sandbox_egress` event in
  `events.jsonl`;
- **chat sandbox:** as a `sandbox_egress` event when the reaper removes it.

Today nothing records a sandbox's network activity.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md`, row 08b.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Measured on pinky, 2026-10-07 (probe sandbox `de-probe-audit`)

The probe ran one `curl` to `https://example.com`, which the default-deny
proxy blocked, and one to `https://github.com`, which was allowed. Then
`sbx policy log de-probe-audit --json` printed (trimmed to three allowed
entries; the real output had five):

```json
{
  "blocked_hosts": [
    {
      "host": "example.com:443",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward",
      "rule": "no applicable policies for op(action=net:connect:tcp, resource=net:domain:example.com:443)",
      "last_seen": "2026-10-07T13:22:09.930609713-07:00",
      "since": "2026-10-07T13:22:09.930609713-07:00",
      "count_since": 1,
      "reason": "No matching allow rule (default deny)"
    }
  ],
  "allowed_hosts": [
    {
      "host": "github.com:443",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward-bypass",
      "rule": "",
      "last_seen": "2026-10-07T13:22:10.315805171-07:00",
      "since": "2026-10-07T13:22:10.315805171-07:00",
      "count_since": 1
    },
    {
      "host": "security.ubuntu.com:80",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward",
      "rule": "",
      "last_seen": "2026-10-07T13:22:10.14940568-07:00",
      "since": "2026-10-07T13:22:10.14940568-07:00",
      "count_since": 1
    },
    {
      "host": "download.docker.com:443",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward-bypass",
      "rule": "",
      "last_seen": "2026-10-07T13:22:09.71884377-07:00",
      "since": "2026-10-07T13:22:09.71884377-07:00",
      "count_since": 1
    }
  ]
}
```

More measured facts:
- A sandbox that never existed gives
  `{"blocked_hosts": [], "allowed_hosts": []}` with exit 0.
- The log **survives `sbx rm`**, and it accumulates across sandboxes that
  reuse a name. That is why it is read **before** teardown, while the
  sandbox's own entries are current.

### The ghost exit sequence (`src/daemon/ghost.rs`, `trigger_ghost_turn`)

```rust
    let state = ghost_terminal_state(&result, &outcome, cancel_stage(run_log.run_id()), session_id);
    run_log.finish(state.clone());          // ← writes RunFinished to the turn log
    … mailbox …
    … close_owner_shells …
    // 06c: the ghost's sbx sandbox dies with the ghost, after its shells do.
    if config.sandbox.enabled && config.sandbox.runtime == "sbx" {
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        let outcome = crate::daemon::executor::ghost_sandbox::teardown_ghost_sandbox(
            &crate::shell::real_runner(program), session_id).await;
        crate::daemon::utils::log_event("ghost_sandbox_teardown",
            serde_json::json!({ "session": session_id, "outcome": format!("{outcome:?}") }));
    }
```

`ghost.rs` already imports `TurnLogEvent` and `TurnLogRecord`. A note is
written like this elsewhere in the file:

```rust
append_turn_event(&TurnLogRecord {
    timestamp: chrono::Utc::now().to_rfc3339(),
    run_id: run_id.clone(),
    event: TurnLogEvent::Note { text },
})
```

### The chat reaper (`src/daemon/executor/chat_sandbox.rs`)

```rust
pub async fn reap_idle_in(
    map: &Mutex<HashMap<String, u64>>, run: &SbxRunner, live: &HashSet<String>, now_unix: u64,
) -> Vec<(String, super::sbx_lifecycle::Teardown)> {
    let idle = {
        let mut stamps = map.lock().unwrap_or_log();
        let idle = idle_chat_sandboxes(&stamps, live, now_unix, CHAT_SANDBOX_IDLE_GRACE_SECS);
        for name in &idle { stamps.remove(name); }
        idle
    };
    let mut outcomes = Vec::with_capacity(idle.len());
    for name in idle {
        let outcome = super::ghost_sandbox::forget_and_teardown(run, &name).await;
        outcomes.push((name, outcome));
    }
    outcomes
}
```

`reap_idle_chat_sandboxes(run, reg, now)` returns whatever `reap_idle_in`
returns. Its only caller is the hourly block in `src/daemon/mod.rs`:

```rust
for (name, outcome) in crate::daemon::executor::chat_sandbox::reap_idle_chat_sandboxes(
    &crate::shell::real_runner(program), crate::shell::registry(), now,
).await {
    crate::daemon::utils::log_event("chat_sandbox_teardown",
        serde_json::json!({ "sandbox": name, "outcome": format!("{outcome:?}") }));
}
```

### The created set (`src/daemon/executor/ghost_sandbox.rs`)

`fn created() -> &'static Mutex<HashSet<String>>` (private) holds every
sandbox name `ensure_sandbox` created or adopted in this daemon.

### The `sbx ls` parser to mirror (`src/daemon/executor/sbx.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct SandboxInfo { pub name: String, pub status: String }

#[derive(serde::Deserialize)]
struct LsOutput { sandboxes: Vec<SandboxInfo> }

pub fn parse_ls_json(text: &str) -> Option<Vec<SandboxInfo>> {
    serde_json::from_str::<LsOutput>(text).ok().map(|o| o.sandboxes)
}
```

## Spec

### Task 1 — Pure parsing and formatting in `sbx.rs`

```rust
/// Argv for one sandbox's egress log: `policy log <name> --json`.
pub fn policy_log_args(sandbox: &str) -> Vec<String>

/// One host the proxy saw. Extra fields (`proxy_type`, `rule`, `since`, …)
/// are ignored by serde.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EgressEntry { pub host: String, pub vm_name: String, pub count_since: u64 }

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressAudit { pub allowed: Vec<EgressEntry>, pub blocked: Vec<EgressEntry> }

/// `None` for text that is not the policy-log JSON. Entries whose
/// `vm_name` is not `sandbox` are dropped.
pub fn parse_policy_log(text: &str, sandbox: &str) -> Option<EgressAudit>

/// The one-line turn-log note.
pub fn egress_summary(audit: &EgressAudit) -> String

/// The `sandbox_egress` event payload.
pub fn egress_event(sandbox: &str, audit: &EgressAudit) -> serde_json::Value
```

- `parse_policy_log` deserializes a private `struct PolicyLogOutput {
  #[serde(default)] allowed_hosts: Vec<EgressEntry>, #[serde(default)]
  blocked_hosts: Vec<EgressEntry> }`, mirroring `parse_ls_json`.
- `egress_summary`:
  - both lists empty → exactly `sandbox egress: no network requests`;
  - otherwise `sandbox egress: allowed <A>; blocked <B>`. Each of `<A>` and
    `<B>` is the entries **sorted by host**, each rendered as `<host>
    (<count_since>)` and joined with `, `, or `none` when that list is empty.
- `egress_event` → `json!({ "sandbox": sandbox, "allowed": audit.allowed,
  "blocked": audit.blocked })`.

### Task 2 — Read it (`sbx_lifecycle.rs`)

```rust
/// The sandbox's egress log, or `None` (with a `log::warn!`) when sbx fails
/// or prints something unrecognised. Never fails the caller.
pub async fn read_egress(run: &SbxRunner, sandbox: &str) -> Option<sbx::EgressAudit>
```

Run `policy_log_args(sandbox)`. Exit `Some(0)` and `parse_policy_log`
returns `Some` → that audit. Anything else → `None`.

### Task 3 — `ghost_sandbox::is_created`

`pub fn is_created(name: &str) -> bool`: whether `created()` holds `name`.
Use `.unwrap_or_log()` on the lock.

### Task 4 — Ghost runs (`ghost.rs`)

Immediately **before** `run_log.finish(state.clone());`:

```rust
if config.sandbox.enabled && config.sandbox.runtime == "sbx" {
    let name = crate::daemon::executor::sbx::sandbox_name(
        crate::daemon::executor::sbx::SandboxKind::Ghost, session_id);
    if crate::daemon::executor::ghost_sandbox::is_created(&name) {
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        if let Some(audit) = crate::daemon::executor::sbx_lifecycle::read_egress(
            &crate::shell::real_runner(program), &name).await
        {
            append_turn_event(&TurnLogRecord {
                timestamp: chrono::Utc::now().to_rfc3339(),
                run_id: run_log.run_id().clone(),
                event: TurnLogEvent::Note { text: crate::daemon::executor::sbx::egress_summary(&audit) },
            });
            crate::daemon::utils::log_event("sandbox_egress",
                crate::daemon::executor::sbx::egress_event(&name, &audit));
        }
    }
}
```

`is_created` keeps a ghost that never ran a sandboxed command from getting a
misleading "no network requests" note. Writing the note before `finish`
means the turn log reads in order: the egress note, then `RunFinished`.

### Task 5 — Chat sandboxes (`chat_sandbox.rs`, `daemon/mod.rs`)

1. Add:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct ReapedSandbox {
       pub name: String,
       pub egress: Option<super::sbx::EgressAudit>,
       pub teardown: super::sbx_lifecycle::Teardown,
   }
   ```
2. `reap_idle_in` and `reap_idle_chat_sandboxes` return
   `Vec<ReapedSandbox>`. For each idle name, `reap_idle_in` first calls
   `super::sbx_lifecycle::read_egress(run, &name).await`, then
   `forget_and_teardown`. The pick-and-remove critical section is
   unchanged.
3. In the hourly block, for each `ReapedSandbox`: if `egress` is `Some(a)`,
   `log_event("sandbox_egress", sbx::egress_event(&r.name, &a))`. Then log
   `chat_sandbox_teardown` as today, with `outcome:
   format!("{:?}", r.teardown)`.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] Each of these eight tests reports `1 passed; 0 failed` when run alone
      (`cargo test --lib <name>`):
      `policy_log_args_match_the_measured_shape`,
      `parse_policy_log_reads_the_measured_output`,
      `parse_policy_log_drops_other_sandboxes_and_rejects_garbage`,
      `egress_summary_lists_hosts_sorted_with_counts`,
      `egress_event_carries_the_sandbox_and_hosts`,
      `read_egress_parses_a_successful_log`,
      `read_egress_is_none_on_failure`,
      `is_created_tracks_the_created_set`.
- [ ] `reap_idle_in_tears_down_only_idle_sandboxes` passes **as amended in
      § Test plan**. It is the one existing test this phase may edit.
- [ ] `cargo test --lib executor::sbx`, `executor::sbx_lifecycle`,
      `executor::ghost_sandbox` and `executor::chat_sandbox` each report
      `0 failed`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code. `container.rs` and `src/ipc.rs` are unchanged.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

`sbx.rs`:
- `policy_log_args_match_the_measured_shape`: `policy_log_args("de-ghost-r1")`
  == `["policy", "log", "de-ghost-r1", "--json"]`.
- `parse_policy_log_reads_the_measured_output`: paste the measured JSON
  above verbatim as a raw string, with sandbox `de-probe-audit`. The result
  has 1 blocked entry (`example.com:443`, count 1) and 3 allowed entries,
  the first `github.com:443`. The extra fields, including `reason`, must
  not break parsing.
- `parse_policy_log_drops_other_sandboxes_and_rejects_garbage`: a log with
  one allowed entry for `vm_name` `de-ghost-a` and one for `de-ghost-b`,
  parsed for `de-ghost-a`, yields one allowed entry. `"not json"` → `None`.
  `{"blocked_hosts": [], "allowed_hosts": []}` → `Some(EgressAudit::default())`.
- `egress_summary_lists_hosts_sorted_with_counts`:
  - allowed `[zeta.io:443 ×2, alpha.io:443 ×1]` and blocked
    `[example.com:443 ×1]` → exactly
    `sandbox egress: allowed alpha.io:443 (1), zeta.io:443 (2); blocked example.com:443 (1)`;
  - allowed only `[a.io:443 ×3]` →
    `sandbox egress: allowed a.io:443 (3); blocked none`;
  - empty → `sandbox egress: no network requests`.
- `egress_event_carries_the_sandbox_and_hosts`: the event's `"sandbox"` is
  the name, `"allowed"[0]["host"]` and `"count_since"` match the entry, and
  `"blocked"` is an array of the blocked length.

`sbx_lifecycle.rs` (its own `fake()` / `ok_run` / `run` fixtures):
- `read_egress_parses_a_successful_log`: `ok_run(<a one-entry log for
  de-ghost-r1>)` → `Some` with that entry. The calls are exactly
  `[sbx::policy_log_args("de-ghost-r1")]`.
- `read_egress_is_none_on_failure`: `run(Some(1), "", "error: daemon gone")`
  → `None`. `ok_run("garbage")` → `None`.

`ghost_sandbox.rs`:
- `is_created_tracks_the_created_set`: `"de-ghost-t-isc"` is not created;
  after inserting it into `created()` it is; after removing it, it is not.
  Remove it at the end.

`chat_sandbox.rs`, **the amended existing test**
`reap_idle_in_tears_down_only_idle_sandboxes`. Keep its setup and its two
`map` assertions. Change only:
- `got` must equal `vec![ReapedSandbox { name: "de-chat-t-old".into(),
  egress: None, teardown: Teardown::Removed }]`. The fake answers the
  policy-log call with empty stdout, which is not the log JSON, so egress
  is `None`.
- `calls` must equal
  `vec![sbx::policy_log_args("de-chat-t-old"), sbx::rm_args(&["de-chat-t-old".to_string()])]`.
  The log is read before the teardown.

**Reviewer mutation:** in `parse_policy_log`, drop the `vm_name` filter, so
`parse_policy_log_drops_other_sandboxes_and_rejects_garbage` fails.

## End-to-end verification

```sh
for t in policy_log_args_match_the_measured_shape parse_policy_log_reads_the_measured_output parse_policy_log_drops_other_sandboxes_and_rejects_garbage egress_summary_lists_hosts_sorted_with_counts egress_event_carries_the_sandbox_and_hosts read_egress_parses_a_successful_log read_egress_is_none_on_failure is_created_tracks_the_created_set reap_idle_in_tears_down_only_idle_sandboxes; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
for m in executor::sbx executor::sbx_lifecycle executor::ghost_sandbox executor::chat_sandbox; do cargo test --lib "$m" 2>&1 | grep -E '^test result' | sed "s/^/$m: /"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.
`executor::sbx` also matches the `executor::sbx_lifecycle` tests. That is
expected.

## Authorizations

- [x] May edit `trigger_ghost_turn` in `src/daemon/ghost.rs`, **only** to add
      Task 4's block before `run_log.finish`.
- [x] May edit the hourly block in `src/daemon/mod.rs`, **only** as Task 5.3
      says.
- [x] May amend `reap_idle_in_tears_down_only_idle_sandboxes`, **only** its
      `got` and `calls` assertions, as § Test plan says.

## Out of scope

- Showing egress in `daemoneye status` / `sandbox status` (08a's section).
- Filesystem policy logs (`--type filesystem`, which sbx v0.46.0 does not
  support yet).
- A live run with real egress: row 10.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-07 17:02 (started)

Executor dispatched (Claude Sonnet 4.5 via rexyMCP). Status flipped
`todo → in-progress` here and in the milestone README's phase table. Starting
with the pure parsing and formatting in `sbx.rs` (Task 1), then the runner in
`sbx_lifecycle.rs` (Task 2), `is_created` (Task 3), the ghost and reaper
wiring (Tasks 4–5), and the end-to-end capture (Task 6).

### Update — 2026-10-07 17:20 (end-to-end verification)

All six tasks implemented. The E2E block from § End-to-end verification, run
verbatim, output redirected to `/tmp/e2e-08b.txt` and pasted below, every line
as printed:

```
policy_log_args_match_the_measured_shape: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
parse_policy_log_reads_the_measured_output: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
parse_policy_log_drops_other_sandboxes_and_rejects_garbage: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
egress_summary_lists_hosts_sorted_with_counts: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
egress_event_carries_the_sandbox_and_hosts: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
read_egress_parses_a_successful_log: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
read_egress_is_none_on_failure: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
is_created_tracks_the_created_set: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
reap_idle_in_tears_down_only_idle_sandboxes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2370 filtered out; finished in 0.00s
executor::sbx: test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 2307 filtered out; finished in 0.00s
executor::sbx_lifecycle: test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 2348 filtered out; finished in 0.00s
executor::ghost_sandbox: test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 2352 filtered out; finished in 0.04s
executor::chat_sandbox: test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2365 filtered out; finished in 0.00s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
fmt_exit=0
passed=2421 failed=0
 docs/dev/milestones/M26-sbx-isolation/README.md    |   2 +-
 .../phase-08b-sbx-egress-audit.md                  |  10 +-
 src/daemon/executor/chat_sandbox.rs                |  47 +++-
 src/daemon/executor/ghost_sandbox.rs               |  18 ++
 src/daemon/executor/sbx.rs                         | 244 +++++++++++++++++++++
 src/daemon/executor/sbx_lifecycle.rs               |  82 +++++++
 src/daemon/ghost.rs                                |  32 +++
 src/daemon/mod.rs                                  |  16 +-
 8 files changed, 435 insertions(+), 16 deletions(-)
```

PASTE MATCH

### Update — 2026-10-08 00:18 (complete, server-authored)

**Summary:** </think>All nine tests individually pass, fmt clean, working tree clean. All acceptance criteria met:

- [x] Eight named tests each pass alone (verified above, plus reap_idle_in amended test)
- [x] reap_idle_in_tears_down_only_idle_sandboxes passes as amended
- [x] Module runs 0 failed
- [x] clippy exit 0
- [x] fmt --check exit 0
- [x] cargo test 0 failed (2421 passed)
- [x] No unwrap/expect/allow/unsafe added outside tests
- [x] container.rs and src/ipc.rs unchanged
- [x] Update Log entry contains E2E output and PASTE MATCH line

Status left at in-progress, no complete entry written by me, server authors that. Done — write the summary.</think>
Phase 08b is complete and committed. Working tree is clean, all gates green.

**Summary:** Implemented the sbx egress audit exactly per spec: `sbx.rs` gained `policy_log_args`, `EgressEntry`/`EgressAudit`, `parse_policy_log` (with the `vm_name` filter that drops other sandboxes' accumulated entries), `egress_summary` (sorted, counted, `none`/`no network requests` forms), and `egress_event`; `sbx_lifecycle::read_egress` runs it through the injectable `SbxRunner` and degrades to `None` + `log::warn!` on any sbx failure or unrecognised output; `ghost_sandbox::is_created` exposes the created set; `trigger_ghost_turn` now reads the log immediately before `run_log.finish` (so the turn log reads note-then-finished) and emits the Note plus `sandbox_egress`, skipping ghosts that never created a sandbox; the reaper path returns `ReapedSandbox { name, egress, teardown }` with the log read before `forget_and_teardown`, and the hourly block in `daemon/mod.rs` logs `sandbox_egress` when egress exists, then `chat_sandbox_teardown` as before. End-to-end block ran verbatim and was pasted mechanically into the Update Log: all nine named tests pass individually (`1 passed; 0 failed`), the four module filters report `0 failed` (64/23/19/6 passed), clippy `-D warnings` exit 0, `fmt_exit=0`, full suite `passed=2421 failed=0`, and the self-check printed `PASTE MATCH` (now inside the entry).

**Notes for review:** One small adaptation: Task 4's block binds `run_id` to a local before building the `TurnLogRecord` (instead of `run_log.run_id().clone()` inline) purely so the borrow of `run_log` doesn't collide with the field-ordering of the struct literal — semantics identical to the spec sketch. The amended `reap_idle_in_tears_down_only_idle_sandboxes` keeps its setup and both `map` assertions, changing only the `got`/`calls` assertions as authorized. No `unwrap`/`expect`/`allow`/`unsafe` outside `#[cfg(test)]`; `container.rs` and `src/ipc.rs` are untouched. Status is left `in-progress` (phase doc + README row) with no self-written completion entry, per the lifecycle split — the server authors the `in-progress → review` flip and the `(complete)` entry. Out-of-scope items (egress in `daemoneye status`, filesystem policy logs, live egress run row 10) untouched.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
r_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test claude_md_tools_table_matches_the_code ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_tools_counts_are_accurate ... ok
test readme_tools_tables_match_the_code ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.41s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (821491): No such process
kill: (-822199): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M26-sbx-isolation/README.md` — +1 -1
- `docs/dev/milestones/M26-sbx-isolation/phase-08b-sbx-egress-audit.md` — +45 -1
- `src/daemon/executor/chat_sandbox.rs` — +35 -12
- `src/daemon/executor/ghost_sandbox.rs` — +18 -0
- `src/daemon/executor/sbx.rs` — +244 -0
- `src/daemon/executor/sbx_lifecycle.rs` — +82 -0
- `src/daemon/ghost.rs` — +32 -0
- `src/daemon/mod.rs` — +14 -2

**Commit:** 8f12312801a948bf2eeb1c3a702eef386ab49d68

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-07

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (109 turns,
  `8f123128`).
- **Scope deviations:** none. The parser mirrors `parse_ls_json` and
  filters on `vm_name`. The ghost note is written before `run_log.finish`
  (`ghost.rs` lines 568 and 578), and only when `is_created`. `reap_idle_in`
  reads the log before `forget_and_teardown`. The one authorized test
  amendment touched only its `got` and `calls` assertions. `src/ipc.rs` and
  `container.rs` are unchanged.
- **Reviewer re-run:** fmt, build and clippy clean; all nine named tests
  `1 passed`; `sbx_lifecycle` 23, `ghost_sandbox` 19 and `chat_sandbox` 6
  passed; full suite `passed=2421 failed=0` on the first run.
- **Reviewer mutation check:** dropping the `vm_name` filter fails
  `parse_policy_log_drops_other_sandboxes_and_rejects_garbage`.
- **Calibration:** none.
- **Deferred:** a real ghost run with egress, checking the turn-log note and
  the `sandbox_egress` event, is part of row 10.
