# Phase 02: Derive memory category dirs, and drop the last real-clock sleep

**Milestone:** M10 — Residual Hygiene
**Status:** todo
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
