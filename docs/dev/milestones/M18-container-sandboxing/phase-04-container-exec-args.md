# Phase 04: Sandbox preflight and container argv construction

**Milestone:** M18 — Container-sandboxed Agents
**Status:** todo
**Depends on:** phase-01 (`SandboxConfig`), phase-02 (uid gate, probe), phase-03 (`SandboxLock`, `check_image_matches`)
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the two pure decisions that stand between an agent and a container: the
**preflight gate** (may we run at all?) and the **argv builders** (exactly
which flags does the container get?). Both are pure and fixture-tested. No
container is started — phase-05 wires these into the background execution
path.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D1 — Runtime" (why
  `--user` is never omitted), § "D4 — Mount policy" (the **measured**
  tmpfs correction), § "D5 — Network policy" (`none` by default).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-28, commit `2a9a26e`):

- `cargo test --lib` → **1414 passed; 0 failed; 1 ignored**. Four gates green.
- `src/daemon/executor/container.rs` is **568 lines** and already provides
  `probe_runtime`, `evaluate_uid_gate` → `UidGateOutcome`, `RuntimeUnavailable`,
  `SandboxLock`, `check_image_matches` → `ImageCheck`, `read_lock`.
  **Reuse them; do not re-derive or duplicate.**
- `grep -c "pub fn run_args" src/daemon/executor/container.rs` → **0**;
  `evaluate_preflight` → **0**; `cargo test --lib sandbox_exec` → **0** lines.
- `SandboxConfig` fields (`src/config/types.rs:502-524`): `enabled`, `runtime`,
  `image`, `workdir`, `run_as`, `docker_host`, `limits`, `profile`,
  `ghost_defaults`. `SandboxLimits`: `memory: String`, `pids: u32`,
  `cpus: f64`, `scratch: String`. `SandboxProfile`: `network: String`,
  `proxy_allow: Vec<String>`.

### Dead-code strategy for this phase

Everything this phase adds is called only by its own tests until phase-05
wires it in. **Keep the existing `#[allow(dead_code)]` on
`pub(crate) mod container;` exactly as it is** — it already covers the whole
module, so no new attribute is needed and none may be added. A criterion pins
the repo-wide count at **7**, unchanged. Phase-05 removes it.

## Gotchas

Six traps. Items 1–4 were measured on this host against the real image; the
executor has no runtime and cannot reproduce them.

1. **The whole argv was prototyped end to end before this spec was written.**
   This exact vector ran successfully against the checked-in Dockerfile's
   image: uid 1000 inside, staged script executed, scratch written, and
   `ls -ld /de/work` showed `drwx------ de de`:

   ```
   docker run --rm --user 1000:1000 --network none --memory 1g \
     --pids-limit 256 --cpus 2.0 \
     --tmpfs /de/work:rw,size=2g,mode=0700,uid=1000,gid=1000 \
     -v de-stage-proto:/de/scripts:ro --label de.ghost=1 \
     --workdir /de/work daemoneye-agent-base sh -lc '<command>'
   ```

   Build the same shape. The flag **order** below is what the tests pin.

2. **`--tmpfs` must carry `mode=0700,uid=…,gid=…` or the scratch dir is not
   writable.** Measured: with the mount options omitted the tmpfs inherits the
   image directory's `drwxr-xr-x root root` and the sandboxed uid gets
   `Permission denied`; an in-image `chown` does **not** help, because the
   tmpfs inherits the mode but not the ownership. This is the single easiest
   way to ship a sandbox that cannot run anything.

3. **The tmpfs `uid=`/`gid=` must come from `run_as`, not a literal `1000`.**
   Parse `run_as` once and use both halves. The negative case is pinned:
   `"10:0"` is uid 10, gid 0 — **not** root, and not 1000.

4. **`f64` renders without a trailing `.0`.** Measured with `rustc`:
   `format!("{}", 2.0f64)` produces `"2"`, and `1.5` produces `"1.5"`. Docker
   accepts `--cpus 2` (verified). So format `cpus` with plain `{}` — do **not**
   use `{:.1}`, and do not expect `"2.0"` in the pinned vector.

