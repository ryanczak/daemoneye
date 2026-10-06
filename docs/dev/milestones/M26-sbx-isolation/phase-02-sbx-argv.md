# Phase 02: sbx-argv — the pure sbx command builders, parsers and preflight classifier

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** in-progress
**Depends on:** phase-01b
**Estimated diff:** ~380 lines (about 170 code, 210 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

Create `src/daemon/executor/sbx.rs`: the one module that knows the `sbx` CLI.
Every later row spawns `sbx` with argv built here and reads `sbx` output
through parsers here. This phase is **pure**: no function in it starts a
process, reads a file or touches the network, so every behaviour is
unit-tested. Nothing calls the module yet; rows 04–08 wire it in.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` — § "Measured on pinky"
  (every CLI fact below comes from that table) and § "Decisions on the open
  questions" (Q1 is the network mapping this phase encodes).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- `src/daemon/executor/container.rs` is the docker sandbox this milestone
  replaces (row 09 deletes it). **Do not edit it, and do not call it.** It is
  the shape to copy: pure argv builders returning `Vec<String>`, pure
  classifiers taking already-collected probe results, a `describe_*`
  function producing the operator-facing message, and an inline
  `#[cfg(test)] mod tests` at the bottom of the file.
- `src/daemon/executor/mod.rs` declares the module at line 2:
  `pub(crate) mod container;`.
- `crate::config::SandboxProfile` (`src/config/types.rs`) holds the network
  settings this phase maps:

  ```rust
  pub struct SandboxProfile {
      /// "none" (default, no route out) or "proxy" …
      pub network: String,
      pub proxy_allow: Vec<String>,
      pub proxy_deny: Vec<String>,
  }
  ```

### Facts about `sbx` v0.46.0 (measured on pinky; quote, don't re-derive)

| Fact | Value |
|---|---|
| Create a mountless shell sandbox | `sbx create --name <n> -q --memory 1g --cpus 2 shell` → exit 0. All flags go **before** the positional `shell` |
| `--cpus` | **integer only**: `--cpus 1.5` → exit 1 `invalid argument "1.5" for "--cpus" flag` |
| `--pull` | default `always`. With the OS keychain locked, `--pull always` fails (`encode registry auth: …`) and **`--pull missing` succeeds**. Always pass `--pull missing` |
| No egress | `--deny-network '**'` (one argv element `**`) → every host 403 |
| Per-sandbox deny | `--deny-network api.github.com` → that host 403, `github.com` still 200 |
| Per-sandbox allow | `sbx policy allow network --sandbox <n> example.org` → 403 becomes 200 for that sandbox only |
| Deny beats allow | `--deny-network '**'` + a `--sandbox` allow → still 403 |
| Names | ≤ 63 characters (`sandbox name cannot exceed 63 characters`); underscores rejected; upper case and dots accepted |
| Interactive shell | `sbx exec -it [-w <dir>] <n> bash` |
| One-off command | `sbx exec <n> <argv…>`; exit status passes through |
| Remove | `sbx rm -f <n>…`. **Fails while the keychain is locked**, while `sbx stop <n>…` succeeds |
| List | `sbx ls --json` → `{"sandboxes": [{"name": "…", "id": "…", "agent": "shell", "status": "running", "last_used_at": "…", "created_at": "…", "workspaces": ["…"]}]}`; `workspaces` is absent when mountless; empty list is `{"sandboxes": []}`, exit 0 |
| Version | `sbx version` → exit 0, stdout `sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693` |
| Daemon | `sbx daemon status` → exit 0, first stdout line `Status: running` |
| Errors seen on stderr | `error: Not authenticated to Docker` (sbx run under a different `HOME`); `… the OS keychain is locked; unlock your keyring and retry …` (keyring locked) |

## Spec

### Task 1 — Create the module

Create `src/daemon/executor/sbx.rs` with a `//!` module doc of 2–4 lines
saying what it is. Declare it in `src/daemon/executor/mod.rs` directly after
the existing `pub(crate) mod container;` line, as **`pub mod sbx;`** — `pub`,
not `pub(crate)`. The module must not `use` anything from `container.rs`.

**Why `pub mod` (measured on this tree, 2026-10-06):** nothing calls this
module until row 04. With `pub(crate) mod sbx;` an unused `pub fn` fails
clippy (`error: function … is never used`); with `pub mod sbx;` the same
file passes, because `daemon` and `executor` are `pub` modules of the library
(`src/lib.rs`: `pub mod daemon;`, `src/daemon/mod.rs`: `pub mod executor;`),
so the items are part of the crate's public API. **Make every non-test item in
this phase `pub`** (the private `LsOutput` wrapper is used by
`parse_ls_json`, so it is fine). Do not add `#[allow(dead_code)]`.

### Task 2 — Sandbox naming and ownership

```rust
pub const BINARY: &str = "sbx";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxKind { Ghost, Chat }

pub fn sandbox_name(kind: SandboxKind, id: &str) -> String
pub fn is_daemon_sandbox(name: &str) -> bool
```

- Prefixes: `Ghost` → `"de-ghost-"`, `Chat` → `"de-chat-"`.
- `id` is sanitized: ASCII letters are lower-cased, ASCII digits and `-` are
  kept, and **every other character becomes `-`** (so `_`, `.`, `/`,
  spaces and non-ASCII all become `-`). An empty `id` becomes `"x"`.
- The result is truncated to **63** characters. Truncation counts bytes, which
  is safe because the result is pure ASCII after sanitizing.
- `is_daemon_sandbox` is true **only** when the name starts with `de-ghost-`
  or `de-chat-`. It must be false for `de-probe-a`, `de-ghost` (no trailing
  `-`), `my-de-ghost-1`, `De-Ghost-1` and `claude-daemoneye`. The sweep in a
  later row deletes whatever this returns true for, so it must never match a
  sandbox the operator made.

### Task 3 — Network plan from a profile (Q1)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPlan {
    /// No egress at all: `--deny-network '**'`.
    None,
    /// The operator's global sbx policy, narrowed by `deny` and widened by `allow`.
    Proxy { allow: Vec<String>, deny: Vec<String> },
}

