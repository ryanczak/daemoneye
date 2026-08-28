# Phase 03: Agent image and digest lockfile

**Milestone:** M18 — Container-sandboxed Agents
**Status:** in-progress
**Depends on:** phase-01 (`SandboxConfig.image`), phase-02 (`container.rs` exists)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Check in `containers/Dockerfile` for the agent base image, add
`daemoneye sandbox build` to build it and record the resulting image ID in
`~/.daemoneye/etc/sandbox.lock`, and add the pure lock read/write/compare
helpers phase-04 will use to refuse execution on a digest mismatch.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "Image lifecycle (supply
  chain)" — why the lock exists and what refuse-on-mismatch means.
- `docs/design/agent-container-sandboxing.md` § "D4 — Mount policy" — in
  particular the **measured correction** about the `/de/work` tmpfs, which is
  why this phase's Dockerfile looks the way it does.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-28, commit `fd7d461`):

- `cargo test --lib` → **1405 passed; 0 failed; 1 ignored**. Four gates green.
- `containers/` does **not** exist. `grep -c "Sandbox" src/main.rs` → **0**.
- `cargo test --lib sandbox_lock` → **0** test lines (the vacuity trap; every
  criterion below is a count).
- `src/daemon/executor/container.rs` exists from phase-02 with
  `probe_runtime`, `evaluate_uid_gate`, `classify_version_probe` and friends.
  Its `mod container;` declaration in `src/daemon/executor/mod.rs:1-4` carries
  `#[allow(dead_code)]` with a comment naming phase-04.
- `crate::config::etc_dir()` (`src/config/load.rs:15`) returns
  `~/.daemoneye/etc`. The lock file goes at `etc_dir().join("sandbox.lock")`.

### Dead-code strategy for this phase (read this first)

Phase-02 had to be bounced because it landed a module nothing called under a
`-D warnings` gate. **This phase does not have that problem, and you must not
re-create it.** Everything you add is reachable: `run_sandbox_build` is wired
into `main.rs`, and it calls the lock helpers. So:

- **Do not add any new `#[allow(dead_code)]`.** A criterion pins the repo-wide
  count at its current value.
- If you find yourself wanting one, that means something you wrote is
  unreachable — wire it into `run_sandbox_build` instead, or record a blocker.
- The phase-02 `#[allow(dead_code)]` on `mod container;` stays exactly as it
  is. Do not remove it (phase-04 does that) and do not widen it.

### The CLI command idiom (copy this shape)

`src/cli/commands/reindex.rs:1-8` is the closest analogue — a pure formatter
plus a thin impure runner:

```rust
//! `daemoneye reindex` — rebuild the derived memory index from disk.

use crate::memory::index::{ReconcileReport, reconcile_index};
use std::io::{self, Write};
use std::process;

/// Render the operator-facing report. Pure, so the wording is unit-testable.
fn format_report(report: &ReconcileReport) -> String {
```

It is exported from `src/cli/commands/mod.rs:12,23`:

```rust
mod reindex;
pub use reindex::run_reindex;
```

and dispatched in `src/main.rs:502-504`:

```rust
        Commands::Reindex => {
            cli::run_reindex();
        }
```

Do the same shape for `sandbox`.

## Gotchas

Five traps. Items 1–3 were **measured on this host** against a real build of
the exact Dockerfile this phase specifies; the executor has no runtime and
cannot reproduce them.

1. **The `/de/work` tmpfs is not writable by the sandboxed uid unless the
   mount flag says so — and the obvious Dockerfile fix does not work.** When
   the mountpoint does not exist in the image, Docker creates the tmpfs mode
   `1777`; once `WORKDIR /de/work` exists in the image, the tmpfs **inherits
   the directory's mode** and the sandboxed uid is denied. Measured against
   the real image:

   | tmpfs flags | resulting `/de/work` | writable as uid 1000 |
   |---|---|---|
   | `rw,size=64m` | `drwxr-xr-x root root` | **no** |
   | image `chown 1000:1000` + `rw,size=64m` | `drwx------ root root` | **no** — mode inherits, ownership does not |
   | `rw,size=64m,mode=0700,uid=1000,gid=1000` | `drwx------ de de` | **yes** |

   **This phase does not mount anything** — it is recorded here so the
   Dockerfile is not "fixed" by adding a `chown` that cannot work. Phase-04
   carries the mount flags.

