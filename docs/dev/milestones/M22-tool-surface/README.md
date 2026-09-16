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
| 04b | watch-shell ([phase-04b-watch-shell.md](phase-04b-watch-shell.md)) — `watch_shell { shell, pattern?, timeout_secs? }`, the blocking observer; reads the **live cast file**, not the dead `.meta.json` index | done                    |
| 05 | send-input ([phase-05-send-input.md](phase-05-send-input.md)) — `send_input { shell, text, enter? }`, **approval-gated**; the `"i"` log is real and no read tool can surface it | done                    |
| 06a | meta-index-producer ([phase-06a-meta-index-producer.md](phase-06a-meta-index-producer.md)) — give the `.meta.json` index a **producer**; makes `read_shell`/`find_in_shells` work in production at last. No new tools | done                    |
| 06b | shell-history ([phase-06b-shell-history.md](phase-06b-shell-history.md)) — `list_shell_logs` / `read_shell_log` as the deferred `history` group over the index 06a made real; also fixes the sidecar-sweep defect 06a made live | done                    |
| 07a | run-command-shell-target ([phase-07a-run-command-shell-target.md](phase-07a-run-command-shell-target.md)) — `shell?` + `timeout_secs?` on `run_terminal_command`; additive, no count change | done                    |
| 07b | run-command-wait ([phase-07b-run-command-wait.md](phase-07b-run-command-wait.md)) — `wait?` replaces `background` on the tool; the tuple field swaps in place, no count change | todo                    |
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

- **A pre-existing flaky test, measured at the baseline so it is not
  re-litigated.** `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  (`src/shell/pty.rs`) fails intermittently under parallel load — roughly 1 run
  in 4 of the full lib suite on this machine. It surfaced during phase-04b's
  review and **is not phase-04b's**: `git diff --stat fa10352a..HEAD -- src/shell/pty.rs`
  is empty for the whole phase, and the failure **reproduces at the pre-phase
  baseline** — a `git worktree` at `fa10352a` running `cargo test --lib 'shell::pty'`
  five times failed once with none of the phase's code present. It spawns a real
  bash and polls against a 10 s deadline, which is the mechanism. Fixing it is
  its own task; until then, a full-suite red naming *only* this test is a flake,
  not a regression. **Re-measure at the baseline before attributing it to a
  phase** — that is the check that settled it here.

- **06 split at a measured seam: the index it is "backed by" has no
  producer.** § 2.5 specifies `list_shell_logs` / `read_shell_log` as *"Backed
  by the `.meta.json` index"*, and 04b measured that `MetaIndex::save`, `mark`
  and `byte_len` have **zero** production callers — the whole index is
  test-only scaffolding. **06a** gives it a producer, **06b** builds the two
  tools on it. That order is forced: 06b's tests cannot use real data until
  06a exists.

  **One correction to this file's own earlier note.** It said the producer
  *"is a host-side change"*. Measured at 06a drafting: it is **not**. The host
  writes bytes it does not interpret; the daemon is what knows a command's
  text, nonce and exit code, and `run_in_shell_with`
  (`src/daemon/executor/shell_exec.rs:62`) already holds the `ShellId` and
  awaits the outcome. 06a touches no protocol frame and no host file.

  **06a's real deliverable is a bug fix**: `read_shell` and `find_in_shells`
  have been landing on their "command index is missing" branch against every
  real shell since 03b. This is the phase that closes it, which is why it
  ships no tool and moves no count.

- **~~A latent panic in `windowed_payload`~~ — resolved at 06a's takeover: it
  was an off-by-`start` in the END branch (`Some(start + p)` where the caller
  slices `from[..stop]`), fixed with a one-token change. Original note:** `src/shell/log.rs`'s END branch computes `stop` via
  `find_subslice(...).unwrap_or(from.len())` and then slices `from[..stop]`;
  a payload truncated mid-END-marker makes `stop` exceed `from.len()` and the
  slice panics. It surfaced under a mutation (`range end index 427 out of
  range for slice of length 61`) but is **reachable from ordinary cast
  content**, not only from the mutant. 06a's authorizations froze that
  function's behaviour, so it was flagged rather than touched. Fix it in 06b
  or its own phase.

- **`src/shell/log.rs` has a design hazard worth naming: raw cast bytes
  versus decoded payloads.** Three separate defects in 06a came from that one
  seam — a spec that searched raw file bytes for needles containing a raw
  control byte the JSON encoding escapes; a marker search that decodes one
  line at a time and so cannot see a marker split across two events; and a
  test assertion that checked raw bytes for patterns that exist only after
  decoding. Anything touching this file should state, per function, which
  side of that line it is on.

  A fourth, found at 06a's takeover: **`extend_output_line` re-windows the
  whole accumulated buffer on every line and keeps only the last
  BEGIN..END.** Correct for one command's record; a hazard for any slice that
  spans two commands' markers — `read_output_since` (the `watch_shell` tail
  reader) shares it, so a poll slice containing `…END0 … BEGIN1 …` drops the
  text before `BEGIN1`. Not fixed; 06b or 07 should decide whether the reader
  windows per command record or per stream.

- **A retention defect 06a made live, fixed in 06b.** `meta_path_for` writes
  `<stem>.meta.json`, but `sweep_*` (`src/daemon/utils/mod.rs:148-152`) only
  recognises `*.cast.meta.json` — so **no `.meta.json` sidecar is ever swept**.
  Its test passes because the fixture hand-writes the sweep's spelling rather
  than calling the producer. Before 06a nothing wrote sidecars, so nothing
  leaked; now every indexed shell leaves a file retention cannot reach. This
  is the **fourth** instance in this file family of writer, finder and test
  agreeing on a shape production does not produce — the same class as the
  escape-mismatch and the non-firing boundary assertion. 06b rebuilds the
  fixture through `meta_path_for` so they cannot drift apart again.

- **Phase 07 re-scoped at drafting, measured.** The charter row bundles two
  additions, one rename and two retirements. Measured: `target_pane` has
  **170** references including the **IPC wire** (`src/ipc.rs:197`, `:447` —
  the approval panel's pane hint) plus 23 in `src/cli/`, 55 in the executor,
  33 in `src/ai/` and 9 in the prompt; `retry_in_pane` has 13. Those are the
  pane *substrate*, not tool parameters, and they die with the pane tools in
  **phase 09**. `host?` has nowhere to land for the third time in this
  milestone (after `open_shell` in 04a and `list_shell_logs` in 06b) and
  waits for M21/M23. So: **07a** adds `shell?` + `timeout_secs?` (additive,
  no count change), **07b** does `wait?` for `background`, and the two
  retirements fold into 09's row.

- **Carried from the M24 close, unresolved:** *state the property to break, not
  the line to edit* for mutation instructions, held at two occurrences (M24
  09c, 10). M22 has nine near-identical tool phases; if a third instance
  arrives here it should be folded.

- **Also carried:** a phase doc is append-only and nothing enforces it — M24
  phase-10's run overwrote its own spec and no gate caught it. Until there is a
  guard, the reviewer checks the doc's headings survived.
