# Phase 21 — Split `top.rs` into loaders, text frames and run detail

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/cli/commands/top.rs` (1394 production lines) drops under the 800-line
cap. Three of its banner-delimited sections move into child modules,
each glob re-exported:
- `// ── Loaders ──` and the fingerprint go to `top_load.rs`;
- `// ── Text frame ──` (panel texts, budget bar, header, `plan_layout`)
  goes to `top_text.rs`;
- `// ── Run detail ──` goes to `top_detail.rs`.

`top.rs` keeps the snapshot types, the Activity rows (with
`activity_row_in`), `run_top` and its tests, at about 620 production
lines. Every `crate::cli::commands::top::…` path `top_tui.rs` uses still
resolves. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: three cuts, the wiring, the allowlist line, the CLAUDE.md row,
   then `cargo fmt`. It was dry-run on a scratch worktree of `06ae5753`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Four lines change inside the moved code. `run_subject`, `state_word`,
   `sort_rows` and `format_age` gain `pub(super)`, because the parent, a
   sibling child or the tests call them. The glob re-exports
   (`pub use load::*;` and so on) re-export the public items and quietly
   limit the `pub(super)` ones to `top`. The moved code reaches the sibling
   commands `ipc_client` and `scorecard` as `super::…`, so the script
   imports both into `top.rs`. **Do not rewrite those paths to
   `super::super::…`.**

   **Why `activity_row_in` stays in `top.rs`.** It was the first choice to
   move, but its signature is 99 columns long. `pub(super)` (the tests call
   it) makes rustfmt wrap it, which grows the function past its
   allowlisted 163 lines, and the hygiene gate fails. The first dry run hit
   exactly that.

   ```bash
   D=src/cli/commands; F=$D/top.rs
   L1=$(grep -n '^// ── Loaders ──' $F | cut -d: -f1)
   X1=$(grep -n '^// ── Text frame ──' $F | cut -d: -f1)
   X2=$(( $(grep -n '^/// `daemoneye top \[--once\]' $F | cut -d: -f1) - 2 ))
   R1=$(grep -n '^// ── Run detail ──' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "L1=$L1 X1=$X1 X2=$X2 R1=$R1 T=$T"     # expect 553 784 1105 1172 1395
   vis() { sed -e 's/^fn \(run_subject\|state_word\|sort_rows\|format_age\)(/pub(super) fn \1(/'; }
   hdr() { printf '//! %s\n\nuse super::*;\n\n' "$1"; }
   { hdr '`top`'"'"'s loaders and change fingerprint: the Runs, Budget, Shells and header reads, `take_snapshot`, `fingerprint`.'
     sed -n "${L1},$((X1-2))p" $F | vis; } > $D/top_load.rs
   { hdr '`top`'"'"'s text frames and layout: the panel texts, the budget bar, the header and `plan_layout`.'
     sed -n "${X1},${X2}p" $F | vis; } > $D/top_text.rs
   { hdr 'The run detail view: `load_run_detail` and `run_detail_text`.'
     sed -n "${R1},$((T-2))p" $F | vis; } > $D/top_detail.rs
   { head -n $((L1-1)) $F
     printf '#[path = "top_load.rs"]\nmod load;\npub use load::*;\n#[path = "top_text.rs"]\nmod text_frame;\npub use text_frame::*;\n// The children reach these sibling commands as `super::…`, through this import.\nuse super::{ipc_client, scorecard};\n\n'
     sed -n "$((X2+2)),$((R1-1))p" $F
     printf '#[path = "top_detail.rs"]\nmod detail;\npub use detail::*;\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/top.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/top_tui_draw.rs` \|/ {print "| `src/cli/commands/top_load.rs`, `top_text.rs`, `top_detail.rs` | `daemoneye top` split by concern, child modules of `src/cli/commands/top.rs` (the snapshot types, the Activity rows and `run_top`) and glob re-exported from it: `top_load.rs` the loaders and `fingerprint`, `top_text.rs` the panel texts and `plan_layout`, `top_detail.rs` the run detail view |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/top_load.rs $D/top_text.rs $D/top_detail.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read the three new files once. For each,
   check these and record every hit, or `none`:
   - a `lock().expect` or `lock().unwrap()` (CLAUDE.md requires
     `.unwrap_or_log()`);
   - any other `expect`, `unwrap` or `panic!` in production code;
   - an error that is swallowed (`let _ =`, `.ok()`, an empty `Err(_) =>`);
   - an obvious duplicate of code elsewhere in `top`.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-21.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, never before. Do not add any other "proof" block; the
   End-to-end output is the evidence. The block is a `bash` heredoc, so
   run it as it stands. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1. **Known flake:** the full suite
   occasionally reports `passed=2498 failed=1`. If the block shows exactly
   that, run it once more and paste both outputs with a line saying so.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `top_load.rs` defines `take_snapshot` and `fingerprint`, `top_text.rs`
      defines `budget_text` and `plan_layout`, and `top_detail.rs` defines
      `load_run_detail` and `run_detail_text`. `top.rs` defines none of
      them, still defines `activity_row_in` and `run_top`, and has at most
      800 production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/cli/commands/top.rs` line. Its
      `fn src/cli/commands/top.rs activity_row_in 163` line stays.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `load_identical=1`, `text_identical=1`, `detail_identical=1` and
      `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=06ae5753; D=src/cli/commands; F=$D/top.rs
A=/tmp/e2e-m31-21.txt; : > "$A"
wc -l $F $D/top_load.rs $D/top_text.rs $D/top_detail.rs | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/top.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^fn src/cli/commands/top.rs activity_row_in 163$' tests/hygiene_allowlist.txt | sed 's/^/fn_kept=/' >> "$A"
vis() { sed -e 's/^fn \(run_subject\|state_word\|sort_rows\|format_age\)(/pub(super) fn \1(/'; }
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-21-old.txt; cat $F $D/top_load.rs $D/top_text.rs $D/top_detail.rs | norm > /tmp/m31-21-new.txt
echo "lost_other=$(comm -23 /tmp/m31-21-old.txt /tmp/m31-21-new.txt | grep -vcE '^fn(format_age|run_subject|sort_rows|state_word)\(')" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-21-old.txt /tmp/m31-21-new.txt | grep -vcE '^(#\[path=|//|mod(load|text_frame|detail);$|pubuse(load|text_frame|detail)::\*;$|usesuper::\*;$|usesuper::\{ipc_client,scorecard\};$|pub\(super\)fn(format_age|run_subject|sort_rows|state_word)\()')" >> "$A"
for p in "load 553 782" "text 784 1105" "detail 1172 1393"; do set -- $p
  if diff -q <(git show $B:$F | sed -n "$2,$3p" | vis) <(tail -n +5 $D/top_$1.rs) >/dev/null; then echo "$1_identical=1"; else echo "$1_identical=0"; fi >> "$A"; done
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `1877 …top.rs`, `234 …top_load.rs`,
`326 …top_text.rs`, `226 …top_detail.rs`, `allowlisted=0`, `fn_kept=1`,
`lost_other=0`, `added_other=0`, `load_identical=1`, `text_identical=1`,
`detail_identical=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/top.rs`; create `top_load.rs`, `top_text.rs` and
  `top_detail.rs` beside it. Edit `tests/hygiene_allowlist.txt` and
  `CLAUDE.md` through the script only.

## Out of scope

- `activity_row_in` (163 lines, a later function split), the snapshot
  types, `run_top`, the inline tests.
- Any change inside the moved code beyond the four `pub(super)` lines,
  including rewriting its `super::` paths.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 18:49 (cuts done)

Ran the Spec 1 script once with `bash`, from a clean tree at `6883e259`
(`06ae5753`'s successor). The four banner lines landed where the spec's
echo line predicted — `L1=553 X1=784 X2=1105 R1=1172 T=1395` — and the
script's tail:

```
 1877 src/cli/commands/top.rs
  234 src/cli/commands/top_load.rs
  326 src/cli/commands/top_text.rs
  226 src/cli/commands/top_detail.rs
 2663 total
```

Line counts match the dry run's expectations exactly (`1877 …top.rs`,
`234 …top_load.rs`, `326 …top_text.rs`, `226 …top_detail.rs`).
`cargo fmt --all` inside the script exited 0. `git status` after:
`CLAUDE.md` and `tests/hygiene_allowlist.txt` modified,
`top_load.rs` / `top_text.rs` / `top_detail.rs` untracked — nothing else
touched.

### Findings — 2026-10-27 18:57

Read all three new files in full (`top_load.rs` 234, `top_text.rs` 326,
`top_detail.rs` 226).

- **`lock().expect` / `lock().unwrap` / `.unwrap_or_log()`:** `none` — the
  three files touch no mutex, RwLock or OnceLock.
- **Other `expect` / `unwrap` / `panic!` in production code:** `none`.
- **Swallowed errors:** three kinds, all pre-existing moves, all of them
  the file-read pattern "a panel row that cannot be read is a blank or a
  zero, not a crash":
  - `top_load.rs:21` `.ok()` on the run-log mtime (a run with no log
    mtime loses only its liveness freshness);
  - `top_load.rs:82` and `top_detail.rs:46,62,90` `.ok()` on
    `DateTime::parse_from_rfc3339` for log timestamps (an unparseable
    stamp renders `--:--:--` or `0s`, the choice the panels already
    made);
  - `top_load.rs:195-196` `.ok()`s and `top_load.rs:200`
    `Err(_) => (0, 0)` in `file_stats`, and the `Err(_) => (0, 0)` /
    `let Ok(..) else` returns in `runs_dir_stats` (a missing directory
    or file fingerprints as zeros — the fingerprint's contract, "stats
    only, never file contents").
  No new swallowing; every hit was verbatim inside the moved text.
- **Obvious duplicate of code elsewhere in `top`:** one, pre-existing —
  `top_load.rs:75` `run_started_at` and `top_detail.rs:40`
  `started_at_of` are byte-identical bodies (find the first
  `RunStarted` record, parse its timestamp) with different names. They
  were separate in `top.rs` before the cut; the move preserves that.
  Recording it, not fixing it — out of scope per the phase doc.

All findings are moves, not introductions: the diff `added_other=0`
check in the End-to-end block below proves nothing else changed.

### Update — 2026-10-27 18:56 (end-to-end verification)

Ran the Spec 4 End-to-end block once (`bash` heredoc, as written); its
output, written by the block itself to `/tmp/e2e-m31-21.txt` and pasted
byte-exact:

```
1877 src/cli/commands/top.rs
234 src/cli/commands/top_load.rs
326 src/cli/commands/top_text.rs
226 src/cli/commands/top_detail.rs
2663 total
allowlisted=0
fn_kept=1
lost_other=0
added_other=0
load_identical=1
text_identical=1
detail_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every expected value from the dry run reproduced: line counts, the
allowlist line gone (`allowlisted=0`) with `activity_row_in 163` kept
(`fn_kept=1`), nothing lost or added beyond the four `pub(super)` lines
and the wiring (`lost_other=0`, `added_other=0`), all three moved
sections byte-identical to their source ranges (`load_identical=1`,
`text_identical=1`, `detail_identical=1`), the hygiene test green,
clippy clean, fmt clean, and the full suite `passed=2586 failed=0`. No
flake: the first run landed on 2586/0.

### Update — 2026-10-28 01:59 (complete, server-authored)

**Summary:** </think>Everything is done. Let me run through the completion checklist:

- [x] Phase doc's Status: line says `in-progress` ✓
- [x] Milestone README's phase-table row says `in-progress` ✓
- [x] No hand-written `(complete)` entry ✓ (only cuts done, Findings, end-to-end verification)
- [x] `cargo fmt --all` was run (inside the script, and `--check` confirms clean) ✓
- [x] All verification commands ran clean ✓
- [x] Final message is substantive Summary + Notes for review — this message
- [x] `git status --short` shows nothing ✓
- [x] `git log -1 --stat` shows commits include every touched file ✓

Acceptance criteria:
- [x] top_load.rs defines take_snapshot and fingerprint ✓ (verified in read)
- [x] top_text.rs defines budget_text and plan_layout ✓
- [x] top_detail.rs defines load_run_detail and run_detail_text ✓
- [x] top.rs defines none of them, still defines activity_row_in and run_top ✓
- [x] at most 800 production lines — hygiene test passed ✓
- [x] Allowlist has no `file src/cli/commands/top.rs` line, `fn ... activity_row_in 163` stays ✓
- [x] E2E block prints all expected values ✓
- [x] Update Log has all three entries, all written with edit tools, all in this file ✓

Grep for spec-pinned literals: `lost_other=0` grep printed 1 ✓.

Now the final summary.</think>
All work is committed and the tree is clean. Completion checklist verified: phase doc and README both read `in-progress`, no `(complete)` entry written, `grep -c '^lost_other=0$'` on the phase doc prints 1, and both commits (`b4c99468` refactor, `daf89b3f` docs) contain every file touched.

**Summary**

Ran the Spec 1 script once, exactly as written, from a clean tree at `6883e259`: the four banner markers landed on the predicted lines (`L1=553 X1=784 X2=1105 R1=1172 T=1395`), the three cuts produced `top_load.rs` (234), `top_text.rs` (326) and `top_detail.rs` (226) beside `top.rs` (1877 total, ≈620 production), each with a module doc, `use super::*`, and a glob re-export wired into `top.rs` along with the `use super::{ipc_client, scorecard}` import — no `super::` path inside the moved code was rewritten. The allowlist lost its `file src/cli/commands/top.rs 1394` line and kept `fn … activity_row_in 163`; `CLAUDE.md` gained the new module-table row. Logged the `(cuts done)` entry, then read all three files for Findings (Spec 3): no `lock().expect`/`unwrap`, no `expect`/`unwrap`/`panic!`, no *new* swallowed errors (all `.ok()`/`Err(_)=>…` hits are verbatim moves of the panels' "unreadable = blank or zero" file-read pattern), and one pre-existing duplicate — `run_started_at` (top_load) vs `started_at_of` (top_detail), byte-identical bodies that were already separate in old `top.rs`; recorded, not fixed, per Out of scope. Then ran the End-to-end block once and pasted its output byte-exact into an `(end-to-end verification)` entry: `lost_other=0`, `added_other=0`, `load_identical=1`, `text_identical=1`, `detail_identical=1`, `allowlisted=0`, `fn_kept=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — the full suite reproduced the dry run's numbers on the first run, no flake. Committed the code as `refactor:` and the doc bookkeeping as `docs:`; `git status` is clean.

**Notes for review**

- No deviations from the spec: the script was used verbatim, nothing hand-edited that it touches, and the E2E diff checks (`lost_other=0`, `added_other=0`, all three `*_identical=1`) prove the move is byte-exact apart from the four authorized `pub(super)` lines and the wiring.
- `run_tui`'s call sites in `top.rs` reach `top_tui` by its full `crate::cli::commands::top_tui::` path, so no import change was needed there; `top_tui.rs`'s `use crate::cli::commands::top::{…}` resolves through the glob re-exports unchanged (build and clippy both clean with `-D warnings`).
- The `run_started_at`/`started_at_of` duplication is the only candidate follow-up surfaced by decision 7's findings sweep; it predates this phase and is left for a later hygiene pass.
- Status left at `in-progress` per the contract — the server authors the `in-progress → review` flip and the completion entry.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
st_only_fn_is_still_counted ... ok
test test_file_classes ... ok
test production_lines_exclude_an_inline_test_module ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test allowlist_has_no_stale_entries ... ok
test exempt_files_exist ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.29s


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
test ipc_tool_call_response_round_trip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test minimal_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
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
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok

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
kill: (1763633): No such process
kill: (-1764334): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-21-top-split.md` — +90 -1
- `src/cli/commands/top.rs` — +11 -775
- `src/cli/commands/top_detail.rs` — +226 -0
- `src/cli/commands/top_load.rs` — +234 -0
- `src/cli/commands/top_text.rs` — +326 -0
- `tests/hygiene_allowlist.txt` — +0 -1

**Commit:** daf89b3f7148afa7072b0c6b9a1f0a32e6c3cbdf

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
