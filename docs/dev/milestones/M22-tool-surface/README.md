# M22 — Tool surface & context

**Goal:** make the AI stop reasoning in tmux panes. Replace the six pane tools
with nine shell tools over M20's substrate, reshape `run_terminal_command`
around shells and hosts, add `Category`/`mutates_state()` to `TOOLS` (which
M25's governor needs), swap the pane context blocks for `[HOSTS]`/`[SHELLS]`,
and rewrite every prompt asset and seeded memory that still speaks tmux.

**Status:** in-progress — scoped 2026-09-12, **PE sign-off on the breakdown 2026-09-12**.
Ordering chosen by the PE at the M24 close: M22 ahead of M21 and M23, because
`Category` is M25's stated dependency and because M21's `host?` params and
M23's `[HOSTS]/[SHELLS]` blocks both land on a cleaner surface once this is
done.

**Depends on:** M20 (the shell engine the new tools drive) and M24 (the Run
model `list_shells`/`get_shell_context` scope by). Not on M21 or M23.

## Exit criteria

- **Zero `%`-pane identifiers in any prompt asset or tool description** — the
  design's own criterion. Measured at scoping: `assets/prompts/sre.toml` has
  **47** pane/tmux references, `src/ai/tools/defs.rs` has **78**, and **5**
  seeded memory files mention tmux.
- **42 tools, 28 core + 14 deferred**, with `tests/doc_truth.rs`, `CLAUDE.md`
  and `README.md` in sync — the gate that already exists keeps them so.
- **`Category` and `mutates_state()` on every `ToolDef`**, so M25 can be scoped
  without a prerequisite of its own.
- **`[HOSTS]` / `[SHELLS]` / `[DEFAULT SHELL]` replace the pane blocks** in the
  prompt context, and the approval panel names the shell and host a command
  will run on.
- **`[execution] backend` defaults to `"pty"`** and the four run kinds — chat,
  ghost, scheduled, background — are live-verified on the substrate.
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
  Live checks architect-run per the M14 convention (isolated `HOME`, canned-AI
  stub; the shell log and the run turn-log as evidence anchors).

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 "The AI tool surface (36 → 42)" — the
  removals, the changed signatures, the twelve-row added table, and the
  `[HOSTS]`/`[SHELLS]` context sketch to quote verbatim in phase docs.
- § 2.3 "Run manager" — `list_shells` and `get_shell_context` take a `scope`
  that is defined in terms of the `Run` M24 built.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — **eleven steps**, and
  `tests/doc_truth.rs` enforces steps 10 and 11. Nine new tools means nine
  passes through it; a phase that adds a tool and skips the tables fails a gate
  that already exists.

## Phases

Proposed, pre-measurement — **each will be re-derived and may split at drafting
on a measured seam** (M24 ran 10 rows → 16 phases that way). Ordering
constraints: `Category` is pure and unblocks M25, so it goes early; the six
removals can only follow their replacements; the prompt assets describe the
final surface, so they go last.

| # | Phase | Status |
|----|-------|--------|
| 01 | pty-default ([phase-01-pty-default.md](phase-01-pty-default.md)) — flip `[execution] backend` to `"pty"`; `"tmux"` becomes the explicit opt-out and an invalid value falls back to the default; stale comments/template/README corrected | done                            |
| 02 | tool-category ([phase-02-tool-category.md](phase-02-tool-category.md)) — `Category` + `mutates_state()` on every `ToolDef`; pure, additive, unblocks M25 | done        |
| 03a | list-shells ([phase-03a-list-shells.md](phase-03a-list-shells.md)) — `list_shells { scope? }`, core + `Category::Read`; establishes the eleven-step wiring on one tool | done        |
| 03b | read-shell ([phase-03b-read-shell.md](phase-03b-read-shell.md)) — `read_shell { shell, lines?, since_command?, filter? }` over the asciicast log; builds the read-one-shell primitive | done        |
| 03c | find-in-shells ([phase-03c-find-in-shells.md](phase-03c-find-in-shells.md)) — `find_in_shells { pattern, scope? }`, 03b's reader in a loop over 03a's collector; completes the 03 row | done        |
| 04a | shell-lifecycle ([phase-04a-shell-lifecycle.md](phase-04a-shell-lifecycle.md)) — `open_shell {}` + `close_shell { shell }`, both `Category::Run`; share the in-process-host test fixture | done        |
| 04b | watch-shell ([phase-04b-watch-shell.md](phase-04b-watch-shell.md)) — `watch_shell { shell, pattern?, timeout_secs? }`, the blocking observer; reads the **live cast file**, not the dead `.meta.json` index | in-progress (bug-04b-2) |
| 05 | send-input — `send_input`, approval-gated; every keystroke into the `"i"` log | todo (not drafted) |
| 06 | shell-history — `list_shell_logs` / `read_shell_log` as a deferred `history` group over the `.meta.json` index | todo (not drafted) |
| 07 | run-terminal-command-reshape — `shell?`/`host?`/`wait?`/`timeout_secs?`; retire `target_pane`, `retry_in_pane`, `background` | todo (not drafted) |
| 08 | shell-context — `get_terminal_context` → `get_shell_context { scope }`; `[HOSTS]`/`[SHELLS]`/`[DEFAULT SHELL]` blocks; approval panel names shell + host | todo (not drafted) |
| 09 | pane-tool-removal — delete the six pane tools and their executor arms | todo (not drafted) |
| 10 | prompt-assets — rewrite `sre.toml` and the seeded memories; retire the two tmux knowledge memories; `doc_truth` updated | todo (not drafted) |

## Notes

- **The design's tool arithmetic is stale, and the target is still right.**
  § 2.5 says "36 → 42" and lists twelve additions including `run_log_tail`,
  `run_log_search` and `get_turn` — but M24 phase-02b already shipped those
  three as the deferred `runs` group, and the tree has **39** tools today, not
  36. Verified at scoping. The real delta is **39 − 6 + 9 = 42**: six pane
  tools out, nine genuinely new shell tools in. Phase docs should cite this
  line, not § 2.5's arithmetic.

- **`backend` defaults to `"tmux"`, so M20's and M24's work is opt-in.** Phase
  01 flips it, on the PE's decision at the M24 close: every later milestone's
  live verification should exercise the real path, not the legacy one. Two
  things to fix while there: the `ExecutionConfig` doc comment still reads
  *"Nothing reads the value yet — phase-07 is the first consumer"*, which went
  stale when M20 phase-07c became that consumer, and the routing gate lives at
  `src/daemon/executor/mod.rs:302` (`uses_pty`) with a second at `:426`.

- **Nine tools means nine passes through an eleven-step checklist**, and two of
  those steps are gated by `tests/doc_truth.rs` (the CLAUDE.md table with its
  `N tools: C core + D deferred` line, and the README's Core/Deferred tables
  with their `**⚠**` approval markers). The README prose above them carries
  three numbers that must move together. Expect a phase that adds tools to
  touch all three docs, and expect `doc_truth` to catch it when it doesn't —
  that gate exists because the README went three tools stale before it.

- **`open_shell`'s two design parameters and `close_shell`'s signal escalation
  all need `src/shell/` changes — measured at 04a drafting.** § 2.5 specifies
  `open_shell { host?, label? }`, but `ShellRecord` has neither field, and
  adding one is a **10-site** struct change across `registry.rs`, `session.rs`
  and the knowledge module. `host` arrives with M21/M23, which must touch the
  registry anyway; `label` belongs in that same change. **04a ships
  `open_shell {}` with no parameters.** Separately, § 2.5 describes
  `close_shell` as *"SIGHUP, then kill after grace"*, but the wire carries only
  `Int | Term | Stop | Cont` — no `Hup`, no `Kill`, and `PtyShell::kill()` lives
  in the host process, unreachable from the daemon. **04a sends `Term` and
  reports what it observes**, escalation deferred to a protocol phase.

- **`read_shell` reads the log, not the live screen — the screen is not
  reachable.** § 2.5 calls it a *"Screen or log slice"*, but the entire
  daemon↔host wire (`src/shell/proto.rs`) is `Subscribe | Input | Resize |
  Signal | Status` in and `Ok | Error | Chunk | Status | Exited` out — **no
  frame asks for the screen grid**, and `ShellScreen` lives in the host
  process. The log half needs no protocol change: `cast_path` →
  `meta_path_for` → `MetaIndex::load` → `read_command_output` are all plain
  file reads the daemon already owns. 03b implements the log slice; a screen
  read would need `proto.rs` + `host.rs` + `client.rs` + `backend.rs` and is
  its own phase if the design still wants it after 03c.

- **Three phases in, every M22 defect has had an architect-side root cause.**
  01: a bug-doc criterion pinned a count without the filter that produces it.
  02: the Spec omitted the end-to-end capture task that `WORKFLOW.md` already
  requires, so the obligation was never seeded. 03a: the Spec dictated a tool
  description saying the shell id "is what `run_terminal_command` … take",
  written as if phase 07's reshape had landed — it has no `shell` parameter, and
  the executor faithfully wrote the false sentence into a prompt the model reads
  on every request. The executor's *code* has been correct in all three. Revisit
  at close: the lesson is about writing specs against the tree as it is, not as
  the design describes it.

- **`list_shells`'s three-way `scope` collapses to two — found by measuring at
  03a drafting.** § 2.5 specifies *"Own run's shells, or session's, or all"*,
  but `Owner` (`src/shell/registry.rs:50`) is `Chat(String) | Ghost(String) |
  Adopted` and that `String` is a **session** id — `executor/mod.rs:330` builds
  `Owner::Chat(session_id)`. A run is one turn; a shell outlives many turns of
  one session, and `ShellRecord` carries no run id. So "own run" and "own
  session" name the same set, and shipping both would be two spellings of one
  behaviour. **03a ships `"mine" | "all"`**, rejecting `"run"` and `"session"`
  rather than aliasing them. A third value becomes meaningful only when runs
  own shells. Phase 08's `get_shell_context { scope }` keeps § 2.5's own
  three-way vocabulary — it scopes *context*, not shell ownership, so it is a
  different question and is not affected.

- **`send_input` is the one genuinely dangerous addition.** It drives an
  interactive program keystroke by keystroke, so it belongs in
  `APPROVAL_GATED_TOOLS` — which is not only a UI decision: that list is also
  what exempts a tool from the per-turn, per-session and per-tool budget caps
  in `daemon::stream`. A tool added there without a real prompt gets an
  uncapped call budget.

- **Two defects in the shell reading path, both measured at 04b drafting, both
  affecting `read_shell` and `find_in_shells` — neither fixed there.**

  1. **Nothing in production writes a `.meta.json` command index.**
     `MetaIndex::save` (`src/shell/log.rs:159`) has no production caller: the
     host writes the `.cast` (`PtyBackend::spawn` → `CastWriter::create`) and
     nothing ever writes the sidecar. The real home shows one real shell's
     `s0-<ts>.cast` with no sidecar beside it. So every `read_shell` /
     `find_in_shells` call against a live shell lands on the *"has a cast log
     but its command index is missing or unreadable"* branch. **This is why
     04b reads the cast file directly instead of "reusing 03b's reader", as
     this table's row for it originally proposed.** The index's producer is a
     host-side change and belongs with **phase 06**, which is the phase that
     needs it to exist.
  2. **The record's `started_unix` and the cast file's are stamped in
     different processes.** `spawn_for` (`src/shell/registry.rs:195`) stamps
     the record in the daemon; `run_host` (`src/shell/launch.rs:63-70`)
     computes its own `now` in the child and names the cast with *that*. A
     spawn crossing a second boundary leaves `cast_path(id,
     record.started_unix)` pointing at no file, and nothing reconciles them for
     a live shell (`recover_started_unix` runs only in `adopt_at_startup`).
     04b works around it by resolving the cast with `parse_cast_name` over
     `shell_logs_dir()`; `read_shell` and `find_in_shells` still trust the
     record.

  Repairing (2) for the 03-row tools is a phase of its own — 04b's
  Authorizations forbid touching them, and a criterion pins their message text.

- **Carried from the M24 close, unresolved:** *state the property to break, not
  the line to edit* for mutation instructions, held at two occurrences (M24
  09c, 10). M22 has nine near-identical tool phases; if a third instance
  arrives here it should be folded.

- **Also carried:** a phase doc is append-only and nothing enforces it — M24
  phase-10's run overwrote its own spec and no gate caught it. Until there is a
  guard, the reviewer checks the doc's headings survived.
