# Phase 01: Execution backend config and shell paths

**Milestone:** M20 — Shell Engine
**Status:** todo
**Depends on:** none
**Estimated diff:** ~380 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the `[execution]` and `[shells]` config sections, the two runtime path
constructors (`var/run/shells`, `var/log/shells`), and their lifecycle and
path-inventory entries — so every later M20 phase has a flag to branch on and
a directory to write to. Nothing in this phase spawns a PTY or changes any
behaviour: `backend` defaults to `"tmux"` and nothing reads it yet.

## Architecture references

Read before starting:

- `docs/dev/milestones/M20-shell-engine/README.md` § "Design decisions on
  record" — why the flag exists and what it gates.
- `docs/design/daemoneye-2.0.md` § 2.1 — the shell engine these paths serve
  (context only; do not implement any of it here).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All nine structural markers this phase adds are **absent today** — measured on
the current tree while drafting (2026-09-03), each returning `0`:

```
ExecutionConfig struct:  0      shells_dir ctor:         0
ShellsConfig struct:     0      shell_logs_dir ctor:     0
Config execution field:  0      lifecycle var/log/shells:0
startup validate:        0      inventory var/run/shells:0
config.toml [execution]: 0
```

`src/config/types.rs` (1156 lines) holds `Config` at line 6 with 18 `#[serde(default)]`
section fields, and `impl Default for Config` at line 45 repeating them.
`src/config/load.rs` (220 lines) holds the path constructors.
`src/config/lifecycle.rs` (590 lines) holds `POLICY_TABLE`.
`src/config/path_audit.rs` holds `INVENTORY` and the constructor test at line 554.
`src/daemon/mod.rs:498-501` calls `startup_config.limits.validate()` and
`startup_config.sandbox.validate()`.
`assets/etc/config.toml` (357 lines) documents every section as commented text;
its `[sandbox]` block is at lines 238-276.

The real binary creates the runtime tree on **every** subcommand —
`config::Config::ensure_dirs()` is called in `main()` at `src/main.rs:282`,
before command dispatch. Verified while drafting: `HOME=$T daemoneye reindex`
under a throwaway `$T` produced `var/log/{panes,pipe,sessions}` and an empty
`var/run`, with **no** `shells` directory under either.

## Spec

### Task 1 — Add `ExecutionConfig` to `src/config/types.rs`

Add the struct, its default fns, its `Default` impl, the derived predicate and
`validate()`. Follow the shape of `SandboxLimits` at `src/config/types.rs:401-442`
exactly — a `#[serde(default = "...")]` on every field, a free `default_*()` fn
per field, and a hand-written `Default` impl that calls those same fns:

```rust
pub struct SandboxLimits {
    /// Container memory ceiling, in Docker's `--memory` syntax. Default: "1g".
    #[serde(default = "default_sandbox_limits_memory")]
    pub memory: String,
    ...
}

fn default_sandbox_limits_memory() -> String {
    "1g".to_string()
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            memory: default_sandbox_limits_memory(),
            ...
        }
    }
}
```

`ExecutionConfig` has exactly one field:

- `backend: String`, `#[serde(default = "default_execution_backend")]`,
  default `"tmux"`.

Derive the branch decision, never compare the string at call sites — the same
shape as `SandboxConfig::runs_as_container_root()` at
`src/config/types.rs:580-589`:

```rust
impl SandboxConfig {
    pub fn runs_as_container_root(&self) -> bool {
        let uid_field = self.run_as.split(':').next().unwrap_or("").trim();
        uid_field.is_empty() || uid_field == "0" || uid_field == "root"
    }
```

Add:

- `pub fn uses_pty(&self) -> bool` — true **iff** the value, after
  `.trim()` and `.to_ascii_lowercase()`, is exactly `"pty"`. Everything else
  is false, which means the tmux backend. This is fail-safe by construction:
  a typo silently keeps today's behaviour rather than half-enabling a
  substrate that does not exist yet.
- `pub fn validate(&self)` — `log::warn!` when the normalised value is
  neither `"tmux"` nor `"pty"`, naming both supported values. Follow the
  wording style of `SandboxConfig::validate()` at `src/config/types.rs:592`.
  It must never panic and never return a value.

### Task 2 — Add `ShellsConfig` to `src/config/types.rs`

Same shape. Four fields, each with a `default_shells_*()` fn:

