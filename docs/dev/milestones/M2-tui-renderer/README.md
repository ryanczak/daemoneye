# M2 — TUI Renderer Overhaul

**Goal:** Replace the CLI chat's DECSTBM scroll-region + absolutely-positioned-chrome
rendering with a **committed-scrollback + fixed inline-viewport** model built on
`ratatui`'s inline viewport, eliminating the chat-history corruption that occurs on
tmux window switches and making the transcript real terminal scrollback. Along the
way, split the three oversized `cli/` files (`render.rs`, `input.rs`,
`commands/mod.rs`) into focused modules (closes the bulk of code-issue C5).

**Status:** in-progress

**Depends on:** M1 (complete)

**Exit criteria:**

- Switching tmux windows away from and back to the chat pane **mid-conversation**
  leaves the transcript above and the input box + status bar below intact — no
  scrolled-away chrome, no interleaved output. Verified end-to-end via tmux
  `capture-pane`, not only unit tests.
- Chat transcript lines are committed to **native terminal scrollback** (via
  `Terminal::insert_before`) — they are visible and clean when scrolling up in tmux
  copy-mode. The fixed inline viewport holds **only** the input box + status bar.
- **No DECSTBM scroll region remains in the live chat path.** `setup_scroll_region*`
  / `teardown_scroll_region` and the manual `\x1b[…r` / absolute-CUP chrome drawing
  are deleted; resize and focus are handled by the new model.
- `src/cli/render.rs`, `src/cli/input.rs`, and `src/cli/commands/mod.rs` are each
  reduced to a focused size (target < ~800 lines each) by extraction into submodules;
  no behavior change in the extracted code.
- `ratatui` and `crossterm` are the **only** new dependencies.
- `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features
  -- -D warnings`, and `cargo test` all pass.

## Architecture references

- `docs/architecture.md#1-system-layers` — the CLI client layer this milestone
  rewrites. **A phase may add a "client rendering model" subsection here** (see
  Authorizations in the relevant phase doc; architecture edits are gated).
- `docs/ROADMAP.md` §2.2 **C5** (oversized files) — the split half of this milestone.

## Phases

Drafted on demand (per WORKFLOW.md). The rewrite (01–03) lands the corruption fix
behind a transitional switch that phase 03 removes; the mechanical splits (04–06)
close C5 inside `cli/`. Phases 07–13 extend C5 to the rest of the tree: every
remaining source file over 1000 lines, biggest first, split toward a ~600-line
target (`defs.rs`-style flat-data files documented as exceptions). Statuses mirror
the phase-doc frontmatter.

| #  | Phase                              | Status |
|----|------------------------------------|--------|
| 01 | render-core — add deps; ratatui inline `Terminal` lifecycle + live-region widgets (input box, status bar) + `insert_before` commit API, selected behind a transitional `DAEMONEYE_RENDERER` switch (default = legacy); reuses existing input editing; hermetic `TestBackend` tests | done |
| 02a | streaming-markdown — stream the AI response: pre-first-token spinner in the live region + styled, wrapped markdown committed line-by-line to scrollback + resize redraw; tools stay auto-denied; default stays legacy ([phase-02a-streaming.md](phase-02a-streaming.md)) | done |
| 02b | tools-and-default — interactive tool-call approval (raw/cooked-mode coexistence) + tool panels through the ratatui path + code-block-state fix, then flip the `DAEMONEYE_RENDERER` default to ratatui ([phase-02b-tools-and-default.md](phase-02b-tools-and-default.md)) | done |
| 03 | retire-legacy-and-verify — delete the DECSTBM scroll-region path, absolute-CUP chrome, manual SIGWINCH repair, and the transitional switch; tmux `capture-pane` E2E proving window-switch no longer corrupts (corruption fix is fully landed here) | done |
| 04 | split-render — extract markdown/syntax-highlight (`render_inline`, `highlight_code`, `MarkdownRenderer`, `lang_*`) into a `cli/markdown` submodule | done |
| 05 | split-input — termios/`AsyncStdin` → `cli/input/tty`; `InputLine`/`InputState` editing → `cli/input/editor` ([phase-05-split-input.md](phase-05-split-input.md)) | done |
| 06 | split-commands — extract `run_chat_inner` + the ratatui chat loop + ctx structs + slash handling from `cli/commands/mod.rs` into a `chat` submodule ([phase-06-split-commands.md](phase-06-split-commands.md)) | done |
| 07 | split-tools — split `ai/tools.rs` (2232) into a `tools/` submodule: `schema` (types + renderers), `defs` (the `TOOLS` table, documented size exception), `args` (typed arg structs), `dispatch` (+ tests) ([phase-07-split-tools.md](phase-07-split-tools.md)) | in-progress |
| 08 | split-server — split `daemon/server.rs` (1976) into `server/` : `catchup` (`build_catchup_brief`, `is_valid_pane_id`), `handlers` (stateless IPC handlers), `ask` (`handle_ask` orchestrator) | todo |
| 09 | split-config — split `config.rs` (1631) into `config/` : `types` (config structs), `load` (load/resolve/validate/path helpers), `seeds` (seeding fns + embedded asset constants) | todo |
| 10 | split-file-ops — split `daemon/executor/file_ops.rs` (1475) into `file_ops/` : `read` (`run_read_file` + helpers), `write` (`EditArgs`, edit-command builder, response wait), `ops` (`run_edit`/`create`/`delete`/`copy`) | todo |
| 11 | split-types — split `ai/types.rs` (1413) into `types/` : `wire` (`ToolCall`/`ToolResult`/`Message`/`TokenBreakdown`), `pending` (`PendingCall`), `events` (`AiEvent`) | todo |
| 12 | split-background — split `daemon/background.rs` (1369) into `background/` : `helpers`, `run` (`run_background_in_window`), `respawn`, `gc` (completion notify + GC) | todo |
| 13 | split-knowledge — split `daemon/executor/knowledge.rs` (1341) into `knowledge/` : `artifacts` (scripts/runbooks CRUD), `memory`, `pane` (`list_panes`/`watch_pane`/bg-window), `ghost`, `agents` | todo |

