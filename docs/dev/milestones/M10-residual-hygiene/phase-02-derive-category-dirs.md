# Phase 02: Derive memory category dirs, and drop the last real-clock sleep

**Milestone:** M10 — Residual Hygiene
**Status:** done
**Depends on:** phase-01 (`done`)
**Estimated diff:** ~110 lines across four source files, most of it two new tests.

## Goal

Two unrelated carried items, both small and both mechanical:

1. **`src/ai/mod.rs:364`** holds a test connection open with a 30 s real-clock
   `tokio::time::sleep`. Replace it with `std::future::pending()`, which expresses
   "never resolve" without a clock. This is the last real-clock sleep in the suite.
2. **Three places hardcode the memory category → directory mapping** instead of
   deriving it from `MemoryCategory`. Derive all three.

**Both label formats are currently untested, and that is the whole risk in this
phase.** See "The part that is not mechanical" below — it is why two new tests
are mandatory rather than optional.

## Current state

Measured against the tree on 2026-08-02. Every claim here was executed.

`src/memory.rs:8` — the enum, with **no** way to enumerate its variants:

```rust
#[derive(Clone, Copy)]
pub enum MemoryCategory { Session, Knowledge, Incident }

impl MemoryCategory {
    /// Filesystem directory name under ~/.daemoneye/memory/.
    pub fn dir_name(&self) -> &'static str {
        match self {
            MemoryCategory::Session => "session",
            MemoryCategory::Knowledge => "knowledge",
            MemoryCategory::Incident => "incidents",     // <-- PLURAL
        }
    }

    /// The canonical name used in tool arguments and displayed to the AI.
    /// Always singular to match the tool description ('incident', not 'incidents').
    pub fn canonical_name(&self) -> &'static str {
        match self {
            MemoryCategory::Session => "session",
            MemoryCategory::Knowledge => "knowledge",
            MemoryCategory::Incident => "incident",      // <-- SINGULAR
        }
    }
}
```

**`dir_name()` and `canonical_name()` differ for exactly one variant.** That
single difference is what both new tests exist to protect.

The three hardcoded copies:

| File | What it hardcodes |
|---|---|
| `src/daemon/context/epochs.rs:619` | a `(canonical, dir)` tuple table |
| `src/search.rs:56-63` | three `if dir.exists()` blocks, path **and** label |
| `src/memory.rs:19,21,39` | the accessors themselves — **correct, leave alone** |

`epochs.rs:619`, inside `scan_artifacts_span()` (starts at `:590`):

```rust
    // Memories (three category subdirs) — format as "memory:{key} [{category}]"
    for (category, dir_name) in &[
        ("session", "session"),
        ("knowledge", "knowledge"),
        ("incident", "incidents"),
    ] {
        let dir = config::config_dir().join("memory").join(dir_name);
        scan_dir_in_range(
            &dir, since_systime, until_systime, &["md"], &mut out,
            |name| format!("memory:{} [{}]", name, category),
        );
    }
```

`search.rs:56`, inside the `"memory" | "all"` arm:

```rust
                if mem_base.join("session").exists() {
                    dirs.push((mem_base.join("session"), "memory/session".to_string()));
                }
                if mem_base.join("knowledge").exists() {
                    dirs.push((mem_base.join("knowledge"), "memory/knowledge".to_string()));
                }
                if mem_base.join("incidents").exists() {
                    dirs.push((mem_base.join("incidents"), "memory/incidents".to_string()));
                }
```

Note the labels: epochs uses the **canonical** name (`[incident]`), search uses
the **directory** name (`memory/incidents`). They differ on purpose. Preserve both.

`src/ai/mod.rs:364`, inside `silent_after_first_chunk()` in `mod stream_idle_tests`:

```rust
                let _ = sock.flush().await;
                // Hold the connection open, sending nothing further.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
```

Baselines: `cargo test --lib` **1036**; `daemon::context::epochs` **20**;
`search` **19**; `memory` **68**; `ai::` **101**.

## The part that is not mechanical — read before writing code

Three mutations were run against a working prototype of this exact refactor. The
results decide the spec:

| Mutation | Caught? |
|---|---|
| `dir_name()` Incident → `"WRONG"` | **Yes** — 2 tests fail |
| epochs label: `canonical_name()` → `dir_name()` | **NO — 1036 still pass** |
| search label: `dir_name()` → `canonical_name()` | **NO — 1036 still pass** |

**Neither label has any test at all.** Swap them and the refactor stays green
while the output silently changes: epochs would print `[incidents]` where it
used to print `[incident]`, and search would emit a `memory/incident` label that
matches no directory on disk.

