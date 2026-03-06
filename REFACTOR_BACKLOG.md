# Refactor Backlog

Items identified during the March 2026 code-review sprint. Work through in order within each tier.

## High Value, Low Risk — COMPLETE

| # | What | Status |
|---|------|--------|
| 1 | Named duration constants in `executor.rs` | ✅ Done |
| 2 | Deduplicate `command_has_sudo` / `command_is_sudo` | ✅ Done |
| 3 | Named window-prefix constants (`BG_WINDOW_PREFIX`, `SCHED_WINDOW_PREFIX`, `DAEMON_WINDOW_PREFIX` in `daemon/mod.rs`) | ✅ Done |
| 4 | `FG_HOOK_COUNTER` (`AtomicUsize` in `daemon/session.rs`) for foreground hook slot naming — replaces `SystemTime::now() % 10000` in `executor.rs` | ✅ Done |

## Medium Effort — TODO

| # | What | Notes |
|---|------|-------|
| 5 | Append-only session history file | Full history `Vec` is rewritten on every message. Switch to an append-only JSONL write (one object per turn) and read the tail on session resume. |
| 6 | Extract approval gate helper | `timeout → read_line → parse ToolCallResponse → check ID` is repeated verbatim for foreground and background approval in `executor.rs`. A ~15-line `async fn` eliminates the duplication. |
| 7 | Shell-escape session names in hook strings | Spaces/special chars in tmux session names can break the `run-shell` command embedded in hooks. Use `shlex`-style escaping or restrict allowed chars. |

## Larger Refactors — TODO

| # | What | Notes |
|---|------|-------|
| 8 | Decompose `execute_tool_call` | ~500-line function in `executor.rs`. The foreground and background arms are large enough to each be their own `async fn`, called from a thin dispatch match. |
| 9 | Unified tool schema | Each AI tool is defined three times (Anthropic / OpenAI / Gemini) in `tools.rs`. A single schema struct with per-provider rendering would eliminate drift when tools are added or changed. |
| 10 | Adaptive cache refresh | `SessionCache` polls tmux every 2 s unconditionally. Could back off (e.g. 5–10 s) when no windows have changed, reducing subprocess churn when the user is idle. |
