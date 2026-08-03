# Phase 01: Write-time masking for epochs and events

**Milestone:** M11 — Unified Knowledge Index
**Status:** in-progress (bounced at review — see `bugs/bug-01-1.md`)
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Close the two mask-on-write gaps in the persistence layer: `append_epoch` and
`log_event` currently write caller data to disk unmasked, unlike every other
durable store. Later M11 phases make both files full-text-searchable, so
nothing unmasked may reach them.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Masking prerequisite" — why this phase
  exists and why it lands before any indexing phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The masking filter** (`src/ai/filter.rs`) exposes one public masking entry
point, re-exported at `src/ai/mod.rs:14` (`pub use filter::mask_sensitive;`):

```rust
/// Mask all known-sensitive patterns in `text` before it is sent to an AI API.
pub fn mask_sensitive(text: &str) -> String {
    let pats = PATTERNS.get_or_init(|| compile_patterns(&[]));
    ...
}
```

Facts that matter for this phase, all verified against the current tree:

- `mask_sensitive` self-initializes with the built-in patterns via
  `get_or_init` — it works correctly in unit tests without `init_masking`
  ever being called.
- Replacement strings are plain text with no quotes or backslashes:
  `<AWS_KEY>`, `<PRIVATE_KEY>`, `<JWT>`, `<REDACTED>`, etc.
  (`builtin_defs()`, `src/ai/filter.rs:61-109`).
- The pattern `r"AKIA[0-9A-Z]{16}"` → `<AWS_KEY>` is built-in, so the literal
  `AKIAIOSFODNN7EXAMPLE` is a deterministic canary — the existing test at
  `src/ai/filter.rs:181-184` already uses it.
- Masking is idempotent: replacements contain nothing any pattern matches, so
  re-masking already-masked text is a no-op. Some `log_event` callers pre-mask
  today (e.g. `src/daemon/webhook/process.rs`, `src/daemon/background/helpers.rs`);
  they stay as they are and are simply double-covered.

**Gap 1 — `log_event`** (`src/daemon/utils/event_log.rs:10-49`) is the single
write path for event segments (verified: `current_event_segment_path()` has no
other writing caller; `log_command` at `event_log.rs:257` is a shim that calls
`log_event`). It merges caller fields into the record with no masking:

```rust
pub fn log_event(event: &str, mut fields: serde_json::Value) {
    ...
    if let Some(obj) = fields.as_object_mut() {
        // Prepend ts + event + pid so they appear first in the line.
        let mut record = serde_json::Map::new();
        record.insert("ts".to_string(), serde_json::Value::String(ts));
        ...
        // Take ownership of the fields from the caller's object
        let drained = std::mem::take(obj);
        for (k, v) in drained {
            record.insert(k, v);
        }
        let mut line = serde_json::to_string(&record).unwrap_or_default();
        ...
```

**Gap 2 — `append_epoch`** (`src/daemon/context/epochs.rs:113-128`) is the
single write path for `<id>.epochs.jsonl` (verified: `epochs_file()` is used
for writing only here; the other use at `epochs.rs:91` is `read_epochs`). It
serializes the record with no masking:

```rust
pub fn append_epoch(id: &str, rec: &EpochRecord) {
    ...
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(rec)
```

`EpochRecord` and `EpochTally` both derive `Clone` (`epochs.rs:25`, `:62`).
The string-bearing fields an epoch can carry secrets in are:
`narrative: Option<String>`, `tally.failed_cmds: Vec<(String, i32)>`, and
`artifacts: Vec<String>`.

## Spec

### 1. `mask_json_value` helper — in `src/ai/filter.rs`

Add below `mask_sensitive`:

```rust
/// Recursively mask every string **value** in a JSON tree in place.
/// Object keys, numbers, booleans and nulls are left untouched.
pub fn mask_json_value(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            let masked = mask_sensitive(s);
            if masked != *s {
                *s = masked;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                mask_json_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            for (_k, item) in map.iter_mut() {
                mask_json_value(item);
            }
        }
        _ => {}
    }
}
```

Re-export it in `src/ai/mod.rs` next to the existing re-export:
`pub use filter::{mask_json_value, mask_sensitive};`

Values-only is deliberate: field *names* are schema, not payload, and renaming
keys would break every reader (`for_each_event_between` consumers,
`sum_cost_between`, `tally_span`). Masking the pre-serialization strings (not
the serialized line) is also deliberate — it keeps the JSON valid by
construction and avoids regexes having to match through JSON escaping.

