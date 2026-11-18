# Phase 08a — registry-replay

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-17
**Depends on:** phase-07b
**Estimated diff:** ~300 lines, more than half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The active-alert registry survives a daemon restart. On start, before the
webhook listener serves its first request, the daemon rebuilds the registry
from the last `ACTIVE_ALERT_TTL_SECS` of the notice journal (the `event:
"notice"` records `Bus::publish` writes to the event log). A shared journal
reader lands with it, for phases 08b (away digest) and 08c (catch-up brief)
to reuse.

This phase is **additive**. No existing reader changes.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design ("Journal first").
- `src/bus/mod.rs` `Bus::new`: the journal is `log_event("notice",
  serde_json::to_value(notice))`. Each record is the serialized `Notice`
  plus `event` and `pid`, and its `ts` is the notice's own `ts`.
- `src/daemon/utils/event_log.rs` `for_each_event_between`.
- `src/daemon/notice_sinks.rs` `record_alert_registry` (the live path) and
  `default_routes(..).alert_registry` (the registry's route).
- `src/webhook/active.rs`: the registry and its `_in(map, …)` test seams.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Read **only** these files:

- Step 1: `src/bus/mod.rs`, `src/bus/notice.rs`, a new `src/bus/journal.rs`.
- Step 2: `src/daemon/notice_sinks.rs`, `src/webhook/active.rs`.
- Step 3: `src/daemon/startup.rs`.
- Step 4: `tests/isolation.rs`, `tests/harness/mod.rs` (read only), and
  the `src/bus/` row of `CLAUDE.md`.

Each step ends at a green `cargo build --tests`.

## Spec

1. **The journal reader**: a new module, `src/bus/journal.rs`.
   - `notice_from_record(&serde_json::Value) -> Option<Notice>`: `Some`
     only for a record whose `event` is `"notice"` and that deserializes as
     a `Notice`. The extra `event` and `pid` keys are ignored.
   - `for_each_notice_between(from, to, f)`: built on
     `for_each_event_between`, it calls `f` with each journaled notice in
     `[from, to)`, in file order. A malformed line or a non-notice record is
     skipped, **never a panic**.

2. **The replay** (in `notice_sinks.rs`, beside `record_alert_registry`):
   - Factor out the registry update both paths share: a firing notice
     records the alert at the notice's `ts`, with the severity label falling
     back to the parsed severity. A resolved notice removes the alert.
   - The live path keeps broadcasting `AlertState` when the registry
     changed.
   - The replay applies, in journal order, exactly the notices the
     registry's route matches (`default_routes(..).alert_registry`):
     webhook firing and resolved, at any severity. It **never broadcasts**:
     no chat is attached at startup.
   - Give the replay a seam that takes the map to fill, as `active.rs`'s
     `_in` functions do. Tests use their own map; production fills the
     process-wide registry.
   - Window: `[now - ACTIVE_ALERT_TTL_SECS, now)`.
   - Log one `info` line with the number of alerts restored.

3. **Startup** (`run_daemon`): replay right before
   `register_default_sinks`. That is earlier than the webhook listener, so
   a live notice never races the replay.

4. **Docs**: name `journal.rs` in the `src/bus/` row of `CLAUDE.md`.

## Acceptance criteria

- [ ] `grep -n 'pub mod journal\|mod journal' src/bus/mod.rs` prints one
      line.
- [ ] Every test in the Test plan exists and passes.
- [ ] Preservation: the five `active_alerts_*` tests in `src/webhook/active.rs`
      and the existing alert-registry sink tests in `notice_sinks.rs` pass
      unchanged.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.

## Test plan

- `src/bus/journal.rs`:
  - `notice_from_record_reads_a_journal_line`: a record built as
    `Bus::new` builds it (the serialized notice plus `event` and `pid`)
    gives back an equal `Notice`.
  - `notice_from_record_skips_other_records`: a `webhook_alert` record, a
    `notice` record missing `source`, and a non-object all give `None`.
- `src/daemon/notice_sinks.rs` (each test uses its own map):
  - `replay_rebuilds_what_is_still_firing`: firing A, firing B, then
    resolved A leaves only B.
  - `replay_keeps_first_seen_across_refires`: B fires at t1 and again at
    t2. Its `first_seen_unix` is t1 and its `last_seen_unix` is t2.
  - `replay_ignores_what_the_registry_route_does_not_match`: a webhook
    `Event` notice (an analysis) and a scheduler notice marked `Firing`
    record nothing.
  - `replay_reads_only_the_ttl_window`: inside `test_home`, journal one
    firing notice an hour old and one older than the TTL. Replay records
    only the first.

## End-to-end verification

`daemon_start_replays_active_alerts` in `tests/isolation.rs`:
- Before `start_daemon`, write a journal record for one firing webhook
  notice, an hour old, with a fingerprint no other test uses, into the
  throwaway root's current event segment. The path is
  `config::current_event_segment_path`'s, rooted at the harness root.
- Start the daemon. Send `Request::AlertList` over its socket, serialized
  with the crate's own type.
- Assert the `AlertState` reply carries that fingerprint.

Paste the `cargo test --test isolation` result line into the Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".

## Out of scope

- The away digest (08b) and the catch-up brief (08c): they keep their
  current readers.
- An alert that fired more than `ACTIVE_ALERT_TTL_SECS` ago and is still
  firing has its `first_seen_unix` restored as its oldest notice inside the
  window. This is accepted: the TTL already bounds the registry.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-17 23:55 (started)

**Executor:** executor (headless run)

Started phase 08a. Status flipped to `in-progress` and the milestone README
row updated. Beginning step 1: the journal reader at `src/bus/journal.rs`.

### Update — 2026-11-17 17:20 (end-to-end verification)

**Ran by:** executor (headless run)

```
$ cargo test --test isolation daemon_start_replays_active_alerts

running 1 test
test daemon_start_replays_active_alerts ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.29s
```

The test starts a real daemon against a throwaway `$HOME`, appends one firing
webhook notice (an hour old, fingerprint `fp-08a-replay-e2e`) to the throwaway
root's current event segment at `config::current_event_segment_path`'s path,
restarts the daemon, and sends `Request::AlertList` over the daemon's Unix
socket serialized with the crate's own `daemoneye::ipc` types. The
`Response::AlertState` reply carried that fingerprint.

The literal grep: `grep -n 'mod journal' src/bus/mod.rs` → `21:mod journal;`.
