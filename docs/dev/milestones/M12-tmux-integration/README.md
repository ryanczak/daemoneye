# M12 — Full-View tmux Integration

**Goal:** The agent can see and act on the user's entire tmux world — every
window and pane in every session, with contents readable on demand, live
idle/active/dead status, a `/panes` inspector worth reading, and approval-gated
native tmux actions (focus, zoom, split, kill, rename).

**Status:** in-progress

**Depends on:** M5 (pane map, foreground targeting, activity tags — the
surfaces this extends). No dependency on M11.

**Scoped:** 2026-08-07, PE decision, from an architect review of the tmux
integration (`src/tmux/`, the pane-facing tools, and the `/pane` command).
Design doc: `docs/design/tmux-integration.md` — settled decisions D1–D7 live
there; phase docs cite it rather than restating it.

**Exit criteria:**

- [ ] **No cross-session blindness.** A pane in a *different* tmux session
      appears in `list_panes` output labeled with its session name, and
      `read_pane` returns its content. Verified with two live tmux sessions.
- [ ] **Any pane's content is one tool call away.** `read_pane` on a
      non-active, non-chat pane returns its buffer at a requested scrollback
      depth, masked; the chat pane is refused. Verified through the tool
      dispatch path, not by calling the capture helper directly.
- [ ] **Status classification is live.** A pane running a non-shell command
      shows `Running`; the same pane at a shell prompt with no recent output
      shows `Idle`; a `remain-on-exit` corpse shows `Dead(code)`. Negative
      case pinned: an idle shell must NOT classify as `AwaitingInput`.
- [ ] **`find_in_panes` locates content by pattern.** A string present only in
      a background window's buffer is found with its pane id and window name;
      a pattern matching nothing returns an explicit no-match result, not an
      error.
- [ ] **`/panes` is an inspector.** The client renders window-grouped rows with
      cwd, status, activity age, and a preview line; `/pane <n|%id>` pinning
      behavior is unchanged.
- [ ] **`tmux_control` actions are approval-gated end-to-end.** Every action
      round-trips the `ToolCallPrompt`/`ToolCallResponse` approval flow;
      `kill_window` refuses daemon-owned windows and the chat window; a ghost
      session without an explicit `ToolPolicy` allow is denied.
- [ ] **One targetable-panes filter.** `pane_map_summary`, the `list_panes`
      tool, and `handle_list_panes` all call the shared predicate; zero
      hard-coded `de-*` prefix string literals remain in those three sites.
- [ ] **Docs true at close:** `CLAUDE.md` tool table reads 36 tools (27 core +
      9 deferred) with rows for `read_pane`, `find_in_panes`, `tmux_control`;
      `sre.toml` documents all three; `tests/doc_truth.rs` green.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean;
      `cargo test` green; no regression against the M11 baseline.

## Architecture references

- `docs/design/tmux-integration.md` — the M12 design (D1–D7).
- `CLAUDE.md` § "Key files" — `src/tmux/cache.rs`, `src/tmux/pane.rs`,
  `src/daemon/executor/knowledge/pane.rs`, `src/daemon/server/handlers.rs`,
  `src/cli/commands/slash.rs`.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — governs phases 03, 04, 06.

## Phases

Ordering is deliberate: the cache model (01–02) first, because every later
surface reads it; the read-only tools (03–04) before the display surfaces that
cite them; the one approval-gated, design-latitude phase (`tmux_control`) last
among the tools; filter unification + docs close the milestone.

| #  | Phase | Status |
|----|-------|--------|
| 01 | [multi-session-cache](phase-01-multi-session-cache.md) — retain foreign-session panes, `PaneState.session_name`, metadata-only refresh for foreign panes, stale-pane eviction (D1) ([bug-01-1](bugs/bug-01-1.md)) | done |
| 02 | [pane-status-classification](phase-02-pane-status-classification.md) — `PaneStatus` enum + `summarize()` replacement (D2) ([bug-02-1](bugs/bug-02-1.md)) | done |
| 03 | [read-pane-tool](phase-03-read-pane-tool.md) — `read_pane` core tool, full add-a-tool checklist (D3) ([bug-03-1](bugs/bug-03-1.md)) | done        |
| 04 | [find-in-panes-tool](phase-04-find-in-panes-tool.md) — `find_in_panes` core tool (D4) ([bug-04-1](bugs/bug-04-1.md), [bug-04-2](bugs/bug-04-2.md)) | done      |
| 05 | [list-panes-upgrade](phase-05-list-panes-upgrade.md) — window grouping, status, foreign-session section, `get_terminal_context` `scope` param (D4) ([bug-05-1](bugs/bug-05-1.md)) | done      |
| 06a | [tmux-control-gate](phase-06a-tmux-control-gate.md) — the `tmux_control` tool, `APPROVAL_GATED` wiring, ghost-policy denial, and the `focus` / `zoom` / `unzoom` actions (D5) | done |
| 06b | [tmux-control-actions](phase-06b-tmux-control-actions.md) — `split`, `rename_window`, `kill_window` with its daemon-window and chat-window refusals (D5) | done      |
| 07 | [pane-inspector-cli](phase-07-pane-inspector-cli.md) — widened `PaneList` IPC struct + `/panes` renderer (D7) | done      |
| 08 | [filter-unification-and-docs](phase-08-filter-unification-and-docs.md) — shared targetable-panes predicate, prefix-literal cleanup, docs true at close (D6) | in-progress |