| Field | Type | Default | Doc comment must say |
|---|---|---|---|
| `max_per_owner` | `u32` | `5` | max concurrent shells one owner (a chat session, a ghost) may hold; `0` = unlimited |
| `exited_retention_secs` | `u64` | `300` | how long an exited shell stays listed before it is reaped |
| `log_retention_days` | `u32` | `7` | days to keep `var/log/shells/*.cast`; `0` = keep forever |
| `scrollback_lines` | `u32` | `5000` | rows of scrollback the screen model retains per shell |

`0 = keep forever` matches `RetentionConfig::pane_log_retention_days` at
`src/config/types.rs:158-166`; use the same phrasing so the two read alike.
`ShellsConfig` gets **no** `validate()` — nothing about these values can
silently defeat a safety property, which is the bar `validate()` exists for.

### Task 3 — Wire both sections into `Config`

In `src/config/types.rs`, add two fields to `pub struct Config` (line 6),
each `#[serde(default)]`:

```rust
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub shells: ShellsConfig,
```

and the two matching lines to `impl Default for Config` (line 45). Both
places, or `Config::default()` and the parsed form disagree.

### Task 4 — Add the two path constructors

In `src/config/load.rs`, next to `pane_logs_dir()` (line 35) and following its
exact form:

```rust
/// Directory where per-shell runtime state lives: `~/.daemoneye/var/run/shells/`.
pub fn shells_dir() -> PathBuf {
    var_run_dir().join("shells")
}

/// Directory where shell session recordings are stored: `~/.daemoneye/var/log/shells/`.
pub fn shell_logs_dir() -> PathBuf {
    var_log_dir().join("shells")
}
```

They are re-exported automatically — `src/config/mod.rs:13` is `pub use load::*;`.

Then create both eagerly in `Config::ensure_dirs()` (`src/config/seeds.rs:19`),
alongside the existing `create_dir_all(pane_logs_dir())?` at line 33:

```rust
        std::fs::create_dir_all(shells_dir())?;
        std::fs::create_dir_all(shell_logs_dir())?;
```

Creating them eagerly is what lets both lifecycle entries be `lazy: false`
(Task 5) and satisfies `every_eager_policy_entry_is_created_by_ensure_dirs`
(`src/config/lifecycle.rs:518`).

### Task 5 — Add two `POLICY_TABLE` entries

In `src/config/lifecycle.rs`, following the `var/log/panes` entry at line 91:

```rust
    LifecycleEntry {
        path: "var/log/panes",
        intent: LifecycleIntent::Sweep {
            default_retention_days: 7,
        },
        config_key: Some("retention.pane_log_retention_days"),
        implemented: ImplementationStatus::Implemented,
        note: "swept every 60th cleanup tick; operator-tunable via retention.pane_log_retention_days",
        lazy: false,
    },
```

Add:

- `var/log/shells` — `Sweep { default_retention_days: 7 }`,
  `config_key: Some("shells.log_retention_days")`,
  `implemented: ImplementationStatus::Pending { owned_by: "M20 phase-09" }`,
  `lazy: false`, note naming these as asciicast recordings whose sweep lands
  with the registry.
- `var/run/shells` — `LifecycleIntent::KeepForever`,
  `config_key: None`, `implemented: ImplementationStatus::Pending { owned_by: "M20 phase-06" }`,
  `lazy: false`. **The note must say why this entry exists even though the
  parent `var/run` entry already covers it:** `var/run` is
  `ClearAtStartup`, and shells are required to survive a daemon restart, so
  this subtree is explicitly exempt from that intent.

**Gotcha — only one of the two is forced by the gate.** `is_covered()`
(`src/config/lifecycle.rs:320-341`) treats a directory as covered when it is a
*subdirectory* of a table entry:

```rust
                dir == table_path
                    || dir.starts_with(&format!("{}/", table_path))
                    || table_path.starts_with(&format!("{}/", dir))
```

So `var/run/shells` is already covered by the `var/run` entry and Direction A
would pass without it; `var/log/shells` is **not** covered by anything (no
bare `var/log` entry exists — it is covered only as a *parent* of
`var/log/events` and friends) and Direction A fails without its own entry.
Add both anyway: the `var/run/shells` entry is a design statement, not a
test-satisfier.

