# Phase 01: Sandbox configuration schema

**Milestone:** M18 — Container-sandboxed Agents
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~330 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the `[sandbox]` configuration section — `SandboxConfig` plus its three
nested structs, serde defaults, a `validate()` that warns on the two
configurations that would silently defeat the sandbox, and full documentation
in the seeded config template. Nothing reads these values yet; phases 02+ do.
This phase is entirely hermetic — it does not touch Docker and must pass on a
host with no container runtime installed.

## Architecture references

Read before starting:

- `docs/design/agent-container-sandboxing.md` § "D1 — Runtime" — why
  `run_as` defaults to `1000:1000` and why container root is the failure
  case this config must warn about.
- `docs/design/agent-container-sandboxing.md` § "Config schema" — the target
  shape. **This phase is the authority on field names**; where the design's
  sketch and this doc disagree, follow this doc.
- `docs/dev/milestones/M18-container-sandboxing/README.md` — the milestone's
  exit criteria and the executor-host constraint (gates must stay green
  without docker).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-28, commit `70a3389`):

- `cargo test --lib` → **1387 passed; 0 failed; 0 ignored**. All four gates green.
- `grep -rn "SandboxConfig\|\[sandbox\]\|pub sandbox" src/ assets/ tests/` → **0 matches**.
  Nothing named `sandbox` exists anywhere in the crate yet.
- `src/config/types.rs` is **928 lines**. The top-level `Config` struct is at
  line 6; its last field is `pub retention: RetentionConfig,` at line 40,
  followed by `}` at line 41. `impl Default for Config` is at line 43 and
  mirrors every field in the same order.
- `src/daemon/mod.rs:479` calls `startup_config.limits.validate();` — the
  single startup validation site this phase extends.
- `assets/etc/config.toml` has **0** occurrences of `sandbox`. Its sections are
  ordered with a `# ── <Name> ───…` banner comment before each.

### The existing section idiom (copy this shape)

`GhostDaemonConfig` at `src/config/types.rs:366-395` is the closest analogue —
a small section struct with per-field `#[serde(default = "…")]` free functions
and a hand-written `Default` that calls the same functions:

```rust
pub struct GhostDaemonConfig {
    /// Hard upper limit on AI turns per ghost shell.
    /// Individual runbooks may set a lower value with `max_ghost_turns`
    /// but can never exceed this ceiling. Default: 20.
    #[serde(default = "default_max_ghost_turns")]
    pub max_ghost_turns: usize,
    /// Maximum number of ghost shells that may run concurrently.
    #[serde(default = "default_max_concurrent_ghosts")]
    pub max_concurrent_ghosts: usize,
}

fn default_max_ghost_turns() -> usize {
    20
}

impl Default for GhostDaemonConfig {
    fn default() -> Self {
        Self {
            max_ghost_turns: default_max_ghost_turns(),
            max_concurrent_ghosts: default_max_concurrent_ghosts(),
        }
    }
}
```

### The existing `validate()` idiom (copy this shape)

`LimitsConfig::validate` at `src/config/types.rs:498-515`. It **only warns**
via `log::warn!` and can never panic or return an error:

```rust
    /// Emit warnings for configuration that is likely unintentional.
    /// Call once at daemon startup after the config is loaded.
    pub fn validate(&self) {
        for tool in crate::ai::tools::APPROVAL_GATED_TOOLS {
            if self.per_tool.contains_key(*tool) {
                log::warn!(
                    "[limits] per_tool.{tool} is set but {tool} is approval-gated and \
                     exempt from per-tool caps — this entry has no effect"
                );
            }
        }
    }
```

Its startup call site, `src/daemon/mod.rs:478-479`:

```rust
    // Warn about any limit configuration that is likely unintentional.
    startup_config.limits.validate();
```

## Gotchas

Four traps, each of which fails a gate you did not touch or produces a test
that cannot fail. All four were measured against this tree while drafting.

1. **`tests/doc_truth.rs` gates `assets/etc/config.toml` in both directions,
   automatically.** `config_sections()` derives the section list from the
   `Config` struct itself, so the moment you add `pub sandbox: SandboxConfig`
   to `Config`, **every `pub` field of `SandboxConfig` must appear as a
   `key =` line in `assets/etc/config.toml`** (commented is fine) or
   `seeded_config_template_documents_every_config_field` fails. The reverse
   test, `seeded_config_template_has_no_phantom_keys`, fails if the template
   documents a `[sandbox]` key that is not a `SandboxConfig` field.

