# Phase 06b: Webhook → Ghost End-to-End

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-01 (done), phase-05 (done), phase-06a (done)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=test, size=m

## Goal

Prove the webhook→ghost pipeline by running it: a payload with **no severity
field** reaches a ghost shell, and the run is observable in the event log as
`webhook_alert` → `webhook_analysis{ghost_trigger:true}` → `ghost_start`.

Two deliverables, because one of them is a production fix:

1. **A seam that makes the ghost spawn observable** — today it is fire-and-forget
   and nothing, in tests *or in production*, can tell whether a triggered ghost
   actually started.
2. **The scenario**: a deterministic test that asserts the full chain, plus an
   `#[ignore]`d full-daemon HTTP variant as a stopgap.

## Architecture references

Read before starting:

- `src/webhook/process.rs:409-470` — the spawn you are making observable.
- `src/daemon/ghost.rs:170-272` — `start_session_with_config`, which logs
  `ghost_start`.
- `tests/harness/mod.rs` — phase 06a's `IsolatedEnv`: canned-AI stub, free
  webhook port, `post_webhook()`. **The stub was mutation-verified twice at
  review — trust it.**
- `docs/dev/STANDARDS.md` §3.3 — the determinism rule that shapes this phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read `src/webhook/process.rs` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 964 lib / 30
   integration (2 ignored) / 7 isolation.

## Current state — why this phase needs a production change

**The ghost spawn is detached and its handle is discarded**
(`src/webhook/process.rs:435`):

```rust
tokio::spawn(async move {
    … GhostManager::start_session_with_config(…) …   // logs `ghost_start`
    … trigger_ghost_turn(…) …
});
```

`process_alert` therefore returns **before** any `ghost_*` event is logged, and
there is no handle to await. The HTTP path is no better: `server.rs:68` spawns
per alert precisely so the POST can return 200 immediately.

So observing a `ghost_*` event requires waiting on wall-clock time — which
`STANDARDS.md` §3.3 forbids (*"no `sleep`, no real wall-clock time"*). Phase 06a
was bounced for exactly that, so the rule is enforced, not decorative.

**This was escalated and the PE chose the seam** (option 3, with an ignored test
as stopgap). The discarded handle is a real observability defect on its own
terms: nothing in production can answer "did that alert's ghost actually start,
and when did it finish?"

**What is already reachable deterministically:** `log_event("webhook_analysis", …)`
fires at `:399`, *before* the spawn, carrying `ghost_trigger` and `ghost_enabled`.

## Spec

### 1. The seam — make the spawn observable

Change `process_alert` to return the spawned ghost task's handle rather than
discarding it. Shape:

```rust
pub async fn process_alert(
    alert: InternalAlert,
    state: Arc<WebhookState>,
) -> Option<tokio::task::JoinHandle<()>>
```

`Some(handle)` when a ghost was spawned; `None` on every other path (gate
discard, no runbook, ghost disabled, capacity reached).

Keep the spawn a spawn — **production behaviour must not change.** The HTTP
handler must still return 200 without waiting. Update the caller in
`src/webhook/server.rs` to bind and drop the handle with a one-line comment
saying why it is not awaited there.

Extracting the spawn body into a named helper first is fine if it reads better;
that is your call.

**Do not** add a task registry, cancellation, shutdown-join, or stats plumbing.
Returning the handle is the whole change — anything more is scope creep and a
later decision.

### 2. The deterministic scenario

An integration test that drives the real path in-process and **awaits the
returned handle**, so no wall-clock waiting is involved:

1. Isolated `HOME` and a **private tmux server** (see the hazard below).
2. A runbook fixture, and a stub AI whose canned body triggers the ghost.
3. Build a `WebhookState`, parse a **severity-less** payload, and
   `block_on(process_alert(...))`.
4. `await` the returned `JoinHandle`.
5. Read the event segment and assert the chain.

**Assert all three, searching the whole segment — never `lines().last()`:**

- a `webhook_alert` record;
- a `webhook_analysis` record with `ghost_trigger == true` and
  `ghost_enabled == true`;
- a `ghost_start` record whose `alert_name` matches the runbook.

The middle assertion is what proves phase 05's fail-open actually carried a
severity-less alert through the gate — the milestone's motivating defect.

### 3. ⚠ The isolation hazard — read this before writing the test

`start_session_with_config` calls `ensure_incident_session()`
(`src/tmux/session.rs:287`), which shells out to tmux and, finding no session,
**creates one**. `std::process::Command` children inherit the parent's
environment, so an in-process test that does not set `TMUX_TMPDIR` **will create
a session on the operator's live tmux server.**

That is the precise failure M6 defect 13 is about and phase 01 exists to prevent.

So the test must set **both** `HOME` and `TMUX_TMPDIR` in its own process,
under `crate::test_home_guard()` (`src/lib.rs:45`) — not the raw
`TEST_HOME_LOCK` (`:32`), which poisons every later HOME-dependent test once one
fails. Edition 2024, so `std::env::set_var` needs `unsafe`. Hold the guard
through **all** environment-dependent work and drop it at the end; a phase-04 bug
was filed for dropping it early. Restore both variables afterwards.

**Prove the isolation, do not assume it.** Add an assertion that the operator's
default tmux server is unaffected — phase 01's `default_server_unchanged`
(`tests/isolation.rs`) is the worked example for how to check that.

### 4. The stopgap — an ignored full-daemon variant

A second test that exercises the real HTTP path: `start_daemon()`, `start_stub()`,
`post_webhook()` with the severity-less payload, then wait for `ghost_start` to
appear in the event segment.

This one **cannot** be deterministic — the daemon spawns per alert so the 200
carries no completion signal. Mark it `#[ignore]` and put §3.3's required
justification in a comment on the test: why it is ignored, what it covers that
the deterministic test does not, and how to run it (`cargo test -- --ignored`).

