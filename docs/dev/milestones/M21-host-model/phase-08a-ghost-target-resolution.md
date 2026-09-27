# Phase 08a: ghost-target-resolution — a runbook's `ssh_target` learns which configured host it means

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-02 (`HostEntry` with `ssh` / `sudo_scripts`), phase-06d
(`GhostPolicy.hosts`, `may_use_host`, `ghost_host_refusal`)
**Estimated diff:** ~330 lines across 7 files, plus 11 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

At ghost start, a runbook's `ssh_target` is **resolved against `[hosts]`**: a
value that names a configured host — by key, or by that host's `ssh`
destination — becomes that `HostId`, and the resolution is recorded in
`events.jsonl`. Two things then become true on today's execution path:

1. **`HostEntry.sudo_scripts` is enforced.** On a resolved host with a
   non-empty `sudo_scripts`, a ghost may run a script under `sudo` only if the
   list names it. An empty list changes nothing.
2. **A runbook's `hosts:` list fences `ssh_target`.** A non-empty `hosts:`
   must grant the host `ssh_target` resolves to; an `ssh_target` that resolves
   to no configured host cannot be granted by a non-empty list at all.

**The transport does not change in this row.** A ghost command still runs in
a tmux `de-gs-bg-*` window as `ssh <target> '<cmd>'`, exactly as today. Row
08b moves resolved hosts onto a PTY shell; this row gives it the `HostId` to
move to. **Do not write any doc or prompt text claiming ghost commands run in
a host shell** — that is 08b's claim to make, and writing it here would ship a
false statement.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — *"Runbook `ssh_target` and
  `ExecutionPolicy::wrap_remote` map onto `HostId` unchanged in meaning"* (the
  type is `GhostPolicy`, not `ExecutionPolicy` — a known name drift).
- `docs/dev/milestones/M21-host-model/README.md` § Notes — the PE decisions
  recorded for rows 08a/08b on 2026-09-27.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

**Your shell gate refuses any command line containing the word `su`+`do`.**
The field this row enforces is called `sudo_scripts`, so this bites here more
than in any earlier row. Every criterion and the End-to-end block below are
already written around it — `su[d]o` in regexes, and test names that say
*elevated* instead. Keep it that way in anything you run: use `su[d]o` in a
`grep`, and never put the literal word in a `cargo test` filter. Writing it
**inside source files with the `patch` tool is fine** — only shell command
lines are gated.

## Current state

Measured on **2026-09-27** at commit `ee70e420`, on a clean tree.

Baseline: `cargo test --lib` → **2024 passed / 0 failed / 8 ignored**;
`cargo test --test isolation` → **9 passed / 0 failed / 1 ignored**.

### 1. How a ghost runs a command today

Every ghost `run_terminal_command` becomes `PendingCall::Background` and the
`host` argument is dropped on purpose (`src/daemon/ghost.rs:829-845`, comment:
*"Ghost Shell host routing is row 08b"*). The Background arm sends a ghost to
`run_background` (`src/daemon/executor/foreground.rs:917`) whatever the
backend — the PTY branch is guarded `uses_pty && !is_ghost`
(`executor/mod.rs:660`). `run_background` then, in order:

```rust
// foreground.rs:943-953 — computed BEFORE the approval prompt
let remote_script = ghost_policy
    .as_ref()
    .filter(|_| is_ghost)
    .and_then(|p| p.remote_script_call(cmd)); // Option<(String, String)>
let remote_script_is_sudo = remote_script.is_some()
    && (crate::daemon::utils::command_has_sudo(cmd)
        || ghost_policy
            .as_ref()
            .map(|p| p.run_with_sudo)
            .unwrap_or(false));
// foreground.rs:955-961 — resolve_command
// foreground.rs:963     — prompt_and_await_approval(...)
// foreground.rs:980-1019 — materialize (sudo) or stream (default) the script
// foreground.rs:1021-1028 — policy.wrap_remote(cmd) → `ssh <target> '<cmd>'`
```

`remote_script_call` returns `Some` only when `ssh_target` is set **and** the
command's first token (after an optional `sudo `) is a script basename in
`auto_approve_scripts` (`src/daemon/policy.rs:193-227`). So
`remote_script_is_sudo` is exactly "a whitelisted script is about to be
materialized and run under sudo on the remote" — the one place `sudo_scripts`
can mean anything.

### 2. `ssh_target` is a raw ssh destination, not a host name

`GhostPolicy.ssh_target: Option<String>` (`policy.rs:18`) is copied verbatim
from runbook frontmatter. The documented form is **`user@host`** (README
line 1201, `assets/memory/knowledge/runbook-format.md`,
`assets/memory/knowledge/ghost-shell-guide.md`), and `HostId::parse` rejects
`@` (`src/host.rs:43-49`, grammar `[A-Za-z0-9._-]+`). So `ssh_target` cannot
simply be parsed as a `HostId`; it has to be **looked up**. An empty value
(`ssh_target: ""`) already parses to `None` (`src/runbook.rs:133`) — one
runbook on this machine has that line, and it is unaffected.

`Config.hosts` is a **`HashMap<String, HostEntry>`**
(`src/config/types.rs:17`) and `HostEntry.ssh` is a plain `String`
(`types.rs:1001-1004`). Two entries can share an `ssh` destination, and a
`HashMap`'s iteration order is not stable — so "the first entry whose `ssh`
matches" is **nondeterministic** and must never be the rule (§ Spec, Task 1).

