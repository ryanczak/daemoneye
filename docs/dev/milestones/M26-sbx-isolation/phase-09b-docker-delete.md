# Phase 09b: docker-delete — remove the docker runtime's code

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-09a
**Estimated diff:** ~4,700 lines deleted, ~10 added
**Tags:** language=rust, kind=refactor, size=m

## Goal

Delete the docker runtime. Since phase 09a an enabled sandbox always means
sbx, and the tmux window path refuses while the sandbox is enabled, so every
docker code path is unreachable. This phase removes:
- `src/daemon/executor/container.rs` (4,237 lines, 128 tests);
- the `containers/` directory (`Dockerfile`, `proxy/Dockerfile`,
  `proxy/tinyproxy.conf`);
- `daemoneye sandbox build` and its module `src/cli/commands/sandbox.rs`;
- every remaining caller of the above.

This is the exit criterion "The docker runtime is gone". **Nothing is
added**: this phase writes no new feature and no new test.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § Exit criteria, "The
  docker runtime is gone".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Measured by the architect, 2026-10-08

The architect trial-deleted `container.rs` and `cli/commands/sandbox.rs`,
removed `pub mod container;`, and ran `cargo check --all-targets`, then
restored the tree. It reported exactly these 22 errors:

| File | Lines | What is there |
|---|---|---|
| `src/cli/commands/mod.rs` | 15 | `mod sandbox;`, and line 28 `pub use sandbox::run_sandbox_build;` |
| `src/daemon/mod.rs` | 528 | the startup `container::sweep_sandbox_leftovers(&startup_config.sandbox);` and its two-line comment |
| `src/daemon/ghost.rs` | 604 | the `teardown_ghost_containers` `spawn_blocking` block (quoted below) |
| `src/daemon/background/run.rs` | 35, 90, 131, 225–295, 430–432, 496–499, 636–647 | the docker blocks in `run_background_in_window` (below) |

`src/main.rs` also has `SandboxCommands::Build` (its doc comment names
`containers/Dockerfile`) and the `SandboxCommands::Build => {
cli::run_sandbox_build(); }` arm. Keep `SandboxCommands::Status`.

### The `ghost.rs` block to delete (whole statement, about lines 602–606)

```rust
    let (sbx, sid) = (config.sandbox.clone(), session_id.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::teardown_ghost_containers(&sbx, &sid)
    })
    .await;
```

### `run_background_in_window` (`src/daemon/background/run.rs`)

Since 09a, the function opens with:

```rust
    let config = crate::config::Config::load().unwrap_or_default();
    if config.sandbox.enabled {
        log::warn!("refusing sandboxed background command: {SANDBOX_TMUX_REFUSAL}");
        return SANDBOX_TMUX_REFUSAL.to_string();
    }
```

So everything below runs with the sandbox **disabled**. Delete each of these:

1. **The `log_proxy_audit` helper** at the top of the file. It is the only
   user of `"proxy_request"`.
2. **`profile_name` and `proxy_rules`:** the `let profile_name = …` and
   `let proxy_rules = …proxy_rules_for_profile(…)` bindings, with their
   comment.
3. **`job_id`:** `let job_id = crate::daemon::executor::container::job_id_for(&pane_id, unix_ts);`.
4. **The three `let cmd` / `let network` stages:**
   - the script-staging `let staged_cmd; let cmd: &str = if config.sandbox.enabled && … else { cmd };`;
   - the egress-proxy `let mut proxy_started = false; let network = if config.sandbox.enabled && … else { "none".to_string() };`;
   - the `let sandboxed_cmd; let cmd: &str = { if config.sandbox.enabled { … } else { cmd } };`.
5. **The `container_run` event:** the `if let Some(payload) = …container_run_event(…) { log_event("container_run", payload); }` statement.
6. **The two docker teardown blocks:** the inline-completion
   `if config.sandbox.enabled { … remove_stage_volume … remove_proxy … }`,
   and the background-monitor `if sandbox_bg.enabled { … }`.
7. **`sandbox_bg`:** `let sandbox_bg = config.sandbox.clone();` and any
   `job_id_bg` clone that only fed the deleted block.

The `SUDO_PROMPT` sentinel stage just above the staging block stays. So does
everything that builds `wrapped` from `cmd`, `exit_var` and `notify`.

## Spec

### Task 1 — Delete the files

```sh
git rm -q src/daemon/executor/container.rs src/cli/commands/sandbox.rs
git rm -q -r containers
```

In `src/daemon/executor/mod.rs`, delete the line `pub mod container;`.

