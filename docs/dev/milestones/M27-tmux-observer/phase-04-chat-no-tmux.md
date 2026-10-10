# Phase 04: chat-no-tmux — chat runs in place outside tmux; its context is the daemon's shells

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
**Depends on:** phase-03
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye chat` and `daemoneye ask` must work in a plain terminal with no
tmux. Two things stand in the way today:

- **The client reaches into tmux it is not in.** When `$TMUX_PANE` is
  unset and `[daemon] tmux_session` is configured (it is by default,
  `"daemoneye"`), `run_chat_inner` (`src/cli/commands/chat.rs`) opens a
  `tmux new-window` and `exec`s `tmux attach-session`. The client also
  resolves its tmux session with `crate::tmux::current_session_name()`,
  which runs `tmux display-message -p '#S'`. Outside tmux, that call
  returns *some other* session whenever a tmux server is running.
- **The prompt's terminal context is always tmux.** `build_first_turn_prompt`
  and `build_subsequent_turn_prompt` (`src/daemon/prompt.rs`) always render
  `cache.get_labeled_context(…)`. That cache polls the daemon's own tmux
  session, so a chat outside tmux is shown panes the user is not in.

PE decision 1: when the client is not in tmux, its context carries **the
daemon's own shells only**.

## Spec

- **Client, `src/cli/commands/chat.rs`.** Delete the managed-session
  auto-attach block. Outside tmux, chat runs in place in the current
  terminal. Every tmux call the client makes is gated on `$TMUX` being set.
  That covers the session-name lookup, target-pane resolution, and the
  pane-width query and resize. The gate must not depend on whether a tmux
  binary or server exists. Put the "am I in tmux / which session" decision
  in one small function that takes the environment value as a parameter,
  so it can be tested without touching the real environment. `ask`
  (`src/cli/commands/ask.rs`) uses the same rule.
- **Daemon, terminal context.** When an `Ask` carries no chat pane (the
  client is outside tmux), the prompt contains **no tmux data**. That means
  no pane blocks, no pane map, no foreground-target line and no session
  topology. In their place it carries
  `knowledge::shells_only_context(&Owner::Chat(session_id))`
  (`src/daemon/executor/knowledge/shell.rs`). This applies to both the
  first-turn prompt and the subsequent-turn snapshot. The
  `get_shell_context` tool (default scope, `src/daemon/executor/mod.rs`)
  follows the same rule. With a chat pane, all three behave exactly as
  today.
- Leave `[daemon] tmux_session`, the daemon's `bg_session`, the cache's
  poll loop and the session-name fallback in `handle_ask` alone. Phase 05
  owns the daemon's tmux side.

## Tests

- `tmux_session_requires_the_tmux_env`: the client's resolver returns
  `None` when the `$TMUX` value passed in is absent or empty, and makes no
  tmux call in that case. It returns the session when the value is present.
- `first_turn_prompt_without_chat_pane_carries_shells_only`: a cache that
  holds at least one pane, `chat_pane` / `client_pane` `None`. The prompt
  contains `[SHELLS]` and no `PANE %`.
- `first_turn_prompt_with_chat_pane_keeps_tmux_context`: preservation. With
  the same cache and a chat pane, the pane blocks are still there.
- `subsequent_turn_snapshot_without_chat_pane_carries_shells_only`: the
  same check for `build_subsequent_turn_prompt` with `inject_snapshot`.
- `get_shell_context_without_chat_pane_is_shells_only`: the default scope
  with no chat pane returns the shells-only block, with no pane data.

## Acceptance criteria

- [ ] Each of the five tests above reports `1 passed` alone. Today each
      reports `0 passed`.
- [ ] `grep -n 'attach-session\|new-window' src/cli/commands/chat.rs`
      prints nothing. Today it prints lines.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports exactly `passed=2323 failed=0`: today's
      `passed=2318` (measured at the phase-03 review; no code has changed
      since) plus the five new tests. If you add supporting tests, raise the
      count by one each and name them in the Update Log.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
for t in tmux_session_requires_the_tmux_env first_turn_prompt_without_chat_pane_carries_shells_only first_turn_prompt_with_chat_pane_keeps_tmux_context subsequent_turn_snapshot_without_chat_pane_carries_shells_only get_shell_context_without_chat_pane_is_shells_only; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
grep -n 'attach-session\|new-window' src/cli/commands/chat.rs ; echo "grep_done"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines. Do not grep the full test output.

## Authorizations

- [x] May edit `src/cli/commands/`, `src/cli/palette.rs`,
      `src/tmux/session.rs` (only to add the `$TMUX` gate),
      `src/daemon/prompt.rs`, `src/daemon/executor/mod.rs` and `src/main.rs`
      (escalation: to drop the dead `chat --session` flag).

## Out of scope

- The daemon's tmux side: the cache poll, `bg_session`, `[daemon]
  tmux_session`, window creation and the hooks (phase 05).
- Alerts over the presence connection (phase 03b).
- Docs and `daemoneye doctor` (phase 06).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-09 18:34 (started)

Executor: opus-4.5 (headless run). Read the phase doc, STANDARDS and the
touched sources (`src/cli/commands/chat.rs`, `ask.rs`, `stream.rs`,
`src/tmux/session.rs`, `src/daemon/prompt.rs`, `src/daemon/executor/mod.rs`,
`src/daemon/server/ask.rs`, `src/tmux/cache.rs`). Baseline `cargo test`
reproduces `passed=2318 failed=0`. Implementing the Spec in order.

### Update — 2026-10-10 (escalation)

**Chosen lever:** resume.
**Rationale:** The run hit a NoProgressStall (120 consecutive read-only
calls) at turn 495, with the design right and almost all of it on disk.
The architect's gate run on the partial tree found only four precise
leftovers. That is a "nearly done, hit a wall" stall, not a spec gap.

Architect gate run on the uncommitted tree: build clean.
`tmux_session_requires_the_tmux_env`,
`first_turn_prompt_with_chat_pane_keeps_tmux_context` and
`subsequent_turn_snapshot_without_chat_pane_carries_shells_only` pass.
`first_turn_prompt_without_chat_pane_carries_shells_only` FAILS on its own
over-broad assertion. `get_shell_context_without_chat_pane_is_shells_only`
does not exist yet. Clippy fails on one unused import.

### Update — 2026-10-10 03:03 (resume, executor opus-4.5)

Resumed per the architect's four-item list; nothing re-derived.

1. `first_turn_prompt_without_chat_pane_carries_shells_only` — replaced the
   tag-absence assertions with fixture-data assertions (`idx:0=%1`,
   `running make`, `[SESSION TOPOLOGY] 1 windows`), keeping the
   `[SHELLS] 1` positive. Now passes.
2. Added `get_shell_context_without_chat_pane_is_shells_only` in
   `executor/mod.rs`'s tests, driving the GetShellContext arm through
   `execute_tool_call` with `chat_pane: None` over the swapped fixture
   registry. Asserts `[SHELLS]` present, `PANE %` and
   `[FOREGROUND TARGET]` absent. Passes.
3. Clippy: deleted the unused cfg(test) re-export of
   `shell_context_block_in` from `knowledge/mod.rs` (the prompt tests did
   not end up needing it; `shell_context_block_in` stays `pub(crate)` —
   the shell.rs tests drive it directly). Clippy clean, no `#[allow]`.
