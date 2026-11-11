# M36 — Chat UX and audit fixes

**Goal:** the four chat and `top` improvements in
[`docs/design/2.7.0-UX-improvements.md`](../../../design/2.7.0-UX-improvements.md)
land, and every `critical` and `major` row of the 2026-11-01 codebase audit
([`FINDINGS.md`](FINDINGS.md)) is fixed with a test or closed with a reason.

**Status:** in-progress

**Release:** 2.8.0

**Depends on:** M35 (the tree as released in 2.7.0).

**Why now.** The 2.7.0 live checks surfaced four UX faults the PE wrote
down: the input box's side borders break a multi-line paste, `/approvals`
can toggle only one of its categories, the status bar is too dim and says
nothing about the turn or the context budget, and `top` can list a shell
but not watch it. Scoping them was the occasion for a full read-only audit
of the tree, which found 28 defects — three of them in the approval prompt
itself (the panel can hide the end of the command it asks about, a typed
redirect starting with `a` approves for the session, a pasted password
leaks into the next chat input). The UX work and the approval fixes touch
the same code, so they ship together.

**PE decisions (2026-11-01, at scoping):**

1. **Numbering.** This is M36 and releases as 2.8.0; MCP surface becomes
   M37 and Knowledge M38.
2. **`/approvals` covers every prompt kind:** commands, sudo, scripts,
   runbooks, files, schedules, agents, deletes (script, runbook, agent)
   and the per-host grants, each with `on|off`, plus `all on|off`. The
   list also names the kinds nothing can auto-approve — a new host, a
   ghost's escape hatch, the user's own `/shell` shell, a sudo file edit —
   as *always asked*. Changes last for the chat session only; `config.toml
   [approvals]` still seeds each session and gains no new keys this
   milestone.
3. **Status bar:** `turn · context · model · session · up`, context amber
   at ≥ 75 % and red at ≥ 90 %, with the ⚡approvals hint at the right
   edge, truncated first when the pane is narrow. Text in a readable
   mid-gray, no `DIM`.
4. **Approval keys:** `y` / `n` / `a` echo in the box and take effect on
   Enter; any further character turns the line into a redirect message.
5. **Shell reclamation is the full fix** (B11): a chat owner's shells
   close when its presence connection ends, a sidecar persists each shell's
   owner and host so a restart re-adopts it under its owner, and adopted
   shells past an idle limit are reaped.
6. **Ghost sudo tightens** (S6): a sudo-bearing ghost command auto-approves
   only when the whole command is `[sudo] <approved script> [args]`, and
   sudo is detected at any command position. A denied chain is logged with
   its reason so the runbook can be split.
7. **`top` attach is read-only.** The view reuses the `/tail` model and the
   `ShellTail` stream; no key sends input or a signal to the shell.
8. **The daemon maintains its own `config.toml`** (added 2026-11-01, after
   scoping). `read_file` and `edit_file` open up for
   `~/.daemoneye/etc/config.toml` alone — `etc/prompts/sre.toml` and the
   rest of the directory stay blocked. A config edit is **always asked**:
   no `files` session grant and no `[approvals] file_edits = true` covers
   it, the panel says so, and secret values (`api_key`, tokens) show masked
   in the diff and in `read_file`'s output; the write itself uses the
   model's real old/new strings, so a masked value can never be written
   back. The new content is parsed and validated as a `Config` before the
   write, so a bad edit is refused with the TOML error and the daemon never
   fails to start on its own change. **The daemon reloads:** a successful
   write, `daemoneye reload` and a `Reload` IPC request re-read
   `config.toml` and swap what can be swapped at runtime — models and
   pricing, approvals seeds, limits, compaction, sessions, retention,
   masking patterns, stream timeouts, budget; what is fixed at startup (the
   socket, the webhook bind, the sandbox, the shell substrate) waits for a
   restart, and the reload's reply names which keys changed and which wait.
   A built-in `config-reference` knowledge memory documents every
   `config.toml` key with its type, range, default and when a change takes
   effect, seeded like the other knowledge memories and held to the
   `Config` structs by `tests/doc_truth.rs` the way the template already
   is.