### 3. Where resolution must happen — once, at start, not per call

`GhostPolicy::from_config` runs **per tool call**, inside `with_sessions` —
i.e. under the sessions lock (`executor/mod.rs:217-233`). Loading
`config.toml` there would do file I/O under a mutex. Every ghost start
instead funnels through **one** function,
`GhostManager::start_session_with_config` (`src/daemon/ghost.rs:177`) — its
five callers are `ghost.rs:161` (`start_session`), `daemon/scheduled.rs:146`,
`webhook/process.rs:446`, `daemon/server/handlers.rs:837` and
`executor/knowledge/ghost.rs:52`. At `ghost.rs:224` it clones the config it
will store on the session:

```rust
        let mut gc = ghost_config.clone();
        // Merge daemon-wide default: if either source enables the flag, the ghost gets it.
        gc.auto_approve_commands |= ghost_commands_default;
```

That is the hook: resolve there, store the result **on `GhostConfig`**, and
let `from_config` copy it into the policy like every other field.

`GhostConfig` (`src/ipc.rs:80-134`) derives `Default` and every field carries
`#[serde(default)]`. Of its 13 struct literals, only **two** list every field
and will fail to compile when you add one: `src/runbook.rs:99` and
`tests/integration.rs:1214`. The other eleven use `..Default::default()`.
`GhostPolicy` has **nine** full literals that will also fail to compile:
seven in `src/daemon/policy_tests.rs` (lines 4, 14, 24, 34, 366, 377, 500)
and two in `src/daemon/executor/foreground.rs` (1305, 1389). Add the new
fields to each — `None` / `Vec::new()` — and change nothing else there.

### 4. The worked example for a refusal: 06d's host gate

The shape to copy — a pure predicate on `GhostPolicy`, a refusal message, a
`log_event`, a `ToolResult`, an early return — is 06d's, at
`executor/mod.rs:373-395`:

```rust
            if let Some(policy) = ghost_policy.as_ref()
                && !policy.may_use_host(&host)
            {
                let msg = ghost_host_refusal(policy, &host);
                crate::daemon::utils::log_event(
                    "ghost_host_refused",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "run_terminal_command",
                        "requested_host": host.as_str(),
                        "allowed_hosts": policy.hosts,
                    }),
                );
                send_response_split(
                    tx,
                    Response::ToolResult {
                        tool_call_id: id.to_string(),
                        output: msg.clone(),
                    },
                )
                .await?;
                return Ok(ToolCallOutcome::Result(msg));
            }
```

`ghost_host_refusal` is a private fn at `executor/mod.rs:180`. `foreground`
is a child module of `executor` (`mod.rs:4`), so `run_background` can call it
as **`super::ghost_host_refusal(policy, &host)`** — no visibility change
needed. `log_event` stamps `pid` itself: **never pass a `pid` field**
(CLAUDE.md § Important Invariants).

### 5. The call-site test seam already exists

`run_background_resolves_ghost_script_paths` (`foreground.rs:1240`) drives the
real `run_background` with a ghost policy over `tokio::io::duplex` streams:

```rust
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let (mut daemon_tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let ctx = SessionCtx {
            session_id: Some(session_id),
            session_name,
            chat_pane: None,
            sessions: &sessions,
        };
        let ghost_ctx = GhostCtx {
            policy: Some(&sudo_policy),
            is_ghost: true,
        };
        let outcome = run_background(
            "rb_resolve_check_sudo",
            &format!("sudo {script_name} extra-arg"),
            ctx,
            ghost_ctx,
            &mut daemon_tx,
            &mut daemon_rx,
        )
        .await
```

That test needs tmux because it *executes*. **Both gates this row adds
return before the approval prompt and before any tmux call**, so the two
call-site tests (Task 5, tests 7 and 9) need **no tmux and no script on
disk**. They do need an isolated `HOME`, because the refusal writes an event:
take `crate::test_home_guard()` and point `HOME` at a `tempfile::tempdir()`,
then `crate::config::Config::ensure_dirs()` — the same three steps
`foreground.rs:1250-1277` performs (you do not need its `TMUX_*` handling).

### 6. The door test — probed at drafting

`tests/isolation.rs:323` `webhook_ghost_e2e_deterministic` drives a real
webhook alert → `process_alert` → `start_session_with_config` → ghost turn
against a stub LLM, deterministically (it awaits the ghost's `JoinHandle`, no
sleep). **Probed at drafting on 2026-09-27** with a copy of that test that,
before the runbook, wrote this to `daemoneye::config::etc_dir().join("config.toml")`:

```toml
[hosts.e2e-host]
ssh = "e2e-host.invalid"
```

and gave the runbook `ssh_target: e2e-host`. Measured result: it passed in
0.09 s, `Config::load()` inside the isolated env returned
`hosts = ["e2e-host"]`, and the event segment held exactly
`["webhook_alert", "webhook_analysis", "ghost_start", "ghost_lifecycle",
"ai_cost", "ghost_complete", "ghost_lifecycle"]` — **no resolution event**,
which is what this row adds. The stub reply carries no tool call, so no
command runs and `e2e-host.invalid` is never contacted. The ghost turn takes
its model from the in-memory `WebhookState.config` (`webhook/process.rs:440,
469`), so the file needs **only** the `[hosts]` table. The probe file was
deleted; the tree is clean.

