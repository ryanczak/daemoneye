# Phase 01: Write-time masking for epochs and events

**Milestone:** M11 — Unified Knowledge Index
**Status:** in-progress
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
