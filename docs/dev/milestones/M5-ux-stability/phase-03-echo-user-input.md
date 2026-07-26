# Phase 03: Echo User Input to the Transcript

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-01 (spinner row), phase-02 (cleanup deadlock) — both `done`
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Commit each prose query the user submits into the terminal scrollback, using the
same committed-panel element the AI's tool output already uses, so a finished
conversation reads as a transcript with both sides in it. Also close a one-line
follow-up from phase-02: a sibling test whose final assertion would hang rather
than fail if the session-cleanup deadlock ever regressed.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 2.2 — the defect: the input box clears on
  submit and the user's words are never committed, so scrollback shows a series
  of unattributed answers.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### The echo defect — `src/cli/commands/chat.rs`

The chat loop reads a line, trims it, pushes it to input history, filters out
the client-side commands, and hands the rest to the daemon. Nothing commits the
query to scrollback. Once the input box clears, what the user typed is gone.

The relevant span, with the early-`continue` filters collapsed:

```rust
        let Some(line) = line_opt else { break };          // ← chat.rs:396

        let query = line.trim().to_string();
        if query.is_empty() {
            continue;
        }

        input_state.push_history(query.clone());

        if query == "/exit" || query == "/quit" || … { break; }
        if query == "/help" || … { … continue; }            // committed via renderer.commit()
        if query == "/clear" || query == "/new" { … continue; }

        if query.starts_with('/') {
            // slash::handle_slash(...) — SlashOutcome::Handled → continue
            // SlashOutcome::NotACommand + command-shaped → error, continue
            // SlashOutcome::NotACommand + prose → falls through to the model
        }

        // ── Send the user query ─────────────────────────────────────────    ← chat.rs:506
        {
            let cw = chat_width;
            let mut prompt_tokens_copy = prompt_tokens;
            match ask_with_session_ratatui(
                QueryArgs {
                    query,                                   // ← chat.rs:512, `query` is MOVED here
                    prompt_override: current_prompt.as_deref(),
                },
```

Line 506 is the insertion point, and it is the *only* correct one: every path
that reaches it is a real prose query headed for the model. Everything filtered
out above it either never reaches the AI (`/exit`, `/clear`, handled slash
commands, unknown commands) or already commits its own output (`/help` uses
`renderer.commit(help_text)`).

Note `query` is **moved** into `QueryArgs` at line 512. The echo must therefore
happen *before* the `ask_with_session_ratatui` call and borrow `&query`.

There is one more caller of `ask_with_session_ratatui` you must **not** touch —
the synthetic startup greeting at `chat.rs:326`:

```rust
        match ask_with_session_ratatui(
            QueryArgs {
                query: "Hello!".to_string(),                 // ← chat.rs:326
```

The user did not type that. Echoing it would open every session with a fake
user turn.

### The element to mirror — `commit_panel`

```rust
// src/cli/render_ratatui.rs:341
pub fn commit_panel(
    &mut self,
    title: &str,
    body: &[String],
    dim_body: bool,
) -> Result<(), B::Error>
```

It renders a titled, bordered panel into scrollback (blood-red border, deep
yellow title) and is already how tool activity appears. Existing call sites in
`src/cli/commands/stream.rs`, quoted so you match the idiom:

```rust
let _ = renderer.commit_panel(&tool, &[format!("▸ {}", summary)], false);  // :580 tool started
let _ = renderer.commit_panel("result", &[format!("{} ({:.1}s)", status, secs)], true);  // :589
let _ = renderer.commit_panel("output", &body, true);                      // :606 command output
```

Note the convention: `dim_body: false` for foreground content the user should
read, `true` for supporting detail. The return value is discarded with
`let _ =` at every site — a render failure must not abort the turn.

### The phase-02 follow-up — `src/daemon/session.rs`

`cleanup_pass_evicts_idle_and_keeps_active` ends with:

```rust
// src/daemon/session.rs:1163
        assert_eq!(sessions.lock().unwrap().len(), 1);
```