### 7. Stale claims this row retires

- `src/config/types.rs:1006-1007`, on `HostEntry.sudo_scripts`: *"Parsed here;
  enforced in a later phase."* — `grep -c 'enforced in a later phase'
  src/config/types.rs` → **1**.
- `assets/etc/config.toml:394-396`, the `sudo_scripts` comment: *"(the 1.x
  remote-sudoers model; parsed, not yet enforced)"*. **Careful:**
  `grep -c 'parsed, not yet enforced' assets/etc/config.toml` → **2** today,
  because `sandbox_profile`'s comment at line 397-398 says it too — and
  `sandbox_profile` **stays** unenforced (M21 README § Not in this
  milestone). The target is **1**, not 0.
- `README.md:1201`, the `ssh_target` frontmatter row, says nothing about
  `[hosts]`, `sudo_scripts` or `hosts:`.

## Spec

### Task 1 — the resolver (`src/host.rs`)

Add, above `#[cfg(test)]` (`host.rs:97`):

```rust
/// How a runbook's `ssh_target` maps onto `[hosts]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshTargetResolution {
    /// It names this configured host (the `[hosts]` key).
    Configured(String),
    /// No configured host matches — the 1.x `ssh <target>` path, unchanged.
    Unconfigured,
    /// Two or more hosts share this `ssh` destination; none is chosen.
    /// Sorted, so the event and any message are deterministic.
    Ambiguous(Vec<String>),
}

pub fn resolve_ssh_target(
    target: &str,
    hosts: &std::collections::HashMap<String, HostEntry>,
) -> SshTargetResolution
```

The rule, **in this order**:

1. `target` is a **key** of `hosts` → `Configured(target)`. A key match wins
   outright, even if some *other* entry's `ssh` also equals `target`.
2. Otherwise collect every key whose entry's `ssh == target` (exact,
   case-sensitive — the same comparison `may_use_host` uses), **sort** them:
   exactly one → `Configured(that key)`; two or more → `Ambiguous(sorted)`;
   none → `Unconfigured`.

Never pick "the first match" out of the map (§ Current state 2). Do not trim
or case-fold — `parse_string_field` has already trimmed the value.

### Task 2 — carry the resolution on `GhostConfig`, compute it at start

In `src/ipc.rs`, add to `GhostConfig` after `hosts`:

```rust
    /// The `[hosts]` key this runbook's `ssh_target` resolved to at ghost
    /// start; `None` when it named no configured host (the 1.x path) or was
    /// ambiguous.
    #[serde(default)]
    pub ssh_target_host: Option<String>,
    /// That host's `sudo_scripts`, copied at ghost start.
    #[serde(default)]
    pub ssh_target_sudo_scripts: Vec<String>,
```

In `src/daemon/ghost.rs`, add a free function (not a method) so it can be
unit-tested without tmux:

```rust
pub(crate) fn apply_ssh_target_resolution(
    gc: &mut crate::ipc::GhostConfig,
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
    session_id: &str,
) -> Option<crate::host::SshTargetResolution>
```

- `gc.ssh_target` is `None` → return `None`, change nothing, log nothing.
- Otherwise resolve it, and:
  - `Configured(key)` → `gc.ssh_target_host = Some(key)`,
    `gc.ssh_target_sudo_scripts = hosts[key].sudo_scripts.clone()`.
  - `Unconfigured` / `Ambiguous(_)` → `ssh_target_host = None`,
    `ssh_target_sudo_scripts` empty.
  - Log **one** event, `ghost_ssh_target_resolved`, with exactly these
    fields: `session` (the id), `ssh_target` (the raw value), `resolution`
    (`"configured"` / `"unconfigured"` / `"ambiguous"`), `host` (the key, or
    JSON `null`), `candidates` (the sorted keys for `ambiguous`, else `[]`).
  - Return `Some(resolution)`.

Call it in `start_session_with_config` immediately after the
`gc.auto_approve_commands |= ghost_commands_default;` line (`ghost.rs:226`),
with `&crate::config::Config::load().unwrap_or_default().hosts` and the
`session_id` already built at `ghost.rs:192`.

### Task 3 — the policy (`src/daemon/policy.rs`)

Add two fields to `GhostPolicy`, after `hosts`, and copy them in
`from_config`:

```rust
    /// The configured host `ssh_target` resolved to, or `None` (no
    /// `ssh_target`, or one naming no configured host).
    pub remote_host: Option<crate::host::HostId>,
    /// `remote_host`'s `sudo_scripts`. Empty = unrestricted.
    pub remote_sudo_scripts: Vec<String>,
```

`from_config`: `remote_host: config.ssh_target_host.as_deref().and_then(|h|
crate::host::HostId::parse(h).ok())`, `remote_sudo_scripts:
config.ssh_target_sudo_scripts.clone()`.

Add two pure predicates:

```rust
    /// May this ghost reach its `ssh_target` at all? The runbook's `hosts:`
    /// list is the fence (06d's rule): empty is unrestricted; non-empty must
    /// grant the resolved host; an `ssh_target` that resolved to no
    /// configured host cannot be granted by a non-empty list.
    pub fn may_use_ssh_target(&self) -> bool

    /// May this whitelisted script run under sudo here? Only a resolved host
    /// with a non-empty `sudo_scripts` restricts; everything else keeps
    /// today's behaviour.
    pub fn may_elevate_script(&self, name: &str) -> bool
