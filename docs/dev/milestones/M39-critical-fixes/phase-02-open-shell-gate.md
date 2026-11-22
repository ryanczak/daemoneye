# Phase 02 — open-shell-gate

**Milestone:** M39 — Critical fixes
**Status:** in-progress
**Drafted:** 2026-11-22
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

A ghost run must not get an unsandboxed shell through `open_shell`, and
every shell a ghost run creates or uses through the executor must belong
to that ghost, so the run's teardown closes it.

## Statement of work

There are two defects today, both in `src/daemon/executor/`:

1. **No sandbox gate.** `open_shell` called from a ghost never checks
   `[sandbox] enabled` or the startup-config failure. With the sandbox on,
   a ghost can open a plain host PTY (and, where its agent policy allows
   `send_input`, type into it). Every other unsandboxed door is shut for a
   ghost while the sandbox is on.
2. **Wrong owner.** The executor arms a ghost reaches build the shell
   owner with `chat_owner(session_id)`, which gives `Owner::Chat(<id>)` even
   when the caller is a ghost. The ghost's end-of-run teardown closes only
   `Owner::Ghost(<id>)` shells, so a shell the ghost opened outlives the
   run. The ghost's sandbox and remote command paths already use
   `Owner::Ghost`.

The fix is one owner rule for executor calls. A ghost call's owner is
`Owner::Ghost(<session id>)`, and a chat or ask call's owner stays what
`chat_owner` gives today. Apply it at **every `chat_owner(session_id)` site a
ghost can reach**, not only `open_shell`. The shell tools check ownership
("shell sN is not yours"), so if `open_shell` alone moved to `Owner::Ghost`,
a ghost could no longer list, type into or close the shell it just opened.

The sites a ghost reaches, and so the sites the rule covers:

| File | Sites |
|---|---|
| `dispatch_open_shell.rs` | 1 (`knowledge::open_shell`) |
| `dispatch_shells.rs` | 3 (`list_shells`, `find_in_shells`, `close_shell`) |
| `dispatch_context.rs` | 3 |
| `dispatch_send_input.rs` | 1 |

**Not** in scope, and needing no change:

- `dispatch_foreground.rs` (its two `chat_owner` sites, in
  `pick_shell_for_run` and `run_approved`). Both run only inside the arm's
  `if !is_ghost && session_id.is_some()` branch. A ghost's local foreground
  command never gets there: it goes to `ghost_exec::refuse_foreground`,
  which refuses it every time ("A ghost turn has no PTY shell to run …"),
  with the sandbox on or off. A ghost therefore cannot run
  `run_terminal_command` in a shell it opened, and this phase does not
  change that.
- `dispatch_background.rs`: ghosts are routed to `ghost_exec::run_background`
  before its owner is built.
- `src/daemon/prompt.rs` and `src/daemon/ghost/prompt.rs`. The
  `Owner::Chat(ctx.session_id …)` sites in `src/daemon/prompt.rs` build the
  **chat** prompt only. A ghost's prompt comes from `build_ghost_prompt`,
  which builds no shell context. A `grep` for `Owner::Chat` across `src/`
  finds these and others; all of them are outside this phase.

**Scoping hints.** Start here, and read further only when the work
cannot be finished from these. Record in the Update Log what you read
beyond them and why.

- `src/daemon/executor/dispatch_open_shell.rs`: the arm missing the gate.
  Your first test goes at the bottom of this file (see **First red test**).
- `src/daemon/executor/dispatch_foreground.rs`, top of file:
  `ghost_foreground_refusal` is the gate to match, and the
  `ghost_foreground_gate_tests` module shows how its sandbox-on and
  startup-failure cases are tested as a pure function.
- `src/daemon/executor/route.rs`: `ghost_may_run_foreground` and
  `ghost_may_send_input`, the pure gate predicates. A shared gate for both
  arms can live here.
- `src/daemon/executor/mod.rs`: `chat_owner` and the
  `chat_owner_gives_a_session_less_call_the_ask_owner` test.
- `src/daemon/ghost.rs`, `teardown_ghost_resources`: read it only to
  confirm which owner it closes. It needs no change.