2. **`docker build -q` prints the image ID on stdout, and that is the value to
   lock.** Measured: `sha256:185a9ca875c6cc5f6a7214cad7799c08953893c35e61a16b55774f7110bf384a`,
   and `docker image inspect --format '{{.Id}}'` returns the identical string.
   Non-`-q` builds print buildkit progress to **stderr** and no bare ID.

3. **Do NOT hardcode that digest — or any digest — in code or tests.** The
   image ID is whatever a given build produces; it changes whenever a layer
   changes. The lock file records the build's own output. A test that asserts
   a specific `sha256:…` value will pass today and fail on the next rebuild.
   Tests must use obviously-synthetic values such as `sha256:aaa…`.

4. **The digest string must be validated before it is written.** `docker`
   failing can leave stdout empty, and writing an empty or malformed lock is
   worse than not writing one: phase-04 will compare against it. Require the
   `sha256:` prefix followed by 64 lowercase hex characters, and reject
   anything else.

5. **`cargo test sandbox_lock` passes today with zero tests.** Criteria are
   line counts, not exit statuses.

## Spec

### Task 1 — Check in the Dockerfile

Create `containers/Dockerfile` with **exactly** this content. It was built and
exercised on the target host while this phase was drafted: the image runs as
uid 1000 by default, `curl`/`jq`/`git`/`python3` are all present, and a
containerized process is host-visible as uid 100999 (the D1 expectation).

```dockerfile
FROM alpine:3.22
RUN apk add --no-cache curl jq git python3 coreutils
RUN adduser -D -u 1000 -g '' de
WORKDIR /de/work
USER 1000:1000
```

Do not add a `chown` of `/de/work` (§ Gotchas item 1 — it cannot work), and do
not add `ENTRYPOINT` or `CMD`; phase-04 supplies the command.

### Task 2 — The lock record and its pure helpers

Add to `src/daemon/executor/container.rs`:

```rust
/// The recorded identity of the agent image, persisted at
/// `~/.daemoneye/etc/sandbox.lock`. Phase-04 refuses to run a container whose
/// image id differs from this.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxLock { pub image: String, pub image_id: String, pub built_at: u64 }

/// True when `s` is a well-formed docker image id: literal "sha256:" followed
/// by exactly 64 lowercase hex characters.
pub fn is_valid_image_id(s: &str) -> bool

/// Serialize a lock to the on-disk form (see below).
pub fn render_lock(lock: &SandboxLock) -> String

/// Parse the on-disk form. `None` for a malformed record, an unknown key set,
/// or an `image_id` that fails `is_valid_image_id`.
pub fn parse_lock(text: &str) -> Option<SandboxLock>

/// Path to the lock file: `crate::config::etc_dir().join("sandbox.lock")`.
pub fn lock_path() -> std::path::PathBuf
```

On-disk form — three `key = value` lines, in this order, so the file is
diffable and greppable:

```
image = daemoneye-agent-base
image_id = sha256:0000000000000000000000000000000000000000000000000000000000000000
built_at = 1787900000
```

`parse_lock` must accept surrounding blank lines and whitespace around `=`,
and must reject: a missing key, a duplicate key, an unknown key, a
non-numeric `built_at`, and an `image_id` failing validation.

### Task 3 — Read, write, and compare

