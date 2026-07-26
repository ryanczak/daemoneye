# Phase 03: Echo User Input to the Transcript

**Milestone:** M5 — UX & Stability
**Status:** in-progress
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
