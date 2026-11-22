# M42 — Performance & concurrency

**Goal:** the daemon's async runtime stops doing blocking work. The tmux
poller, the event log and the memory index run off the Tokio workers.
SQLite stops dropping writes under contention. Global locks are not held
across disk I/O and are taken in one order. The hot paths stop rescanning
from byte 0.

**Status:** planning (scoped 2026-11-21)

**Release:** 2.14.0

**Depends on:** M41 (released as 2.13.0).

**Sources:** the performance rows of
[`docs/design/daemoneye_audit_ledger.md`](../../../design/daemoneye_audit_ledger.md)
(P5 went to M41 as a data-integrity race).

**Decisions (architect proposals at scoping, for PE sign-off):**

1. **A performance phase states its measurement.** Either a test that
   fails on the old shape (a contention test that saw `SQLITE_BUSY`, an
   offset read that touches only its own line, a scan bounded by `n`) or
   a before/after number captured the same way on both trees. "Should
   be faster" is not a finish condition.
2. **Lock order is written down.** Phase 04 records the global lock order
   in `docs/architecture.md` and a test or debug assertion holds it. Phase
   05 conforms to it.
3. M39's decisions 1–3 carry over.

**Exit criteria:**

- Every ledger row targeted at M42 reads `act: M42 phase-NN` or
  `reject: <why>`.
- With one chat session and four idle panes, the daemon spawns no `tmux`
  subprocess from a Tokio worker thread (a counted test seam or a
  `strace -f` capture in the live check), and no thread sleeps on one.
- Two concurrent memory-index writers both land (a test).
- `cargo test`, `tests/hygiene.rs` and `tests/doc_truth.rs` pass.
  `2.14.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/architecture.md` § 1.2 (orchestration and the supervised tasks)
  and § 1.4 (tmux integration and persistence).

## Phases

Drafted one at a time with `/rexymcp:architect next`. Each phase may split
at drafting. 02, 03 and 04 are the large ones and are likely to split.

| #  | Phase | Ledger | Status |
|----|-------|--------|--------|
| 01 | sqlite-wal — WAL, `synchronous=NORMAL`, `busy_timeout`; `open_and_reconcile_if_empty` drops its connection before reconciling, and a failed reconcile can retry | P2 | todo |
| 02 | event-log-writer — `log_event` and the FTS writes go through one background writer; `append_archive_message` takes its offset from the write, not a prior `metadata()` | P3 | todo |
| 03 | tmux-cache-offload — `SessionCache::refresh` runs off the workers and skips `capture_pane` for an unchanged pane; `get_pane_remote_host` reads the cache; `bounded_output` waits without spawning a thread pair per call | P1 | todo |
| 04 | lock-order-bus — `Bus::publish` stops holding `publish_lock` and `subscribers` across inline sink I/O; the documented lock order is enforced | P4 | todo |
| 05 | lock-scope-rest — `attach_brief_for`, `spend::LEDGER`, webhook dedup, `ShellRegistry` and `handle_set_pane` release their locks before disk I/O | P4 | todo |
| 06 | seek-and-caps — `read_line_at_offset` seeks; `PtyShell::run` scans only new bytes; `read_shell_log` caps before reading; the `ops*.rs` reads check `is_file` and a byte cap as `read.rs` does | P6 | todo |
| 07 | screen-scrollback — `scrollback_rows(n)` visits at most `n` rows under the mutex | P7 | todo |
| 08 | sbx-probe — the program-notices tick reads a sandbox shell's program status from the local backend without an `sbx exec` | P8 | todo |
| 09 | blocking-io-offload — prompt assembly and the status, shell-list and ask handlers read disk off the workers, each file once; `persist_final_turn_history` clones only when compacting | P9 P10 | todo |
| 10 | resource-leaks — bounded presence channel; no permanent reaper thread per shell; the `PtyBackend` pump holds a `Weak`; no sleep under the shell mutex; dead bus subscribers are pruned | P11 | todo |
| 11 | hot-loops — the missing indexes, a `RegexSet` pre-filter for masking, the Gemini short-circuit order, `SseBuffer` without quadratic drain, a cached hostname | P12 | todo |

## Notes

- **Scoped 2026-11-21** with M39. Phase 02 changes when an event is on
  disk, so readers that expect a record right after `log_event` returns
  (the catch-up brief, the notice journal and their tests) need a flush
  point. Settle it at drafting; it may be its own phase.
