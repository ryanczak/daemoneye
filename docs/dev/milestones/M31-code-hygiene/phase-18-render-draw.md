# Phase 18 — Move `RatatuiRenderer`'s drawing methods out of `render_ratatui.rs`

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/render_ratatui.rs` (1225 production lines) drops under the
800-line cap. Its generic `impl<B: Backend> RatatuiRenderer<B>` block moves
into a new `src/cli/render_ratatui_draw.rs`, declared as a child module of
`render_ratatui`. The block holds `draw` and its panel variants, `commit`,
`commit_panel` and the repaint history. A child module may implement its
parent's type and read the type's private fields, so nothing is
re-exported and no caller changes. The struct, the Stdout-backend impl, the
ANSI parser and the region renderers stay. Pure move: no line inside the
block changes.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: the cut, the module declaration, the allowlist line, the
   CLAUDE.md row, then `cargo fmt`. It was dry-run on a scratch worktree of
   `d3bc6ad3`: clippy and the full suite (2586) green, the moved block
   byte-identical. Do not hand-edit anything it touches. Do not dry-run it
   again in a worktree of your own. Do not `sed -i` a source file.

   Every method that crosses the boundary is already `pub`. The one private
   method, `record_committed_rows`, is called only inside the block.

   ```bash
   D=src/cli; F=$D/render_ratatui.rs; N=$D/render_ratatui_draw.rs
   S=$(grep -n '^impl<B: Backend> RatatuiRenderer<B> {$' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// Render the live (non-committed) region' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 401 939
   { printf '//! `RatatuiRenderer`'"'"'s drawing and committing methods, for any backend:\n//! `draw` and its panel variants, `commit` and `commit_panel`, and the repaint\n//! history they keep.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "render_ratatui_draw.rs"]\nmod draw;\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/render_ratatui.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/viewer_layout.rs` \|/ {print "| `src/cli/render_ratatui_draw.rs` | `RatatuiRenderer<B>`'"'"'s drawing and committing methods (`draw`, `draw_spinner`, `draw_prompt`, the approval / credential / picker panels, `commit`, `commit_panel`, the repaint history); a child module of `src/cli/render_ratatui.rs` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `render_ratatui_draw.rs` once and record
   what you see, or `none`. It was flagged at scoping as a 700-line block;
   note where `draw`'s panel variants share code.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-18.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^moved_identical=1$'` on this doc before you commit; it must
   print 1.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `render_ratatui_draw.rs` holds `impl<B: Backend> RatatuiRenderer<B>`,
      and `render_ratatui.rs` no longer does. `render_ratatui.rs` has at
      most 800 production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/cli/render_ratatui.rs` line.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `moved_identical=1` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=d3bc6ad3; D=src/cli; F=$D/render_ratatui.rs; N=$D/render_ratatui_draw.rs
A=/tmp/e2e-m31-18.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/render_ratatui.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-18-old.txt; cat $F $N | norm > /tmp/m31-18-new.txt
echo "lost_other=$(comm -23 /tmp/m31-18-old.txt /tmp/m31-18-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-18-old.txt /tmp/m31-18-new.txt | grep -vcE '^(#\[path=|//!|moddraw;$|usesuper::\*;$)')" >> "$A"
if diff -q <(git show $B:$F | sed -n 401,939p) <(tail -n +7 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `694 …render_ratatui.rs`,
`545 …render_ratatui_draw.rs`, `allowlisted=0`, `lost_other=0`,
`added_other=0`, `moved_identical=1`, hygiene `ok.`, clippy clean,
`fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/render_ratatui.rs`; create `render_ratatui_draw.rs` beside
  it. Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md` through the script
  only.

## Out of scope

- The struct, the Stdout-backend impl, the ANSI parser, the region
  renderers and both test files.
- Any change inside the moved block.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 00:08 (started)

**Executor:** headless run.
Phase doc flipped to in-progress, README phase table updated. Beginning the cut script (Spec 1).

### Update — 2026-10-28 00:09 (cuts done)

Ran the Spec 1 script once with `bash`, exactly as written. Output:

```
S=401 E=939
  694 src/cli/render_ratatui.rs
  545 src/cli/render_ratatui_draw.rs
 1239 total
```

`cargo fmt --all` inside the script reported nothing to change. The moved
block went into `src/cli/render_ratatui_draw.rs` under the `use super::*;`
header, `#[path = "render_ratatui_draw.rs"] mod draw;` landed where the impl
block was, the allowlist lost its `file src/cli/render_ratatui.rs 1225`
line, and the CLAUDE.md row was inserted after the `viewer_layout.rs` row.

### Findings — 2026-10-28 00:12 (decision 7, scoped per Spec 3)

Method: read `render_ratatui_draw.rs` (545 lines) once, end to end, plus the
parent-file helpers it calls (`render_ratatui.rs`: `render_live_region`,
`render_spinner_region`, `render_prompt_region`, `approval_options_line`,
`credential_hint_line`, `picker_window`, `truncate_with_ellipsis`,
`short_session`, `fmt_uptime`, `MAX_REPAINT_HISTORY`) to see what the block
shares. No line changed; findings recorded, not fixed.

**1. (`smell`, minor) Three panel variants duplicate the status-bar tail.**
`draw_approval_panel` (lines ~247–263), `draw_credential_panel`
(~332–348) and `draw_picker_panel` (~419–435) each re-declare the identical
status-bar block: the same `Layout::default()` with
`[Constraint::Min(1), Constraint::Length(1)]`, the same `fmt_uptime(start_time.elapsed())`,
the same `session:{short_session} · {model} · up {uptime}` format string, and
the same dim `DarkGray` status paragraph rendered into `chunks[1]`. The
closure-boundary (each needs `&mut self` inside `terminal.draw`) makes the
obvious extraction mildly awkward, but a free fn taking `(frame, area,
session_id, model, start_time)` would drop ~17 lines per panel. Also a
third copy of the status text lives in `render_live_region` /
`render_spinner_region` / `render_prompt_region` in the parent, so the
format string `" session:{} · {} · up {} "` exists 6+ times across the two
files.

**2. (`smell`, minor) Two panel variants duplicate the panel shell.**
`draw_approval_panel` and `draw_credential_panel` build the same
`Block::default().borders(Borders::ALL).border_type(Rounded).border_style(red)`
+ bold-yellow title span, then differ only in the content paragraph. The
picker variant omits the red border (default style) — possibly intentional,
possibly drift; worth a look when these are next touched.

**3. (`smell`, minor) `commit_panel` / `commit_panel_labeled` render into a
manually-built `Vec<Line>` instead of a `Paragraph` inside the panel Block
the draw variants use.** The committed shape (top/bottom border strings
with manual `─.repeat(fill)` arithmetic) is a separate visual dialect from
the `Block`-based panels; the manual `inner.saturating_sub(title_len + 4)`
fill math silently mis-frames when the title contains wide (double-width)
characters, since `chars().count()` is not display width. Severity is
bounded by the fact that titles here are ASCII tool names today.

**4. (`perf`, nit) `commit` builds the history `Vec<Line>` even when
`insert_before` fails.** `record_committed_rows` is called unconditionally
after `self.terminal.insert_before(...)`, so on a backend error the rows are
still pushed into the repaint history and `inserted_rows` was already
incremented — committed-count drifts from reality on a failed commit. Not
user-visible in the current flow (the error propagates and the caller tears
down), but the accounting is optimistic.

**5. (`smell`, nit) `record_committed_rows` is the only private method in the
block** — correct per spec (called only by `commit`, `commit_styled`,
`commit_panel_labeled` inside the child module), and `commit_panel`'s
`use ratatui::widgets::Clear;` is a fn-local import while everything else
comes from `use super::*;`; harmless but inconsistent with the file's other
imports.

**6. (`smell`, nit) The module doc comment's first line has a stray
backtick** — `` //! `RatatuiRenderer's drawing … `` opens a backtick before
`RatatuiRenderer` that is never closed (the apostrophe consumed the closing
one in the spec's printf). Cosmetic; rustdoc renders it literally.

### Update — 2026-10-28 00:20 (end-to-end verification)

Ran the Spec 4 End-to-end block (bash heredoc) as written; `/tmp/e2e-m31-18.txt`
contents, byte-exact:

```
694 src/cli/render_ratatui.rs
545 src/cli/render_ratatui_draw.rs
1239 total
allowlisted=0
lost_other=0
added_other=0
moved_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every line matches the dry-run expectation: parent 694 / child 545 (both under
the 800 cap), no allowlist entry, nothing lost or added beyond the four
expected new lines, the moved block byte-identical to `d3bc6ad3` lines
401–939, hygiene `ok.`, clippy clean, `fmt_exit=0`, full suite 2586/0.

### Update — 2026-10-28 00:16 (complete, server-authored)

**Summary:** </think>All completion checklist items:

- [x] Phase doc Status: still `in-progress` ✓
- [x] README phase-table row still `in-progress` ✓
- [x] No hand-written `(complete)` entry ✓ (I wrote started, cuts done, Findings, e2e verification)
- [x] `cargo fmt --all` run immediately before git add ✓ (ran, then built/clippy'd which don't modify, then add)
- [x] All verification commands ran clean ✓
- [x] Final message substantive ✓ (about to write)
- [x] `git status --short` clean ✓
- [x] `git log -1 --stat` shows all touched files ✓

Now write the Summary + Notes for review.</think>
**Summary**

Phase 18 executed as a pure move: the generic `impl<B: Backend> RatatuiRenderer<B>` block (539 lines — `draw`, `draw_spinner`, `draw_prompt`, the approval/credential/picker panels, `commit`, `commit_styled`, `commit_panel`/`commit_panel_labeled`, `input_content_width`, `restore`, and the private `record_committed_rows`) left `src/cli/render_ratatui.rs` for the new child module `src/cli/render_ratatui_draw.rs` via the Spec 1 script, run once exactly as written. The parent drops to 694 production lines (under the 800 cap, its allowlist entry removed); the child is 545. The Spec 4 end-to-end block ran clean and matched the dry-run expectation on every line: `allowlisted=0`, `lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — pasted byte-exact into the `(end-to-end verification)` entry, and `grep -c '^moved_identical=1$'` on the phase doc prints 1. Committed as `refactor:` (`139ce685`), working tree clean, status left `in-progress` with no hand-written completion entry.

**Notes for review**

- Findings (decision 7, recorded in the phase doc, not acted on): six items — the three draw-panel variants duplicate an identical ~17-line status-bar tail (and the same `" session:{} · {} · up {} "` format string exists 6+ times across parent and child); the approval and credential panels duplicate the red-bordered `Block` shell while the picker variant silently omits the red border (possible drift); `commit_panel_labeled`'s manual `chars().count()` fill math mis-frames double-width titles; `commit` increments `inserted_rows` and records history rows even when `insert_before` fails; `record_committed_rows` is the block's lone private method (correct per spec) and `commit_panel_labeled` has a fn-local `use ratatui::widgets::Clear;` inconsistent with the file's `use super::*;` style; and the child module's doc-comment first line has a stray unclosed backtick (the spec's printf string consumed the closing one — cosmetic, and I left the script's output untouched per "do not hand-edit anything it touches").

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
 ok
test function_length_spans_signature_to_closing_brace ... ok
test production_lines_exclude_an_inline_test_module ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test functions_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.28s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (1579964): No such process
kill: (-1580690): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-e8cf52a7c32204b4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-e3c81dbbd815c973)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-13504e43862292bc)
     Running tests/hygiene.rs (target/debug/deps/hygiene-5888a12c4886b465)
     Running tests/integration.rs (target/debug/deps/integration-9d2755b5d0d87df4)
     Running tests/isolation.rs (target/debug/deps/isolation-aff2e96eb7fd44a5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-ed33cf3a0b1b3008)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -0
- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-18-render-draw.md` — +107 -1
- `src/cli/render_ratatui.rs` — +2 -539
- `src/cli/render_ratatui_draw.rs` — +545 -0
- `tests/hygiene_allowlist.txt` — +0 -1

**Commit:** 139ce685bc12780c0281841c31e8cc31f2cc5521

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
