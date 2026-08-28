# Phase 03: Agent image and digest lockfile

**Milestone:** M18 — Container-sandboxed Agents
**Status:** done
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

### Added 2026-08-28 by the round-1 review (bug-phase-03-1)

Round 1 met every criterion above — verified independently. The dead-code
strategy worked (count held at 7, no new `#[allow]`), and no digest is
hardcoded anywhere in `src/`. These carry the outstanding defects; each was
run against the round-1 tree and produced the "before" value shown.

- [ ] `grep -c 'let missing_key = "image_id = {id}' src/daemon/executor/container.rs`
      prints `0` (**before: 1**). The fixture is a plain string literal
      containing the characters `{id}`, not a `format!`, so `parse_lock`
      rejects it at the image-id check and never reaches the missing-key path
      the case is named for.
- [ ] **Both missing-required-key paths are guarded.** Measured at review,
      each of these mutations leaves all 9 tests green today:
      `image: image?` → `image: image.unwrap_or_default()`, and
      `built_at: built_at?` → `built_at: built_at.unwrap_or(0)`. After the
      fix both must **FAIL**, with the results pasted into the Update Log and
      the file restored afterwards. (The unknown-key path is already guarded —
      neutering it fails 1 of 9 — so only these two are at issue.)
- [ ] `grep -c "^pub mod container;" src/daemon/executor/mod.rs` prints `0`
      (**before: 1**) and
      `grep -c "^pub(crate) mod container;" src/daemon/executor/mod.rs` prints
      `1` (**before: 0**). `pub mod` puts the whole module in the crate's
      public API; `pub(crate)` was measured sufficient for the CLI to reach
      the helpers (`cargo build` exit 0).
- [ ] `cargo test --lib sandbox_lock 2>&1 | grep -c "^test .* ok$"` still
      prints `9` — fix the fixtures, do not add tests.

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

### Update — ts=1787943764139 (complete, server-authored)

**Summary:** </think>Everything is committed and clean. Let me do a final checklist against the completion checklist:

- [x] Phase doc's Status says `in-progress` (flipped at start, left there)
- [x] Milestone README's phase-table row says `in-progress`
- [x] Did NOT hand-write a `(complete)` Update Log entry — only progress + end-to-end entries
- [x] `cargo fmt --all` was run immediately before `git add`
- [x] All verification commands ran clean (fmt, build, clippy, test)
- [x] Final message will be substantive Summary + Notes
- [x] `git status --short` shows nothing — clean
- [x] `git log -1 --stat` shows the commit includes every file touched

All acceptance criteria met. Time for the final summary.</think>
Summary: Phase-03 complete — the agent image lifecycle is implemented and verified. Built `containers/Dockerfile` (verbatim from spec, no chown, `USER 1000:1000`), added the `SandboxLock` record and pure helpers (`is_valid_image_id`, `render_lock`, `parse_lock`, `lock_path`, `read_lock`, `write_lock`, `check_image_matches`, `ImageCheck`) to `container.rs`, and `daemoneye sandbox build` (pure formatter + thin runner, one `Command::new` site, id validated before writing, `built_at` = epoch seconds, `rebuilt` true when the prior lock had a different id). Wired `Sandbox { cmd: SandboxCommands }` into `main.rs` following the `Schedule` shape and exported it from `cli/commands/mod.rs`.

**Deviations (one, forced):** `container.rs` had to become `pub mod container;` instead of staying private `mod container;` — `cli` is a crate sibling of `daemon`, not a descendant, so `run_sandbox_build` cannot name a private `daemon::executor` submodule. The existing `#[allow(dead_code)]` stays in place, the repo-wide count is unchanged at 7 (criterion met), and phase-04 can re-privatize the module when it wires the executor. This is the same class of reachability issue the phase doc's dead-code strategy anticipated, resolved without adding any `#[allow]`.

**Verification:** all 9 acceptance greps pass (Dockerfile present, `USER` count 1, `chown` 0, both `pub fn`s present, `run_sandbox_build` in main.rs 1, dead_code total 7, hardcoded digest 0); `cargo test --lib sandbox_lock` shows exactly 9 `ok` lines; lib suite reports `1414 passed; 0 failed; 1 ignored`; `sandbox --help` resolves against the real clap tree. End-to-end block ran verbatim, its output was pasted into the phase doc's Update Log, and the self-check printed `PASTE MATCH` (appended after a progress entry naming the `pub mod` adaptation). All four gates green: `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`. Committed as `2c6d201`; working tree clean.

