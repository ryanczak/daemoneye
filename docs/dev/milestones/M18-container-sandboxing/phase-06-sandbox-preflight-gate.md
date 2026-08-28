# Phase 06: Fail closed — the sandbox preflight gate

**Milestone:** M18 — Container-sandboxed Agents
**Status:** todo
**Depends on:** phase-04 (`evaluate_preflight`, `SandboxUnavailable`), phase-05 (`sandbox_window_command` and its call site)
**Estimated diff:** ~380 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase-05 shipped sandboxed background execution **with no preflight**: if the
runtime is missing, the uid map is wrong, or the image does not match the
lock, the `de-bg-*` window just runs a `docker` line that fails with a
confusing error. This phase adds the gate — probe once, cache the verdict, and
**refuse the command** when the sandbox is not sane, instead of running it on
the host.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D1 — Runtime": the uid gate
  and why a container running as root defeats the design.
- `docs/design/agent-container-sandboxing.md` § "Image lifecycle": the lock
  exists so a drifted image is refused, not silently run.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-28, commit `f4880ef`):

- `cargo test --lib` → **1432 passed; 0 failed; 2 ignored**. Four gates green.
- `container.rs` already provides, from phase-04, everything the *decision*
  needs — **reuse them, do not re-derive**:
  `evaluate_preflight(run_as, &Result<String, RuntimeUnavailable>,
  &UidGateOutcome, Option<&SandboxLock>, live_image_id) -> Result<(),
  SandboxUnavailable>`, plus `probe_runtime`, `evaluate_uid_gate`,
  `read_lock`, and the `SandboxUnavailable` variants
  `BadRunAs / Runtime / UidGate / NoLock / Image`.
- Phase-05 wired `sandbox_window_command` into
  `src/daemon/background/run.rs` at line ~166-186, inside a block that
  currently reads `if config.sandbox.enabled { … }`.
- `run_background_in_window` (`run.rs:35-43`) returns **`String`** — the text
  handed back to the AI as the tool result. Refusal is therefore expressible:
  return a message instead of opening a window.
- `grep -c "pub fn sandbox_preflight" src/daemon/executor/container.rs` → **0**.
- `cargo test --lib sandbox_gate` → **0** test lines (the vacuity trap).
- `~/.daemoneye/etc/sandbox.lock` does **not** exist on this host —
  `daemoneye sandbox build` has never been run. So a live preflight today
  returns `SandboxUnavailable::NoLock`, which is correct behaviour and is
  pinned as a live-test expectation below.

### The cached-static idiom (copy this shape)

`src/daemon/mod.rs:17-25` — a `OnceLock` static with a small accessor:

```rust
static DAEMON_START: OnceLock<Instant> = OnceLock::new();

/// Returns the number of seconds since the daemon started, or 0 before init.
pub fn daemon_uptime_secs() -> u64 {
    DAEMON_START
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0)
}
```

Do the same shape for the preflight verdict. Probing costs a container start,
so it must happen **once per daemon lifetime**, not once per command.

## Gotchas

Six traps. Items 1–3 were measured on this host; the executor has no runtime.

1. **One container run yields both gate inputs.** Measured — do not start two
   containers:

   ```
   $ docker run --rm --user 1000:1000 --network none daemoneye-agent-base \
       sh -c 'id -u; echo ---; cat /proc/self/uid_map'
   1000
   ---
            0       1000          1
            1     100000      65536
   ```

   The `---` line is the sentinel your parser splits on. Everything before it
   is the container uid; everything after is the uid map, passed to
   `evaluate_uid_gate` unchanged.

2. **The live image id comes from `docker image inspect`, not from the run.**
   Measured: `docker image inspect daemoneye-agent-base --format '{{.Id}}'`
   → `sha256:0d02beb…`. **Do not hardcode that value** — it changes on every
   rebuild; a criterion pins that no 64-hex `sha256:` literal appears in
   production code.