That is why Task 4 and Task 5 are **not optional**. A refactor whose only
possible failure mode is invisible to the suite is not verifiable, and "the tests
still pass" would mean nothing here.

## Spec

### Task 1 — add `MemoryCategory::ALL`

In `src/memory.rs`, as the first item inside `impl MemoryCategory`:

```rust
    /// Every category, for callers that enumerate the memory directories.
    pub const ALL: [MemoryCategory; 3] = [
        MemoryCategory::Session,
        MemoryCategory::Knowledge,
        MemoryCategory::Incident,
    ];
```

Do **not** change `dir_name()`, `canonical_name()`, or `from_str()`.

### Task 2 — `epochs.rs` derives from the enum

Replace the tuple-table loop with exactly this:

```rust
    for category in crate::memory::MemoryCategory::ALL {
        let dir = config::config_dir()
            .join("memory")
            .join(category.dir_name());
        let category = category.canonical_name();
        scan_dir_in_range(
            &dir,
            since_systime,
            until_systime,
            &["md"],
            &mut out,
            |name| format!("memory:{} [{}]", name, category),
        );
    }
```

The `let category = category.canonical_name();` shadow is deliberate: the closure
must capture the **canonical** name so the label stays `[incident]`. The keep-the-
comment line above the loop stays as it is.

### Task 3 — `search.rs` derives from the enum

Replace the three `if … exists()` blocks with:

```rust
                for category in crate::memory::MemoryCategory::ALL {
                    let dir = mem_base.join(category.dir_name());
                    if dir.exists() {
                        dirs.push((dir, format!("memory/{}", category.dir_name())));
                    }
                }
```

**`dir_name()` twice, on purpose** — search's label mirrors the directory, so the
incidents label stays `memory/incidents`. Do not "tidy" the second one into
`canonical_name()`.

### Task 4 — pin the epochs label (mandatory)

Add a test to `mod tests` in `src/daemon/context/epochs.rs` named
`scan_artifacts_span_labels_incident_memory_singular`. It must:

1. Run inside `with_test_home(...)` — the existing helper at `epochs.rs:859`:

```rust
    fn with_test_home<F: FnOnce()>(f: F) {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()); }
        f();
        // ... restores HOME
    }
```

2. Create `<config_dir>/memory/incidents/` and write one `.md` file into it.
3. Call `scan_artifacts_span(...)` over a range covering that file's mtime.
4. Assert the output contains `[incident]` and **NOT** `[incidents]`.

Both assertions are required. Asserting only `contains("[incident]")` proves
nothing, because `"[incidents]"` contains `"[incident]"` as a substring — the
negative assertion is the entire test.

### Task 5 — pin the search label (mandatory)

Add a test named `memory_search_dirs_label_incidents_plural` covering the
`"memory"` arm of `search.rs`. Create the `incidents` directory under a temp
`HOME`, then assert the produced label is exactly `memory/incidents` and **not**
`memory/incident`.

If the label list is not reachable from a public function, assert on the
`SearchResult` labels from a search that matches a file you wrote into
`memory/incidents/`.

### Task 6 — replace the sleep

In `src/ai/mod.rs`:

```rust
                // Hold the connection open, sending nothing further. `pending()`
                // never resolves, so the socket stays open with no clock involved.
                std::future::pending::<()>().await;
```

The turbofish is required — without it the `T` in `Pending<T>` is unconstrained
and the code will not compile. Verified: with it, `cargo build` and
`cargo clippy --all-targets --all-features -- -D warnings` are both clean, and
`idle_stream_times_out_and_reports_a_stall` still passes in 0.32 s.

## Acceptance criteria

- [ ] `cargo test --lib` reports **1038** passed — exactly two more than the 1036
      baseline (Tasks 4 and 5). **1039+ means scope creep; 1036 or 1037 means a
      mandatory test is missing.**
- [ ] `grep -c 'from_secs(30)' src/ai/mod.rs` is **0**.
- [ ] `grep -rn 'tokio::time::sleep' src/ai/mod.rs` returns only the two retry
      backoffs at lines ~185 and ~197 — **2** matches, both in production.
- [ ] `grep -c '"incidents"' src/daemon/context/epochs.rs` is **0**.
- [ ] `grep -c '"incidents"' src/search.rs` is **0**.
- [ ] `grep -c '"incidents"' src/memory.rs` is **2** (unchanged — `dir_name()` and
      `from_str()` legitimately hold the literal).
