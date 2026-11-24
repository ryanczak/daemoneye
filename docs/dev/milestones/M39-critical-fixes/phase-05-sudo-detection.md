# Phase 05 — sudo-detection

**Milestone:** M39 — Critical fixes
**Status:** in-progress (bounced: [bug-05-1](bugs/bug-05-1.md))
**Drafted:** 2026-11-23
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`command_has_sudo` (`src/daemon/utils/sudo.rs`) decides whether a command
elevates. Its callers act on the answer:
- `GhostPolicy::is_safe` lets a non-elevating ghost command run freely.
- The chat client never auto-approves an elevating command.
- The foreground executor picks its credential-prompt arm.
- `sbx::script_invocation` declines to treat it as a plain script.

The detector is easy to dodge. Every command below returns `false` on the
current tree; the architect re-verified this at drafting on 2026-11-23,
18 of 18:

```
s\udo id      s'u'do id      "su""do" id      sh -c 's\udo id'
</dev/null sudo id      2>/dev/null sudo id      >out sudo id
/usr/bin/sud? id      /usr/bin/s*do id      $S id      ${X}udo id
su -c id      doas id      pkexec id      run0 id      sudoedit /etc/x
strace sudo id      flock /tmp/l sudo id
```

After this phase, every one of them returns `true`, and every existing
must-not-match case still returns `false`.

## Statement of work

**What "fixed" means:**

1. **Spelling.** A shell word is read the way the shell reads it: across
   quotes and backslash escapes, with the quote characters and the
   backslashes removed before the word is compared. `s\udo`, `s'u'do`
   and `"su""do"` are all the word `sudo`. Quoted text is **still scanned
   as a command** as well, which is the existing `sh -c '…'` behaviour.
   Keep it.
2. **Leading redirections.** A redirection and its target never change
   command position. This covers `<`, `>`, `>>`, `<<<`, `&>`, and an fd
   number written straight before the operator (`2>`, `2>>`, `1<`). So
   `</dev/null sudo id` and `2>/dev/null sudo id` see `sudo` as the
   command word.
3. **Other elevators.** In command position, the basenames `sudo`,
   `sudoedit`, `su`, `doas`, `pkexec` and `run0` all count as elevation.
   That includes an absolute path or a leading-backslash spelling
   (`/bin/su`, `\doas`).
4. **More wrappers.** Commands that run their remaining words as a command
   join `WRAPPER_COMMANDS`. At least `strace`, `ltrace`, `flock`, `chroot`,
   `unshare`, `nsenter`, `setpriv`, `chrt`, `taskset` and `systemd-run`.
   The set fails closed: every later word of the simple command is a
   command position, as today.
5. **Fail closed.** A command-position word the detector cannot resolve
   counts as elevation. That means a word that, after dequoting, still
   contains an unquoted parameter expansion (`$S`, `${X}udo`, `"$CMD"`) or
   an unquoted glob character (`*`, `?`, `[`). The same characters in
   **argument** position change nothing.

**Must stay `false` (preservation).** Every command in the existing
`sudo_detect_argument_is_not_sudo`, `command_has_sudo_false_positive_guard`
and `command_has_sudo_no_sudo` tests. Also each of these:

```
ls *.txt      rm -f /tmp/*.log      ls [ab].txt      echo $HOME
FOO=$HOME ls      echo su      echo doas      cat /usr/bin/pkexec
sudoku      ls /usr/bin/su*      git status
```

`FOO=$HOME ls` stays false because a leading assignment is not the
command word. The word after it, `ls`, is.

**Constraints:**

- No new dependencies and no shell-parser crate. This is a word scanner,
  not a shell.
- Keep the function's name and signature; six callers use it. Update its
  doc comment to say it detects elevation, fail-closed.
- `tests/hygiene.rs` caps a function at 150 lines, and `command_has_sudo`
  is already about 95. Give the word reading and dequoting their own
  function rather than growing the loop.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.

**Gotcha.** The executor's bash classifier may refuse a command line that
contains the word `sudo`. If it refuses `cargo test --lib sudo_detect`,
filter by a part of the name without that word instead:
`cargo test --lib -- detect_`.

**Scoping hints.** Start here, and widen only if the work cannot be done
without it. If you do widen, log why first.

