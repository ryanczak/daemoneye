# Phase 01c: file-op-error-label — a failed sudo edit names its file and host, and no run error doubles its full stop

**Milestone:** M23 — Client decoupling & shell UX
**Status:** done
**Depends on:** none (M21 09b review residual)
**Estimated diff:** ~40 lines of production code across 2 files; 3 new tests
**Tags:** language=rust, kind=bugfix, size=s

## Goal

When `edit_file { host, sudo: true }` fails to run, the model reads an error
built by `shell_run_error_text`, a helper written for `run_terminal_command`.
That produces three problems:

1. **It names a shell that does not exist.** The helper takes a shell id,
   and the sudo edit passes its owner key, so the text reads "Error running
   command in shell **edit_file**". No shell has that id, so a model that
   tries to use it gets nothing.
2. **Its exited branch gives impossible advice:** "Omit `shell` (or pass
   "new")". `edit_file` has no `shell` parameter.
3. **Its give-up branch doubles a full stop, for every caller.** The
   give-up reason already ends with a period, and the helper appends
   `. The password prompt …`, producing `…from a chat session.. The password
   prompt…`. `run_terminal_command` shows this too.

After this phase, a sudo edit's run error is worded for a file operation
(the file, the host, and what to do next), and no run error doubles its full
stop.

## Architecture references

- `docs/dev/milestones/M21-host-model/README.md` § "Review residuals carried
  forward" — the 09b residual this closes.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

**Shell-gate note.** Your shell refuses any command you type that contains
the literal word for the privilege tool. The new tests and the new helper are
named `root_…`; filter with those names. Source code may contain the word.

## Current state

Measured on **2026-09-30** at commit `42eac1b8`.

Baseline: `cargo test --lib` → **2092 passed / 0 failed / 14 ignored**.

### 1. The helper (`src/daemon/executor/mod.rs:243-264`)

```rust
fn shell_run_error_text(shell_id: &str, e: &anyhow::Error) -> String {
    if e.chain().any(|c| c.is::<crate::shell::PromptGivenUp>()) {
        format!(
            "Error running command in shell {shell_id}: {e:#}. The password prompt \
             was cancelled with Ctrl-C, so the command did not run with elevated \
             privileges. Fix what the error names before retrying."
        )
    } else if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
        format!(
            "Error running command in shell {shell_id}: {e:#}. That shell has \
             exited, so the command is not running there. Omit `shell` (or pass \
             \"new\") to run it in a live shell."
        )
    } else {
        format!(
            "Error running command in shell {shell_id}: {e:#}. \
             The command may still be running in that shell — \
             do not assume it failed outright; retrying could \
             double-apply its effect."
        )
    }
}
```

Its four other callers pass a real shell id (`mod.rs:574`, `:690`,
`ghost_remote.rs:87`) and keep it. The sudo edit calls it at
`src/daemon/executor/file_ops/ops.rs:151`:

```rust
                        return Ok(ToolCallOutcome::Result(super::super::shell_run_error_text(
                            "edit_file",
                            &e,
                        )));
```

### 2. Measured: the three branches as a sudo edit renders them

