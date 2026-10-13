# DaemonEye post-2.0 TODO

Work deferred at the M27 close (2026-10-12), after `v2.0.0` was tagged.
None of it blocked the release. The source for each item is the M27
retrospective (`docs/dev/milestones/M27-tmux-observer/README.md`) and the
phase docs it cites. Code locations were current at `v2.0.0`. Re-check them
before scoping, since they will drift.

## 1. Unify session residency (design)

**Problem.** The daemon has three separate views of "the session":

- the in-memory `SessionStore` (`src/daemon/session.rs`);
- the presence registry, which tracks attached chats;
- the disk history: `var/log/sessions/<id>.jsonl`, plus the
  non-resident detach registry `NON_RESIDENT_DETACHES`.

A session is resident only after its first `Ask` since the daemon started.
`handle_ask` (`src/daemon/server/ask.rs`) reads the disk history only when
the session is *not* resident, so an empty resident entry would hide it.

Each fix in M27 reconciled one pair of these views, and the gap came back
three times:

- **F5:** no catch-up brief after `chat --resume`.
- **F8:** alerts injected after a restart missed a reconnected chat
  (fixed in 07d by injecting into presence ids too).
- **F11 and bug-07e-1:** no brief, and no alert in the history, after a
  restart followed by a quit with no ask (fixed in 07e by a non-resident
  detach registry, which injection also reads).

**Want.** One notion of a session that may not be resident, shared by
presence attach and detach, `inject_into_sessions`
(`src/webhook/process.rs`), `attach_brief_for`
(`src/daemon/server/chat_presence.rs`) and `handle_ask`. Options to weigh:

- a lightweight "known session" record created on attach;
- making a session resident on presence attach, by loading its history
  from disk;
- a single registry that replaces both the presence-id union and
  `NON_RESIDENT_DETACHES`.

**Done when** a test drives each real sequence end to end and passes:

- restart, then reconnect, then an alert, then an ask;
- restart, then reconnect, then a quit with no ask, then an alert, then a
  resume;
- a fresh `chat --resume` of a session the daemon has never loaded.

Also verify the same sequences live, the way phase 07's S2 does.

## 2. F9 — `[ACTIVE PANE]` labels the wrong pane

When the chat pane is excluded from the context (it is the client's own
pane), the `[ACTIVE PANE …]` label lands on the next pane. The model then
reads that pane as the one the user is looking at. Seen in rehearsal 2 and
again in the PE run, where the context held two `[ACTIVE PANE]` lines,
`%1` and `%0`. The code is in `src/tmux/cache.rs`, in the labelled-context
builder (the `"[ACTIVE PANE {}…]"` format). This is a design question: what
should the label mean when the chat pane is excluded, "the user's focus"
or "the pane the user should act on"? This only applies inside tmux.

## 3. Invalid-args error always says "optional"

`invalid_args_event` (`src/ai/tools/dispatch.rs`) writes the type-mismatch
message as `parameter '<name> (<type>, optional)' is …` for every
parameter, required or not. The expected-params list a few lines below it
gets this right (`if p.required { "required" } else { "optional" }`).
Cosmetic. Found in rehearsal 3: `timeout_secs (integer, optional)` was
correct there only by luck.

## 4. `InvalidArgs` inherits a gated tool's budget exemption

`PendingCall::InvalidArgs::tool_name()` (`src/ai/types/pending.rs`)
returns the name of the tool the model *meant* to call. The per-turn,
per-session and per-tool budget caps in `daemon::stream` exempt any tool
listed in `APPROVAL_GATED_TOOLS` (`src/ai/tools/defs.rs`). So a model that
keeps sending bad arguments to an approval-gated tool (for example
`edit_file`) is never capped, even though nothing prompts and nothing runs.
Decide whether invalid-args calls should count against the caps, for
example by keying the exemption on the variant rather than the name.
Noted in 07d's review.

## 5. Non-resident detach record never consumed

`record_chat_detach` (`src/daemon/session.rs`) writes a
`NON_RESIDENT_DETACHES` entry when a chat leaves while its session is not
resident. Only the next `attach_brief_for` removes it. A chat that never
comes back leaves its entry until the daemon restarts, and injection keeps
appending alerts to that session's history file meanwhile. The leak is
small and bounded by the number of sessions. Fix options: expire entries
after the sessions-index retention, or drop them when the session is
deleted. Also, `NON_RESIDENT_DETACHES` is `pub` only so a test can backdate
it, and `pub(crate)` would do. Item 1 may make this item moot.

## 6. 1.x wording in logs and approval records

- daemon.log says `foreground command approved: <cmd>` and
  `background command approved: <cmd>`
  (`src/daemon/executor/mod.rs`, `log::info!("{} command approved: {}", …)`).
  In 2.0 the distinction is "waited" vs `wait: false`, and every command
  runs in a PTY shell.
- `command` events in `events.jsonl` carry `mode` values from 1.x
  (`foreground` / `background` on approval decisions, `shell` on runs) and
  a `pane` field that now holds a shell id. Fields are documented in
  README § "Command audit log". Renaming them would break readers of
  existing logs, so plan a compatible change, such as new fields alongside
  the old ones, or a version bump in the record.