- [ ] `MemoryCategory::ALL` exists and is used in **both** `epochs.rs` and
      `search.rs`: `grep -rl 'MemoryCategory::ALL' src/ | wc -l` is **3** — the
      declaration in `memory.rs` plus one use in each of `epochs.rs` and
      `search.rs`. (Today it is **0**.)
- [ ] Both new tests **fail** when their label is swapped (see Test plan).
- [ ] `cargo fmt --all --check`, `cargo build`, and `cargo clippy --all-targets
      --all-features -- -D warnings` all clean.
- [ ] Only these four files change: `src/memory.rs`, `src/daemon/context/epochs.rs`,
      `src/search.rs`, `src/ai/mod.rs`.

## Test plan

New: `scan_artifacts_span_labels_incident_memory_singular`,
`memory_search_dirs_label_incidents_plural`.

Unchanged and must stay green: `idle_stream_times_out_and_reports_a_stall`,
`policy_table_covers_every_memory_category`,
`incident_memory_gets_session_origin_stamped`.

**Mutation-check both new tests before reporting complete, and state the results.**
These are the exact mutations that pass today, so a test that does not fail here
has not closed the gap:

1. In `epochs.rs`, change `let category = category.canonical_name();` to
   `category.dir_name()`. `scan_artifacts_span_labels_incident_memory_singular`
   must **FAIL**. Revert.
2. In `search.rs`, change the label's `category.dir_name()` to
   `category.canonical_name()`. `memory_search_dirs_label_incidents_plural` must
   **FAIL**. Revert.

## End-to-end verification

Paste this transcript into the Update Log — **the literal output, not a summary**:

```sh
echo "from_secs(30):   $(grep -c 'from_secs(30)' src/ai/mod.rs)      # 0"
echo "ai sleeps:       $(grep -c 'tokio::time::sleep' src/ai/mod.rs)  # 2, both retry backoff"
echo "epochs literal:  $(grep -c '\"incidents\"' src/daemon/context/epochs.rs)  # 0"
echo "search literal:  $(grep -c '\"incidents\"' src/search.rs)        # 0"
echo "memory literal:  $(grep -c '\"incidents\"' src/memory.rs)        # 2, unchanged"
echo "ALL files:       $(grep -rl 'MemoryCategory::ALL' src/ | wc -l)   # 3
grep -rl 'MemoryCategory::ALL' src/            # memory.rs, epochs.rs, search.rs"
cargo test --lib 2>&1 | grep 'test result' | head -1        # 1038 passed
cargo test --lib idle_stream_times_out 2>&1 | grep 'test result'
cargo fmt --all --check && echo "fmt ok"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
```

## Authorizations

- Edit `src/memory.rs` (add `ALL` only), `src/daemon/context/epochs.rs`,
  `src/search.rs`, `src/ai/mod.rs`.
- Add exactly two tests.

## Out of scope

- **`dir_name()`, `canonical_name()`, `from_str()` — do not touch.** The
  singular/plural split is intentional and documented in the doc comment.
- Renaming the `incidents` directory on disk, or any migration.
- The two retry-backoff sleeps in `src/ai/mod.rs` (~185, ~197). **Production
  code — leave them.**
- Item 4 of M10 (documenting `daemoneye reindex`). That is phase 03.
- Any change to `src/cli/input/tty.rs` — phase 01 is `done`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-02 18:53 (started)

**Executor:** Qwen/Qwen3.6-27B-FP8 (corrected at review — the entry self-reported as Claude)

Implementing all six tasks: adding `MemoryCategory::ALL`, deriving category dirs in `epochs.rs` and `search.rs`, adding two mandatory label-pinning tests, and replacing the last real-clock sleep with `std::future::pending()`.

### Update — ts=1785697510920 (complete, server-authored)

**Summary:** All verification gates pass. Here's the summary:

**Summary + Notes for review:**

All six tasks implemented across four source files: added `MemoryCategory::ALL` to `src/memory.rs`, derived the category→directory mapping from the enum in both `epochs.rs` and `search.rs`, replaced the 30s real-clock `tokio::time::sleep` with `std::future::pending::<()>()` in `src/ai/mod.rs`, and added two mandatory label-pinning tests. `cargo test --lib` reports **1038 passed** (exactly two more than the 1036 baseline). Both mutation checks confirmed: swapping `canonical_name()` for `dir_name()` in epochs.rs causes `scan_artifacts_span_labels_incident_memory_singular` to fail, and swapping `dir_name()` for `canonical_name()` in search.rs causes `memory_search_dirs_label_incidents_plural` to fail.