```rust
/// Read and parse the lock. `None` when the file is absent or malformed —
/// the caller distinguishes "no lock yet" from "bad lock" by its own logic.
pub fn read_lock() -> Option<SandboxLock>

/// Write `lock` to `lock_path()`, creating `etc/` if needed.
pub fn write_lock(lock: &SandboxLock) -> std::io::Result<()>

/// Compare a live image id against the lock. Phase-04's refusal gate.
pub fn check_image_matches(lock: &SandboxLock, live_image_id: &str) -> ImageCheck
```

with

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ImageCheck {
    Match,
    Mismatch { locked: String, live: String },
    /// The live id is not a well-formed image id at all.
    MalformedLive { live: String },
}
```

Check malformed **before** comparing, so a garbage live id is never reported
as a plain mismatch.

### Task 4 — `daemoneye sandbox build`

Create `src/cli/commands/sandbox.rs` following the `reindex.rs` shape quoted
in § Current state: a pure formatter plus a thin impure runner.

```rust
/// Render the operator-facing result. Pure, so the wording is unit-testable.
fn format_build_result(image: &str, image_id: &str, rebuilt: bool) -> String

/// `daemoneye sandbox build` — build the agent image and record its id.
pub fn run_sandbox_build()
```

`run_sandbox_build` must:

1. Load the config (`crate::config::Config`) and read `sandbox.image`.
2. Run `docker build -q -t <image> -f containers/Dockerfile containers`
   through `crate::tmux::bounded_output_with(&mut cmd, Duration::from_secs(600))`,
   with `DOCKER_HOST` set from `sandbox.docker_host`. **One `Command::new`
   site**, as in phase-02.
3. On spawn failure or non-zero exit, print a clear error naming the runtime
   and exit non-zero via `std::process::exit(1)`. Do not write a lock.
4. Trim stdout to get the image id. **Reject it with `is_valid_image_id`
   before writing** (§ Gotchas item 4); on rejection, error and exit non-zero.
5. Write the lock with `built_at` = seconds since the Unix epoch.
6. Print `format_build_result(...)`, where `rebuilt` is true when a lock
   already existed with a **different** `image_id`.

Export it from `src/cli/commands/mod.rs` (`mod sandbox;` +
`pub use sandbox::run_sandbox_build;`) alongside the existing entries.

### Task 5 — Wire the subcommand

In `src/main.rs`, add a `Sandbox` variant to the `Commands` enum with a
nested subcommand enum (follow the existing `Schedule { cmd: SchedCommands }`
shape already in the file):

```rust
    /// Manage the container sandbox image
    Sandbox {
        #[command(subcommand)]
        cmd: SandboxCommands,
    },
```

```rust
#[derive(Subcommand)]
enum SandboxCommands {
    /// Build the agent image from containers/Dockerfile and record its id
    Build,
}
```

and dispatch `SandboxCommands::Build => { cli::run_sandbox_build(); }`.

### Task 6 — Unit tests

Add the tests named in § Test plan. Put the lock tests in `container.rs`'s
existing `mod tests` and the formatter test in `sandbox.rs`. Every name must
contain `sandbox_lock` so the § Acceptance criteria filter matches it.

Tests that touch the filesystem must take `crate::test_home_guard()`
(`src/lib.rs:45`) — **not** the raw `TEST_HOME_LOCK` — because they mutate
`HOME`. Edition 2024, so `std::env::set_var` needs `unsafe`. The RAII idiom is
at `src/daemon/context/recall.rs:246`.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `test -f containers/Dockerfile && echo yes` prints `yes`.
- [ ] `grep -c "^USER 1000:1000" containers/Dockerfile` prints `1`.
- [ ] `grep -c "chown" containers/Dockerfile` prints `0` (§ Gotchas item 1).
- [ ] `grep -c "pub fn is_valid_image_id" src/daemon/executor/container.rs`
      prints `1`.
- [ ] `grep -c "pub fn check_image_matches" src/daemon/executor/container.rs`
      prints `1`.
- [ ] `grep -c "run_sandbox_build" src/main.rs` prints `1`.
- [ ] `cargo test --lib sandbox_lock 2>&1 | grep -c "^test .* ok$"` prints
      `9` — one per test in § Test plan. A count, not an exit status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1414 passed; 0 failed; 1 ignored` (1405 today + 9 new).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `7` — unchanged from today. This phase adds **no** new
      `#[allow]`; everything it writes is reachable from `main.rs`.
