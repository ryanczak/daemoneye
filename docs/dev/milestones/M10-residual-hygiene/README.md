# M10 — Residual Hygiene

**Goal:** Clear the four carried items M7, M8 and M9 left behind. None is a
user-visible bug; each is a way the codebase can mislead someone later — a test
that hangs instead of failing, a sleep that pretends to be a wait, a hardcoded
table that will drift, and a shipped command the project docs never mention.

**Status:** planning

**Depends on:** M9 (Operator Tooling) — closed 2026-08-02.

**Scoped:** 2026-08-02, PE decision: "make 1–4 part of M10." The four items are
the top of the carried list from three consecutive retrospectives. Item 7
(`hooks_land_on_private_server`) is deliberately **excluded** — it has never
reproduced in 300 runs, so there is nothing to fix.

**Exit criteria:**

- [ ] **A regression that starves `read_key` fails the tty tests instead of
      hanging them.** Measured: the current suite **hangs indefinitely** in that
      case (verified by mutation — killed at 25 s, see Notes).
- [ ] **No real-clock `sleep` anywhere in the test suite**, including spawned
      tasks. This finishes what M8's exit criterion 3 left named-but-unfixed.
- [ ] **Every caller derives its memory directory names from `MemoryCategory`**
      rather than a hardcoded table. Scoping named only `epochs.rs`; drafting
      phase 02 found a **third** copy at `src/search.rs:56-63`, so the criterion
      was widened rather than leaving two of three fixed.
- [ ] **`daemoneye reindex` is documented** in `CLAUDE.md` and
      `docs/architecture.md`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
      fmt --all --check` clean; `cargo test` green with no regression against the
      **1035 lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
      bug_tracker + 1 doc_truth** baseline M9 closed at.

## Architecture references

- `src/cli/input/tty.rs:161` `read_key()` — the first `read_byte()` at `:164`
  has no timeout; every subsequent read does.
- `src/cli/commands/stream.rs:686,711` — the two production `select!` arms that
  await `read_key`, and the reason the unbounded wait is **correct** in
  production.
- `src/ai/mod.rs:364` — a 30 s `tokio::time::sleep` in a spawned task.
- `src/daemon/context/epochs.rs:618` — the hardcoded `(category, dir_name)` table.
- `src/memory.rs:17` `MemoryCategory::dir_name()` and `:27` `canonical_name()` —
  the accessors the table duplicates.

## Phases

| #  | Phase | Status |
|----|-------|--------|
| 01 | [read-key-test-bound](phase-01-read-key-test-bound.md) — bound `read_key` in the tty tests so starvation fails instead of hanging | done        |

| 02 | [derive-category-dirs](phase-02-derive-category-dirs.md) — derive the memory category dirs from `MemoryCategory` in three places; drop the last real-clock sleep | done        |

| 03 | [document-reindex](phase-03-document-reindex.md) — document `daemoneye reindex` in `CLAUDE.md` and `architecture.md`, and gate it against silent removal | review      |

**All three phases are drafted.** Phase 03 is the last in-scope phase.

## Notes

### The hang is real, and the obvious fix is wrong

Verified by mutation before scoping: stop the bytes reaching `read_key` in
`read_key_bare_cr_yields_enter` and the test does not fail — it **hangs**, killed
externally at 25 s. In CI that is worse than a failure, because a hang burns the
job's whole time budget and reports nothing useful.

The tempting fix is to give `read_key` a timeout. **That would be a bug.**
Production awaits `read_key` inside a `tokio::select!` (`stream.rs:686` and
`:711`) racing daemon messages and a tick; the unbounded wait for the first byte
is exactly how the chat loop waits for the user to type. A timeout there would
make `read_key` return spuriously, and since `None` already means EOF, the loop
could not tell "user is thinking" from "terminal closed."

So the bound belongs in the **tests**. The phase-01 spec says this explicitly,
because "`read_key` has no timeout" invites precisely the wrong repair.

### The trap in the guard test

A test that proves the bound fires must keep the pipe's **write end alive**.
Measured both ways:

| Write end | `timeout(50ms, read_key(&stdin))` |
|---|---|
| Held | `Err(Elapsed)` — the bound fires |
| Dropped | `Ok(None)` — EOF, returns immediately |

Binding it as `_write_file` keeps it alive to the end of the test; a bare `_`
drops it at once and the guard test passes for the wrong reason, proving nothing.
Pinned as a negative case in the phase spec.

### Why item 7 is excluded

`hooks_land_on_private_server` was the phase-04-review flake. It binds no ports
and has not failed once in 300 runs across M8 and M9. There is no evidence to
work from, so there is nothing to fix — it stays on the carried list as "only a
bug if it recurs."
