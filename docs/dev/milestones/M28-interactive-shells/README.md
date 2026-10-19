# M28 — Interactive shells

**Goal:** an interactive TUI — Claude Code is the reference case — runs
inside a DaemonEye PTY shell, and the model can see it, type into it
safely, wait on it, and never poison it with a command meant for a prompt.

**Status:** done (scoped 2026-10-13, closed 2026-10-18; release pending PE sign-off)

**Release:** 2.1.0

**Depends on:** M20 (shell engine), M23 (attached mode), M27 (PTY-only
execution).

**Exit criteria:**

- **See.** `read_shell_screen` returns a shell's rendered screen, masked,
  with inverse-video runs marked `[SEL: …]`, optional scrollback, and a
  header naming the shell's `Foreground`.
- **Never poison.** With Claude Code running in the owner's lowest shell,
  a bare `run_terminal_command` lands in a fresh shell and `shell: "<id>"`
  naming the busy shell is refused before any byte is written. Two
  consecutive commands on a configured `[hosts.*]` host reuse one shell.
- **Type safely.** `send_input` sends `\r` for Enter, takes named `keys`
  (with CSI u for modifier combinations) and bracketed `paste`; its
  approval panel shows the foreground and a masked screen tail; a ghost
  without `allow_tui_input` is refused.
- **Launch and wait.** `run_terminal_command(interactive: true)` starts a
  TUI without waiting for it or signalling it; `read_shell_screen` with
  `wait_for` / `settle_ms` waits on the rendered screen and reports a
  screen that never settles as a result, not an error.
- **Static flag gone.** `ShellRecord.interactive` and `mark_interactive`
  no longer exist; busy-ness is observed (`last_foreground`) or known
  (`in_flight`).