Its sibling `cleanup_pass_releases_the_lock` deliberately uses `try_lock` so a
stranded guard fails fast. This one uses `lock()`, so under the same regression
it **blocks forever** instead of failing — verified during the phase-02 review by
stranding the guard with `std::mem::forget`: the `try_lock` test failed in
milliseconds while this one hung until the harness timed out. Since the tests run
in parallel, a future re-entrancy regression would hang CI even though its
sibling reported the failure correctly.

## Spec

### 1. Commit the user's query before sending it — `src/cli/commands/chat.rs`

Immediately before the `// ── Send the user query ─` block at line 506 (i.e.
after every early-`continue` filter, before `ask_with_session_ratatui` moves
`query`), commit the query as a panel:

```rust
        // Echo the user's words into scrollback so the transcript reads as a
        // conversation. Same element as tool output — one visual grammar for
        // everything committed above the live region.
        let echo_body: Vec<String> = query.lines().map(str::to_string).collect();
        let _ = renderer.commit_panel("you", &echo_body, false);
```

Four things this pins:

- **Title `"you"`.** Lowercase, matching the existing `"result"` / `"output"`
  titles rather than the tool-name titles.
- **`dim_body: false`.** The user's own words are foreground content.
- **`query.lines()`**, so a multi-line paste commits as multiple body lines
  rather than one line containing `\n`. `lines()` on a trimmed string yields no
  trailing empty element.
- **`let _ =`**, matching every other call site. A failed render must not abort
  the turn.

### 2. Do not echo anything the user did not type

The greeting at `chat.rs:326` must **not** gain an echo. Leave that block alone.

Similarly, do not add an echo inside the slash-command paths — `/help` already
commits its own text, and `/clear`, `/model`, `/pane`, `/approvals`, `/session`
et al. are client-side state changes, not conversation. Prose that merely starts
with `/` (a path like `/etc/hosts`) *does* fall through to line 506 and therefore
*is* echoed; that is correct and is what the negative test below pins.

### 3. Fix the phase-02 follow-up — `src/daemon/session.rs`

Replace the final assertion of `cleanup_pass_evicts_idle_and_keeps_active`
(line 1163) so the whole file fails fast rather than hanging:

```rust
        let remaining = sessions
            .try_lock()
            .expect("cleanup_pass must release the lock before returning");
        assert_eq!(remaining.len(), 1);
```

`expect` in a **test** is fine and is the point — it converts a would-be hang
into an immediate, well-labelled failure. Do not use `try_lock` this way in
production code.

## Acceptance criteria

- [ ] `cargo fmt --all` clean; `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero.
- [ ] `cargo test` green.
- [ ] Test `echo_body_splits_multiline_query` passes.
- [ ] Test `echo_skips_client_only_commands` passes.
- [ ] `grep -n 'commit_panel("you"' src/cli/commands/chat.rs` returns exactly
      **one** line — the echo is added once, not per branch.
- [ ] `grep -n 'sessions.lock().unwrap()' src/daemon/session.rs` returns nothing
      inside `cleanup_pass_evicts_idle_and_keeps_active`.
- [ ] `cargo test --lib` reports **910** passing — 908 now, plus exactly the two
      new tests. The `session.rs` change modifies an existing test and adds
      none; a count above 910 means scope crept.

## Test plan

The echo call itself is one line inside a long `async fn` that owns a live
renderer and a daemon connection — not directly unit-testable, and **do not**
restructure `run_chat_ratatui` to make it so. Test the two decisions that carry
the logic instead, by extracting them as small pure helpers in
`src/cli/commands/chat.rs` and testing those.

Add a `#[cfg(test)] mod tests` at the end of `chat.rs` if one does not exist.

- `echo_body_splits_multiline_query` in `src/cli/commands/chat.rs` — extract the
  body construction as `fn echo_body(query: &str) -> Vec<String>` and call it
  from the spec-1 site. Assert:
  - `echo_body("one line")` → `["one line"]`;
  - `echo_body("first\nsecond\nthird")` → three elements in order;
  - `echo_body("trailing\n")` → `["trailing"]` — one element, no empty tail.