2. **The `profile` map needs a bare `# [sandbox.profile]` line, and this is
   not guessable.** The gate satisfies a struct field either by a `key =`
   line or by a `[section.<field>]` sub-table heading — but it matches the
   sub-table on the heading's **last** dot-segment. `[sandbox.profile.researcher]`
   therefore registers as `researcher`, **not** `profile`, and the `profile`
   field reads as undocumented. Simulated against the real gate functions
   while drafting:

   ```
   template with only [sandbox.profile.researcher]  -> MISSING: ['profile']
   same template plus a bare [sandbox.profile] line -> MISSING: []
   ```

   So the template must carry a commented `# [sandbox.profile]` heading line
   **in addition to** the concrete `# [sandbox.profile.researcher]` example.
   `limits` and `ghost_defaults` do not need this — `[sandbox.limits]` and
   `[sandbox.ghost_defaults]` already end in the field name.

3. **`cargo test sandbox` passes today, with zero tests.** A bare filter is
   satisfied by a test that was never written — `cargo test` exits 0 when the
   filter matches nothing. Measured on this tree, the § End-to-end block's
   section A printed **no test lines at all** while reporting `cargo_exit=0`.
   That is why every criterion below is a **line count**, never an exit
   status. Do not "fix" a criterion by checking the exit code.

4. **`run_as` must not be matched by substring.** `"10:0"` contains `0` and
   `:0` but its uid is `10`, which is not root. Split on `:` and test the
   **first field only**. The test plan pins this case explicitly.

## Spec

### Task 1 — Add the three nested structs

In `src/config/types.rs`, immediately **after** the `impl Default for
GhostDaemonConfig` block (which ends at line 395) and before the
`/// Daemon-wide caps on tool call frequency…` doc comment that introduces
`LimitsConfig`, add three structs. Follow the `GhostDaemonConfig` shape quoted
in § Current state: doc comment naming the default on every field, per-field
`#[serde(default = "…")]`, free default functions, hand-written `Default`.

```rust
/// Per-container resource ceilings (`[sandbox.limits]`).
pub struct SandboxLimits {
    /// Container memory ceiling, in Docker's `--memory` syntax. Default: "1g".
    pub memory: String,
    /// Maximum process IDs inside the container (`--pids-limit`), which is what
    /// bounds a fork bomb. Default: 256.
    pub pids: u32,
    /// CPU quota (`--cpus`). Default: 2.0.
    pub cpus: f64,
    /// Size of the `/de/work` scratch tmpfs. Default: "2g".
    pub scratch: String,
}

/// Per-profile overrides (`[sandbox.profile.<name>]`), keyed by agent/profile name.
pub struct SandboxProfile {
    /// "none" (default, no route out) or "proxy" (egress via the containerized
    /// proxy on a shared user-defined network).
    pub network: String,
    /// Hostnames this profile may reach when `network = "proxy"`. Ignored for
    /// "none". Default: empty.
    pub proxy_allow: Vec<String>,
}

/// Defaults applied to ghost-shell containers (`[sandbox.ghost_defaults]`).
pub struct SandboxGhostDefaults {
    /// Destroy the container on ghost exit, on every path including failure.
    /// Default: true.
    pub destroy_on_exit: bool,
    /// Mount mode for the staged script volume. Default: "ro".
    pub mount_scripts: String,
}
```

Derive `Debug, Deserialize, Serialize, Clone` on all three, as every other
section struct in the file does. Field defaults: `memory` `"1g"`, `pids` `256`,
`cpus` `2.0`, `scratch` `"2g"`, `network` `"none"`, `proxy_allow` empty,
`destroy_on_exit` `true`, `mount_scripts` `"ro"`.

### Task 2 — Add `SandboxConfig`

In the same location, after the three structs from Task 1, add:

```rust
/// Container sandboxing for agent command execution (`[sandbox]`).
/// Nothing here takes effect while `enabled = false`; later M18 phases consume
/// these values. The defaults describe the safe, disabled state.
pub struct SandboxConfig {
    /// Master feature flag. Default: false — behaviour is unchanged until set.
    pub enabled: bool,
    /// Container runtime. Only "docker" is supported today.
    pub runtime: String,
    /// Image the agent containers run. Default: "daemoneye-agent-base".
    pub image: String,
    /// Working directory / scratch mount point inside the container.
    /// Default: "/de/work".
    pub workdir: String,
    /// `--user` value for every sandboxed process, as "uid:gid".
    /// Default: "1000:1000". Under rootless Docker container root maps to the
    /// daemon's own host uid, so running as root would return execution to the
    /// exact identity the sandbox exists to contain.
    pub run_as: String,
    /// Docker API endpoint. Default: "unix:///run/user/1000/docker.sock"
    /// (the rootless per-user socket, not the rootful /var/run one).
    pub docker_host: String,
    pub limits: SandboxLimits,
    pub profile: std::collections::HashMap<String, SandboxProfile>,
    pub ghost_defaults: SandboxGhostDefaults,
}
```

`enabled` uses a bare `#[serde(default)]` (bool defaults to false, matching how
`total_tool_calls_per_turn` at `types.rs:415` handles its zero default). The
three struct/map fields also use bare `#[serde(default)]`. The five string
fields each need a named default function.

### Task 3 — Add `runs_as_container_root()`

In an `impl SandboxConfig` block, add:

```rust
    /// True when `run_as` would put the process at container uid 0 — which
    /// under rootless Docker is the daemon's own host uid. An empty value
    /// counts as root, because Docker's own default is root when `--user` is
    /// omitted.
    pub fn runs_as_container_root(&self) -> bool {
```

Semantics, pinned exactly. Take the substring before the first `:`, trim it,
and return true when that uid field is `""`, `"0"`, or `"root"`. Return false
otherwise. **Compare the whole uid field, never a substring of `run_as`** —
`"10:0"` must return `false`.

### Task 4 — Add `SandboxConfig::validate()`

In the same `impl` block, following the `LimitsConfig::validate` shape quoted
in § Current state — `log::warn!` only, no panic, no `Result`:

```rust
    /// Emit warnings for sandbox configuration that would silently defeat the
    /// sandbox. Call once at daemon startup after the config is loaded.
    pub fn validate(&self) {
```

Emit a warning, each with the `[sandbox]` prefix the other sections use, for
each of these conditions:

1. `self.runtime != "docker"` — name the value and say only "docker" is
   supported.
2. `self.runs_as_container_root()` — say that container root maps to the
   daemon's host uid under rootless Docker, so the sandbox would not reduce
   the blast radius.
3. For each `(name, profile)` in `self.profile` whose `network` is neither
   `"none"` nor `"proxy"` — name the profile and the bad value.
4. For each `(name, profile)` with `network == "proxy"` and an empty
   `proxy_allow` — say the profile can reach nothing and is equivalent to
   `"none"`.

Warnings 1 and 2 must fire regardless of `enabled`, so a misconfiguration is
visible before the flag is turned on.

### Task 5 — Wire `SandboxConfig` into `Config`

In `src/config/types.rs`: add

```rust
    #[serde(default)]
    pub sandbox: SandboxConfig,
```

as the **last** field of `pub struct Config` (after `pub retention:
RetentionConfig,`, currently line 40), and add the matching
`sandbox: SandboxConfig::default(),` as the last entry of `impl Default for
Config` (after `retention: RetentionConfig::default(),`).

### Task 6 — Call `validate()` at startup

In `src/daemon/mod.rs`, directly after the existing
`startup_config.limits.validate();` at line 479, add a comment in the same
style plus:

```rust
    // Warn about sandbox configuration that would silently defeat the sandbox.
    startup_config.sandbox.validate();
```

### Task 7 — Document every field in the seeded template

In `assets/etc/config.toml`, add a `[sandbox]` block. Place it directly after
the `# ── Ghost shells ───…` block (which ends with the
`max_concurrent_ghosts` line) and before the `# ── Daemon ───…` banner, using
the same `# ── Sandbox ───…` banner style and the same fully-commented form as
every other section.