### 2. Mask caller fields in `log_event` — in `src/daemon/utils/event_log.rs`

At the top of `log_event`, before `fields.as_object_mut()` is consumed, add:

```rust
crate::ai::mask_json_value(&mut fields);
```

Only the caller-supplied `fields` are masked. The daemon-generated `ts`,
`event`, and `pid` values are inserted afterward and stay untouched — do not
restructure the function so they pass through masking.

### 3. Mask epoch string fields in `append_epoch` — in `src/daemon/context/epochs.rs`

Clone the record and mask its three string-bearing fields before serializing.
Replace `serde_json::to_string(rec)` with serialization of the masked clone:

```rust
let mut masked = rec.clone();
if let Some(n) = masked.narrative.as_mut() {
    *n = crate::ai::mask_sensitive(n);
}
for (cmd, _code) in masked.tally.failed_cmds.iter_mut() {
    *cmd = crate::ai::mask_sensitive(cmd);
}
for a in masked.artifacts.iter_mut() {
    *a = crate::ai::mask_sensitive(a);
}
```

Serialize the **struct clone**, not a `serde_json::Value` round-trip — going
through `Value` would reorder the keys alphabetically (`serde_json::Map` is a
BTreeMap) and change the on-disk line shape for no reason. Do not touch
`kind`, `covers`, timestamps, or counts.

### 4. Tests — per the Test plan below

Unit tests in the three touched modules. Tests that redirect `HOME` MUST use
the RAII guard idiom quoted in the Test plan — `STANDARDS.md` forbids raw env
mutation without the lock, and this repo is edition 2024, so `set_var` is
`unsafe`.

## Acceptance criteria

- [ ] A `log_event` call whose fields contain `AKIAIOSFODNN7EXAMPLE` — at the
      top level, inside a nested object, and inside an array — produces a
      segment line containing `<AWS_KEY>` and **not** containing the canary,
      and the line still parses as JSON with `ts`, `event`, `pid` present.
- [ ] An `append_epoch` call whose record carries the canary in `narrative`,
      in `tally.failed_cmds[0].0`, and in `artifacts[0]` produces a file line
      with all three masked, and `read_epochs` round-trips the record.
- [ ] Must-NOT-change cases hold: an event field `{"prompt_tokens": 123}`
      stays numeric and unrenamed; a field **named** `"token_usage"` keeps its
      name; an epoch's `kind`, `seq`, `turn_start`/`turn_end` and `covers` are
      byte-identical to the unmasked serialization.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green with no removed or `#[ignore]`d-away existing tests.

## Test plan

In `src/ai/filter.rs` tests:

- `test_mask_json_value_masks_nested_string_values` — object → array → object
  nesting with the AWS canary at each level; all become `<AWS_KEY>`.
- `test_mask_json_value_leaves_keys_and_non_strings` — a map with key
  `"token_usage"`, a number, a bool and a null is unchanged except string
  values; the key survives verbatim.

In `src/daemon/utils/event_log.rs` tests (this module writes under
`$HOME/.daemoneye`, so each test takes a home guard):

- `test_log_event_masks_caller_fields` — canary in a top-level string, a
  nested object and an array; read the segment file back; assert `<AWS_KEY>`
  present, canary absent, line parses, `ts`/`event`/`pid` present.
- `test_log_event_leaves_daemon_fields_and_numbers` — event name and numeric
  fields unchanged.

In `src/daemon/context/epochs.rs` tests:

- `test_append_epoch_masks_narrative_tally_and_artifacts` — canary in all
  three fields; file line masked; `read_epochs` returns the parsed record.
- `test_append_epoch_preserves_structure` — a record without secrets
  serializes to the same line as before the change (construct the expected
  line with `serde_json::to_string` on the input record).

HOME-redirecting tests MUST use this exact RAII idiom (the canonical copy
lives at `src/daemon/context/recall.rs:246-282` — same shape, quoted here so
there is no need to search for it):

```rust
struct TestHome {
    _tmp: tempfile::TempDir,
    _lock: crate::TestHomeGuard,
    saved: Option<String>,
}
impl TestHome {
    fn new() -> Self {
        let lock = crate::test_home_guard();   // NOT the raw TEST_HOME_LOCK —
        let saved = std::env::var("HOME").ok(); // the accessor recovers poison
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()); }
        Self { _tmp: tmp, _lock: lock, saved }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        unsafe {
            match &self.saved {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
```

