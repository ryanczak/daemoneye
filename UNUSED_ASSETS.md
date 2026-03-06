All unused assets identified in this file have been resolved as of the cleanup pass.

Items removed:
- poll_until_dead (server.rs) — dead code
- classify_exit_code (utils.rs) — dead code
- pane_pid (tmux/pane.rs) — dead code
- get_context_summary (tmux/cache.rs) — dead code
- parse_malformed_gemini_call duplicate (openai.rs) — dead code (live copy in gemini.rs retained)
- ToolCall.thought_signature — now active: Gemini backend extracts and round-trips it
- PromptDef.name / .description — removed from struct; prompts listing shows system preview instead
- SessionEntry.id / .info_pane — removed
- Runbook.description — removed
- client_pane param in find_best_target_pane — removed
- All #[allow(dead_code)] suppressors — removed
- RichPaneInfo.window_name / .dead / .dead_status — now propagated into PaneState and surfaced as [dead: N] in AI context