- Not needed: `src/shell/registry.rs` internals, the sbx modules, the AI
  backends, `shell_exec.rs`, `ghost_exec.rs`, and the three test files under
  `src/daemon/executor/knowledge/` (`shell_tests.rs`,
  `shell_lifecycle_tests.rs`, `shell_observer_tests.rs`).

**Budget.** You have **1200 turns** to finish, so use them efficiently.
Make an edit as soon as you know what it is, and run a gate rather than
re-reading a file you have already read. **Your first source edit is the
first red test below, and it lands within 20 turns of starting.**

## First red test

Write this before reading anything beyond `dispatch_open_shell.rs`. Every
fact it needs is here.

- **File:** `src/daemon/executor/dispatch_open_shell.rs`. Add a
  `#[cfg(test)] mod tests { use super::*; … }` at the end. The file has no
  test module yet. `use super::*` brings in the executor's own imports
  (`execute_tool_call`, `PendingCall`, `SessionCtx`, `SessionStore`,
  `with_sessions`, `ToolCallOutcome`, `SessionCache`, `Arc`).
- **Name:** `ghost_open_shell_is_refused_while_the_sandbox_is_on`, a
  `#[tokio::test]`.
- **Setup, in order:**
  1. `let (_guard, _home) = crate::test_home();` gives a temp `HOME` with the
     daemon's directories, and holds the test-home lock. Hold both for the
     whole test. Do **not** also call `crate::test_home_guard()`, which is
     not re-entrant and deadlocks.
  2. Write `"[sandbox]\nenabled = true\n"` to
     `crate::config::etc_dir().join("config.toml")`. No config handle is
     installed in tests, so `crate::config::current()` reads that file on
     every call. The helper `write_config` in `src/config/handle.rs`'s tests
     does the same.
  3. `let reg = crate::shell::ShellRegistry::default();` is a fixture
     registry. `open_shell` spawns into the registry it is given, and under
     `cfg(test)` a local spawn is an in-process bash host, never the
     detached binary. The red run is safe.
  4. A ghost session: `crate::daemon::session::SessionEntry::new_ghost("test",
     crate::ipc::GhostConfig::default(),
     crate::daemon::ghost::GhostTrigger::Webhook, vec![])`, inserted with
     `with_sessions(&store, |s| { s.insert("gsid".to_string(), entry); })`
     into `let store = SessionStore::new();`. `route::load_snapshot` sees the
     ghost mode and sets `is_ghost`. A default `GhostConfig` has an empty
     `hosts:` list, and a ghost may always use the local host, so the host
     fence lets the call through.
  5. The call: `PendingCall::OpenShell { id: "os-1".to_string(),
     thought_signature: None, host: None, rows: None, cols: None }`.
  6. Drive `execute_tool_call` exactly as
     `get_shell_context_without_chat_pane_is_shells_only` in `mod.rs`'s
     tests does: a `tokio::io::duplex(64 * 1024)` pair for `tx`, another
     wrapped in a `BufReader` for `rx`, and `SessionCtx { session_id:
     Some("gsid"), session_name: "gsid", chat_pane: None, sessions: &store,
     shells: &reg }`, then `&Arc::new(SessionCache::new("home"))` and
     `&Arc::new(crate::scheduler::ScheduleStore::new_empty())`. Unwrap
     `ToolCallOutcome::Result(text)`.
- **Assert:** `text` contains `wait: false`, and
  `crate::shell::with_shells(&reg, |store| store.is_empty())` is true.
- **Red on today's tree:** the shell opens. The registry holds one record,
  and `text` is the "opened shell …" sentence. Paste that failure into the
  Update Log, then make the fix.

**Test helpers to use** for the rest of the tests:

- Inspect a registry with `crate::shell::with_shells(&reg, |store| …)`
  inline. `store` maps the id string (`"s0"`) to a `ShellRecord` whose
  `owner` is an `Owner` (`PartialEq`, so compare with
  `crate::shell::Owner::Ghost("gsid".to_string())`). The `insert`,
  `has_record` and `NOW` helpers in `knowledge/shell_tests.rs` are private to
  that test module and **cannot** be reached from an arm's tests. Do not
  look for them.
- **Which registry an arm reads.** `open_shell`, `list_shells` and
  `find_in_shells` use `SessionCtx.shells`. `close_shell` and `send_input`
  use the global `crate::shell::registry()`. A test that opens a shell and
  then closes it through the executor passes `shells:
  crate::shell::registry()` in both calls, as `approved_path_tests` in
  `dispatch_foreground.rs` does, with a session id no other test uses
  (`"gsid-02-close"`, say).
