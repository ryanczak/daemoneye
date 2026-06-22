# Phase 06: Namespace Access Control (lock the memory/search ACL as correct-by-design)

**Milestone:** M1 — Agent Tooling Improvements
**Status:** done
**Depends on:** none (the ACL this phase locks already exists; this phase only adds tests)
**Estimated diff:** ~120 lines (tests only)
**Tags:** language=rust, kind=test, size=s

> **Scope note (architect, 2026-06-22).** The M1 review inventory flagged this as a
> live security bug: "`read_memory` / `search_repository` trust the **caller-supplied**
> `namespaces` slice → an agent can read another agent's namespace." Re-verification
> against the current tree shows that bug is **not reachable**: none of the read tools
> expose a namespace parameter, and the slice is built **server-side** by
> `build_memory_namespaces()` from the agent's own config. The ACL is enforced *by
> construction*. The principal engineer therefore re-scoped this phase to
> **lock that property with regression tests** — no production behavior change. The
> README finding inventory has been updated to match.

## Goal

Pin the memory/search namespace ACL — already enforced server-side — with
regression tests that fail if the property ever regresses. Three properties are
locked:

1. **The model cannot inject a namespace.** `read_memory`, `list_memories`, and
   `search_repository` expose no `namespace` tool parameter — so the namespace set is
   never caller-supplied from the LLM.
2. **The storage layer reads exactly the namespaces it is handed — no more.** A memory
   written only in namespace `victim` is invisible to a scan over `["analyst","global"]`
   and visible only when `victim` is explicitly in the slice.
3. **`build_memory_namespaces` scopes correctly.** A non-ghost session resolves to
   exactly `["global"]`; a ghost session resolves to its agent's own namespace + its
   declared `read_namespaces` + `"global"`, and **never** to an unrelated agent's
   private namespace.

This is a test-only phase. **Do not change any production code, tool schema, IPC type,
`PendingCall` variant, or function signature.** If you believe a production change is
required to make a test pass, that is a blocker — stop and report it.

## Architecture references

Read before starting:

- `docs/architecture.md#24-remote-host-execution-model` — the daemon-host-storage
  model; managed-artifact tools (memory included) are daemon-host-scoped.
- `docs/dev/milestones/M1-agent-tooling/README.md` — § "Confirmed findings inventory" →
  **Phase 06** (updated to reflect the re-scope).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom (note §3 test rules: hermetic,
   deterministic, no real network/home writes — use the temp-HOME harness below).
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Re-verify the cited line numbers in `src/daemon/executor/mod.rs`,
   `src/daemon/executor/knowledge.rs`, `src/ai/tools.rs`, `src/memory_tests.rs`,
   and `src/daemon/session.rs` before writing — the tree moves; the numbers below
   were captured at draft time.
5. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The ACL these tests lock is implemented here (do **not** modify it):

**`build_memory_namespaces` — `src/daemon/executor/mod.rs:78`** (the server-side
allowlist builder):

```rust
pub fn build_memory_namespaces(
    session_id: Option<&str>,
    sessions: &SessionStore,
    is_ghost: bool,
) -> Vec<String> {
    if !is_ghost {
        return vec!["global".to_string()];
    }
    let mut namespaces: Vec<String> = Vec::new();
    if let Some(sid) = session_id
        && let Ok(store) = sessions.lock()
        && let Some(entry) = store.get(sid)
        && let Some(ref gc) = entry.ghost_config
        && let Some(ref agent_name) = gc.agent
        && let Ok(agent) = crate::agents::load_agent(agent_name)
    {
        namespaces.push(agent.memory_namespace.clone());
        for extra in &agent.read_namespaces {
            namespaces.push(extra.clone());
        }
    }
    if !namespaces.iter().any(|s| s == "global") {
        namespaces.push("global".to_string());
    }
    namespaces
}
```