Bound the wait so a failure fails loudly rather than hanging.

### 5. Fixture requirements (verified against source)

- **Runbook:** `runbooks_dir()/<name>.md`, flat YAML frontmatter with
  `enabled: true` and `max_ghost_turns: 1`. `find_runbook_for_alert`
  (`process.rs:298`) tries kebab-case, then lowercase, then exact — so an alert
  named `DiskFull` resolves to `disk-full.md`. **If no runbook matches,
  `maybe_analyze_alert` returns early with only a debug log and no ghost is ever
  triggered** — a silent no-op that will look like a broken test.
- **Canned AI body:** the stub answers *every* request, so the same body serves
  both the watchdog call and the ghost's own turn. It must contain
  `GHOST_TRIGGER: YES` and no tool calls. `max_ghost_turns: 1` bounds the loop.
- **Capacity:** `check_ghost_capacity` must pass; the default
  `max_concurrent_ghosts` is 3, so one ghost is fine.

## Acceptance criteria

- [ ] `process_alert` returns `Some(handle)` when it spawns a ghost and `None`
      otherwise.
- [ ] `src/webhook/server.rs` still returns 200 without awaiting it.
- [ ] A deterministic test asserts `webhook_alert`, then `webhook_analysis` with
      `ghost_trigger == true`, then `ghost_start` — from a payload with **no
      severity field**.
- [ ] That test awaits the returned handle; it contains no `sleep`, no polling,
      and no wall-clock wait.
- [ ] It sets `TMUX_TMPDIR` and asserts the operator's default tmux server is
      unchanged.
- [ ] An `#[ignore]`d full-daemon HTTP variant exists, with a comment giving
      §3.3's required justification.
- [ ] Phase 01's three isolation tests and phase 06a's four still pass unchanged.
- [ ] All four gates green.

## Test plan

Place tests wherever they read best (`tests/integration.rs` alongside phase 05's
webhook tests, or `tests/isolation.rs` alongside the harness ones). They must run
under plain `cargo test` with no special flags and **no network**.

**Mutation-check the headline assertion before reporting.** Break the fail-open
arm in `severity_rank`/the gate so a severity-less alert is discarded, confirm the
deterministic test **fails**, revert, and state the result in the Update Log. A
scenario that passes when the pipeline is broken is worth nothing, and this is the
milestone's headline verification.

**Do not pin a test count in advance.** Report the resulting count and explain the
delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the file's contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase receives automatically. **It does not satisfy
this requirement.** This has been the most common defect on this milestone.

Capture at least:

```sh
cargo test --test integration -- --nocapture \
  > /tmp/e2e-06b.txt 2>&1; echo "exit=$?" >> /tmp/e2e-06b.txt

grep -n "ghost_start\|webhook_analysis\|ghost" /tmp/e2e-06b.txt \
  > /tmp/e2e-06b-grep.txt 2>&1; echo "grep-exit=$?" >> /tmp/e2e-06b-grep.txt
```

Also paste the **actual `ghost_start` JSONL record** your test read out of the
event segment — this phase finally can produce one, which phase 05 could not.
And paste the mutation-check transcript from the Test plan.

## Authorizations

- [ ] May change `process_alert`'s return type in `src/webhook/process.rs` and
      update its caller in `src/webhook/server.rs`.
- [ ] May add tests to `tests/integration.rs` and/or `tests/isolation.rs`, and
      may add helpers to `tests/harness/mod.rs`.
- [ ] May mark **one** test `#[ignore]` — the stopgap in task 4, and only with
      §3.3's justification comment.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not change `STANDARDS.md` §3.3**, or any contract doc. The determinism
  rule stands; this phase works within it.
- **Do not add a ghost task registry, cancellation, shutdown-join, or stats
  plumbing.** Returning the handle is the entire production change.
- **Do not change ghost internals** — `start_session_with_config`,
  `trigger_ghost_turn`, `check_ghost_capacity`, the watchdog prompt, or
  `parse_ghost_trigger`.
- **Do not change the severity gate or `webhook_discarded`.** Phase 05 shipped
  them and they are mutation-verified.
- **Do not fix the pre-existing `tokio::time::sleep` at
  `tests/integration.rs:615`.** It predates M6 and is milestone housekeeping.
- **Do not touch `.gitignore`, `src/pane_prefs.rs`, or `main.rs`'s stale
  `daemon.log` help strings.** Phase 11 and housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 (escalation)

**Chosen lever:** resume (`continue_phase`)

**Rationale:** The executor wrote the whole phase and then stalled re-reading
files, hard-failing on `NoProgressStall` at turn 234 before running a single
gate. The spec was not the problem and the work is not lost — the architect
verified the partial tree directly: it **builds**, and
`webhook_ghost_e2e_deterministic` **passes**, asserting the full chain
`webhook_alert` → `webhook_analysis{ghost_trigger,ghost_enabled}` →
`ghost_start{alert_name}` plus an unchanged default tmux server. The operator's
live tmux server was **not** polluted, so the phase's headline hazard did not
fire.

Three defects remain, all found by the architect after the stall: `cargo clippy`
fails (`mod harness;` in `tests/integration.rs` compiles the harness into a
binary that uses none of `webhook_port` / `stub_port` / `tmux` / `default_tmux` /
`stop_daemon`, so `-D warnings` turns dead_code into an error); the test
hand-rolls its own `TcpListener` + `std::thread` SSE stub instead of phase 06a's
mutation-verified one; and it silently `return`s when tmux is missing, which
would let it pass vacuously. Takeover was rejected — this is assist 1 of 3, the
work is sound, and a fresh context is what the stall calls for.