- Live close on pinky per the M14 convention; `tests/doc_truth.rs` passes;
  `2.1.0` released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/design/interactive-shells.md` — the design; D1–D10 and the § 5
  matrix are what the phases implement.
- `docs/design/daemoneye-2.0.md` § 6 — the roadmap row.

## Phases

Drafted one at a time with `/rexymcp:architect next`. The design's three
phases are split further where one is more than a session.

| # | Phase | Status |
|---|---|---|
| 01 | host-truth ([phase-01-host-truth.md](phase-01-host-truth.md)) — the host's `Screen` frame with `[SEL: …]` markers, the three `Status` foreground facts, the pure `Foreground` classifier (local and transport); no daemon behaviour change | done (approved_first_try, 2026-10-13) |
| 01b | record-truth ([phase-01b-screen-tool.md](phase-01b-screen-tool.md)) — `last_foreground` and `in_flight` on `ShellRecord`, `classify_record`; the `unreachable!` carried from 01. *Re-scoped 2026-10-13 after two stalls: the tool moved to 01c* | done (approved_first_try, 2026-10-13) |
| 01c | screen-tool ([phase-01c-screen-tool.md](phase-01c-screen-tool.md)) — `read_shell_screen` (no waits yet), prompt asset, tool tables | done (escalated: architect takeover, 2026-10-14) |
| 02a | busy-shells ([phase-02a-busy-shells.md](phase-02a-busy-shells.md)) — a live foreground probe; reuse skips a busy shell; a named busy shell is refused before any write; the prompt digit rule narrowed to `%` | done (approved_first_try, 2026-10-14) |
| 02b | foreground-listed ([phase-02b-foreground-listed.md](phase-02b-foreground-listed.md)) — `list_shells` and `[SHELLS]` show the observed foreground instead of `[interactive]`; `[DEFAULT SHELL]` skips a shell last seen busy | done (approved_first_try, 2026-10-14) |
| 02c | flag-gone ([phase-02c-flag-gone.md](phase-02c-flag-gone.md)) — remove `ShellRecord.interactive`, `mark_interactive`, the reuse predicates and the IPC / `/shells` uses; the interactive-command path becomes an honest refusal | done (approved_first_try, 2026-10-14) |
| 02d | keys ([phase-02d-keys.md](phase-02d-keys.md)) — the pure key encoder (tmux names, xterm modifiers, CSI u) and paste rule in `src/shell/keys.rs`; `ScreenText` reports bracketed paste and application cursor; `send_input`'s Enter is `\r` | done (approved_first_try, 2026-10-14) |
| 02e | keys-tool ([phase-02e-keys-tool.md](phase-02e-keys-tool.md)) — `send_input` gains `keys` and `paste`, the panel and result name them, prompt and doc tables. *Split from 02d at drafting* | done (approved_first_try, 2026-10-14) |
| 02f | input-guard ([phase-02f-input-guard.md](phase-02f-input-guard.md)) — `send_input` probes the live foreground before the panel; the panel names it and shows a TUI's masked screen tail; a ghost may type only at a prompt | done (approved_first_try, 2026-10-14) |
| 02g | allow-tui-input ([phase-02g-allow-tui-input.md](phase-02g-allow-tui-input.md)) — the runbook `allow_tui_input` field through `GhostConfig` / `GhostPolicy`, lifting 02f's ghost refusal; docs. *Split from 02f at drafting* | done (approved_first_try, 2026-10-14) |
| 03a | screen-wait ([phase-03a-screen-wait.md](phase-03a-screen-wait.md)) — `read_shell_screen` gains `wait_for`, `settle_ms`, `timeout_secs`; quiescence on rendered text; a wait that runs out is a result | done (approved_after_1, 2026-10-16) |
| 03b | geometry ([phase-03b-geometry.md](phase-03b-geometry.md)) — `[shells] rows`/`cols` (40×120) through one `spawn_size()` at all 13 spawn sites | done (approved_first_try, 2026-10-16) |
| 03c | open-shell-size ([phase-03c-open-shell-size.md](phase-03c-open-shell-size.md)) — `open_shell(rows, cols)`; the result names the size. *Split from 03b at drafting* | done (approved_first_try, 2026-10-17) |
| 03d | launch ([phase-03d-launch.md](phase-03d-launch.md)) — an interactive command (ssh, mosh, …) is launched: bare command + `\r`, settle, return the screen; replaces 02c's refusal | done (approved_first_try, 2026-10-17) |
| 03e | interactive-param ([phase-03e-interactive-param.md](phase-03e-interactive-param.md)) — `run_terminal_command(interactive: true)` for any command, its panel line, the `wait: false` refusal. *Split from 03d at drafting: tool plumbing apart from behaviour* | done (approved_after_2, 2026-10-18) |
| 04 | live-close ([phase-04-live-close.md](phase-04-live-close.md)) — architect rehearsals then a PE run of B1–B11 on pinky (Claude Code in a daemon PTY, zoidberg, ghost guard); then the 2.1.0 release | done (architect-run; PE run passed 2026-10-18) |
| 04a | arg-tolerance ([phase-04a-arg-tolerance.md](phase-04a-arg-tolerance.md)) — live F2 + F5: `keys` as an array, a JSON string or one name; string booleans; the invalid-args error names undeclared fields | done (approved_first_try, 2026-10-18) |
| 04b | launch-settle ([phase-04b-launch-settle.md](phase-04b-launch-settle.md)) — live F1 + F3 + F4: a launch does not settle on the pre-launch screen; the busy-refusal wording; `top` is not the never-settles example | done (approved_first_try, 2026-10-18) |
| 04c | live-r2 ([phase-04c-live-r2.md](phase-04c-live-r2.md)) — rehearsal 2's F6 (`keys:"[]"` is no keys), F7 (the cap refusal is pre-write and names no shell), F9 (a ghost refuses `interactive`) | done (approved_first_try, 2026-10-18) |
| 04d | launch-panel ([phase-04d-launch-panel.md](phase-04d-launch-panel.md)) — PE decision: the approval panel says a launch keeps the shell (D6), as its own frame field; F10 `Error: Error:` | done (approved_first_try, 2026-10-18) |

## Retrospective (2026-10-18)

**Outcome.** Every exit criterion is met and checked live on pinky.
- Claude Code runs inside a DaemonEye PTY shell. The model launches it with
  `interactive: true` and sees its rendered screen, including `[SEL: …]`
  inverse runs.
- The model types into it with `send_input` (`text`, named `keys` with CSI
  u, bracketed `paste`) and waits on it with `read_shell_screen`
  (`wait_for`, `settle_ms`).
- A busy shell is never written to.
- The static `interactive` flag is gone.

The PE run passed B1–B9 and B11. B10, the ghost guard, cannot be reached
live (F8 below). The suite went from 2325 to 2410.

**Numbers** (from the telemetry store). 19 executor phases, 32 runs, 6,370
executor turns, about 26 hours of executor wall clock. Plus the
architect-run live close: two rehearsals, the PE run and the 04d check.
- **16 `approved_first_try`:** 01, 01b, 02a–02g, 03b, 03c, 03d, 04a–04d.
  Several needed a resume or a refined re-dispatch after a hard fail, which
  is not counted as a bounce.
- **`approved_after_1`:** 03a. **`approved_after_2`:** 03e.
- **`escalated`:** 01c (architect takeover).
- **9 hard-failed runs:** six stalls in 01–02c, 03d's stall, 03e's
  suite-retry loop, and 01c.
- **Bug docs:** bug-03a-1 and bug-03e-1.

**What the live close found that nothing else did.** Ten findings (F1–F10).
All but F8 are fixed in 04a–04d.
- **F1:** a launch "settled" before the program drew.
- **F2, F5, F6:** argument shapes GLM really sends did not parse: `keys` as
  a string, `background:"false"`, and `keys:"[]"`.
- **F3, F7, F10:** pre-write refusals wore the "may still be running /
  double-apply" text, named a shell they had not used, or printed `Error:`
  twice.
- **F4:** plain `top` was the docs' example of a screen that never
  settles; it redraws every 3 s.
- **F9:** a ghost silently dropped `interactive`.

Every one of these passed its unit tests. The lesson is the same as M26's
and M27's: an entry point is verified only when it is driven live.

**Architect errors, recorded.** The most consequential defects this
milestone were spec or review errors, not executor errors.
- **Specs.**
  - 01's assertion was impossible (an idle interactive bash reads as raw
    mode).
  - The exact count pins trapped executors three times: 02b, 03e (stalled
    re-running a green suite), and 04b (16 suite reruns).
  - Combined behaviour shipped with no named test: 03a's `wait_for` +
    `settle_ms` broke, and 03e's `wait: false` note was dropped silently.
  - 03d named no fixture recipe for a direct call, and stalled.
  - 03e's spec said "two approval sites" where there is one.
  - A stale resume note contradicted a bounce note, so 03e's first
    re-dispatch made no edit.
- **Reviews.**
  - 02g's approval was committed without its verdict block, because a
    `&&` chain stopped before the heredoc.
  - 03e's first approval was committed before the standards grep was read;
    it showed a dead `#[allow]`, and the approval was withdrawn.
  - 04a's "harmless nit" (`keys:"[]"`) was F6.