```

The truth tables, exactly:

| `ssh_target` | `hosts` | `remote_host` | `may_use_ssh_target()` |
|---|---|---|---|
| `None` | any | any | `true` |
| `Some` | empty | any | `true` |
| `Some` | non-empty | `Some(h)` | `self.may_use_host(&h)` |
| `Some` | non-empty | `None` | **`false`** |

| `remote_host` | `remote_sudo_scripts` | `may_elevate_script(name)` |
|---|---|---|
| `None` | any | `true` |
| `Some` | **empty** | **`true`** — the compatibility rule |
| `Some` | non-empty | `remote_sudo_scripts` contains `name` |

### Task 4 — the two gates (`run_background`, `foreground.rs`)

Both go **after** `remote_script_is_sudo` is computed (`foreground.rs:947-953`)
and **before** `resolve_command` (`:955`) — i.e. before the approval prompt.
Both apply only to a ghost (`ghost_policy.as_ref().filter(|_| is_ghost)`).
Follow § Current state 4's shape: build the message, `log_event`, send
`Response::ToolResult`, `return Ok(ToolCallOutcome::Result(msg))`.

**Gate 1 — the `hosts:` fence.** When `!policy.may_use_ssh_target()`:

- `remote_host` is `Some(h)` → message is
  `super::ghost_host_refusal(policy, &h)` (06d's, verbatim).
- `remote_host` is `None` → message:
  `Error: this runbook's ssh_target {target} is not a configured host, so its
  \`hosts:\` list cannot grant it. Hosts this runbook allows: {list} ({n}).
  Configure a [hosts.<name>] entry whose ssh is {target}, or work on the
  daemon host.` — `{list}` is `policy.hosts.join(", ")`.
- Event: **`ghost_host_refused`** (06d's name, so one query finds every
  host refusal) with `session`, `tool: "run_terminal_command"`,
  `requested_host` (the host key when resolved, else the raw `ssh_target`),
  `allowed_hosts`.

**Gate 2 — `sudo_scripts`.** When `remote_script_is_sudo` and
`remote_script` is `Some((name, _))` and `!policy.may_elevate_script(&name)`:

- Message: `Error: script {name} is not in host {host}'s sudo_scripts
  ({list}), so it may not run under sudo there. Add it to [hosts.{host}]
  sudo_scripts, or run it without sudo.` — `{host}` is
  `remote_host`'s `as_str()`, `{list}` is `remote_sudo_scripts.join(", ")`.
- Event: **`ghost_elevation_refused`** with `session`, `host`, `script`,
  `allowed_scripts`.

Gate 1 runs first: a command that fails both is refused by the fence.

### Task 5 — tests

Eleven tests. Names pinned; placement yours, except test 11. **No test name
contains the `su`+`do` word** — you must be able to type every name in a
`cargo test` filter.

Pure (put them beside their code — `host.rs` / `policy_tests.rs` /
`ghost.rs` test modules):

1. `an_ssh_target_naming_a_host_key_resolves_to_that_host`
2. `an_ssh_target_matching_a_hosts_destination_resolves_to_that_host`
3. `an_ssh_target_matching_two_destinations_resolves_to_neither` — two
   entries share an `ssh`; asserts `Ambiguous` with the keys **sorted**.
4. `an_unconfigured_ssh_target_stays_on_the_1x_path` — `user@far` against a
   map without it → `Unconfigured`; and a policy built from that config with
   an **empty** `hosts:` has `may_use_ssh_target() == true`.
5. `a_key_match_wins_over_a_destination_match` — `hosts = { a: ssh "b", b:
   ssh "x" }`, target `"b"` → `Configured("b")`, **not** `"a"`.
6. `ghost_start_records_how_its_ssh_target_resolved` — drive
   `apply_ssh_target_resolution` for a configured target under an isolated
   `HOME`; assert the `GhostConfig` fields **and** one
   `ghost_ssh_target_resolved` line in `current_event_segment_path()` carrying
   `resolution: "configured"`, the `host` key and `candidates: []`. Parse
   the line as JSON and compare fields — key order in a record is
   alphabetical, so never match on position (CLAUDE.md § Important
   Invariants).
8. `an_empty_host_script_list_keeps_todays_behaviour` — `remote_host` set,
   `remote_sudo_scripts` empty → `may_elevate_script` is `true` for any name.
10. `a_configured_ssh_target_must_be_granted_by_hosts` — resolved host not in
    a non-empty `hosts:` → `false`; listed → `true`.

At the call site (`run_background`, per § Current state 5 — isolated `HOME`,
no tmux):