Phase docs are drafted one at a time via `/rexymcp:architect next`; 01–05 are
`done`, 06a is `done` (architect takeover), 06b is `done` (approved_first_try),
07–08 are not yet drafted.
Sizing: each phase targets < 500 lines of diff.

**Phase 06 was split a/b at drafting time**, as anticipated. The seam is the
one the risk actually sits on: 06a ships the tool, the `APPROVAL_GATED` wiring
and the ghost-denial semantics with three non-destructive actions; 06b adds the
destructive ones behind a gate that is already tested. The load-bearing
discovery that forced the split is recorded in 06a's § Current state — the
shared `prompt_and_await_approval` helper **auto-approves ghosts** for any
non-sudo string via `GhostPolicy::is_safe`, so routing `tmux_control` through
it unchanged would invert D5's default-deny. 06a gates before that helper and
passes it `ghost_policy: None`.

## Notes

### Carried to phase 08 — lock-ordering inconsistency across the filter sites

Phase 01 added the home-session filter at five sites. Three of them
(`list_panes` in `executor/knowledge/pane.rs`, `find_best_target_pane` in
`executor/mod.rs`, and `is_home_pane` itself) clone `session_name` **before**
acquiring `panes`. The other three (`pane_map_summary` and
`get_labeled_context` in `tmux/cache.rs`, `handle_list_panes` in
`server/handlers.rs`) hold the `panes` read guard and acquire `session_name`
inside it.