## Notes

### Pre-02b follow-up: code-block state on the ratatui streaming path (from 02a review)

Phase 02a's streaming renderer (`MarkdownRenderer::render_line_to_spans`, `&self`) does
**not** track fenced-code-block state — `feed_to_lines` never toggles
`in_code_block`/`code_lang` (only the legacy stdout `process_line` does). On the ratatui
path this mis-renders fenced code blocks: bodies lose syntax highlighting, markdown-like
lines *inside* a code block render as headings/bullets, and the closing fence renders as a
second opening border. **Accepted for 02a** because the ratatui path is not the default and
no acceptance criterion covers code-block fidelity — but **02b must fix this before it
flips `DAEMONEYE_RENDERER` to ratatui** (give the streaming path a stateful line renderer
so code-block state carries across lines). See phase-02a Review verdict for detail.

### Phase 02 split into 02a + 02b (2026-06-24)

Original phase 02 ("streaming-and-default") bundled the streaming migration, interactive
tool-call approval, and the default-flip into one phase. Split on principal-engineer
direction because of a load-bearing coupling and a risk concern:

- **Coupling:** flipping the `DAEMONEYE_RENDERER` default to `ratatui` is only safe once
  the ratatui path supports **interactive** tool approval — today it auto-denies every
  tool call, so shipping it as the default would silently break all tool use. So the
  default-flip and tool approval must move together, separate from streaming.
- **Risk:** interactive approval requires raw/cooked-mode coexistence (the renderer owns
  raw mode; the y/N prompt needs cooked mode) — the hardest integration in M2. Phase 01
  needed three live-E2E bounces, every one tracing to the executor reusing legacy
  integration seams instead of routing through the new renderer. Isolating approval into
  its own phase keeps each executor session tractable and yields cleaner per-piece
  calibration signal.

Result: **02a** = streamed markdown + spinner + resize (behind the flag, default stays
legacy, tools stay auto-denied); **02b** = tool panels + interactive approval, then flip
the default. Phases 03–06 unchanged. Both 02a/02b remain LEAN rewrite-phase specs per the
calibration protocol.

### Locked decisions (2026-06-23, milestone kickoff)

The principal engineer answered three scoping questions at kickoff:

- **Render engine: `ratatui` inline viewport (+ `crossterm`).** Not hand-rolled.
  `Viewport::Inline` is purpose-built for the committed-scrollback + fixed-bottom-
  region model; the library owns resize, cursor, clear, and frame-diffing. Adding
  the two deps is authorized at the milestone level (the specific phase that edits
  `Cargo.toml` still declares it in its Authorizations).
- **Visual fidelity: correctness-first ("allowed to refine").** The rewrite need not
  be pixel-identical to today's maroon/gold bordered boxes; minor border/spacing
  drift that falls out of the new model is acceptable. Review judges the look, not a
  pixel diff.
- **Scope: broad.** Beyond the renderer rewrite and the `render.rs` split, M2 also
  splits `input.rs` and `cli/commands/mod.rs` (the other two C5 files in `cli/`).

### Root cause being fixed (pre-injection)

