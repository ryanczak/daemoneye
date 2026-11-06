# Phase 08 — char-boundary-panics

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-05
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

Fix four findings in `FINDINGS.md` where text from a model or a sender
reaches a byte slice or unchecked arithmetic and panics a daemon task.

- **B3:** `src/webhook/process.rs` · `process_alert` slices the
  sender-controlled fingerprint with `[..len().min(16)]` (twice) and
  `[..len().min(8)]`. A fingerprint like `abcdefgü` panics the spawned task
  after the dedup entry is inserted. The alert is lost, and redeliveries are
  dropped as duplicates for the dedup window.
- **B4:** `src/ai/types/pending.rs` · `PendingCall::summary` slices `query`
  (the `SearchRepository` arm) and `cmd` (the `Foreground` arm) at byte 40. A
  search for 14 or more CJK characters panics the client handler mid-turn.
- **B16:** `src/ai/backends/{anthropic,openai,gemini}.rs` log a malformed
  frame with `&data[..data.len().min(120)]`, which panics when a multibyte
  character straddles byte 120.
- **B14:** `src/scheduler.rs` · `parse_iso_duration` computes `val * multiplier`
  and `secs +=` unchecked. Its caller in `src/daemon/executor/schedule.rs`
  then casts with `secs as i64`, calls `chrono::Duration::seconds`, and adds
  to `Utc::now()`. `ScheduleKind::advance` and `skip_to_future` repeat the
  cast and the add.
  - `P200000000000D` panics in `Duration::seconds`.
  - `P100000000D` passes that step and panics on the add.

## Behaviour

- **Truncation is by char, never by byte.** Every site above cuts at the
  largest char boundary at or below its byte limit (16, 8, 40, 120), so the
  output is never longer than today's output for ASCII. ASCII output is
  unchanged byte for byte. The `SearchRepository` and `Foreground` summaries
  keep their current shape for ASCII input.
- **Durations:**
  - `parse_iso_duration` returns `None` when the arithmetic overflows `u64`.
  - The `schedule_command` path refuses an interval whose seconds do not fit
    a `chrono::Duration` or whose first `next_run` does not fit a `DateTime`,
    with the existing `Invalid interval '…'` message and no panic.
  - `P36500D` (100 years) is still accepted.
- **A stored job:** if an `Every` job loaded from `schedules.json` has a step
  that would overflow, `advance` and `skip_to_future` do not panic. They set
  `next_run` to `DateTime::<Utc>::MAX_UTC`, so the job never fires again
  instead of re-firing on every tick.

## Constraints

- One char-safe truncation helper serves all the sites. Placing it in
  `src/util.rs` is fine. `str::floor_char_boundary` is stable on this
  toolchain (rustc 1.97). `src/daemon/digest.rs` has a private
  `floor_char_boundary` that may be replaced by the std one or left as it is.
- Use checked chrono arithmetic (`TimeDelta::try_seconds`,
  `checked_add_signed`) at the duration sites. Never use `as i64` on a value
  that can exceed `i64::MAX`.
- Preservation: the existing `parse_iso_duration` tests and the `summary()`
  tests in `pending.rs` stay green unedited.
- **Seams are named; do not search for others.** Every test below names
  the function it calls. Do **not** build a `WebhookState`, run
  `process_alert`, start a daemon, or call the `schedule_command` executor
  arm. Do not read `tests/isolation.rs`, `tests/integration.rs`,
  `src/daemon/spend.rs` or `src/ai/mod.rs`. For B3 and B16, the
  `byte_slices=0` criterion proves the call sites changed, and the helper
  test proves the helper is safe.
- **One duration seam:** add a pure `checked_next_run(from: DateTime<Utc>,
  secs: u64) -> Option<DateTime<Utc>>` in `src/scheduler.rs`, returning
  `None` when the seconds do not fit a `TimeDelta` or the sum does not fit a
  `DateTime`. The `schedule_command` interval path in
  `src/daemon/executor/schedule.rs`, `advance` and `skip_to_future` all call
  it. The tool path maps `None` to the existing `Invalid interval` reply.

## Spec

