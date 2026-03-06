the following list contains unused code assets. that were identified previously.  

  1. Unused Functions & Methods
   * src/daemon/server.rs → poll_until_dead: An async polling helper for pane death that has been replaced by
     the event-driven bg_done_subscribe channel.
   * src/daemon/utils.rs → classify_exit_code: A utility mapping exit codes (like 127) to strings (like "command
     not found"). Currently not called anywhere.
   * src/tmux/pane.rs → pane_pid: A helper to find the OS-level PID of a tmux pane. Superseded by other tmux
     metadata queries.
   * src/tmux/cache.rs → get_context_summary: An older summary generator for the AI. Superseded by the much
     richer get_labeled_context.
   * src/ai/backends/openai.rs → parse_malformed_gemini_call: A regex fallback parser for malformed tool calls
     that is no longer needed.


  2. Unused Struct Fields
   * src/ai/types.rs → ToolCall.thought_signature: (and its associated presence in the PendingCall and AiEvent
     enums). This field is defined and parsed but never actually used by the logic.
   * src/config.rs → PromptDef.name and PromptDef.description: Fields loaded from prompt files that are never
     displayed or read by the assistant logic.
   * src/daemon/session.rs → SessionEntry.id and SessionEntry.info_pane: Redundant fields; the ID is already the
     key in the session map, and the info pane logic has been removed.
   * src/runbook.rs → Runbook.description: Parsed from runbook TOML but never read.


  3. Leftover Variables
   * src/daemon/executor.rs → client_pane: Present in find_best_target_pane but no longer used since we removed
     the "Invocation Source" priority.


  4. Code Suppressions
   * Global: Remove various #[allow(dead_code)] markers across these files once the assets above are pruned.

  5. The following assets are unused but we need a plan to integrate these fields into the AI context summary: RichPaneInfo.window_name, .dead, or .dead_status in src/tmux/pane.rs.

