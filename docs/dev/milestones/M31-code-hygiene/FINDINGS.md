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