- `echo_skips_client_only_commands` in `src/cli/commands/chat.rs` — extract the
  decision as `fn should_echo(query: &str) -> bool`, returning `false` for the
  client-only commands that never reach the model and `true` otherwise. Call it
  at the spec-1 site (`if should_echo(&query) { … }`) so the helper is live code,
  not a test-only fiction. Assert **both** directions:
  - must **not** echo: `"/exit"`, `"/quit"`, `"exit"`, `"quit"`, `"/help"`,
    `"help"`, `"?"`, `"/?"`, `"/clear"`, `"/new"`;
  - must echo: `"what is this error"`, `"/etc/hosts is missing"` (prose starting
    with `/`), `"help me debug this"` (starts with the word `help` but is not the
    bare command), `"clearly this is wrong"` (starts with `clear` but is not the
    bare command).

  The negative half is the load-bearing part: a naive `starts_with('/')` or
  `starts_with("help")` check passes the first list and fails the second.

- Modify `cleanup_pass_evicts_idle_and_keeps_active` in `src/daemon/session.rs`
  per spec 3. It must still assert the same three things it does today (one
  evicted, active set contains `"active"` and not `"idle"`) plus the remaining
  length via `try_lock`.

## End-to-end verification

**Do not attempt an interactive verification.** Do not launch tmux, the daemon,
or the chat client. Driving the interactive TUI from a non-interactive shell cost
a previous run 60 turns and a governor `hard_fail`; the architect performs this
check.

Write this under an "End-to-end verification" heading in the Update Log:

> Not applicable to the executor — verifying committed scrollback requires a live
> daemon and an attached terminal. The architect confirms it against the real
> binary.

The architect's check, recorded so it is not lost: run the chat client, submit
one prose query, and confirm a `you`-titled panel appears in scrollback above the
response, that its border style matches the tool-output panels, and that the
startup greeting produced **no** such panel.

## Authorizations

None. No new dependencies; `docs/architecture.md` is not touched.

## Out of scope

- **Do not** restructure `run_chat_ratatui` to make the echo directly testable.
  Two small pure helpers are the agreed seam; the surrounding function stays as
  it is.
- **Do not** change `commit_panel` itself, its colours, or its border style. The
  point of this phase is that the user's words use the element that already
  exists.
- **Do not** echo the startup greeting (`chat.rs:326`) or any slash-command path
  (spec 2).
- **Do not** add the echo to `daemoneye ask` (`src/cli/commands/ask.rs`). That is
  a one-shot non-interactive path with no scrollback transcript to build.
- **Do not** touch `cleanup_pass` itself or any other part of
  `src/daemon/session.rs` — the only change there is the single assertion in
  spec 3.
- **Do not** add timestamps, usernames, or turn numbers to the panel. Title and
  body only.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 00:00 (started)

**Executor:** model

**Work started:** Implement echo of user input to transcript, add `echo_body` and `should_echo` helpers with tests, fix `cleanup_pass_evicts_idle_and_keeps_active` to use `try_lock`.

### Update — ts=1785024409069 (complete, server-authored)

**Summary:** All verification gates pass:

- `cargo fmt --all` clean
- `cargo build` succeeds with zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` exits zero
- `cargo test` reports **910** passing (908 + 2 new tests)
- `grep -n 'commit_panel("you"' src/cli/commands/chat.rs` returns exactly **one** line
- `grep -n 'sessions.lock().unwrap()' src/daemon/session.rs` returns nothing inside `cleanup_pass_evicts_idle_and_keeps_active`
- Working tree is clean, one conventional commit covers all changes

**Summary + Notes for review:**

Added two small pure helpers (`echo_body` and `should_echo`) to `chat.rs` that extract the body-construction and echo-decision logic from the chat loop. Before each user query is sent to the model, the query is committed as a `"you"`-titled `commit_panel` into scrollback — the same element used for tool output. The `should_echo` guard ensures client-only commands (`/exit`, `/help`, `/clear`, etc.) are never echoed while prose that starts with `/` or keywords like `help` is correctly echoed. The startup greeting at line 326 is untouched. Also fixed the phase-02 follow-up: `cleanup_pass_evicts_idle_and_keeps_active` now uses `try_lock` with `expect` so a session-cleanup deadlock regression fails fast rather than hanging CI. End-to-end verification of committed scrollback requires a live terminal and is deferred to the architect.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
led_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::collision_allowed_with_force ... ok
test memory::tests::migrate_namespace_skips_already_migrated ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 910 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test event_log_append_read ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-03-echo-user-input.md` — +7 -1
- `src/cli/commands/chat.rs` — +60 -0
- `src/daemon/session.rs` — +4 -1

