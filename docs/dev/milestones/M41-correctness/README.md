# M41 — Correctness & reliability

**Goal:** the audit's remaining correctness bugs are fixed, starting with
the test suite and the source gates, so every later review can trust a
green run. The AI provider layer retries and trips the breaker for the
right reasons. Sessions, compaction, recall and the scheduler keep their
data intact. The terminal input and TUI paths handle real input.

**Status:** active (scoped 2026-11-21, signed off by the PE 2026-11-27)

**Release:** 2.13.0

**Depends on:** M40 (released as 2.12.0 on 2026-11-27).

**Sources:** the correctness, test and quality rows of
[`docs/design/daemoneye_audit_ledger.md`](../../../design/daemoneye_audit_ledger.md)
not taken by M39, plus P5 (a data-integrity race) and A3/A4a (two bugs
filed under architecture). Also GitHub issue #4 (cron is UTC-only, so
local-time schedules drift an hour across DST), added by PE decision
2026-11-21. It has no ledger row; phase 10b closes it with `Fixes #4`.
Phase 09 also closes issue #16, the same defect as C6. Added by PE decision
2026-11-27, also without ledger rows:
- the M39 live-check finding on the interrupt grace;
- issues #11, #12, #13, #15, #17 and #18.

Each of these is reproduced at drafting like a ledger row.

**Decisions (proposed at scoping, signed off by the PE 2026-11-27):**

1. M39's decisions 1–3 carry over: a failing test first, re-verification
   at drafting, lean closed specs.
2. **Tests and gates first** (M32 decision 4). Phase 01 removes the
   environment races, and phase 02 makes the two source gates see the
   whole file. Every review after that pays less for flakes, and the
   gates guard the rest of the milestone.
