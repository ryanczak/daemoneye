# Phase 08: Ellipsis markers on truncation + complete `/help`

**Milestone:** M3 — Polish & Maintenance
**Status:** done
**Depends on:** none
**Estimated diff:** ~110 lines (two small pure helpers + 5 call-site edits, one
help-text extraction, ~5 unit tests).
**Tags:** language=rust, kind=feature, size=s

## Goal

Two user-facing papercuts from the M3 survey:

1. **Silent truncation.** Three TUI render paths cut text to fit the available
   width/length and drop the overflow with **no marker**, so the user can't tell
   content was lost: the status-bar session id (chopped to 8 chars), panel body
   lines (clipped to inner width), and committed scrollback lines (clipped to
   terminal width). Add an ellipsis (`…`) marker so truncation is visible.
2. **Incomplete `/help`.** The `/help` text lists ten primary commands but omits
   the recognized **aliases** and two non-obvious behaviors: the message-redirect
   affordance at a tool-approval prompt, and the 10-line on-screen tool-output
   cap. Document them.

Both are additive, behavior-preserving for the non-truncated/common case.

## Architecture references

Read before starting:

- `docs/architecture.md#21-interactive-requestresponse` — the chat/approval flow
  whose help text and tool-output rendering this phase documents/marks. Confirm
  no protocol change is needed (there isn't one — this is presentation only).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` is clean; phase-07 is already committed).

## Current state

### Truncation site A — status bar session id (THREE identical sites)

`src/cli/render_ratatui.rs` has three byte-identical status-bar blocks, in
`render_live_region` (line ~458), `render_prompt_region` (line ~529), and
`render_spinner_region` (line ~565):

```rust
    let status_text = format!(
        " session:{} · {} · up {} ",
        &session_id[..8.min(session_id.len())],
        model,
        uptime,
    );
```

`&session_id[..8.min(session_id.len())]` shows the first 8 chars of the (36-char
UUID) session id with no indication it was abbreviated.

### Truncation site B — panel body lines

`src/cli/render_ratatui.rs:359-364` in `commit_panel`:

```rust
        for line in body {
            let truncated: String = line.chars().take(inner.saturating_sub(2)).collect();
            lines.push(Line::from(Span::styled(
                format!("  {}", truncated),
                body_style,
            )));
        }
```

Each body line is clipped to `inner - 2` chars (the panel inner width minus the
2-space indent) with no marker.

### Truncation site C — committed scrollback lines

`src/cli/render_ratatui.rs:177-184` in `commit`:

```rust
            for (i, line) in lines.split('\n').enumerate() {
                let y = i as u16;
                if y >= area.height {
                    break;
                }
                let text: String = line.chars().take(area.width as usize).collect();
                buf.set_string(area.x, area.y + y, &text, Style::default());
            }
```

Each scrollback line is clipped to terminal width with no marker.

> All three sites already truncate by **char count** (`.chars().take(n)`), not by
> display columns. Keep that convention — do **not** introduce a unicode-width
> dependency (that would need authorization). Char-count truncation is the
> existing, accepted behavior; this phase only adds the marker.

### `/help` text

`src/cli/commands/chat.rs:358-370` — a `let help_text = [ ... ].join("\n")` local
inside the chat loop, committed at line 405 when the user types `/help`:

```rust
    let help_text = [
        "Commands:",
        "  /help      show this list           /exit      quit",
        "  /clear     reset session            /refresh   resync host context",
        "  /model     list or switch model     /pane      list or pin target pane",
        "  /approvals list/on/off/revoke       /prompt    list or switch system prompt",
        "  /limits    show active limits       /session   save/load/list/delete/rename",
        "",
        "Up/Down navigate the input; at the top/bottom edge they recall history.",
        "",
    ]
    .join("\n");
```

**Recognized aliases not shown** (grep-verified): `/quit`, `/?`, `/new`,
`/models`, `/panes`, `/approval`, `/sessions`, plus the bare words `help`, `?`,
`exit`, `quit`. (`/exit` / `/help` / `/clear` dispatch in `chat.rs:401-421`; the
rest in `src/cli/commands/slash.rs:57-67`.)

**Undocumented behaviors:**

- **Message redirect**: at a tool-approval prompt the user can type a message
  instead of `Y`/`A`/`N` to redirect the agent — `build_approval_prompt`
  (`src/cli/commands/stream.rs:741`) shows `or type a message` only as a terse
  inline hint; `/help` never explains it.
- **Tool-output cap**: tool output panels show at most 10 lines on screen
  (`const MAX_LINES: usize = 10` at `src/cli/commands/stream.rs:594`), appending
  `… N more lines`. `/help` never mentions this; the full result is in history.

## Spec

### 1. Add two pure truncation helpers

In `src/cli/render_ratatui.rs`, add two **module-level free functions** (place
them near the existing free helper `fmt_uptime`). Both operate on char counts to
match the existing `.chars().take()` convention.

```rust
/// Clip `s` to at most `max` characters, marking truncation with a trailing '…'.
/// Returns `s` unchanged when it already fits. The '…' counts toward `max`, so a
/// truncated result is exactly `max` chars wide.
fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let kept: String = s.chars().take(max - 1).collect();
        format!("{kept}…")
    }
}

/// Abbreviate a session id for the status bar: the first 8 chars followed by '…'
/// when longer, otherwise the id unchanged.
fn short_session(id: &str) -> String {
    if id.chars().count() <= 8 {
        id.to_string()
    } else {
        let head: String = id.chars().take(8).collect();
        format!("{head}…")
    }
}
```

### 2. Route the three status-bar sites through `short_session`

In each of the three identical status-bar blocks (`render_live_region`,
`render_prompt_region`, `render_spinner_region`), replace the first format
argument:

```rust
        &session_id[..8.min(session_id.len())],
```

with:

```rust
        short_session(session_id),
```

The `{}` format placeholder accepts the returned `String` unchanged. Do not
otherwise modify the three blocks (this phase does not de-duplicate them — that
is out of scope).

### 3. Route the panel body lines through `truncate_with_ellipsis`

In `commit_panel` (site B), replace:

```rust
            let truncated: String = line.chars().take(inner.saturating_sub(2)).collect();
```

with:

```rust
            let truncated = truncate_with_ellipsis(line, inner.saturating_sub(2));
```

Leave the surrounding `format!("  {}", truncated)` indent untouched.

### 4. Route the committed scrollback lines through `truncate_with_ellipsis`

In `commit` (site C), replace:

```rust
                let text: String = line.chars().take(area.width as usize).collect();
```

with:

```rust
                let text = truncate_with_ellipsis(line, area.width as usize);
```

### 5. Extract `/help` text to a testable constant and complete it

In `src/cli/commands/chat.rs`, replace the local `let help_text = [ ... ].join("\n")`
with a reference to a new **module-level** `const HELP_TEXT: &str` (so its content
is unit-testable). Update the `renderer.commit(&help_text)` call at line ~405 to
commit `HELP_TEXT`.

The new help text must, in addition to the existing command table, document:

- the **aliases** — `/models`, `/panes`, `/sessions`, `/approval`, `/new`,
  `/quit` (and that bare `help` / `?` also open help);
- the **message-redirect** behavior at the approval prompt;
- the **10-line tool-output cap** (`… N more lines`; full output in history).

Suggested rendering (the exact wording/layout is the executor's call; the
**content** above is what's pinned):

```rust
const HELP_TEXT: &str = "\
Commands:
  /help      show this list           /exit      quit
  /clear     reset session            /refresh   resync host context
  /model     list or switch model     /pane      list or pin target pane
  /approvals list/on/off/revoke       /prompt    list or switch system prompt
  /limits    show active limits       /session   save/load/list/delete/rename

Aliases: /models /panes /sessions /approval /new /quit (and help, ? open help).

At a tool-approval prompt, type a message instead of Y/A/N to redirect the agent.
Tool output is capped at 10 lines on screen (… N more lines); full output is kept in history.

Up/Down navigate the input; at the top/bottom edge they recall history.
";
```

Do **not** change the dispatch/alias logic itself — the aliases already work;
this task only documents them.

## Acceptance criteria

- [ ] `truncate_with_ellipsis` and `short_session` exist as module-level fns in
      `src/cli/render_ratatui.rs`.
- [ ] The three status-bar blocks call `short_session(session_id)`; the panel and
      committed sites call `truncate_with_ellipsis(...)`. (Verify by reading the
      five edited sites; `grep -n "chars().take" src/cli/render_ratatui.rs` no
      longer shows the three replaced truncation lines.)
- [ ] `/help` text lives in a module-level `const HELP_TEXT: &str` in `chat.rs`
      and the `/help` handler commits it.
- [ ] `HELP_TEXT` contains the alias tokens (`/models`, `/panes`, `/sessions`,
      `/approval`, `/new`, `/quit`), a sentence describing the approval-prompt
      redirect, and a sentence describing the 10-line output cap.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` leaves the tree clean.
- [ ] `cargo test` passes (existing + new tests).

## Test plan

New unit tests in the existing `#[cfg(test)] mod tests` in
`src/cli/render_ratatui.rs` (helper tests) and a new `#[cfg(test)] mod tests` in
`src/cli/commands/chat.rs` (help-content test). Pin behavior + names; test count
and exact placement are the executor's call.

- `truncate_with_ellipsis_leaves_short_string_unchanged` — a string at or below
  `max` is returned verbatim with **no** `…` appended (the must-NOT case: assert
  the result does not contain `…`).
- `truncate_with_ellipsis_marks_overflow` — a string longer than `max` returns a
  result whose char count is exactly `max` and that ends in `…`.
- `truncate_with_ellipsis_zero_max_is_empty` — `max == 0` returns `""` (boundary;
  no underflow).
- `short_session_marks_long_id` — a 36-char UUID returns its 8-char prefix + `…`.
- `short_session_leaves_short_id_unchanged` — an id of ≤ 8 chars is returned
  verbatim with no `…`.
- `help_text_documents_aliases_and_behaviors` in `chat.rs` — asserts `HELP_TEXT`
  contains `/models`, `/panes`, `/sessions`, `/approval`, `/new`, `/quit`, the
  substring `redirect`, and the substring `10 lines` (or whatever cap phrasing
  the executor chose — assert against the actual content it wrote).

## End-to-end verification

`HELP_TEXT` is the exact byte sequence the running binary commits when the user
types `/help` (the `/help` handler commits the const verbatim). The
`help_text_documents_aliases_and_behaviors` test exercises that real artifact
directly — there is no separate non-interactive CLI path to drive `/help` in a
headless run, so the const-content test **is** the end-to-end check for the help
change. Quote its passing line in the completion Update Log.

The truncation helpers are exercised by the real render paths (`commit`,
`commit_panel`, the status bar), which now route through them; the inline TUI
viewport cannot be driven for visual diffing in a headless run, so verification
is the pure-helper unit tests above plus reading the five call sites to confirm
they route through the helpers. State this in the Update Log.

## Authorizations

None. (No new dependencies — do **not** add `unicode-width` or similar; char-count
truncation is intended. `docs/architecture.md` is not modified — confirm at
Pre-flight that no protocol/format change is implied. No STANDARDS §5 files.)

## Out of scope

- **No de-duplication of the three status-bar blocks.** They are byte-identical
  and a shared render helper would be reasonable, but that is a larger refactor;
  this phase only swaps the one truncation expression in each. Leave the rest.
- **No display-width (unicode) truncation.** Keep char-count semantics; do not
  add a width-aware crate.
- **No changes to the alias/dispatch logic** in `chat.rs` / `slash.rs` — the
  aliases already work; this phase only documents them.
- **No changes to the tool-output cap value** (`MAX_LINES = 10`) or its
  rendering — only document it in `/help`.
- **The plain-stdout truncations** in `src/cli/local_cmds.rs` (prompt-preview
  60-char cut at line ~30, job-id 8-char cut at line ~126) are **not** in scope —
  they are non-TUI `println!` paths outside the status-bar/panel/committed
  categories this phase targets.
- **Other large files / later M3 phases** — leave them alone.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-29 00:19 (started)

**Executor:** rexyMCP executor
**Summary:** Implementing ellipsis truncation helpers and expanded `/help` text.

### Update — 2026-06-29 00:25 (complete)

**Executor:** rexyMCP executor

**Tasks completed:**
1. Added `truncate_with_ellipsis` and `short_session` as module-level free functions in `src/cli/render_ratatui.rs` (near `fmt_uptime`).
2. Replaced all three status-bar `&session_id[..8.min(session_id.len())]` calls with `short_session(session_id)`.
3. Replaced panel body line truncation in `commit_panel` with `truncate_with_ellipsis(line, inner.saturating_sub(2))`.
4. Replaced committed scrollback line truncation in `commit` with `truncate_with_ellipsis(line, area.width as usize)`.
5. Extracted `/help` text to module-level `const HELP_TEXT: &str` in `src/cli/commands/chat.rs`, documenting aliases, approval-prompt redirect, and 10-line tool-output cap.

**Files changed:**
- `src/cli/render_ratatui.rs` — two new helpers, 5 call-site edits, 5 unit tests
- `src/cli/commands/chat.rs` — `HELP_TEXT` const, 1 unit test
- `docs/dev/milestones/M3-polish-maintenance/phase-08-help-and-truncation.md` — status flip + Update Log
- `docs/dev/milestones/M3-polish-maintenance/README.md` — phase table status flip

**Verification commands (all clean):**
- `cargo fmt --all` — clean
- `cargo build` — zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test` — 831 passed, 0 failed (including 3 `truncate_with_ellipsis*`, 2 `short_session*`, 1 `help_text_documents_aliases_and_behaviors`)

**Grep verification:**
- `grep -n 'chars().take' src/cli/render_ratatui.rs` shows only the two helper internals (lines 590, 601) — the three replaced truncation lines are gone.
- `grep -rn '…' src/cli/render_ratatui.rs src/cli/commands/chat.rs` confirms ellipsis literals landed in both files.
- `HELP_TEXT` contains all required tokens: `/models`, `/panes`, `/sessions`, `/approval`, `/new`, `/quit`, `redirect`, `10 lines`.

**End-to-end verification:**
- `HELP_TEXT` is the exact byte sequence committed when the user types `/help` (the handler commits the const verbatim). The `help_text_documents_aliases_and_behaviors` test asserts against the real const — this is the end-to-end check for the help change.
- The truncation helpers are exercised by the real render paths (`commit`, `commit_panel`, the status bar), which now route through them. The inline TUI viewport cannot be driven for visual diffing in a headless run; verification is the pure-helper unit tests plus reading the five call sites.

**Notes for review:** None — all changes are additive and behavior-preserving for the non-truncated/common case.

### Review verdict — 2026-06-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** rexyMCP executor (Qwen/Qwen3.6-27B-PrismaAURA)
- **Scope deviations:** none
- **Calibration:** Initial dispatch hard-failed before any work — the model
  emitted `update_task` with null arguments, which vLLM rejected as a 400 on the
  next request (`Can only get item pairs from a mapping`). Disabling
  `task_tracking` in `rexymcp.toml` resolved it; the re-dispatch completed clean
  in 70 turns. Not a code-quality lesson; an executor/endpoint interaction note.
- **Review checks:** independent re-run clean — `cargo fmt --all` (clean),
  `cargo build` (zero warnings), `cargo clippy --all-targets --all-features -D warnings`
  (clean), `cargo test` (831 unit + 27 integration passed). Five edited call sites
  verified by grep (three `short_session`, two `truncate_with_ellipsis`); the two
  remaining `chars().take` hits are the helper internals. All `.unwrap()` in the
  touched files are within `#[cfg(test)]`. HELP_TEXT contains all eight pinned
  tokens. The six new tests pin real behavior (overflow test asserts exact
  `"hello w…"` + char count 8).
