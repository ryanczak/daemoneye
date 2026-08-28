# Phase 02: Container runtime probe and UID-mapping gate

**Milestone:** M18 — Container-sandboxed Agents
**Status:** todo
**Depends on:** phase-01 (`SandboxConfig` supplies `runtime`, `docker_host`, `run_as`)
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add `src/daemon/executor/container.rs`: the runtime health probe and the D1
UID-mapping gate that decides whether sandboxed execution may run at all. All
decision logic is **pure and fixture-tested**; the only impure part is a thin
command runner whose live tests are `#[ignore]`d. Nothing calls the gate yet —
phase-04 does.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D1 — Runtime: Docker,
  rootless, user-namespace-mapped execution" — the measured uid map, why
  container root is the failure case, and what the gate must assert.
- `docs/dev/milestones/M18-container-sandboxing/README.md` § Notes — the
  executor-host constraint: the four gates must stay green on a machine with
  **no docker binary**.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-28, commit `d258292`):

- `cargo test --lib` → **1395 passed; 0 failed; 0 ignored**. Four gates green.
- `src/daemon/executor/container.rs` does **not** exist. `grep -rn
  "executor::container\|mod container" src/` → **0 matches**.
- `cargo test --lib sandbox_runtime` → **0** test lines (the § Gotchas item 1
  vacuity trap; criteria below are counts, never exit status).
- `src/daemon/executor/mod.rs` declares its submodules at lines 1-4:
  ```rust
  mod file_ops;
  mod foreground;
  mod knowledge;
  mod schedule;
  ```
- Phase-01 landed `SandboxConfig` in `src/config/types.rs` with
  `runtime` (`"docker"`), `docker_host`
  (`"unix:///run/user/1000/docker.sock"`), `run_as` (`"1000:1000"`), and
  `runs_as_container_root()`. Reuse them; do not re-derive.

### The command-runner idiom (reuse, do not reimplement)

`src/tmux/mod.rs:125` already provides a bounded `Command::output` replacement
that drains stdout and stderr on their own threads (a plain `output()` can
deadlock on large output):

```rust
pub fn bounded_output(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    bounded_output_with(cmd, TMUX_TIMEOUT)
}
```

Use `crate::tmux::bounded_output_with(&mut cmd, Duration::from_secs(10))` for
every docker invocation. Do **not** call `Command::output()` directly and do
**not** write a second draining helper.

## Gotchas

Five traps. Every fixture below was captured from the live rootless Docker on
this host on 2026-08-28 — the executor cannot reproduce them, which is why
they are quoted verbatim.

1. **`cargo test <filter>` passes with zero tests.** Measured on this tree:
   `cargo test --lib sandbox_runtime` prints no test lines and still exits 0.
   Every criterion below is a **line count**. Never "fix" one by checking an
   exit code.