The read sites consume that slice and never widen it: `read_memory`
(`knowledge.rs:521`) iterates `namespaces`; `list_memories` (`knowledge.rs:540`) and
`search_repository` (`knowledge.rs:604`) forward it to
`memory::list_memories_with_tags` / `search::search_repository_with_namespaces`. The
single dispatch site builds the slice via `build_memory_namespaces` at
`executor/mod.rs:201` and passes it everywhere.

`SessionStore` is `Arc<Mutex<HashMap<String, SessionEntry>>>` (`session.rs:101`).
`AgentConfig.read_namespaces: Vec<String>` (`agents/mod.rs:48`) is loaded from each
agent's `config.toml`; `build_memory_namespaces` is the only consumer that grants it.

`TOOLS: &[ToolDef]` (`ai/tools.rs:47`); `ToolDef { name, params: &[ParamDef], .. }`;
`ParamDef { name, .. }`. `ai/tools.rs` already has a `#[cfg(test)] mod tests` at
line ~1553 — extend it.

## Spec

Three test locations. Pin the test **names** and what each asserts; the assertion
shape and any local helper are yours. **No production file changes.**

### 1. Tool-def negative property — in `src/ai/tools.rs` (extend the existing `mod tests`)

Add **`read_tools_expose_no_namespace_param`**. For each tool name in
`["read_memory", "list_memories", "search_repository"]`, look it up in `TOOLS` (assert
it is present) and assert **none** of its `params` has `name == "namespace"` (nor
`"namespaces"`). This is the direct lock on "the model cannot supply a namespace into a
read." Worked shape:

```rust
#[test]
fn read_tools_expose_no_namespace_param() {
    for tool in ["read_memory", "list_memories", "search_repository"] {
        let def = TOOLS
            .iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("tool {tool} missing from TOOLS"));
        for p in def.params {
            assert!(
                p.name != "namespace" && p.name != "namespaces",
                "{tool} must not expose a namespace param (got '{}') — the namespace \
                 set is built server-side, never caller-supplied",
                p.name
            );
        }
    }
}
```

(`unwrap_or_else(|| panic!(...))` and `assert!` are test code — STANDARDS §2 exempts
tests from the no-`panic!` rule. Do **not** add `.unwrap()`/`panic!` to production
code.)

### 2. Storage-layer confinement — in `src/memory_tests.rs` (extend; reuse the file's harness)

`src/memory_tests.rs` already has `temp_home()` / `with_home()` (lines 25–44) and
`use super::*;` so `add_memory`, `read_memory`, `list_memories_with_tags`, and
`MemoryCategory` are in scope. The existing test near line 500 (`read_namespaces`
sharing) is the positive control; add the **negative** companion.

Add **`memory_scan_is_confined_to_supplied_namespaces`**:

- Inside `with_home(&temp_home(), || { … })`:
  - `add_memory("secret", "v", MemoryCategory::Knowledge, "victim").unwrap();`
  - **Negative (read):** `read_memory("secret", MemoryCategory::Knowledge, "analyst").is_err()`
    — the key exists only in `victim`, so an `analyst`-scoped read must not find it.
  - **Negative (scan):** `list_memories_with_tags(Some(MemoryCategory::Knowledge), &["analyst", "global"])`
    must contain **no** entry with `key == "secret"`.
  - **Positive control:** `list_memories_with_tags(Some(MemoryCategory::Knowledge), &["victim"])`
    **must** contain an entry with `key == "secret"` (proves the memory was written and
    the only thing keeping it out of the analyst scan is the namespace slice — i.e. the
    slice *is* the boundary).

This is the property that makes server-side slice construction sufficient: the storage
layer never reaches outside the namespaces it is given.

### 3. `build_memory_namespaces` scoping — new `#[cfg(test)] mod tests` in `src/daemon/executor/mod.rs`

`executor/mod.rs` has no test module yet — add one at the end of the file:
`#[cfg(test)] mod tests { use super::*; … }`. `build_memory_namespaces`,
`SessionStore`, and `SessionEntry` are reachable via `super::*` /
`crate::daemon::session::SessionEntry`.

