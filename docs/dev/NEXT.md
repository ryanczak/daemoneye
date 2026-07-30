# NEXT

**Active phase: none — M5 is closed.**

M5 — UX & Stability is `done` as of 2026-07-30. All nine exit criteria met, all 46
phases approved, retrospective written in
`docs/dev/milestones/M5-ux-stability/README.md` § "M5 retrospective".

**This is a human gate.** The calibration folds are resolved (below); **M6 scope
is the one thing still open.**

## 1. Folds — one applied, one parked (PE decisions, 2026-07-30)

**Fold 2 is applied.** "Confirm the property is observable before pinning it" now
sits in `docs/dev/WORKFLOW.md` § "Coverage claims are inadmissible without mutation
proof". Three occurrences, all architect-authored.

**Fold 1 is parked as tooling, not prose** — see `docs/dev/TODO.md` § 1. The PE's
call was to explore a mechanical realisation later rather than add another paragraph;
the existing counting fold already stated the rule and already predicted that prose
would not be the remedy. Evidence and a design sketch are in the TODO.

The original proposals, retained for reference:

**a. A mechanical pre-dispatch criteria check.** Eight defective acceptance
criteria in M5, three of which cost a run (110 turns on 07, 60 on 11, plus 06n).
**Every one of the three `hard_fail`s in this milestone was a criterion the
executor could not satisfy or verify — never code it could not write.** The
existing counting fold's dated note already predicted the remedy: "if a sixth
occurs, the remedy is a *mechanical* pre-dispatch check, not stronger prose." We
reached eight. Proposed: run every criterion against the tree with the list
*final*, and treat an unrunnable or ambiguous criterion as a spec blocker.

**b. Observable-property discipline.** Three occurrences — the fold-immediately
bar. A test can satisfy a spec and prove nothing: 09's EOF test passed via a
different arm returning the same value; 10's ordering test passed on the alphabet
because `serde_json` discards insertion order. Proposed: when a spec pins a
property, confirm the property is *observable*; when it names a branch, give a
sequence that *reaches* it.

Full evidence for both is in the retrospective. Not proposed, held at one
occurrence: the warn-vs-error cascade distinction from 06r.

**Standing backlog:** `docs/dev/TODO.md` — cross-milestone items parked with their
evidence. Currently one entry (the criteria-check mechanisation).

## 2. M6 has no design and no scope

`docs/architecture.md`'s roadmap does not name an M6. Nothing is drafted, and
nothing should be until you say what the next capability is. Candidates visible
from M5's work, none chosen:

- **The `--console` teeing gap** — 11's Out-of-scope notes that lifecycle output
  under `--console` still goes to the terminal rather than `daemon.log`.
- **`src/cli/` has no concurrency** (established when 06k was dropped), so its 19
  tmux call sites are bounded but never off-runtime. Fine today; a constraint to
  remember if the CLI ever gains threads.
- **Old event segments carry no `pid`** — 10 deliberately did not backfill.

## Where things stand

- `cargo clippy --all-targets --all-features -- -D warnings` clean; **947** lib +
  **27** integration tests, zero failures.
- Working tree clean. `CLAUDE.md` carries four new invariants from phases 08–11 but
  is in `.gitignore` (untracked since `a793e4d`), so those live only in the working
  tree — worth knowing if the repo is ever cloned fresh.
- **No daemon is running.** Phases 09–11's E2E scenarios stopped it; nothing has
  restarted it. `daemoneye chat` (PID 559023) has been up without a daemon since
  early in the session.

## To proceed

Run `/rexymcp:architect` (no args) to scope M6 — it will survey and ask what the
next capability should be. Decide the two folds first, or explicitly defer them;
they are recorded and will not be lost.