3. **`NoLock` is the expected verdict on a fresh host.** Measured:
   `~/.daemoneye/etc/sandbox.lock` does not exist here, because
   `daemoneye sandbox build` has not been run. The live test asserts the
   verdict is `Ok(())` **or** `NoLock` — anything else is a real failure.
   Do not "fix" a `NoLock` by writing a lock file from this phase.

4. **Fail closed, not open.** When `[sandbox] enabled = true` and preflight
   fails, the command must be **refused** — the operator asked for isolation,
   so silently running it on the host is the wrong answer and the one this
   phase exists to prevent. Return the refusal message as the tool result and
   never open the window.

5. **Probe once.** `sandbox_preflight` must consult the cached `OnceLock`
   before shelling out. A per-command probe adds a container start to every
   background command and would be visible as latency.

6. **`cargo test --lib sandbox_gate` passes today with zero tests.** Every
   criterion is a line count, not an exit status.

## Spec

### Task 1 — Parse the probe output (pure)

In `src/daemon/executor/container.rs`:

```rust
/// Split the combined probe's stdout into `(container_uid, uid_map)`.
/// The probe prints the uid, a line containing only `---`, then the map.
/// `None` when the sentinel is missing or the uid line is not a `u32`.
pub fn parse_probe_output(text: &str) -> Option<(u32, String)>
```

Trim the uid line. Return the map **unmodified apart from leading/trailing
newlines** — `evaluate_uid_gate` does its own whitespace handling and the
map's internal padding is meaningful to `parse_uid_map`.

### Task 2 — Describe a refusal (pure)

```rust
/// Operator-facing explanation of why sandboxed execution is unavailable,
/// including the concrete fix. Used as the tool result the AI sees.
pub fn describe_unavailable(reason: &SandboxUnavailable) -> String
```

Every message must start with the literal prefix `sandbox unavailable: ` and
name the remedy. One arm per variant — `NotInstalled` says to install the
runtime, `DaemonUnreachable` names the `docker_host` and says to start the
user service, `UnsupportedRuntime` names the value, `UidGate` says the
container would not run as an unprivileged uid, `NoLock` says to run
`daemoneye sandbox build`, `Image` says the image differs from the lock and to
rebuild, `BadRunAs` names the bad `run_as`.

### Task 3 — The impure probe and the cached verdict

```rust
/// Run the combined probe and the image inspection, then decide.
/// Impure: starts one container and runs one `image inspect`.
fn collect_preflight(cfg: &SandboxConfig) -> Result<(), SandboxUnavailable>

/// Cached sandbox verdict — probed once per daemon lifetime.
pub fn sandbox_preflight(cfg: &SandboxConfig) -> Result<(), SandboxUnavailable>
```

`collect_preflight` must:

1. `probe_runtime(cfg)` for the version result (phase-02; already handles the
   missing-binary vs dead-daemon distinction).
2. Run the combined probe of § Gotchas 1 through
   `crate::tmux::bounded_output_with(&mut cmd, Duration::from_secs(30))`,
   with `DOCKER_HOST` from `cfg.docker_host`. Feed its stdout to
   `parse_probe_output`, then `evaluate_uid_gate(uid, &map)`. If the probe
   fails to run or does not parse, use `UidGateOutcome::MalformedMap`.
3. Read the live image id with
   `<runtime> image inspect <cfg.image> --format {{.Id}}`, trimmed. On failure
   use the empty string — `evaluate_preflight` already treats a malformed live
   id correctly.
4. `read_lock()` for the lock.
5. Return `evaluate_preflight(&cfg.run_as, &version, &gate, lock.as_ref(),
   &live_id)`.

`sandbox_preflight` wraps it in a `OnceLock` following the § Current state
idiom, so the probe runs once. **Do not** probe when `!cfg.enabled` — return
`Ok(())` immediately, so a disabled sandbox never starts a container.

### Task 4 — Fail closed at the call site

In `src/daemon/background/run.rs`, inside the existing
`if config.sandbox.enabled { … }` block that phase-05 added, **before**
calling `sandbox_window_command`:

- Call `sandbox_preflight(&config.sandbox)`.
- On `Err(reason)`, `log::warn!` the reason and **return
  `describe_unavailable(&reason)` from `run_background_in_window`
  immediately** — no window, no `send-keys`, no container. The function
  returns `String`, so the refusal becomes the tool result the AI reads.
- On `Ok(())`, proceed exactly as today.

The refusal must happen **before** any tmux window is created, so a refused
command leaves no `de-bg-*` window behind.

### Task 5 — Unit tests

Add the tests named in § Test plan to `container.rs`'s existing `mod tests`.
Every name must contain `sandbox_gate`.

### Task 6 — One `#[ignore]`d live test

Add exactly one, `sandbox_gate_preflight_reaches_a_real_runtime`, marked
`#[ignore = "requires a running rootless Docker daemon"]`. It calls
`sandbox_preflight` with an `enabled = true` config and asserts the result is
`Ok(())` **or** `Err(SandboxUnavailable::NoLock)` — per § Gotchas 3, a host
that has never run `daemoneye sandbox build` legitimately reports `NoLock`.
Any other variant fails the test. The `#[ignore]` count becomes **3**.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `grep -c "pub fn parse_probe_output" src/daemon/executor/container.rs`
      prints `1` (**before: 0**).
- [ ] `grep -c "pub fn describe_unavailable" src/daemon/executor/container.rs`
      prints `1` (**before: 0**).
- [ ] `grep -c "pub fn sandbox_preflight" src/daemon/executor/container.rs`
      prints `1` (**before: 0**).
- [ ] `grep -c "sandbox_preflight" src/daemon/background/run.rs` prints `1`
      (**before: 0**) — the single gate call site.
- [ ] `grep -c "describe_unavailable" src/daemon/background/run.rs` prints `1`
      (**before: 0**) — the refusal is returned, not swallowed.
- [ ] `grep -c "OnceLock" src/daemon/executor/container.rs` prints `1`
      (**before: 0**) — the verdict is cached (§ Gotchas 5).
- [ ] `cargo test --lib sandbox_gate 2>&1 | grep -c "^test .* ok$"` prints
      `7` — one per non-ignored test in § Test plan. A count, not an exit
      status.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1439 passed; 0 failed; 3 ignored` (1432 + 7 new; ignored 2 → 3).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `7` — **unchanged**. This phase wires most of the module's
      remaining items, but `stage_args` and `script_name_is_safe` stay
      unreachable until staging lands, so the attribute must stay and **no
      new one may be added**. Do not attempt to remove it; if you believe it
      can go, record a blocker with the clippy output rather than deleting it.
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -cE 'sha256:[0-9a-f]{64}'`
      prints `0` (**before: 0**) — no image digest is hardcoded
      (§ Gotchas 2). The `sed` scoping is required: test fixtures may contain
      synthetic digests.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

Seven non-ignored tests plus the one ignored live test, all in
`container.rs`. Every name contains `sandbox_gate`.

**`parse_probe_output`** — use the **measured** probe output from
§ Gotchas 1 as the fixture:

- `sandbox_gate_parses_the_real_probe_output` — the fixture yields
  `Some((1000, map))` where the map, passed straight to `parse_uid_map`,
  produces the two ranges `(0, 1000, 1)` and `(1, 100000, 65536)`. **Assert
  through `parse_uid_map`, not by string equality** — that proves the map
  survived the split in a form the gate can actually use.
- `sandbox_gate_probe_output_rejects_malformed_input` — each yields `None`:
  no `---` sentinel; a non-numeric uid line; the empty string; a sentinel but
  an empty uid line.
- `sandbox_gate_probe_output_feeds_the_uid_gate` — the parsed pair passed to
  `evaluate_uid_gate` yields `UidGateOutcome::Ok { container_uid: 1000,
  host_uid: 100999 }`. This is the end-to-end pure path from probe text to
  verdict.

**`describe_unavailable`** — pin behaviour, not wording:

- `sandbox_gate_describes_every_unavailable_variant` — one call per variant
  (`BadRunAs`, `Runtime(NotInstalled)`, `Runtime(DaemonUnreachable)`,
  `Runtime(UnsupportedRuntime)`, `UidGate(ContainerRoot)`, `NoLock`,
  `Image(Mismatch)`); every message starts with `sandbox unavailable: `, and
  **all seven strings are distinct** (collect into a `HashSet` and assert its
  length is 7). Distinctness is the real property — an arm that falls through
  to a generic message passes a prefix check and fails this.
- `sandbox_gate_describes_nolock_with_the_build_command` — the `NoLock`
  message contains `sandbox build`, the actual remedy.
- `sandbox_gate_describes_bad_run_as_with_the_offending_value` — the
  `BadRunAs { run_as: "nope" }` message contains `nope`.

**Disabled short-circuit:**

- `sandbox_gate_disabled_config_is_ok_without_probing` — with
  `SandboxConfig::default()` (`enabled` false), `sandbox_preflight` returns
  `Ok(())`. This must hold **on a host with no docker at all**, which is what
  makes it safe to run in the default suite (§ Gotchas: `!cfg.enabled` returns
  before any process spawn).

**Ignored:**

- `sandbox_gate_preflight_reaches_a_real_runtime` — per Task 6.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. sandbox_gate tests (expect 7 lines) =="
cargo test --lib sandbox_gate 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "parse_probe_output:   "; grep -c "pub fn parse_probe_output" src/daemon/executor/container.rs
echo -n "describe_unavailable: "; grep -c "pub fn describe_unavailable" src/daemon/executor/container.rs
echo -n "sandbox_preflight:    "; grep -c "pub fn sandbox_preflight" src/daemon/executor/container.rs
echo -n "gate call site:       "; grep -c "sandbox_preflight" src/daemon/background/run.rs
echo -n "refusal returned:     "; grep -c "describe_unavailable" src/daemon/background/run.rs
echo -n "verdict cached:       "; grep -c "OnceLock" src/daemon/executor/container.rs
echo -n "allow(dead_code) tot: "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "no hardcoded digest:  "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -cE 'sha256:[0-9a-f]{64}'
} > /tmp/e2e-06.txt 2>&1
cat /tmp/e2e-06.txt
```

Paste the contents of `/tmp/e2e-06.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-06-sandbox-preflight-gate.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06.txt
diff /tmp/pasted-06.txt /tmp/e2e-06.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section A is the one that can lie** — on the current tree it prints zero
test lines and still reports `cargo_exit=0`. Seven lines is the pass
condition.

## Authorizations

- Edit `src/daemon/executor/container.rs` and
  `src/daemon/background/run.rs`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The one live test is `#[ignore]`d;
  the architect runs it at milestone close.
- **Do not add or remove any `#[allow(...)]`.** The module's existing
  attribute stays — see the criterion above and its reason.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause — record a blocker
  Update Log entry naming the exact criterion or failing test, and stop.
  Reporting the blocker *is* the successful outcome in that case.** Do not
  proceed past a blocker you have filed: if you write one, stop there and let
  the architect reconcile it. Do not repair unrelated code to turn a gate
  green, and do not edit this phase doc's criteria.
- **Record what you decide, not what you wish had been decided.** Update Log
  entries are the audit trail: never attribute a choice to guidance you did
  not receive.

## Out of scope

- **Staging** — `stage_args` and `script_name_is_safe` stay unwired, and the
  `#[allow(dead_code)]` stays with them.
- **Volume and container GC** — still unclaimed; a later phase owns
  `docker rm -f`, the `de.ghost=1` orphan sweep, and the `de-stage-*` volume
  leak phase-05 recorded.
- Ghost-specific container lifecycle, the escape hatch, the egress proxy,
  `Request::ContainerStatus`, and the `log` relay opcode.
- **Foreground execution** — unchanged, host-level by design.
- Changing `sandbox_window_command`'s own internal fallback: with the gate in
  front of it, its bad-`run_as` branch becomes unreachable in practice and
  stays as defence in depth. Leave it and its test alone.
- Editing `CLAUDE.md`, `README.md`, `assets/etc/config.toml`, or
  `containers/Dockerfile`.

## Update Log
