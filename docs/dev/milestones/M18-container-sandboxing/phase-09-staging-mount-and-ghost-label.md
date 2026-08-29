# Phase 09: Give the staging helper its source mount, and label ghost containers

**Milestone:** M18 — Container-sandboxed Agents
**Status:** todo
**Depends on:** phase-04 (`stage_args`), phase-05 (`ExecSpec` call site), phase-08 (`de.sandbox=1` label)
**Estimated diff:** ~260 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Two defects, both measured against the live runtime.

**`stage_args` cannot work as built** — it copies from `/de/src/<script>`, but
nothing mounts `/de/src`, so the helper fails with `cannot stat`. Verified
live. **And no ghost container is ever labelled `de.ghost=1`**, because the one
call site hardcodes `is_ghost: false` even though `run.rs` already knows which
sessions are ghosts.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D4 — Mount policy": the
  staging design is a **root helper** container that reads the 0700 originals
  and chowns the copy. It reads them from a mount — that mount is what this
  phase adds.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree and the live runtime at drafting time (2026-08-29,
commit `4cad433`):

- `cargo test --lib` → **1450 passed; 0 failed; 4 ignored**. Four gates green.
- `stage_args` builds this shell line and mounts only the destination volume:

  ```
  -v <de-stage-job>:/stage  …  sh -c
  "cp /de/src/<script> /stage/<script> && chmod 0500 … && chown <uid>:<gid> …"
  ```

  `grep -c '"/de/src' src/daemon/executor/container.rs` → **0**: the path
  appears only inside the shell string, never as a mount.
- `src/daemon/background/run.rs` hardcodes `is_ghost: false` in the one
  `ExecSpec` it builds (`grep -c "is_ghost: false"` → **1**), while the same
  function already branches on `sid.starts_with("ghost-")` at
  `run.rs:57-58` to pick the window prefix.
- `crate::config::scripts_dir()` (`src/config/load.rs:194`) returns
  `~/.daemoneye/scripts`.
- `cargo test --lib sandbox_stage` → **0** test lines (the vacuity trap).

## Gotchas

Six traps. Items 1–3 were measured live; the executor has no runtime.

1. **The staging helper is broken today, and the failure is exact.** Measured
   with a real 0700 script from `~/.daemoneye/scripts`:

   ```
   $ docker … run --rm --user 0:0 -v de-stage-proto9:/stage <image> \
       sh -c "cp /de/src/<script> /stage/<script> && …"
   cp: cannot stat '/de/src/<script>': No such file or directory
   ```

2. **Adding `-v <scripts_dir>:/de/src:ro` fixes it, and the whole chain then
   works.** Measured, same script:

   ```
   $ docker … run --rm --user 0:0 -v ~/.daemoneye/scripts:/de/src:ro \
       -v de-stage-proto9:/stage <image> sh -c "cp … && chmod 0500 … && chown 1000:1000 …"
   STAGED_OK
   $ docker … run --rm --user 1000:1000 --network none \
       -v de-stage-proto9:/de/scripts:ro <image> sh -c 'ls -l /de/scripts/…; head -1 …'
   -r-x------ 1 de de 10891 … <script>
   READABLE_BY_SANDBOX
   ```

   Note `-r-x------ de de`: the helper runs as **container root, which is host
   `matt`**, so it can read the 0700 originals — that is the whole reason D4
   uses a root helper. The sandboxed uid then reads the *copy*.

3. **The source mount must be read-only.** The helper runs as container root
   = host `matt`, so a writable mount would give a compromised helper write
   access to the operator's real script library. `:ro` is the difference
   between staging and handing over the keys.

4. **`is_ghost` is already derivable — do not invent a new signal.**
   `run.rs:57-58` decides the window prefix with
   `sid.starts_with("ghost-")`. Use the same predicate for `ExecSpec.is_ghost`
   so the two can never disagree; do not add a config flag or a parameter.

5. **This changes the pinned `stage_args` slice again.** Phases 07 and 08 each
   moved it. Update the expectation; do not work around it.

6. **`cargo test --lib sandbox_stage` passes today with zero tests.** Every
   criterion is a line count, not an exit status.

## Spec

### Task 1 — Mount the script source into the staging helper

In `stage_args` (`src/daemon/executor/container.rs`), add a read-only mount of
the host scripts directory at `/de/src`, immediately **before** the existing
`-v <volume>:/stage` pair:

```
"-v", format!("{}:/de/src:ro", crate::config::scripts_dir().display()),
"-v", format!("{volume}:/stage"),
```

The `:ro` suffix is mandatory (§ Gotchas 3). Everything else about the vector
is unchanged.

### Task 2 — Label ghost containers

In `src/daemon/background/run.rs`, replace the hardcoded `is_ghost: false` in
the `ExecSpec` with the same ghost predicate the function already uses for the
window prefix — `session_id.as_deref().is_some_and(|sid| sid.starts_with("ghost-"))`
or an equivalent expression over the existing binding. **Compute it once** into
a local before the `ExecSpec`, and do not duplicate the string literal
`"ghost-"` if a binding is already in scope that carries the answer.