pub fn network_plan(profile: &crate::config::SandboxProfile) -> Result<NetworkPlan, String>
```

- `network == "none"` → `NetworkPlan::None`. `proxy_allow` / `proxy_deny` are
  ignored, because deny-all already wins over any allow (measured).
- `network == "proxy"` → `Proxy` with `allow`/`deny` copied from
  `proxy_allow`/`proxy_deny`, in order, with **empty or all-whitespace
  entries dropped** and the rest trimmed.
- Anything else → `Err` naming the bad value and the two accepted ones:
  `"unknown sandbox network mode `<v>` (expected \"none\" or \"proxy\")"`.
  Matching is exact: `"None"`, `" none"` and `""` are errors.

### Task 4 — argv builders

Each returns `Vec<String>` **without** the leading `sbx`. The caller does
`Command::new(BINARY).args(...)`.

```rust
pub fn create_args(name: &str, plan: &NetworkPlan, memory: &str, cpus: f64) -> Vec<String>
pub fn policy_allow_args(name: &str, host: &str) -> Vec<String>
pub fn shell_exec_args(name: &str, workdir: Option<&str>) -> Vec<String>
pub fn exec_args(name: &str, argv: &[&str]) -> Vec<String>
pub fn stop_args(names: &[String]) -> Vec<String>
pub fn rm_args(names: &[String]) -> Vec<String>
pub fn ls_args() -> Vec<String>
pub fn version_args() -> Vec<String>
pub fn daemon_status_args() -> Vec<String>
```

Pinned shapes:

- `create_args("de-ghost-r1", &NetworkPlan::None, "1g", 2.0)` →
  `["create", "--name", "de-ghost-r1", "-q", "--pull", "missing", "--memory", "1g", "--cpus", "2", "--deny-network", "**", "shell"]`.
- With `Proxy { allow: [..], deny: ["a.example", "b.example"] }`, one
  `"--deny-network", "<host>"` pair per deny entry, in order, in place of the
  `"**"` pair. **The `allow` list does not appear in `create_args`**: allows
  are separate `policy_allow_args` calls after create. With an empty `deny`
  there is no `--deny-network` at all. `"shell"` is always the last element.
- `cpus` is rendered as an integer: `cpus.ceil()`, clamped to at least `1`.
  So `2.0` → `"2"`, `1.5` → `"2"`, `0.0` and negative values → `"1"`, and a
  non-finite value (`NaN`, `inf`) → `"1"`. Never emit a decimal point.
- `policy_allow_args(n, h)` → `["policy", "allow", "network", "--sandbox", n, h]`.
- `shell_exec_args(n, None)` → `["exec", "-it", n, "bash"]`;
  `shell_exec_args(n, Some("/w"))` → `["exec", "-it", "-w", "/w", n, "bash"]`.
- `exec_args(n, &["kill", "-STOP", "--", "-42"])` →
  `["exec", n, "kill", "-STOP", "--", "-42"]`.
- `stop_args(&[a, b])` → `["stop", a, b]`; `rm_args(&[a, b])` →
  `["rm", "-f", a, b]`. **Both return an empty `Vec` for an empty slice**,
  because `sbx rm -f` with no names must never run. Callers skip an empty
  argv.
- `ls_args()` → `["ls", "--json"]`; `version_args()` → `["version"]`;
  `daemon_status_args()` → `["daemon", "status"]`.

### Task 5 — Parse `sbx ls --json` and find stale sandboxes

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct SandboxInfo {
    pub name: String,
    pub status: String,
}

pub fn parse_ls_json(text: &str) -> Option<Vec<SandboxInfo>>
pub fn stale_daemon_sandboxes(listed: &[SandboxInfo], live: &[String]) -> Vec<String>
```

