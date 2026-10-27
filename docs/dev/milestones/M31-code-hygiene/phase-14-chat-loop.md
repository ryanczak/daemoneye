# Phase 14 — Move the ratatui chat loop out of `chat.rs`

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/commands/chat.rs` (1147 production lines) drops under the 800-line
cap by moving its ratatui chat loop into a new
`src/cli/commands/chat_loop.rs`, declared as a child module of `chat`. The
moved block runs from the `// ── Ratatui chat loop` banner through
`read_input_line_inner_ratatui`. That covers `run_chat_ratatui`,
`RatatuiInputCtx` and the line reader. Both of the file's allowlisted giant
functions move with it, unchanged. `run_chat_inner`, the context structs,
the slash-command matchers, the banner and the label helpers stay in
`chat.rs`. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   code change: the cut, the wiring, both allowlist edits and the CLAUDE.md
   row, then `cargo fmt`. It was dry-run on a scratch worktree of `e6450f12`:
   clippy and the full suite (2586) green. Do not hand-edit anything it
   touches. Do not dry-run it again in a worktree of your own. Do not
   `sed -i` a source file: the script rewrites through temp files and `mv`.

   One line changes inside the moved code: `run_chat_ratatui` gains
   `pub(super)`, because `run_chat_inner` calls it from the parent. The
   moved code reaches four sibling command modules as `super::…`
   (`chat_presence`, `ipc_client`, `agents`, `resume_hint`). From the child,
   `super` is `chat`, so the script adds `use super::{agents, ipc_client,
   resume_hint};` to `chat.rs`, which already imports `chat_presence`, and
   those paths resolve unchanged. **Do not rewrite them to
   `super::super::…`.** A longer path makes rustfmt wrap a line, which grows
   `run_chat_ratatui` past its allowlisted 500 lines, and the hygiene gate
   fails. The first dry run hit exactly that.

   ```bash
   D=src/cli/commands; F=$D/chat.rs; N=$D/chat_loop.rs
   S=$(grep -n '^// ── Ratatui chat loop ' $F | cut -d: -f1)
   E=$(( $(grep -n '^fn banner_lines(' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 276 983
   { printf '//! The ratatui chat loop and its line reader: `run_chat_ratatui` drives a\n//! chat session turn by turn, `read_input_line_inner_ratatui` reads one line.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F | sed 's/^async fn run_chat_ratatui(/pub(super) async fn run_chat_ratatui(/'; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "chat_loop.rs"]\nmod chat_loop;\nuse chat_loop::run_chat_ratatui;\n// The loop reaches these sibling modules as `super::…`, through this import.\nuse super::{agents, ipc_client, resume_hint};\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/chat.rs ' $A \
     | sed -e 's|^fn src/cli/commands/chat.rs run_chat_ratatui |fn src/cli/commands/chat_loop.rs run_chat_ratatui |' \
           -e 's|^fn src/cli/commands/chat.rs read_input_line_inner_ratatui |fn src/cli/commands/chat_loop.rs read_input_line_inner_ratatui |' > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/slash_session.rs` \|/ {print "| `src/cli/commands/chat_loop.rs` | The ratatui chat loop: `run_chat_ratatui` (one chat session, turn by turn: input, slash commands, the ask stream, the viewers) and `read_input_line_inner_ratatui` (one line of input); a child module of `src/cli/commands/chat.rs` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output (`S=… E=…` and the `wc -l` counts). The stall governor
   counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `chat_loop.rs` once and record what you
   see, or `none`. `run_chat_ratatui` (500 lines) and
   `read_input_line_inner_ratatui` (183) will be split later, so note
   their natural seams.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-14.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Your tool shell
   is `dash`, and the heredoc carries the bash-only syntax past it. Check
   with `grep -c '^lost_other=0$'` on this doc before you commit; it must
   print 1.
5. Commit the code as `refactor:`.

## Acceptance criteria

- [ ] `chat_loop.rs` defines `run_chat_ratatui`, `RatatuiInputCtx` and
      `read_input_line_inner_ratatui`. `chat.rs` defines none of them and
      has at most 800 production lines (`cargo test --test hygiene`
      measures it).
- [ ] The allowlist has no `file src/cli/commands/chat.rs` line, and its
      `run_chat_ratatui 500` and `read_input_line_inner_ratatui 183` lines
      name `chat_loop.rs`.
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
B=e6450f12; D=src/cli/commands; F=$D/chat.rs; N=$D/chat_loop.rs
A=/tmp/e2e-m31-14.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/chat.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -cE '^fn src/cli/commands/chat_loop.rs (run_chat_ratatui 500|read_input_line_inner_ratatui 183)$' tests/hygiene_allowlist.txt | sed 's/^/fn_moved=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-14-old.txt; cat $F $N | norm > /tmp/m31-14-new.txt
echo "lost_other=$(comm -23 /tmp/m31-14-old.txt /tmp/m31-14-new.txt | grep -vcE '^asyncfnrun_chat_ratatui\(')" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-14-old.txt /tmp/m31-14-new.txt | grep -vcE '^pub\(super\)asyncfnrun_chat_ratatui\(|^#\[path=|^//|^modchat_loop;$|^usesuper::\*;$|^usechat_loop::run_chat_ratatui;$|^usesuper::\{agents,ipc_client,resume_hint\};$')" >> "$A"
if diff -q <(git show $B:$F | sed -n 276,983p | sed 's/^async fn run_chat_ratatui(/pub(super) async fn run_chat_ratatui(/') <(tail -n +6 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `645 …chat.rs`, `713 …chat_loop.rs`,
`allowlisted=0`, `fn_moved=2`, `lost_other=0`, `added_other=0`,
`moved_identical=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/chat.rs`; create `chat_loop.rs` beside it. Edit
  `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script only.

## Out of scope

- `chat.rs`'s inline `mod tests`, `run_chat_inner` and everything else
  outside the moved block.
- Any change inside the moved functions beyond `run_chat_ratatui`'s
  `pub(super)`, including splitting it or rewriting its `super::` paths.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 17:46 (started)

Starting phase 14. Status flipped to `in-progress` here and in the milestone
README's phase table. Next: run the Spec 1 script once with `bash`, then log
its output per Spec 2.

### Update — 2026-10-27 17:46 (cuts done)

Ran the Spec 1 script once with `bash`. Output:

```
S=276 E=983
  645 src/cli/commands/chat.rs
  713 src/cli/commands/chat_loop.rs
 1358 total
```

`S=276 E=983` matches the script's echo comment (expect 276 983). The `wc -l`
counts match the dry run's expected `645 …chat.rs` and `713 …chat_loop.rs`.
Allowlist edit and CLAUDE.md row applied through the script; `cargo fmt --all`
ran clean.

### Findings — 2026-10-27 17:47

Read `chat_loop.rs` once (713 lines). Decision-7 findings:

- `run_chat_ratatui` is 500 lines. Its natural seams, in order:
  1. The startup prologue (lines ~7–125): ctx destructure, presence hold,
     tmux-attach wait, the `redraw` closure, the initial banner and the
     greeting `ask_with_session_ratatui` block — a setup helper returning the
     loop's state would cut ~120 lines.
  2. The per-turn input dispatch inside the main loop: the `/exit`, `/help`,
     `/shell`, `/attach`, `/log`, `/tail`, `/clear` arms are each a match-arm
     body followed by an identical `redraw(...)` call — a
     `dispatch_slash_command` helper returning `Handled`/`Continue`/
     `SendToModel` would collapse the 11 `redraw` call sites into one tail
     call and cut ~200 lines.
  3. The query-send block (~lines 420–460) duplicates the greeting block's
     `ask_with_session_ratatui` invocation almost line for line — a shared
     `send_query(...)` helper would serve both.
- `read_input_line_inner_ratatui` is 183 lines. Natural seams: the
  `tokio::select!` arms are already one per concern; the `Key` match arm is
  the big one — `StatusBarState` construction is repeated 5 times inside it
  and could be a small `status_bar(...)` helper, and the `Key::CtrlO`
  transcript-viewer arm could become one too. Splitting on select-arm
  boundaries (input vs pushes vs sigwinch) is harder because all three share
  every field of `RatatuiInputCtx`.
- `RatatuiInputCtx` has 16 fields and is constructed exactly once; the
  `redraw` closure in `run_chat_ratatui` takes 10 positional parameters —
  both are the shape a later split should collapse first.
- No other surprises: the file contains only the moved block plus the new
  module docstring and `use super::*;` (I measure `run_chat_ratatui` at 500
  and `read_input_line_inner_ratatui` at 183, matching the allowlist).
  `banner_lines`, `HELP_TEXT`, the slash matchers, `local_user_host`,
  `echo_body`, `should_echo` and `turn_budget_label` all remain in `chat.rs`
  and resolve through `super::*`.

### Update — 2026-10-27 17:50 (end-to-end verification)

Ran the End-to-end block as written (a `bash` heredoc, run with `bash`),
redirecting nothing by hand — the block itself writes `/tmp/e2e-m31-14.txt`
and cats it. Output, byte-exact from `/tmp/e2e-m31-14.txt`:

```
645 src/cli/commands/chat.rs
713 src/cli/commands/chat_loop.rs
1358 total
allowlisted=0
fn_moved=2
lost_other=0
added_other=0
moved_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every value matches the dry run's expectation: `645`/`713` line counts,
`allowlisted=0`, `fn_moved=2`, `lost_other=0`, `added_other=0`,
`moved_identical=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`. `grep -c '^lost_other=0$'` on this doc prints 1.

### Update — 2026-10-27 17:53 (complete, server-authored)

**Summary:** </think>Everything is done. Let me do the final completion checklist:

- [x] Phase doc Status: in-progress (still — server will flip to review)
- [x] Milestone README phase-table row says in-progress
- [x] No hand-written `(complete)` entry
- [x] cargo fmt --all run before git add (ran as part of script, and fmt --check clean after)
- [x] All verification commands ran clean (fmt, clippy, hygiene test, full suite)
- [x] Final message will be substantive Summary + Notes for review
- [x] git status --short shows nothing
- [x] git log -1 --stat shows all touched files

All acceptance criteria:
1. chat_loop.rs defines run_chat_ratatui, RatatuiInputCtx, read_input_line_inner_ratatui ✓; chat.rs defines none and is 645 lines ✓ (hygiene test passed, 16 tests)
2. Allowlist has no `file src/cli/commands/chat.rs` line (allowlisted=0) and the two fn lines name chat_loop.rs (fn_moved=2) ✓
3. E2E prints lost_other=0, added_other=0, moved_identical=1, passed=2586 failed=0 ✓
4. Update Log has (cuts done), Findings, (end-to-end verification) entries, all written with edit tools, all in this file ✓
5. Committed as refactor: ✓

Now write the Summary + Notes for review.</think>
Phase 14 is complete. All spec items executed, verified, and committed.

**Summary**

Ran the Spec 1 script once with `bash`: `S=276 E=983` (matching the script's expectation), producing `src/cli/commands/chat_loop.rs` (713 lines) and trimming `chat.rs` to 645 production lines — under the 800 cap with no `file chat.rs` allowlist row left. The script applied both allowlist edits (`run_chat_ratatui 500` and `read_input_line_inner_ratatui 183` now name `chat_loop.rs`) and the CLAUDE.md row, then `cargo fmt`. Logged the cuts, wrote the decision-7 Findings entry (natural seams for both giant functions: a setup helper for the prologue, a `dispatch_slash_command` returning Handled/Continue/SendToModel to collapse the 11 `redraw` call sites, a shared `send_query` for the duplicated `ask_with_session_ratatui` blocks, and a `status_bar` helper for the 5 repeated `StatusBarState` constructions in the key match), and ran the End-to-end block verbatim as a `bash` heredoc. Committed everything as one `refactor:` commit; `git status --short` is clean.

**End-to-end verification** (byte-exact from `/tmp/e2e-m31-14.txt`): `645 chat.rs` / `713 chat_loop.rs`, `allowlisted=0`, `fn_moved=2`, `lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — every value matches the dry run's expectation. `grep -c '^lost_other=0$'` on the phase doc prints 1.

**Notes for review**

- No deviations from the script; it ran as written and first try, including the `pub(super)` on `run_chat_ratatui` and the `use super::{agents, ipc_client, resume_hint};` import in `chat.rs` (which keeps the moved block byte-identical so the 500-line allowlist count holds).
- One cosmetic hiccup in the commit flow, not in the code: my first commit message was passed through `-m` with embedded backticks, so the shell executed `` `use super::{agents, ipc_client, resume_hint};` `` as command substitution (printing `sh: 1: use: not found` and dropping that phrase from the message). I amended immediately with a heredoc (`-F -`) — the final commit message on `67729a6` is correct and contains the backticked path. No source or doc content was affected.
- Findings for the later split phases are in the `### Findings — 2026-10-27 17:47` entry in the phase doc; the highest-leverage seam is the slash-command dispatch (its `redraw` tail call is repeated at 11 sites).
- Adjacent observation only (not fixed, out of scope): `run_chat_ratatui`'s startup prologue contains a duplicated "Wait for tmux client attachment." comment line that predates this phase.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.3.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.17s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 ok
test production_lines_exclude_an_inline_test_module ... ok
test functions_in_a_test_module_are_not_measured ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test functions_fit_the_cap ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.25s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (1368060): No such process
kill: (-1368768): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-14-chat-loop.md` — +84 -1
- `src/cli/commands/chat.rs` — +5 -708
- `src/cli/commands/chat_loop.rs` — +713 -0
- `tests/hygiene_allowlist.txt` — +2 -3

**Commit:** 67729a67a3b829cfb503f3da1e05f0b39aeee708

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-27

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (41 turns)
- **Scope deviations:** none. `git diff --stat d52e328d HEAD` names the six
  authorized files. The moved block equals `d52e328d:chat.rs` lines 276–983
  apart from `run_chat_ratatui`'s `pub(super)`. The code commit is typed
  `refactor:`. Reviewer's gates: fmt 0, build and clippy clean, 2586
  passed / 0 failed / 13 ignored.
- **Findings:** split seams for both giant functions, carried to
  `FINDINGS.md` #45–#47 as input to their function-split phases.
- **Calibration:** 41 turns, the lowest of the milestone (12: 62, 13: 78).
  Two changes held. The End-to-end block, wrapped as `bash <<'E2E'`, ran
  as written under the executor's `dash` tool shell, with no workaround
  this time. And validating the doc's own script and block in a fresh
  worktree caught a hygiene failure that the clippy-only dry run missed: a
  `super::super::` path rewrite made rustfmt wrap a line, and
  `run_chat_ratatui` grew past its allowlisted 500. The fix was an import
  in the parent instead of a path rewrite.

