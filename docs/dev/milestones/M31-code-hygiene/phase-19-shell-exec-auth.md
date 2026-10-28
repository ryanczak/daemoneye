# Phase 19 — Move credentials and connecting out of `shell_exec.rs`

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/executor/shell_exec.rs` (1129 production lines) drops under the
800-line cap by moving one contiguous section into a new
`src/daemon/executor/shell_exec_auth.rs`, declared as a child module of
`shell_exec`. The section runs from `SudoAttempts` through
`run_sudo_command`: the sudo retry bookkeeping, the credential prompt round
trip, the cold-host connect step (`drive_connect`, `ensure_connected`) and
the sudo run. `shell_exec` re-exports `ensure_connected`, so the three
executor files that call `shell_exec::ensure_connected` are unchanged. The
file's two inline test modules (3091 lines) are a later phase.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: the cut, the wiring, the allowlist line and the CLAUDE.md row,
   then `cargo fmt`. It was dry-run on a scratch worktree of `b808a710`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Two lines change inside the moved code, and the move requires both:
   - `ensure_connected` becomes `pub(in super::super)`: the executor
     module, its exact old reach. A child's `pub(super)` reaches only
     `shell_exec`, too narrow for the re-export.
   - `run_sudo_command` becomes `pub(super)`, because the parent's
     `run_in_shell_core` calls it.

   The wiring has two parts that look odd but are each forced.
   - **`use auth::*;`, not a named or `#[cfg(test)]` import.** It brings
     in `run_sudo_command` and, for the tests, `drive_connect`. A separate
     `#[cfg(test)] use auth::drive_connect;` counts as test lines and grows
     the file's allowlisted `test … 3091` entry, which the hygiene gate
     forbids. The first dry run hit exactly that.
   - **`use super::USER_PROMPT_TIMEOUT;`.** The moved code reaches that
     executor-level const as `super::USER_PROMPT_TIMEOUT`. From the child,
     `super` is `shell_exec`, so the parent imports it and the path resolves
     unchanged. **Do not rewrite it to `super::super::…`.**

   ```bash
   D=src/daemon/executor; F=$D/shell_exec.rs; N=$D/shell_exec_auth.rs
   S=$(grep -n '^/// Prompt round-trip bookkeeping for one sudo run' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// Record one completed command in the shell' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 655 1013
   { printf '//! Credentials and connecting: the sudo retry bookkeeping, the credential\n//! prompt round trip, the cold-host connect step (`drive_connect`,\n//! `ensure_connected`) and `run_sudo_command`.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F | sed -e 's/^pub(super) async fn ensure_connected</pub(in super::super) async fn ensure_connected</' \
                                  -e 's/^async fn run_sudo_command</pub(super) async fn run_sudo_command</'; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "shell_exec_auth.rs"]\nmod auth;\n// The glob brings in `run_sudo_command` and, for the tests, `drive_connect`.\nuse auth::*;\npub(super) use auth::ensure_connected;\n// `auth` reaches this as `super::USER_PROMPT_TIMEOUT`, through this import.\nuse super::USER_PROMPT_TIMEOUT;\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/daemon/executor/shell_exec.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/daemon\/executor\/shell_exec.rs` \|/ {print "| `src/daemon/executor/shell_exec_auth.rs` | Credentials and connecting for `run_terminal_command`: the sudo retry bookkeeping, the credential prompt round trip, the cold-host connect step (`drive_connect`, `ensure_connected`) and `run_sudo_command`; a child module of `src/daemon/executor/shell_exec.rs`, which re-exports `ensure_connected` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `shell_exec_auth.rs` once and record
   what you see, or `none`.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-19.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^moved_identical=1$'` on this doc before you commit; it must
   print 1. **Known flake:** the full suite occasionally reports
   `passed=2498 failed=1`, once in phase 05 and once in this phase's
   validation run. A re-run is clean. If the block shows exactly that, run
   it once more, and paste both outputs with a line saying so. Any other
   failure is real.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `shell_exec_auth.rs` defines `drive_connect`, `ensure_connected` and
      `run_sudo_command`. `shell_exec.rs` defines none of them and has at
      most 800 production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/daemon/executor/shell_exec.rs` line.
      Its `test src/daemon/executor/shell_exec.rs 3091` line stays
      unchanged.
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
B=b808a710; D=src/daemon/executor; F=$D/shell_exec.rs; N=$D/shell_exec_auth.rs
A=/tmp/e2e-m31-19.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/daemon/executor/shell_exec.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^test src/daemon/executor/shell_exec.rs 3091$' tests/hygiene_allowlist.txt | sed 's/^/test_entry_kept=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-19-old.txt; cat $F $N | norm > /tmp/m31-19-new.txt
echo "lost_other=$(comm -23 /tmp/m31-19-old.txt /tmp/m31-19-new.txt | grep -vcE "^(asyncfnrun_sudo_command<|pub\(super\)asyncfnensure_connected<)")" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-19-old.txt /tmp/m31-19-new.txt | grep -vcE '^(#\[path=|//|modauth;$|useauth::\*;$|pub\(super\)useauth::ensure_connected;$|usesuper::\*;$|usesuper::USER_PROMPT_TIMEOUT;$|pub\(insuper::super\)asyncfnensure_connected<|pub\(super\)asyncfnrun_sudo_command<)')" >> "$A"
if diff -q <(git show $B:$F | sed -n 655,1013p | sed -e 's/^pub(super) async fn ensure_connected</pub(in super::super) async fn ensure_connected</' -e 's/^async fn run_sudo_command</pub(super) async fn run_sudo_command</') <(tail -n +7 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `3868 …shell_exec.rs`,
`365 …shell_exec_auth.rs`, `allowlisted=0`, `test_entry_kept=1`,
`lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/shell_exec.rs`; create `shell_exec_auth.rs`
  beside it. Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md` through the
  script only.

## Out of scope

- Both inline test modules (`tests`, `connect_tests`) and
  `run_in_shell_core` (250 lines, a later function split).
- Any change inside the moved code beyond the two visibility lines,
  including rewriting its `super::` path.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 17:40 (started)

**Executor:** executor-19. Flipped Status to `in-progress` (here and in the
milestone README's phase table). Next: run the Spec 1 script once with `bash`.
### Update — 2026-10-27 17:41 (cuts done)

Ran the Spec 1 script once with `bash`, as written. It performed the cut
(lines 655–1013 of `shell_exec.rs` → new `shell_exec_auth.rs`), the two
visibility rewrites inside the moved code, the child-module wiring
(`#[path]` mod, glob `use auth::*;`, `pub(super) use auth::ensure_connected;`,
`use super::USER_PROMPT_TIMEOUT;`), the allowlist `file` line removal, the
CLAUDE.md table row, and `cargo fmt --all`.

Script output, quoted verbatim:

```
S=655 E=1013
  3868 src/daemon/executor/shell_exec.rs
   365 src/daemon/executor/shell_exec_auth.rs
  4233 total
```

`S=655 E=1013` matches the comment's expectation. Line counts match the dry
run (`3868` / `365`). `cargo fmt --all` inside the script exited 0.

### Findings — 2026-10-27 17:41

Read `src/daemon/executor/shell_exec_auth.rs` once (365 lines) after the cut:

- The file is a clean child module: module doc, `use super::*;`, then the
  moved section — `SudoAttempts`, the credential prompt round trip,
  `drive_connect`, `ensure_connected` (now `pub(in super::super)`),
  `run_sudo_command` (now `pub(super)`) — and nothing else.
- The two rewritten visibility lines landed exactly as the script's `sed`
  expressions intended: `pub(in super::super) async fn ensure_connected<`
  and `pub(super) async fn run_sudo_command<`.
- The moved code reaches `USER_PROMPT_TIMEOUT` as `super::USER_PROMPT_TIMEOUT`;
  from the child, `super` is `shell_exec`, which imports the const, so the
  path resolves unchanged — no `super::super::` rewrite was needed or made.
- Findings beyond that: none.
### Update — 2026-10-27 17:46 (end-to-end verification)

Ran the Spec End-to-end block once with `bash` (a heredoc, as it stands) and
captured `/tmp/e2e-m31-19.txt`. Its contents, pasted byte-exact:

```
3868 src/daemon/executor/shell_exec.rs
365 src/daemon/executor/shell_exec_auth.rs
4233 total
allowlisted=0
test_entry_kept=1
lost_other=0
added_other=0
moved_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every expected value matched the dry run on the first pass — no flake, no
re-run needed. `grep -c '^moved_identical=1$'` on this doc prints 1 (the
`moved_identical=1` line above is the only occurrence).

Checklist against the acceptance criteria:

- `shell_exec_auth.rs` (365 lines) defines `drive_connect`,
  `ensure_connected` and `run_sudo_command`; `shell_exec.rs` (3868 lines
  total, 777 production lines after the inline test modules — under the
  800-line cap, measured by `cargo test --test hygiene`: `ok.`) defines
  none of them.
- The allowlist lost its `file src/daemon/executor/shell_exec.rs` line
  (`allowlisted=0`) and kept `test src/daemon/executor/shell_exec.rs 3091`
  unchanged (`test_entry_kept=1`).
- `lost_other=0`, `added_other=0`, `moved_identical=1`, `passed=2586
  failed=0` — all four exact.
- `cargo clippy --all-targets --all-features -- -D warnings` finished
  clean; `cargo fmt --all --check` exited 0.

### Update — 2026-10-28 00:47 (complete, server-authored)

**Summary:** </think>All checks done. Status still `in-progress` in both places, no `(complete)` entry written by me, working tree clean, commit includes all touched files. Final message: Summary + Notes for review.</think>
**Summary:** Ran the Spec 1 script once with `bash` exactly as written: lines 655–1013 of `src/daemon/executor/shell_exec.rs` (`SudoAttempts`, the credential round trip, `drive_connect`, `ensure_connected`, `run_sudo_command`) moved into the new child module `src/daemon/executor/shell_exec_auth.rs` with only the two authorized visibility rewrites, wired via `#[path]` + glob `use auth::*;` + `pub(super) use auth::ensure_connected;` + `use super::USER_PROMPT_TIMEOUT;`; the allowlist lost its `file` line (kept its `test … 3091` line) and CLAUDE.md gained the new table row. The end-to-end block passed clean on the first run — `3868`/`365` line counts, `allowlisted=0`, `test_entry_kept=1`, `lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — so the known `failed=1` flake never triggered and no re-run was needed. Committed as `8568d9e1` (`refactor:`); the phase doc's Status and the README phase-table row are left at `in-progress` for the server's completion bookkeeping, and the working tree is clean.

**Notes for review:** No deviations from the spec — the script ran unmodified and its `S=655 E=1013` expectation matched. Findings (Spec 3) recorded in the Update Log: the moved file is a clean child module with no edits beyond the two visibility lines, `super::USER_PROMPT_TIMEOUT` resolves unchanged through the parent's import, and no other findings. The three executor callers of `ensure_connected` (`mod.rs` ×2, `foreground.rs`) compile unchanged through the re-export. No out-of-scope code touched; the inline test modules and `run_in_shell_core` remain for later phases.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 ok
test functions_in_a_test_module_are_not_measured ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test production_lines_exclude_an_inline_test_module ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.27s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.32s


running 4 tests
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (1656624): No such process
kill: (-1657346): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-19-shell-exec-auth.md` — +80 -1
- `src/daemon/executor/shell_exec.rs` — +7 -359
- `src/daemon/executor/shell_exec_auth.rs` — +365 -0
- `tests/hygiene_allowlist.txt` — +0 -1

**Commit:** 8568d9e1ed51bdff6bdb7a2a35c8f2d9f4eace78

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (35 turns)
- **Scope deviations:** none. `git diff --stat 2f1c3c6f HEAD` names the six
  authorized files. The moved block equals `2f1c3c6f:shell_exec.rs` lines
  655–1013 after the two visibility edits. The three external callers of
  `ensure_connected` compile unchanged. The code commit is typed
  `refactor:`. Reviewer's gates: fmt 0, build and clippy clean, 2586 passed
  / 0 failed / 13 ignored. The known flake did not fire.
- **Findings:** the executor reported `none`, which decision 7 allows. Its
  entry describes the move rather than auditing the code, though. The
  reviewer found one issue it missed, recorded as `FINDINGS.md` #61.
- **Calibration:** 35 turns, the lowest of the milestone. Against phase
  18's six findings, this run's thin audit is one data point. If the next
  production-code move also comes back with an empty audit, the Findings
  step needs a checklist (lock sites, `expect` and `unwrap`, error paths)
  rather than an open "record what you see".