Verified while drafting: nothing in the tree deletes `var/run` wholesale —
`grep -rn 'var_run_dir()' src/` returns only path constructors and writers, no
`remove_dir_all`. The `ClearAtStartup` intent on `var/run` describes sockets
and PID files being replaced individually, so no existing code will delete a
live shell socket. Do not add any sweep in this phase.

### Task 6 — Add two `INVENTORY` entries and register both constructors

In `src/config/path_audit.rs`, add to `INVENTORY` following the existing form:

```rust
    InventoryEntry {
        path: "etc",
        status: PathStatus::Current,
        source: "config::etc_dir()",
    },
```

— one for `var/run/shells` (`source: "config::shells_dir()"`) and one for
`var/log/shells` (`source: "config::shell_logs_dir()"`), both
`PathStatus::Current`.

**Then add both functions to the `constructors` vec in
`inventory_contains_all_config_constructors` (`src/config/path_audit.rs:554`).**

```rust
        let constructors: Vec<fn() -> PathBuf> = vec![
            crate::config::etc_dir,
            crate::config::var_run_dir,
            ...
            crate::config::runbooks_dir,
            Config::schedules_path,
        ];
```

**Gotcha:** that vec is hand-maintained. A new constructor that is not added
to it is never checked, so the test stays green while the guard is vacuous —
adding the `INVENTORY` entries alone would look like it passed. Both edits are
required, and Acceptance criteria check the vec by grep, not by test colour.

### Task 7 — Document both sections in `assets/etc/config.toml`

Add a commented block following the `[sandbox]` block's style
(`assets/etc/config.toml:238-276`) — a `# ── Section ──` rule, then every key
commented out with its default and a short comment. Place it **before** the
`# ── Daemon ──` rule.

The block must contain, as commented lines, a bare `# [execution]` heading, a
bare `# [shells]` heading, and every one of the five keys with its default
value. State plainly that `backend` defaults to `"tmux"`, that `"pty"` is the
2.0 substrate, and that nothing reads the value yet.

This file is compiled into the binary with `include_str!` at
`src/config/seeds.rs` and written on first run, so the E2E block checks the
*seeded copy* under a throwaway `HOME`, not this file.

### Task 8 — Call `validate()` at startup

In `src/daemon/mod.rs`, next to the two existing calls at lines 498-501:

```rust
    startup_config.limits.validate();
    ...
    startup_config.sandbox.validate();
```

add `startup_config.execution.validate();`. There is no `shells.validate()`.

### Task 9 — Write the tests named in § Test plan

All hermetic; no PTY, no daemon, no tmux. Tests that touch `HOME` must take
`crate::test_home_guard()` (`src/lib.rs:45`) — **not** the raw
`TEST_HOME_LOCK`; the accessor recovers from poisoning. Edition 2024, so
`std::env::set_var` needs `unsafe`. The RAII idiom is at
`src/daemon/context/recall.rs:246`.

### Task 10 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-01.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. Then run the PASTE MATCH self-check
in that same section and paste its verdict line into the same entry.

## Acceptance criteria

Each was run against the current tree while drafting and returns the "before"
value shown, so every one of them is failing now and must pass after.

- [ ] `grep -c "^pub struct ExecutionConfig" src/config/types.rs` → **1** (now `0`).
- [ ] `grep -c "^pub struct ShellsConfig" src/config/types.rs` → **1** (now `0`).
- [ ] `grep -c "pub execution: ExecutionConfig" src/config/types.rs` → **2** (now `0`) — the struct field and the `Default` impl.
- [ ] `grep -c "^pub fn shells_dir" src/config/load.rs` → **1** (now `0`).
- [ ] `grep -c "^pub fn shell_logs_dir" src/config/load.rs` → **1** (now `0`).
- [ ] `grep -c '"var/log/shells"' src/config/lifecycle.rs` → **1** (now `0`).
- [ ] `grep -c '"var/run/shells"' src/config/lifecycle.rs` → **1** (now `0`).
- [ ] `grep -c '"config::shells_dir()"' src/config/path_audit.rs` → **1** (now `0`) — the `INVENTORY` entry's `source` string.
- [ ] `grep -c '"config::shell_logs_dir()"' src/config/path_audit.rs` → **1** (now `0`).
- [ ] `grep -c "crate::config::shells_dir," src/config/path_audit.rs` → **1** (now `0`) — the entry in the `constructors` vec. Note the trailing comma: it distinguishes the vec line from the quoted `source` string above, which has none.
- [ ] `grep -c "crate::config::shell_logs_dir," src/config/path_audit.rs` → **1** (now `0`).
- [ ] `grep -c "startup_config.execution.validate()" src/daemon/mod.rs` → **1** (now `0`).
- [ ] `grep -c "^# \[execution\]$" assets/etc/config.toml` → **1** (now `0`).
- [ ] `grep -c "^# \[shells\]$" assets/etc/config.toml` → **1** (now `0`).
- [ ] After `cargo build`, running the real binary under a throwaway `HOME`
      creates both directories: `HOME=$T ./target/debug/daemoneye reindex`
      then `test -d $T/.daemoneye/var/run/shells && test -d $T/.daemoneye/var/log/shells`
      exits `0` (both absent today).