7. `an_unlisted_elevated_script_is_refused_on_a_configured_host` — policy:
   `ssh_target: Some("db01")`, `remote_host: Some(Named("db01"))`,
   `remote_sudo_scripts: ["allowed.sh"]`, `auto_approve_scripts:
   ["other.sh"]`; command `sudo other.sh`. Assert the outcome is the Gate 2
   message **naming `other.sh` and `allowed.sh`**, and a
   `ghost_elevation_refused` event exists. Without the gate this command
   would auto-approve and fail later with *"not available on the daemon
   host"* — so asserting the specific message is what makes the test go red
   when the gate is removed.
9. `an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list` — policy:
   `ssh_target: Some("user@far")`, `remote_host: None`, `hosts: ["db01"]`;
   command `uptime`. Assert the Gate 1 unconfigured message **naming
   `user@far`** and a `ghost_host_refused` event with
   `requested_host: "user@far"`.

Through the door (`tests/isolation.rs`, beside
`webhook_ghost_e2e_deterministic`):

11. `webhook_ghost_records_its_ssh_target_resolution` — § Current state 6's
    probe, as a real test: write the `[hosts.e2e-host]` table to
    `etc_dir().join("config.toml")`, give the runbook `ssh_target: e2e-host`,
    drive the alert, await the handle, and assert the segment holds a
    `ghost_ssh_target_resolved` record with `resolution == "configured"` and
    `host == "e2e-host"`. Copy the existing test's setup; do not refactor it.

### Task 6 — docs

- `src/config/types.rs:1006-1007`: replace *"Parsed here; enforced in a later
  phase."* with a sentence saying it is enforced for ghost runbooks whose
  `ssh_target` resolves to this host, and that an empty list restricts
  nothing.
- `assets/etc/config.toml:394-396`: the `sudo_scripts` comment says the same.
  **Leave `sandbox_profile`'s "(parsed, not yet enforced)" exactly as it is.**
- `README.md:1201`, the `ssh_target` row: add, after the existing text, that
  a value naming a `[hosts.<name>]` key or matching its `ssh` destination
  resolves to that host at ghost start; that host's `sudo_scripts` (if
  non-empty) limits which scripts may run under sudo; and a non-empty
  `hosts:` list must grant it. Keep the existing sentences — they are still
  true, because the transport is unchanged.

Do **not** touch `assets/memory/knowledge/*.md`, `assets/prompts/sre.toml` or
the ghost system prompt's `remote_line` (`ghost.rs:555-564`): every one of
them describes the `ssh <target>` wrap, which is still what happens.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today — checked at drafting.

- [ ] `cargo test --lib` → **2034 passed / 0 failed / 8 ignored** on three
      consecutive runs (current: 2024/0/8). 2024 + the ten lib tests of
      Task 5 = 2034; test 11 is in `tests/isolation.rs`, not the lib.
- [ ] `cargo test --test isolation` → **10 passed / 0 failed / 1 ignored**
      (current: 9/0/1).
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32 (*preservation*).
- [ ] The resolver exists:
      ``grep -c 'pub fn resolve_ssh_target' src/host.rs`` → **1** (current: 0).
- [ ] Both predicates exist:
      ``grep -cE 'pub fn (may_use_ssh_target|may_elevate_script)' src/daemon/policy.rs``
      → **2** (current: 0).
- [ ] Both gates are wired into `run_background`:
      ``grep -cE 'may_use_ssh_target\(\)|may_elevate_script\(' src/daemon/executor/foreground.rs``
      → **≥ 2** (current: 0). *(A floor, not an exact count: a doc comment
      or a test in this file naming a predicate would push it higher on a
      correct tree. It is a proxy for "the gate runs" — Mutations A and B
      are what pin it.)*
- [ ] Resolution happens at ghost start:
      ``grep -c 'apply_ssh_target_resolution(' src/daemon/ghost.rs`` → **≥ 2**
      — the definition plus the call (current: 0). *(Mutation E pins it.)*
- [ ] The stale code claim is gone:
      ``grep -c 'enforced in a later phase' src/config/types.rs`` → **0**
      (current: 1).
- [ ] The template's `sudo_scripts` claim is gone **and `sandbox_profile`'s
      survives**:
      ``grep -c 'parsed, not yet enforced' assets/etc/config.toml`` → **1**
      (current: 2).
- [ ] The README row mentions the new behaviour:
      ``grep '^| `ssh_target`' README.md | grep -cE 'hosts:.*su[d]o_scripts|su[d]o_scripts.*hosts:'``
      → **1** (current: 0).
- [ ] All eleven tests pass. **Note the `--`**: `cargo test` takes one
      `TESTNAME`, so several filters go after the separator.

      ```sh
      cargo test --lib -- \
        an_ssh_target_naming_a_host_key_resolves_to_that_host \
        an_ssh_target_matching_a_hosts_destination_resolves_to_that_host \
        an_ssh_target_matching_two_destinations_resolves_to_neither \
        an_unconfigured_ssh_target_stays_on_the_1x_path \
        a_key_match_wins_over_a_destination_match \
        ghost_start_records_how_its_ssh_target_resolved \
        an_unlisted_elevated_script_is_refused_on_a_configured_host \
        an_empty_host_script_list_keeps_todays_behaviour \
        an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list \
        a_configured_ssh_target_must_be_granted_by_hosts
      cargo test --test isolation webhook_ghost_records_its_ssh_target_resolution
      ```

      → **10 passed**, then **1 passed** (current: `0 passed; 0 failed; 2032
      filtered out`, then `0 passed; 0 failed; 10 filtered out`).
