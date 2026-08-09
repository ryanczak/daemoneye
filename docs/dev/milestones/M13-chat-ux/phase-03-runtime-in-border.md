# Phase 03: Runtime in the panel border; body word-wrap; turn/budget label

**Milestone:** M13 — Chat UX Polish
**Status:** todo
**Depends on:** phase-02
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Three panel-rendering upgrades in `daemoneye chat`. (1) A tool's run time no
longer appears as a separate `result` history entry — the tool's single panel
carries `✓ 1.2s` right-justified in its bottom border. (2) Panel body lines
word-wrap at the terminal width instead of being truncated with `…` (long user
input is silently cut off today — a regression vs the legacy renderer).
(3) The user-echo panel's bottom border regains the legacy
`turn N · <tokens> / <window> (<pct>%)` context label.

## Architecture references

Read before starting:

- `docs/dev/milestones/M13-chat-ux/README.md` § "Derived code facts" issue 5
  and § "Legacy-renderer delta audit" — where these requirements come from.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(All line numbers verified 2026-08-09 against the post-phase-02 tree.)

- **`commit_panel`** (`src/cli/render_ratatui.rs:352`) truncates each body
  line: `let truncated = truncate_with_ellipsis(line, inner.saturating_sub(2));`
  (`:394`) and draws an unlabeled bottom rule
  `format!("╰{}╯", "─".repeat(inner))`. Border/title styles come from
  `self.palette.red()` / `self.palette.yellow()` (phase-01).
  `truncate_with_ellipsis` (`:632`) is **also** used by `commit()` (`:217`)
  and has its own tests — it must NOT be deleted; only `commit_panel` stops
  using it.
- **The separate runtime entry.** `src/cli/commands/stream.rs:578-590`:

  ```rust
  Response::ToolStarted { tool, summary, .. } => {
      if !summary.is_empty() {
          let _ = renderer.commit_panel(&tool, &[format!("▸ {}", summary)], false);
      } else {
          let _ = renderer.commit_panel(&tool, &["▸ running".to_string()], false);
      }
  }
  Response::ToolFinished { ok, elapsed_ms, .. } => {
      let status = if ok { "✓" } else { "✗" };
      let secs = elapsed_ms as f64 / 1000.0;
      let _ =
          renderer.commit_panel("result", &[format!("{} ({:.1}s)", status, secs)], true);
  }
  ```

  Wire facts (verified in `src/daemon/executor/mod.rs`): `ToolStarted` /
  `ToolFinished` fire only for the **silent** tools
  (`should_emit_tool_feedback`, `src/ai/types/pending.rs:563`) —
  approval-gated tools (incl. `run_terminal_command`) never emit them, and
  their `Response::ToolResult` output carries no elapsed time on the wire at
  all. So the separate-`result`-entry symptom is entirely the silent-tool
  pair above, and merging it is a **CLI-only** change.
- **Scrollback is immutable** (`insert_before`) — a committed `ToolStarted`
  panel can never be amended. Embedding the runtime therefore requires
  *deferring* the panel commit until `ToolFinished`.
- The response match sits in a loop that ends just before the
  `// Update approval from config in case it changed during the turn.` block
  (`stream.rs:624`).
- **User echo site** (`src/cli/commands/chat.rs:524-527`, post-phase-02):

  ```rust
  if should_echo(&query) {
      let echo_body: Vec<String> = echo_body(&query);
      let _ = renderer.commit_panel(&user_host, &echo_body, false);
  }
  ```

  `prompt_tokens: u32` and `context_window` are loop-local `mut` variables
  (declared at `:267` / `:271`) and in scope at the echo site. **There is no
  turn counter in this loop** — the legacy one died with `print_user_query`;
  this phase adds one.
- **The legacy bottom-label pattern to port** (`src/cli/render.rs:114-126`,
  dead code — port the shape, do not call or modify `render.rs` except that
  `wrap_line_hard` at `:133` is `pub` and IS called):

  ```rust
  let label = format!(" turn {} · {} ", turn, budget_label);
  let label_vis = visual_len(&label);
  let dashes = inner.saturating_sub(label_vis + 1);
  // ╰────────────────── label ─╯   (label right-justified, one dash + corner after)
  ```

