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
| 3 | 01 | `ai/types/pending.rs` · `PendingCall::to_tool_call` | smell | minor | 617-line match in an exempt file | open: PE decides whether the exemption covers functions |
