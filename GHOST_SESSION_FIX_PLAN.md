# Ghost Session Fix & Improvement Plan

**Author:** Claude Code
**Date:** 2026-03-22
**Status:** Awaiting review

---

## Background

Ghost Sessions are autonomous AI agents triggered when a webhook alert fires against a runbook
that has `ghost_config.enabled: true`. They are supposed to investigate and remediate incidents
headlessly, running commands in `de-incident-*` tmux windows.

**They do not work.** This plan fixes the root-cause bugs (Phase 1), hardens the pipeline
(Phase 2), and improves observability and UX (Phase 3). Each phase ends with a commit.

---

## Bug Summary (Priority Order)

| # | Severity | Location | Description |
|---|----------|----------|-------------|
| B1 | Critical | `ghost.rs:37` | Initial message has `role: "assistant"` — Anthropic rejects it, causing a deadlock hang |
| B2 | Critical | `server.rs:160` | `ai_tx` channel held across loop iterations — if AI call fails, `ai_rx.recv()` hangs forever |
| B3 | High | `webhook.rs:541` | `find_runbook_for_alert` returns `None` silently — no log, impossible to diagnose |
| B4 | High | `webhook.rs:577` | ALERT trigger requires the exact word "ALERT" in AI response — extremely brittle |
| B5 | Medium | `ghost.rs:18` | Comment claims a window is created in `start_session()` — it isn't |
| B6 | Medium | `server.rs:143` | Dead duplex `_rx_duplex` dropped immediately — if `ghost_policy` is absent, `prompt_and_await_approval` waits on a closed channel causing EOF errors |

---

## Phase 1 — Critical Bug Fixes (make ghost sessions work at all)

**Goal:** Fix B1 and B2. These two bugs prevent ghost sessions from running entirely.

### 1.1 Fix initial message role (B1)

**File:** `src/daemon/ghost.rs`

**Problem:** `start_session()` pushes an `{ role: "assistant", ... }` message as the first entry
in `SessionEntry.messages`. When `trigger_ghost_turn()` passes this to `client.chat()` as the
conversation history, Anthropic rejects the request (messages must begin with `role: "user"`).
The spawned task logs the error and exits, dropping `ai_tx_clone`. But the outer `ai_tx` stays
alive, so the `while let Some(ev) = ai_rx.recv()` loop in `trigger_ghost_turn` hangs
indefinitely. The ghost turn never completes and "Ghost Turn: failed for..." is never logged.

**Fix:**
- Change the initial message `role` from `"assistant"` to `"user"`.
- The alert context and ghost instructions should be a user-turn prompt, not a synthetic
  assistant message. Move the ghost behavioral notes (autonomous mode, background-only, etc.)
  into the `system` string that `trigger_ghost_turn` already constructs at line 135, so they
  are part of the system prompt rather than polluting the conversation history.
- The initial user message should simply be the alert payload: `"Incoming alert:\n{alert_msg}"`.

**Before:**
```rust
let system_msg = Message {
    role: "assistant".to_string(),
    content: format!(
        "[System] You are operating in an unattended Ghost Session responding to: {}\n\n\
         Investigate and remediate autonomously...",
        alert_msg
    ),
    ...
};
```

**After:**
```rust
let user_msg = Message {
    role: "user".to_string(),
    content: format!("Incoming alert:\n{}", alert_msg),
    tool_calls: None,
    tool_results: None,
};
```

The ghost behavioral instructions (autonomous mode, background-only, no user present) move
into the `system` string in `trigger_ghost_turn`, appended to `system_base`.

### 1.2 Fix AI event channel lifecycle (B2)

**File:** `src/daemon/server.rs` — `trigger_ghost_turn()`

**Problem:** A single `(ai_tx, ai_rx)` pair is created before the loop. Each iteration clones
`ai_tx` into a spawned task. The outer `ai_tx` stays alive across all iterations. If
`client.chat()` returns `Err` without sending `AiEvent::Done` or `AiEvent::Error`, the spawned
task exits (dropping `ai_tx_clone`) but the channel remains open because `ai_tx` is still held.
`ai_rx.recv()` waits forever.

**Fix:**
- Create a fresh `(ai_tx, ai_rx)` pair **inside** each loop iteration, not before the loop.
- Drop `ai_tx` immediately after cloning into the spawn (`drop(ai_tx)`) so the channel
  closes as soon as the spawned task exits — `ai_rx.recv()` then returns `None` and the
  `while let` terminates cleanly.
- Add a timeout guard on the inner `while let` loop (e.g., 5-minute wall-clock limit per turn)
  so a silent channel never hangs the ghost indefinitely.