- From 04a on, every review ran all its greps before writing a verdict.

**Process calibration** (fold proposals below need PE sign-off).
- **Tool-plumbing cost.** A phase that threads a new tool parameter through
  args → event → pending → stream → defs → executor ran 263–587 turns
  (02e, 03a, 03c). Phases that touched no tool surface ran 92–164. Splitting
  plumbing from behaviour (03d/03e, 04a–04d) kept the later phases short.
- **Compile-derived site lists work.** 02g ("exactly 6, from a compile")
  took 92 turns. A grep-derived "complete list" (02b, 02c) missed sites.


**Fold proposals** (PE sign-off needed before any lands in `WORKFLOW.md`).
- **Proposal A: report the full-suite count, don't pin it.** Three
  occurrences (02b, 03e, 04b), so this is a fix. The text: "A full-suite
  pass count is a report, not a gate. Pin the new test names; state the
  expected count, and say an extra test is fine: name it and report; do
  not re-run the suite to chase a number." The existing count folds cover
  `grep -c` pins, not the suite total, which is how this trap survived them.
- **Proposal B: combined behaviour gets its own named test.** Two
  occurrences (03a, 03e), so this is a trend and is held. When a spec says
  "A and B together do C", one named test must pin C, or the combination
  ships unverified.

**rexyMCP issues seen this milestone** (for the rexyMCP backlog, not this
repo):
1. **`continue_phase` and the client timeout.** It blocks past the MCP
   client's 1800 s idle timeout (five times); the run continues server-side.
