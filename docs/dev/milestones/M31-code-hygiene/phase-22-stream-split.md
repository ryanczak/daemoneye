# Phase 22 — Split the chat turn and its prompts out of `cli/commands/stream.rs`

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/cli/commands/stream.rs` (1520 production lines) drops under the
800-line cap. Two blocks move into child modules of `stream`:
- `ask_with_session_ratatui`, the 673-line chat-turn function, goes to
  `stream_ask.rs`, re-exported so `chat.rs`, `chat_loop.rs` and `ask.rs`
  still import it from `super::stream`.
- The `// ── Ratatui interactive approval primitives` section and the
  `// ── Ratatui prompt functions` section after it go to
  `stream_prompts.rs`.

`stream.rs` keeps the four context structs, the stream helpers
(`recv_line`, `select_stream`, `send_cancel`, the silence budget) and its
three inline test modules, at about 234 production lines. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: both cuts, the wiring, both allowlist edits and the CLAUDE.md
   row, then `cargo fmt`. It was dry-run on a scratch worktree of `b8c6a006`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Seven lines change inside the moved code, and the move requires all of
   them:
   - `ask_with_session_ratatui` becomes `pub(in super::super)`: the
     `commands` module, its exact old reach, so `stream` can re-export it.
     Its parameters are already one per line, so the longer signature
     does not wrap, and the function stays at its allowlisted 673 lines.
   - Six private helpers in the prompts block gain `pub(super)`, because
     the parent's `select_stream` or the tests call them:
     `key_outcome`, `tool_runtime_label`, `build_approval_prompt`,
     `parse_approval_response`, `edit_file_auto_approved` and
     `edit_file_grants_session`.

   **The structs stay in `stream.rs` on purpose.** Their 13 fields are all
   `pub(super)`, and moving them would mean widening every one. The moved
   function reads them fine from inside `stream`. The function also reaches
   `super::approval::…`, so the script adds `use super::approval;` to
   `stream.rs`. **Do not rewrite that path to `super::super::…`.**

   ```bash
   D=src/cli/commands; F=$D/stream.rs
   A1=$(grep -n '^/// Stream the AI response through the ratatui renderer\.$' $F | cut -d: -f1)
   A2=$(( $(grep -n '^/// Read one newline-delimited `Response`, accumulating' $F | cut -d: -f1) - 2 ))
   B1=$(grep -n '^// ── Ratatui interactive approval primitives ──' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | head -1 | cut -d: -f1)
   echo "A1=$A1 A2=$A2 B1=$B1 T=$T"       # expect 112 789 903 1517
   { printf '//! `ask_with_session_ratatui`: one chat turn streamed through the ratatui\n//! renderer, from the request to the final token.\n\nuse super::*;\n\n'
     sed -n "${A1},${A2}p" $F | sed 's/^pub(super) async fn ask_with_session_ratatui(/pub(in super::super) async fn ask_with_session_ratatui(/'; } > $D/stream_ask.rs
   { printf '//! The ratatui approval primitives and prompt functions: reading an\n//! approval, and the tool-call, credential, pane, yes/no, write and\n//! edit-file prompts.\n\nuse super::*;\n\n'
     sed -n "${B1},$((T-2))p" $F | sed -e 's/^fn \(key_outcome\|tool_runtime_label\|build_approval_prompt\|parse_approval_response\|edit_file_auto_approved\|edit_file_grants_session\)(/pub(super) fn \1(/'; } > $D/stream_prompts.rs
   { head -n $((A1-1)) $F
     printf '#[path = "stream_ask.rs"]\nmod ask;\npub(super) use ask::ask_with_session_ratatui;\n// `ask` reaches `super::approval::…` through this import.\nuse super::approval;\n\n'
     sed -n "$((A2+2)),$((B1-1))p" $F
     printf '#[path = "stream_prompts.rs"]\nmod prompts;\nuse prompts::*;\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/stream.rs ' $A \
     | sed 's|^fn src/cli/commands/stream.rs ask_with_session_ratatui |fn src/cli/commands/stream_ask.rs ask_with_session_ratatui |' > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/chat_loop.rs` \|/ {print "| `src/cli/commands/stream_ask.rs`, `stream_prompts.rs` | The chat turn'"'"'s client side, child modules of `src/cli/commands/stream.rs` (the context structs and the stream helpers): `stream_ask.rs` `ask_with_session_ratatui` (re-exported), `stream_prompts.rs` the approval primitives and the ratatui prompt functions |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/stream_ask.rs $D/stream_prompts.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read the two new files once. For each,
   check these and record every hit, or `none`:
   - a `lock().expect` or `lock().unwrap()` (CLAUDE.md requires
     `.unwrap_or_log()`);
   - any other `expect`, `unwrap` or `panic!` in production code;
   - an error that is swallowed (`let _ =`, `.ok()`, an empty `Err(_) =>`);
   - an obvious duplicate of code elsewhere in the file.

   For `ask_with_session_ratatui` (673 lines, a later split), also name its
   natural seams.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-22.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, and add no other "proof" block. The block is a `bash`
   heredoc, so run it as it stands. Check with `grep -c '^lost_other=0$'`
   on this doc before you commit; it must print 1. **Known flake:**
   `passed=2498 failed=1` on the full suite. If the block shows exactly
   that, run it once more and paste both outputs with a line saying so.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `stream_ask.rs` defines `ask_with_session_ratatui`, and
      `stream_prompts.rs` defines `prompt_tool_call_ratatui`,
      `prompt_edit_file_ratatui` and `read_approval_input`. `stream.rs`
      defines none of them, still defines `QueryArgs` and `recv_line`, and
      has at most 800 production lines (`cargo test --test hygiene`
      measures it).
- [ ] The allowlist has no `file src/cli/commands/stream.rs` line, and its
      `ask_with_session_ratatui 673` line names `stream_ask.rs`.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `ask_identical=1`, `prompts_identical=1` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=b8c6a006; D=src/cli/commands; F=$D/stream.rs
A=/tmp/e2e-m31-22.txt; : > "$A"
wc -l $F $D/stream_ask.rs $D/stream_prompts.rs | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/stream.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^fn src/cli/commands/stream_ask.rs ask_with_session_ratatui 673$' tests/hygiene_allowlist.txt | sed 's/^/fn_moved=/' >> "$A"
P='key_outcome|tool_runtime_label|build_approval_prompt|parse_approval_response|edit_file_auto_approved|edit_file_grants_session'
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-22-old.txt; cat $F $D/stream_ask.rs $D/stream_prompts.rs | norm > /tmp/m31-22-new.txt
echo "lost_other=$(comm -23 /tmp/m31-22-old.txt /tmp/m31-22-new.txt | grep -vcE "^(fn($P)\(|pub\(super\)asyncfnask_with_session_ratatui\()")" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-22-old.txt /tmp/m31-22-new.txt | grep -vcE "^(#\[path=|//|mod(ask|prompts);$|pub\(super\)useask::ask_with_session_ratatui;$|useprompts::\*;$|usesuper::\*;$|usesuper::approval;$|pub\(insuper::super\)asyncfnask_with_session_ratatui\(|pub\(super\)fn($P)\()")" >> "$A"
if diff -q <(git show $B:$F | sed -n 112,789p | sed 's/^pub(super) async fn ask_with_session_ratatui(/pub(in super::super) async fn ask_with_session_ratatui(/') <(tail -n +6 $D/stream_ask.rs) >/dev/null; then echo ask_identical=1; else echo ask_identical=0; fi >> "$A"
if diff -q <(git show $B:$F | sed -n 903,1515p | sed -E "s/^fn ($P)\(/pub(super) fn \1(/") <(tail -n +7 $D/stream_prompts.rs) >/dev/null; then echo prompts_identical=1; else echo prompts_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `841 …stream.rs`, `683 …stream_ask.rs`,
`619 …stream_prompts.rs`, `allowlisted=0`, `fn_moved=1`, `lost_other=0`,
`added_other=0`, `ask_identical=1`, `prompts_identical=1`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/stream.rs`; create `stream_ask.rs` and
  `stream_prompts.rs` beside it. Edit `tests/hygiene_allowlist.txt` and
  `CLAUDE.md` through the script only.

## Out of scope

- The context structs, the stream helpers and the three inline test
  modules.
- Any change inside the moved code beyond the seven visibility lines,
  including splitting `ask_with_session_ratatui` or rewriting its
  `super::` path.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 19:27 (cuts done)

Ran the Spec-1 script once with `bash`. Its output, verbatim:

```
A1=112 A2=789 B1=903 T=1517
  841 src/cli/commands/stream.rs
  683 src/cli/commands/stream_ask.rs
  619 src/cli/commands/stream_prompts.rs
 2143 total
```

`cargo fmt --all` ran clean inside the script. `cargo build` afterwards:
`Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.23s`, zero
warnings. Files touched: `src/cli/commands/stream.rs` (rewritten in place by
the script), new `src/cli/commands/stream_ask.rs` and
`src/cli/commands/stream_prompts.rs`, `tests/hygiene_allowlist.txt` (the
`file src/cli/commands/stream.rs 1520` line deleted, the
`fn … ask_with_session_ratatui 673` line repointed to `stream_ask.rs`),
`CLAUDE.md` (one row added after the `chat_loop.rs` row).

### Findings — 2026-10-27 19:30 (decision 7)

Read both new files in full. Findings:

- `lock().expect` / `lock().unwrap`: **none** in either file (no `lock()` at
  all; CLAUDE.md's `.unwrap_or_log()` rule is not triggered).
- Other `expect` / `unwrap` / `panic!` in production code: **one hit** —
  `stream_ask.rs:240` `_ => unreachable!()` in the `let msg = match outcome`
  match after the outer `match outcome` consumed `StreamOutcome::Msg`.
  Pre-existing (moved verbatim); it is an invariant assertion, not error
  suppression, but it is the one panic path in the moved code.
- Swallowed errors: the 41 `let _ = renderer.…` calls in `stream_prompts.rs`
  and 12 in `stream_ask.rs` (renderer draw/commit failures are ignored
  wholesale — a TUI redraw is best-effort); `stream_prompts.rs:59`
  `timeout_at(...).await.ok().flatten()` (the deadline→`None` contract, not a
  swallow); `stream_ask.rs:46,49` `std::env::var("TMUX_PANE").ok()` (env unset
  is a normal case); `stream_prompts.rs:436` `.ok()` on
  `trim().parse::<usize>()` (parse failure falls through to the default-pane
  fallback). All are intentional model-visible outcomes, not swallowed
  `Result`s the caller needs. One dead binding:
  `stream_prompts.rs:587` `let _path_approved = approval.file_edits.contains(path)`
  is computed and never used (`edit_file_auto_approved` recomputes it).
- Duplicates: **one obvious pair** — `read_approval_input` (line 75) and
  `read_approval_input_panel` (line 137) are the same 60-line state machine
  differing only in the redraw call (`draw_prompt` vs `draw_approval_panel`);
  a closure taking the redraw fn would collapse them. Same shape smaller-scale:
  the `StatusBarState { … }` literal is constructed verbatim nine times in
  `stream_ask.rs` with only `cost_usd`/`has_untracked`/`model` varying, and
  `prompt_tool_call_ratatui` vs `prompt_edit_file_ratatui` end in the same
  approved/redirect/skipped commit ladder.

Natural seams in `ask_with_session_ratatui` (673 lines, a later split):

1. **The `Response` match (~lines 250–595)** — the largest block; each
   `Response::…Prompt` arm is self-contained (build a `StatusBarState`, call a
   `prompt_*_ratatui`, send a `Request`). It wants an enum→handler dispatch,
   one handler per prompt kind, all taking a small turn-state struct
   (`renderer`, `stdin`, `tx`, `approval`, `transcript`, `session_cost`,
   `session_has_untracked`, `prompt_tokens`, `context_window`, `model`) that
   replaces the six destructured parameters.
2. **The spinner/`StatusBarState` animation** — the `Tick` / `Warn` /
   `KeepAlive` arms duplicate one spinner-draw block three times (verb,
   dot_period, dot_count, status bar); a `draw_spin(renderer, spin, sb, verb)`
   helper deletes ~60 lines.
3. **The outer event loop** (`select_stream` + `StreamOutcome` match, the
   deadline messages, the viewer detour) — once the Response arms move out,
   this is a compact ~150-line driver, which is the shape `run_conversation_loop`
   already has on the daemon side.

### Update — 2026-10-27 19:47 (end-to-end verification)

Ran the Spec-4 `bash` heredoc once. `/tmp/e2e-m31-22.txt`, pasted byte-exact:

```
841 src/cli/commands/stream.rs
683 src/cli/commands/stream_ask.rs
619 src/cli/commands/stream_prompts.rs
2143 total
allowlisted=0
fn_moved=1
lost_other=0
added_other=0
ask_identical=1
prompts_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

All acceptance criteria in the block met on the first run: `lost_other=0`,
`added_other=0`, `ask_identical=1`, `prompts_identical=1`, hygiene gate
`ok.` (16 passed), clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — the
expected full-suite count with no flake, so no re-run was needed. `wc -l`
matches the dry run exactly (841 / 683 / 619).

### Update — 2026-10-28 02:36 (complete, server-authored)

**Summary:** </think>The last grep for README row shows "0 (x2)"? Let me recheck — output shows "0 (x2)" is from `grep -c '(complete)'` printing 0 and `git status --short | wc -l` printing 0. The README row grep printed "in-progress". Good.

Completion checklist:
- Phase doc Status: in-progress ✓
- README row: in-progress ✓
- No (complete) entry ✓
- cargo fmt run before git add ✓
- All verification commands ran clean ✓
- git status clean ✓
- git log shows all files ✓

Now write the final Summary + Notes for review.</think>
All done. Final state:

**Summary:** Ran the Spec-1 script once with `bash` — both cuts landed exactly as expected (`A1=112 A2=789 B1=903 T=1517`), leaving `stream.rs` at 841 production lines with `ask_with_session_ratatui` (673 lines, allowlisted at its old count, now pointing at `stream_ask.rs`) in `stream_ask.rs` and the approval primitives/prompt functions in `stream_prompts.rs`; the context structs, stream helpers and three inline test modules stayed put. Logged the `(cuts done)` entry, a `### Findings — 2026-10-27` entry, and an `(end-to-end verification)` entry with `/tmp/e2e-m31-22.txt` pasted byte-exact. Committed as `refactor:` (63ccf8b0); working tree clean.

**Notes for review:**
- The End-to-end block passed on the **first run** with no flake: `lost_other=0`, `added_other=0`, `ask_identical=1`, `prompts_identical=1`, hygiene `ok.` (16 passed), clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — so the Spec-4 flake re-run clause never fired, and `grep -c '^lost_other=0$'` on the phase doc prints 1 as required.
- Findings (decision 7): no `lock().expect`/`lock().unwrap` in either file. One pre-existing `unreachable!()` in the moved code (`stream_ask.rs:240`) and the usual intentional `let _ =` on TUI redraws; also flagged one dead binding (`stream_prompts.rs:587` `_path_approved`) and one obvious duplicate pair (`read_approval_input` / `read_approval_input_panel`, identical 60-line state machines differing only in the redraw call). Natural seams for a later split of `ask_with_session_ratatui` are recorded: the ~340-line `Response` match (enum→handler dispatch over a turn-state struct), the thrice-duplicated spinner/`StatusBarState` block, and the outer event loop.
- No deviations from the spec; the script was used verbatim and no file it touched was hand-edited.

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
le_classes ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test functions_in_a_test_module_are_not_measured ... ok
test cfg_all_test_counts_as_test_only ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test functions_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.32s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (1823184): No such process
kill: (-1823883): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-22-stream-split.md` — +98 -1
- `src/cli/commands/stream.rs` — +111 -1394
- `src/cli/commands/stream_ask.rs` — +683 -0
- `src/cli/commands/stream_prompts.rs` — +619 -0
- `tests/hygiene_allowlist.txt` — +1 -2

**Commit:** 63ccf8b010c5251a33c2a56e9c877359b047dff0

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (78 turns)
- **Scope deviations:** none. `git diff --stat 64511de5 HEAD` names the
  seven authorized files. Both moved blocks equal their
  `64511de5:stream.rs` ranges after the seven visibility lines. The code
  commit is typed `refactor:`. Reviewer's gates: fmt 0, build and clippy
  clean, 2586 passed / 0 failed / 13 ignored. The flake did not fire.
- **Findings:** carried to `FINDINGS.md` #66–#69. The reviewer confirmed
  the two concrete claims:
  - `read_approval_input` and `read_approval_input_panel` differ only in
    the redraw call;
  - `let _path_approved = …` at `stream_prompts.rs:587` is computed and
    never used.

  The seams named for `ask_with_session_ratatui` are input to its
  function-split phase.
- **Calibration:** the third run under the checklist and the paste-order
  rule, and both held: the End-to-end block ran at turn 65 and was pasted
  at turn 68. The audit is the most useful of the milestone, with
  concrete, checkable claims and every one verified.

