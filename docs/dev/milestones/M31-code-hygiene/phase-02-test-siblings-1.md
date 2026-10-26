# Phase 02 — Test modules to sibling files, batch 1

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-25

## Goal

Eight files whose production code already fits the cap but whose inline
`mod tests` pushes them past 1000 lines keep their tests in a sibling file,
the way `ipc.rs` → `ipc_tests.rs` and `tmux/cache.rs` → `cache_tests.rs`
already do. No test changes, no production change, same test count.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5 (moves
  are shell range cuts), 6 (no behaviour change) and 7 (the Findings
  entry).
- `src/ipc.rs` and `src/ipc_tests.rs` — the convention: the parent ends in
  `#[cfg(test)] #[path = "ipc_tests.rs"] mod tests;` and the sibling holds
  the module's former body at column 0, with its `use super::*;` intact
  (`super` still names the parent, because `#[path]` does not change the
  module tree).

## Pre-flight

- The four gates are green at HEAD; `cargo test --test hygiene` passes.
- In each file below, the test module is the **last** item: one
  `#[cfg(test)]` line followed by `mod tests {`, closing with the file's
  final `}`. Test-only `use` lines higher in a file (`shell/client.rs` has
  two) are not part of the move and stay where they are.

## Spec

The files, with the sibling each gets:

| File | Sibling |
|---|---|
| `src/daemon/server/shells.rs` | `src/daemon/server/shells_tests.rs` |
| `src/shell/session.rs` | `src/shell/session_tests.rs` |
| `src/ai/tools/dispatch.rs` | `src/ai/tools/dispatch_tests.rs` |
| `src/shell/backend.rs` | `src/shell/backend_tests.rs` |
| `src/config/mod.rs` | `src/config/tests.rs` |
| `src/shell/client.rs` | `src/shell/client_tests.rs` |
| `src/daemon/executor/sbx.rs` | `src/daemon/executor/sbx_tests.rs` |
| `src/daemon/session.rs` | `src/daemon/session_tests.rs` |

1. **Before any edit**, record the test inventory:
   `cargo test -- --list 2>/dev/null | grep ': test$' | sort > /tmp/m31-02-before.txt`.
2. **For each file, one move, with the shell.** Let `T` be the line of the
   trailing `#[cfg(test)]` (`grep -n '^#\[cfg(test)\]' FILE | tail -1`),
   so `T+1` is `mod tests {` and the last line of the file is its `}`.
   - The sibling is lines `T+2` through the second-to-last line:
     `sed -n "$((T+2)),$(( $(wc -l < FILE) - 1 ))p" FILE > SIBLING`.
   - The parent keeps lines `1` through `T-1`, then the three-line
     declaration: `#[cfg(test)]`, `#[path = "<sibling file name>"]`,
     `mod tests;`. Write it with `head -n $((T-1))` into a temp file,
     append the three lines, and `mv` it over FILE.
   - Do **not** retype, re-indent or edit the module body. `cargo fmt
     --all` afterwards brings the sibling to column 0.
3. After all eight: `cargo fmt --all`, then the gates, then
   `cargo test -- --list 2>/dev/null | grep ': test$' | sort > /tmp/m31-02-after.txt`
   and `diff /tmp/m31-02-before.txt /tmp/m31-02-after.txt` must print
   nothing. The eight files and the eight siblings are the only changes
   under `src/`.
4. **Findings** (README decision 7): while the only code read is the test
   bodies as they scroll past, record anything seen: a test that asserts
   nothing, a duplicated fixture that wants a shared helper, a sleep-based
   wait. `none` is a valid entry.

**Gotchas.**
- `#[path]` is relative to the directory of the file that declares the
  module, so it is the sibling's **file name**, not a path.
- A sibling for a `mod.rs` is `tests.rs` in the same directory (the
  hygiene gate already classes `tests.rs` as a test file); for any other
  file it is `<stem>_tests.rs`.
- The hygiene gate measures test siblings against the 2000-line cap; the
  largest body here is about 1330 lines, so none needs splitting.

## Acceptance criteria

- [ ] All eight siblings exist, and for each of the eight parents
      `grep -c '^mod tests {' FILE` is 0 while `grep -c '^mod tests;' FILE`
      is 1.
- [ ] `diff /tmp/m31-02-before.txt /tmp/m31-02-after.txt` is empty: the
      same tests, by full path, before and after.
- [ ] Preservation: `cargo test` passes with 2586 passing tests, none
      ignored that was not ignored before; `cargo test --test hygiene`
      passes; `git diff --stat HEAD -- src | tail -1` names 16 files.