For review: the `pub mod` visibility change on `daemon::executor::container` is the only spec deviation — everything else matches the spec exactly. No docker was run; `run_sandbox_build` remains unwritten-at-runtime as specified (architect builds at milestone close).

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nored; 0 measured; 0 filtered out; finished in 3.92s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `containers/Dockerfile` — +5 -0
- `docs/dev/milestones/M18-container-sandboxing/README.md` — +1 -1
- `docs/dev/milestones/M18-container-sandboxing/phase-03-image-lifecycle.md` — +59 -1
- `src/cli/commands/mod.rs` — +2 -0
- `src/cli/commands/sandbox.rs` — +104 -0
- `src/daemon/executor/container.rs` — +245 -0
- `src/daemon/executor/mod.rs` — +1 -1
- `src/main.rs` — +16 -0

**Commit:** 2c6d201ae685c30a0e252f9c29218f3f1ea79281

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-08-28 12:25 (progress)

Resuming after the round-1 bounce (bug-phase-03-1):

1. **Missing-key fixtures fixed.** The `missing_key` string literal — which
   contained the characters `{id}` instead of a real id, so `parse_lock`
   rejected it at the image-id check and never reached the missing-key path —
   is now `format!`-interpolated (`missing_image`), and a second
   `missing_built_at` fixture was added covering the missing `built_at` key.
   The unknown-key/malformed-id/duplicate/non-numeric fixtures are unchanged.
   `sandbox_lock_parse_rejects_bad_records` still exercises all six rejects;
   test count stays 9.
2. **`pub(crate) mod container;`** — narrowed from `pub mod container;`,
   keeping the phase-02 `#[allow(dead_code)]` and its comment intact. Build
   and all 9 filter tests pass.
3. **Mutation evidence** — each mutation below was applied, tested, and
   restored; both now FAIL as required:

```
$ image: image? → image: image.unwrap_or_default()
test result: FAILED. 8 passed; 1 failed (sandbox_lock_parse_rejects_bad_records
  — accepted the missing-image record: "image_id = sha256:dddd…\nbuilt_at = 1787900000")

$ built_at: built_at? → built_at: built_at.unwrap_or(0)
test result: FAILED. 8 passed; 1 failed (accepted the missing-built_at record:
  "image = daemoneye-agent-base\nimage_id = sha256:dddd…")
```

Restored `image?` / `built_at?`; `git status --short` now shows only
`container.rs` and `mod.rs` modified.

### Update — 2026-08-28 12:26 (end-to-end verification)

Ran the § End-to-end block verbatim on the fixed tree; full block below.

```text
== A. sandbox_lock tests (expect 9 lines) ==
test daemon::executor::container::tests::sandbox_lock_accepts_a_well_formed_image_id ... ok
test cli::commands::sandbox::tests::sandbox_lock_build_result_distinguishes_first_build_from_rebuild ... ok
test daemon::executor::container::tests::sandbox_lock_check_reports_match ... ok
test daemon::executor::container::tests::sandbox_lock_check_reports_malformed_live_before_mismatch ... ok
test daemon::executor::container::tests::sandbox_lock_check_reports_mismatch ... ok
test daemon::executor::container::tests::sandbox_lock_parse_tolerates_whitespace_and_blank_lines ... ok
test daemon::executor::container::tests::sandbox_lock_parse_rejects_bad_records ... ok
test daemon::executor::container::tests::sandbox_lock_rejects_malformed_image_ids ... ok
test daemon::executor::container::tests::sandbox_lock_render_parse_round_trip ... ok
cargo_exit=0
== B. lib suite totals ==
test result: ok. 1414 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 4.04s
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

### Update — ts=1787945214533 (complete, server-authored)

**Summary:** All tasks complete. Summary:

**What I built (fix for bug-phase-03-1 on top of the round-1 phase-03 work):**

1. **Fixed the missing-key rejection fixtures** in `src/daemon/executor/container.rs`. The round-1 bug: `sandbox_lock_parse_rejects_bad_records` used a plain string literal `"image_id = {id}\nbuilt_at = ..."` containing the literal characters `{id}`, so `parse_lock` rejected it at the image-id validation step and never reached the missing-`image` path the case was named for. It's now `format!`-interpolated with a real id, and I added a `missing_built_at` fixture so **both** required keys are guarded (the bug report asked for both, and confirmed both were unguarded pre-fix).
2. **Narrowed `pub mod container;` → `pub(crate) mod container;`** in `src/daemon/executor/mod.rs`, keeping the phase-02 `#[allow(dead_code)]` and its comment exactly as-is. `cargo build` passes, so the CLI still reaches the helpers — the bug report's measured finding held.

