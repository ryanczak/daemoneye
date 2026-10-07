# Phase 08b: sbx-egress-audit — record each sandbox's allowed and blocked hosts

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
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
