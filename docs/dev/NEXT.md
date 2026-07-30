# NEXT

**Active phase: 01 — test-isolation-harness.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-01-test-isolation-harness.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-01-test-isolation-harness`.

## What phase 01 does

Builds the environment every other M6 phase needs: a throwaway `HOME` **and** a
private tmux server, so an end-to-end scenario can run a real `daemoneye` daemon
without touching the operator's `~/.daemoneye/` or their default tmux server.

Deliverables: `tests/harness/mod.rs` (an `IsolatedEnv` type) and
`tests/isolation.rs` (three scenarios). No `src/` changes.

## The design question, and why it is already answered in the doc

M6's README flagged 01 as design-discovery, with the load-bearing unknown being
"how a private tmux server is addressed by every `Command::new("tmux")` in the
tree — there is no `-L` plumbing today."

**That is settled and pre-injected into the phase doc.** tmux resolves its server
socket from `$TMUX_TMPDIR`, and `std::process::Command` children inherit the
environment, so setting `TMUX_TMPDIR` on the spawned daemon gives all **82** call
sites a private server with **zero** changes under `src/`. Verified live during
drafting; the probe transcript is quoted in the phase doc. Plumbing `-L` is now
an explicit scope violation rather than an open question.

Two gotchas are pre-injected alongside it: the ~108-byte `sun_path` cap (hit
during drafting — the throwaway root must be under `/tmp`, not
`std::env::temp_dir()`, which honours a possibly-long `$TMPDIR`), and the fact
that `daemoneye daemon` forks, so the parent's exit status *is* the readiness
signal and the daemon outlives the test process.

## The one thing to look at before dispatching

Task 5 is a **required mutation**: remove `TMUX_TMPDIR` from the harness, watch
the suite fail, restore it, watch it pass, quote both. Per `WORKFLOW.md`
§ "Coverage claims are inadmissible without mutation proof" — a harness whose
isolation has never been demonstrated to fail is not evidence of isolation, and
this phase's entire deliverable is that evidence.

## Field note from 2026-07-30 — this problem is not hypothetical

While drafting, the operator's tmux server exited repeatedly and the daemon died
with it: `var/run/daemoneye.pid` and `daemoneye.sock` were both still on disk
with no `daemoneye` process alive, and `tmux ls` reported no server.

The mechanism is the one phase 01 contains. The daemon installs **four
server-wide `-g` hooks** — `pane-died`, `after-new-session`, `client-attached`,
`client-detached` (`src/daemon/mod.rs:563`–`:620`) — on whatever tmux server it
can reach, and those hooks keep firing `daemoneye notify …` at a socket that may
no longer be there. This is defect 13 in the inventory, observed live rather than
inferred. It is also the reason the architect is currently running outside tmux.

## Where things stand

- Working tree was clean at drafting; the only change is this file plus the new
  phase doc.
- No daemon currently running (see the field note above).
- `cargo clippy --all-targets --all-features -- -D warnings` clean at M5 close;
  **947** lib + **27** integration, zero failures — the baseline phase 01 must
  not regress.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 02–12 are
  named, not drafted; draft each with `/rexymcp:architect next` once its
  predecessor is `done`.
- Standing backlog: `docs/dev/TODO.md`.