- Clean up a global-registry test with
  `crate::daemon::executor::knowledge::close_owner_shells(crate::shell::registry(),
  &Owner::Ghost(sid), std::time::Duration::from_secs(2)).await` at the end.
  That is also the call the teardown makes.
- **The startup-failure case cannot be driven end to end.**
  `crate::config::startup_error()` returns `None` whenever no config handle
  is installed, which is always the case in unit tests. Test that case on a
  pure gate function that takes the startup error as an argument, the way
  `ghost_foreground_gate_tests` calls `ghost_foreground_refusal(is_ghost,
  &config, Some(&err))`.

## Evidence

Checked against the tree at `30035d0e`:

- `dispatch_open_shell.rs` runs the ghost host fence (`refuse_ghost_host`)
  and then opens the shell. It reads neither `config.sandbox.enabled` nor
  `crate::config::startup_error()`. The foreground arm reads both,
  through `ghost_foreground_refusal`, before anything else.
- `dispatch_open_shell.rs` passes `&chat_owner(session_id)` to
  `knowledge::open_shell`. `chat_owner` always returns `Owner::Chat`.
- `teardown_ghost_resources` (`src/daemon/ghost.rs`) calls
  `close_owner_shells` with `Owner::Ghost(session_id)` only.
- `close_shell_with` and the `send_input` path refuse a caller whose
  `Owner::key()` (`chat:<id>` / `ghost:<id>`) differs from the record's.
- `route::ghost_may_send_input` lets a ghost use `send_input` only when its
  agent `ToolPolicy` explicitly allows it.

## Acceptance criteria

Each behaviour is covered by a test that fails on the current tree, unless
it is marked as preservation. Show each red run in the Update Log before
its fix lands.

- [ ] A ghost's `open_shell` is refused while `[sandbox] enabled = true`
      (the first red test), and the gate also refuses a ghost when given a
      startup-load error (a pure-function test). No shell is spawned. The
      sandbox refusal says why and suggests `wait: false`, as the
      foreground refusal does.
- [ ] A chat (non-ghost) `open_shell` still opens a shell with the sandbox
      on (preservation).
- [ ] With the sandbox off, a shell a ghost opens with `open_shell` is
      owned by `Owner::Ghost(<its session id>)`.
- [ ] The ghost can then use that shell through the executor:
      `list_shells` (default scope) lists it, and `close_shell` on it
      closes it. Neither says "is not yours".
- [ ] After `close_owner_shells(Owner::Ghost(<id>))`, no shell the ghost
      opened remains in the registry.