4. Dropped the dead `_session_override` parameter from `run_chat` and
   `run_chat_inner` and removed the now-meaningless `chat --session`
   flag from `Commands::Chat` (authorized escalation); fixed the
   `main.rs` call site.

No supporting tests were added. Gate set: build clean (zero new
warnings), clippy `-D warnings` exit 0, `cargo fmt --all --check`
exit 0, full `cargo test` `passed=2323 failed=0`. End-to-end
verification captured in the next entry.

### Update — 2026-10-10 03:03 (end-to-end verification)

Captured from the single § End-to-end verification run above (output
redirected to /tmp/e2e_phase04.txt and pasted verbatim).

```
$ for t in tmux_session_requires_the_tmux_env first_turn_prompt_without_chat_pane_carries_shells_only first_turn_prompt_with_chat_pane_keeps_tmux_context subsequent_turn_snapshot_without_chat_pane_carries_shells_only get_shell_context_without_chat_pane_is_shells_only; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
tmux_session_requires_the_tmux_env: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2268 filtered out; finished in 0.02s
first_turn_prompt_without_chat_pane_carries_shells_only: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2268 filtered out; finished in 0.05s
first_turn_prompt_with_chat_pane_keeps_tmux_context: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2268 filtered out; finished in 0.07s
subsequent_turn_snapshot_without_chat_pane_carries_shells_only: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2268 filtered out; finished in 0.03s
get_shell_context_without_chat_pane_is_shells_only: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2268 filtered out; finished in 0.00s

$ grep -n 'attach-session\|new-window' src/cli/commands/chat.rs ; echo "grep_done"
grep_done

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.90s

$ cargo fmt --all --check; echo "fmt_exit=$?"
fmt_exit=0

$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2323 failed=0

$ git diff --stat
 docs/dev/milestones/M27-tmux-observer/README.md    |   2 +-
 .../M27-tmux-observer/phase-04-chat-no-tmux.md     |  29 ++-
 src/cli/commands/chat.rs                           |  91 ++-----
 src/cli/commands/mod.rs                            |   4 +-
 src/cli/palette.rs                                 |   5 +-
 src/daemon/executor/knowledge/mod.rs               |   5 +
 src/daemon/executor/knowledge/shell.rs             |  18 +-
 src/daemon/executor/mod.rs                         |  78 ++++++
 src/daemon/prompt.rs                               | 266 ++++++++++++++++++--
 src/main.rs                                        |   9 +-
 src/tmux/session.rs                                |  65 ++++-
 11 files changed, 457 insertions(+), 115 deletions(-)
```

PASTE MATCH: yes