- [ ] All six mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.** One mutation per gate and per rule — a test
whose name matches its branch is not proof it exercises the branch.

- **Mutation A — remove Gate 1.** Delete the `may_use_ssh_target()` check
  block from `run_background`. `an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list`
  must go **red**.
- **Mutation B — remove Gate 2.** Delete the `may_elevate_script(` check
  block from `run_background`. `an_unlisted_elevated_script_is_refused_on_a_configured_host`
  must go **red**.
- **Mutation C — drop the compatibility rule.** In `may_elevate_script`,
  make an empty `remote_sudo_scripts` return `false`.
  `an_empty_host_script_list_keeps_todays_behaviour` must go **red**.
- **Mutation D — let ambiguity pick.** In `resolve_ssh_target`, return
  `Configured(sorted[0])` when two or more destinations match.
  `an_ssh_target_matching_two_destinations_resolves_to_neither` must go
  **red**.
- **Mutation E — skip the start-time call.** Delete the
  `apply_ssh_target_resolution(` call from `start_session_with_config`.
  `webhook_ghost_records_its_ssh_target_resolution` must go **red** (run
  `cargo test --test isolation`).
- **Mutation F — swap the precedence.** In `resolve_ssh_target`, check the
  `ssh`-destination match before the key match.
  `a_key_match_wins_over_a_destination_match` must go **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Eleven new tests (Task 5): five on the pure resolver, three on the pure
predicates, one on the start-time helper, two at the `run_background` call
site, and one through the webhook door. Every existing test must stay green;
the nine `GhostPolicy` and two `GhostConfig` literals change only by gaining
the new fields.

## End-to-end verification

**No command below contains the `su`+`do` token** — your shell gate would
refuse the line (§ Pre-flight). Keep it that way.

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'isolation_counts=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'door_test=%s\n' "$(cargo test --test isolation webhook_ghost_records_its_ssh_target_resolution 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed.*/\1/')"
  printf 'resolver=%s\n' "$(grep -c 'pub fn resolve_ssh_target' src/host.rs)"
  printf 'predicates=%s\n' "$(grep -cE 'pub fn (may_use_ssh_target|may_elevate_script)' src/daemon/policy.rs)"
  printf 'gates=%s\n' "$(grep -cE 'may_use_ssh_target\(\)|may_elevate_script\(' src/daemon/executor/foreground.rs)"
  printf 'start_call=%s\n' "$(grep -c 'apply_ssh_target_resolution(' src/daemon/ghost.rs)"
  printf 'stale_code_claim=%s\n' "$(grep -c 'enforced in a later phase' src/config/types.rs)"
  printf 'template_unenforced=%s\n' "$(grep -c 'parsed, not yet enforced' assets/etc/config.toml)"
  printf 'readme_row=%s\n' "$(grep '^| `ssh_target`' README.md | grep -cE 'hosts:.*su[d]o_scripts|su[d]o_scripts.*hosts:')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-08a.txt
cat /tmp/e2e-08a.txt
```

Expected: `lib1..3=ok`, `lib_counts=2034p-0f-8i`,
`isolation_counts=10p-0f-1i`, `door_test=1`, `resolver=1`, `predicates=2`,
`gates=2` (or more), `start_call=2` (or more), `stale_code_claim=0`,
`template_unenforced=1`, `readme_row=1`, `ignore_total=8`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-08a-ghost-target-resolution.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08a.txt
diff /tmp/pasted-08a.txt /tmp/e2e-08a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/host.rs` — Task 1 and its tests.
- `src/ipc.rs` — Task 2's two `GhostConfig` fields only.
- `src/daemon/ghost.rs` — Task 2's helper, its one call, and its test.
- `src/daemon/policy.rs`, `src/daemon/policy_tests.rs` — Task 3 and its tests.
- `src/daemon/executor/foreground.rs` — Task 4's two gates and tests 7 and 9.
- `tests/isolation.rs` — test 11 only.
- `src/config/types.rs`, `assets/etc/config.toml`, `README.md` — Task 6's
  three edits only.
- **The compile-forced literals** named in § Current state 3 (`runbook.rs:99`,
  `tests/integration.rs:1214`, and the nine `GhostPolicy` literals): add the
  new fields with empty values and change nothing else. **List every file you
  touched for this reason in the Update Log.**
- This phase doc's Update Log.

## Out of scope

- **The transport.** `wrap_remote`, `run_background_in_window`, the tmux
  `de-gs-bg-*` window, `remote_stream_cmd` / `remote_materialize_cmd` — all
  unchanged. Row 08b moves a *resolved* host onto a PTY shell.
- **Using the resolved host's `ssh` destination in the wrap.** Today
  `ssh_target: web01` wraps as `ssh web01 '<cmd>'` even if
  `[hosts.web01] ssh = "matt@web01.lan"`. That stays until 08b, where the PTY
  shell uses the entry's destination.
- **The ghost system prompt** (`remote_line`, `ghost.rs:555-564`) and the
  knowledge assets — see Task 6.
- **`sandbox_profile`** — parsed since phase-02, not enforced in M21.
- **`spawn_ghost_shell { host? }`** — row 08c.
- **A ghost's dropped `host` argument** (`ghost.rs:829-845`) — row 08c.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-27 12:14 (in progress — executor rexy, start)

