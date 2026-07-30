# NEXT

**Active phase: 02 — prompt-path-audit-test.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-02-prompt-path-audit-test.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-02`.

## What phase 02 does

Lands `src/config/path_audit.rs` — a path-literal extractor plus an explicit path
inventory — and a test that fails when the shipped prompt or knowledge memories
name a path that is wrong or superseded. Ships as **production code**, not
test-only, because phase 04's `daemoneye audit-prompts` must reuse it rather than
grow a second extractor.

No asset is fixed in this phase; that is phase 03. The deliverable is the gate
plus the evidence of what it caught.

## Two findings from drafting that changed the design

**1. The milestone's own exit criterion would have passed the motivating defect.**
It read: fail when a path literal "does not correspond to a path the `config`
module constructs." But `config::events_path()` still returns
`var/log/events.jsonl` and has 19 live call sites — `event_log.rs:93` binds it as
`legacy`. The path *resolves* and is still the wrong thing to put in front of the
agent, because writes go to dated segments now. **Existence is not the property;
current-vs-superseded is.** The criterion is restated in the README, and phase 02
audits against an inventory carrying a `Current` / `Legacy` status per path.

**2. The stale-path damage is six times what the inventory recorded.** Defect 5
said the knowledge memories "repeat the error twice". Extracting every backticked
path span across all seven files found an entire pre-`var/` generation still
shipping: `~/.daemoneye/config.toml`, `~/.daemoneye/daemon.log`,
`~/.daemoneye/events.jsonl`, `~/.daemoneye/pane_logs/`,
`~/.daemoneye/schedules.json`, `~/.daemoneye/sessions/…`. All six predate the
`var/` reorganisation, all six are in the file the prompt calls authoritative.
Defect 5 and phase 03's scope line are corrected in the README.

## What to look at before dispatching

- **The phase is expected to go red first.** Task 5 requires running the gate with
  no quarantine, quoting the failure, then adding a `PENDING_FIX` list containing
  exactly the literals that failure reported. `PENDING_FIX` must be non-empty at
  the end — an empty one means the extractor is broken, not that the assets are
  clean. Phase 03 empties it.
- **The extraction rule is the part most likely to go wrong**, so it is pinned
  with both populations spelled out. The trap: `assets/prompts/sre.toml` contains
  backticked spans like `/clear`, `/limits reset`, `/session save <name> [desc]`
  and `#!/usr/bin/env python3`. A "contains a slash" rule matches all of them and
  produces false failures. The spec pins a leading-segment allowlist and lists
  every must-NOT-extract span verbatim.

## Where things stand

- Phase 01 `done` (approved_after_1). `tests/harness/mod.rs` + `tests/isolation.rs`
  give every later phase a throwaway `HOME` and a private tmux server. Phase 02
  needs neither — it is a pure library-plus-unit-test phase.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 947 lib + 27
  integration (2 ignored, pre-existing) + 3 isolation, zero failures. Phase 02
  should raise the lib count and leave the other two untouched.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 03–12 named,
  not drafted. Re-verify each phase's "Current state" against the tree before
  dispatching — 01 changed `tests/`, and 02 will change `src/config/`.
- Standing backlog: `docs/dev/TODO.md` § 1, the pre-dispatch criteria check. Phase
  01's bounce (two architect-authored unobservable properties) and finding 1 above
  (an architect-authored criterion that could not catch its own motivating defect)
  are both arguments for building it. Worth deciding at the milestone close.