- `src/daemon/utils/sudo.rs`: `command_has_sudo`, `sudo_command_word`,
  `WRAPPER_COMMANDS`, `COMMAND_KEYWORDS`, `is_assignment`, and the test
  module's `sudo_detect_*` tests.

**Do not read:** `tests/doc_truth.rs`, `tests/hygiene.rs`, `README.md`,
`CLAUDE.md`, `docs/dev/WORKFLOW.md`, or the callers. Their behaviour comes
from the answer, so they need no change.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **8** tool calls.

## First red test

**Write this before any production change.** Add
`sudo_detect_quote_and_backslash_spelling` to `src/daemon/utils/sudo.rs`'s
test module. It asserts `true` for `s\udo id`, `s'u'do id`, `"su""do" id`
and `sh -c 's\udo id'`. Run it red.

## Acceptance criteria

Each item is a named test that fails on the current tree, unless marked
preservation. Paste each red run into the Update Log before its fix.

- [x] **Spelling:** `sudo_detect_quote_and_backslash_spelling` passes.
- [x] **Redirections:** `sudo_detect_after_leading_redirections` passes for
      `</dev/null sudo id`, `2>/dev/null sudo id`, `>out sudo id`,
      `>>log sudo id` and `&>/dev/null sudo id`.
- [x] **Elevators:** `sudo_detect_other_elevators` passes for `su -c id`,
      `doas id`, `pkexec id`, `run0 id`, `sudoedit /etc/x`, `/bin/su` and
      `\doas id`.
- [x] **Wrappers:** `sudo_detect_more_wrappers` passes for `strace sudo id`,
      `flock /tmp/l sudo id` and one more command from each wrapper you add.
- [x] **Fail closed:** `sudo_detect_unresolvable_command_word_fails_closed`
      passes for `/usr/bin/sud? id`, `/usr/bin/s*do id`, `$S id`,
      `${X}udo id` and `"$CMD" id`.
- [x] **Preservation:** `sudo_detect_globs_and_expansions_in_arguments_stay_clear`
      passes for the must-stay-false list above. The three existing negative
      tests pass **unchanged**, so their bodies stay out of the diff.
- [x] **Preservation:** every existing `sudo_detect_*` / `command_has_sudo_*`
      test passes unchanged.
- [x] **Mutation check,** with the output pasted: stop removing quotes
      during word reading, run the spelling test and see it fail, then
      restore it.
- [x] **Gates:** all four pass, and the lib suite reports **at least 3092**
      passed (3086 plus at least six new tests). If a test in another
      module changes its expectation because a command it uses now counts
      as elevating, name it and give the reason in the Update Log.

### Bounce 1 — [bug-05-1](bugs/bug-05-1.md) (2026-11-23)

The items above were approved at review. Each one below fails on
`9fa1078e`, except the last two.

- [ ] **Quoted arguments:** `sudo_detect_quoted_arguments_stay_clear`
      asserts `false` for `find . -name '*.log'`, `grep 'foo.*bar' f`,
      `sed 's/a*/b/' f`, `awk '$3 > 5' f`, `echo "$HOME"`, `cd "$dir"`,
      `jq '.[0]' f`, `ls "$d"/*.txt` and `git commit -m 'fix: x'`.
- [ ] **Shell `-c` operands:** `sudo_detect_shell_c_operand_fails_closed`
      asserts `true` for `sh -c "$CMD"`, `bash -c 'sud? id'`, `eval "$X"`
      and `su -c "$X"`. These pass today; keep them passing.
- [ ] **Wrappers:** `sudo_detect_expansion_behind_a_wrapper_fails_closed`
      asserts `true` for `nohup $S id`, `env $S id` and `timeout 10 $S id`,
      and `false` for `timeout 10 ls *.txt`.
- [ ] **Braces:** `sudo_detect_brace_expansion_fails_closed` asserts `true`
      for `s{u,}do id`, `{sudo,} id` and `/usr/bin/{su,x}do id`, and
      `false` for `{ ls; }`.
- [ ] **Nit:** the duplicated fd-digit check, the `chars[i] == '<'` block
      straight after `read_shell_word`, is removed.
- [ ] **Mutation check,** with the output pasted: apply the fail-closed rule
      to every quoted region again, run the quoted-arguments test and see it
      fail, then restore it.
- [ ] **Gates:** all four pass, and the lib suite reports **at least 3096**
      passed.