The chat TUI pins its input box + status bar to fixed rows using a **DECSTBM scroll
region** (`\x1b[1;{N}r`, `render.rs:setup_scroll_region_n`) and draws the chrome at
**absolute rows** via DEC save/restore cursor (`render.rs:draw_status_bar:486-489`,
`draw_input_frame_n`). Streamed chat output is emitted with plain `println!` so it
scrolls *inside* the region. Any event that resets DECSTBM **without** a `SIGWINCH`
— a tmux window switch is the canonical case (full pane repaint, no size change) —
unpins the chrome: the next `println!` scrolls the whole screen and drags the status
bar / input frame up into the history. The existing `SIGWINCH` handler
(`input.rs:534-562`) already repairs this *on resize*, and its own comment
(`input.rs:544`) names the cause: "on resize, the terminal emulator (or tmux) may
reset DECSTBM." A no-resize window switch never triggers that handler.

Claude Code does not suffer this because it never uses a scroll region: finalized
lines become real scrollback; a small transient bottom region is redrawn each frame.
M2 adopts that model via ratatui inline.

### ratatui inline-viewport facts (verified against live docs 2026-06-23)

Pinned here so phase drafts inherit them; **phase specs touching ratatui MUST carry a
Pre-flight "verify against live docs, trust docs over this sketch" step** (WORKFLOW
§ "Verify external APIs against live docs") — these were read from docs.rs/the
official inline example, version ~0.29, and the API may have moved.

- Construct: `ratatui::init_with_options(TerminalOptions { viewport: Viewport::Inline(rows) })`
  (handles crossterm raw-mode enter/restore internally; `ratatui::restore()` on exit).
- Commit to scrollback: `terminal.insert_before(height, |buf: &mut Buffer| { widget.render(buf.area, buf) })`
  — inserts `height` lines **above** the viewport, pushing them into scrollback.