- [ ] `grep -c "sha256:185a9ca" src/ -r` prints `0` — no measured digest is
      hardcoded anywhere (§ Gotchas item 3).
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Nine tests. Names all contain `sandbox_lock`.

**Image-id validation** (`container.rs`) — pin the negative cases:

- `sandbox_lock_accepts_a_well_formed_image_id` — `sha256:` + 64 lowercase hex
  is accepted. Build the fixture as `format!("sha256:{}", "a".repeat(64))`;
  **do not** paste a real digest.
- `sandbox_lock_rejects_malformed_image_ids` — each must be rejected:
  no prefix (`"a".repeat(64)`), wrong prefix (`md5:` + 64 hex), 63 hex chars,
  65 hex chars, uppercase hex (`"A".repeat(64)` with the prefix), and the
  empty string.

**Round-trip and parsing** (`container.rs`):

- `sandbox_lock_render_parse_round_trip` — a `SandboxLock` survives
  `render_lock` → `parse_lock` unchanged.
- `sandbox_lock_parse_tolerates_whitespace_and_blank_lines` — leading/trailing
  blank lines and spaces around `=` still parse.
- `sandbox_lock_parse_rejects_bad_records` — each yields `None`: a missing
  key, a duplicated key, an unknown key, a non-numeric `built_at`, and a
  malformed `image_id`.

**Comparison** (`container.rs`):

- `sandbox_lock_check_reports_match` — identical ids yield `ImageCheck::Match`.
- `sandbox_lock_check_reports_mismatch` — different valid ids yield
  `Mismatch`, carrying both values.
- `sandbox_lock_check_reports_malformed_live_before_mismatch` — a garbage live
  id against a valid lock yields `MalformedLive`, **not** `Mismatch`. This is
  the ordering rule from Task 3.

**Formatter** (`sandbox.rs`):

- `sandbox_lock_build_result_distinguishes_first_build_from_rebuild` —
  `format_build_result` with `rebuilt = false` and `rebuilt = true` produce
  different strings, and both contain the image name and the id.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_lock tests (expect 9 lines) =="
cargo test --lib sandbox_lock 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. the CLI is really wired (no daemon, no docker needed) =="
cargo run --quiet -- sandbox --help 2>&1 | head -5; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. structural greps =="
echo -n "Dockerfile present:   "; test -f containers/Dockerfile && echo 1 || echo 0
echo -n "USER 1000:1000:       "; grep -c "^USER 1000:1000" containers/Dockerfile
echo -n "no chown in image:    "; grep -c "chown" containers/Dockerfile
echo -n "is_valid_image_id:    "; grep -c "pub fn is_valid_image_id" src/daemon/executor/container.rs
echo -n "check_image_matches:  "; grep -c "pub fn check_image_matches" src/daemon/executor/container.rs
echo -n "main.rs wiring:       "; grep -c "run_sandbox_build" src/main.rs
echo -n "allow(dead_code) tot: "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "hardcoded digest:     "; grep -rc "sha256:185a9ca" src/ | awk -F: '{s+=$2} END {print s}'
} > /tmp/e2e-03.txt 2>&1
cat /tmp/e2e-03.txt
```

Paste the contents of `/tmp/e2e-03.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-03-image-lifecycle.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03.txt
diff /tmp/pasted-03.txt /tmp/e2e-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section C is the one that proves the wiring.** `sandbox --help` exercises
the real clap tree in the real binary without touching docker or the daemon;
if the subcommand is not wired it fails there rather than in a unit test.

## Authorizations

