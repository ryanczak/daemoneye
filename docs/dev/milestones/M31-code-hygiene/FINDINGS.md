# M31 findings ledger

Everything the executor saw while moving code, triaged at review (README
decision 7). The executor appends nothing here; it writes the phase's
`### Findings` entry, and the architect carries each item over with a
triage. Severity is the executor's; the triage and the final severity are
the architect's, with the PE's say on `major` and above.

**Triage values:** `act: <phase or bug doc>` — fixed in M31 or filed;
`defer: <milestone>` — carried; `reject: <why>` — not a defect.

| # | Phase | File · symbol | Kind | Severity | What | Triage |
|---|---|---|---|---|---|---|
| 1 | 01 | `daemon/ghost.rs` · `do_ghost_turn` | smell | major | 896-line function missed by the audit | act: group 3 phase with the dispatch matches |
| 2 | 01 | `server/ask.rs` `handle_ask`, `executor/foreground.rs` `run_background`, `cli/status.rs` `run_status`, `tmux/cache.rs` `get_labeled_context_scoped` | smell | minor | 321–573-line functions missed by the audit | act: allowlisted; split in their files' phases |
| 3 | 01 | `ai/types/pending.rs` · `PendingCall::to_tool_call` | smell | minor | 617-line match in an exempt file | reject: the exemption covers functions (PE, 2026-10-25) |
| 4 | 02 | `shells_tests.rs` `shell_test_home`, `session_tests.rs` `test_home`, and copies in `shell/launch.rs`, `executor/{ghost_remote,bg_shell,shell_exec,foreground,ghost_sandbox}.rs` | smell | minor | eight copies of one temp-HOME fixture beside the existing `crate::test_home_guard()` | act: a later M31 phase, one shared `test_home()` |
| 5 | 02 | PTY tests in `shell/{backend,client,session}_tests.rs`, `server/shells_tests.rs` | perf | minor | 50+ wall-clock deadline poll loops (10–50 ms) waiting on real output | defer: a test-speed pass after M31 |
| 6 | 02 | `client_tests.rs` (3.3 s of sleeps), `backend_tests.rs` (2 × 300 ms after `Int`) | perf | nit | bare sleeps; the client one is semantic (a pause must outlast a deadline) | defer: with #5 |
| 7 | 02 | `ai/tools/dispatch_tests.rs` | smell | nit | 1295 lines in banner-delimited sections that would read better split | reject: under the 2000-line cap; revisit only if it grows |
| 8 | 03 | `search_tests.rs` · `static COUNTER: AtomicU64` | smell | nit | hand-rolled unique temp-dir names, same family as #4 | act: with #4, one shared test-helper phase |
| 9 | 03 | `scheduled_tests.rs` · `presence_count` expectations | smell | nit | two tests recompute expected counts inline | reject: two sites, a helper would not pay for itself |
| 10 | 04 | `types.rs` · `SessionsConfig.auto_name_enabled`, `ApprovalsConfig.commands` | smell | nit | serde names `default_true`, `impl Default` writes a literal `true`; they agree today | defer: a config-defaults tidy after M31 |
| 11 | 05 | `server/sessions.rs` · `handle_load_session` | bug | minor | an unknown `session_id` makes the write-back a no-op, yet `SessionLoaded` is replied; the client thinks it resumed | defer: with #12, one unknown-session guard for all five handlers, after M31 |
| 12 | 05 | `server/sessions.rs` · `handle_save_session` | smell | minor | an unknown `session_id` saves an empty session (0 turns, model `default`) instead of an error | defer: with #11 |
| 13 | 05 | `server/sessions.rs` · `handle_load_session` banner | smell | nit | with `load_recent_turns` set, `turn_count` is the full saved count while `message_count` is the truncated one | defer: with #11 |
| 14 | 05 | `server/sessions.rs` · `handle_list_saved_sessions`, `handle_rename_saved_session` | perf | nit | one `meta.toml` read per session on list; rename sweeps every store entry | reject: fine at current scale |
| 15 | 05 | `server/sessions.rs` | smell | nit | `crate::session_store::` full paths beside glob imports | reject: style only |
| 16 | 06 | `daemon/startup.rs` · `run_daemon` | smell | minor | six pieces of shared state (`startup_config`, `shutdown`, `cache`, `sessions`, `schedule_store`, `initial_session`) cross its sections; the four `tokio::spawn(supervise(…))` blocks are self-contained seams (~250 lines); startup order lock → config → tmux-detect → cache → supervisors → bind is load-bearing | act: input to the `run_daemon` function-split phase in group 3 |
| 17 | 06 | `daemon/startup.rs` | smell | nit | `supervise`, `log_event`, `handle_client` and `SessionStore` arrive through `use super::*` and glob re-exports, so the module's real dependencies are implicit | defer: an explicit-imports tidy after M31 |
| 18 | 07 | `context/epoch_extract.rs` · `apply_extraction` | smell | nit | an `add_memory` failure is dropped from the count without a log line; best-effort is the documented contract | defer: with the post-M31 logging tidy |
| 19 | 07 | `context/epoch_extract.rs` · `apply_extraction` | smell | nit | `_session_id` and `_epoch_seq` are accepted and unused | reject: kept for provenance, harmless |
| 20 | 08 | `cli/commands/slash_session.rs` · `cmd_session` load arm | bug | minor | the client rehydrates the transcript from the **full** session file on its own filesystem while the daemon loads only `load_recent_turns`; on a client off the daemon's host the file is absent and the visible transcript is silently cleared | defer: with #11–#13, one session-load fix after M31 |
| 21 | 08 | `cli/commands/slash_session.rs` · `cmd_session` | smell | nit | six arms repeat one request/reply shape; seams are one function per subcommand plus `rehydrate_transcript` | act: input to the function-split phase for `cmd_session` (155) |
| 22 | 08 | `cli/commands/slash_session.rs` · `cmd_session`, `cmd_stop` | smell | nit | clear-then-push loop where `extend` fits; `cmd_stop` drops the sentinel path the CLI prints | reject: style and parity only |
| 23 | 09 | `cli/viewer_layout.rs` · `wrap_words` | bug | minor | the overlong-token path gates on byte length but counts chars; a mid-UTF-8 chunk falls back to the whole token, so a non-ASCII token over the width in bytes is rendered once per chunk (30 CJK chars at width 80 shows twice), and one over the width in chars underflows `space_remaining` (debug panic) | defer: a post-M31 wrap fix (char-boundary chunking, with a CJK/emoji test) |
| 24 | 09 | `cli/viewer_layout.rs` · `wrap_words` | smell | nit | `flush` is a closure rebuilt per hard line over three `&mut` locals | defer: with #23 |
| 25 | 09 | `cli/viewer_layout.rs` · `layout_block_collapsed` | perf | nit | lays out the whole block into a throwaway `Vec` to count its rows | reject: small blocks, one call site |
| 26 | 09 | `cli/viewer_layout.rs` · `wrap_words` | smell | nit | the whitespace-only line handling (`has_content = true` on a blank body) is load-bearing for the round-trip and undocumented | defer: with #23 |
| 27 | 10 | `cli/tail_view.rs` · `run_log_viewer`, `run_tail_viewer`; `cli/attach.rs` · `run_attach` | smell | minor | three copies of the alternate-screen setup: enter, `AltScreenGuard` teardown, terminal, key and resize streams | defer: a post-M31 viewer tidy, one `enter_alt_screen` helper |
| 28 | 10 | `cli/tail_view.rs` · `run_log_viewer` | smell | nit | "no frame source" is expressed as an empty reader passed to `tail_loop` | defer: with #27 |
| 29 | 10 | `cli/tail_view.rs` · `tail_loop` | smell | nit | a paused replay's clock advances only between keys, and the status line does not say so | reject: documented in code, matches the skip affordance |
| 30 | 10 | `cli/tail_view.rs` · `render_tail` | smell | nit | the status `match`'s guard order (attached, replay, log, search) is load-bearing and unguarded | defer: with #27 |
| 31 | 10 | `cli/tail_view.rs` · `color` | smell | nit | one arm per ANSI colour where a table would be shorter | reject: the explicit arms show the theme mapping, as the executor notes |
| 32 | 11 | `ipc_types.rs` · `ContainerInfo.image` | smell | nit | the only producer (`sbx_lifecycle.rs` `sandbox_rows`) hard-codes `"shell"`, and no renderer prints it | defer: a post-M31 sbx payload tidy; the field is wire format |
| 33 | 11 | `ipc_types.rs` · `ContainerInfo.session`, `is_ghost` docs | smell | nit | the docs name the docker-era `de.session` / `de.ghost=1` labels; the source is the `de-ghost-` / `de-chat-` sandbox name prefix | defer: with #32 |
| 34 | 11 | `ipc_types.rs` · `GhostConfig.auto_approve_scripts` doc | smell | nit | the prose predates `auto_approve_commands` and `tool_policy` and does not cross-reference them | reject: doc polish only |
| 35 | 12 | `file_ops/ops_copy.rs` · `run_copy_with_spawn` | smell | minor | 213 lines doing validation, the remote branch, local pre-flight and the local commit; seams are `run_copy_remote` / `run_copy_local` split on host, and `run_delete_with_spawn` (137) has the same shape | act: input to the function-split phase for `run_copy_with_spawn` |
| 36 | 12 | `file_ops/ops_copy.rs` · `run_copy_with_spawn` remote branch | smell | nit | the remote shell string is built before the approval prompt | defer: with #35, when the function is split |
| 37 | 12 | `file_ops/ops_copy.rs` · dest-exists check | smell | nit | the local branch rejects an existing destination before approval, the remote one inside the shell after it, with two message formats | defer: with #35 |
| 38 | 12 | `file_ops/ops_delete.rs`, `ops_copy.rs` · remote sentinel scan | smell | nit | the result scan matches `DE_OK:` / `DE_ERROR:` anywhere in the output, so a path containing `DE_ERROR:` misreads a success; `__DE_DONE__` is echoed and never checked | defer: a post-M31 remote-write sentinel tidy |
| 39 | 12 | `file_ops/ops_delete.rs` · local branch | perf | nit | reads the whole file only to count its lines for the prompt and result | reject: fine at these sizes |
| 40 | 12 | `file_ops/ops_copy.rs` · `tmp_path` | smell | nit | `with_extension("de_tmp")` is not unique per invocation; a failed rename cleans up | defer: with #38 |
| 41 | 13 | `cli/commands/top_tui_draw.rs` · `runs_window` | perf | nit | allocates a full index vector every frame in the common, non-overflowing case | reject: a handful of rows per frame |
| 42 | 13 | `cli/commands/top_tui_draw.rs` · `window_text` | perf | nit | rebuilds each panel body as a fresh `String`, and `lines().nth(i)` makes it quadratic in the body's line count | defer: a post-M31 `top` tidy, if large activity logs ever show up in a profile |
| 43 | 13 | `cli/commands/top_tui_draw.rs` · `activity_window_text` | smell | nit | a one-line alias of `window_text` | defer: with #42 |
| 44 | 13 | `cli/commands/top_tui_draw.rs` · `draw_panel` | perf | nit | `fades.contains(&i)` per line is linear | reject: panel row counts are small |