3. **Gates parse, they do not split strings.** Phase 02 moves
   `tests/lock_invariant.rs` and `tests/tmux_read_only.rs` onto the same
   `syn` traversal `tests/hygiene.rs` uses, and widens the lock gate to
   `read`/`write`/`try_*`. A file the new gate flags is fixed in the same
   phase (the audit's full scan found none).

**Exit criteria:**

- Every ledger row targeted at M41 reads `act: M41 phase-NN` or
  `reject: <why>`.
- `cargo test` passes 20 runs in a row with no failure, and `$SHELL`,
  `$TMUX` and `$TMUX_PANE` are unchanged after the suite.
- A gate fixture with production code below a `#[cfg(test)]` item is
  scanned by both gates (each gate's own test).
- An Anthropic session resumed after a cancelled empty turn and a
  redirect sends no two consecutive `user` messages (a converter test),
  and a resumed named session never opens on an orphan tool result.
- Issue #4's acceptance criteria hold. A cron job created with
  `tz = "America/Los_Angeles"` and `0 5 * * *` fires at 05:00 local on
  both sides of both 2027 DST transitions (a test over pinned instants),
  keeps its `tz` across a daemon restart, and `list_schedules` shows its
  next run in UTC and in its zone. A job without `tz` keeps UTC.
- Issues #11, #12, #13, #15, #16, #17 and #18 are closed by their phases'
  `Fixes #N` commits. A `run_terminal_command` with `timeout_secs: 5` on
  `yes` returns in under 7 s.
- `cargo test`, `tests/hygiene.rs` and `tests/doc_truth.rs` pass.
  `2.13.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/architecture.md` § 1.3 (the AI provider layer), § 1.4
  (persistence), § 2 (data flows).

## Phases

Drafted one at a time with `/rexymcp:architect next`. Each phase may split
at drafting.

| #  | Phase | Ledger | Status |
|----|-------|--------|--------|
| 01 | [test-hazards](phase-01-test-hazards.md) — the 17 no-op `with_home` tests hold the HOME guard across their await; `launch.rs`'s multi-thread test stops holding a `std` guard across `.await`; env cleanup restores the previous value instead of removing it | Q9 Q10 Q11 | done |
| 02 | [gate-blind-spots](phase-02-gate-blind-spots.md) — `lock_invariant.rs` and `tmux_read_only.rs` walk the `syn` AST, skipping only test-only items; the lock gate covers `read`/`write`/`try_*` | Q4 Q5 | review                           |
| 03 | circuit-breaker — success is recorded when a stream finishes, a 4xx does not trip the breaker, the breaker is per provider, and half-open lets exactly one probe through | C7 | todo |
| 04 | sse-retry — transport errors, stalls and non-retriable provider errors are classified apart; Gemini reports a malformed call once; `"error": null` is not an error | C12 C24 | todo |
| 05 | role-coalescing — `AnthropicClient::convert_messages` merges consecutive same-role messages; a redirect keeps the results of tools that already ran in its batch | C8 | todo |
| 06 | session-load — `load_session_messages` cuts at a clean turn start (as `read_session_file` does); `save_session` keeps `tags` and `parent`; `rename_session` reports a failed `meta.toml` write | C9 C21 Q3b | todo |
| 07 | compaction — background compaction's first epoch starts at `started_at`, uses the session's active model window and the real tail turn; chapter blocks print turn numbers, with checked arithmetic | C10 C15 | todo |
| 08 | recall — the stemming fallback returns an excerpt, and `build_excerpt` clamps on char boundaries after case folding | C16 | todo |
| 09 | webhook-lifecycle — dedup keys on `(fingerprint, status)` so a resolve is delivered; `spawn_webhook_task` does not re-supervise a consumed listener | C6 A3, issue #16 | todo |
| 10 | scheduler — `mark_done` leaves a cancelled job cancelled; `interval_secs == 0` is refused at creation and load; `save` is serialized, unique-temp, `sync_all`ed; save errors are logged, not dropped | C18 P5 Q3a | todo |
| 10b | cron-timezone — an optional IANA `tz` on a cron schedule and on `schedule_command`, validated at creation (an unknown zone is refused). The cron is evaluated in that zone and fired in UTC, so DST gaps and repeats have a stated rule. `list_schedules` shows the next run in UTC and in the job's zone. A job without `tz` is unchanged, and old `schedules.json` files still load. Issue #4 option 1 only: no daemon-wide default zone. *Decide at drafting:* the new dependency, `chrono-tz` or `jiff` | issue #4 | todo |
| 11 | turn-inbox — the inbox is found after a cold reload and released on every exit path of the turn | C14 | todo |
| 12 | shell-leaks — a shell whose `await_ready` or `spawn_command` fails is closed; the chat sandbox reaper re-checks use under its lock and keeps failed removals for the next pass | C19 C20 | todo |
| 13 | ghost-capacity — a ghost's slot is reserved at the capacity check, so concurrent spawns cannot exceed `max_concurrent_ghosts` | A4a | todo |
| 14 | input-decoding — `AsyncStdin` decodes UTF-8; SGR mouse math is checked; bracketed paste keeps `[`; OSC 7501 accepts unpadded base64 | C11 C17 C23c | todo |
| 15 | cli-tui — `wrap_line` across spans, `InputLine` by display width, `/clear` and `/new` reset the status bar, transcript and presence, `top` restores the terminal on panic | C23a C23b C23d C23e | todo |
| 16 | small-fixes — DST-safe day bounds; expired memories are not indexed; `finish_command` keeps evicted counters; current OpenAI model families get their real windows; log rotation and `pane_prefs` write atomically | C13 C22 C25 C26 Q3c | todo |
| 17 | interrupt-grace — a timed-out command stopped by Ctrl-C ends its interrupt grace when the shell is back at its prompt, not after the full 5 s (the M39 live check: `timeout_secs: 5` returned after about 10 s) | M39 live finding | todo |
| 18 | program-notices — a shell that reports `blocked` / `error` / `done` over OSC 7501 within its first 2 s probe still produces a notice (`ProgramWatcher`'s first-sight baseline) | issue #13 | todo |
| 19 | ui-labels — the status bar's model and vendor glyph follow `/model`; the approval panel names a remote host by its `[hosts.<name>]` key, not its ssh destination | issues #15 #11 | todo |
| 20 | host-context — the host context is refreshed at session start and periodically, not captured once at daemon start | issue #12 | todo |
| 21 | tooling — `classify_version` accepts `sbx version v0.54.0 <sha>`, so sandbox status is not always unavailable; `audit-prompts` stops reporting existing files as unknown paths | issues #17 #18 | todo |

## Notes

- **Scoped 2026-11-21** with M39. Issue #4 joined as phase 10b (PE
  decision 2026-11-21). It comes after phase 10 because both edit
  `scheduler.rs`. It must ship before the 2027-03-14 US DST change, so if
  M41 is not on track for that, 10b can go out early as a patch release.
  Its new `schedule_command` parameter needs the tool-checklist doc
  updates (`sre.toml`, `README.md`) and passes `tests/doc_truth.rs`.
- **Signed off 2026-11-27.** The PE added phases 17–21: the M39 live
  check's interrupt-grace finding, and the open bug issues #11, #12, #13,
  #15, #17 and #18. Each may merge with a neighbour or split at drafting.
  10b still has to ship before 2027-03-14.
