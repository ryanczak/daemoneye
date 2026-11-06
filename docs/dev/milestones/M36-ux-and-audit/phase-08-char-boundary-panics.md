# Phase 08 — char-boundary-panics

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
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
- **Lesson from phase 07:** a pure helper with tests is not enough on its
  own. Each finding gets a test that drives its real call site, or the
  smallest function that contains the slice, with the offending input.

## Spec

1. **Red tests first.** Write the tests below against the current tree, run
   them, and keep the failing output (panics included) for step 4. All carry
   the `char_boundary_` prefix:
   - `char_boundary_webhook_fingerprint`: an alert whose fingerprint has a
     multibyte char across byte 8 and across byte 16 goes through the code
     that formats and logs the fingerprint without panicking. Choose the
     seam: extract it if `process_alert` has none.
   - `char_boundary_search_summary`: a `SearchRepository` whose query is 20
     CJK chars has a `summary()` that does not panic and is a prefix of the
     query. A 50-char ASCII query still summarizes to its first 40 chars.
   - `char_boundary_foreground_summary`: the same for a `Foreground` `cmd`.
   - `char_boundary_frame_preview`: the malformed-frame preview for a
     120+-byte string with a multibyte char straddling byte 120 does not
     panic, through whatever function the three backends now share.
   - `char_boundary_iso_overflow`: `parse_iso_duration` handles
     `P200000000000D` and `PT18446744073709551615S` without panicking and
     returns `None` (or a value the caller then refuses). The schedule
     tool's interval path refuses `P100000000D` with `Invalid interval`.
     `P36500D` is still accepted.
   - `char_boundary_stored_interval_overflow`: an `Every` with a huge
     `interval_secs` survives `advance` and `skip_to_future` and ends with
     `next_run == DateTime::<Utc>::MAX_UTC`.
2. **Implement** in this order, running `cargo build` after each: the
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
