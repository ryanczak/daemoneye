# Phase 06a: sbx-lifecycle — preflight, create, teardown and sweep sandboxes

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** in-progress
**Depends on:** phase-05c
**Estimated diff:** ~420 lines (about 150 code, 270 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the functions that **run** `sbx` to manage a sandbox's life:

- **preflight**: is sbx usable;
- **create**: one sandbox with the profile's network plan and the configured
  limits;
- **teardown**: remove it, falling back to `stop` when removal fails;
- **sweep**: tear down every daemon sandbox no live record owns.

Each one runs `sbx` through the injectable `SbxRunner` from row 05b, so
every behaviour is unit-tested with a fake runner. Nothing calls these yet;
row 06b wires ghosts to them.

The phase also fixes the orphan-sidecar gap found at the 04b review.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Measured on pinky":
  the error strings, `--pull missing`, the locked keychain (`rm` fails while
  `stop` works), and the `ls --json` shape.
- § "Decisions on the open questions", Q1: the network plan.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Builders and parsers you call (`src/daemon/executor/sbx.rs`, all `pub`, tested)

```rust
pub enum SandboxKind { Ghost, Chat }
pub fn sandbox_name(kind: SandboxKind, id: &str) -> String
pub fn network_plan(profile: &crate::config::SandboxProfile) -> Result<NetworkPlan, String>
pub enum NetworkPlan { None, Proxy { allow: Vec<String>, deny: Vec<String> } }
pub fn create_args(name: &str, plan: &NetworkPlan, memory: &str, cpus: f64) -> Vec<String>
pub fn policy_allow_args(name: &str, host: &str) -> Vec<String>
pub fn stop_args(names: &[String]) -> Vec<String>      // empty for no names
pub fn rm_args(names: &[String]) -> Vec<String>        // empty for no names
pub fn ls_args() -> Vec<String>                        // ["ls", "--json"]
pub fn version_args() -> Vec<String>
pub fn daemon_status_args() -> Vec<String>
pub fn parse_ls_json(text: &str) -> Option<Vec<SandboxInfo>>
pub fn stale_daemon_sandboxes(listed: &[SandboxInfo], live: &[String]) -> Vec<String>
pub struct Probe { pub spawn_error: Option<std::io::ErrorKind>, pub exit_ok: bool, pub stdout: String, pub stderr: String }
pub enum SbxUnavailable { NotInstalled, SpawnFailed { detail: String }, NotAuthenticated, KeychainLocked, DaemonNotRunning { detail: String }, UnrecognisedVersion { output: String } }
pub fn classify_preflight(version: &Probe, daemon: &Probe) -> Result<String, SbxUnavailable>
```

### The runner (`src/shell/sbx_backend.rs`, exported from `crate::shell`)

```rust
pub struct VmRun { pub code: Option<i32>, pub stdout: String, pub stderr: String }
pub type SbxRunner = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, VmRun> + Send + Sync>;
pub fn real_runner(program: String) -> SbxRunner   // code: None on spawn error or 10 s timeout
```

### The fake runner to copy (`src/shell/sbx_backend.rs` tests)

```rust
type CannedRuns = Arc<Mutex<Vec<VmRun>>>;

fn fake_runner(canned: CannedRuns, calls: Arc<Mutex<Vec<Vec<String>>>>) -> SbxRunner {
    Arc::new(move |argv: Vec<String>| {
        calls.lock().unwrap().push(argv.clone());
        let mut queue = canned.lock().unwrap();
        let run = if queue.is_empty() {
            queue.last().cloned().unwrap_or_else(|| VmRun {
                code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        } else {
            queue.remove(0)
        };
        async move { run }.boxed()
    })
}
```

It is private to that test module. **Copy it** into this phase's test module;
do not make it `pub`.

### Measured `sbx` facts this phase relies on

| Call | Result |
|---|---|
| `rm -f <missing>` / `exec <missing>` | exit 1, stderr `error: sandbox '<n>' not found` |
| `rm -f <n>` with the OS keychain locked | exit 1, stderr `… the OS keychain is locked …`; **`stop <n>` succeeds** |
| `ls --json` with none | `{"sandboxes": []}`, exit 0 |

### The orphan-sidecar gap (04b review)

`spawn_sbx_host` (`src/shell/session.rs`) today writes the sidecar **before**
`which_sbx()` and `spawn_detached`:

```rust
let dir = crate::config::shells_dir();
std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
write_sandbox_sidecar(&dir, id.as_str(), sandbox).with_context(|| { … })?;
let argv = crate::daemon::executor::sbx::tagged_shell_exec_args(sandbox, id.as_str());
let program = which_sbx()?;
launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await
```

If either later step fails, an `<id>.sandbox` file is left with no `.sock`,
and `sweep_stale` (which scans by `.sock`) never removes it. Shell ids are
reused, so a later **plain** shell with that id would be adopted as a sandbox
shell after a restart. The plain spawners are `spawn_detached_host`
(line ~141), `spawn_ssh_host` (~160) and `spawn_ghost_ssh_host` (~242), all in
the same file.

## Spec

### Task 1 — The lifecycle module

Create `src/daemon/executor/sbx_lifecycle.rs` and declare it in
`src/daemon/executor/mod.rs` as `pub mod sbx_lifecycle;`. It must be `pub`,
so unused `pub` items do not warn; this was measured in phase 02. Write a 2–3
line `//!` doc. Everything below is `pub`.

```rust
use crate::daemon::executor::sbx::{self, NetworkPlan, SandboxKind, SbxUnavailable};
use crate::shell::{SbxRunner, VmRun};

/// The profile for `name`, or the default (network "none") when `name` is
/// `None` or is not configured. A job that cannot be matched to a profile
/// must not acquire egress.
pub fn profile_for(cfg: &crate::config::SandboxConfig, name: Option<&str>) -> crate::config::SandboxProfile

pub async fn preflight(run: &SbxRunner) -> Result<String, SbxUnavailable>

pub async fn create_sandbox(
    run: &SbxRunner,
    cfg: &crate::config::SandboxConfig,
    profile: Option<&str>,
    kind: SandboxKind,
    id: &str,
) -> anyhow::Result<String>   // the sandbox name

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Teardown { Removed, StoppedOnly { rm_error: String }, Failed { error: String } }

pub async fn teardown_sandbox(run: &SbxRunner, name: &str) -> Teardown

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub removed: Vec<String>,
    pub stopped: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub list_error: Option<String>,
}

pub async fn sweep_daemon_sandboxes(run: &SbxRunner, live: &[String]) -> SweepReport
```

A small private helper converts a run into a `Probe`; use it in `preflight`:
`Probe { spawn_error: None, exit_ok: run.code == Some(0), stdout, stderr }`.

**`profile_for`.** `name.and_then(|n| cfg.profile.get(n)).cloned().unwrap_or_default()`.
`SandboxProfile` implements `Default`, with network `"none"`.

**`preflight`.**
1. Run `sbx::version_args()`, then `sbx::daemon_status_args()`.
2. If either run has `code: None` (spawn failure or timeout), return
   `Err(SbxUnavailable::SpawnFailed { detail: <that run's stderr, trimmed> })`.
3. Otherwise return
   `sbx::classify_preflight(&probe(&version), &probe(&daemon))`.

**`create_sandbox`.**
1. `name = sbx::sandbox_name(kind, id)`.
2. `plan = sbx::network_plan(&profile_for(cfg, profile))`. Map an `Err(msg)`
   to an `anyhow` error **before** running anything.
3. Run `sbx::create_args(&name, &plan, &cfg.limits.memory, cfg.limits.cpus)`.
   A non-zero or `None` code → `Err` naming the sandbox and carrying the
   trimmed stderr.
4. For `NetworkPlan::Proxy { allow, .. }`, run `sbx::policy_allow_args(&name, host)`
   for each `host` in order. If one fails, call
   `teardown_sandbox(run, &name).await` (ignore the result) and return `Err`
   naming that host and carrying the stderr. **A half-configured sandbox is
   never handed back.**
5. `Ok(name)`.

**`teardown_sandbox`.**
1. Run `sbx::rm_args(&[name.to_string()])`.
   - `Some(0)` → `Removed`.
   - A non-zero code whose stderr contains `not found` → `Removed`: it is
     already gone.
2. Otherwise run `sbx::stop_args(&[name.to_string()])`:
   - `Some(0)` → `StoppedOnly { rm_error: <rm stderr, trimmed> }`. That is the
     locked-keychain case: the VM is stopped, and removal is left to the next
     sweep.
   - anything else → `Failed { error: <"rm: … ; stop: …" with both trimmed stderrs> }`.

**`sweep_daemon_sandboxes`.**
1. Run `sbx::ls_args()`. If the code is not `Some(0)`, or
   `sbx::parse_ls_json(&stdout)` is `None`, return a report with only
   `list_error` set (the trimmed stderr, or `"unrecognised sbx ls output"`).
2. For each name in `sbx::stale_daemon_sandboxes(&listed, live)`, in order,
   call `teardown_sandbox`. Push the name to `removed` / `stopped`, or
   `(name, error)` to `failed`.
3. Return the report.

The sweep therefore never touches a non-daemon sandbox: that guarantee is
`stale_daemon_sandboxes`, and it is tested.

### Task 2 — Close the orphan-sidecar gap (`src/shell/session.rs`)

1. In `spawn_sbx_host`, reorder so that `which_sbx()?` runs **first**,
   before the directory and the sidecar. Then, if
   `launch::spawn_detached(...)` returns `Err`, remove the sidecar
   (`let _ = std::fs::remove_file(sandbox_sidecar_path(&dir, id.as_str()));`)
   before returning the error.
2. Add, near `which_sbx`:

   ```rust
   /// Remove any `<id>.sandbox` left by an earlier sandbox shell with this id,
   /// so a plain shell that reuses the id is never adopted as a sandbox shell.
   pub fn clear_sandbox_sidecar(dir: &std::path::Path, id: &str)
   ```

   It calls `std::fs::remove_file(sandbox_sidecar_path(dir, id))` and ignores
   the result.
3. At the top of `spawn_detached_host`, `spawn_ssh_host` and
   `spawn_ghost_ssh_host`, call
   `clear_sandbox_sidecar(&crate::config::shells_dir(), id.as_str());`.

### Task 3 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib executor::sbx_lifecycle` reports **at least 14
      passed; 0 failed**, covering the tests in § Test plan.
- [ ] `cargo test --lib clear_sandbox_sidecar_removes_only_that_ids_sidecar`
      reports `1 passed; 0 failed`.
- [ ] `cargo test --lib executor::sbx` and `cargo test --lib shell::` report
      `0 failed`, with no existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/daemon/executor/sbx_lifecycle.rs` tests, with the copied fake runner,
on a `tokio::runtime::Runtime` or with `#[tokio::test]`:

- `profile_for_defaults_to_no_egress`: `None`, and an unknown name, both give
  `network == "none"`. A configured `proxy` profile is returned as
  configured.
- `preflight_accepts_the_measured_outputs`: canned
  `sbx version: v0.46.0 <sha>` then `Status: running` → `Ok("v0.46.0")`. The
  calls are `version_args()` then `daemon_status_args()`.
- `preflight_reports_a_spawn_failure`: first run
  `VmRun { code: None, stderr: "spawning sbx: No such file", .. }` →
  `Err(SpawnFailed { detail })` with that detail.
- `preflight_reports_a_locked_keychain`: version run exit 0 with stderr
  containing `the OS keychain is locked` → `Err(KeychainLocked)`.
- `create_sandbox_runs_create_with_limits_and_no_egress`: the default config
  and `profile = None` → `Ok("de-ghost-r1")` for `(Ghost, "r1")`. The single
  call equals `create_args("de-ghost-r1", &NetworkPlan::None, "1g", 2.0)`.
- `create_sandbox_applies_each_allow_after_create`: a `proxy` profile with
  `allow = ["a.example", "b.example"]` → calls are create, then
  `policy_allow_args(name, "a.example")`, then `… "b.example"`.
- `create_sandbox_tears_down_when_an_allow_fails`: second call exits 1 → `Err`
  naming `a.example`. The last call is `rm_args(&[name])`.
- `create_sandbox_rejects_an_invalid_profile_without_running_sbx`: a profile
  with `network = "bogus"` → `Err`, and **zero** calls.
- `create_sandbox_reports_a_failed_create`: create exits 1 with stderr
  `boom` → `Err` containing `boom`. Exactly one call.
- `teardown_removes_or_treats_not_found_as_removed`: `rm` exit 0 → `Removed`;
  `rm` exit 1 `error: sandbox 'x' not found` → `Removed`, with no `stop` call.
- `teardown_falls_back_to_stop_when_rm_fails`: `rm` exit 1 with
  `the OS keychain is locked`, then `stop` exit 0 →
  `StoppedOnly { rm_error }` containing `keychain`.
- `teardown_reports_when_rm_and_stop_both_fail`: both exit 1 → `Failed`, with
  both stderrs in `error`.
- `sweep_removes_only_stale_daemon_sandboxes`: `ls` returns
  `{"sandboxes":[{"name":"de-ghost-a","status":"running"},{"name":"de-ghost-b","status":"running"},{"name":"de-probe-a","status":"running"},{"name":"claude-x","status":"stopped"}]}`
  with `live = ["de-ghost-b"]`. The report has `removed == ["de-ghost-a"]`, and
  **no** call names `de-probe-a`, `claude-x` or `de-ghost-b`.
- `sweep_reports_an_unreadable_listing`: `ls` exit 1 → `list_error` is
  `Some`, with nothing removed and no further calls.

In `src/shell/session.rs` tests:

- `clear_sandbox_sidecar_removes_only_that_ids_sidecar`: in a tempdir, write
  sidecars for `s1` and `s2` with `write_sandbox_sidecar`, call
  `clear_sandbox_sidecar(dir, "s1")`; `s1.sandbox` is gone and `s2.sandbox`
  remains. Calling it again for `s1` (now missing) does not panic.

**Reviewer mutations:** (a) make `teardown_sandbox` return `Failed` without
trying `stop`, so `teardown_falls_back_to_stop_when_rm_fails` fails; (b) make
the sweep tear down every listed name, so
`sweep_removes_only_stale_daemon_sandboxes` fails.

## End-to-end verification

```sh
cargo test --lib executor::sbx_lifecycle 2>&1 | grep -E '^test result'
cargo test --lib clear_sandbox_sidecar_removes_only_that_ids_sidecar 2>&1 | grep -E '^test result'
cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
cargo test --lib shell:: 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.
Note that `executor::sbx` also matches `executor::sbx_lifecycle`, so its
count includes the new tests.

## Authorizations

- [x] May add `pub mod sbx_lifecycle;` to `src/daemon/executor/mod.rs`.
- [x] May edit `spawn_sbx_host`, `spawn_detached_host`, `spawn_ssh_host` and
      `spawn_ghost_ssh_host` in `src/shell/session.rs` **only** as Task 2 says,
      and add `clear_sandbox_sidecar` with its test (export it from
      `src/shell/mod.rs` if you need to).

## Out of scope

- Calling any of these from ghosts, chat or daemon startup: rows 06b and 07.
- Caching the preflight result.
- Status and audit output: row 08.
- Removing the docker runtime: row 09.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-06 20:37 (started, executor dispatch 1)

**Executor:** executor (M26 phase-06a, dispatch 1)

Flipped the phase and README rows to `in-progress`. Beginning Task 1: the
`sbx_lifecycle` module, then the Task 2 sidecar-gap fix, then Task 3's
end-to-end capture.

### Update — 2026-10-06 20:52 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim, once, into
`/tmp/e2e-06a.txt`:

```
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 2304 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2320 filtered out; finished in 0.00s
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 2272 filtered out; finished in 0.00s
test result: ok. 347 passed; 0 failed; 3 ignored; 0 measured; 1971 filtered out; finished in 11.83s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.86s
fmt_exit=0
passed=2371 failed=0
 docs/dev/milestones/M26-sbx-isolation/README.md    |  2 +-
 .../M26-sbx-isolation/phase-06a-sbx-lifecycle.md   | 10 ++++-
 src/daemon/executor/mod.rs                         |  1 +
 src/shell/mod.rs                                   |  4 +-
 src/shell/session.rs                               | 43 +++++++++++++++++++++-
 5 files changed, 54 insertions(+), 6 deletions(-)
```

PASTE MATCH: yes

Line 1 is `executor::sbx_lifecycle` (17 ≥ the required 14). Line 2 is the
sidecar test. Line 3 is `executor::sbx` (49 = 32 sbx + 17 new lifecycle
tests, as the phase doc notes the prefix overlap). Line 4 is `shell::`
(347 passed, 3 pre-existing ignored live-model tests). clippy exited 0;
fmt check exited 0; the full `cargo test` run is 2371 passed, 0 failed.
`git diff --stat` was captured with only the phase doc + README status
flips and the code files present at that moment; the Update Log entry
itself was appended after, so it is not in that stat.