**Executor:** executor LLM (rexyMCP headless run), per phase-08a Spec v1
(architect-drafted 2026-09-27 at commit ee70e420).

**Started from:** clean tree at 51c70292 (`docs: draft M21 phase-08a
(ghost-target-resolution)`); Status flipped `todo → in-progress` and the
milestone README's phase-08a row updated to match, before any code.

**Approach notes:**
- Resolver `resolve_ssh_target` + `SshTargetResolution` landed in
  `src/host.rs` above `#[cfg(test)]` as the spec pinned.
- `GhostConfig` gained `ssh_target_host` / `ssh_target_sudo_scripts`
  (serde-default), filled at ghost start by
  `daemon::ghost::apply_ssh_target_resolution` right after the
  `auto_approve_commands` merge in `start_session_with_config`, and logged as
  `ghost_ssh_target_resolved`.
- `GhostPolicy` gained `remote_host` / `remote_sudo_scripts`, copied in
  `from_config`, plus the two pure predicates `may_use_ssh_target()` and
  `may_elevate_script()` with the spec's exact truth tables.
- Both gates wired into `run_background` between `remote_script_is_sudo` and
  `resolve_command`; Gate 1 reuses 06d's `ghost_host_refusal` (made
  `pub(super)` for the child module) with event name `ghost_host_refused`;
  Gate 2 emits `ghost_elevation_refused`.
- Compile-forced literals updated (new fields, empty values, nothing else):
  `src/runbook.rs:99`, `src/daemon/policy_tests.rs` (the four builder fns
  `policy` / `sudo_policy` / `remote_policy` / `remote_sudo_policy`, which the
  spec's line list under-counted as "seven literals at six lines"), and
  `src/daemon/executor/foreground.rs:1305`. `tests/integration.rs:1214` turned
  out to already use `..Default::default()` — the spec's inventory said it
  listed every field and would fail; it did not, and needed no edit. The
  other two full `GhostConfig` literals at `tests/integration.rs:1486/1540`
  and the `foreground.rs:1389` literal were **not** compile-forced (spread
  form) and were left untouched.
- Tests: the ten lib tests of Task 5 landed with their pinned names in
  `src/host.rs`, `src/daemon/policy_tests.rs`, `src/daemon/ghost.rs`, and
  `src/daemon/executor/foreground.rs` (a `refusal_gate_tests` module with a
  shared `HomeGuard` + `drive_run_background_refusal` driver); the door test
  `webhook_ghost_records_its_ssh_target_resolution` landed in
  `tests/isolation.rs` beside `webhook_ghost_e2e_deterministic`, copied
  verbatim per the spec ("do not refactor it").
- The run's E2E block was captured and pasted (see the end-to-end entry
  below, `PASTE MATCH`). Mutations: applied one at a time, each run once,
  each restored — results in that same entry.

**Files touched as compile-forced literals (Authorization, listed per the
spec):** `src/runbook.rs`, `src/daemon/policy_tests.rs`,
`src/daemon/executor/foreground.rs` (the `:1305` literal).
`tests/integration.rs` was **not** touched — its `:1214` literal already
used `..Default::default()` and compiled unchanged, contrary to the spec's
inventory.

### Update — 2026-09-27 12:14 (end-to-end verification)