9. **OSC 7501 program status** (added 2026-11-03, PE request). DaemonEye
   speaks the Program Status Protocol
   ([digest](osc7501-spec.md), from
   <https://mitchellh.com/writing/program-status-osc7501>) on both sides.
   **Consume:** the daemon's PTY shells parse the reports the programs in
   them emit (Claude Code, cargo, terraform, …) into per-shell records, held
   to the spec's grammar, limits, lifetime and security rules (a bad report
   is dropped whole; `msg`/`title` are masked like other shell output); the
   records reach the AI and the user where a shell's state already shows —
   `list_shells`, `read_shell_screen`'s header, `watch_shell` (wait until a
   program reports `blocked`/`done`/`error`) and `top`'s Shells panel — and
   the program's own word outranks the screen-scraped `PaneStatus`. Only
   the feature-detection reply is ever written back. **Emit:** the chat
   client reports its own state to the user's terminal — `working` during a
   turn, `blocked:kind=permission` (or `auth` at a credential prompt) while
   an approval waits, `idle` between turns, `clear` on exit. Scope, the
   tmux passthrough question the spec leaves open, and whether this is one
   phase or two are settled at drafting.

**Rules every phase carries:**

- **A defect fix comes with a test that fails on the pre-phase tree.** The
  phase doc names the test and what it asserts; the executor pastes the red
  run.
- **A UX change pins behaviour on a `TestBackend`**, not pixels: what a
  row contains or does not contain, where the cursor sits, what a key does.
- **The phase marks its findings fixed** in the ledger row's triage cell
  (`fixed: M36 phase-NN, <test name>`).
- **The function cap is a ratchet** (PE, 2026-10-31): a fix that grows an
  allowlisted function raises its `tests/hygiene_allowlist.txt` line and
  says why in the Update Log; nothing is squeezed to fit.
- **Every Spec step terminates:** named commands or files, and the output
  that ends the step.
- **CLAUDE.md and README.md follow the code** when a phase changes a tool's
  behaviour or a slash command's surface; `tests/doc_truth.rs` is the gate.

**Exit criteria:**

- The four items of `docs/design/2.7.0-UX-improvements.md` pass a live
  check on pinky: a three-line paste into the input box arrives as three
  lines with no `│`; `/approvals` lists every category with its state and
  `/approvals files off` makes the next `edit_file` prompt; the status bar
  shows the turn and `ctx N/M (p%)` after the first reply; in `top`, Enter
  on a shell row shows that shell's screen updating live and `q` returns.
- No `critical` or `major` row in `FINDINGS.md` is triaged `act:`; each is
  `fixed:` or closed with a reason the PE accepted.
- The approval panel on an 80-column pane shows every character of the
  command, wrapping by display width, with control characters visible.
- Typing `abort` at an approval prompt denies it and sends `abort` as the
  redirect; a bracketed paste at the credential prompt is the credential.
- `daemoneye ask hi` with no daemon leaves the tty in cooked mode.
- A `[Background Task Completed]` that lands during a turn is in the
  session's history after the turn, and `read_shell` on a `wait: false`
  shell returns the command's output.
- After `daemoneye chat` exits, its shells are gone from `list_shells`
  within a minute; after a daemon restart, a surviving `/shell` shell is
  listed under its session, not as `adopted`.
- Asked in chat to raise `[limits]`'s per-turn tool cap, the AI reads
  `config.toml` (the `api_key` line masked), the edit prompt says it cannot
  be session-approved, the diff shows the key masked, and after approval
  `daemoneye status` reports the new limit without a restart; an edit that
  leaves the file unparseable is refused naming the TOML error and the
  file is unchanged. `read_memory config-reference` lists the edited key
  with its range and default.
- A program in a daemon shell that emits `OSC 7501 ; state=blocked:kind=permission:app=x ST`
  shows as blocked (with its `app`) in `list_shells` and `top`, and
  `watch_shell` on it returns on that report; during a chat approval prompt
  a 7501-aware outer terminal sees the client as `blocked`.