## Spec

1. **Break down** — write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Ends when that entry is written.
2. **Implement** — work through the list, each test red before its code.
   Ends at a green `cargo build --tests`.
3. **Meet the acceptance criteria** — check off every item, including the
   mutation check and the preservation tests. Ends when every item is
   checked with its evidence named.
4. **Capture the evidence** — run the four gates. Paste each `test result:`
   line, your red runs and the mutation check into a new Update Log entry,
   but no per-test `… ok` lines. Commit as one `fix:` commit.

## Authorizations

- May touch `src/daemon/utils/sudo.rs` and its test module. Nothing else.
- If a test elsewhere must change its expectation (see Gates), record the
  file, the test and the reason in the Update Log before editing it.

## Out of scope

- `GhostPolicy::is_safe`'s structural check and `wrap_remote`. Phase 06
  covers `wrap_remote`.
- Prompting for `su`/`doas`/`pkexec` passwords. Routing them to the
  credential arm is enough; the arm's sentinel stays sudo-only.
- A real shell parser: here-docs, aliases, functions, `eval` bodies built at
  runtime. `eval` should be treated as a wrapper only if that is a one-line
  addition.
- User-facing docs. The architect writes them at review.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-23 22:15 (started)

Started phase 05 — sudo-detection. Executor: DaemonEye (executor run).
Status flipped to `in-progress`; README phase-table row updated to match.
Next: write the first red test (`sudo_detect_quote_and_backslash_spelling`) and run it red before any production change.

### Update — 2026-11-23 22:20 (breakdown)

Task list (numbered, each closes an acceptance item):