1. **Red tests first.** Write the tests below against the current tree, run
   them, and keep the failing output (panics included) for step 4. All carry
   the `char_boundary_` prefix:
   - `char_boundary_webhook_fingerprint`: the helper, called with
     `abcdefgü` at 8 and with a 15-ASCII + `ü` string at 16, does not panic
     and returns a prefix of its input of at most that many bytes. ASCII
     input of 20 bytes cut at 16 returns its first 16 bytes.
   - `char_boundary_search_summary`: a `SearchRepository` whose query is 20
     CJK chars has a `summary()` that does not panic and is a prefix of the
     query. A 50-char ASCII query still summarizes to its first 40 chars.
   - `char_boundary_foreground_summary`: the same for a `Foreground` `cmd`.
   - `char_boundary_frame_preview`: the helper, called with 119 ASCII bytes
     followed by `é` and more text, cut at 120, returns the 119 ASCII bytes
     and does not panic.
   - `char_boundary_iso_overflow`: `parse_iso_duration` returns `None` for
     `PT18446744073709551615M` (the `u64` multiply overflows) and does not
     panic on `P200000000000D`. `checked_next_run(now, secs)` returns `None`
     for the seconds of `P200000000000D` and of `P100000000D`, and `Some`
     for the seconds of `P36500D`.
   - `char_boundary_stored_interval_overflow`: an `Every` with a huge
     `interval_secs` survives `advance` and `skip_to_future` and ends with
     `next_run == DateTime::<Utc>::MAX_UTC`.
2. **Implement**, one edit per turn, in this order, running `cargo build` after each: the
   helper, the webhook sites, the summary arms, the three backends, the
   duration parser and the schedule caller, then `advance` and
   `skip_to_future`.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add one `### Update — <date> (red runs)` entry and paste
   step 1's `test result:` lines and panic messages verbatim.
5. **Findings.** Add one `### Findings — <date>` entry, two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: model and sender text no longer panics on a char boundary or an overflowing interval (M36 phase-08)`.

## Acceptance criteria

- [ ] The E2E block's first line reads `char_boundary passed=P failed=0`
      with P ≥ 6. Today it reads `passed=0`.
- [ ] The E2E block shows `byte_slices=0`: no `[..….len().min(` slice is
      left in `src/webhook/process.rs` or `src/ai/backends/`, and no `[..40]`
      is left in `pending.rs`. Today the count is 8.
- [ ] The E2E block shows `unchecked_secs=0`: `scheduler.rs` and
      `executor/schedule.rs` have no `Duration::seconds(… as i64)`. Today the
      count is 3.
- [ ] The red-runs entry shows the new tests failing or panicking on the
      pre-phase code.
- [ ] Preservation: the existing `parse_iso_duration` and `summary()` tests
      pass unedited.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test char_boundary_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "char_boundary passed=" p, "failed=" f}'
{ grep -rE '\[\.\.[a-z_.]+\.len\(\)\.min\(' src/webhook/process.rs src/ai/backends/; grep -F '[..40]' src/ai/types/pending.rs; } | wc -l | sed 's/^/byte_slices=/'
grep -E 'Duration::seconds\([^)]* as i64\)' src/scheduler.rs src/daemon/executor/schedule.rs | wc -l | sed 's/^/unchecked_secs=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/util.rs`, `src/webhook/process.rs`, `src/ai/types/pending.rs`,
  `src/ai/backends/{anthropic,openai,gemini}.rs`, `src/scheduler.rs`,
  `src/daemon/executor/schedule.rs`, and `src/daemon/digest.rs` (only to
  share the helper).
- A new or raised `tests/hygiene_allowlist.txt` entry is acceptable if a
  touched file or function crosses its cap. State the reason in Findings.

## Out of scope

- Other byte slices in the tree that take no model or sender text.
- `interval_secs == 0` in a hand-edited store, and the `Cron` arms.
- The FINDINGS.md ledger's `act:` column (updated at milestone close).

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-11-06 (refined re-dispatch)

The first run spent 923 turns, 900 of them on `bash` greps and
`read_file`, and wrote no code. It was hunting for a way to drive
`process_alert` and the schedule tool end to end. The spec now names every
seam, under § Constraints and Spec step 1. Everything you need is in the
files § Authorizations lists. Read the four finding sites once, then start
writing the tests. Your first `patch` should land within about 30 turns.
Nothing from the first run is on disk except this doc's status flip.

### Update — 2026-11-06 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the run was cancelled (`user_stop`) at turn 923 with no code
written. Resuming would keep the open-ended "choose the seam" wording that
caused the exploration loop, so the spec now names each test's seam and adds
`checked_next_run` as the one duration seam.

### Update — 2026-11-06 00:24 (started)

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local (the run's own line misnamed the model; corrected by the architect)

Read the phase doc, `STANDARDS.md`, and the four finding sites (B3, B4, B16,
B14). Flipped the phase doc and the README row to `in-progress`. Beginning
with the red tests per the spec's task order.