5. **`--user` is never omitted, even though the image sets `USER 1000:1000`.**
   The image default is not a control: images change, and a future profile may
   use a different image. The flag is the enforcement point, and a criterion
   pins it present in every produced vector.

6. **`cargo test --lib sandbox_exec` passes today with zero tests.** Every
   criterion is a line count, not an exit status.

## Spec

### Task 1 — Split `run_as`

In `src/daemon/executor/container.rs`:

```rust
/// Split a `"uid:gid"` string into its numeric halves.
/// `None` when either half is missing or non-numeric. Used for both the
/// `--user` flag and the tmpfs `uid=`/`gid=` options, so the two can never
/// disagree.
pub fn split_run_as(run_as: &str) -> Option<(u32, u32)>
```

Trim around the colon. Reject: no colon, an empty half, a non-numeric half,
and more than one colon.

### Task 2 — The preflight decision

```rust
/// Why sandboxed execution cannot proceed. One operator-facing reason,
/// collapsed from the three independent checks.
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxUnavailable {
    /// The runtime is missing, unreachable, or unsupported.
    Runtime(RuntimeUnavailable),
    /// The uid gate did not return `Ok` — carries the outcome that failed.
    UidGate(UidGateOutcome),
    /// No `sandbox.lock` exists; `daemoneye sandbox build` has not been run.
    NoLock,
    /// The live image does not match the lock — carries the failing check.
    Image(ImageCheck),
    /// `run_as` is not a parseable `uid:gid` pair.
    BadRunAs { run_as: String },
}

/// Decide whether sandboxed execution may proceed, from inputs the caller has
/// already collected. Pure: it starts no process and reads no file.
pub fn evaluate_preflight(
    run_as: &str,
    version: &Result<String, RuntimeUnavailable>,
    gate: &UidGateOutcome,
    lock: Option<&SandboxLock>,
    live_image_id: &str,
) -> Result<(), SandboxUnavailable>
```

Check order, and it matters — report the most fundamental failure first:
`BadRunAs` → `Runtime` → `UidGate` → `NoLock` → `Image`. Rationale: a bad
`run_as` makes every later check meaningless, and a missing runtime makes the
uid gate unanswerable. Only `UidGateOutcome::Ok` passes the gate; every other
variant becomes `UidGate(..)` carrying itself.

### Task 3 — The staging volume

```rust
/// Per-run staging volume name for `job_id`: `de-stage-<job_id>`.
pub fn stage_volume_name(job_id: &str) -> String

/// argv for the short-lived helper that stages one approved script into the
/// per-run volume. Runs as **container root** (`--user 0:0`) because it must
/// read the 0700 originals and chown the copy — it never runs agent-supplied
/// code, only this fixed shell line.
pub fn stage_args(cfg: &SandboxConfig, job_id: &str, script_name: &str) -> Vec<String>
```

Produce, in order:

```
run --rm --user 0:0 -v <volume>:/stage <image>
sh -c "cp /de/src/<script_name> /stage/<script_name> && chmod 0500 /stage/<script_name> && chown <uid>:<gid> /stage/<script_name>"
```

where `<uid>:<gid>` come from `split_run_as`. **The script name is
interpolated into a shell line**, so reject any `script_name` containing
`/`, `..`, whitespace, or a shell metacharacter (`;`, `&`, `|`, `$`, `` ` ``,
quotes, newline) by returning an empty `Vec`. Pin the negative cases.

### Task 4 — The run argv

```rust
/// One sandboxed job's identity and payload.
pub struct ExecSpec<'a> {
    pub job_id: &'a str,
    pub network: &'a str,
    pub is_ghost: bool,
    pub command: &'a str,
}