- `wrap_line_hard(s, width) -> Vec<String>` (`src/cli/render.rs:133-162`) is
  the ANSI-aware hard-wrapper the legacy panels used; it splits on `\n` and
  breaks at exactly `width` visible chars, skipping escape sequences.

## Spec

### Task 1 — `commit_panel_labeled` in `render_ratatui.rs`

Add a new method on `RatatuiRenderer<B>` directly below `commit_panel`, and
turn `commit_panel` into a delegator — **additive shape; every existing
`commit_panel` call site stays untouched**:

```rust
pub fn commit_panel(
    &mut self,
    title: &str,
    body: &[String],
    dim_body: bool,
) -> Result<(), B::Error> {
    self.commit_panel_labeled(title, body, dim_body, None)
}

pub fn commit_panel_labeled(
    &mut self,
    title: &str,
    body: &[String],
    dim_body: bool,
    bottom_label: Option<&str>,
) -> Result<(), B::Error> {
    // ... current commit_panel body, with the two changes below ...
}
```

**Change A — word-wrap instead of truncation.** Replace the body loop's
`truncate_with_ellipsis` call with the legacy ANSI-aware wrapper, one output
row per wrapped segment (this exact call is pinned — it is mutation M2's
target):

```rust
for line in body {
    for seg in crate::cli::render::wrap_line_hard(line, inner.saturating_sub(2)) {
        lines.push(Line::from(Span::styled(format!("  {}", seg), body_style)));
    }
}
```

`row_count` is computed from `lines.len()` after building, so the
`insert_before` height follows automatically — verify that stays true.

**Change B — optional bottom label.** Replace the plain bottom rule with:

```rust
let bottom_line: Line<'static> = match bottom_label {
    Some(label) => {
        let padded = format!(" {label} ");
        let label_vis = padded.chars().count();
        let dashes = inner.saturating_sub(label_vis + 1);
        Line::from(vec![
            Span::styled(format!("╰{}", "─".repeat(dashes)), border_style),
            Span::styled(padded, title_style),
            Span::styled("─╯".to_string(), border_style),
        ])
    }
    None => Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(inner)),
        border_style,
    )),
};
lines.push(bottom_line);
```

(Width check: `╰` + dashes + label_vis + `─╯` = 1 + (inner − label_vis − 1) +
label_vis + 2 = inner + 2 — same total as the unlabeled `╰{inner}╯` row.)

### Task 2 — Merge the silent-tool pair in `stream.rs`

In `src/cli/commands/stream.rs`:

1. Add a pure helper (near the other free functions) — pinned exactly, it is
   tested and its format is the user-visible contract:

   ```rust
   /// Bottom-border label for a finished tool: "✓ 1.2s" / "✗ 0.5s".
   fn tool_runtime_label(ok: bool, elapsed_ms: u64) -> String {
       let status = if ok { "✓" } else { "✗" };
       format!("{status} {:.1}s", elapsed_ms as f64 / 1000.0)
   }
   ```

2. Declare `let mut pending_tool: Option<(String, Vec<String>)> = None;`
   before the response loop.
3. Rewrite the `ToolStarted` arm to **buffer instead of commit**:

   ```rust
   Response::ToolStarted { tool, summary, .. } => {
       let body = if !summary.is_empty() {
           vec![format!("▸ {}", summary)]
       } else {
           vec!["▸ running".to_string()]
       };
       pending_tool = Some((tool, body));
   }
   ```

4. Rewrite the `ToolFinished` arm to commit **one** panel with the runtime in
   the border; keep a fallback for an unmatched finish:

   ```rust
   Response::ToolFinished { ok, elapsed_ms, .. } => {
       let label = tool_runtime_label(ok, elapsed_ms);
       match pending_tool.take() {
           Some((title, body)) => {
               let _ = renderer.commit_panel_labeled(&title, &body, false, Some(&label));
           }
           None => {
               let _ = renderer.commit_panel_labeled("result", &[label.clone()], true, None);
           }
       }
   }
   ```

5. **Flush on clean loop exit:** immediately before the
   `// Update approval from config` block (`:624`), flush a started-but-never-
   finished tool so its panel is not lost:

   ```rust
   if let Some((title, body)) = pending_tool.take() {
       let _ = renderer.commit_panel(&title, &body, false);
   }
   ```

   (Early `?` returns skip the flush — acceptable: those paths tear down the
   stream entirely. Do not restructure error handling for this.)