- Live region: `terminal.draw(|frame| { … })` — diffed redraw of the inline region.
- **Constraint (load-bearing):** the inline viewport height is **fixed at creation**.
  There is no clean runtime height-change API (`Terminal::resize(Rect)` exists but
  interprets the rect as the backend size, with the viewport origin moving to
  preserve the cursor's relative row). **Resolution for our growing multi-line
  input:** keep **only** the input box + status bar in the fixed viewport, sized to a
  modest cap (status row + frame + up to K input rows); commit *everything else*
  (transcript, streamed AI text, tool panels) to scrollback via `insert_before`. If
  the input exceeds K rows, scroll the input internally rather than growing the
  viewport. Confirm the exact resize behavior in-phase against live docs.

### Streaming impedance (pre-injection for phase 02)

`insert_before` commits **whole lines** to scrollback, but the AI streams **tokens**.
The existing `MarkdownRenderer` / `WrapWriter` (`render.rs:500+`) already buffers
output and emits it line-by-line via `println!`; the migration replaces that line
emission with `insert_before(1, …)`. The pre-first-token spinner is transient and
lives in the `draw` loop (replaced each frame), not in scrollback. `stream.rs`
(`src/cli/commands/stream.rs`) is the AI streaming loop that drives this and is in
scope for phase 02 — its many `print!("\r\x1b[K")` spinner/erase calls and the
`MarkdownRenderer` emission move to the new model.

### Verification strategy (the M1 "green-but-inert" trap)

A ratatui renderer can compile and pass `TestBackend`/`Buffer` assertions while still
corrupting on a real tmux window switch — exactly the gap the M1 retrospective
flagged (compiles + unit tests pass ≠ feature runs). Therefore:

- **Unit tier (hermetic, every phase):** render widgets into a fixed-size
  `ratatui::backend::TestBackend` buffer and assert cell contents — deterministic, no
  real TTY, satisfies STANDARDS §3.
- **E2E tier (phase 03, the acceptance gate):** a tmux-driven test — `new-session
  -d`, run the chat, `send-keys` a turn, `new-window` + `select-window` to switch away
  and back, `capture-pane -p`, assert the input frame border and status bar are on
  the expected bottom rows and the transcript above is unbroken. Gate as `#[ignore]`
  if it needs tmux + an API key (mirror `daemon_ping_status_loop`), but make the
  capture-pane assertion the literal exit-criterion check.

### Executor: all phases, deliberately (2026-06-23)

Principal-engineer directive at kickoff: **the local-LLM executor (Qwen3.6-27B-FP8)
runs every phase**, including the design-heavy rewrite (01–03), and **specs are
authored lean — deliberately NOT front-loaded.** The explicit intent is to probe how
much complexity this large, capable-but-not-fully-characterized executor can absorb
from a spec that pins *what* (behavior, acceptance criteria, boundaries) and leaves
*how* (the ratatui API, the implementation shape) to the executor to discover. M2 is a
calibration milestone as much as a feature one: a hard_fail or escalation is a
**successful probe** (it locates the ceiling), not a process failure.

This **intentionally deviates from WORKFLOW.md's "front-load everything" fold**, which
was calibrated on smaller models (gemma-4-12b; Qwen3.6-27B on multi-site mutations).
The hypothesis under test: at 27B the front-loading default may be unnecessary for
single-subsystem rewrites. If the executor clears lean specs here, the WORKFLOW
guidance should become **model-size-conditional** — that fold decision is deferred to
the M2 retrospective, which has the bounce/escalation data to judge it.

Spec leanness applies to the **rewrite phases (01–03)**, where complexity is the test
surface. The mechanical splits (04–06) are normally specced — they are low-complexity
and would yield little calibration signal either way. The "verify against live ratatui
docs" Pre-flight is **kept** in API-touching phases: it is discovery-directing, not
pre-injection. Bounces/takeover are recorded in review verdicts and rolled up in the
retrospective. This overrides the earlier "architect takeover for the rewrite"
candidate.

### Calibration protocol (the experiment design)

- **Variable under test:** spec density (lean → heavy). **Held fixed:** phase size
  (natural per-subsystem complexity, not shrunk) and build-green (the transitional
  switch below). This isolation means a hard_fail points at *spec density*, not at an
  artificially-hard scope or a broken-build path.
- **Graded re-dispatch ladder.** When a rewrite phase hard_fails or escalates, the
  architect re-dispatches the **same** phase with the next rung of pre-injection added,
  and records the rung that unblocked it:
  1. **Lean** (default for 01–03): what + acceptance + boundaries; executor discovers the API.
  2. **+ API sketch:** add the exact ratatui call signatures the phase needs.
  3. **+ worked example:** add a minimal compiling ratatui inline snippet to adapt.
  4. **+ test skeleton:** add the `TestBackend` test bodies to make pass.
  5. **Architect takeover:** Claude Code finishes; log the failure mode.
- **Recording:** each phase's Review verdict `Calibration:` field names the rung that
  landed it (e.g. "lean cleared first try" / "unblocked at rung 2 (API sketch)" /
  "takeover at rung 5 — failure mode: …"). The retrospective rolls these into the
  fold decision on whether WORKFLOW.md's front-loading default should become
  model-size-conditional.
- **Review depth (deep, by directive 2026-06-23).** Reviews of M2 phases are **not**
  pass/fail against the acceptance checklist alone. Each Review verdict additionally
  carries a qualitative assessment across three axes, so we accumulate high-quality
  ceiling data:
  1. **Spec conformance** — met acceptance criteria + stayed inside the boundaries
     (no scope creep, no banned deps, legacy path untouched where required).
  2. **Reasoning quality** — did it *discover and trust the live ratatui API* vs.
     hard-coding against the README sketch? Where did it reason well, guess, or
     flounder? Did it correctly handle the load-bearing constraints (fixed viewport
     height; raw-mode coexistence; commit-vs-live-region split)? Did it surface the
     "green-but-inert" risk on its own? Note misunderstandings even when the build is
     green — an inert pass is the failure mode this milestone exists to catch.
  3. **Code & test quality** — idiomatic ratatui/Rust; no error-suppressing idioms or
     dead code (STANDARDS); tests that actually *assert rendered cells* vs. tests that
     trivially pass; whether the E2E was genuinely run and its `capture-pane` output
     quoted, not asserted-by-narration.
  The verdict records concrete evidence (file:line, quoted output) for each axis, not
  adjectives. This depth is intentional and costs review effort; it is the data
  collection, not overhead.

### Build-green slicing: transitional renderer switch (resolved)

The inline-viewport + `insert_before` model and the DECSTBM + absolute-CUP model
cannot co-own the screen, so a naive build-alongside leaves the new path as dead code
until cutover (`clippy -D warnings` forbids it). **Resolution:** an
architect-authorized transitional switch — `DAEMONEYE_RENDERER` env (`legacy` default
in 01; `ratatui` default in 02) — selects the path at runtime. Both paths are real
call sites, so neither is dead; the build stays green at every phase. The switch and
the entire legacy path are **deleted in phase 03**. This is the one sanctioned feature
flag for the milestone (STANDARDS §2.2 allows it when the phase authorizes it); each
phase doc that touches it restates the authorization and phase 03 owns its removal.

## Interim calibration findings (2026-06-26, phases 01–06 done; milestone still in-progress)

> **Not a final retrospective.** Phases 01–06 are `done` but more phases remain before
> M2 closes (scope under discussion with the principal engineer). The calibration data
> below covers 01–06; the fold decision and milestone retrospective are deferred until
> M2 actually closes. The fold is **not** applied to WORKFLOW.md.

Phases 01–06 `done`. The corruption fix landed (DECSTBM scroll-region path gone;
ratatui inline viewport + `insert_before` scrollback is the only path), and the three
oversized `cli/` C5 files are split (`render.rs` → `cli/markdown`; `input.rs` →
`cli/input/{tty,editor}`; `commands/mod.rs` → `commands/chat`). Only `ratatui` +
`crossterm` were added.

### Outcome ledger (executor: Qwen/Qwen3.6-27B-FP8, every phase)

| Phase | Kind | Verdict | Bounces/Escalation |
|---|---|---|---|
| 01 render-core | rewrite | approved_after_2 | 2 review bounces |
| 02a streaming | rewrite | approved_after_1 | 1 bounce (green-but-inert: tokens → stdout, not scrollback) |
| 02b tools-and-default | rewrite | approved_after_2 | 2 bounces (raw/cooked approval; credential-masking regression in the bounce-fix) |
| 03 retire-legacy-and-verify | rewrite + E2E | escalated | architect takeover after 2 hard_fails |
| 04 split-render | mechanical | approved_first_try | none |
| 05 split-input | mechanical | approved_first_try | none |
| 06 split-commands | mechanical | approved_first_try | none |

### The calibration result (the experiment's payload)

The milestone was run as a spec-density probe (lean specs on the rewrite phases,
normal specs on the mechanical splits) to test the hypothesis that *at 27B,
front-loading may be unnecessary for single-subsystem rewrites*. The data **refutes
that hypothesis for design-discovery work and confirms it for mechanical work** —
and the discriminator that fell out is **task shape, not model size**:

- **Rewrite phases (01–03): lean specs failed, every time.** 5 review bounces + 1
  escalation across 3 phases. The recurring ceiling was identical at every bounce: the
  executor reached for the **wrong renderer seam** — plain-text `commit` / one
  `insert_before` per byte / the legacy integration path — instead of the live-region
  editor + commit-vs-live-region split the spec *named but did not pin*. A lean
  "what + acceptance + boundaries" spec did not let this executor *discover* the
  load-bearing architectural constraint (fixed viewport height; raw/cooked coexistence;
  commit-to-scrollback vs. draw-in-viewport). Phase 03 required full architect takeover.
  The "green-but-inert" trap fired literally (02a: streamed to stdout and passed its own
  self-check; 02b: E2E self-declared, never run).
- **Mechanical phases (04–06): normal specs cleared first try, every time.** Three
  verbatim move-and-re-path splits, zero bounces, each verified by a sorted-multiset
  line diff proving byte-for-byte fidelity. Front-loading here would have been wasted
  effort — the spec already fully determined the shape.

Scorecard corroboration (Qwen3.6-27B-FP8, cross-project): `kind=refactor`
approved_first_try 0.83 but escalation 0.45 (the rewrite-shaped refactors drag it);
`size=s` escalation 0.08 vs `size=l` 0.59. Small, shape-determined work is reliable;
large design-discovery work escalates.

### Fold candidate (proposed — deferred until M2 closes; on hold per PE 2026-06-26)

Three design-heavy bounces/escalation (01/02a/02b/03) and three clean mechanical passes
(04/05/06) is a **trend → fix** under the WORKFLOW calibration rule. The proposed fold:
**make WORKFLOW.md's front-loading default _task-shape-conditional_, not model-size-
conditional.** The discriminator is whether the phase is *design-discovery* (executor
must find a load-bearing API/architecture constraint) vs. *mechanical* (move/rename/
extract whose shape the spec fully determines):

- **Design-discovery phase →** front-load the load-bearing constraint. Do not make the
  executor discover the renderer/architecture seam; pin it (rung 2 "API sketch" or rung
  3 "worked example" from the calibration ladder) in the first dispatch. At this model
  tier, lean discovery specs on design-heavy work cost ~2 bounces or a takeover.
- **Mechanical phase →** lean/normal specs suffice; front-loading is wasted. Keep the
  byte-for-byte move + multiset-diff verification idiom (it caught fidelity cleanly in
  04–06).

This fold is **not yet applied** to WORKFLOW.md — per STANDARDS §5 and the skill's §9,
it needs principal-engineer sign-off before editing WORKFLOW.md. **On hold** (PE
2026-06-26): defer the fold until M2 actually closes; more phases remain.
