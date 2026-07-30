# NEXT

**Active phase: 06b — webhook-to-ghost-e2e. NOT DRAFTED — blocked on a human
decision.**

The `/rexymcp:auto` loop stopped here. Phases 01–06a are `done`.

## The blocker

M6's headline exit criterion says the webhook→ghost pipeline must be **verified
end-to-end** by a scenario in the phase-01 harness: *"a generic payload with no
severity field reaches a ghost shell, and the run is observable in
`events/events-<date>.jsonl` as a `webhook_alert` followed by a `ghost_*`
lifecycle event."*

That scenario cannot currently be written without breaking `STANDARDS.md` §3.3.

**Why.** The ghost spawn is detached and its handle is discarded
(`src/webhook/process.rs:435`):

```rust
tokio::spawn(async move {
    … GhostManager::start_session_with_config(…) …   // logs `ghost_start` (ghost.rs:266)
});
```

So `process_alert` returns **before** any `ghost_*` event is logged, and there is
no join handle to await. The HTTP path is no better — `server.rs:68` spawns per
alert precisely so the POST can return 200 immediately. Observing a `ghost_*`
event therefore requires waiting on wall-clock time.

`STANDARDS.md` §3.3 forbids exactly that: *"Tests are deterministic: no `sleep`,
no real wall-clock time (inject a clock), no unseeded RNG. If a test can't be
made deterministic, mark it as ignored and explain why."* Phase 06a was just
bounced (bug-06a-1) for two sleeps under this same rule, so the standard is being
enforced, not merely stated.

**What is reachable deterministically.** `log_event("webhook_analysis", …)` fires
at `process.rs:399`, *before* the spawn at `:435`, carrying `ghost_trigger: true`
and `ghost_enabled: true`. So an in-process `block_on(process_alert(…))` test can
prove the whole chain up to the spawn decision — severity-less payload passes the
gate (phase 05), the runbook is found, the watchdog returned YES — but not the
spawn itself.

## Four ways out — your call

1. **Ignored E2E test.** Write the full scenario with a bounded wait and mark it
   `#[ignore]` with justification, per §3.3's own escape hatch. Repo precedent
   exists (2 integration tests are already ignored). **Cost:** the milestone's
   headline verification never runs in CI, which cuts against M6's thesis that
   "is this broken?" should be answered by running something.
2. **Relax §3.3** to permit a bounded await-for-condition (fail-loudly, with a
   timeout) in end-to-end tests specifically, distinct from a fixed-guess sleep.
   **Cost:** a contract-doc change, and it weakens a rule that just caught a real
   defect.
3. **Add a production test seam** so the spawn is observable/joinable — e.g. the
   spawn returns a handle the daemon retains, or emits a completion signal.
   **Cost:** production design change motivated by testability; also arguably the
   right fix, since a detached task with a discarded handle is untestable and
   unobservable in production too.
4. **Narrow the exit criterion** to the synchronous prefix: assert
   `webhook_alert` + `webhook_analysis{ghost_trigger:true}`, and treat the spawn
   as covered by existing unit tests. **Cost:** M6 would close without ever
   having observed a ghost start from a webhook — the exact gap the milestone was
   scoped to close.

I did not choose. Options 2, 3 and 4 change a contract doc, production design, or
the milestone's own headline criterion; option 1 quietly guts the criterion. All
four are human territory.

**My recommendation: option 3**, with 1 as a stopgap. The discarded handle is a
real observability defect in its own right — nothing in production can tell
whether a triggered ghost actually started — and fixing it makes the criterion
satisfiable as written rather than negotiating the criterion down.

## Where things stand

- Phases 01–06a `done`. 06a closed `approved_after_1`; its stub was
  mutation-verified twice (breaking the emitted token failed the test; moving the
  bind back inside the spawned task failed it 8/8), so 06b starts from a
  trustworthy instrument.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 964 lib + 30
  integration (2 ignored, pre-existing) + 7 isolation, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 07–12
  named, not drafted.

## Everything 06b will need once unblocked

Established while drafting, verified against source:

- **Runbook fixture:** `runbooks_dir()/<name>.md` with flat YAML frontmatter
  (`enabled: true`, `max_ghost_turns: 1`). `find_runbook_for_alert`
  (`process.rs:298`) tries kebab-case, lowercase, then exact — so alert
  `DiskFull` resolves to `disk-full.md`. If no runbook matches,
  `maybe_analyze_alert` returns early with only a debug log and the ghost never
  triggers.
- **Event chain:** `webhook_alert` (`process.rs:98`) → `webhook_analysis`
  (`:399`, carries `ghost_trigger`/`ghost_enabled`) → `ghost_start`
  (`ghost.rs:266`, with `session_id`/`alert_name`/`tmux_session`) →
  `ghost_complete` (`ghost.rs:1029`).
- **The stub answers every AI request**, so the same canned body serves both the
  watchdog call and the ghost's own turns. It must contain `GHOST_TRIGGER: YES`
  and no tool calls, and the fixture should cap `max_ghost_turns: 1`.
- **`check_ghost_capacity`** must pass — default `max_concurrent_ghosts` is 3, so
  a single ghost is fine.

## Carried forward for milestone close

- **A pre-existing `tokio::time::sleep` at `tests/integration.rs:615`** violates
  `STANDARDS.md` §3.3. It predates M6 and sits outside every phase's
  Authorizations, so two reviews correctly declined to touch it. Worth a decision
  alongside the §3.3 question above.

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