Rendered by calling the helper with `"edit_file"` and each error the run path
produces (a throwaway probe in `mod.rs`'s tests, since reverted):

| Branch | Text (abridged) |
|---|---|
| give-up | `Error running command in shell edit_file: … or run it from a chat session.. The password prompt was cancelled with Ctrl-C, …` |
| exited | `Error running command in shell edit_file: shell exited before the command finished: "x". That shell has exited, … Omit \`shell\` (or pass "new") to run it in a live shell.` |
| timeout | `Error running command in shell edit_file: timed out after 30s waiting for command output: "x". The command may still be running in that shell — …` |

The `..` is in the helper, not in the edit path. Any give-up reason that ends
with a period doubles it, and 08f's reason does ("…or run it from a chat
session.").

### 3. How the tests build each error

`mod.rs`'s existing tests `a_dead_shell_error_does_not_say_the_command_may_still_be_running`
and `a_given_up_prompt_error_does_not_say_the_command_may_still_be_running`
construct the three shapes. Reuse them:

```rust
        let exited = anyhow::Error::new(crate::shell::ShellExited {
            cmd: "echo hi".to_string(),
        });
        let timeout =
            anyhow::anyhow!("timed out after 45s waiting for command output: \"sleep 60\"");
        let gave_up = anyhow::Error::new(crate::shell::PromptGivenUp {
            reason: "nobody is attached to answer it".to_string(),
        })
        .context("the password prompt in shell s7 was cancelled");
```

Both existing tests must stay green unchanged (*preservation*). They assert
that the shell id appears, and on the may-still-be-running caveat.

## Spec

### Task 1 — render the chain without a trailing full stop (`src/daemon/executor/mod.rs`)

1. Beside `shell_run_error_text`, add:

   ```rust
   /// An error chain as one sentence fragment: `{e:#}` with any trailing full
   /// stop (and whitespace) removed, so a caller can append `. <guidance>`
   /// without printing `..` when the innermost reason already ended a sentence.
   fn error_chain(e: &anyhow::Error) -> String {
       let chain = format!("{e:#}");
       chain.trim_end().trim_end_matches('.').to_string()
   }
   ```

2. In all three branches of `shell_run_error_text`, replace `{e:#}` with the
   value of `error_chain(e)`: bind it once at the top of the function and
   interpolate that binding. Nothing else in the helper changes. Its wording
   and its `shell_id` stay as they are for the terminal callers.

### Task 2 — a sudo edit's own error wording (`src/daemon/executor/file_ops/ops.rs`)

1. Add this private free function to `ops.rs`, above `run_edit_with_spawn`:

   ```rust
   /// What a sudo edit that failed to run tells the model. Worded for a file
   /// operation: it names the file and the host, never a shell id (the edit
   /// claims its shell under an owner key, not an id the model could use), and
   /// never advises a `shell` parameter `edit_file` does not have.
   fn root_edit_error_text(path: &str, host: &str, e: &anyhow::Error) -> String {
       let chain = super::super::error_chain(e);
       if e.chain().any(|c| c.is::<crate::shell::PromptGivenUp>()) {
           format!(
               "Error: the sudo edit of {path} on host {host} did not run: {chain}. \
                The password prompt was cancelled, so {path} was not changed."
           )
       } else if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
           format!(
               "Error: the sudo edit of {path} on host {host} did not run: {chain}. \
                The shell on host {host} exited before the edit ran, so {path} was \
                not changed; retrying the edit opens a new shell there."
           )
       } else {
           format!(
               "Error: the sudo edit of {path} on host {host} did not finish: {chain}. \
                It may still complete on the host — read {path} before retrying, or a \
                retry could apply the edit twice."
           )
       }
   }
   ```

2. At `ops.rs:151`, replace the `shell_run_error_text("edit_file", &e)` call
   with `root_edit_error_text(path, name, &e)`. `path` and `name` are
   already in scope in that branch, as the success sentence
   `"Edited {} on host {}."` uses them.

### Task 3 — tests

Three tests, names pinned.

In `src/daemon/executor/mod.rs` tests (beside the two quoted in § Current
state 3):

1. `a_run_error_never_doubles_its_full_stop` — build a `PromptGivenUp` whose
   `reason` **ends with a full stop**
   (`"nobody is attached to answer it — run it from a chat session."`), with
   the same `.context(...)` as above. For that, the `exited` error and the
   `timeout` error, `shell_run_error_text("s7", …)` never contains `".."`.
   For the give-up, also assert it contains `"chat session. The password prompt"`.
   That pins that the reason's sentence survives and the guidance follows it
   once.

In `src/daemon/executor/file_ops/ops.rs` tests:

2. `a_root_edit_error_names_the_file_and_host` — for each of the three errors
   (the give-up reason again ending with a full stop),
   `root_edit_error_text("/etc/x.conf", "pinky", &e)`:
   - starts with `"Error: "` and contains `"/etc/x.conf"` and `"pinky"`;
   - does **not** contain `"in shell edit_file"`, ``"Omit `shell`"`` or
     `".."`.
3. `a_root_edit_error_keeps_each_branch_meaning` — for the same three errors:
   - **give-up:** contains `"nobody is attached"` and `"was not changed"`,
     not `"may still"`;
   - **exited:** contains `"exited"` and `"was not changed"`, not
     `"may still"`;
   - **timeout:** contains `"may still complete"`, not `"was not changed"`.

**Test count:** three new, so **2095 passed / 14 ignored**.

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output **into
this phase doc** as the first fenced block of a new Update Log entry titled
exactly `### Update — <date> (end-to-end verification)`. **Then run the paste
check and put its literal output line in the entry, on a line of its own.**
The last two phases each reported a paste check without printing its line
into the doc. The criterion below counts that line.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was run at drafting.

- [ ] `cargo test --lib` → **2095 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2092/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The three new tests pass:
      ``cargo test --lib -- a_run_error_never_doubles_its_full_stop root_edit_error``
      → **3 passed** (current: `0 passed; … filtered out`).
- [ ] The two existing helper tests still pass unchanged (*preservation*):
      ``cargo test --lib -- a_dead_shell_error_does_not_say_the_command_may_still_be_running a_given_up_prompt_error_does_not_say_the_command_may_still_be_running``
      → **2 passed**.
- [ ] The 09b edit tests still pass (*preservation*):
      ``cargo test --lib root_edit`` → **0 failed**.
- [ ] The sudo edit no longer calls the terminal helper:
      ``grep -c 'shell_run_error_text' src/daemon/executor/file_ops/ops.rs``
      → **0** (current: **1**).
- [ ] Mutations A and B were applied, run, and their observed results recorded.
- [ ] The § End-to-end verification block was run verbatim and pasted as the
      **first fenced block** of the `(end-to-end verification)` entry:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>``
      → **1** (current: 0).
- [ ] **The paste-check line is in the doc**, on a line of its own:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: **0**). The
      only full-line `PASTE MATCH` in this file is the one you paste. The
      spec's own mentions are inside backticks or longer lines.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run once, restore, and record the observed
result.

- **Mutation A:** make `error_chain` return `format!("{e:#}")` unchanged (no
  trimming). Run test 1. Expected **red** (`..` present).
- **Mutation B:** in `root_edit_error_text`'s give-up branch, interpolate
  `{e:#}` instead of `{chain}` (the untrimmed chain). Run test 2. Expected
  **red** (`..` present in the give-up rendering). The call-site swap itself
  is pinned by the `ops_terminal_helper=0` criterion, not by a mutation.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Three tests. The shared helper no longer doubles a full stop, on any branch.
A sudo edit's error names the file and the host, never a pseudo shell id or
a `shell` parameter, and keeps each branch's meaning.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- a_run_error_never_doubles_its_full_stop root_edit_error 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'helper_tests=%s\n' "$(cargo test --lib -- a_dead_shell_error_does_not_say_the_command_may_still_be_running a_given_up_prompt_error_does_not_say_the_command_may_still_be_running 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'root_edit_suite=%s\n' "$(cargo test --lib root_edit 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ failed')"
  printf 'ops_terminal_helper=%s\n' "$(grep -c 'shell_run_error_text' src/daemon/executor/file_ops/ops.rs)"
} > /tmp/e2e-m23-01c.txt
cat /tmp/e2e-m23-01c.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2095p-0f-14i`,
`new_tests=3p-0f`, `helper_tests=2 passed`, `root_edit_suite=0 failed`,
`ops_terminal_helper=0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Paste the file into
this phase doc, then run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-01c-file-op-error-label.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-01c.txt
diff /tmp/pasted-m23-01c.txt /tmp/e2e-m23-01c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the line that command prints into the entry, on its own line** (the
bare line, not inside backticks and not in a sentence). Mutation results go
in the same entry, after it.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/executor/mod.rs`: Task 1 and test 1.
- `src/daemon/executor/file_ops/ops.rs`: Task 2 and tests 2–3.
- This phase doc's Update Log.

## Out of scope

- **`shell_run_error_text`'s wording for the terminal callers.** Only the
  chain rendering changes. Its `shell_id` and guidance sentences stay.
- **The give-up reason's own text** (08f's "…run it from a chat session."),
  which the chat case shows too. That is a separate wording question.
- **Non-sudo remote writes.** They go through `run_in_host_shell` and render
  `Error: {e}` directly; this phase does not change them.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-01 01:07 (started)

**Started by:** Claude (executor, phase 01c file-op-error-label)

Status flipped to `in-progress`; README phase-table row updated to match.
Baseline confirmed: clean tree on `master` at `9835064e`. Working through the
spec in order: Task 1 (`error_chain` + the helper's three branches), Task 2
(`root_edit_error_text` in `file_ops/ops.rs`), then the three pinned tests,
the two mutations, and the end-to-end capture.

### Update — 2026-10-01 01:30 (end-to-end verification)

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2095p-0f-14i
new_tests=3p-0f
helper_tests=2 passed
root_edit_suite=0 failed
ops_terminal_helper=0
```