**Re-read § Gotchas items 1 and 2 before writing this.** The block must
contain a `key =` line for every one of the six scalar `SandboxConfig` fields,
a `[sandbox.limits]` table with all four keys, a `[sandbox.ghost_defaults]`
table with both keys, **a bare `# [sandbox.profile]` heading line**, and a
`# [sandbox.profile.researcher]` example carrying `network` and `proxy_allow`.
Every line stays commented out, so the shipped default behaviour is unchanged.

### Task 8 — Add the unit tests

Add the tests named in § Test plan to the existing `mod tests` in
`src/config/mod.rs`, alongside the `limits`/`approvals` section tests already
there (`limits_section_parses_all_fields` at line 295 is the closest model for
the TOML-parsing tests). Every TOML fixture needs the `[models.default]`
preamble those tests already use:

```rust
        let toml_src = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"

            [sandbox]
            enabled = true
        "#;
```

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
`PASTE MATCH` verdict line the block prints.

## Acceptance criteria

Every count below was measured against the current tree while drafting and is
`0` or absent today.

- [ ] `grep -c "^pub struct SandboxConfig" src/config/types.rs` prints `1`.
- [ ] `grep -c "pub sandbox: SandboxConfig" src/config/types.rs` prints `1`.
- [ ] `grep -c "startup_config.sandbox.validate()" src/daemon/mod.rs` prints `1`.
- [ ] `grep -c "^# \[sandbox.profile\]$" assets/etc/config.toml` prints `1`
      (the bare heading of § Gotchas item 2 — **not** the `.researcher` line).
- [ ] `cargo test --test doc_truth seeded_config_template 2>&1 | grep -c "^test .* ok$"`
      prints `2`. This is a **regression guard**, so unlike the criteria above
      it already passes today — its meaningful direction is that it must fail
      when the template is wrong. Proven live while drafting: seeding
      `# nonexistent_knob = 1` under `[ghost]` made
      `seeded_config_template_has_no_phantom_keys` FAIL, naming
      `"[ghost] nonexistent_knob"`; reverting restored `2 passed`. If your
      `[sandbox]` block is wrong in either direction, this gate bites.
- [ ] `cargo test --lib sandbox 2>&1 | grep -c "^test .* ok$"` prints `8` —
      one line per test named in § Test plan. A count, not an exit status, per
      § Gotchas item 3.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports `1395 passed;
      0 failed` (1387 today + the 8 new tests).
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`.

## Test plan

All eight in `src/config/mod.rs`'s `mod tests`. Each must contain the substring
`sandbox` in its name so the filter in § Acceptance criteria matches it.

- `sandbox_defaults_are_disabled_and_non_root` — `SandboxConfig::default()` has
  `enabled == false`, `runtime == "docker"`, `run_as == "1000:1000"`,
  `workdir == "/de/work"`, and `runs_as_container_root() == false`.
- `sandbox_limits_defaults_match_documented_values` — `memory == "1g"`,
  `pids == 256`, `cpus == 2.0`, `scratch == "2g"`.
- `missing_sandbox_section_uses_defaults` — a config TOML with no `[sandbox]`
  parses and yields `SandboxConfig::default()` values.
- `sandbox_section_parses_all_fields` — a TOML setting all six scalars plus
  `[sandbox.limits]` and `[sandbox.ghost_defaults]` round-trips every value.
- `partial_sandbox_section_fills_remaining_defaults` — a `[sandbox]` setting
  only `enabled = true` leaves `run_as`, `runtime` and the nested structs at
  their defaults.
- `sandbox_profile_table_parses_named_profiles` — `[sandbox.profile.researcher]`
  with `network = "proxy"` and `proxy_allow = ["crates.io"]` lands in the map
  under key `"researcher"`; a profile absent from the map is simply absent
  (no panic).
- `sandbox_run_as_root_detection_pins_negative_cases` — the table below, which
  is the § Gotchas item 4 guard. **All five cases required**:

  | `run_as` | `runs_as_container_root()` |
  |---|---|
  | `"1000:1000"` | `false` |
  | `"10:0"` | `false` |
  | `"0:0"` | `true` |
  | `"root:root"` | `true` |
  | `""` | `true` |

- `sandbox_validate_warns_and_never_panics` — following
  `validate_approval_gated_per_tool_entry_does_not_panic` at
  `src/config/mod.rs:392`: build a `SandboxConfig` with `run_as = "0:0"`, a
  `runtime` of `"podman"`, and a profile with `network = "proxy"` and empty
  `proxy_allow`; assert the preconditions hold, then call `validate()` and
  assert it returns (the warnings are observable in `daemon.log` at runtime).

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-01.txt`, then
re-extracts the fence you pasted and diffs it against that file.

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-01-sandbox-config.md
{
echo "== A. sandbox unit tests (expect 8 lines) =="
cargo test --lib sandbox 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. doc_truth seeded-config gates (expect 2 lines) =="
cargo test --test doc_truth seeded_config_template 2>&1 | grep -E "^test .* ok$"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. structural greps =="
echo -n "SandboxConfig struct: "; grep -c "^pub struct SandboxConfig" src/config/types.rs
echo -n "Config field:         "; grep -c "pub sandbox: SandboxConfig" src/config/types.rs
echo -n "startup validate:     "; grep -c "startup_config.sandbox.validate()" src/daemon/mod.rs
echo -n "bare profile heading: "; grep -c "^# \[sandbox.profile\]$" assets/etc/config.toml
} > /tmp/e2e-01.txt 2>&1
cat /tmp/e2e-01.txt
```

Paste the contents of `/tmp/e2e-01.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-01-sandbox-config.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01.txt
diff /tmp/pasted-01.txt /tmp/e2e-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section A is the one that can lie.** On the current tree it prints zero test
lines and still reports `cargo_exit=0` — measured while drafting. Eight lines
is the pass condition; the exit code is not.