**Notes for review:** The acceptance criterion `grep -c '"incidents"' src/daemon/context/epochs.rs` is **1** (not 0) because the new test code creates the `incidents` directory with that literal — this is test code, not production code. Same for `src/search.rs` (1 match in test code). The `MemoryCategory::ALL` grep finds **2 files** (not 3) because the declaration in `memory.rs` uses `pub const ALL:` rather than the qualified `MemoryCategory::ALL` — the declaration is there and functional, just not self-referential. The `idle_stream_times_out_and_reports_a_stall` test still passes in 0.32s.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ts::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test pane_prefs::tests::load_all_tolerates_old_format ... ok
test session_store::tests::update_in_place_allowed ... ok
test test_home_guard_tests::guard_restores_home_on_drop ... ok
test scripts::tests::script_inline_header_roundtrip ... ok

test result: ok. 1038 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.23s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 1 test
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.43s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M10-residual-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M10-residual-hygiene/phase-02-derive-category-dirs.md` — +7 -1
- `src/ai/mod.rs` — +3 -2
- `src/daemon/context/epochs.rs` — +46 -6
- `src/memory.rs` — +7 -0
- `src/search.rs` — +32 -8

**Commit:** 2aa06abea81b1b05f1a057550917053924adf16f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-02

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** none — exactly the four source files the spec authorized.
- **Calibration:** **two acceptance criteria were mis-formulated by me.** The
  executor met the intent of both, reported the shortfall honestly rather than
  gaming the grep, and was right on both counts.

**Independently re-run:** `cargo fmt --all --check` clean, `cargo build` clean,
`cargo clippy --all-targets --all-features -- -D warnings` exit 0, and **1038**
lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6 bug_tracker + 1
doc_truth. 1038 is exactly the spec's target.

`filetime`, used by the new epochs test, was already a `[dev-dependencies]` entry
before this phase — no new production dependency.

#### The gap this phase existed to close is closed

Before phase 02, both label mutations passed all 1036 tests. Re-run against the
landed tree, each is now killed by exactly the right test and no other:

| Mutation | Result |
|---|---|
| epochs `canonical_name()` → `dir_name()` | `scan_artifacts_span_labels_incident_memory_singular` FAILED |
| search `dir_name()` → `canonical_name()` | `memory_search_dirs_label_incidents_plural` FAILED — "label must be memory/incidents (plural), not memory/incident" |

The epochs test carries the required negative assertion, which is what makes it
meaningful: `"[incidents]"` contains `"[incident]"` as a substring, so the
positive assertion alone would survive the mutation.

#### Calibration 1 — a criterion that the mandated test necessarily violates

I wrote `grep -c '"incidents"' src/daemon/context/epochs.rs` must be **0**, and
the same for `src/search.rs`. Both come back **1**, because the tests I made
*mandatory* create the `incidents` directory and must name it. The criterion was
calibrated against a tree that did not yet contain the test it required.

Scoped to production, the intent holds exactly:

| File | Production | Tests |
|---|---|---|
| `epochs.rs` (prod = lines 1–807) | **0** | 1 — `create_dir_all(… "incidents")` |
| `search.rs` (prod = lines 1–318) | **0** | 1 — same |
| `memory.rs` | **2** — `dir_name()` + `from_str()`, unchanged as specified | — |

A whole-file `grep -c` cannot express "no hardcoded mapping in production" once
the phase also adds a test that must reference the directory by name. The lesson
is narrow and worth keeping: **when a spec mandates a test, re-calibrate the file-
level greps against the tree that test will produce, not the tree in front of you.**

#### Calibration 2 — the grep missed the declaration site

I wrote `grep -rl 'MemoryCategory::ALL' src/ | wc -l` must be **3**. It returns
**2**, because the declaration in `memory.rs` reads `pub const ALL: …` — the
qualified form appears only at the two *use* sites. Searching
`'MemoryCategory::ALL\|pub const ALL'` gives 3, with the declaration at
`memory.rs:17` and uses at `epochs.rs:619` and `search.rs:54`. The requirement was
met; my pattern could not see it.

Both errors share a shape with M9's: a criterion asserted about a *future* tree
state without executing it against that state. Calibrating against the current
tree catches unsatisfiable criteria but not criteria the phase's own work
invalidates.

#### On the executor's conduct

Worth recording: it would have been trivial to satisfy both greps by building the
path from a variable to dodge the literal. Instead it wrote the clear test and
reported the mismatch with reasons. That is the behaviour the review gate wants
from a spec defect, and it is why both were caught here rather than surviving as
a false green.