**Sketch:**
```rust
loop {
    let chat_messages = { /* ... */ };
    let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
    let ai_tx_clone = ai_tx;          // move the only sender into the task
    tokio::spawn(async move {
        if let Err(e) = client_clone.chat(&system_clone, chat_messages, ai_tx_clone).await {
            log::error!("Ghost Session AI error: {}", e);
        }
        // ai_tx_clone dropped here → channel closes → recv() returns None
    });

    // Timeout: 5 min per turn
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, ai_rx.recv()).await {
        /* ... */
    }
    /* ... */
}
```

### 1.3 Fix dead duplex / EOF on missing ghost policy (B6)

**File:** `src/daemon/executor.rs` — `execute_tool_call()`

**Problem:** `trigger_ghost_turn` passes a dead duplex as `tx`/`rx`. If `ghost_policy` is `None`
(edge case: session exists but `ghost_config` is `None` even though `is_ghost = true`),
`prompt_and_await_approval` sends a `ToolCallPrompt` to the dead tx and then reads from `rx`,
which returns `EOF`. This propagates as `Err("EOF")` through `?` in `trigger_ghost_turn`.

**Fix:**
- Add a guard in `execute_tool_call`: if `is_ghost` is true but `ghost_policy` is `None`,
  return `ToolCallOutcome::Result("Error: ghost session has no policy configured")` instead
  of falling through to the human-approval path.
- This is a defensive check; B1's fix means ghost sessions will have valid policies, but the
  guard prevents silent hangs if the invariant breaks.

### 1.4 Fix misleading comment (B5)

**File:** `src/daemon/ghost.rs:18`

Remove or correct the comment `"2. Creates a dedicated de-incident-* window."` — no window is
created in `start_session()`. Windows are created lazily per background command in
`run_background_in_window()`.

### Phase 1 — Deliverables
- [ ] `src/daemon/ghost.rs`: fix role, move instructions to system prompt
- [ ] `src/daemon/server.rs`: fix channel lifecycle, add per-turn timeout
- [ ] `src/daemon/executor.rs`: add ghost-without-policy guard
- [ ] Tests: add a unit test that verifies ghost `SessionEntry.messages[0].role == "user"`
- [ ] Tests: verify `trigger_ghost_turn` returns `Ok(())` when AI errors without hanging
- **Commit:** `Fix: ghost session role bug, channel lifecycle, and EOF guard`

---

## Phase 2 — Pipeline Hardening (make ghost sessions reliable)

**Goal:** Fix B3 and B4. Add logging, make the ALERT trigger robust, add a turn cap,
add session-existence validation before tool execution.

### 2.1 Log when no runbook matches (B3)

**File:** `src/webhook.rs` — `maybe_analyze_alert()`

```rust
let Some(rb) = find_runbook_for_alert(&alert.alert_name) else {
    log::debug!(
        "Webhook: no runbook found for alert '{}' (tried kebab, lowercase, exact)",
        alert.alert_name
    );
    return;
};
```

### 2.2 Replace brittle ALERT word-check with structured AI output (B4)

**File:** `src/webhook.rs` — `maybe_analyze_alert()`

**Problem:** `response.to_uppercase().contains("ALERT")` fails if the model describes a
critical condition without using the word "alert". The check is also structurally wrong —
a ghost session should fire whenever the analysis determines the runbook applies, not only
when the AI happens to say "ALERT".

**Fix (two options; option A is simpler):**

**Option A — Explicit trigger field**: Ask the AI to output a structured prefix on the last
line: `GHOST_TRIGGER: YES` or `GHOST_TRIGGER: NO`. Update `watchdog_system_prompt()` in
`runbook.rs` to instruct the model accordingly. Parse only that line. This avoids JSON
parsing and is easy to prompt for.

**Option B — Separate trigger call**: Add a second, cheaper AI call: `"Based on the above
analysis, should a Ghost Session be triggered? Answer YES or NO only."` This is cleaner but
doubles the AI calls.

**Recommendation:** Option A. Update `watchdog_system_prompt()` and the ALERT check.

### 2.3 Add max-turn limit

**File:** `src/daemon/server.rs` — `trigger_ghost_turn()`

Add a `MAX_GHOST_TURNS: usize = 20` constant. If the loop exceeds this, break and log
a warning. Prevents runaway billing when every tool call is denied or the AI loops on error.

```rust
const MAX_GHOST_TURNS: usize = 20;
let mut turn = 0;
loop {
    if turn >= MAX_GHOST_TURNS {
        log::warn!("Ghost Session {}: reached max turns ({}), stopping", session_id, MAX_GHOST_TURNS);
        break;
    }
    turn += 1;
    /* ... */
}
```

### 2.4 Validate tmux session exists before executing tools

