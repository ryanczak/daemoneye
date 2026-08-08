# M12 — Full-View tmux Integration

**Goal:** The agent can see and act on the user's entire tmux world — every
window and pane in every session, with contents readable on demand, live
idle/active/dead status, a `/panes` inspector worth reading, and approval-gated
native tmux actions (focus, zoom, split, kill, rename).

**Status:** planning

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
| 01 | [multi-session-cache](phase-01-multi-session-cache.md) — retain foreign-session panes, `PaneState.session_name`, metadata-only refresh for foreign panes, stale-pane eviction (D1) | in-progress |
| 02 | pane-status-classification — `PaneStatus` enum + `summarize()` replacement (D2) | todo |
| 03 | read-pane-tool — `read_pane` core tool, full add-a-tool checklist (D3) | todo |
| 04 | find-in-panes-tool — `find_in_panes` core tool (D4) | todo |
| 05 | list-panes-upgrade — window grouping, status, foreign-session section, `get_terminal_context` `scope` param (D4) | todo |
| 06 | tmux-control-tool — approval-gated action tool, `APPROVAL_GATED` wiring, ghost-policy denial (D5) | todo |
| 07 | pane-inspector-cli — widened `PaneList` IPC struct + `/panes` renderer (D7) | todo |
| 08 | filter-unification-and-docs — shared targetable-panes predicate, prefix-literal cleanup, docs true at close (D6) | todo |

Phase docs are drafted one at a time via `/rexymcp:architect next`; none is
drafted yet. Sizing: each phase targets < 500 lines of diff. Phase 06 is the
highest-risk (approval-flow integration + policy semantics) and may split a/b
at drafting time (gate machinery vs. actions) per the M11 a/b convention.

## Notes

(Design decisions made during the milestone, dead ends, and calibration land
here as the milestone progresses.)