2. **`/proc/self/uid_map` is identical whether or not `--user` is passed.**
   This is the trap that makes a naive gate useless. Measured, both runs on
   the same host:

   ```
   $ docker run --rm --user 1000:1000 alpine:3.22 cat /proc/self/uid_map
            0       1000          1
            1     100000      65536
   $ docker run --rm alpine:3.22 cat /proc/self/uid_map
            0       1000          1
            1     100000      65536
   ```

   The map describes the **namespace**, not the process. So the gate needs
   **two** inputs: the map (to translate) and `id -u` from inside the
   container (the process's actual container uid). A gate that reads only the
   map cannot tell a root container from a non-root one.

3. **The columns are `container_start host_start length`, whitespace-padded
   with a leading run of spaces.** Parse by splitting on ASCII whitespace
   after trimming; do not slice by column offset. From the fixture above, the
   two ranges are container `[0,1) → host [1000,1001)` and container
   `[1, 65537) → host [100000, 165536)`.

4. **Container root maps to the daemon's own host uid — that is the whole
   point of the gate.** With the map above, container uid `0` → host uid
   `1000` (`matt`), and container uid `1000` → host `100000 + (1000 - 1)` =
   **`100999`**. Verified live: a `--user 1000:1000` container's process is
   host-visible as uid 100999, and a default container's process is
   host-visible as uid 1000. A gate that "passes" on a root container has
   inverted its own purpose.

5. **`docker version` exits 1 and prints nothing on stdout when the daemon is
   unreachable — the error goes to stderr.** Measured:

   ```
   $ DOCKER_HOST=unix:///nonexistent/docker.sock docker version --format '{{.Server.Version}}'
   exit=1
   stderr: failed to connect to the docker API at unix:///nonexistent/docker.sock; check if the path is correct and if the daemon is running: dial unix /nonexistent/docker.sock: connect: no such file or directory
   ```

   And when the binary is missing entirely, the spawn itself fails —
   `std::io::ErrorKind::NotFound`, not a non-zero exit. **These are two
   distinct outcomes and the enum must keep them apart**, because the operator
   fix differs (install docker vs. start the user service). Healthy, for
   contrast: `exit=0`, stdout `29.7.2`.

## Spec

### Task 1 — Create the module and its outcome types

Create `src/daemon/executor/container.rs` and add `mod container;` to the
submodule list at the top of `src/daemon/executor/mod.rs` (keep the list
alphabetical — it goes first, before `file_ops`).

Define two public enums. Derive `Debug, Clone, PartialEq` on both.

```rust
/// Why sandboxed execution is unavailable. Each variant maps to a different
/// operator fix, so they must stay distinct (§ Gotchas item 5).
pub enum RuntimeUnavailable {
    /// The runtime binary is not on PATH (spawn failed with NotFound).
    NotInstalled { runtime: String },
    /// The binary ran but could not reach its daemon.
    DaemonUnreachable { docker_host: String, stderr: String },
    /// `[sandbox] runtime` names something this build does not support.
    UnsupportedRuntime { runtime: String },
}

/// Result of the D1 UID-mapping gate.
pub enum UidGateOutcome {
    /// Sandboxed execution may proceed.
    Ok { container_uid: u32, host_uid: u32 },
    /// The container process is container root, which maps to the daemon's
    /// own host uid — the sandbox would not reduce the blast radius.
    ContainerRoot { host_uid: u32 },
    /// The container uid is not covered by any range in the uid map.
    Unmapped { container_uid: u32 },
    /// `/proc/self/uid_map` could not be parsed.
    MalformedMap,
}
```

### Task 2 — Parse the uid map (pure)

```rust
/// One `container_start host_start length` range from `/proc/self/uid_map`.
pub struct UidRange { pub container_start: u32, pub host_start: u32, pub length: u32 }

/// Parse `/proc/self/uid_map` content into its ranges.
/// Blank lines are skipped. A line that does not yield exactly three
/// whitespace-separated `u32` fields makes the whole parse fail (`None`) —
/// a partially-understood map must never be treated as authoritative.
pub fn parse_uid_map(text: &str) -> Option<Vec<UidRange>>
```

Split each line on ASCII whitespace after trimming (§ Gotchas item 3).

### Task 3 — Translate a container uid to a host uid (pure)

```rust
/// Host uid for `container_uid` under `ranges`, or `None` when no range
/// covers it. A range covers `[container_start, container_start + length)`
/// and maps to `host_start + (container_uid - container_start)`.
pub fn host_uid_for(container_uid: u32, ranges: &[UidRange]) -> Option<u32>
```

### Task 4 — The gate decision (pure)

```rust
/// Decide the D1 gate from the two inputs the probe collects: the container's
/// own uid (`id -u` inside it) and its `/proc/self/uid_map` content.
/// Both are needed — the map alone cannot distinguish a root container from a
/// non-root one (§ Gotchas item 2).
pub fn evaluate_uid_gate(container_uid: u32, uid_map: &str) -> UidGateOutcome
```

Rules, in order: an unparseable map → `MalformedMap`; a `container_uid` no
range covers → `Unmapped`; `container_uid == 0` → `ContainerRoot` carrying the
host uid it maps to; otherwise `Ok` with both uids.

### Task 5 — Classify a `docker version` probe result (pure)

```rust
/// Classify the outcome of running `<runtime> version --format '{{.Server.Version}}'`.
/// `spawn_kind` is `Some(ErrorKind)` when the spawn itself failed.
pub fn classify_version_probe(
    runtime: &str,
    docker_host: &str,
    spawn_kind: Option<std::io::ErrorKind>,
    exit_ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<String, RuntimeUnavailable>
```

`Ok(version)` carries the trimmed stdout. `spawn_kind == Some(NotFound)` →
`NotInstalled`. A non-zero exit → `DaemonUnreachable` carrying `docker_host`
and the trimmed stderr. **Keep those two apart** (§ Gotchas item 5). A
successful exit with empty trimmed stdout is also `DaemonUnreachable` — the
template renders empty when the server half is missing.

### Task 6 — The impure probe

```rust
/// Run the runtime's version probe. The only impure function in this module:
/// it shells out and hands the raw results to `classify_version_probe`.
pub fn probe_runtime(cfg: &crate::config::SandboxConfig) -> Result<String, RuntimeUnavailable>
```

Return `UnsupportedRuntime` immediately when `cfg.runtime != "docker"`.
Otherwise build `std::process::Command::new(&cfg.runtime)` with args
`["version", "--format", "{{.Server.Version}}"]`, set the `DOCKER_HOST`
environment variable to `cfg.docker_host`, and run it through
`crate::tmux::bounded_output_with(&mut cmd, Duration::from_secs(10))`. Map an
`Err(e)` from that call to `classify_version_probe(.., Some(e.kind()), ..)`
with empty stdout/stderr; map `Ok(out)` to the same function with
`spawn_kind = None`. **All decision-making stays in Task 5** — this function
must contain no branching on the *content* of stdout or stderr.

Keep this function small enough that the pure classifier holds every rule; a
reviewer must be able to see that moving the logic here would be the defect.

### Task 7 — Unit tests

Add the tests named in § Test plan in a `#[cfg(test)] mod tests` at the bottom
of `container.rs`. Every name must contain `sandbox_runtime` so the § Acceptance
criteria filter matches it. Use the **verbatim fixture** below for every map
test — it is the real map from this host:

```rust
    const UID_MAP: &str = "         0       1000          1\n         1     100000      65536\n";
```

### Task 8 — One `#[ignore]`d live test

Add exactly one live test, `sandbox_runtime_probe_reaches_a_real_docker`,
marked `#[ignore = "requires a running rootless Docker daemon"]`. It calls
`probe_runtime` with `SandboxConfig::default()` and asserts the result is
`Ok`. It must **not** run under the default `cargo test`; the milestone runs
the ignored set explicitly at close.

This is the only `#[ignore]` this phase may add, and it is authorized here
precisely so no other test needs one.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting and is `0`
or absent today unless stated.

- [ ] `grep -c "^mod container;" src/daemon/executor/mod.rs` prints `1`.
- [ ] `grep -c "pub fn evaluate_uid_gate" src/daemon/executor/container.rs`
      prints `1`.
- [ ] `grep -c "pub fn classify_version_probe" src/daemon/executor/container.rs`
      prints `1`.
- [ ] `cargo test --lib sandbox_runtime 2>&1 | grep -c "^test .* ok$"` prints
      `10` — one per non-ignored test in § Test plan. A count, not an exit
      status (§ Gotchas item 1).
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1405 passed; 0 failed; 1 ignored` (1395 today + 10 new + the one
      `#[ignore]`d live test).
- [ ] `grep -c "#\[ignore" src/daemon/executor/container.rs` prints `1` — the
      single authorized live test, and no other.
- [ ] `grep -c "Command::new" src/daemon/executor/container.rs` prints `1` —
      exactly one spawn site, in `probe_runtime`.
- [ ] `grep -c "\.output()" src/daemon/executor/container.rs` prints `0` — the
      bounded helper is used instead.
- [ ] All four gates green **on this host, which has docker installed but must
      not be consulted by the default test run**: `cargo fmt --all`,
      `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Ten non-ignored tests plus the one ignored live test, all in
`src/daemon/executor/container.rs`. Every name contains `sandbox_runtime`.

**Uid-map parsing and translation** (fixture `UID_MAP` from Task 7):

- `sandbox_runtime_parses_the_real_uid_map` — yields two ranges:
  `(0, 1000, 1)` and `(1, 100000, 65536)`.
- `sandbox_runtime_uid_map_rejects_malformed_lines` — **negative cases**, each
  must yield `None`: `"0 1000"` (two fields), `"0 1000 1 2"` (four fields),
  `"a b c"` (non-numeric). A blank line among valid lines must **not** fail
  the parse.
- `sandbox_runtime_translates_container_uids_to_host_uids` — against the real
  map: `0 → Some(1000)`, `1 → Some(100000)`, `1000 → Some(100999)`,
  `65536 → Some(165535)`. The `1000 → 100999` case is the measured one from
  D1; if your arithmetic gives `101000` you are off by the range's own start.
- `sandbox_runtime_translation_rejects_uids_outside_every_range` —
  `65537 → None` and `70000 → None`.

**Gate decision:**

- `sandbox_runtime_gate_passes_for_container_uid_1000` — `Ok { container_uid:
  1000, host_uid: 100999 }`.
- `sandbox_runtime_gate_rejects_container_root` — `container_uid = 0` yields
  `ContainerRoot { host_uid: 1000 }`. **This is the load-bearing case**: the
  host uid it reports is the daemon's own.
- `sandbox_runtime_gate_reports_unmapped_uid` — `container_uid = 70000` yields
  `Unmapped`.
- `sandbox_runtime_gate_reports_malformed_map` — a map of `"garbage"` yields
  `MalformedMap`, whatever the uid.

**Version-probe classification:**

- `sandbox_runtime_version_probe_classifies_healthy` — `spawn_kind = None`,
  `exit_ok = true`, stdout `"29.7.2\n"` → `Ok("29.7.2")`.
- `sandbox_runtime_version_probe_distinguishes_missing_binary_from_dead_daemon`
  — **both halves required in one test**, because keeping them apart is the
  point (§ Gotchas item 5):
  - `spawn_kind = Some(ErrorKind::NotFound)` → `NotInstalled`.
  - `exit_ok = false` with the measured stderr
    `"failed to connect to the docker API at unix:///nonexistent/docker.sock; check if the path is correct and if the daemon is running: dial unix /nonexistent/docker.sock: connect: no such file or directory"`
    → `DaemonUnreachable`, and the variant carries that `docker_host`.
  - `exit_ok = true` with stdout `"  \n"` → `DaemonUnreachable` (empty render).

**Ignored:**

- `sandbox_runtime_probe_reaches_a_real_docker` — `#[ignore]`, per Task 8.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-02-container-runtime-probe.md
{
echo "== A. sandbox_runtime tests (expect 10 lines) =="
cargo test --lib sandbox_runtime 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "mod container:        "; grep -c "^mod container;" src/daemon/executor/mod.rs
echo -n "evaluate_uid_gate:    "; grep -c "pub fn evaluate_uid_gate" src/daemon/executor/container.rs
echo -n "classify_version:     "; grep -c "pub fn classify_version_probe" src/daemon/executor/container.rs
echo -n "ignore count (want 1):"; grep -c "#\[ignore" src/daemon/executor/container.rs
echo -n "Command::new (want 1):"; grep -c "Command::new" src/daemon/executor/container.rs
echo -n "raw .output() (want 0):"; grep -c "\.output()" src/daemon/executor/container.rs
} > /tmp/e2e-02.txt 2>&1
cat /tmp/e2e-02.txt
```

Paste the contents of `/tmp/e2e-02.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-02-container-runtime-probe.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02.txt
diff /tmp/pasted-02.txt /tmp/e2e-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section A is the one that can lie** — on the current tree it prints zero test
lines and still reports `cargo_exit=0`. Ten lines is the pass condition.

## Authorizations

- Create `src/daemon/executor/container.rs`; edit
  `src/daemon/executor/mod.rs` (the `mod container;` line only).
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The one live test is `#[ignore]`d
  precisely so this phase never needs a runtime; the architect runs it at
  milestone close.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause (for example a
  pre-existing failing test in a file this phase does not own) — record a
  blocker Update Log entry naming the exact criterion or the exact failing
  test, and stop.** Do not repair unrelated code to turn a gate green, and do
  not edit this phase doc's criteria. Reporting the blocker *is* the
  successful outcome in that case.

## Out of scope

- **Calling the gate.** Nothing wires `probe_runtime` or `evaluate_uid_gate`
  into the executor, the daemon startup path, or any tool. Phase-04 does that.
- **The IPC surface and `daemoneye status` reporting** — moved to a later
  phase, because there is nothing to report until phase-04 can actually run a
  container. Do not add `Request`/`Response` variants.
- Running containers, building images, mounts, networks, `docker exec`
  (phases 03–05).
- Changing `SandboxConfig` or any phase-01 code. If a field seems missing,
  record it as a blocker rather than adding it.
- Editing `CLAUDE.md`, `README.md`, or `assets/etc/config.toml`.

## Update Log