Add **`namespaces_non_ghost_is_global_only`** (no HOME needed — non-ghost early-returns
before any disk access):

```rust
#[test]
fn namespaces_non_ghost_is_global_only() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    let store: SessionStore = Arc::new(Mutex::new(HashMap::new()));
    let ns = build_memory_namespaces(Some("any-sid"), &store, false);
    assert_eq!(ns, vec!["global".to_string()]);
}
```

Add **`namespaces_ghost_excludes_foreign_namespace`** — the direct refutation of the
stale finding. It needs (a) an agent config on disk under a temp `HOME`, and (b) a ghost
`SessionEntry` in the store whose `ghost_config.agent` names that agent. Pre-injected
worked example (adapt field values; keep the structure):

```rust
#[test]
fn namespaces_ghost_excludes_foreign_namespace() {
    use crate::agents::AgentConfig;
    use crate::daemon::session::SessionEntry;
    use crate::util::UnpoisonExt;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // Temp-HOME guard (same shape as src/memory_tests.rs::with_home). HOME mutation
    // must hold TEST_HOME_LOCK — see src/main.rs TEST_HOME_LOCK doc.
    let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
    let tmp = std::env::temp_dir().join(format!("de_ns_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let old = std::env::var("HOME").ok();
    unsafe { std::env::set_var("HOME", &tmp); }

    // Agent "analyst": own namespace "analyst", grants read of "shared", NOT "victim".
    crate::agents::save_agent(&AgentConfig {
        name: "analyst".to_string(),
        description: String::new(),
        prompt: String::new(),
        model: None,
        memory_namespace: "analyst".to_string(),
        max_turns: None,
        auto_approve_read_only: false,
        auto_approve_scripts: Vec::new(),
        read_namespaces: vec!["shared".to_string()],
        tools: None,
    })
    .unwrap();

    // Ghost session entry referencing that agent. Full SessionEntry literal copied
    // from session.rs (auto_name_suggested_starts_false test), flipping is_ghost +
    // ghost_config. Re-verify the field list against src/daemon/session.rs at draft —
    // SessionEntry has no Default derive.
    let entry = SessionEntry {
        messages: vec![],
        last_accessed: std::time::Instant::now(),
        chat_pane: None,
        default_target_pane: None,
        bg_windows: vec![],
        last_prompt_tokens: 0,
        tmux_session: "test".to_string(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        pipe_source_pane: None,
        is_ghost: true,
        ghost_config: Some(crate::ipc::GhostConfig {
            agent: Some("analyst".to_string()),
            ..Default::default()
        }),
        ghost_bg_prefix: "",
        started_at: chrono::Utc::now(),
        turn_count: 0,
        tool_calls_this_session: 0,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: None,
        dirty: false,
        artifacts_created: vec![],
        auto_name_suggested: false,
        ghost_task_message: None,
        cost_usd: 0.0,
        cost_by_agent: HashMap::new(),
        has_untracked_cost: false,
    };
    let store: SessionStore = Arc::new(Mutex::new(HashMap::new()));
    store.lock().unwrap_or_log().insert("gsid".to_string(), entry);

    let ns = build_memory_namespaces(Some("gsid"), &store, true);

    // restore HOME before asserting (so a panic still leaves HOME clean is nice-to-have;
    // restoring first keeps the lock-held window correct).
    match old {
        Some(v) => unsafe { std::env::set_var("HOME", v); },
        None => unsafe { std::env::remove_var("HOME"); },
    }
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(ns.iter().any(|n| n == "analyst"), "own namespace present: {ns:?}");
    assert!(ns.iter().any(|n| n == "shared"), "declared read_namespace present: {ns:?}");
    assert!(ns.iter().any(|n| n == "global"), "global fallback present: {ns:?}");
    assert!(
        !ns.iter().any(|n| n == "victim"),
        "agent must NOT reach an unrelated namespace it was never granted: {ns:?}"
    );
}
```