2. **Governor blind spots.** Detected only by the 120-call read-only
   limit, or not at all:
   - near-identical commands with changing filenames (`/tmp/dbgN.sh`);
   - verification spread across many module test runs;
   - forward single-line reads through a file (03d crawled `shell_exec.rs`
     a line at a time).
3. **Executor claims that were false.**
   - 03d's started entry named the executor "Claude Opus 4.6".
   - 02g claimed `PASTE MATCH` without writing the line.
   - GLM said it had sent a real array for `keys` when `daemon.log` showed
     strings.
4. **`get_run_status` result size.** Its results exceed the context budget.

**Deferred to a later milestone.**
1. **F8, ghosts are background-only.** `ghost.rs` maps every ghost
   `run_terminal_command` to `PendingCall::Background`. A ghost therefore
   never holds a live TUI, and D9's ghost rows are defence in depth with
   unit-test coverage only. Driving a TUI from a ghost needs a ghost
   foreground path.
2. **O3:** the masker turns the word `hostname` into `<SSH_HOST>` (seen
   once).
3. **O4:** a successful `run_terminal_command` result does not name its
   shell id.
4. **The `sbx.rs` flake.** A `HOME`-reading test in `sbx.rs` flaked in 02c
   and 04b.
5. **03e's per-result `wait: false` note.** Left as is by PE decision.
6. **Still queued from M27:** unify session residency, F9, and the
   invalid-args nits.

## Notes

- **The interactive-command path was a stub (found drafting 02c).** For
  `ssh host` and friends, `run_terminal_command` wrote nothing to the
  shell but told the model the session was established; only the flag made
  the next command use a fresh shell. 02c turns it into an honest refusal
  pointing at `open_shell` + `send_input`; phase 03's `interactive: true`
  replaces the refusal.
- **Phase 02 split (2026-10-14).** Five phases (02b split again at drafting: the flag has four deciding files and five rendering ones, so the model-facing surfaces go first and the field removal second) of two or three files each,
  per the stall note below. Drafting 02a found that phase 01's digit rule
  rejects bash's default `bash-5.2$ ` prompt, which minimal hosts and sbx
  sandboxes show; 02a narrows it to `%`.
- **M28 stalls, so far (2026-10-14).** Four `NoProgressStall`s across
  phases 01, 01b and 01c, each traced to the spec: an impossible assertion
  (an idle interactive bash reads raw), an open-ended caller audit, a
  nine-file checklist, and in 01c a wrong fixture pointer plus count-pinned
  tests the spec did not name. Narrowing 01b to two files worked. 01c,
  already one-file-per-task, still stalled reading every file before the
  first write, so it was taken over. For phase 02: name every test that
  pins a count the phase moves, name the real fixture, and keep a phase to
  a handful of files.
- **Phase 01b re-split (2026-10-13).** Two dispatches stalled at 120
  read-only calls, the second with zero edits, after reading the nine files
  the tool checklist names. The record work stays in 01b; the tool is 01c
  with one file per task. Same class as M26 10b: breadth, not difficulty.
- **Phase 01 split (2026-10-13).** The design's phase 01 covers the host,
  the classifier, a new tool and two record fields. That is two sessions:
  01 stops at the shell layer, 01b adds the tool and the record fields.
- **Transport prompt rule refined at drafting.** D3 reads a last row ending
  in `$ # % >` as a prompt. Phase 01 adds one negative: the character must
  not follow a digit, so a progress line such as `Progress 50%` is not a
  prompt. The error direction stays "busy".
- **Name collision to keep straight.** `src/daemon/executor/foreground.rs`
  already exists (foreground command execution). The classifier is
  `src/shell/foreground.rs`, a different module.