**Verification highlights:**
- Both new fixtures land in the existing `sandbox_lock_parse_rejects_bad_records` test — count remains **9** (no new tests).
- **Mutation evidence**, both halves, captured verbatim in the Update Log: `image: image.unwrap_or_default()` → `FAILED. 8 passed; 1 failed` (accepted the missing-image record) and `built_at: built_at.unwrap_or(0)` → `FAILED. 8 passed; 1 failed` (accepted the missing-built_at record); both restored to `?`, `git status` clean afterward.
- The § End-to-end block ran **verbatim**: A shows 9 `ok` lines, B shows `1414 passed; 0 failed; 1 ignored`, C shows `sandbox --help` resolving, D shows all structural greps at their required values (dead_code 7, hardcoded digest 0, `USER 1000:1000` 1, chown 0). The self-check `diff` printed **PASTE MATCH**.
- All four gates green after `cargo fmt --all`: `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (full suite `1414 passed; 0 failed; 1 ignored`).
- Criterion greps: missing_key literal → 0, `pub mod container;` → 0, `pub(crate) mod container;` → 1.

**Notes for review:** No deviations this round — the only delta vs. the phase spec is exactly what bug-phase-03-1 demanded. The phase status in the doc header still reads `in-progress` (its status was already flipped at round-1 start; the README table row reflects `in-progress` from the start flip, and I left it untouched per the executor contract — the server flips to `review` when it finalizes the run). Committed as `5f61ac2` (`fix:`); working tree clean. Docker was not run — as authorized, the build command stays unexecuted.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.87s


LINT
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.35s


TEST
nored; 0 measured; 0 filtered out; finished in 3.99s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_strips_trailing_prose ... ok
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M18-container-sandboxing/phase-03-image-lifecycle.md` — +70 -0
- `src/daemon/executor/container.rs` — +4 -2
- `src/daemon/executor/mod.rs` — +1 -1

**Commit:** 5f61ac2d2d60ab9031990d93d7f381b654e87276

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-28

- **Verdict:** approved_after_1
- **Bounces:** 1 (bug-phase-03-1, major — resolved)
- **Executor:** deepseek-v4-flash-0731
- **Scope deviations:** none in round 2. Round 1's `pub mod container;` is
  narrowed to `pub(crate)`; the phase-02 `#[allow(dead_code)]` and its comment
  are untouched.
- **Calibration:** one architect-side item, held at 1 occurrence. **A
  multi-case rejection test that asserts only *that* input is rejected, never
  *why*, cannot detect a fixture that is rejected for the wrong reason.**
  § Test plan asked for five rejection cases inside one test and did not ask
  the reasons to be told apart; a dropped `format!` then made two of them
  silently untested while the test stayed green. Where a test bundles several
  rejection cases, either assert the discriminating reason per case or require
  mutation evidence per path. Not folded — one occurrence.

**The phase-02 calibration worked, applied up front rather than after a
bounce.** § Current state's "Dead-code strategy" block and the criterion
pinning the repo-wide `allow(dead_code)` count at 7 held through both rounds:
no new `#[allow]`, no dead-code blocker, no improvisation. The digest guard
also held — a broader review sweep (`grep -rnE 'sha256:[0-9a-f]{64}' src/`)
finds **0**, so no build-specific id is hardcoded anywhere, not merely none of
the one the criterion names.

**Round 1** delivered the Dockerfile, the lock record and helpers, the CLI
command and its clap wiring, and met every acceptance criterion. It was
bounced for two defects, both now fixed.

**Round 2** verified at review, with the mutations re-run independently rather
than read from the pasted evidence:

| Mutation to `parse_lock` | round 1 | round 2 (reviewer re-run) |
|---|---|---|
| `image: image?` → `image.unwrap_or_default()` | 9 passed — unguarded | **FAILED. 8 passed; 1 failed** |
| `built_at: built_at?` → `built_at.unwrap_or(0)` | 9 passed — unguarded | **FAILED. 8 passed; 1 failed** |
| unknown-key path neutered (control) | 1 failed | **FAILED. 8 passed; 1 failed** |

So both previously-unguarded required-key paths now bite, and the control is
undisturbed. Test count stayed at **9** — the fixtures were fixed, not padded
with new tests.

Also confirmed: the `missing_key` literal is gone (grep `0`);
`pub(crate) mod container;` is in place (`pub mod` grep `0`, `pub(crate)`
grep `1`) and the CLI still reaches the helpers; `allow(dead_code)` still 7;
`USER 1000:1000` 1 and `chown` 0 in the Dockerfile; no `unwrap`/`expect` in
`src/cli/commands/sandbox.rs`; four gates green; 1414 passed, 1 ignored; and
this round's E2E artifact re-extracts identical apart from the elapsed-time
line.

**Deferred to milestone close:** `daemoneye sandbox build` has never been
executed — the phase forbade running docker. The architect builds the image
and verifies the lock round-trip at close, against the real runtime.
