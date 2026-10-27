# Phase 10 — Move the tail and log viewers out of `tail.rs`

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/tail.rs` (988 production lines) drops under the 800-line cap by
moving its viewer section, `open_tail` through `color`, into a new
`src/cli/tail_view.rs` declared as a child module of `tail`. The section
covers opening a model from the first frame, `run_log_viewer`,
`run_tail_viewer`, `tail_loop`, the two status-line builders and
`render_tail`. The child is re-exported, so `crate::cli::tail::open_tail`,
`render_tail`, `run_log_viewer` and `run_tail_viewer` (used by
`cli/attach.rs` and `cli/commands/chat.rs`) still resolve. Pure move:
`TailModel`, its impls and the key-action functions stay in `tail.rs`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `b39b5aff`: fmt (a no-op), clippy and the full suite (2586)
   green. **No `sed -i` on source files** (the executor refuses in-place
   edits): the rewrite goes through a temp file and `mv`. Nothing inside
   the moved code changes; every function that crosses the boundary is
   already `pub`.

   ```bash
   F=src/cli/tail.rs; N=src/cli/tail_view.rs
   S=$(grep -n '^/// The model a tail starts from, given ' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 607 989
   { printf '//! The tail and log viewers: opening a model from the first frame, the\n//! viewer loops, and drawing the screen and its status line.\n\nuse super::*;\n\n'
     sed -n "${S},$((T-2))p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "tail_view.rs"]\nmod view;\npub use view::{open_log, open_tail, render_tail, run_log_viewer, run_tail_viewer, tail_loop};\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. `cargo fmt --all`. No import pruning: the build has no warnings.
4. **Allowlist:** delete the line `file src/cli/tail.rs 988`. No `fn`
   entry names `tail.rs`.
5. **CLAUDE.md** "Key files": add a row for `src/cli/tail_view.rs`: the
   tail and log viewers (`open_tail`, `open_log`, `run_tail_viewer`,
   `run_log_viewer`, `tail_loop`, `render_tail`), a child module of
   `cli/tail.rs`, re-exported from it.
6. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.
7. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding the contents of `/tmp/e2e-m31-10.txt` in
   a fenced block. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1.

**Work in this checkout.** The spec is already dry-run, so do not repeat
it in a scratch worktree of your own. Phase 09 did, and it cost turns and
added no evidence.

## Acceptance criteria

- [ ] `src/cli/tail_view.rs` defines `open_tail`, `open_log`,
      `run_log_viewer`, `run_tail_viewer`, `tail_loop` and
      `render_tail`. `tail.rs` defines none of them and has at most 800
      production lines (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/cli/tail.rs ' tests/hygiene_allowlist.txt` is 0
      and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 7).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=b39b5aff; F=src/cli/tail.rs; N=src/cli/tail_view.rs
A=/tmp/e2e-m31-10.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/cli/tail.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-10-old.txt; cat $F $N | norm > /tmp/m31-10-new.txt
echo "lost_other=$(comm -23 /tmp/m31-10-old.txt /tmp/m31-10-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-10-old.txt /tmp/m31-10-new.txt | grep -vE '^(usesuper::\*;|pubuseview::|//!|#\[path=|modview;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `1442 …tail.rs`, `386 …tail_view.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, hygiene `ok.`, clippy
clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/tail.rs`; create `src/cli/tail_view.rs`. Edit
  `tests/hygiene_allowlist.txt` (one line) and `CLAUDE.md` (one row).

## Out of scope

- `tail.rs`'s inline `mod tests` (stays inline, under the 2000-line cap),
  `TailModel` and the key-action functions.
- Any change inside the moved functions.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 02:05 (cuts done)

Ran the Spec 1 pipeline as written in this checkout (no scratch worktree —
the spec is already dry-run; see "Work in this checkout"). `S=607 T=989`
printed exactly as the spec expects. The cut produced `src/cli/tail_view.rs`
(386 lines) and left `src/cli/tail.rs` at 1442 total lines. The diff against
`b39b5aff` is a pure move: 381 lines left `tail.rs`, all of them reappearing
in `tail_view.rs`; the only new lines are the module wiring
(`#[path = "tail_view.rs"] mod view;` and the one-line
`pub use view::{…}`) plus the new file's two doc-comment lines and
`use super::*;`. Nothing inside the moved code changed; every function that
crossed the boundary (`open_tail`, `open_log`, `run_log_viewer`,
`run_tail_viewer`, `tail_loop`, `render_tail`) was already `pub`, so no
visibility edit was needed. The `color`, `attached_status` and
`replay_status` helpers stay private inside `tail_view.rs`, which uses them;
`tail.rs`'s inline tests reach the re-exported functions through the `pub
use` and their existing `use super::*;`.

### Findings — 2026-10-27

Read the whole moved section once (`src/cli/tail_view.rs`, 386 lines:
`open_tail`, `open_log`, `run_log_viewer`, `run_tail_viewer`, `tail_loop`,
`attached_status`, `replay_status`, `render_tail`, `color`). Findings, none
acted on per decision 6:

- **`tail_view.rs` · `run_log_viewer`/`run_tail_viewer` — smell, minor.**
  The two viewer entry points duplicate eleven lines of alternate-screen
  plumbing: `EnterAlternateScreen`, the `AltScreenGuard` teardown closure
  (`LeaveAlternateScreen`, `Show`, `renderer.reanchor()`), the
  `ratatui::Terminal::new(CrosstermBackend::new(stdout))` and the two
  `Box::pin(futures_util::stream::unfold(…))` key/resize streams, all
  identical in the two functions — and `attach.rs`'s `run_attach` repeats
  the same four pieces a third time. A shared
  `enter_alt_screen(renderer)` helper would leave each entry point with its
  stream setup only. Not acted on: three call sites is the
  abstract-when-the-fourth-appears boundary, and a helper returning three
  borrowed streams is awkward.
- **`tail_view.rs` · `run_log_viewer` — smell, nit.** Its frame source is
  `tokio::io::BufReader::new(tokio::io::empty()).lines()` — an empty reader
  type-swizzled into `tail_loop`'s `tokio::io::Lines<R>` parameter so a log
  (which has no stream) can share the loop. It works, but the real contract
  ("no frame source") is expressed as a fake source that can never yield; a
  `None`-able frames parameter would say it. Confined to this one call.
- **`tail_view.rs` · `tail_loop` — smell, nit.** The replay branch resets
  `last` after every key, so a replay's clock advances only between keys —
  the in-code comment says so and the skip affordance matches; but nothing
  in `replay_status` tells a user that a paused replay needs a key to keep
  counting time. Display choice, not a bug.
- **`tail_view.rs` · `render_tail` — smell, nit.** The status `match`'s
  guard ordering is load-bearing and easy to disturb: `model.attached`
  wins first, then `replay.is_some()`, then `model.log`, then the
  search/note arms. An attached replay shows the attached status line,
  which is right, but nothing guards the order; a match over a small
  ordered enum would.
- **`tail_view.rs` · `color` — smell, nit.** Two match arms per vt100
  colour where `Idx(n) → Color::Indexed(n)` plus a 16-entry name table
  would read shorter; the explicit arms make the theme mapping visible at
  a glance, which is the stated why. Keep.

No `unwrap`/`expect`/`panic` in the moved production code; the two
`map_err(|msg| anyhow::anyhow!(msg))` calls are the documented
`Result<TailModel, String>` → `anyhow` boundary at the viewer's edge. No
commented-out code, no behaviour change.

### Update — 2026-10-27 (end-to-end verification)

Ran the phase's End-to-end block with `bash` in this checkout; output below,
captured mechanically (redirected to `/tmp/e2e-m31-10.txt` and pasted
byte-exact):

```text
1442 src/cli/tail.rs
386 src/cli/tail_view.rs
1828 total
allowlisted=0
lost_other=0
added_other=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

### Update — 2026-10-27 02:18 (complete, server-authored)

**Summary:** </think>Everything looks complete. Let me verify the completion checklist:

1. Phase doc's Status: line says `in-progress` ✓
2. Milestone README's phase-table row says `in-progress` ✓
3. No hand-written `(complete)` entry ✓
4. `cargo fmt --all` was run immediately before `git add` ✓
5. All verification commands ran clean ✓ (fmt=0, build clean, clippy clean, test 2586/0, hygiene 16 passed)
6. Final message will be substantive Summary + Notes for review
7. `git status --short` shows nothing ✓
8. `git log -1 --stat` shows the commit includes every file touched ✓ (6 files: CLAUDE.md, README, phase doc, tail.rs, tail_view.rs, hygiene_allowlist.txt)

Acceptance criteria:
- [x] `src/cli/tail_view.rs` defines open_tail, open_log, run_log_viewer, run_tail_viewer, tail_loop, render_tail. tail.rs defines none and has at most 800 production lines (606, hygiene measures it) ✓
- [x] grep -c '^file src/cli/tail.rs ' → 0, hygiene passes ✓
- [x] E2E prints lost_other=0 and added_other=0 ✓
- [x] cargo test passes with exactly 2586 passing ✓
- [x] Update Log has (cuts done), Findings, (end-to-end verification) entries, all written by edit tools ✓

Also NEXT.md says the phase is `todo` — but NEXT.md is maintained by the architect, and I'm not authorized to edit it. The milestone README row is what the contract requires. NEXT.md is the architect's file — I should leave it alone (the phase doc's Authorizations only name tail.rs, tail_view.rs, hygiene_allowlist.txt, CLAUDE.md — and the README row flip is part of the contract's step 1). NEXT.md's "(`todo`)" mention is stale but it's not in my authorization; the server/architect handles it. I'll note it in Notes for review.

Wait — should I check that the phase doc's Update Log grep for `lost_other=0` is 1 — yes, verified (prints 1).

One more check — the spec's task list has tasks 1, 2, 4, 5, 6 (no task 3 or 7 in the task panel; task 7 in the spec is the E2E capture). Let me check the task panel: "- [pending] 1 — Cut with the shell...", "2 — Log the cut at once", "4 — Allowlist:", "5 — CLAUDE.md", "6 — Findings". Tasks 1, 2, 4, 5, 6 are all done now. Task 3 (cargo fmt) and task 7 (E2E paste) weren't seeded as tracked tasks but are done in the work. Good.

Everything is complete. Time to write the final Summary + Notes for review.</think>
All work is complete and committed. Final verification of the checklist: phase doc at `in-progress`, README row matches, `lost_other=0` pasted once (`grep -c` = 1), PASTE MATCH confirmed against `/tmp/e2e-m31-10.txt`, working tree clean.

**Summary:** Executed M31 phase 10 as a pure move: ran the Spec 1 shell cut as written in this checkout (`S=607 T=989`, matching the spec's expected markers exactly), which moved the viewer section — `open_tail`, `open_log`, `run_log_viewer`, `run_tail_viewer`, `tail_loop`, `attached_status`, `replay_status`, `render_tail`, `color` (381 lines) — byte-identical into the new child module `src/cli/tail_view.rs` (386 lines) via temp-file redirect and `mv`, leaving `tail.rs` at 1442 total lines (606 production, measured by the hygiene gate) wired as `#[path = "tail_view.rs"] mod view;` with a one-line re-export so every `crate::cli::tail::` path used by `attach.rs` and `commands/chat.rs` still resolves. Deleted the `file src/cli/tail.rs 988` allowlist line (`grep -c` → 0, no `fn` entry names tail.rs), added the `tail_view.rs` row to CLAUDE.md's Key files, flipped the phase doc and README row to `in-progress`, and wrote all three required Update Log entries with edit tools: `(cuts done)`, `### Findings`, and `(end-to-end verification)` carrying the E2E block's output captured mechanically to `/tmp/e2e-m31-10.txt` and pasted byte-exact (`lost_other=0`, `added_other=0`, hygiene `ok. 16 passed; 0 failed`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — the exact preservation count).

**Notes for review:** (1) No deviations from the spec — the pipeline produced the dry run's expected numbers (`1442 tail.rs`, `386 tail_view.rs`, `1828 total`) on the first application, and the E2E `norm`-diff confirms the only lines lost or added are the moved block and the authorized wiring. (2) The first `cargo test --test hygiene` run failed exactly as the allowlist design intends — `allowlist_has_no_stale_entries` reporting tail.rs "now 610 lines, within the cap: remove it" — before I deleted the line; the phase's five tracked tasks plus the fmt and E2E steps are all done. (3) Findings logged, none acted on per decision 6: the two viewer entry points duplicate eleven lines of alt-screen plumbing that `attach.rs`'s `run_attach` repeats a third time (a candidate for a shared helper when a fourth caller appears), `run_log_viewer` fakes its "no frame source" contract with an empty reader to fit `tail_loop`'s `Lines` parameter, `render_tail`'s status-match guard ordering is load-bearing but unguarded, and two nits on the replay-clock reset and `color`'s arm shape. (4) `docs/dev/NEXT.md` still names phase 10 as `todo` — architect-maintained, outside my authorization, left alone. (5) Status left at `in-progress` with no hand-written `(complete)` entry, per the lifecycle split; the server authors the completion tail. Committed as `e41e5ba9` (`refactor:`, 6 files, +475 −384).

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
 ok
test production_lines_exclude_an_inline_test_module ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.30s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test ghost_config_parsing ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test ipc_ask_round_trip ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test event_log_append_read ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
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
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.32s


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
kill: (1092378): No such process
kill: (-1093087): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-10-tail-view.md` — +84 -1
- `src/cli/tail.rs` — +3 -381
- `src/cli/tail_view.rs` — +386 -0
- `tests/hygiene_allowlist.txt` — +0 -1

**Commit:** e41e5ba99b16503889441e5e761c06cc6b36da21

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