- The four gates pass; `cargo test` passes at 2637 tests plus the new ones.
- `2.8.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/design/2.7.0-UX-improvements.md` — the four PE items.
- `docs/dev/milestones/M36-ux-and-audit/FINDINGS.md` — the audit ledger.
- `docs/architecture.md` § 1 System layers (the client and the shell engine), § 2 Major data flows (the approval round trip).
- `src/cli/render_ratatui.rs`, `src/cli/commands/approval.rs`,
  `src/cli/commands/slash.rs`, `src/cli/tail.rs`, `src/cli/commands/top_tui.rs`.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Expected count: 28 (phase 21 split into 21a/21b/21c on 2026-11-09; phase 20 split into 20a/20b on 2026-11-08; phase 04 split into 04a/04b on 2026-11-03; 21 added 2026-11-03; 05 split into 05a/05b on 2026-11-04; 17 split into 17a/17b/17c on 2026-11-07).
Phases 01–04 are the PE's four items; 05–07 the approval-prompt findings
that share their code; 08–16 the remaining `major` rows, most severe
first; 17 the shell-lifecycle fix; 18 the performance tail; 19–20 the
self-maintained config (decision 8), reload first so the edit can use it. 21a–21c are OSC 7501 (decision 9), after the
config so the shell-state surfaces it extends have settled.