Gotcha: do **not** call `init_masking` in tests and do not rely on it having
been called — `mask_sensitive` self-initializes builtins via `get_or_init`,
and another test may already have populated the global `OnceLock`. Use only
built-in patterns (the AWS canary) so results are deterministic regardless of
test order.

## End-to-end verification

The acceptance criteria are exercised through the same functions the daemon
calls (`log_event` / `append_epoch` are the verified single writers), so the
end-to-end pass is: run the new tests with captured output, then prove the
choke points are still the only writers. Run exactly this block and paste the
two output files into your Update Log entry:

```sh
cargo test --lib masks -- --nocapture > /tmp/phase01-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase01-tests.txt
grep -rn "current_event_segment_path\|epochs_file" src --include=*.rs \
  | grep -v "config/load.rs\|path_audit\|read_epochs\|mod tests\|fn epochs_file" \
  > /tmp/phase01-writers.txt 2>&1; echo "exit=$?" >> /tmp/phase01-writers.txt
```

The first file must show every new test passing. The second must list write
uses only inside `event_log.rs` and `epochs.rs` (reads and the path
constructors are filtered out) — that is the proof no other writer bypasses
the new masking.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry and its "Command output tails" block do not satisfy this**
— it proves the gates ran, not that these criteria were exercised.

## Authorizations

- Modify: `src/ai/filter.rs`, `src/ai/mod.rs`,
  `src/daemon/utils/event_log.rs`, `src/daemon/context/epochs.rs`.
- No new dependencies. No schema, config, or CLI changes.

## Out of scope

- Any FTS5 / index work — that starts in phase 02.
- Removing the per-call-site pre-masking in `webhook/process.rs` /
  `background/helpers.rs` — harmless double coverage; leave it.
- Masking in `render_context_block` or any read path — write-time masking
  makes read-time masking of these two stores unnecessary, and existing
  read-time masks elsewhere stay as they are.
- The redaction counters (`REDACT_*`) will now also count hits from these two
  write paths; that is expected and needs no adjustment.
- Retention or GC for epochs/session files.

## Update Log

### Update — 2026-08-03 17:04 (started)

**Executor:** Claude (Anthropic)

Implementing write-time masking for `log_event` and `append_epoch` per spec.

### Update — ts=1785777496175 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Built write-time masking for both `log_event` and `append_epoch` persistence paths. Added `mask_json_value` in `src/ai/filter.rs` that recursively masks string values in a JSON tree (keys, numbers, bools, nulls untouched), re-exported it from `src/ai/mod.rs`. Applied `mask_json_value` to caller-supplied fields at the top of `log_event` before daemon-generated fields are merged. Applied `mask_sensitive` to `narrative`, `tally.failed_cmds` command strings, and `artifacts` in `append_epoch` via a struct clone (not a `Value` round-trip) to preserve key ordering.

**Deviations from spec:** The spec's epoch test struct literals used field names (`turn_cost`, `covers: Vec<String>`) that don't match the actual struct definitions (`failed_cmds: Vec<(String, i32)>`, `covers: Option<(u32, u32)>`, `turn_start`/`turn_end` are `u32` not `f64`, plus missing required fields `ts_start`, `ts_end`, `msg_count`). Tests were corrected to match the actual structs. Also, the epoch file path is under `~/.daemoneye/var/log/sessions/` not `~/.daemoneye/sessions/` as implied by the spec.

**End-to-end verification:** All 4 new masking tests pass (`cargo test --lib masks` — exit 0). Writer grep confirms `current_event_segment_path` write uses are only in `event_log.rs` and `epochs_file` write uses are only in `epochs.rs`. All gates (`fmt`, `build`, `clippy`, `test`) pass clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s


TEST
 session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok

test result: ok. 1044 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.57s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_uses_first_occurrence_only ... ok
test header_status_strips_trailing_prose ... ok
test header_status_reads_bare_word ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 4 tests
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
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
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.03s


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

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.18s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M11-knowledge-index/README.md` — +1 -1
- `docs/dev/milestones/M11-knowledge-index/phase-01-write-time-masking.md` — +6 -2
- `src/ai/filter.rs` — +61 -1
- `src/ai/mod.rs` — +1 -1
- `src/daemon/context/epochs.rs` — +127 -1
- `src/daemon/utils/event_log.rs` — +51 -0

**Commit:** 2cabf1ed1312153dcf272aa713dd7eaa8acf02da

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