- Create `containers/Dockerfile` and `src/cli/commands/sandbox.rs`; edit
  `src/daemon/executor/container.rs`, `src/cli/commands/mod.rs`, and
  `src/main.rs`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`,
  and `cargo run --quiet -- sandbox --help`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. `run_sandbox_build` is written but
  never executed by this phase; the architect builds the image at milestone
  close.
- **Do not add any `#[allow(...)]`.** See § Current state, "Dead-code strategy".
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause — record a blocker
  Update Log entry naming the exact criterion or failing test, and stop.**
  Do not repair unrelated code to turn a gate green, and do not edit this
  phase doc's criteria. Reporting the blocker *is* the successful outcome in
  that case.
- **Record what you decide, not what you wish had been decided.** Update Log
  entries are the audit trail: never attribute a choice to guidance you did
  not receive.

## Out of scope

- **Running the build.** No `docker build`, no image, no container.
- **Consuming the lock.** Nothing calls `check_image_matches` outside its
  tests; phase-04 wires the refusal gate.
- Mounts, networks, `docker exec`, staleness warnings, and the
  `requires_tools` runbook check — all later phases.
- Changing phase-01 config or phase-02 probe code, including the existing
  `#[allow(dead_code)]` on `mod container;`.
- Editing `CLAUDE.md`, `README.md`, or `assets/etc/config.toml`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-08-28 18:40 (progress)

Started phase-03: flipped status to in-progress, checked in
`containers/Dockerfile` verbatim, added the `SandboxLock` record and pure
helpers (`is_valid_image_id`, `render_lock`, `parse_lock`, `lock_path`,
`read_lock`, `write_lock`, `check_image_matches`, `ImageCheck`) to
`src/daemon/executor/container.rs`, plus the formatter test in
`src/cli/commands/sandbox.rs` and nine `sandbox_lock` tests total. One
adaptation (noted for review): the module had to become `pub mod container;`
(instead of private `mod container;` alongside its existing
`#[allow(dead_code)]`) because `cli` is a sibling of `daemon` in the crate
tree, so `run_sandbox_build` cannot reach a private `daemon::executor`
submodule — the `#[allow(dead_code)]` count stays at 7 and phase-04 can
re-privatize. Wired `Sandbox { cmd: SandboxCommands }` into `main.rs`; the
`--help` tree resolves. All nine filter tests pass; full lib suite is 1414
passed / 1 ignored. End-to-end capture pending along with the final gates.

### Update — 2026-08-28 18:45 (end-to-end verification)

Ran the § End-to-end block verbatim on the finished tree; full block below.

```text
== A. sandbox_lock tests (expect 9 lines) ==
test daemon::executor::container::tests::sandbox_lock_accepts_a_well_formed_image_id ... ok
test cli::commands::sandbox::tests::sandbox_lock_build_result_distinguishes_first_build_from_rebuild ... ok
test daemon::executor::container::tests::sandbox_lock_check_reports_malformed_live_before_mismatch ... ok
test daemon::executor::container::tests::sandbox_lock_check_reports_match ... ok
test daemon::executor::container::tests::sandbox_lock_check_reports_mismatch ... ok
test daemon::executor::container::tests::sandbox_lock_rejects_malformed_image_ids ... ok
test daemon::executor::container::tests::sandbox_lock_render_parse_round_trip ... ok
test daemon::executor::container::tests::sandbox_lock_parse_tolerates_whitespace_and_blank_lines ... ok
test daemon::executor::container::tests::sandbox_lock_parse_rejects_bad_records ... ok
cargo_exit=0
== B. lib suite totals ==
test result: ok. 1414 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 3.93s
cargo_exit=0
== C. the CLI is really wired (no daemon, no docker needed) ==
Manage the container sandbox image

Usage: daemoneye sandbox <COMMAND>

Commands:
cargo_exit=0
== D. structural greps ==
Dockerfile present:   1
USER 1000:1000:       1
no chown in image:    0
is_valid_image_id:    1
check_image_matches:  1
main.rs wiring:       1
allow(dead_code) tot: 7
hardcoded digest:     0
```

PASTE MATCH