PASTE MATCH

Mutation A — `error_chain` returns `format!("{e:#}")` unchanged (no
trimming), test `a_run_error_never_doubles_its_full_stop`: **red** as
expected —
`no run error doubles its full stop: Error running command in shell s7: the
password prompt in shell s7 was cancelled: nobody is attached to answer it —
run it from a chat session.. The password prompt was cancelled with Ctrl-C, …`
Restored; `git diff --stat` back to its pre-mutation state.

Mutation B — `root_edit_error_text`'s give-up branch interpolates `{e:#}`
instead of `{chain}`, test `a_root_edit_error_names_the_file_and_host`:
**red** as expected —
`no doubled full stop: Error: the sudo edit of /etc/x.conf on host pinky did
not run: the password prompt in shell s7 was cancelled: nobody is attached to
answer it — run it from a chat session.. The password prompt was cancelled,
so /etc/x.conf was not changed.`
Restored; `git diff --stat` back to its pre-mutation state.

### Update — 2026-10-01 01:18 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Implemented phase 01c: added `error_chain` beside `shell_run_error_text` and made all three of its branches interpolate the trimmed chain instead of `{e:#}` (terminal wording and shell ids untouched), and gave the sudo edit its own `root_edit_error_text(path, host, e)` in `file_ops/ops.rs` — give-up and exited branches say "did not run … was not changed", the timeout branch says "did not finish … may still complete" — replacing the `shell_run_error_text("edit_file", &e)` call at the edit's error site. Added the three pinned tests (`a_run_error_never_doubles_its_full_stop`, `a_root_edit_error_names_the_file_and_host`, `a_root_edit_error_keeps_each_branch_meaning`); all existing tests are untouched and green. End-to-end block ran verbatim and pasted clean: `lib1..3=ok` with no failures, `lib_counts=2095p-0f-14i` on three consecutive runs, `new_tests=3p-0f`, `helper_tests=2 passed`, `root_edit_suite=0 failed`, `ops_terminal_helper=0`; both mutations were applied, ran red with the expected `..` text, and were restored (`git diff --stat` confirmed each restore); `cargo fmt --all`, `cargo build`, and clippy `-D warnings` are clean. **Notes for review:** (1) the spec's `grep -c` paste-check criterion counts `^### Update — .* (end-to-end verification)$` — my entry's heading matches that pattern, and `grep -cx 'PASTE MATCH'` returns 1, so both doc greps hold; the first paste attempt failed only because the entry was not yet written (the check was re-run after writing and printed the required line). (2) Committed as `fix:` `bc2ec099` including the README status flip and phase-doc updates; the tree is clean and the phase doc's status remains `in-progress` for the server to flip.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
der_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
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
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 11 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok
test webhook_ghost_e2e_deterministic ... ok

