# NEXT

**Active phase: 06a — e2e-harness-ai-stub.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-06a-e2e-harness-ai-stub.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-06a`.

## What phase 06a does

Gives the phase-01 harness the two things the webhook→ghost scenario needs and
does not have: a **canned-AI stub server** the daemon can be pointed at, and
**webhook plumbing** (a collision-free port plus a POST helper). It ships no
scenario — it ships the instrument and proves the instrument works. 06b writes
the scenario.

## The milestone's open design question for 06 is now closed

The README said 06 was genuinely design-discovery: *what does a passing scenario
assert on when the ghost's own behaviour depends on a live AI call?*

Answered at drafting, from source: `maybe_analyze_alert`
(`webhook/process.rs:349-354`) builds its client from
`model_entry.effective_base_url()`, and `ModelConfig.base_url` is an
`Option<String>` that takes precedence over the provider default
(`config/types.rs:586`, `:661`). **Pointing the test config at a local stub makes
the whole pipeline deterministic and offline** — no Rust-level mocking, no
network. The watchdog call is `use_tools=false` and its result is consumed as
plain `AiEvent::Token`s, so the stub only has to stream tokens; it never needs to
support tool calls.

Two further constraints found while drafting:

- **The webhook listener binds eagerly and a bind failure is fatal**
  (`CLAUDE.md`). An isolated daemon asking for the default 9393 will fail to
  start whenever the operator's own daemon holds it, so every `IsolatedEnv` needs
  its own free port.
- **No new dependency is required.** `axum` and `tokio` are in `[dependencies]`,
  which Cargo makes available to test targets. Adding a dependency would be a
  blocker; the phase says so explicitly.

## Why 06 was split

The scenario needs a stub server, free-port allocation, config plumbing, a
runbook fixture *and* the assertion — more than one executor session
(`WORKFLOW.md` § Phases). 06a is the infrastructure; 06b is the assertion. The
split also means that if 06b's scenario fails, the instrument is already proven,
so the failure localises.

## What to look at before dispatching

- **06a's key acceptance criterion is that the stub is proven without the
  daemon**: a test drives `make_client(...)` directly — the same four arguments
  `maybe_analyze_alert` passes — and asserts the concatenated token text equals
  the canned string. Without that, a 06b failure is ambiguous between a broken
  stub and a broken scenario.
- **`maybe_analyze_alert` has a gate 06b will need to satisfy**: it returns early
  when no runbook name-matches the alert (`find_runbook_for_alert`, debug-log
  only). 06b's fixture must supply a matching runbook with ghosts enabled.

## Where things stand

- Phases 01–05 `done`. 05 closed `approved_after_1`; its code was
  mutation-verified at review (breaking the fail-open arm failed exactly the two
  tests that should fail).
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 964 lib + 30
  integration (2 ignored, pre-existing) + 3 isolation, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 06b–12
  named, not drafted. Re-verify each phase's "Current state" against the tree
  before dispatching.

## Carried forward for milestone close

- **A second E2E-transcript fold is worth considering.** The first fold
  (`STANDARDS.md` §1 capture box + `WORKFLOW.md` step 4) is working — it caught
  bug-04-4, where a real `diff` prints nothing on identical input so
  `"(empty - no changes)"` could only have been hand-typed. But the requirement
  still failed on phase 05, and the likely structural cause is that the
  **server-authored `(complete)` entry** carries a "Command output tails" block
  that looks like captured evidence while being the standard gate capture every
  phase gets. Naming that explicitly in the refinement unblocked the executor
  immediately. Candidate fold: the E2E block must be a distinct executor-authored
  entry, and the server-authored gate tails do not satisfy it.

- **`.gitignore` has no `.daemoneye/` entry.** A full seeded 168K runtime tree
  was found untracked in the repo root during phase 04 and had to be moved out
  before it was committed. Two reviews recommended the entry; both correctly
  declined to make it, as it sits outside any phase's Authorizations. It is
  milestone housekeeping.
- **`src/main.rs:17` and `:30`** still document the daemon log as
  `~/.daemoneye/daemon.log`; the real path is `var/log/daemon.log`. Same drift
  class as the prompt defect, in CLI help text the asset gate does not cover.
  Noted for phase 11.
- **The dedup log-level narrowing** described above.