### Task 2 — Remove every caller

Work through § Current state, then let the compiler confirm.
`cargo build` must report **no** `cannot find \`container\`` error. Then:
- **CLI:** `cli/commands/mod.rs` loses `mod sandbox;` and its `pub use`.
  `main.rs` loses `SandboxCommands::Build` and its arm.
- **`ghost.rs` and `daemon/mod.rs`:** delete the quoted statements.
- **`run.rs`:** delete items 1–7.

### Task 3 — Clean up what the deletion leaves dead

After Task 2, run `cargo clippy --all-targets --all-features -- -D warnings`.
For every `unused` / `never used` / `dead_code` finding, **delete the dead
item**. Likely candidates are an import, `config` fields read only by the
deleted code, or a parameter. **Never** add `#[allow(…)]`: phase 09a bounced
on exactly that.

One exception: **keep** the docker-only `SandboxConfig` fields (`image`,
`proxy_image`, `workdir`, `run_as`, `docker_host`, `limits.pids`,
`limits.scratch`, `ghost_defaults.mount_scripts`) and
`runs_as_container_root()`. `SandboxConfig::warnings()` reads the fields so it
can warn that they no longer do anything. If clippy calls one of them dead,
stop and record it in the Update Log; do not allow it.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `test -e src/daemon/executor/container.rs || test -e src/cli/commands/sandbox.rs || test -e containers; echo $?`
      prints `1`: none of the three exists.
- [ ] `grep -rn 'executor::container' src/ tests/` prints nothing.
- [ ] `grep -rn '"--host"' src/ --include=*.rs` prints nothing. The
      `docker --host` argv is gone.
- [ ] `cargo run -q -- sandbox --help` lists `status` and does **not** list
      `build`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0,
      **and** `git diff 7a54d97d -- src/ | grep -c '^+.*allow('` prints `0`
      (`7a54d97d` is the tree this phase starts from).
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports **exactly** `passed=2297 failed=0`. Today it is
      2422. The 124 passing `container.rs` tests and the 1 `sandbox.rs` test
      go with their files. Any other number means a test elsewhere was
      deleted or added.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

No new tests. The proof is:
- the exact count drop above (2422 → 2297);
- every remaining sandbox test still passing, namely `executor::sbx`,
  `sbx_lifecycle`, `ghost_sandbox`, `chat_sandbox`, `bg_shell`, `config::`
  and `tmux_window_refusal_names_the_fix`.

**Reviewer check:** `git show --stat` lists only deletions, plus the small
edits in `main.rs`, `cli/commands/mod.rs`, `executor/mod.rs`, `ghost.rs`,
`daemon/mod.rs` and `run.rs`, and any Task 3 cleanup.

## End-to-end verification

```sh
test -e src/daemon/executor/container.rs || test -e src/cli/commands/sandbox.rs || test -e containers; echo "exists_any=$?"
grep -rn 'executor::container' src/ tests/ ; echo "grep_container_done"
grep -rn '"--host"' src/ --include=*.rs ; echo "grep_host_done"
cargo run -q -- sandbox --help 2>&1 | grep -E '^\s+(status|build)'
for m in executor::sbx executor::ghost_sandbox executor::chat_sandbox executor::bg_shell config:: tmux_window_refusal_names_the_fix; do cargo test --lib "$m" 2>&1 | grep -E '^test result' | sed "s/^/$m: /"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff 7a54d97d -- src/ | grep -c '^+.*allow('
git diff --stat 7a54d97d -- src/ containers/ | tail -1
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.
`exists_any=1` is the pass value.

## Authorizations

- [x] May delete the three paths in Task 1 and every caller listed in
      § Current state.
- [x] May delete items that clippy reports dead **as a direct result** of
      the deletion (Task 3), except the listed `SandboxConfig` fields and
      `runs_as_container_root()`.
- [x] Deleting the 125 tests inside the deleted files is expected. Deleting
      or editing any **other** test is not authorized.

## Out of scope

- Docs (README, CLAUDE.md, `security.md`, `ghost-shell-guide`,
  `architecture.md`): row 09c. If `cargo test`'s doc checks
  (`tests/doc_truth.rs`) fail because a doc names `sandbox build`, stop and
  record it. Do not edit docs here.
- Renaming `ContainerStatusReport` / `Request::ContainerStatus`. They are
  wire names, so they stay.
- Removing the docker-only `SandboxConfig` fields: they back the warnings.

## Update Log

<!-- entries appended below this line -->