test result: ok. 10 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M23-client-decoupling/README.md` — +1 -1
- `docs/dev/milestones/M23-client-decoupling/phase-01c-file-op-error-label.md` — +43 -1
- `src/daemon/executor/file_ops/ops.rs` — +98 -3
- `src/daemon/executor/mod.rs` — +41 -3

**Commit:** bc2ec0990e2bb7270e3e7b457839a85c2b24f4c3

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-01

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (61 turns; the
  `(started)` entry says "Claude")
- **Reviewer re-run:** fmt, build and clippy `-D warnings` clean.
  `cargo test` gave lib **2095/0/14** on three runs, with bug_tracker 6,
  doc_truth 14, integration 32 and isolation 10 unchanged. The reviewer
  re-applied both mutations: A (untrimmed `error_chain`) and B (`{e:#}` in the
  give-up branch) both went red. The reviewer's re-run of the end-to-end block
  matched the pasted capture on every line except `lib1`, where the named
  pre-existing flake (`input_after_a_prompt_logs_a_redaction_marker`) fired,
  which the spec allows. The production code is the spec's dry-run text,
  verbatim.
- **Scope deviations:** none.
- **Evidence:** **the bare `PASTE MATCH` line is in the doc**
  (`grep -cx 'PASTE MATCH'` → 1). It was a counted criterion for the first
  time, after two phases reported the check without printing it, and it
  worked first try.
- **Calibration:** (1) **Fold C, fourth occurrence** of a wrong
  self-reported executor name ("Claude"). (2) **Making the self-check a
  `grep -cx` criterion worked** where prose had not: two misses, then a hit
  first time once it was counted. This matches the M12 lesson that the
  executor responds to conditions it can evaluate. Candidate for the M23
  close, alongside fold C.