The spinner keeps animating between `ToolStarted` and `ToolFinished`, so the
user still has live feedback while the panel is deferred. A rare `SystemMsg`
committed during that window now lands *before* the tool panel instead of
between the pair — accepted, do not special-case it.

### Task 3 — Turn counter + budget label at the echo site

In `src/cli/commands/chat.rs`:

1. Add a pure helper (near `user_host_label`) — pinned exactly; it is
   mutation M1's target:

   ```rust
   /// Bottom-border context label for the user-echo panel.
   fn turn_budget_label(turn: usize, prompt_tokens: u32, context_window: u32) -> String {
       let budget = if prompt_tokens == 0 {
           "new session".to_string()
       } else {
           let pct = (prompt_tokens as f64 / f64::from(context_window.max(1)) * 100.0) as u32;
           format!("{prompt_tokens} / {context_window} ({pct}%)")
       };
       format!("turn {turn} · {budget}")
   }
   ```

2. Declare `let mut turn: usize = 0;` alongside the other loop-local state
   (near `:267`).
3. At the echo site, count the query and attach the label (a slash command or
   client-only input never reaches this point counted — the increment sits
   inside the send path, right before the echo block):

   ```rust
   turn += 1;
   if should_echo(&query) {
       let echo_body: Vec<String> = echo_body(&query);
       let label = turn_budget_label(turn, prompt_tokens, context_window);
       let _ = renderer.commit_panel_labeled(&user_host, &echo_body, false, Some(&label));
   }
   ```

   Place `turn += 1;` at the same spot the echo block sits today (after the
   slash-command dispatch has `continue`d away non-queries), so only queries
   that go to the daemon are counted.

### Task 4 — Tests

Write the tests named in § Test plan. The `render_ratatui.rs` panel tests
follow the buffer-cell shape of `commit_panel_uses_blood_red_border_and_yellow_title`
(`:1157`). If `src/cli/commands/stream.rs` has no `#[cfg(test)] mod tests`,
add one for `tool_runtime_label`; `chat.rs`'s existing `mod tests` takes the
`turn_budget_label` tests.

### Task 5 — Mutation M1 apply + restore (label separator)

Apply a `patch` on `src/cli/commands/chat.rs` changing
`format!("turn {turn} · {budget}")` to `format!("turn {turn} - {budget}")`,
then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-03.txt
cargo test --lib turn_budget_label 2>&1 | tail -5 >> /tmp/e2e-m13-03.txt
```

`turn_budget_label_new_session` must show **FAILED**. If it stays green,
report a blocker — do not adjust a test to make it fail. Restore with the
inverse `patch`, then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-03.txt
grep -c 'turn {turn} · {budget}' src/cli/commands/chat.rs >> /tmp/e2e-m13-03.txt
cargo test --lib turn_budget_label 2>&1 | tail -5 >> /tmp/e2e-m13-03.txt
```

The grep count must be `1` and the tests green.

### Task 6 — Mutation M2 apply + restore (wrap width)

Apply a `patch` on `src/cli/render_ratatui.rs` changing
`wrap_line_hard(line, inner.saturating_sub(2))` to
`wrap_line_hard(line, usize::MAX)`, then:

```sh
echo "== M2 APPLIED ==" >> /tmp/e2e-m13-03.txt
cargo test --lib commit_panel_wraps_long_body_lines 2>&1 | tail -5 >> /tmp/e2e-m13-03.txt
```

The test must show **FAILED** (with an effectively-infinite width nothing
wraps). Restore with the inverse `patch`, then:

```sh
echo "== M2 RESTORED ==" >> /tmp/e2e-m13-03.txt
grep -c 'wrap_line_hard(line, usize::MAX)' src/cli/render_ratatui.rs >> /tmp/e2e-m13-03.txt
cargo test --lib commit_panel 2>&1 | tail -5 >> /tmp/e2e-m13-03.txt
```

The grep count must be `0` and all `commit_panel` tests green.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m13-03.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `grep -c 'commit_panel("result"' src/cli/commands/stream.rs` prints `0`.
      (Currently: 1.)
- [ ] `grep -c 'wrap_line_hard' src/cli/render_ratatui.rs` prints `1` — the
      call in `commit_panel_labeled`. (Currently: 0.)
- [ ] `grep -c 'turn_budget_label' src/cli/commands/chat.rs` prints at least
      `3` (definition + call + tests). (Currently: 0.)