**File:** `src/daemon/server.rs` — `trigger_ghost_turn()`

After loading `tmux_session` from the session entry, verify it still exists:

```rust
if !crate::tmux::session_exists(&tmux_session) {
    anyhow::bail!(
        "Ghost Session {}: tmux session '{}' no longer exists",
        session_id, tmux_session
    );
}
```

This requires a `session_exists(name: &str) -> bool` helper in `src/tmux/session.rs` (check
if `tmux has-session -t name` exits 0). The function `list_sessions()` already exists;
`session_exists` can be a thin wrapper.

### 2.5 Log ghost session denial counts

**File:** `src/daemon/server.rs` — `trigger_ghost_turn()`

Track per-session denial count. If all tool calls in a turn are denied, log a warning so the
user knows why the ghost session "completed" without doing anything:

```rust
if tool_results.iter().all(|r| r.content.starts_with("Command denied by Ghost Policy")) {
    log::warn!(
        "Ghost Session {}: all {} tool calls denied by policy — runbook may need \
         auto_approve_scripts or auto_approve_read_only: true",
        session_id, tool_results.len()
    );
}
```

### Phase 2 — Deliverables
- [ ] `src/webhook.rs`: log no-runbook case; replace ALERT word check with GHOST_TRIGGER field
- [ ] `src/runbook.rs` (`watchdog_system_prompt()`): update system prompt to emit `GHOST_TRIGGER: YES/NO`
- [ ] `src/daemon/server.rs`: add `MAX_GHOST_TURNS` cap; add session-exists validation
- [ ] `src/tmux/session.rs`: add `session_exists(name: &str) -> bool`
- [ ] `src/daemon/server.rs`: log all-denied-turn warning
- [ ] Tests: test `find_runbook_for_alert` returns `None` for unknown alert name (covered implicitly; ensure log appears)
- [ ] Tests: test that `watchdog_system_prompt()` output includes `GHOST_TRIGGER` instruction
- **Commit:** `Harden ghost session pipeline: trigger reliability, turn cap, session validation`

---

## Phase 3 — Observability & UX Improvements

**Goal:** Surface ghost session lifecycle to users; improve status command; make ghost
session easier to configure and debug.

### 3.1 Notify users when ghost session starts and completes

**File:** `src/webhook.rs` and `src/daemon/server.rs`

- On ghost session **start**: call `notify_chat_panes(&state.sessions, "Ghost Session started for: {alert_name}")`.
- On ghost session **completion** (`trigger_ghost_turn` returns `Ok(())`): inject a summary
  message into the session history so it appears in the N15 catch-up brief. Also call
  `notify_chat_panes` with a one-liner.
- On ghost session **failure**: notify with the error cause.

These notifications mean the user sees what the ghost did even if they were away.

### 3.2 Surface ghost session state in `daemoneye status`

**Files:** `src/ipc.rs`, `src/daemon/server.rs`, `src/cli/commands.rs`

Add ghost session info to `Response::DaemonStatus`:
- Count of active ghost sessions
- Count of completed/failed ghost sessions (from `stats`)
- Names of currently-running ghost sessions (session_id list)

Update `run_status()` in `cli/commands.rs` to display this in the status table.

### 3.3 Add `daemoneye ghost list` / `ghost status` subcommand (optional, stretch goal)

A new `ghost` subcommand (or `status --verbose`) that lists all ghost session entries from
the sessions map, their current state (running/completed), and recent tool call results.

### 3.4 Improve ghost session context in `trigger_ghost_turn`

**File:** `src/daemon/server.rs`

Currently the ghost system prompt suffix is:
```
## Execution Context
Daemon Host: {hostname}
Target Pane: -
```

Improve it to include the tmux session name, available pre-approved scripts, and whether
read-only commands are auto-approved:

```
## Ghost Session Execution Context
Daemon Host: {hostname}
Tmux Session: {tmux_session}
Pre-approved Scripts: {list or "none"}
Read-only Commands: auto-approved: {yes/no}
All commands run in background (de-incident-* windows). No user is present.
```

This helps the AI plan around what it can and cannot do.

### 3.5 Update `sre.toml` system prompt ghost section

**File:** `assets/prompts/sre.toml`

Update the ghost session rules to reflect:
- The new `GHOST_TRIGGER: YES/NO` output format for watchdog analysis (Phase 2.2)
- Max turns limit (users setting up runbooks should know the AI has a turn budget)
- Notification behavior (AI should write a brief summary as its final message so the
  catch-up brief has useful content)

Also update the webhook alert section to explain the `GHOST_TRIGGER` field.