/// argv for the sandboxed run. Pure — the caller prepends the runtime binary
/// and spawns it.
pub fn run_args(cfg: &SandboxConfig, spec: &ExecSpec) -> Vec<String>
```

Produce exactly this order (matching the prototyped vector in § Gotchas 1):

1. `run`, `--rm`
2. `--user`, `<cfg.run_as>`
3. `--network`, `<spec.network>`
4. `--memory`, `<limits.memory>`
5. `--pids-limit`, `<limits.pids>`
6. `--cpus`, `<limits.cpus>` — plain `{}` formatting (§ Gotchas 4)
7. `--tmpfs`,
   `<cfg.workdir>:rw,size=<limits.scratch>,mode=0700,uid=<uid>,gid=<gid>`
8. `-v`, `<stage_volume_name(job_id)>:/de/scripts:ro`
9. `--label`, `de.ghost=1` — **only when `spec.is_ghost`**
10. `--workdir`, `<cfg.workdir>`
11. `<cfg.image>`
12. `sh`, `-lc`, `<spec.command>`

Return an empty `Vec` when `split_run_as(&cfg.run_as)` is `None` — a vector
without a valid `--user` must never be produced.

### Task 5 — Unit tests

Add the tests named in § Test plan to `container.rs`'s existing `mod tests`.
Every name must contain `sandbox_exec`.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `grep -c "pub fn run_args" src/daemon/executor/container.rs` prints `1`.
- [ ] `grep -c "pub fn evaluate_preflight" src/daemon/executor/container.rs`
      prints `1`.
- [ ] `grep -c "pub fn split_run_as" src/daemon/executor/container.rs` prints `1`.
- [ ] `cargo test --lib sandbox_exec 2>&1 | grep -c "^test .* ok$"` prints
      `11` — one per test in § Test plan. A count, not an exit status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1425 passed; 0 failed; 1 ignored` (1414 today + 11 new).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `7` — unchanged. No new `#[allow]`.
**The next three greps are scoped to the production half of the file with
`sed -n '1,/^#\[cfg(test)\]/p'`, and the scoping is required.** The pinned
test vector legitimately contains `mode=0700` and `uid=1000,gid=1000` as
expected *output*, so an unscoped grep counts those too and the criteria
could never hit their targets. Today the boundary is at
`container.rs:284`; all three scoped counts are `0`.

- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '"--user"'`
      prints `1` (**before: 0**) — the flag is emitted from exactly one place
      in production code, so it cannot be conditionally skipped.
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c "mode=0700"`
      prints `1` (**before: 0**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -cE 'uid=1000|gid=1000'`
      prints `0` (**before: 0**, and must stay `0`) — the tmpfs ids are
      derived from `run_as`, never literals (§ Gotchas 3). A hardcoded `1000`
      in production code fails this *and*
      `sandbox_exec_run_args_derive_tmpfs_ids_from_run_as`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Eleven tests, all in `container.rs`, every name containing `sandbox_exec`.

**`split_run_as`:**

- `sandbox_exec_splits_a_valid_run_as` — `"1000:1000"` → `Some((1000,1000))`,
  `"10:0"` → `Some((10,0))`, `"0:0"` → `Some((0,0))`.
- `sandbox_exec_rejects_malformed_run_as` — each `None`: `"1000"` (no colon),
  `":1000"`, `"1000:"`, `"a:b"`, `"1000:1000:1000"`, `""`.

**`run_args` — the load-bearing test:**

- `sandbox_exec_run_args_match_the_prototyped_vector` — with
  `SandboxConfig::default()` and `ExecSpec { job_id: "j1", network: "none",
  is_ghost: false, command: "echo hi" }`, assert the **whole vector**, equal
  to:

  ```rust
  vec!["run", "--rm",
       "--user", "1000:1000",
       "--network", "none",
       "--memory", "1g",
       "--pids-limit", "256",
       "--cpus", "2",
       "--tmpfs", "/de/work:rw,size=2g,mode=0700,uid=1000,gid=1000",
       "-v", "de-stage-j1:/de/scripts:ro",
       "--workdir", "/de/work",
       "daemoneye-agent-base",
       "sh", "-lc", "echo hi"]
  ```

  Note `"2"`, not `"2.0"` (§ Gotchas 4), and no `--label` when not a ghost.
- `sandbox_exec_run_args_label_ghost_jobs` — with `is_ghost: true` the vector
  contains `--label` immediately followed by `de.ghost=1`, and the non-ghost
  vector contains neither.
- `sandbox_exec_run_args_derive_tmpfs_ids_from_run_as` — with
  `run_as = "10:0"`, the `--tmpfs` value ends `mode=0700,uid=10,gid=0` and the
  `--user` value is `"10:0"`. **This is the § Gotchas 3 guard**: a hardcoded
  `1000` passes the default-config test and fails this one.
- `sandbox_exec_run_args_honour_limits_and_workdir` — non-default
  `SandboxLimits { memory: "4g", pids: 64, cpus: 1.5, scratch: "8g" }` and
  `workdir = "/scratch"` appear in the vector, with `--cpus` rendered `"1.5"`
  and the tmpfs path `/scratch:rw,size=8g,...`.
- `sandbox_exec_run_args_are_empty_for_bad_run_as` — `run_as = "nope"` yields
  an empty vector, so no `--user`-less command can escape.

**`stage_args`:**

- `sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid` — the
  vector contains `--user` `0:0`, mounts `de-stage-j1:/stage`, and its shell
  line contains `chmod 0500` and `chown 1000:1000`.
- `sandbox_exec_stage_args_reject_unsafe_script_names` — each yields an empty
  vector: `"../etc/passwd"`, `"a/b"`, `"a b"`, `"a;rm -rf /"`, `"a$(id)"`,
  `"a|b"`, `"a\nb"`. **Pin every one** — this string is interpolated into a
  shell line.

**`evaluate_preflight`:**

- `sandbox_exec_preflight_passes_when_everything_is_healthy` — valid
  `run_as`, `Ok(version)`, `UidGateOutcome::Ok`, a lock, and a matching live
  id → `Ok(())`.
- `sandbox_exec_preflight_reports_each_failure` — one case per variant, each
  asserting the **specific** returned variant, not merely that it is an
  error: bad `run_as` → `BadRunAs`; `Err(NotInstalled)` → `Runtime`;
  `UidGateOutcome::ContainerRoot { .. }` → `UidGate`; `lock = None` →
  `NoLock`; a non-matching live id → `Image`.
- `sandbox_exec_preflight_reports_the_most_fundamental_failure_first` — with
  **all five** inputs simultaneously bad, the result is `BadRunAs`; with
  `run_as` fixed but the rest still bad, it is `Runtime`; then `UidGate`; then
  `NoLock`. This pins the Task 2 ordering, which a per-variant test cannot
  see.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_exec tests (expect 11 lines) =="
cargo test --lib sandbox_exec 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "run_args:             "; grep -c "pub fn run_args" src/daemon/executor/container.rs
echo -n "evaluate_preflight:   "; grep -c "pub fn evaluate_preflight" src/daemon/executor/container.rs
echo -n "split_run_as:         "; grep -c "pub fn split_run_as" src/daemon/executor/container.rs
echo -n "--user emitted once:  "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '"--user"'
echo -n "mode=0700 once:       "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c "mode=0700"
echo -n "no literal uid=1000:  "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -cE 'uid=1000|gid=1000'
echo -n "allow(dead_code) tot: "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
} > /tmp/e2e-04.txt 2>&1
cat /tmp/e2e-04.txt
```

Paste the contents of `/tmp/e2e-04.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-04-container-exec-args.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04.txt
diff /tmp/pasted-04.txt /tmp/e2e-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section A is the one that can lie** — on the current tree it prints zero
test lines and still reports `cargo_exit=0`. Eleven lines is the pass
condition.

## Authorizations

- Edit `src/daemon/executor/container.rs` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. Every argv this phase builds is
  returned as data and never spawned.
- **Do not add any `#[allow(...)]`.** The module's existing one already
  covers this code; see § Current state.
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

- **Spawning anything.** No `Command`, no `docker run`, no volume creation.
  `run_args`/`stage_args` return vectors; phase-05 spawns them.
- Wiring into `run_terminal_command`, the `de-bg-*` window path, or ghost
  lifecycle — phase-05 and phase-06.
- Removing the module's `#[allow(dead_code)]` — phase-05 does that when it
  adds the first caller.
- The egress proxy (`network = "proxy"` is passed through as a string here;
  nothing implements it yet), the `log` relay opcode, `Request::ContainerStatus`,
  and the `daemoneye status` surface.
- Changing phase-01/02/03 code, including `containers/Dockerfile`.
- Editing `CLAUDE.md`, `README.md`, or `assets/etc/config.toml`.

## Update Log