- [ ] Tests `commit_panel_wraps_long_body_lines`,
      `commit_panel_bottom_label_right_justified`,
      `commit_panel_without_label_keeps_plain_rule`,
      `tool_runtime_label_formats_ok_and_err`,
      `turn_budget_label_new_session`, `turn_budget_label_reports_usage`
      pass. (Currently: none exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] `commit_panel_uses_blood_red_border_and_yellow_title` and
      `commit_panel_borders_follow_palette_depth` still pass (short bodies,
      no label — behavior unchanged through the delegator).
- [ ] `truncate_with_ellipsis_*` tests still pass (the function stays, used
      by `commit()`).
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

In `src/cli/render_ratatui.rs` `mod tests` (TestBackend is 60×10, so
`inner = 58`, wrap width 56):

- `commit_panel_wraps_long_body_lines` — commit a panel whose single body
  line is 100 `x` chars; assert **no** cell in the buffer holds `…`, and the
  characters of the line continue on the row below the first body row (e.g.
  both body rows start with `x` at x = 2). (Mutation M2 target.)
- `commit_panel_bottom_label_right_justified` —
  `commit_panel_labeled("output", &body, true, Some("✓ 1.2s"))`; the bottom
  border row's text contains `✓ 1.2s` and ends with `─╯`; the label cells
  carry the palette title color (truecolor test palette → `Rgb(220, 160, 0)`).
- `commit_panel_without_label_keeps_plain_rule` — `commit_panel(...)` (no
  label): bottom row is exactly `╰` + 58 `─` + `╯`.

In `src/cli/commands/stream.rs` `mod tests`:

- `tool_runtime_label_formats_ok_and_err` —
  `tool_runtime_label(true, 1234)` == `"✓ 1.2s"` and
  `tool_runtime_label(false, 450)` == `"✗ 0.5s"` (note the `{:.1}` rounding).

In `src/cli/commands/chat.rs` `mod tests`:

- `turn_budget_label_new_session` — `turn_budget_label(1, 0, 200_000)` ==
  `"turn 1 · new session"`. (Mutation M1 target.)
- `turn_budget_label_reports_usage` —
  `turn_budget_label(3, 50_000, 200_000)` == `"turn 3 · 50000 / 200000 (25%)"`.

## End-to-end verification

```sh
: > /tmp/e2e-m13-03.txt
echo "== GATES ==" >> /tmp/e2e-m13-03.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-03.txt; echo "fmt exit=$?" >> /tmp/e2e-m13-03.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-03.txt; echo "build exit=$?" >> /tmp/e2e-m13-03.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-03.txt; echo "clippy exit=$?" >> /tmp/e2e-m13-03.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-03.txt; echo "test exit=$?" >> /tmp/e2e-m13-03.txt
echo "== SURFACES ==" >> /tmp/e2e-m13-03.txt
echo "result panels: $(grep -c 'commit_panel(\"result\"' src/cli/commands/stream.rs)" >> /tmp/e2e-m13-03.txt
echo "wrap calls: $(grep -c 'wrap_line_hard' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-03.txt
echo "label fn uses: $(grep -c 'turn_budget_label' src/cli/commands/chat.rs)" >> /tmp/e2e-m13-03.txt
wc -l /tmp/e2e-m13-03.txt >> /tmp/e2e-m13-03.txt
```

(The mutation runs of Tasks 5-6 append into the same file in task order.)

A live visual check (single tool panel with `✓ Ns` in its border, wrapped long
input, `turn N` label) happens at milestone close with the other live checks.

## Authorizations

None.

## Out of scope

- Adding elapsed time to `Response::ToolResult` for approval-gated commands —
  an IPC/daemon change; the wire carries no elapsed for them today and this
  phase is CLI-only. If wanted later it is its own phase.
- Cursor math (phase 04), resize handling (phase 05), deleting `render.rs`
  dead code (phase 05 — `wrap_line_hard` gains a live caller here and stays).
- Do not change `commit_panel`'s existing call sites (the delegator keeps
  them source-compatible), `truncate_with_ellipsis`, `commit()`,
  `commit_styled()`, or anything under `src/daemon/` / `src/ipc.rs`.
- Do not touch `src/cli/render.rs` (its dead code is phase 05's).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