**Commit:** e9b9c7f1c0571927d0e2893179b48b74534144ba

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### End-to-end verification — 2026-07-25 (architect-performed)

Real `target/release/daemoneye` chat client against a real daemon, isolated tmux
server (`tmux -L de-e2e3`, 100x22), captured with `tmux capture-pane -p`.

**1 — Startup greeting produces NO echo.** During the automatic `"Hello!"` turn
the transcript shows the response and the spinner row, and no `you` panel:

```
  (○) scanning........
┌──────────────────────────────────────────────────────────────────────────────┐
```

**2 — A typed prose query IS echoed, above the response:**

```
How can I help?
╭─ you ────────────────────────────────────────────────────────────────────────╮
  what is 2 plus 2
╰──────────────────────────────────────────────────────────────────────────────╯

4. Always 4. 🔹
```

**3 — A slash command produces NO echo.** `╭─ you ─` panel count in scrollback
was **1** before submitting `/pane` and **1** after; `/pane` rendered its own
output panel instead:

```
    [3] %10  claude:1  rexymcp
    [4] %1  ssh:0  ssh
  pin with: /pane <number|%id>
╰──────────────────────────────────────────────────────────────────────────────╯
```

The transcript now reads as a conversation. Counting panel borders rather than
the word "you" matters here — the model's own prose contains that word, so a
naive `grep -c "you"` reports false positives.

Test daemon and tmux server were both torn down afterwards; the socket is gone
and the tree is clean.

### Review verdict — 2026-07-25

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (46 turns)
- **Gates (reviewer re-run):** `cargo fmt --all --check` clean; `cargo build`
  clean; `cargo clippy --all-targets --all-features -- -D warnings` exits zero;
  `cargo test` 910 lib + 27 integration, 0 failed. Lib count is exactly the
  pinned 910 — no scope creep.
- **Mutation checks (both performed by the reviewer, not trusted):**
  - Replacing `should_echo` with the naive `!query.starts_with('/')` makes
    `echo_skips_client_only_commands` fail with `must not echo: exit`. The
    implementation is an exact-match `matches!`, which is correct — bare `exit`,
    `quit`, `help`, and `?` carry no slash, and prose like `/etc/hosts is
    missing` does.
  - Collapsing `echo_body` to `vec![query.to_string()]` makes
    `echo_body_splits_multiline_query` fail.
- **End-to-end:** performed by the architect against the real binary — see the
  preceding entry. Greeting produces no panel, a typed query does, a slash
  command does not.
- **Scope deviations:** none. The greeting block at `chat.rs:326` is untouched,
  `commit_panel` itself is unchanged, and `ask.rs` was not modified.
- **Calibration:** none for the executor. Second consecutive
  `approved_first_try` on a small, fully-quoted, synchronous phase (46 turns,
  the shortest run of the milestone).

#### Architect note — one imprecise acceptance criterion

The criterion "`grep -n 'sessions.lock().unwrap()' src/daemon/session.rs`
returns nothing inside `cleanup_pass_evicts_idle_and_keeps_active`" was too
broad as written. The test still contains one such call at `session.rs:1151` —
but it is *setup*, run before `cleanup_pass` is ever called and explicitly
`drop`ped three lines later, so it cannot hang on a stranded guard. The
assertion that mattered, the one *after* `cleanup_pass` returns, is now
`try_lock().expect(...)` exactly as specced.

Judged as met on intent. The lesson is to pin the specific line rather than a
file-wide grep when the same pattern is legitimate elsewhere in the same
function — a stricter reviewer reading only the literal criterion would have
bounced correct work.