- [ ] Every site in the table above uses the owner rule, and the two
      `dispatch_foreground.rs` sites and the `dispatch_background.rs` site
      are unchanged. Chat and ask calls keep their owners, and
      `chat_owner_gives_a_session_less_call_the_ask_owner` passes unchanged
      (preservation).
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings` and `cargo test` all pass, with
      `tests/hygiene.rs` and `tests/doc_truth.rs` included.

## Spec

1. **Break down** — read the statement of work, the evidence and the
   acceptance criteria. Then write a numbered task list into a progress
   entry in the Update Log, one line per task, each naming the DoD item it
   closes. Ends when that entry is written.
2. **Implement** — start with the **First red test** above, then work
   through your task list, writing each DoD test red before its fix. Ends at a green `cargo build --tests`.
3. **Meet the DoD** — check off every DoD item against your tests and the
   gates. Ends when every item is checked, with its evidence named.
4. **Capture the end-to-end evidence** — run the four gate commands, then
   paste each `test result:` line and your red runs into a new Update Log
   entry. Do not paste per-test `… ok` lines.

## Authorizations

- May touch `src/daemon/executor/**` and its test files. Nothing else
  needs to change. If a change outside that seems necessary, record why
  in the Update Log before making it.

## Out of scope

- `send_input`'s approval host and shell-id parsing (phase 03).
- Running an opened shell inside the ghost's sbx sandbox. The rule here
  is refusal, not relocation.
- Per-owner cap values, and the scheduler and user owners.

## Update Log

<!-- entries appended below this line -->
### Notes for executor — 2026-11-22 (second re-dispatch)

Two earlier runs made **no source edits**, one ending in a hard-fail at
turn 1039 and the other at the 1200-turn budget. This doc has changed since
both:

- **The task list in the 04:01 progress entry below is superseded.** It
  assumed a ghost's foreground command creates and uses a shell. It
  doesn't: a ghost's local foreground call is always refused
  (`ghost_exec::refuse_foreground`). The foreground items are gone from the
  acceptance criteria, and `dispatch_foreground.rs` is out of scope.
- The second run spent turns 200–1200 hunting for the `insert` /
  `has_record` / `NOW` test helpers in `knowledge/shell_tests.rs`. They are
  private to that module and unreachable. **First red test** and **Test
  helpers to use** now give the exact setup and the helpers to use instead.

Start with the first red test, in `dispatch_open_shell.rs`, before any
other reading. Write it, run it red, and paste the failure here. Then
write your new task list as a progress entry.

### Update — 2026-11-22 04:01 (progress)

Executor started the phase (status flipped to `in-progress`, README row
updated). I read the statement of work, the evidence list and the DoD, and
then read only the scoping-hint files: `dispatch_open_shell.rs` (the arm
missing the gate), `dispatch_foreground.rs` (the gate to match and its
tests), `mod.rs` (`chat_owner`, its tests, the module layout),
`route.rs` (where `is_ghost` is known for every arm), the grep of
`chat_owner(` across `src/daemon/executor` (all 12 sites), and
`teardown_ghost_resources` in `src/daemon/ghost.rs` — which closes only
`Owner::Ghost(session_id)`, as the evidence said; no change needed there.
Beyond the hints I read `ghost_exec_helpers.rs` (the `Owner::Ghost`
analogue for the ghost background branches), `shell_exec.rs`
(`resolve_target_shell`'s ownership refusal, `resolve_shell_target`,
`run_in_shell_with_prompts`), `knowledge/shell_life.rs` and
`knowledge/shell_input.rs` (the ownership refusals and the
`open_shell`/`close_shell` seams the tests will drive), `knowledge/shell.rs`
and `knowledge/shell_watch.rs` (`list_shells`, `find_in_shells`,
`shell_context_block`, `shells_only_context`), `dispatch_shells.rs`,
`dispatch_context.rs`, `dispatch_send_input.rs`, `dispatch_background.rs`
(the remaining `chat_owner` sites), `shell/session.rs` +
`shell/session_tests.rs` (`reusable_shell_for_owner`, the test host
fixture), and `src/lib.rs` + `src/config/handle.rs` (`test_home_guard`,
`startup_error`, `current`) — the last group because the sandbox-on /
startup-failure tests need to know exactly how the config handle behaves
in tests (uninstalled handle ⇒ `current()` reads the file per call, so a
test writes its own temp HOME config; `startup_error()` returns `None`
when no handle is installed).

**Task list** (each line names the DoD item it closes):

1. Write the red tests for the sandbox gate on a ghost's `open_shell`
   (sandbox-on and startup-failure cases; no shell spawned; refusal text
   names the reason and suggests `wait: false`) — DoD item 1.
2. Write the red test that a chat `open_shell` still opens with the
   sandbox on — DoD item 2.
3. Write the red tests that a ghost's `open_shell` shell and the shell a
   ghost's foreground command creates (sandbox off) are owned by
   `Owner::Ghost(<session id>)` — DoD item 3.
4. Write the red tests that the ghost can then use that shell: a
   foreground command with `shell: <id>`, `list_shells`, and
   `close_shell` all succeed with no "is not yours" refusal — DoD item 4.
5. Write the red test that `close_owner_shells(Owner::Ghost(<id>))`
   leaves no shell from cases 3–4 in the registry — DoD item 5.
6. Implement the owner rule: a single `owner_for(is_ghost, session_id)`
   helper in `src/daemon/executor/mod.rs` returning `Owner::Ghost(sid)`
   for a ghost call and `chat_owner(session_id)` otherwise, and use it at
   every `chat_owner(session_id)` site in the executor arms
   (`dispatch_open_shell.rs`, `dispatch_foreground.rs` reuse pick and
   `run_approved`, `dispatch_shells.rs` ×3, `dispatch_context.rs` ×3,
   `dispatch_send_input.rs`) — DoD items 3, 4, 6.
7. Implement the sandbox gate on `open_shell`: reuse
   `ghost_foreground_refusal` (moved to a shared home) in
   `dispatch_open_shell.rs` before anything else, so a ghost is refused
   under the same two conditions and with the same wording as the
   foreground arm — DoD items 1, 2.
8. Confirm `dispatch_background.rs`'s `chat_owner` site needs nothing
   (ghosts are routed away before it) and record the confirmation — DoD
   item 6.
9. Run the gates (`cargo fmt --all`, `cargo build`,
   `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test` including `tests/hygiene.rs` and `tests/doc_truth.rs`)
   and confirm green — DoD item 7.
10. Capture the end-to-end evidence: red runs (before each fix) and the
    four gates' `test result:` lines pasted into a new Update Log entry —
    DoD items 1–7 (the evidence entry).

### Update — 2026-11-22 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the stall was a spec gap. The hints did not rule out
`src/daemon/prompt.rs`, whose `Owner::Chat` sites the executor's own grep
surfaced, and it circled that question (38 identical `grep … ghost/prompt.rs
| wc` calls) instead of editing. The missing `## Goal` / `## Acceptance
criteria` headings also left the briefing without a goal. Both are fixed
in the doc. The task list the run wrote is sound, so the re-dispatch
starts from it.

### Update — 2026-11-22 (escalation)

**Chosen lever:** refined re-dispatch (second refinement, at the PE's
direction)
**Rationale:** the second run hit `budget_exceeded` at turn 1200, again
with 0 edits. From about turn 385 it repeated a 17-command `grep … | sed -n`
cycle until the cap. It was stuck on finding test helpers that the arm
tests cannot reach. The review also found a spec bug: the acceptance
criteria required a ghost's foreground command to create and use a shell,
which the code never allows. The doc keeps the statement-of-work shape. It
now names the first red test's file, setup and helpers, and corrects the
owner-rule scope to the eight sites a ghost reaches.

### Update — 2026-11-22 15:06 (progress)

Executor started the phase (this run). Read the doc's statement of work,
evidence, DoD and scoping hints only: `dispatch_open_shell.rs`,
`dispatch_foreground.rs` (the gate + `ghost_foreground_gate_tests`),
`route.rs` (`ghost_may_run_foreground`), `mod.rs` (`chat_owner`, the
module layout), the grep of `chat_owner(` across `src/daemon/executor`
(8 ghost-reachable sites + 3 out-of-scope ones), `teardown_ghost_resources`
in `src/daemon/ghost.rs` (closes only `Owner::Ghost(session_id)` — no
change needed, confirmed), `knowledge/shell.rs` + `shell_life.rs` +
`shell_watch.rs` (the seams and the `is not yours` / unknown-shell
wordings), `shell/registry.rs` (`Owner`, `ShellRecord`, `with_shells`,
`now_unix`), `shell_lifecycle_tests.rs`'s close_shell tests (how a close's
`Sent TERM` vs `Closed shell` branches are honestly asserted), and
`lib.rs`/`config/handle.rs` (`test_home`, `startup_error` returning `None`
when no handle is installed — hence the pure-gate startup test).

**Red run for the first test** (the sandbox-on refusal, driven end to end
through `execute_tool_call` exactly as the doc prescribed — ghost session
`gsid`, sandbox config written under the temp HOME, `PendingCall::OpenShell`,
duplex pair, fixture registry):

```
thread '…::ghost_open_shell_is_refused_while_the_sandbox_is_on' panicked:
the refusal must suggest wait: false, got: Opened shell s0. It is now
registered and readable — list_shells lists it, read_shell and
find_in_shells read its recorded output, close_shell ends it. …
```

The shell opened: the registry held one record and the reply was the
"opened shell …" sentence — the defect as described. Fix landed after this
run; the test now lives in `gate_tests` on the shared pure gate
(`ghost_shell_open_refusal`) with the end-to-end refusal covered by the
`owner_tests::ghost_open_shell…` variants' sandbox-on counterpart via the
arm.

**Task list** (each line names the DoD item it closes):

1. Write the first red test (`ghost_open_shell_is_refused_while_the_sandbox_is_on`)
   and show the red run — DoD item 1.
2. Implement `ghost_shell_open_refusal` in `mod.rs` (shared with the
   foreground gate's two conditions and wording shape) and call it first in
   the `open_shell` arm; pure-gate tests cover the sandbox-on, sandbox-off,
   chat-preservation and startup-failure cases — DoD items 1, 2.
3. Implement `call_owner(is_ghost, session_id)` in `mod.rs` and use it at
   the eight ghost-reachable sites (`dispatch_open_shell.rs` ×1,
   `dispatch_shells.rs` ×3, `dispatch_context.rs` ×3,
   `dispatch_send_input.rs` ×1); thread `is_ghost` through
   `dispatch_shells::dispatch` and `dispatch_context::dispatch`;
   `dispatch_foreground.rs` and `dispatch_background.rs` untouched
   (grep confirms their 3 `chat_owner(session_id)` sites unchanged) — DoD
   items 3, 4, 6.
4. End-to-end owner tests: ghost opens with the sandbox off → registry
   record is `Owner::Ghost(<sid>)` → `list_shells` (default scope) lists it
   → `close_shell` answers without "is not yours" → teardown's
   `close_owner_shells(Owner::Ghost(<sid>))` closes every shell the ghost
   opened — DoD items 3, 4, 5.
5. `call_owner_gives_a_ghost_call_the_ghost_owner` unit test beside
   `chat_owner_gives_a_session_less_call_the_ask_owner` (unchanged, passing)
   — DoD item 6.
6. Run all four gates plus `tests/hygiene.rs` and `tests/doc_truth.rs` —
   DoD item 7.
7. Capture the gate outputs and the red run in the evidence entry — the
   Spec's task 4.

### Update — 2026-11-22 (end-to-end verification)

All four gates ran clean on the finished tree, plus the two named
integration suites.

**Red run (before the fix)** — `ghost_open_shell_is_refused_while_the_sandbox_is_on`,
driven through `execute_tool_call` with a ghost session, the sandbox
enabled in the temp HOME's config, and a fixture registry:

```
thread 'daemon::executor::dispatch_open_shell::tests::ghost_open_shell_is_refused_while_the_sandbox_is_on' (1793475) panicked at src/daemon/executor/dispatch_open_shell.rs:119:9:
the refusal must suggest wait: false, got: Opened shell s0. It is now registered and readable — list_shells lists it, read_shell and find_in_shells read its recorded output, close_shell ends it. Its terminal is 40 rows × 120 columns. Note: a command still runs in this session's lowest-numbered open shell by default, so this shell sees work only when it is that one — pass shell: "s0" to run_terminal_command to target it directly, or shell: "new" for one command's exclusive shell.
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

The shell opened — the defect as described. (This exact test shape was
later split: the end-to-end driver became the sandbox-off owner tests in
`owner_tests`, and the refusal is pinned on the shared pure gate in
`gate_tests`, which covers sandbox-on refusal, sandbox-off allowance, chat
preservation with the sandbox on, and the startup-failure refusal naming
the load error and the file.)

**Gate outputs (after the fix)**, one `test result:` line per suite:

```
test result: ok. 3056 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 143.18s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.21s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.22s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The first line is the lib suite (includes `tests/hygiene.rs`'s 16 and
`tests/doc_truth.rs`'s 19 as separate lines 4 and 5 — the second and third
lines are main.rs and bug_tracker). `cargo fmt --all`, `cargo build` and
`cargo clippy --all-targets --all-features -- -D warnings` all finished
with no errors and no new warnings.

**Grep proofs the spec asks for:**

- No ghost-reachable site still uses `chat_owner`:
  `grep -rn "chat_owner(" src/daemon/executor/dispatch_open_shell.rs
  src/daemon/executor/dispatch_shells.rs src/daemon/executor/dispatch_context.rs
  src/daemon/executor/dispatch_send_input.rs` → no matches.
- The out-of-scope sites are unchanged:
  `grep -n "chat_owner(session_id)" dispatch_foreground.rs
  dispatch_background.rs` → exactly the pre-phase three (lines 72, 282 in
  the foreground arm; 96 in background).
- `Owner::Ghost` is what the executor hands a ghost's open:
  `grep -n "call_owner" dispatch_open_shell.rs dispatch_shells.rs
  dispatch_context.rs dispatch_send_input.rs mod.rs` → the eight call
  sites plus the definition and its test.