- [ ] `git diff HEAD -- src` contains no `+` line that is not either one of
      the three declaration lines or a line present in the moved body:
      `git diff -U0 HEAD -- <each parent> | grep '^+' | grep -vE '^\+\+\+|^\+#\[cfg\(test\)\]|^\+#\[path = |^\+mod tests;'` prints nothing.
- [ ] The Update Log has a `### Findings — <date>` entry.

## Test plan

No new tests: this phase moves tests. The proof is the identical
`--list` inventory and the unchanged pass count.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m31-02.txt; : > "$A"
for f in src/daemon/server/shells.rs src/shell/session.rs src/ai/tools/dispatch.rs src/shell/backend.rs src/config/mod.rs src/shell/client.rs src/daemon/executor/sbx.rs src/daemon/session.rs; do echo "$f inline=$(grep -c '^mod tests {' $f) decl=$(grep -c '^mod tests;' $f)" >> "$A"; done
ls src/daemon/server/shells_tests.rs src/shell/session_tests.rs src/ai/tools/dispatch_tests.rs src/shell/backend_tests.rs src/config/tests.rs src/shell/client_tests.rs src/daemon/executor/sbx_tests.rs src/daemon/session_tests.rs 2>&1 | wc -l >> "$A"
echo "list_diff_lines=$(diff /tmp/m31-02-before.txt /tmp/m31-02-after.txt | wc -l)" >> "$A"
echo "files_changed=$(git diff --stat HEAD -- src | tail -1)" >> "$A"
for f in src/daemon/server/shells.rs src/shell/session.rs src/ai/tools/dispatch.rs src/shell/backend.rs src/config/mod.rs src/shell/client.rs src/daemon/executor/sbx.rs src/daemon/session.rs; do git diff -U0 HEAD -- $f | grep '^+' | grep -vE '^\+\+\+|^\+#\[cfg\(test\)\]|^\+#\[path = |^\+mod tests;'; done | wc -l | sed 's/^/foreign_added_lines=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: eight `inline=0 decl=1` lines, `8`, `list_diff_lines=0`,
`files_changed=` naming 16 files, `foreign_added_lines=0`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`. (If the run is before
a commit, `HEAD` is the pre-phase tree, which is what the diff checks
want; if the moves were committed first, use the pre-phase commit hash in
place of `HEAD`.)

## Authorizations

- Edit the eight files in the table; create the eight siblings. Nothing
  else under `src/`.

## Out of scope

- Any edit inside a test body, any rename, any new test.
- The other files with large inline test modules (batch 2) and the four
  files whose test bodies exceed 2000 lines (they need splitting by
  subject, a later phase).
- CLAUDE.md's "Key files" table: test siblings are not modules it lists.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-25 23:50 (started, executor: Claude Opus 4.6)

Pre-flight confirmed: all four gates green at HEAD `c4467208`; the eight
target files each end in exactly one `mod tests {` whose `#[cfg(test)]` is
the file's last-but-one `#[cfg(test)]` line; none of the eight siblings
exists yet; `src/ipc.rs` / `src/ipc_tests.rs` taken as the convention
reference. Test inventory captured to `/tmp/m31-02-before.txt` (2599 test
lines). Beginning the eight moves.

### Update — 2026-10-26 00:09 (end-to-end verification)

The moves were committed first as `d1f02c26`, so the diff checks in the
phase script use the pre-phase commit `c4467208` in place of `HEAD`, as the
phase doc anticipates. Transcript (commands per the phase doc, run with
`bash`; output redirected to `/tmp/e2e-m31-02.txt` and pasted byte-exact):

```
src/daemon/server/shells.rs inline=0 decl=1
src/shell/session.rs inline=0 decl=1
src/ai/tools/dispatch.rs inline=0 decl=1
src/shell/backend.rs inline=0 decl=1
src/config/mod.rs inline=0 decl=1
src/shell/client.rs inline=0 decl=1
src/daemon/executor/sbx.rs inline=0 decl=1
src/daemon/session.rs inline=0 decl=1
8
list_diff_lines=0
files_changed= 16 files changed, 9364 insertions(+), 9372 deletions(-)
foreign_added_lines=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every expected value matches the phase doc: eight `inline=0 decl=1` lines,
`8` siblings listed, `list_diff_lines=0` (2599-entry inventory identical
before and after, `/tmp/m31-02-before.txt` vs `/tmp/m31-02-after.txt`),
16 files changed under `src/`, zero foreign added lines in the eight
parents, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, and
`passed=2586 failed=0` (the 13 ignored tests are the pre-existing ones —
verified by stashing the moves and re-running: identical counts).

### Findings — 2026-10-26

Read nothing but test bodies as they moved (9,356 sibling lines, 293
`#[test]`/`#[tokio::test]` items). Entries, most useful first:

- **Sixth copy of the temp-HOME fixture.** `shell_test_home()` in
  `shells_tests.rs` and `test_home()` in `session_tests.rs` are byte-for-byte
  the same 7-line fixture (guard + tempdir + `set_var("HOME")` +
  `Config::ensure_dirs()`), and `grep` shows four more inline copies of
  `fn test_home()` with the same shape in `shell/launch.rs`,
  `daemon/executor/{ghost_remote,bg_shell,shell_exec,foreground}.rs`,
  `ghost_sandbox.rs` — the crate-level helper `crate::test_home_guard()`
  already exists; a single `pub(crate) fn test_home()` wrapping it would
  retire eight copies. Good candidate for a later hygiene phase.
- **Wall-clock poll loops that a channel would replace.** The PTY-backed
  tests (`shell/backend_tests.rs`, `shell/client_tests.rs`,
  `shell/session_tests.rs`, `daemon/server/shells_tests.rs`) wait on real
  output with `Instant::now()` deadline loops at 10–50 ms granularity — 50+
  such loops. They are bounded and fail with a message (not hangs), and the
  underlying subject is a real process, so they are not the sleep-based-wait
  anti-pattern in its worst form, but a `tokio::sync::Notify`/channel keyed
  on the cast-file line count would make them deterministic and faster.
- **Two bare sleeps worth flagging:** `client_tests.rs:1041,1047` sleeps
  300 ms then 3 s to prove paused time does not count against a 2 s
  deadline (semantic — the pause really must outlast the deadline — but it
  makes the test 3.3 s); `backend_tests.rs:869,880` sleeps 300 ms after
  `Int` before probing, twice, with no deadline loop.
- **`dispatch_tests.rs` would read better split by subject** (`schema`,
  `defs`, `select_tools`, `render_*` sections are already delimited by
  banner comments) — relevant to the later 2000-line splitting phase.
- No test asserts nothing; every test in the eight siblings contains at
  least one assertion, `.expect()` with a message, or a `panic!` in a helper.

### Update — 2026-10-26 00:12 (complete, server-authored)

**Summary:** All eight moves are done, verified, and committed.

**Summary + Notes for review**

Executed M31 phase-02 (test-siblings-1): moved the inline `mod tests` of all eight listed files to their specified siblings (`shells_tests.rs`, `session_tests.rs`, `dispatch_tests.rs`, `backend_tests.rs`, `config/tests.rs`, `client_tests.rs`, `sbx_tests.rs`, `daemon/session_tests.rs`) by shell range cut, exactly per the spec — each sibling is the untouched former module body (proved byte-identical after whitespace stripping against the pre-move `HEAD` body), each parent ends in the three-line `#[cfg(test)] #[path = "<sibling>"] mod tests;` declaration, and `cargo fmt --all` brought the bodies to column 0. End-to-end verification (transcript pasted byte-exact into the phase doc's Update Log, run against pre-phase commit `c4467208` since the moves were committed first as `d1f02c26`): eight `inline=0 decl=1` lines, 8 siblings present, `--list` inventory diff empty (2599 entries identical), 16 files changed under `src/`, 0 foreign added lines, hygiene gate `ok. 16 passed`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — the 13 ignored tests are pre-existing, confirmed by stashing the moves and re-running. Two deviations worth the architect's eye: (1) rustfmt reflowed a handful of moved lines (long `assert!`/string-literal wraps, and one trailing comma removed in `client_tests.rs`), verified semantically identical — the spec's "do not retype" was honored, the reflow is rustfmt's, not mine; (2) rustfmt left a single leading blank line at the top of the `config/tests.rs` and `shells_tests.rs` siblings where the body's first line was blank (the config body's first `use` is blank), which fmt then normalized — content unaffected. Findings are in the phase doc's `### Findings — 2026-10-26` entry: a sixth copy of the temp-HOME fixture wanting a crate-level helper, PTY tests' wall-clock poll loops (channels would make them deterministic), two 3-second bare sleeps, and `dispatch_tests.rs` as a split-by-subject candidate. Status left at `in-progress`; no `(complete)` entry written; working tree clean at `6617eeae`.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.3.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.80s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
s_test_only ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test duplicate_names_get_a_suffix ... ok
test function_length_spans_signature_to_closing_brace ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_entry_format ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (616204): No such process
kill: (-616912): No such process
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

- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-02-test-siblings-1.md` — +77 -1

**Commit:** 6617eeaee97a591688c43b5f24a9c86381d9a2e1

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