- `parse_ls_json` deserializes the shape in the facts table with
  `serde_json`. It returns `None` for anything that is not that shape: not
  JSON, the human text `No sandboxes found.`, or a missing `sandboxes` key.
  Extra fields (`id`, `agent`, `workspaces`, …) are ignored. Serde ignores
  unknown fields by default; do not add `deny_unknown_fields`. Private
  wrapper struct for the top level:

  ```rust
  #[derive(serde::Deserialize)]
  struct LsOutput { sandboxes: Vec<SandboxInfo> }
  ```

  `serde_json::from_str::<LsOutput>(text).ok().map(|o| o.sandboxes)` is the
  whole body.
- `stale_daemon_sandboxes` returns the names in `listed`, in listed order,
  for which `is_daemon_sandbox(name)` is true **and** the name is not in
  `live`. A non-daemon sandbox is never returned, whatever its status.

### Task 6 — Preflight classification

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// `Some(kind)` when the spawn itself failed.
    pub spawn_error: Option<std::io::ErrorKind>,
    pub exit_ok: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SbxUnavailable {
    NotInstalled,
    SpawnFailed { detail: String },
    NotAuthenticated,
    KeychainLocked,
    DaemonNotRunning { detail: String },
    UnrecognisedVersion { output: String },
}

pub fn classify_preflight(version: &Probe, daemon: &Probe) -> Result<String, SbxUnavailable>
pub fn describe_unavailable(reason: &SbxUnavailable) -> String
```

`classify_preflight` checks `version` first, then `daemon`, and returns
`Ok(version_string)`, for example `Ok("v0.46.0")`. For each probe, in this
order:

1. `spawn_error == Some(NotFound)` → `NotInstalled`. Any other
   `Some(kind)` → `SpawnFailed { detail: format!("{kind:?}") }`.
2. If `stderr` contains `the OS keychain is locked` → `KeychainLocked`.
   Otherwise, if `stderr` contains `Not authenticated to Docker` →
   `NotAuthenticated`. These are checked **even when `exit_ok` is true**,
   because sbx prints the keychain message as a `WARN` on a command that
   still succeeds. Keychain wins when both appear.
3. `exit_ok == false` → for the version probe,
   `UnrecognisedVersion { output: <stdout+stderr trimmed> }`; for the daemon
   probe, `DaemonNotRunning { detail: <first non-empty line of stderr, else of stdout, else ""> }`.

Then the content checks:

- Version: stdout's first line must start with `sbx version: `; the version
  is the next whitespace-separated token, and it must start with `v`. For
  `sbx version: v0.46.0 991967dc…` that is `v0.46.0`. Anything else →
  `UnrecognisedVersion { output: stdout.trim() }`.
- Daemon: stdout's first line, trimmed, must equal `Status: running`
  exactly. Anything else → `DaemonNotRunning { detail: <that line> }`.

`describe_unavailable` returns a one-line, operator-facing message starting
`sandbox unavailable: ` and naming the fix, in the same voice as the
existing docker messages. For example, from `container.rs`:

```rust
SandboxUnavailable::Runtime(RuntimeUnavailable::NotInstalled { runtime }) => format!(
    "sandbox unavailable: runtime `{runtime}` is not installed — install it (e.g. `docker`) and try again"
),
```

Required content per variant:

- `NotInstalled`: names `sbx`.
- `KeychainLocked`: says the operator's OS keychain is locked and must be
  unlocked.
- `NotAuthenticated`: names `sbx login`.
- `DaemonNotRunning`: names `sbx daemon start` and includes `detail`.
- `SpawnFailed`: includes `detail`.
- `UnrecognisedVersion`: includes `output`.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib executor::sbx` reports **0 failed** and at least the
      21 tests named in § Test plan, each `ok`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`. The known pre-existing flake
      `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
      may fail; if it does, run `cargo test` **once** more and paste both
      results. **Never run the full suite more than twice.** Any other failure
      is a real finding.