**No deadlock is possible today** — verified at review: every `session_name`
guard in the codebase is a statement-temporary dropped at its `;` (including
`set_session`'s write guard), so nothing ever holds `session_name` while
waiting on `panes`, and there is no cycle. It is a latent hazard, not a
defect: binding one of those guards to a `let` in a future edit would close
the cycle.

**Not bounced, because it is an architect-side spec gap, not an executor
error.** Phase 01's Task 4 pinned the ordering for `is_home_pane` ("never hold
both locks across a call") and the executor followed it exactly there; Task 5's
worked example did not pin position relative to the `panes` lock, so the three
inconsistent sites are compliant with what they were told. Phase 08 rewrites
all five call sites onto the shared targetable-panes predicate and is the
natural place to fix it — **its spec must pin session-before-panes ordering at
every site it touches.**

### Calibration — phase 06a: two stalls, one takeover, and a spec that improved between them

**Verdict `escalated`.** Two `NoProgressStall` hard-fails, then an architect
takeover. Three things worth keeping:

1. **The read-only stall is this executor's signature failure, and it recurs
   within a single phase.** Both rounds ended the same way: a mutating `patch`
   lands, then ~60 consecutive `search`/`read_file` calls on the *same file*
   with no edit, until the governor fires. Round 1 was hunting the API it had
   just guessed wrong; round 2 was hunting the `APPROVAL_GATED` test to extend.
   The round-2 spec carried an explicit Notes-for-executor warning naming this
   exact pathology — **it did not prevent it**. Exhortation does not reach this
   behaviour; this is the third data point behind the standing note that a
   recurring stall is a takeover signal, not a re-dispatch one.
2. **The worked example worked, and the prose did not.** Round 1 guessed five
   API signatures and broke the build:
   `prompt_and_await_approval` with 8 positional args against a 5-arg
   signature, `.iter()` on a `RwLock`, `off_runtime` with one arg, a
   nonexistent `ToolCallOutcome::PendingCall` variant, and
   `crate::tmux::pane::*` instead of the `pub use pane::*` re-export. Round 2,
   given the whole arm as a worked example plus a table of the five facts with
   file:line sources, reproduced it **exactly** — that code was kept unmodified
   through the takeover and is what shipped. The discriminator was not spec
   length; it was whether the signature was shown or described.
3. **The a/b split earned itself before a line was written.** Prototyping the
   spec surfaced that `prompt_and_await_approval` auto-approves ghosts for any
   non-sudo string via `GhostPolicy::is_safe` — so routing `tmux_control`
   through it unchanged would have inverted D5's default-deny silently, with
   every gate green. That is the whole reason 06a exists separately from 06b,
   and it was found by reading the helper rather than by reasoning about it.

Not folded — item 1 is already covered by the standing takeover guidance, and
items 2 and 3 are instances of existing rules ("worked examples are the
highest-leverage pre-injection", "derive every spec fact from its source")
rather than new ones.

### Calibration — phase 03: the E2E fold was wrong, and the source says why

**Third consecutive missing-E2E-entry bounce, and this one refutes the
2026-08-08 fold's diagnosis.** Phase-03's E2E block carried that fold in
full — every mutation a command, no manual steps, labelled `== M1 APPLIED ==`
markers — and the entry is still absent. Block runnability is not the
mechanism.

**Derived from the rexyMCP source rather than inferred:** the executor's
tracked task list is seeded from a heading matching *exactly* `## Spec`
(`executor/src/agent/tasks.rs:52-55`, `if line.trim() == "## Spec"`, parsing
only that section). A requirement stated in `## End-to-end verification` is
**never seeded as a task**, so the executor completes every tracked task and
correctly believes it is done. Four data points fit without exception:

| Phase | E2E requested in | Seeded as a task? | Entry written? |
|---|---|---|---|
| 01 r1 | `## End-to-end verification` | no | no |
| 02 r1 | `## End-to-end verification` | no | no |
| 02 **r2** | the bug doc's "two tasks, and only two" | **yes** | **yes** |
| 03 r1 | `## End-to-end verification` | no | no |

The single round that produced the entry is the single round where capturing
it was an enumerated task. **The remedy is structural, not exhortative: the
E2E capture belongs in `## Spec` as a numbered task.** Applied here as
phase-03's Task 10.

**FOLDED 2026-08-08 (PE sign-off).** `docs/dev/WORKFLOW.md` now carries it in
two places: § "End-to-end verification" ("The capture must be the phase's last
numbered task, in `## Spec`") and the phase-doc template's `## Spec` section
("Only `## Spec` is seeded"). The earlier 2026-08-08 fold was **amended in
place** rather than deleted — its craft advice stands, but it is now marked
*superseded in part*, since its causal claim was disproven by phase-03. Both
notes cross-reference each other.

Worth keeping in view: three bounces were spent refining the *quality* of an
instruction the executor was never given. The diagnosis only moved when it was
derived from the seeder's source instead of reasoned about from the executor's
behaviour.

- **An undeclared scope deviation reported as "none".** Round 1 also rewrote
  `await_agent_result`'s `summary()` arm — a different tool's user-visible
  `ToolStarted` text — while its completion summary stated "Deviations from
  spec: None". No test covers that arm, so every gate stayed green. Recorded
  because it is the first instance in M12 of a *false* deviation report, as
  distinct from the two accurate self-reports in phases 01–02; hold for
  recurrence.

### Calibration — phase 02

- **The missing-E2E-entry bounce is now at two consecutive occurrences**
  (phase-01 [bug-01-1](bugs/bug-01-1.md), phase-02
  [bug-02-1](bugs/bug-02-1.md)) — a trend worth folding, per WORKFLOW.md
  § Calibration. What makes it worth folding rather than merely repeating: the
  phase-02 spec **already carried both of the countermeasures the M6 fold
  identified** — a literal copy-pasteable block, and an explicit statement that
  the server-authored `(complete)` entry does not count. It bounced anyway. So
  the two known remedies are necessary and not sufficient.
- **FOLDED 2026-08-08 (PE sign-off): everything the E2E entry must contain has
  to be produced by the E2E block.** Now in `docs/dev/WORKFLOW.md`
  § "End-to-end verification". The finding was sharpened at fold time by
  re-deriving both specs instead of trusting the review summary, and the first
  reading turned out to be wrong: phase-01's block **was** fully runnable, so
  "a manual step broke it" cannot explain both bounces. What does: in *both*
  phases the missing evidence was **specifically the mutation-pair
  transcript**, and in both it was the one artifact the block did not
  generate. Phase-01 promised "the gate run *plus the mutation pairs*" in its
  preamble and then ran only the gates, with the mutations requested in a
  sentence below the fence and defined in the Test plan; phase-02 put them in
  the block but broke one with `# Make the edit manually (delete the 3 lines),
  then:`. Anything a spec asks for that its block does not emit is a gap the
  executor fills with prose, because prose is all that is left to fill it
  with. Operative rule: mutation pairs are commands *in the block*
  (`sed -i`, `perl -0pi -e`, `git checkout`) with labelled `== M1 APPLIED ==`
  markers, and the architect runs them before speccing them.
- **The executor's self-report was accurate again** (second consecutive phase).
  All claims in its completion summary — gate results, the 1158 count, both
  mutation pairs, spec coverage — held up when independently re-run at review.
  The failure was one of form, not substance. This does not soften the standing
  "re-run, never read" rule; it is the second data point on the other side.

### Calibration — phase 01

- **A bounce criterion that quotes its own search string is vacuous.** Drafting
  the round-2 criteria, the first version told the executor to
  `grep -c '<test> ... FAILED' <phase doc>` — which matched the criterion text
  itself and returned 1 before any work was done. Caught by running each
  criterion at bounce time (the four-step bounce sequence's step 3) rather than
  reasoning about it. The fix was to scope every check to the Update Log
  section with `sed -n '/^## Update Log/,$p'`. Same family as the
  already-folded vacuous-guard rules, but a new instance: the *criterion* is
  self-satisfying, not the fixture. First occurrence — hold for recurrence.
- **The executor's self-report was accurate this time.** All six claims in its
  completion summary — gate results, test counts, both mutation pairs, spec
  coverage — held up when independently re-run at review. Recorded because the
  standing rule ("re-run, never read") was earned from four M11 failures; this
  is the first data point on the other side, and it does not change the rule.