If the full `SessionEntry` literal has drifted (a field added/removed since draft),
adapt it to the **current** struct — do not invent fields. If you cannot construct a
`SessionEntry` from outside its module (e.g. a private field blocks it), that is a
blocker: stop and report it rather than changing `SessionEntry`'s visibility.

The two ghost assertions (own/shared/global present; victim absent) may be split into a
second test if you prefer; keep `namespaces_ghost_excludes_foreign_namespace` as the
name carrying the **victim-absent** assertion (the security property).

## Acceptance criteria

- [ ] `read_tools_expose_no_namespace_param` passes and would fail if any of the three
      read tools gained a `namespace`/`namespaces` param.
- [ ] `memory_scan_is_confined_to_supplied_namespaces` passes: an `analyst`-scoped read
      of a `victim`-only key errs; an `["analyst","global"]` scan omits it; a `["victim"]`
      scan includes it.
- [ ] `namespaces_non_ghost_is_global_only` passes: non-ghost → exactly `["global"]`.
- [ ] `namespaces_ghost_excludes_foreign_namespace` passes: ghost for agent `analyst`
      (own `analyst`, read `shared`) yields a list containing `analyst`, `shared`,
      `global` and **not** `victim`.
- [ ] **No production code changed.** `git diff --stat` shows only test additions in
      `src/ai/tools.rs`, `src/memory_tests.rs`, `src/daemon/executor/mod.rs` (a new
      `#[cfg(test)] mod tests`). No change to any non-test function, schema, or signature.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Already enumerated in the Spec. Summary of names + asserted behavior:

- `read_tools_expose_no_namespace_param` in `src/ai/tools.rs` — asserts the three read
  tools carry no namespace param.
- `memory_scan_is_confined_to_supplied_namespaces` in `src/memory_tests.rs` — asserts
  the storage layer reads only the supplied namespaces (negative + positive control).
- `namespaces_non_ghost_is_global_only` in `src/daemon/executor/mod.rs` — asserts the
  non-ghost allowlist is exactly `["global"]`.
- `namespaces_ghost_excludes_foreign_namespace` in `src/daemon/executor/mod.rs` —
  asserts the ghost allowlist includes own + declared-read + global and excludes a
  foreign namespace.

## End-to-end verification

This phase ships **no runtime-loadable artifact** — it adds regression tests over
existing behavior, changing no production code path. Per WORKFLOW.md, write in the
completion log:

> Not applicable — phase ships no runtime-loadable artifact (test-only; locks an
> already-shipped ACL).

Still quote the passing output of `cargo test namespaces_`, `cargo test
memory_scan_is_confined`, and `cargo test read_tools_expose_no_namespace_param` in the
completion Update Log.

## Authorizations

- [ ] May add dependencies: **no.**
- [ ] May touch `docs/architecture.md`: **no.**
- [ ] May change production code: **no** — test additions only.

None beyond adding tests to `src/ai/tools.rs`, `src/memory_tests.rs`, and
`src/daemon/executor/mod.rs`.

## Out of scope

- **Any production behavior change.** No new `AgentConfig` method, no intersection guard
  at the read sites, no change to `build_memory_namespaces`. The principal engineer
  explicitly chose "lock with tests, no code change" over the defense-in-depth option;
  do not implement the latter here.
- **Validating `read_namespaces` grants** (e.g. requiring the target agent to opt in).
  `read_namespaces` is operator-granted via the agent's `config.toml` and is treated as
  authoritative; revisiting that is a design decision, not this phase.