| #  | Phase | Scope | Status |
|----|-------|-------|--------|
| 01 | input-box-paste ([phase-01-input-box-paste.md](phase-01-input-box-paste.md)) — the input box keeps its top and bottom rules and draws a space where its side borders were, so a multi-line paste arrives intact; cursor and scroll math unchanged | design item 1 | done |
| 02 | status-bar ([phase-02-status-bar.md](phase-02-status-bar.md)) — `turn · ctx N/M (p%) · model · session · up`, readable gray, amber/red budget thresholds, the approvals hint at the right edge; the renderer learns the turn and the budget | design item 3 | done |
| 03 | approvals-categories ([phase-03-approvals-categories.md](phase-03-approvals-categories.md)) — `SessionApproval` gains a flag per prompt kind (schedules, agents, script/runbook/agent deletes); `/approvals [list]` shows every category and the always-asked kinds; `/approvals <category> on\|off`, `/approvals all on\|off`; every client prompt arm consults its flag | design item 2 | done        |
| 04a | shells-select ([phase-04a-shells-select.md](phase-04a-shells-select.md)) — `top`'s Shells panel gets a selected row (keys move it, refresh keeps it by id); the disk refresh runs on a blocking thread, one in flight (split from 04 on 2026-11-03) | design item 4, P5 | done |
| 04b | top-attach ([phase-04b-top-attach.md](phase-04b-top-attach.md)) — Enter (or `a`) on the selected shell opens it read-only through `ShellTail` + `TailModel`, `q`/Esc returns to the panels as they were | design item 4 | done |
| 05a | panel-wrap ([phase-05a-panel-wrap.md](phase-05a-panel-wrap.md)) — `wrap_display` replaces `wrap_line_hard`: rows measured by display width (`unicode-width`), control and bidi characters shown visibly, used by the approval panel and the viewer's hard wrap (split from 05 on 2026-11-04) | S1 | done |
| 05b | markdown-wrap ([phase-05b-markdown-wrap.md](phase-05b-markdown-wrap.md)) — every committed markdown row fits its width (one span-level split in `feed_to_lines`/`flush_to_lines`, styles kept); `visual_len` and the viewer's `wrap_words` measure display columns; the viewer's output rows expand tabs | B8, P6 (wrapping), L6 | done |
| 06 | approval-diff ([phase-06-approval-diff.md](phase-06-approval-diff.md)) — write and edit approvals commit their diff as styled spans (no literal `[32m`), wrapped, with the row count shown and the full content one key away when clipped | B7 | done |
| 07 | approval-input — the approval and credential readers go through `read_key`: y/n/a on Enter, bracketed paste as one value, focus events and arrows ignored, UTF-8 decoded | S2, S3 | done |
| 08 | char-boundary-panics ([phase-08-char-boundary-panics.md](phase-08-char-boundary-panics.md)) — the webhook fingerprint, `PendingCall::summary`, the malformed-frame logging and the ISO duration overflow stop panicking on model- or sender-supplied text | B3, B4, B16, B14 | done |
| 09 | terminal-restore ([phase-09-terminal-restore.md](phase-09-terminal-restore.md)) — an RAII guard plus panic hook restores the tty on every exit of `ask` and `chat`; `top --once` escapes control characters | B6, S9 | done |
| 10 | secrets ([phase-10-secrets.md](phase-10-secrets.md)) — the Gemini key moves to the `x-goog-api-key` header and reqwest errors drop their URL; masking covers AWS keys, JSON-quoted secrets, `Bearer` tokens and `sk-` keys; invalid-args warnings log masked arguments | S4, S5, S10 | done |
| 11 | sse-utf8 ([phase-11-sse-utf8.md](phase-11-sse-utf8.md)) — `SseBuffer` holds bytes and decodes whole lines; a malformed tool-argument frame becomes an `invalid_args` tool result and its usage is recorded | B5, B15 | done |
| 12 | mid-turn-messages ([phase-12-mid-turn-messages.md](phase-12-mid-turn-messages.md)) — the turn's write-back keeps the messages the entry gained after its snapshot, in order, on every ending path; the compaction rewrite includes them | B1 | done |
| 13 | ghost-slot-and-sudo ([phase-13-ghost-slot-and-sudo.md](phase-13-ghost-slot-and-sudo.md)) — `trigger_ghost_turn` releases the concurrency slot on every exit (a guard); the whole-command sudo rule and position-independent detection; `await_agent_result` validates its names | B2, S6, L1 | done |
| 14 | read-file-bounds ([phase-14-read-file-bounds.md](phase-14-read-file-bounds.md)) — `read_file` refuses non-regular files, streams to `offset + limit` under a byte cap off the runtime; the `~/.daemoneye` guard compares canonical with canonical | S8, S7 | done |
| 15 | shell-client-scan ([phase-15-shell-client-scan.md](phase-15-shell-client-scan.md)) — `read_to_marker` scans only new bytes (marker-length overlap), offers chunks without re-decoding, and a `Lagged` subscriber becomes an explicit error frame; `MetaIndex` saves atomically and fails on a corrupt index | P1, B13 | done |
| 16 | background-index ([phase-16-background-index.md](phase-16-background-index.md)) — `wait: false` and timed-out commands get their `.meta.json` record when the outcome arrives; a timeout sends the shell `^C` and names the shell in the reply | B9, B10 | done        |
| 17a | detach-close ([phase-17a-detach-close.md](phase-17a-detach-close.md)) — a chat session's non-dedicated shells close `[shells] detach_close_secs` (default 300) after its last presence detach unless it re-attaches; shell-host children are waited on, so exited hosts are not zombies (split from 17 on 2026-11-07) | B11 (first half), B12 | done        |
| 17b | owner-sidecar ([phase-17b-owner-sidecar.md](phase-17b-owner-sidecar.md)) — `<id>.owner` records each shell's owner key and host at spawn; `adopt_at_startup` restores them (absent or invalid → `Adopted`/local as today); `sweep_stale` removes it (split 17b/17c on 2026-11-07) | B11 (second half) | done        |
| 17c | idle-reap ([phase-17c-idle-reap.md](phase-17c-idle-reap.md)) — the maintenance tick closes shells (never `/shell` ones, never one with a command in flight, never a chat shell whose session is present) whose recording has been silent for `[shells] idle_reap_secs` | B11 (second half) | done        |
| 18 | perf-tail ([phase-18-perf-tail.md](phase-18-perf-tail.md)) — `read_shell_log` walks records with a cursor off the runtime; one index connection and schema check per daemon, the offset from the append; prompt assembly stops rebuilding an empty corpus each turn; `ip_limits` pruned; the `build_match_expr` sentence in CLAUDE.md | P2, P3, B17, P4, P8, L2 | done        |
| 19 | config-reload ([phase-19-config-reload.md](phase-19-config-reload.md)) — the daemon holds its `Config` behind one shared, swappable handle that every per-request reader goes through; `Request::Reload` / `daemoneye reload` re-read and validate `config.toml`, swap the runtime-safe sections, re-init masking and stream timeouts, and reply with the keys that changed and the keys that wait for a restart; an invalid file is refused and the running config stays | decision 8 | done        |
| 20a | self-config ([phase-20a-self-config.md](phase-20a-self-config.md)) — `read_file` and `edit_file` admit `etc/config.toml` alone: masked read, an always-asked edit with a masked diff, `Config` validation before the write, a reload after it whose report the result carries; the `read_file` / `edit_file` rows in CLAUDE.md and README follow (split 20a/20b on 2026-11-08) | decision 8 | done        |
| 20b | config-reference ([phase-20b-config-reference.md](phase-20b-config-reference.md)) — the `config-reference` knowledge memory documents every `config.toml` key (type, range, default, when a change takes effect), seeded and in `--overwrite-memory`, gated by `doc_truth` against the `Config` structs (keys and the reload/restart column); `sre.toml` stops calling the config blocked | decision 8 | done        |
| 21a | program-status-host ([phase-21a-program-status-host.md](phase-21a-program-status-host.md)) — shell hosts parse OSC 7501 reports into per-shell records (grammar, limits, lifetime, security), answer only the feature query, carry the records on `Status`; `read_shell_screen`'s header shows them masked (split 21a/21b/21c on 2026-11-09) | decision 9 | done |
| 21b | program-status-surfaces ([phase-21b-program-status-surfaces.md](phase-21b-program-status-surfaces.md)) — `list_shells`, `ShellInfo` and `top`'s Shells panel show each shell's program status from a bounded live refresh; `watch_shell` returns on a `blocked`/`done`/`error` report; the program's word outranks the screen-scraped state | decision 9 | done        |
| 21c | program-status-emit ([phase-21c-program-status-emit.md](phase-21c-program-status-emit.md)) — the chat client reports `working` / `blocked:kind=permission\|auth\|question` / `idle` / `clear` to its terminal, only on change, only to a tty; inside tmux the report rides the passthrough envelope and the client sets `allow-passthrough on` for its own pane, unset again at exit (PE, 2026-11-11) | decision 9 | todo |