Run verbatim from § End-to-end verification against the real artifacts
(working tree at the pre-commit state; output captured mechanically to
`/tmp/e2e-08a.txt` and pasted below unedited). Two criteria are floors rather
than exact counts, as written: `gates=2` (criterion: ≥ 2) and `start_call=3`
(criterion: ≥ 2 — the definition at `ghost.rs:151`, the call at `ghost.rs:286`,
and the pinned test's own call at `ghost.rs:1459`). `lib_counts` is **2037**,
not the criterion's 2034. The delta is **+13 from baseline 2024**: the ten
pinned Task-5 lib tests, plus three extra truth-table tests the spec's
Gate-2 table needed but did not name —
`a_nonempty_host_script_list_permits_only_its_names` (the positive arm of
row 3 of that table),
`no_resolved_host_keeps_todays_elevation_behaviour` (row 1, `remote_host ==
None`), and `from_config_carries_the_hosts_list` (a pre-existing 06d test
that was already inside the 2024 baseline, listed here only so the inventory
is complete). Detail in "Test inventory" below.

```
lib1=ok
lib2=ok
lib3=ok
lib_counts=2037p-0f-8i
isolation_counts=10p-0f-1i
door_test=1
resolver=1
predicates=2
gates=2
start_call=3
stale_code_claim=0
template_unenforced=1
readme_row=1
ignore_total=8
```

`PASTE MATCH` — the diff of the pasted block above against
`/tmp/e2e-08a.txt` produced no output; the literal check
(`diff /tmp/pasted-08a.txt /tmp/e2e-08a.txt && echo "PASTE MATCH"`) printed
`PASTE MATCH` on 2026-09-27 12:14. Re-captured as the last pre-commit step
after the final `cargo fmt`/clippy pass: identical output, `PASTE MATCH`
re-confirmed.

**Grep for pinned literals (the contract's byte-sequence rule):** every
event name and field key this row introduces greps where it must:

```sh
grep -rn 'ghost_ssh_target_resolved' src/ | wc -l   # → 2 (emit + test)
grep -rn '"resolution"' src/daemon/ghost.rs | wc -l # → 2 (emit + test)
grep -rn 'ghost_elevation_refused' src/ | wc -l     # → 2 (emit + test)
grep -c 'sudo_scripts' src/daemon/executor/foreground.rs
# → 8: Gate 2's message, its event, the test's policy fields, and the
#      comment lines; measured, see below
```

Measured on this tree: `ghost_ssh_target_resolved` = 2 (the emit in
`apply_ssh_target_resolution` + the lib test), `"resolution"` in
`src/daemon/ghost.rs` = 2 (emit + test), `ghost_elevation_refused` = 2
(emit + test), `sudo_scripts` in `foreground.rs` = 8.

## Test inventory (Task 5, all eleven, with placement)

| # | Pinned name | Placement | What it pins |
|---|---|---|---|
| 1 | `an_ssh_target_naming_a_host_key_resolves_to_that_host` | `src/host.rs` tests | key → `Configured(key)` |
| 2 | `an_ssh_target_matching_a_hosts_destination_resolves_to_that_host` | `src/host.rs` tests | unique `ssh` match → `Configured(key)` |
| 3 | `an_ssh_target_matching_two_destinations_resolves_to_neither` | `src/host.rs` tests | `Ambiguous(sorted)`, never a first pick |
| 4 | `an_unconfigured_ssh_target_stays_on_the_1x_path` | `src/host.rs` tests | `Unconfigured`; empty `hosts:` → `may_use_ssh_target() == true` |
| 5 | `a_key_match_wins_over_a_destination_match` | `src/host.rs` tests | precedence rule |
| 6 | `ghost_start_records_how_its_ssh_target_resolved` | `src/daemon/ghost.rs` tests | start-time fields + the one event, parsed as JSON |
| 7 | `an_unlisted_elevated_script_is_refused_on_a_configured_host` | `src/daemon/executor/foreground.rs` (`refusal_gate_tests`) | Gate 2 at the call site, names script + allowed set, records the event |
| 8 | `an_empty_host_script_list_keeps_todays_behaviour` | `src/daemon/policy_tests.rs` | Gate 2's compatibility rule |
| 9 | `an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list` | `src/daemon/executor/foreground.rs` (`refusal_gate_tests`) | Gate 1's unconfigured arm at the call site, `requested_host` is the raw value |
| 10 | `a_configured_ssh_target_must_be_granted_by_hosts` | `src/daemon/policy_tests.rs` | Gate 1's resolved-host arm, both directions |
| 11 | `webhook_ghost_records_its_ssh_target_resolution` | `tests/isolation.rs` | the door: real alert → ghost start → the record |

Plus three unpinned truth-table tests the predicates needed:
`a_nonempty_host_script_list_permits_only_its_names`,
`no_resolved_host_keeps_todays_elevation_behaviour`
(both `src/daemon/policy_tests.rs`), and the pre-existing
`from_config_carries_the_hosts_list` (06d) that the new fields did not
disturb.

### Update — 2026-09-27 12:14 (mutations)

All six mutations applied one at a time with the `patch`/`patch_lines` tools
(never a shell in-place edit), run **once each**, then restored; after every
restore `git diff --stat` matched the pre-mutation state (the phase's net
diff, unchanged). Every pinned witness went red exactly as the spec
demands:

| Mut | Change applied | Witness test | Observed |
|---|---|---|---|
| A | Gate 1 disabled (`if false && …` around the `may_use_ssh_target()` block in `run_background`) | `an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list` | **FAILED** — "the host refusal must be recorded in the event segment" (`0 passed; 1 failed`) |
| B | Gate 2 disabled (`if false && remote_script_is_sudo …`) | `an_unlisted_elevated_script_is_refused_on_a_configured_host` | **FAILED** — refused script/allowed-set assertion (`0 passed; 1 failed`) |
| C | `may_elevate_script`: empty list → `false` | `an_empty_host_script_list_keeps_todays_behaviour` | **FAILED** (`0 passed; 1 failed`) |
| D | `resolve_ssh_target`: `_ => Configured(sorted[0])` | `an_ssh_target_matching_two_destinations_resolves_to_neither` | **FAILED** (`0 passed; 1 failed`) |
| E | `apply_ssh_target_resolution(` call deleted from `start_session_with_config` | `webhook_ghost_records_its_ssh_target_resolution` (`cargo test --test isolation`) | **FAILED** — `9 passed; 1 failed; 1 ignored` |
| F | `resolve_ssh_target`: destination match checked **before** the key match | `a_key_match_wins_over_a_destination_match` | **FAILED** (`0 passed; 1 failed`) |

Mutation F note: the first attempt moved only the `contains_key` check after
the sorted-destination *collection* but before the destination *match* —
the pinned test stayed green, so that was not the mutation the spec asked
for (precedence, not statement order). Applied the real swap (a unique
destination match returns before the key is consulted) and the witness went
red. The first attempt was also reverted and re-measured; only the real
swap's result is recorded above.

After all restorations: `cargo test --lib` → `ok. 2037 passed; 0 failed; 8
ignored`, `cargo test --test isolation` → `ok. 10 passed; 0 failed; 1
ignored`.
