# Phase 06: dedup approval dialogs — no command copy inside the panels

**Milestone:** M15 — Chat Reliability & Dialog UX
**Status:** in-progress
**Depends on:** phase-04 (approval panel), phase-05 (credential panel) — both in-tree
**Estimated diff:** ~180 lines (mostly tests)
**Tags:** language=rust, kind=refactor, size=s

## Goal

The command-approval dialog and the sudo credential dialog must not contain
a copy of the command they are about (PE direction, 2026-08-14). The command
is already committed to chat scrollback directly above the dialog — the
`where_label` panel with `$ <command>` — so repeating it inside the live
panel (and inside the daemon's credential prompt text) is pure duplication.
Remove the duplicate: the approval panel loses its summary row; the daemon's
`CredentialPrompt` text no longer embeds the command.

## Architecture references

Read before starting:

- `src/cli/render_ratatui.rs` — `draw_approval_panel` (the summary row to
  remove) and `draw_credential_panel` (unchanged code, updated test
  fixtures).
- `src/cli/commands/stream.rs` — `prompt_tool_call_ratatui`,
  `read_approval_input_panel`.
- `src/daemon/executor/foreground.rs` — the two `CredentialPrompt` send
  sites.
- `src/daemon/utils/sudo.rs` — where the new prompt-text helper lives.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**Approval flow** (`src/cli/commands/stream.rs:984`,
`prompt_tool_call_ratatui`): first commits the scrollback record — this is
the copy that STAYS, it is "the command directly above the dialog":

```rust
let mut body = vec![format!("$ {}", command)];
if let Some(tp) = target_pane {
    body.push(format!("→ target: {}", tp));
}
let _ = renderer.commit_panel(where_label, &body, false);
```

Then it builds a SECOND copy and threads it into the live panel — this is
the duplicate this phase removes:

```rust
let session_label = if is_sudo { "sudo session" } else { "session" };
let summary = format!("$ {}", command);
let input = read_approval_input_panel(
    renderer,
    stdin,
    "approve command",
    &summary,
    session_label,
    status,
)
.await;
```

`read_approval_input_panel` (`stream.rs:886`) passes `summary` through to
each of its six `renderer.draw_approval_panel(title, summary, session_label,
&line, status)` calls. `draw_approval_panel`
(`src/cli/render_ratatui.rs:460`) renders three content lines inside the
bordered panel — the first is the duplicate:

```rust
let content = Paragraph::new(vec![
    Line::from(Span::styled(
        truncate_with_ellipsis(&summary_owned, inner_width),
        Style::default().fg(Color::Gray),
    )),
    approval_options_line(&session_label_owned, red, yellow),
    Line::from(vec![
        Span::styled("› ", Style::default().fg(yellow)),
        Span::raw(input_text),
    ]),
])
.block(panel);
```

**Credential flow**: the daemon embeds the command in the prompt text at
both `CredentialPrompt` send sites in `src/daemon/executor/foreground.rs`.
The retry loop (`foreground.rs:548`):

```rust
let prompt = if attempt == 0 {
    format!("[sudo] password required for: {}", cmd)
} else {
    format!(
        "sudo: Sorry, try again. \
     Password for attempt {}/{}: {}",
        attempt + 1,
        MAX_SUDO_RETRIES,
        cmd
    )
};
```

and the pre-flight site (`foreground.rs:1187`):

```rust
prompt: format!("[sudo] password required for: {}", cmd),
```

The client (`prompt_credential_ratatui`, `stream.rs:1057`) commits
`⚠ {prompt}` to scrollback and renders `prompt` as the credential panel's
detail row — so the command currently appears **three** times in the
credential flow (approval scrollback panel, ⚠ line, dialog detail row).
Fixing the daemon-side prompt text fixes the latter two at once; the client
code is untouched.

By the time a credential prompt fires, the `$ sudo …` scrollback panel from
the approval flow is always already on screen: sudo commands are
approval-gated, and the `commit_panel` record is written before the
auto-approved check, so it appears even under session auto-approve.

**Codebase idioms**: `src/daemon/utils/mod.rs` re-exports `sudo.rs` items
via `pub use sudo::*;` (line 18); `foreground.rs` imports them through the
`use crate::daemon::utils::{ … }` list at lines 8–13, alphabetically
sorted. `sudo.rs` has an existing `mod tests` at line 147.

## Spec

### 1. `sudo_password_prompt` helper — in `src/daemon/utils/sudo.rs`

Add below `command_has_sudo`:

```rust
/// The credential-prompt text sent to the client for a sudo password
/// request. Deliberately takes no command argument: the command is already
/// on screen directly above the dialog (the approval flow's scrollback
/// panel), so the prompt text must never embed a copy of it.
pub fn sudo_password_prompt(attempt: usize, max: usize) -> String {
    if attempt == 0 {
        "[sudo] password required".to_string()
    } else {
        format!(
            "sudo: Sorry, try again. Password for attempt {}/{}",
            attempt + 1,
            max
        )
    }
}
```

No re-export edit needed in `src/daemon/utils/mod.rs` (`pub use sudo::*;`
already covers it).

### 2. Rewire the two `CredentialPrompt` sites — in `src/daemon/executor/foreground.rs`

Replace the whole `let prompt = if attempt == 0 { … };` expression quoted in
§ Current state (retry loop, line ~548) with:

```rust
let prompt = sudo_password_prompt(attempt, MAX_SUDO_RETRIES);
```

Replace the pre-flight site (line ~1187):

```rust
prompt: sudo_password_prompt(0, MAX_SUDO_RETRIES),
```

Add `sudo_password_prompt` to the `use crate::daemon::utils::{ … }` import
list (alphabetical: between `sudo_credentials_cached` and `sudo_sentinel`).
`cmd` remains used elsewhere in both scopes — do not remove other code.

### 3. Drop the summary row from `draw_approval_panel` — in `src/cli/render_ratatui.rs`

New signature (the `summary` parameter is deleted):

```rust
pub fn draw_approval_panel(
    &mut self,
    title: &str,
    session_label: &str,
    input: &InputLine,
    status: &StatusBarState<'_>,
) -> Result<(), B::Error>
```

In the body: delete the `let summary_owned = summary.to_string();` local,
delete the `let inner_width = …` local (its only use in this function is
the deleted line — it is still used by `draw_credential_panel`, leave that
function alone), and reduce the content vec to two lines:

```rust
let content = Paragraph::new(vec![
    approval_options_line(&session_label_owned, red, yellow),
    Line::from(vec![
        Span::styled("› ", Style::default().fg(yellow)),
        Span::raw(input_text),
    ]),
])
.block(panel);
```

Update the function's doc comment: it no longer holds "the command
summary" — say the panel holds the multicolor Y/A/N options line and the
editable input line, and that the command it approves is in the scrollback
panel directly above. Everything else (layout split, border block, yellow
bold title, status-bar bottom row, `area.height < 6` fallback to
`draw_prompt`) is untouched.

### 4. Update the call sites — in `src/cli/commands/stream.rs`

- `read_approval_input_panel` (line ~886): delete the `summary: &str`
  parameter and remove the `summary` argument from all six
  `renderer.draw_approval_panel(…)` calls inside it.
- `prompt_tool_call_ratatui`: delete the `let summary = format!("$ {}",
  command);` line and the `&summary` argument in the
  `read_approval_input_panel` call. The `commit_panel` scrollback record
  (the `body` vec with `$ {command}`) is NOT touched.

### 5. Unit tests

In `mod tests` of `src/cli/render_ratatui.rs` (reuse
`approval_test_status()` / `buffer_rows()` / the existing renderer
construction shape):

- Update the `draw_approval_panel_test` helper (line ~2476): drop its
  `summary` parameter; update its three surviving callers (lines ~2525,
  ~2609, ~2682 — the ~2643 caller belongs to the deleted test) **and** the
  direct `.draw_approval_panel(` call inside
  `approval_panel_short_region_falls_back` (line ~2720).
- **Delete** `approval_panel_truncates_long_summary` — the row it asserts
  on no longer exists.
- **Add** `approval_panel_has_no_command_row` (the must-NOT case): draw the
  panel with session label `"session"` and empty input on an 80-col
  TestBackend; assert **no** buffer row contains `"$ "` — the panel must
  not render a command row.
- Update the three credential-test fixture details
  `"[sudo] password required for: /usr/bin/apt"` (in
  `credential_panel_title_and_hint`, `credential_panel_shows_bullets`, and
  `credential_panel_short_region_falls_back`) to the new daemon text
  `"[sudo] password required"`. In
  `credential_panel_truncates_long_detail`, change the fixture to
  `format!("[sudo] password required {}", "x".repeat(300))` and the row
  finder from `r.contains("password required for")` to
  `r.contains("password required")`. Assertions otherwise unchanged.

In `mod tests` of `src/daemon/utils/sudo.rs`:

- `sudo_password_prompt_first_attempt_omits_command` —
  `assert_eq!(sudo_password_prompt(0, 3), "[sudo] password required");`
  and `assert!(!sudo_password_prompt(0, 3).contains("for:"));`
- `sudo_password_prompt_retry_names_attempt` —
  `assert_eq!(sudo_password_prompt(1, 3),
  "sudo: Sorry, try again. Password for attempt 2/3");`

### 6. Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`.

## Acceptance criteria

- [ ] `awk '/pub fn draw_approval_panel/,/Result<\(\), B::Error>/' src/cli/render_ratatui.rs | grep -c summary`
      prints `0` (currently 1) — the signature carries no summary.
- [ ] `grep -rn 'password required for' src/ | wc -l` prints `0`
      (currently 7) — no prompt text or fixture embeds a command after
      `password required`.
- [ ] `awk '/fn prompt_tool_call_ratatui/,/^}/' src/cli/commands/stream.rs | grep -c 'format!("\$ {}", command)'`
      prints `1` (currently 2) — the scrollback `commit_panel` copy stays,
      the panel-summary copy is gone.
- [ ] `approval_panel_has_no_command_row` passes: no rendered panel row
      contains `"$ "`.
- [ ] `sudo_password_prompt(0, 3)` returns exactly
      `[sudo] password required`; `sudo_password_prompt(1, 3)` returns
      exactly `sudo: Sorry, try again. Password for attempt 2/3`.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      and `cargo test` all pass.

## Test plan

Named in § Spec task 5: `approval_panel_has_no_command_row` (new),
`sudo_password_prompt_first_attempt_omits_command` (new),
`sudo_password_prompt_retry_names_attempt` (new);
`approval_panel_truncates_long_summary` deleted; the four surviving
`approval_panel_*` tests and four `credential_panel_*` tests updated and
green.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
cargo test --lib approval_panel 2>&1 | tail -12; echo "exit=${PIPESTATUS[0]}"
cargo test --lib sudo_password_prompt 2>&1 | tail -8; echo "exit=${PIPESTATUS[0]}"
awk '/pub fn draw_approval_panel/,/Result<\(\), B::Error>/' src/cli/render_ratatui.rs | grep -c summary; echo "exit=$?"
grep -rn 'password required for' src/ | wc -l; echo "exit=$?"
```

The first `grep -c` must print `0` (and exit 1 — that is the success case);
the `wc -l` must print `0` (exit 0). Redirect the whole run to a file and
paste the file's contents; never retype or summarize.

Live verification (a real chat approval showing the panel without a command
row, and an uncached sudo run showing the credential dialog without the
command in its detail row) is performed **architect-side at milestone
close** together with the phase-01/02/03/05 live checks.

## Authorizations

- Edit `src/cli/render_ratatui.rs`, `src/cli/commands/stream.rs`,
  `src/daemon/executor/foreground.rs`, `src/daemon/utils/sudo.rs` only.
- Run the gate commands. No daemon restart, no tmux interaction, no sudo
  invocation, no files outside the repo.

## Out of scope

- The scrollback `commit_panel` record (`where_label` + `$ {command}` +
  target hint) — it is the surviving, intentional copy.
- The `⚠ {prompt}` scrollback commit and everything else in
  `prompt_credential_ratatui` — the daemon-side text change is the whole
  credential-flow fix.
- Credential handling (`cred_real` lifecycle, `CredentialResponse`,
  `Zeroizing`), the other prompt flows (`prompt_edit_file_ratatui` etc. —
  their diffs/details are content, not duplication), and the legacy
  non-panel prompt path.
- Panel layout retuning (the `area.height < 6` fallback threshold stays).

## Update Log

### Update — 2026-08-14 (created)

Phase drafted by the architect on PE direction: the approval and sudo
credential dialogs must not contain a copy of the command they concern —
the command is already in scrollback directly above. Approval panel loses
its summary row (signature change, six call sites, tests); daemon
credential prompt text loses the `: {cmd}` suffix via a new pure
`sudo_password_prompt` helper (two `foreground.rs` sites, two tests). All
four mechanical criteria were run against the current tree and fail
(1 / 7 / 2 / test-absent). Status: todo.

### Update — 2026-08-14 (started)

Qwen3.8 executor started phase-06. Plan: add `sudo_password_prompt` to
`src/daemon/utils/sudo.rs`, rewire the two `CredentialPrompt` sites in
`foreground.rs`, drop the summary row/parameter from `draw_approval_panel`
and its six call sites, update/add tests, then capture the end-to-end
evidence.

### Update — 2026-08-14 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim from
`/home/matt/src/daemoneye`; output captured mechanically to
`/tmp/m15p6-e2e.txt` and pasted below. The `grep -c summary` over the
`draw_approval_panel` signature prints `0` (exit 1 — the success case) and
`grep -rn 'password required for' src/ | wc -l` prints `0` (exit 0),
satisfying the two pinned-literal criteria.

```
exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
exit=0
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 5 tests
test cli::render_ratatui::tests::approval_panel_short_region_falls_back ... ok
test cli::render_ratatui::tests::approval_panel_sudo_session_label ... ok
test cli::render_ratatui::tests::approval_panel_has_no_command_row ... ok
test cli::render_ratatui::tests::approval_panel_shows_typed_input ... ok
test cli::render_ratatui::tests::approval_panel_options_multicolor ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 1262 filtered out; finished in 0.00s

exit=0
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 2 tests
test daemon::utils::sudo::tests::sudo_password_prompt_first_attempt_omits_command ... ok
test daemon::utils::sudo::tests::sudo_password_prompt_retry_names_attempt ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 1265 filtered out; finished in 0.00s

exit=0
0
exit=1
0
exit=0
```

Grep proof the pinned literals landed: `awk '/pub fn
draw_approval_panel/,/Result<\(\), B::Error>/' src/cli/render_ratatui.rs |
grep -c summary` → `0`; `grep -rn 'password required for' src/ | wc -l` →
`0`; `awk '/fn prompt_tool_call_ratatui/,/^}/' src/cli/commands/stream.rs |
grep -c 'format!("\$ {}", command)'` → `1` (the scrollback `commit_panel`
copy is the sole remaining one).