The self-check itself was validated both ways while drafting, against a copy
of this doc: a byte-exact paste printed `PASTE MATCH`, and the same paste with
one line retyped printed `PASTE MISMATCH`.

## Authorizations

- Edit `src/config/types.rs`, `src/config/mod.rs`, `src/daemon/mod.rs`, and
  `assets/etc/config.toml`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- If an acceptance criterion cannot be satisfied honestly, **record a blocker
  Update Log entry naming the criterion and stop**. Do not improvise past it,
  do not edit this phase doc's criteria, and do not run any command that
  touches a tmux server, a system service, or `docker`.

## Out of scope

- **Anything that talks to Docker.** No `docker` invocation, no runtime probe,
  no `std::process::Command` for a container. This phase is pure schema; the
  runtime probe is phase-02. The gates must pass on a host with no container
  runtime installed.
- Reading `SandboxConfig` anywhere outside `validate()` — no executor, ghost,
  or background-execution wiring. Those are phases 04–08.
- `containers/Dockerfile`, the image lockfile, and `daemoneye sandbox build`
  (phase-03).
- Changing any existing config field, default, or test.
- Editing `CLAUDE.md` or `README.md` (phase-10 does the doc sweep).

## Update Log

### Update — 2026-08-28 16:40 (started)

Executor session for phase-01 (sandbox schema). Status flipped
todo → in-progress; milestone README row updated to match. Implementing the
eight Spec tasks: three nested structs + `SandboxConfig`+ `runs_as_container_root()`
+ `validate()` added to `src/config/types.rs`, wired into `Config` and the
startup `validate()` call, `[sandbox]` docs added to `assets/etc/config.toml`,
eight unit tests appended to `src/config/mod.rs`'s `mod tests`. All four gates
green so far; e2e capture is the final task.

### Update — 2026-08-28 16:42 (end-to-end verification)

```
== A. sandbox unit tests (expect 8 lines) ==
test config::tests::sandbox_limits_defaults_match_documented_values ... ok
test config::tests::sandbox_defaults_are_disabled_and_non_root ... ok
test config::tests::sandbox_run_as_root_detection_pins_negative_cases ... ok
test config::tests::partial_sandbox_section_fills_remaining_defaults ... ok
test config::tests::missing_sandbox_section_uses_defaults ... ok
test config::tests::sandbox_validate_warns_and_never_panics ... ok
test config::tests::sandbox_profile_table_parses_named_profiles ... ok
test config::tests::sandbox_section_parses_all_fields ... ok
cargo_exit=0
== B. doc_truth seeded-config gates (expect 2 lines) ==
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
cargo_exit=0
== C. lib suite totals ==
test result: ok. 1395 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.25s
cargo_exit=0
== D. structural greps ==
SandboxConfig struct: 1
Config field:         1
startup validate:     1
bare profile heading: 1
```

PASTE MATCH