- **The write path** (`add_memory` namespace selection). Writes already go to
  `namespaces.first()` (the agent's own namespace); not in scope to re-test here beyond
  the existing coverage.
- **Search index (FTS5) internals.** `search_repository` forwards the same slice; the
  storage-confinement test covers the property at the namespace boundary.
- **Non-ghost agent namespaces.** Interactive sessions are `["global"]` by design; do
  not add a mechanism to give them agent namespaces.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 18:16 (started)

**Executor:** Claude (opus-4-5-20251101)

**Summary:** Implementing 4 regression tests across 3 files to lock the memory/search namespace ACL:
- `read_tools_expose_no_namespace_param` in `src/ai/tools.rs`
- `memory_scan_is_confined_to_supplied_namespaces` in `src/memory_tests.rs`
- `namespaces_non_ghost_is_global_only` and `namespaces_ghost_excludes_foreign_namespace` in `src/daemon/executor/mod.rs`

### Update — 2026-06-22 18:16 (complete)

**Summary:** All 4 regression tests implemented and passing. No production code changed.

**Commands:**

```
cargo fmt --all
(no output — clean)

cargo build 2>&1 | tail -20
   Compiling daemoneye v0.1.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.4s

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
    Checking daemoneye v0.1.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.3s

cargo test 2>&1 | tail -30
test result: ok. 740 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 27 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

**Verification summary:** `cargo fmt --all` clean, `cargo build` zero warnings, `cargo clippy` clean, `cargo test` all 795 tests pass.

**Per-test verification:**
```
cargo test read_tools_expose 2>&1 | tail -10
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 739 filtered out

cargo test memory_scan_is_confined 2>&1 | tail -10
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 739 filtered out

cargo test namespaces_ 2>&1 | tail -10
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 738 filtered out
```

**End-to-end verification:**
Not applicable — phase ships no runtime-loadable artifact (test-only; locks an already-shipped ACL).

**Files changed:**
- `src/ai/tools.rs` — added `read_tools_expose_no_namespace_param` test to existing `mod tests`
- `src/memory_tests.rs` — added `memory_scan_is_confined_to_supplied_namespaces` test
- `src/daemon/executor/mod.rs` — added `#[cfg(test)] mod tests` with `namespaces_non_ghost_is_global_only` and `namespaces_ghost_excludes_foreign_namespace`

**New tests:**
- `read_tools_expose_no_namespace_param` in `src/ai/tools.rs`
- `memory_scan_is_confined_to_supplied_namespaces` in `src/memory_tests.rs`
- `namespaces_non_ghost_is_global_only` in `src/daemon/executor/mod.rs`
- `namespaces_ghost_excludes_foreign_namespace` in `src/daemon/executor/mod.rs`

**Commits:**
- (pending) — `test: lock memory/search namespace ACL with regression tests`

**Notes for review:**
- `MemoryInfo` does not derive `Debug`, so assertion messages in `memory_scan_is_confined_to_supplied_namespaces` omit `{results:?}` formatting to avoid requiring a production-code change. The assertions themselves (`.any(|e| e.key == "secret")`) are unaffected.
- The `namespaces_ghost_excludes_foreign_namespace` test constructs a full `SessionEntry` literal — if fields drift in a future phase, this test will fail to compile, serving as the intended regression signal.

### Review verdict — 2026-06-22

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Claude (opus-4-5-20251101) via rexyMCP local executor
- **Scope deviations:** none — test-only as scoped; `git diff` confirms all source
  additions are inside `#[cfg(test)]` scopes (`src/ai/tools.rs`, `src/memory_tests.rs`,
  new `mod tests` in `src/daemon/executor/mod.rs`); no production function, schema,
  IPC type, or signature touched.
- **Independent re-run:** `cargo fmt --all --check`, `cargo build` (zero warnings),
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (740 + 27 pass, 1 ignored) all green on a fresh run.
- **Test-reality spot-check:** mutating `build_memory_namespaces` to push a foreign
  `"victim"` namespace makes `namespaces_ghost_excludes_foreign_namespace` FAIL at the
  victim-absent assertion (mod.rs:1013) — the security property is genuinely pinned,
  not a vacuous pass. Reverted; tree clean.
- **Calibration:** none