- [ ] `grep -c 'container::' src/daemon/executor/sbx.rs` prints `0`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()`, `.expect(` or `panic!(` in the
      non-test part of `sbx.rs` (above `#[cfg(test)]`).
- [ ] `grep -c '^pub mod sbx;$' src/daemon/executor/mod.rs` prints `1`.
- [ ] `git diff --stat` lists only `src/daemon/executor/sbx.rs` (new) and
      `src/daemon/executor/mod.rs` (one line), plus this phase doc and the
      README row.
- [ ] The Update Log entry contains the § End-to-end verification output and
      a line `PASTE MATCH: yes`, meaning the pasted block equals the terminal
      output.

## Test plan

All in `#[cfg(test)] mod tests` at the bottom of `src/daemon/executor/sbx.rs`:

- `sandbox_name_prefixes_by_kind` — `Ghost`/`"r1"` → `de-ghost-r1`;
  `Chat`/`"s1"` → `de-chat-s1`.
- `sandbox_name_sanitizes_the_id` — `"Run_7.x/y z"` → `de-ghost-run-7-x-y-z`;
  `"é"` → `de-ghost--` (non-ASCII becomes `-`); `""` → `de-ghost-x`.
- `sandbox_name_caps_at_63_characters` — a 100-character id gives a name of
  length exactly 63 that still starts with `de-ghost-`.
- `sandbox_name_never_contains_an_underscore` — for ids `"a_b"`, `"__"` and
  `"x_y_z"`, the name contains no `_`.
- `is_daemon_sandbox_matches_only_daemon_prefixes` — true for `de-ghost-r1`
  and `de-chat-s1`; false for every negative in Task 2.
- `network_plan_none_ignores_lists` — `none` with non-empty lists → `None`.
- `network_plan_proxy_copies_and_trims` — `proxy` with
  `allow = [" a.example ", "", "  "]`, `deny = ["b.example"]` →
  `Proxy { allow: ["a.example"], deny: ["b.example"] }`.
- `network_plan_rejects_unknown_modes` — `"None"`, `" none"`, `""`, `"host"`
  are `Err`, and the message contains the bad value.
- `create_args_none_denies_everything` — the exact vector in Task 4.
- `create_args_proxy_lists_each_deny_and_no_allow` — two deny entries in
  order; no allow entry appears anywhere in the argv.
- `create_args_proxy_with_no_deny_has_no_deny_flag` — no `--deny-network`.
- `create_args_always_pulls_missing_and_ends_with_shell`.
- `create_args_renders_cpus_as_an_integer` — `2.0→"2"`, `1.5→"2"`,
  `0.0→"1"`, `-3.0→"1"`, `f64::NAN→"1"`, `f64::INFINITY→"1"`; no rendered
  value contains `.`.
- `exec_and_policy_args_match_the_measured_shapes` —
  `policy_allow_args`, both `shell_exec_args` forms, `exec_args`.
- `stop_and_rm_args_are_empty_for_no_names` — and correct for two names.
- `parse_ls_json_reads_the_measured_shape` — the facts-table JSON with two
  sandboxes, one with `workspaces` and one without, gives both names and
  statuses.
- `parse_ls_json_reads_an_empty_list` — `{"sandboxes": []}` → `Some(vec![])`.
- `parse_ls_json_rejects_non_json_and_wrong_shapes` — `No sandboxes found.`,
  `{}`, `[]` → `None`.
- `stale_daemon_sandboxes_skips_live_and_foreign` — listed `de-ghost-a`,
  `de-ghost-b`, `de-probe-a`, `claude-x`, with live `["de-ghost-b"]` →
  `["de-ghost-a"]`.
- `classify_preflight_accepts_the_measured_outputs` — the two measured
  stdouts → `Ok("v0.46.0")`.
- `classify_preflight_maps_each_failure` — one assertion per
  `SbxUnavailable` variant, including: keychain `WARN` on an `exit_ok`
  version probe → `KeychainLocked`; `Not authenticated to Docker` →
  `NotAuthenticated`; both messages → `KeychainLocked`; spawn `NotFound` →
  `NotInstalled`; spawn `PermissionDenied` → `SpawnFailed`; daemon stdout
  `Status: stopped` → `DaemonNotRunning` with that detail; version stdout
  `sbx 0.46` → `UnrecognisedVersion`.

`describe_unavailable` needs no exact-string test. One test,
`describe_unavailable_names_the_fix`, asserts each message starts with
`sandbox unavailable: ` and contains the required content from Task 6.
That makes 22 tests; the criterion asks for at least the 21 above it.

## End-to-end verification

```sh
cargo test --lib executor::sbx 2>&1 | grep -E '^test |^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
grep -c 'container::' src/daemon/executor/sbx.rs
git diff --stat
```

Each line runs **once**. Expected: every `executor::sbx` test `ok` and the
result line `0 failed`; clippy ends in `Finished`; `fmt_exit=0`; `failed=0`;
`0`; the diffstat in the criteria.

## Authorizations

None. `serde` and `serde_json` are already dependencies.

## Out of scope

- Spawning `sbx` (no `Command`, no `bounded_output_with` in this phase).
- Config changes (`[sandbox] runtime`, limits mapping): row 03.
- Shells, signals, ghosts, chat, status: rows 04–08.
- Mapping stored service secrets to deny hosts (Q2): needs the per-service
  host list, which is not measured yet.
- Any edit to `container.rs`, `background/`, `ghost.rs` or the config types.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-10-06 17:37 (started)

**Executor:** claude (M26 phase-02 run 1)

Flipped the phase status to `in-progress` (and the README row). Implementing
tasks 1–6: `src/daemon/executor/sbx.rs` with naming, network plan, argv
builders, the `ls --json` parser, and preflight classification, plus the 22
unit tests in the test plan. End-to-end capture (task 7) follows.
