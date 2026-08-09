# Phase 02: Flush-left throbber and user@host history attribution

**Milestone:** M13 — Chat UX Polish
**Status:** todo
**Depends on:** phase-01
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Two small chat-history fixes: the streaming throbber renders two spaces in
from the left edge and must be flush with column 0; and user messages in chat
history are titled with the literal `you` instead of the user's identity
(`matt@pinky`-style `user@shorthost` of the machine `daemoneye chat` runs on).

## Architecture references

Read before starting:

- `docs/dev/milestones/M13-chat-ux/README.md` § "Derived code facts" issues
  3 and 4 — the milestone-wide inventory these facts come from.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(All line numbers re-verified 2026-08-09 against the post-phase-01 tree.)

- **Throbber indent.** `draw_spinner` (`src/cli/render_ratatui.rs:270`)
  builds the spinner line with a literal two-space leading span at `:293`:

  ```rust
  let spinner_line = Line::from(vec![
      Span::raw("  "),
      Span::styled(open, blood_red),
      Span::styled(center, bright_yellow),
      Span::styled(close, blood_red),
      Span::styled(format!(" {verb}"), blood_red),
      Span::styled(".".repeat(dot_count), bright_yellow),
  ]);
  ```

  The spinner row itself already starts at x = 0 (`render_spinner_region`
  renders at `spinner_rect.x`), so the `Span::raw("  ")` is the whole offset.
  The existing spinner tests (`spinner_renders_above_input_box_not_inside_it`
  at `:1373` and siblings) assert only `contains(...)`, never a column — they
  keep passing when the indent is removed.

- **The `you` label.** `src/cli/commands/chat.rs:522-524`:

  ```rust
  if should_echo(&query) {
      let echo_body: Vec<String> = echo_body(&query);
      let _ = renderer.commit_panel("you", &echo_body, false);
  }
  ```

  This is the only `commit_panel("you", ...)` call site in the tree.

- **Identity sources.** The robust hostname read already exists:
  `crate::daemon::utils::daemon_hostname()` (`src/daemon/utils/host.rs:2`,
  re-exported via `pub use host::*` in `src/daemon/utils/mod.rs:13`) —
  `/proc/sys/kernel/hostname` → `$HOSTNAME` → `"unknown"`. The dead legacy
  `local_user_host()` in `src/cli/render.rs:38-48` leans on `$HOSTNAME` only
  (a bash-only variable, often unexported under tmux/ssh) — do **not** call
  it and do **not** modify `render.rs` (it is deleted wholesale in phase 05).

- `chat.rs` already has a `#[cfg(test)] mod tests` (around `:885`) holding
  `echo_body_splits_multiline_query` etc. — the new pure-function tests join
  it.

## Spec

### Task 1 — Remove the throbber indent

In `src/cli/render_ratatui.rs` `draw_spinner`, delete the
`Span::raw("  "),` element (`:293`) from `spinner_line`. Nothing else in the
function changes. The interrupt variant (`draw_spinner("⚡", "interrupt?", 0,
&sb)` at `src/cli/commands/stream.rs:234`) needs no change — with the indent
gone its glyph lands at column 0 too.

### Task 2 — Identity helper in `chat.rs`

In `src/cli/commands/chat.rs`, add a **pure** label function plus a thin
env-reading wrapper (same hermetic-test pattern as phase-01's
`detect_color_depth` / `detect_from_env` split — the pure function never
touches the environment):

```rust
/// Build the chat-history attribution label from identity parts.
/// `host` is domain-stripped (`pinky.home.planetfoo.org` → `pinky`).
/// A missing/`"unknown"` host degrades to the bare user; a missing user
/// degrades to the literal `you`.
fn user_host_label(user: Option<&str>, host: Option<&str>) -> String {
    let Some(user) = user.filter(|u| !u.is_empty()) else {
        return "you".to_string();
    };
    let short = host
        .map(|h| h.split('.').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if short.is_empty() || short == "unknown" {
        user.to_string()
    } else {
        format!("{user}@{short}")
    }
}

/// The label for this CLI process's host — the machine `daemoneye chat`
/// runs on, which can differ from the daemon host.
fn local_user_host() -> String {
    let user = std::env::var("USER").ok();
    let host = crate::daemon::utils::daemon_hostname();
    user_host_label(user.as_deref(), Some(&host))
}
```

(`daemon_hostname()` reads the *calling process's* `/proc/sys/kernel/hostname`,
so calling it from the CLI yields the CLI host — the right one here.)

### Task 3 — Use it at the echo site

In `chat.rs`, compute the label **once** before the REPL loop (it cannot
change mid-session):

```rust
let user_host = local_user_host();
```

and replace the literal at the echo site (`:524`):

```rust
let _ = renderer.commit_panel(&user_host, &echo_body, false);
```

### Task 4 — Tests

Write the tests named in § Test plan: the three `user_host_label_*` tests in
`chat.rs`'s existing `mod tests`; the spinner-column test in
`render_ratatui.rs`'s `mod tests`, following the buffer-cell shape of
`spinner_renders_above_input_box_not_inside_it` (`:1373`) — locate the
spinner row via the existing `corner_row` helper, then assert on
`buf.cell((0, spinner_row))`.

### Task 5 — Mutation M1 apply (spinner indent)

Using the `patch` tool on `src/cli/render_ratatui.rs`, re-insert the indent:
old_str