Since `SRE_PROMPT_TOML` in `src/config.rs` is `include_str!("../assets/prompts/sre.toml")`,
the prompt is updated simply by editing `sre.toml`. The `builtin_sre_prompt_parses` test
in `config.rs` validates TOML on every `cargo test`.

### 3.6 Update `CLAUDE.md` architecture overview

**File:** `CLAUDE.md`

Add/update:
- Ghost session architecture in the Key files table (already has `ghost.rs` listed implicitly,
  make it explicit with the correct role of `ghost.rs`, `policy.rs`)
- Document `MAX_GHOST_TURNS` constant
- Document the `GHOST_TRIGGER` output convention for watchdog analysis
- Add `session_exists` to the tmux helper table if added in Phase 2.4
- Correct the N15 catch-up brief section to mention ghost session start/completion events

### Phase 3 — Deliverables
- [ ] `src/webhook.rs`: notify on ghost session start
- [ ] `src/daemon/server.rs`: notify on ghost session completion/failure; richer context in system prompt
- [ ] `src/ipc.rs`: add ghost session fields to `DaemonStatus` response
- [ ] `src/cli/commands.rs`: display ghost session state in `daemoneye status`
- [ ] `assets/prompts/sre.toml`: update ghost and webhook sections
- [ ] `CLAUDE.md`: update architecture documentation
- [ ] Tests: verify status response includes ghost session count
- **Commit:** `Feat: ghost session observability — notifications, status display, richer context`

---

## Phase 4 — Pipe-Pane Error Hardening (separate from ghost sessions)

**Goal:** Make the R1 pipe-pane failure graceful and diagnose why pane IDs arrive as invalid.

### 4.1 Validate pane_id before calling `start_pipe_pane` in `handle_client`

**File:** `src/daemon/server.rs`

The existing `is_valid_pane_id()` function (line 33) validates the `%<digits>` format.
There's no check whether the pane actually exists. Before calling `start_pipe_pane`, use
`tmux::pane_exists(pane_id)` (already implemented in `src/tmux/pane.rs`):

```rust
if let Some(ref pane_id) = client_pane {
    if !crate::tmux::pane_exists(pane_id) {
        log::warn!("R1: skipping pipe-pane for {} — pane no longer exists", pane_id);
    } else {
        match crate::tmux::start_pipe_pane(pane_id) {
            Ok(_) => { entry.pipe_source_pane = Some(pane_id.clone()); }
            Err(e) => { log::warn!("R1: could not start pipe-pane for {}: {}", pane_id, e); }
        }
    }
}
```

This changes the error from a confusing tmux stderr message to a clear daemon-level log.

### 4.2 Propagate tmux session name in pipe-pane error messages

**File:** `src/tmux/pane.rs` — `start_pipe_pane()`

The current error is `"pipe-pane failed for {pane_id}: {tmux_stderr}"`. Add context so it's
clear which session the pane was expected in. Callers should log the session name alongside.

### Phase 4 — Deliverables
- [ ] `src/daemon/server.rs`: validate pane exists before `start_pipe_pane` in `handle_client`
- [ ] `src/daemon/background.rs`: same guard before `start_pipe_pane` in `run_background_in_window`
- **Commit:** `Fix: validate pane existence before pipe-pane to avoid confusing tmux errors`

---

## Commit Sequence

```
Phase 1: Fix: ghost session role bug, channel lifecycle, and EOF guard
Phase 2: Harden ghost session pipeline: trigger reliability, turn cap, session validation
Phase 3: Feat: ghost session observability — notifications, status display, richer context
Phase 4: Fix: validate pane existence before pipe-pane to avoid confusing tmux errors
```

---

## Risks & Notes

- **Anthropic API message format**: Phase 1's role fix is the highest-confidence change.
  After it, do a live test with a webhook firing to confirm ghost sessions actually run.
- **GHOST_TRIGGER format (Phase 2.2)**: The exact output format should be validated against
  all three AI backends (Anthropic, OpenAI, Gemini). A simple `GHOST_TRIGGER: YES` on its
  own line is more reliable than JSON. If the model refuses to follow the format, fall back
  to checking if the response does NOT contain "NO ACTION NEEDED" or similar negative phrases.
- **`session_exists` helper (Phase 2.4)**: Should use `tmux has-session -t name` which exits
  0 if the session exists, non-zero otherwise. Avoid `list_sessions()` for this check since
  it does more work.
- **Max turns (Phase 2.3)**: 20 turns is a conservative cap. A ghost session doing 3-4 tool
  calls per turn over 5 turns = 15-20 tool calls total is reasonable. Adjust after observing
  real ghost sessions in production.
- **Phase 3.3 (ghost list subcommand)** is marked stretch goal — implement only if Phase 3.2's
  status additions are insufficient.