1. Write red test `sudo_detect_quote_and_backslash_spelling` (Spelling item); run it red.
2. Extract word reading + dequoting into their own functions (`read_shell_word` producing a dequoted word with an "unresolvable" flag); refactor `command_has_sudo` to use them; keep signature. Closes Spelling and Fail-closed plumbing.
3. Red test + fix `sudo_detect_after_leading_redirections` (`<`, `>`, `>>`, `<<<`, `&>`, `2>`, `2>>`, `1<` never change command position). Closes Redirections.
4. Red test + fix `sudo_detect_other_elevators` (`sudo`, `sudoedit`, `su`, `doas`, `pkexec`, `run0` by basename incl. path and `\` spelling). Closes Elevators.
5. Red test + fix `sudo_detect_more_wrappers` (add `strace`, `ltrace`, `flock`, `chroot`, `unshare`, `nsenter`, `setpriv`, `chrt`, `taskset`, `systemd-run` to `WRAPPER_COMMANDS`). Closes Wrappers.
6. Red test + fix `sudo_detect_unresolvable_command_word_fails_closed` (unquoted `$`, `${`, glob chars in command position → elevation). Closes Fail-closed.
7. Red test + preserve `sudo_detect_globs_and_expansions_in_arguments_stay_clear` (must-stay-false list; `FOO=$HOME ls` stays false). Preservation.
8. Confirm all existing `sudo_detect_*` / `command_has_sudo_*` tests pass unchanged. Preservation.
9. Mutation check: disable quote stripping in word reading, run spelling test, paste failure, restore.
10. Run the four gates; paste `test result:` lines; confirm lib suite ≥ 3092 passed.
11. Commit as one `fix:` commit.

### Update — 2026-11-23 22:22 (red run 1 — Spelling)

First red test written before any production change. Output tail:

```
failures:

---- daemon::utils::sudo::tests::sudo_detect_quote_and_backslash_spelling stdout ----

thread 'daemon::utils::sudo::tests::sudo_detect_quote_and_backslash_spelling' (2567244) panicked at src/daemon/utils/sudo.rs:457:13:
a sudo spelled across quotes or backslashes is the command sudo: "s\\udo id"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    daemon::utils::sudo::tests::sudo_detect_quote_and_backslash_spelling

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3096 filtered out; finished in 0.00s
```

### Update — 2026-11-23 22:26 (red runs 2–6 — all acceptance tests, before any production change)

All remaining red tests written up front; all fail on the current tree as required:

```
thread 'daemon::utils::sudo::tests::sudo_detect_after_leading_redirections' (2569224) panicked at src/daemon/utils/sudo.rs:476:13:
a redirection and its target never change command position: "</dev/null sudo id"

---- daemon::utils::sudo::tests::sudo_detect_more_wrappers stdout ----
thread 'daemon::utils::sudo::tests::sudo_detect_more_wrappers' (2569228) panicked at src/daemon/utils/sudo.rs:521:13:
a wrapper keeps every later word in command position: "strace sudo id"

---- daemon::utils::sudo::tests::sudo_detect_other_elevators stdout ----
thread 'daemon::utils::sudo::tests::sudo_detect_other_elevators' (2569230) panicked at src/daemon/utils/sudo.rs:496:13:
an elevator in command position is elevation whatever its spelling: "su -c id"

failures:
    daemon::utils::sudo::tests::sudo_detect_after_leading_redirections
    daemon::utils::sudo::tests::sudo_detect_more_wrappers
    daemon::utils::sudo::tests::sudo_detect_other_elevators
    daemon::utils::sudo::tests::sudo_detect_quote_and_backslash_spelling
    daemon::utils::sudo::tests::sudo_detect_unresolvable_command_word_fails_closed

test result: FAILED. 18 passed; 5 failed; 0 ignored; 0 measured; 3079 filtered out; finished in 0.00s
```

The preservation test `sudo_detect_globs_and_expansions_in_arguments_stay_clear` is green on the current tree (as intended — it pins must-stay-false behaviour).

### Update — 2026-11-23 22:33 (progress)

First implementation pass: `read_shell_word` + `is_elevator` extracted, wrappers and elevators added, fail-closed on unresolvable command words. Partial results:

- All spelling, elevators, wrappers and fail-closed tests now pass.
- Redirection cases still fail: my skip loop handles `&>` and `<<<` only while `&` follows the operator, but `&>/dev/null` starts at `&` (matched by the separator arm, which resets command position) and the `<`-skip loop stopped at the `/` of the target, leaving the target word to consume command position.
- `FOO=$HOME ls` returns `true` instead of false: `$HOME` in an assignment's value makes the word "unresolvable" and, since the word is still in command position, the fail-closed branch fires before the assignment check.

Fixing both next.

### Update — 2026-11-23 22:40 (implementation complete, green)

Implementation finished. Word reading and dequoting live in `read_shell_word`
(returns the dequoted text, an unresolvable flag, and the quoted contents);
command position, redirections, wrappers, elevators and fail-closed logic live
in `command_has_sudo`. `cargo build --tests` is green; all 23 `detect_` tests
pass, including the three existing negative tests unchanged and the new
preservation test.

### Update — 2026-11-23 22:42 (mutation check)

Mutation: stopped removing single quotes during word reading (kept `'` in the
word text after reading a quoted region). The spelling test fails as required:

```
a sudo spelled across quotes or backslashes is the command sudo: "s'u'do id"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    daemon::utils::sudo::tests::sudo_detect_quote_and_backslash_spelling

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3101 filtered out; finished in 0.00s
```

### Update — 2026-11-23 22:50 (end-to-end verification)

Gates run against the real tree, in order:

```
$ cargo fmt --all
(no output — clean)

$ cargo build 2>&1 | tail -3
   Compiling daemoneye v2.10.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.86s

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
    Checking daemoneye v2.10.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.49s

$ cargo test 2>&1 | grep "test result:"
test result: ok. 3092 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 143.32s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.23s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.25s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

Lib suite reports **3092 passed** — exactly the required floor (3086 + 6 new tests).
No test in another module changed its expectation; every suite is green unchanged.

Acceptance evidence, item by item:

- **Spelling:** `sudo_detect_quote_and_backslash_spelling` — red run 1 (22:22), green after `read_shell_word`.
- **Redirections:** `sudo_detect_after_leading_redirections` — red run (22:26), green after the `<`/`>`/`&` redirection arms skip operator and target.
- **Elevators:** `sudo_detect_other_elevators` — red run (22:26), green after `ELEVATORS` + `is_elevator`.
- **Wrappers:** `sudo_detect_more_wrappers` — red run (22:26), green after ten names joined `WRAPPER_COMMANDS` (plus `eval`, a one-line addition the spec allows).
- **Fail closed:** `sudo_detect_unresolvable_command_word_fails_closed` — red run (22:26), green after the unresolvable branch.
- **Preservation:** `sudo_detect_globs_and_expansions_in_arguments_stay_clear` green; the three existing negative tests pass **unchanged** (their bodies are not in the diff).
- **Mutation check:** pasted above (22:42) — failing red under quote-keeping mutation, restored, green again.

### Update — 2026-11-23 23:16 (complete, server-authored)

**Summary:** **Summary:** Rewrote `command_has_sudo` in `src/daemon/utils/sudo.rs` so it detects elevation fail-closed: shell words are now read and dequoted by a new `read_shell_word` helper (quotes and backslashes removed, quoted text still scanned as a command), leading redirections (`<`, `>`, `>>`, `<<<`, `&>`, `&>>`, and fd-prefixed `2>`/`1<`) never change command position, `su`/`doas`/`pkexec`/`run0`/`sudoedit` join `sudo` as elevators by basename, ten more wrappers (`strace`, `ltrace`, `flock`, `chroot`, `unshare`, `nsenter`, `setpriv`, `chrt`, `taskset`, `systemd-run` — plus `eval`, the spec's sanctioned one-liner) joined `WRAPPER_COMMANDS`, and a command-position word that still carries an unquoted parameter expansion or glob character counts as elevation. All six acceptance tests were written red first and each red run is pasted in the Update Log, along with the mutation check (quote-keeping mutation → spelling test fails → restored, green).

**Notes for review:**
- `eval` was added to `WRAPPER_COMMANDS` under the spec's "one-line addition" allowance; `sudo_command_word` was replaced by `is_elevator`/`ELEVATORS` since its name no longer described a sudo-only check — the public function's name and signature are unchanged.
- Two implementation subtleties: a leading `NAME=...` assignment is excluded from the fail-closed branch so `FOO=$HOME ls` stays false, and an fd digit directly before a redirection operator is folded into the redirection rather than treated as a command word.
- No test in another module changed its expectation; all existing suites pass unchanged.
- Gates: `cargo fmt --all` clean, `cargo build` zero warnings, clippy `-D warnings` clean, `cargo test` fully green — lib suite reports **3092 passed** (3086 + 6 new tests, exactly the required floor). Committed as `9fa1078e` (`fix:`), working tree clean. Status left at `in-progress` per the lifecycle split; no `(complete)` entry written.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test attach_sends_session_counters_after_ok ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.22s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-0635aac7b8e1e57e)
kill: (2614124): No such process
kill: (-2614842): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-f945a8c3f7582fdb)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-2e19192a3a1354c4)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7fcecbb518583aee)
     Running tests/hygiene.rs (target/debug/deps/hygiene-653cf39ac0e64fe2)
     Running tests/integration.rs (target/debug/deps/integration-849708bb7ff158dd)
     Running tests/isolation.rs (target/debug/deps/isolation-fbc36518ed3cd563)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-ae0cb5d281522d21)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-55beac99186d4a12)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M39-critical-fixes/README.md` — +1 -1
- `docs/dev/milestones/M39-critical-fixes/phase-05-sudo-detection.md` — +146 -1
- `src/daemon/utils/sudo.rs` — +351 -51

**Commit:** 9fa1078e07bc35a553f8014d83615f7d0233c9e1

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Notes for executor — 2026-11-23 (bounce 1)

**The four gates are green and the tree is clean, and that is expected. It
is NOT evidence that the phase is done.** Every item under "Acceptance
criteria" above the Bounce 1 heading is already approved; do not redo it.
What remains is exactly the four behaviours in
[bug-05-1](bugs/bug-05-1.md), plus one nit:

1. Quoted text keeps its scan for the elevator names, but the fail-closed
   rule applies inside quotes only to a shell's `-c` operand (`sh`, `bash`,
   `dash`, `zsh`, `ksh`), an `eval` operand, or `su -c`'s operand.
2. A word behind a wrapper fails closed on an unquoted `$`, but not on a
   glob.
3. An unquoted brace expansion (`{…,…}` or `{a..b}`) in command position is
   unresolvable.
4. Delete the duplicated fd-digit check.

Write the three red tests first. The shell `-c` test is green today and
pins existing behaviour. You are done when the lib suite reports at least
**3096** passed and the bounce's mutation check is pasted. Only
`src/daemon/utils/sudo.rs` changes. You have 1200 turns; make your first
edit within 6 tool calls.