## Notes

- **Scoped 2026-11-01.** The design doc was written by the PE after the
  2.7.0 release; the audit ran as four read-only passes over the whole
  tree the same day. Two of its findings were re-read by the architect
  before scoping: B1 (the three write-back sites and the start-of-turn
  clone in `server/ask.rs`) and S2 (the first-byte match in
  `read_approval_input_with`). B1 is the probable cause of the "drops
  completion events" half of GitHub issue #1, whose other two defects M35
  fixed; the issue is re-checked at this milestone's live check.
- **Decision 8 added 2026-11-01**, after the first scoping pass, when the
  PE asked for a self-maintained config. Measured at scoping: `Config` is
  loaded once in `run_daemon` and cloned into every subsystem, there is no
  reload path (no SIGHUP handler, no `Reload` request, no `restart`
  subcommand), and `EditFilePrompt` carries the file's old and new content
  to the client unmasked — which is why the reload is its own phase and
  why the config edit masks its diff.
- **Out of scope:** F8 (ghost foreground) stays unscheduled; config keys
  for the new approval categories (decision 2); `/approvals save`; the
  viewer's per-key relayout (P6, deferred); B18 and B19 (deferred with
  reasons in the ledger); opening `etc/prompts/sre.toml` or any other
  `~/.daemoneye/` path to `edit_file`; a reload of what is fixed at startup
  (socket, webhook bind, sandbox, shell substrate).