```rust
        let spinner_line = Line::from(vec![
            Span::styled(open, blood_red),
```

new_str

```rust
        let spinner_line = Line::from(vec![
            Span::raw("  "),
            Span::styled(open, blood_red),
```

Then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-02.txt
cargo test --lib spinner_glyph_renders_at_column_zero 2>&1 | tail -5 >> /tmp/e2e-m13-02.txt
```

The test must show **FAILED**. If it stays green, report a blocker — do not
adjust the test to make it fail.

### Task 6 — Mutation M1 restore

Apply the inverse `patch` (old_str = the three-line form, new_str = the
two-line form). Then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-02.txt
grep -c 'Span::raw("  ")' src/cli/render_ratatui.rs >> /tmp/e2e-m13-02.txt
cargo test --lib spinner_glyph_renders_at_column_zero 2>&1 | tail -5 >> /tmp/e2e-m13-02.txt
```

The grep count must be `0` and the test green.

### Task 7 — Mutation M2 apply + restore (label join)

Apply a `patch` on `src/cli/commands/chat.rs` changing
`format!("{user}@{short}")` to `format!("{user}")`, then:

```sh
echo "== M2 APPLIED ==" >> /tmp/e2e-m13-02.txt
cargo test --lib user_host_label 2>&1 | tail -5 >> /tmp/e2e-m13-02.txt
```

`user_host_label_joins_user_and_shorthost` must show **FAILED**. Restore with
the inverse `patch`, then:

```sh
echo "== M2 RESTORED ==" >> /tmp/e2e-m13-02.txt
grep -c 'format!("{user}@{short}")' src/cli/commands/chat.rs >> /tmp/e2e-m13-02.txt
cargo test --lib user_host_label 2>&1 | tail -5 >> /tmp/e2e-m13-02.txt
```

The grep count must be `1` and all `user_host_label` tests green.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m13-02.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `grep -c 'Span::raw("  ")' src/cli/render_ratatui.rs` prints `0`.
      (Currently: 1.)
- [ ] `grep -c 'commit_panel("you"' src/cli/commands/chat.rs` prints `0`.
      (Currently: 1.)
- [ ] Tests `spinner_glyph_renders_at_column_zero`,
      `user_host_label_joins_user_and_shorthost`,
      `user_host_label_missing_user_falls_back_to_you`, and
      `user_host_label_unknown_host_degrades_to_bare_user` pass. (Currently:
      none exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] `spinner_renders_above_input_box_not_inside_it` and
      `spinner_row_is_blank_when_idle` still pass.
- [ ] `echo_skips_client_only_commands` still passes (the echo gating is
      untouched; only the title string changes).
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

In `src/cli/commands/chat.rs` `mod tests`:

- `user_host_label_joins_user_and_shorthost` —
  `user_host_label(Some("matt"), Some("pinky.home.planetfoo.org"))` ==
  `"matt@pinky"`. (Mutation M2 target.)
- `user_host_label_missing_user_falls_back_to_you` —
  `user_host_label(None, Some("pinky"))` == `"you"`, and
  `user_host_label(Some(""), Some("pinky"))` == `"you"`.
- `user_host_label_unknown_host_degrades_to_bare_user` —
  `user_host_label(Some("matt"), Some("unknown"))` == `"matt"`, and
  `user_host_label(Some("matt"), Some(""))` == `"matt"`.

In `src/cli/render_ratatui.rs` `mod tests`:

- `spinner_glyph_renders_at_column_zero` — after
  `draw_spinner("(◉)", "scrying", 3, &status)`, the spinner row's cell at
  x = 0 holds `(` (assert the cell symbol equals `"("`, not merely
  non-space). (Mutation M1 target.)

## End-to-end verification

```sh
: > /tmp/e2e-m13-02.txt
echo "== GATES ==" >> /tmp/e2e-m13-02.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-02.txt; echo "fmt exit=$?" >> /tmp/e2e-m13-02.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-02.txt; echo "build exit=$?" >> /tmp/e2e-m13-02.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-02.txt; echo "clippy exit=$?" >> /tmp/e2e-m13-02.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-02.txt; echo "test exit=$?" >> /tmp/e2e-m13-02.txt
echo "== SURFACES ==" >> /tmp/e2e-m13-02.txt
echo "indent spans: $(grep -c 'Span::raw("  ")' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-02.txt
echo "you literals: $(grep -c 'commit_panel(\"you\"' src/cli/commands/chat.rs)" >> /tmp/e2e-m13-02.txt
wc -l /tmp/e2e-m13-02.txt >> /tmp/e2e-m13-02.txt
```

(The mutation runs of Tasks 5-7 append into the same file in task order; the
artifact accumulates chronologically.)

A live visual check (throbber at column 0, `matt@<host>` panel title in a real
tmux session) happens at milestone close with the other live checks — not in
this phase's executor block.

## Authorizations

None.

## Out of scope

- The runtime-in-border / panel word-wrap work — phase 03 (do not touch
  `commit_panel`'s signature or body).
- Cursor math (phase 04), resize handling (phase 05).
- Deleting the dead legacy `local_user_host` in `src/cli/render.rs` — phase
  05 removes that file's dead code wholesale. Do not modify `render.rs`.
- Any change under `src/daemon/` — `daemon_hostname()` is consumed as-is.
- The `"⚡"` interrupt-variant call site in `stream.rs` — no change needed.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
