# NEXT

**Active phase: none — M6 is scoped but no phase is drafted.**

M6 — Verification & Hygiene was scoped 2026-07-30. Milestone README:
`docs/dev/milestones/M6-verification-and-hygiene/README.md`. Nine phases are
**named, not drafted**, with a thirteen-item defect inventory behind them, all of it
verified against the tree or the live runtime during scoping.

**This is a human gate.** Review the milestone README — particularly the exit
criteria and the phase ordering — then draft phase 01 with
`/rexymcp:architect next`.

## Why this milestone exists

A live webhook→ghost-shell test produced three reported symptoms. **Two were
measurement errors and one was a real defect.** That ratio is the thesis: the system
was partly working, and the tooling could not tell the difference.

- The event log *was* written — to `events/events-<date>.jsonl`. The grep went to
  `var/log/events.jsonl`, dead since July 9.
- The `[Webhook Alert]` block *was* injected — three times, on disk.
- The ghost shell genuinely never fired: an alert with **no severity** ranks 0,
  the default threshold is `warning` (rank 2), and the gate discards it **with no
  log line and no event**.

And the reason the first conclusion was wrong is itself a defect: the agent's own
prompt names `var/log/events.jsonl` in the same sentence as the correct tool
(`search_repository(kind:"events")`, which reads the segments properly). The agent
followed the path.

## The four axes

1. **Test isolation** — throwaway `HOME` **and** a private tmux server. Phase 01,
   first because everything else needs it. Across M5, every real-artifact check
   disrupted the operator's live daemon and tmux hooks, and one scenario could not be
   re-run at review for that reason.
2. **Agent-belief accuracy** — a repo test that fails on any unresolvable path
   literal in the prompt and knowledge memories, then the fixes, then an operator
   `daemoneye audit-prompts`. **Report-only by PE constraint: auto-refresh is ruled
   out because it would clobber local prompt edits.**
3. **Pipeline correctness** — the severity gate, and then whatever the end-to-end
   scenario surfaces behind it. Everything downstream of that gate
   (`maybe_analyze_alert`, the `GHOST_TRIGGER` parse, `check_ghost_capacity`,
   `GhostManager::start_session`) has never run for a severity-less alert.
4. **Runtime-tree hygiene** — `daemon.log` is 25.8 MB with *no* rotation logic
   anywhere in `src/`; `pane_prefs.json` exists twice, one orphaned, with a doc
   comment pointing at the dead one; `lib/` is documented as holding SDK modules and
   has been empty since March.

## Not an M5 regression

Worth stating plainly since it was the initial hypothesis: `severity_rank` and the
gate were last touched in `3fde6cd` (2026-03-07), four months before M5 opened.
`ae4e833` (the phase-11 fork handshake) is exonerated, and M5's close stands.

## Two things I did not do

- **Did not draft phase 01.** Milestone boundaries are a human gate; the README is
  for your review first.
- **Did not fix any defect.** All thirteen are inventory, not work-in-progress. The
  severity gate is a two-line change and tempting, but it belongs in phase 05 behind
  the harness that can prove it.

## Where things stand

- `docs/architecture.md` § 5 updated: M4 and M5 moved to shipped, M6 recorded as
  active. It had still named M4 — the same drift class the milestone is about.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; **947** lib +
  **27** integration, zero failures.
- Working tree clean. `CLAUDE.md` is now tracked (`51dff3e`) and no longer ignored.
- **A daemon is running** (PID in `var/run/daemoneye.pid`, socket present) — you
  restarted it for the webhook test.
- Standing backlog: `docs/dev/TODO.md` — one entry, the pre-dispatch criteria-check
  mechanisation parked at the M5 close. M6 phase 02 is a narrower instance of the
  same idea and worth reading against it.