- [ ] The seeded `$T/.daemoneye/etc/config.toml` written by that same run
      contains both `# [execution]` and `# [shells]` headings.
- [ ] Every test named in § Test plan appears as a passing line in
      `cargo test --lib`.
- [ ] All four gates pass: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

Names are pinned; placement is not — put each beside the code it covers
(`src/config/types.rs` or `src/config/mod.rs`'s existing test module, matching
where `SandboxConfig`'s tests live). Every name below contains `execution_` or
`shells_config_` so the E2E block can find them by name.

- `execution_backend_defaults_to_tmux` — `ExecutionConfig::default().backend == "tmux"`,
  and `Config::default().execution.uses_pty()` is `false`.
- `execution_uses_pty_only_for_exact_pty_value` — **the negative cases are the
  point.** `uses_pty()` is `true` for `"pty"`, `"PTY"`, `" pty "`; and `false`
  for each of `"tmux"`, `""`, `"ptyx"`, `"p ty"`, `"docker"`, `"Pty!"`.
  Assert every listed value individually so a regression names which one broke.
- `execution_config_parses_from_toml` — a TOML fragment
  `[execution]\nbackend = "pty"\n` deserialised into `Config` yields
  `uses_pty() == true`; a `Config` parsed from an **empty** string yields
  `backend == "tmux"`.
- `execution_validate_does_not_panic` — calling `validate()` on backends
  `"tmux"`, `"pty"` and `"nonsense"` returns normally. Mirrors the existing
  `sbx.validate(); // must not panic` at `src/config/mod.rs:750`.
- `shells_config_defaults` — the four documented defaults, each asserted by value.
- `shells_config_parses_from_toml` — a fragment setting all four keys to
  non-default values round-trips; a fragment setting **only** `max_per_owner`
  leaves the other three at their defaults (this is what `#[serde(default)]`
  per field buys, and it breaks if a field is added without its default fn).
- `shell_paths_are_under_the_runtime_tree` — with a throwaway `HOME`,
  `shells_dir()` ends with `var/run/shells` and `shell_logs_dir()` ends with
  `var/log/shells`, and both start with `config_dir()`.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-01.txt`.

```sh
D=docs/dev/milestones/M20-shell-engine/phase-01-execution-config.md
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test .*(execution_|shells_config_|shell_paths_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. lifecycle + path-audit gates =="
cargo test --lib config::lifecycle 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib path_audit 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== E. real binary, throwaway HOME =="
T=$(mktemp -d)
HOME=$T ./target/debug/daemoneye reindex >/dev/null 2>&1; echo "reindex_exit=$?"
echo -n "var/run/shells created:  "; test -d "$T/.daemoneye/var/run/shells" && echo YES || echo NO
echo -n "var/log/shells created:  "; test -d "$T/.daemoneye/var/log/shells" && echo YES || echo NO
echo -n "seeded [execution] block: "; grep -c '^# \[execution\]$' "$T/.daemoneye/etc/config.toml"
echo -n "seeded [shells] block:    "; grep -c '^# \[shells\]$' "$T/.daemoneye/etc/config.toml"
rm -rf "$T"
echo "== F. structural greps (each must print the stated number) =="
echo -n "ExecutionConfig struct   (1): "; grep -c "^pub struct ExecutionConfig" src/config/types.rs
echo -n "ShellsConfig struct      (1): "; grep -c "^pub struct ShellsConfig" src/config/types.rs
echo -n "Config execution field   (2): "; grep -c "pub execution: ExecutionConfig" src/config/types.rs
echo -n "Config shells field      (2): "; grep -c "pub shells: ShellsConfig" src/config/types.rs
echo -n "shells_dir ctor          (1): "; grep -c "^pub fn shells_dir" src/config/load.rs
echo -n "shell_logs_dir ctor      (1): "; grep -c "^pub fn shell_logs_dir" src/config/load.rs
echo -n "lifecycle var/log/shells (1): "; grep -c '"var/log/shells"' src/config/lifecycle.rs
echo -n "lifecycle var/run/shells (1): "; grep -c '"var/run/shells"' src/config/lifecycle.rs
echo -n "inventory shells_dir src (1): "; grep -c '"config::shells_dir()"' src/config/path_audit.rs
echo -n "inventory shell_logs src (1): "; grep -c '"config::shell_logs_dir()"' src/config/path_audit.rs
echo -n "ctor vec shells_dir      (1): "; grep -c "crate::config::shells_dir," src/config/path_audit.rs
echo -n "ctor vec shell_logs_dir  (1): "; grep -c "crate::config::shell_logs_dir," src/config/path_audit.rs
echo -n "startup validate         (1): "; grep -c "startup_config.execution.validate()" src/daemon/mod.rs
} > /tmp/e2e-01.txt 2>&1
cat /tmp/e2e-01.txt
```

Paste the contents of `/tmp/e2e-01.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-01-execution-config.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01.txt
diff /tmp/pasted-01.txt /tmp/e2e-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Section B is the one that can lie.** Measured on the current tree while
drafting: `cargo test --lib execution` prints
`test result: ok. 0 passed; 0 failed; ... 1533 filtered out` and reports
`cargo_exit=0` with **no** test matching. A zero exit proves nothing here —
the pass condition is that every test name from § Test plan appears as its own
line in section B.

The self-check was validated both ways while drafting, against a copy of this
doc: a byte-exact paste printed `PASTE MATCH`, and the same paste with one
line retyped printed `PASTE MISMATCH` and named the divergent line.

**Section E is the real-artifact check.** Both directories are absent today
under a fresh `HOME` (measured), so `NO`/`NO` is the current output and
`YES`/`YES` is the phase's proof.

**What this phase deliberately cannot verify end to end, and why.** No
daemon-free subcommand loads `Config` — measured while drafting: a deliberate
type error (`max_tool_calls_per_turn = "not-an-int"`) written into a throwaway
`config.toml` left `daemoneye costs` exiting `0` with normal output, because
`costs` reads the event log directly and never parses the config. So the
*parsing* of `[execution]` / `[shells]` by the running binary is covered by
the unit tests in section B, and its real-binary verification arrives when the
daemon first branches on `uses_pty()` (phase-07) and at the M20 live sweep
(phase-09). Do **not** spend turns hunting for a CLI that prints the resolved
config; there isn't one, and adding one is out of scope.

## Authorizations

- Edit `src/config/types.rs`, `src/config/load.rs`, `src/config/seeds.rs`,
  `src/config/lifecycle.rs`, `src/config/path_audit.rs`, `src/config/mod.rs`
  (tests only), `src/daemon/mod.rs` (the one `validate()` line), and
  `assets/etc/config.toml`.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **No new dependencies.** `portable-pty` and `vt100` arrive in phase-02.
- May **not** touch `docs/architecture.md`, `CLAUDE.md` or `README.md` —
  documentation updates for M20 land in phase-09.

## Out of scope

- **Anything that reads `uses_pty()`.** The predicate ships with tests and no
  production caller. That is intentional; phase-07 is its first consumer.
  Do not add a `#[allow(dead_code)]` — a `pub fn` on a `pub struct` in a
  library crate is not dead code, and the gates confirm it.
- **Any PTY, `portable-pty`, `vt100`, spawning, or shell-host work** (phases
  02-06).
- **Any sweep, GC or retention implementation** for the new directories. The
  lifecycle entries are marked `Pending { owned_by: ... }` precisely because
  the implementation is a later phase. Writing a sweeper here would make the
  entry's own status field a lie.
- **A `daemoneye config show` subcommand**, or any other new CLI surface.
- **Touching `RetentionConfig`.** `shells.log_retention_days` lives in
  `[shells]` with the rest of the shell settings, not beside
  `retention.pane_log_retention_days`; do not move or alias it.
- **Renaming or removing anything tmux.** The tmux backend stays byte-for-byte
  as it is until M26.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