No other behaviour changes: a ghost container still gets `de.sandbox=1` from
phase-08, and now also `de.ghost=1`.

### Task 3 — Unit tests

Add the tests named in § Test plan. Every name must contain `sandbox_stage`.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c ':/de/src:ro'`
      prints `1` (**before: 0**) — the read-only source mount. The `sed`
      scoping is required; the tests also contain the literal.
- [ ] `grep -c "is_ghost: false" src/daemon/background/run.rs` prints `0`
      (**before: 1**).
- [ ] `grep -c "is_ghost" src/daemon/background/run.rs` prints `1`
      (**before: 1**) — still exactly one mention, now a computed value
      rather than a literal `false`.
- [ ] `cargo test --lib sandbox_stage 2>&1 | grep -c "^test .* ok$"` prints
      `4` — one per test in § Test plan. A count, not an exit status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1454 passed; 0 failed; 4 ignored` (1450 + 4 new; ignored unchanged —
      this phase adds no `#[ignore]`).
- [ ] `grep -c "#\[ignore" src/daemon/executor/container.rs` prints `4`
      (**unchanged**).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `7` — **unchanged**. `stage_args` still has no production
      caller, so the attribute stays; do not add or remove any `#[allow]`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Four tests. Every name contains `sandbox_stage`.

**In `container.rs`:**

- `sandbox_stage_args_mount_the_script_source_read_only` — `stage_args`
  contains an element ending `:/de/src:ro`, and that element appears
  **before** the `…:/stage` element. **Negative half:** no element ends
  `:/de/src` without the `:ro` suffix (§ Gotchas 3) — assert on the full
  element, not a substring, so a writable mount cannot pass.
- `sandbox_stage_args_keep_the_root_helper_and_chown` — the vector still
  contains `--user` immediately followed by `0:0`, and its shell line still
  contains `chmod 0500` and `chown 1000:1000` for a default config. This is
  the D4 invariant the new mount must not disturb.
- `sandbox_stage_args_still_reject_unsafe_script_names` — `../etc/passwd`
  still yields an empty vector. The new mount must not bypass
  `script_name_is_safe`; a shell line that interpolates a name is now
  reachable from a directory the operator actually owns, so this guard
  matters more, not less.

**Ghost labelling — in `container.rs`, exercising `run_args` (the `run.rs`
change itself has no unit-testable seam):**

- `sandbox_stage_ghost_spec_carries_both_labels` — with `is_ghost: true` the
  vector contains **both** `de.sandbox=1` and `de.ghost=1`; with
  `is_ghost: false` it contains `de.sandbox=1` and **not** `de.ghost=1`. This
  duplicates part of phase-08's coverage deliberately: phase-09 is the phase
  that makes `is_ghost: true` reachable in production, so its own test plan
  should pin what that now produces.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_stage tests (expect 4 lines) =="
cargo test --lib sandbox_stage 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "ro source mount (1):   "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c ':/de/src:ro'
echo -n "hardcoded false (0):   "; grep -c "is_ghost: false" src/daemon/background/run.rs
echo -n "is_ghost mentions (1): "; grep -c "is_ghost" src/daemon/background/run.rs
echo -n "ignore count (4):      "; grep -c "#\[ignore" src/daemon/executor/container.rs
echo -n "allow(dead_code) (7):  "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
} > /tmp/e2e-09.txt 2>&1
cat /tmp/e2e-09.txt
```

Paste the contents of `/tmp/e2e-09.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-09-staging-mount-and-ghost-label.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-09.txt
diff /tmp/pasted-09.txt /tmp/e2e-09.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit `src/daemon/executor/container.rs` and
  `src/daemon/background/run.rs`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The staging chain was prototyped by
  the architect and is re-verified at milestone close.
- **Do not add or remove any `#[allow(...)]` or `#[ignore]`.**
- **Append to the Update Log; never edit or delete an existing entry.**
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause — record a blocker
  Update Log entry naming the exact criterion or failing test, and stop.
  Reporting the blocker *is* the successful outcome.** Do not proceed past a
  blocker you have filed.
- **Record what you decide, not what you wish had been decided.**

## Out of scope

- **Calling `stage_args` from production.** This phase makes the helper
  *correct*; nothing invokes it yet, so `script_name_is_safe` and `stage_args`
  stay unreachable and the `#[allow(dead_code)]` stays with them. Wiring a
  caller means deciding when a background command *is* a script invocation,
  which is its own phase.
- Ghost-scoped teardown beyond the label — no per-ghost `docker rm -f` on
  ghost exit; the phase-08 startup sweep already reclaims orphans.
- The escape hatch, the egress proxy, `Request::ContainerStatus`, the `log`
  relay opcode, docs and the pilot.
- Editing `gc.rs`, `CLAUDE.md`, `README.md`, `assets/etc/config.toml`, or
  `containers/Dockerfile`.

## Update Log
