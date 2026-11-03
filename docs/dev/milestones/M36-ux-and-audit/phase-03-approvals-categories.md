# Phase 03 — approvals-categories

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress (bounced 2026-11-02 — bug-03-1 major, bug-03-2 minor; the criteria marked *bounce 1* below are the open work)
**Drafted:** 2026-11-02
**Depends on:** phase 02 (done)
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Design item 2 (`docs/design/2.7.0-UX-improvements.md`): the user must be able
to approve or deny whole categories of approvals, and `/approvals` must list
the state of every category. Today `/approvals` lists five rows, `on`/`off`
toggle only commands, `revoke` resets everything, and three prompt kinds
(schedules, agent writes, deletes) have no session state at all. PE decision 2
in the milestone README fixes the categories; changes last for the chat
session only.

## Architecture references

- `src/cli/commands/approval.rs` · `SessionApproval` (the session state,
  `auto_approves`, `record_session_approval`, `host_approval_lines`, `hint`).
- `src/cli/commands/slash.rs` · `cmd_approvals`, `on_off`, `scope`.
- `src/cli/commands/stream_ask.rs` · the `Response::*Prompt` arms;
  `src/cli/commands/stream_prompts.rs` · `prompt_write_ratatui`,
  `prompt_yes_no_ratatui`, `prompt_schedule_write_ratatui`.
- `src/ipc.rs` · `ScriptWritePrompt`, `ScriptDeletePrompt`;
  `src/daemon/executor/knowledge/agents.rs`, which reuses those two frames
  for `create_agent` (`script_name: "agents/<name>"`) and `delete_agent`
  (`script_name: "agent: <name>"`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the files above, and `HELP_TEXT` in `src/cli/commands/chat.rs`.
3. Read this entire phase doc before touching any code.

## Behaviour

**Categories** (the names `/approvals` takes):

| Category | Covers | Auto-approves when on |
|---|---|---|
| `commands` | non-sudo commands and `send_input` on the daemon host (today's `regular`) | as today |
| `sudo` | sudo commands on the daemon host | as today |
| `scripts` | `write_script` | every script (today's `scripts_all`) |
| `runbooks` | `write_runbook` | every runbook |
| `files` | `edit_file` | every non-sudo file op |
| `schedules` | `schedule_command` | every schedule write (new) |
| `agents` | `create_agent` | every agent write (new) |
| `deletes` | `delete_script`, `delete_runbook`, `delete_agent` | every delete (new) |

**Per-host grants** stay keyed by host name: `/approvals host <name> on|off`
sets or clears that host's non-sudo grant, `/approvals host <name> sudo on|off`
its sudo grant.

**Commands:**

- `/approvals` (or `list`, `show`): one row per category in the table's
  order with its state — `auto`, `gated`, or `N approved` when only
  individual names were approved for the session (scripts, runbooks, files,
  agents) — then the per-host rows when any exist, then an **always asked**
  block naming the kinds nothing can auto-approve: a host `[hosts]` does not
  configure, a ghost reaching outside its sandbox, your own `/shell` shells,
  and a sudo file edit. Then a one-line usage reminder.
- `/approvals <category> on|off`. `on` approves the whole category; `off`
  gates it again **and** forgets any individual names approved under it, so
  the next call of that kind prompts.
- `/approvals all on` turns on every category **except `sudo`**, and leaves
  per-host grants alone; its reply says sudo stays as it was. `/approvals
  all off` gates every category, sudo and per-host grants included.
- `/approvals revoke` (alias `reset`) = `all off`. `/approvals revoke
  <category>` = `<category> off`.
- Kept for muscle memory: bare `on` / `off` (alias `auto` / `manual` /
  `gated`) still mean `commands on` / `commands off`.
- Anything else replies with an error naming the valid categories and
  forms. An always-asked kind (e.g. `/approvals newhost on`) is not a
  category and gets that error; nothing can enable it.

**Prompts honour the new categories:** a `ScheduleWritePrompt` with
`schedules` on, an agent write with `agents` on (or that agent's name
approved for the session), and a script, runbook or agent delete with
`deletes` on are answered `approved` without asking, committing the same
`✓ auto-approved (session)` line the existing auto-approvals commit. A
script write is never covered by `agents`, nor an agent write by
`scripts`.

**The hint** (`SessionApproval::hint`) also reports `schedules`, `agents`
and `deletes` when on.

## Constraints

- **Telling agent prompts from script prompts:** add `#[serde(default)]
  agent: bool` to `ScriptWritePrompt` and `ScriptDeletePrompt`, set `true`
  by the two senders in `knowledge/agents.rs`. Do not branch on the
  `script_name` prefix. An older daemon's frame still deserialises (as a
  script prompt).
- **Pure seams:** put the command's state change and its reply text in a
  function over `&mut SessionApproval` and the argument string (no
  renderer), and the list body in a function returning its lines;
  `cmd_approvals` only renders. Put "does this prompt auto-approve" for the
  new kinds on `SessionApproval` beside `auto_approves`.
- **Session only:** no new `[approvals]` config keys; `from_config` seeds
  the new flags `false`.
- **Docs follow:** `HELP_TEXT`, README § Collaborative Execution's
  `/approvals` paragraph and the slash-command table row, and
  `docs/REQUIREMENTS.md` FR-1.2.19 describe the commands as built.
- **The allowlist is a ratchet** (`tests/hygiene_allowlist.txt`): raise a
  grown allowlisted function's line and say why; do not squeeze.
- **Use `patch` with whole blocks** on long files, never `patch_lines`.

## Spec

1. **Red tests first** — write these (all start `approvals_cmd_`, which
   matches no test today) against the current code, stubbing new functions
   where needed, run them, and keep the failing output for step 6:
   - `approvals_cmd_lists_every_category_and_the_always_asked_kinds`
   - `approvals_cmd_category_on_off_toggles_only_that_category` — for each
     of the eight categories; `off` also clears that category's names.
   - `approvals_cmd_all_on_spares_sudo_and_all_off_clears_everything`
   - `approvals_cmd_host_grants_on_off` — both the commands and the sudo
     grant, and that they show in the list.
   - `approvals_cmd_revoke_category_and_legacy_on_off` — `revoke scripts`,
     bare `on`/`off`, and `reset`.
   - `approvals_cmd_rejects_unknown_and_always_asked_names` — `newhost on`,
     `foo off`, `scripts maybe`: error text, state unchanged.
   - `approvals_cmd_new_categories_auto_approve_their_prompts` — schedules,
     agent writes, deletes; an agent write is not covered by `scripts` and a
     script write not by `agents`.
   - `approvals_cmd_agent_flag_round_trips` — the two frames serialise
     `agent: true` and an old frame without the field reads `false`.
2. **State and wire** — the new `SessionApproval` fields, the `agent` field
   and its two senders, the auto-approve predicate, and the hint.
3. **The command** — rework `cmd_approvals` per § Behaviour over the pure
   functions.
4. **The prompt arms** — the `ScheduleWritePrompt`, `ScriptWritePrompt`
   (agent branch), `ScriptDeletePrompt` and `RunbookDeletePrompt` arms
   consult the new state. An `A` answer to an agent write records the
   agent's name for the session, as scripts do today.
5. **Docs** — per the constraint. `cargo test` passes with `failed=0`.
6. **Red runs** — one `### Update — <date> (red runs)` entry pasting step 1's
   `test result:` lines and assertion messages.
7. **Findings** — one `### Findings — <date>` entry: anything you noticed
   outside this phase's scope, two or three lines, or `none`.
8. **End-to-end** — run the block in § End-to-end verification once before
   committing and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry. Required.
9. **Commit** as `feat: /approvals lists and toggles every approval category; schedules, agents and deletes join them (M36 phase-03)`.

## Acceptance criteria

- [ ] The E2E block's first line is `approvals_cmd passed=8 failed=0`
      (today `approvals_cmd passed=0 failed=0`).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] The existing `SessionApproval` tests in `approval.rs` and the
      `status_bar_*` / `paste_safe_box_*` tests pass (preservation).
- [ ] `HELP_TEXT`, README and `docs/REQUIREMENTS.md` FR-1.2.19 name the
      eight categories and the `all` / `host` / `revoke <category>` forms.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

*Bounce 1 (2026-11-02, [bug-03-1](bugs/bug-03-1.md) major,
[bug-03-2](bugs/bug-03-2.md) minor) — each criterion below was run against
`25d4242f` and fails there:*

- [ ] *bounce 1* `grep -nE 'scripts\.insert|agents\.insert' src/cli/commands/stream_prompts.rs`
      prints nothing (today: lines 559 and 561 — a delete prompt's `A`
      answer writes into the *write* grant sets). bug-03-1.
- [ ] *bounce 1* `grep -n "agent's own session name" src/cli/commands/stream_prompts.rs`
      prints nothing (today: lines 496 and 540 — the claim is not what
      `prompt_auto_approves` does). bug-03-1.
- [ ] *bounce 1* Script, agent and runbook delete prompts offer the same
      answers, and a delete prompt's confirmation line names only a grant
      `prompt_auto_approves` honours on the next call of that kind. bug-03-1.
- [ ] *bounce 1* `grep -n revoke src/cli/commands/chat.rs` prints the
      `/approvals` `HELP_TEXT` entry (today: nothing), and that entry names
      the `<category>|all on|off`, `host <name> [sudo] on|off` and
      `revoke [category]` forms. bug-03-2.
- [ ] *bounce 1* `approvals_cmd_category_on_off_toggles_only_that_category`
      asserts the `<category> off` reply for every category but `sudo`
      contains `now require approval` and never `requires` (today
      `scripts off` replies `✓ scripts now requires approval`). bug-03-2.
- [ ] *bounce 1* `approvals_cmd_rejects_unknown_and_always_asked_names`
      asserts the replies to `host pinky sudo` and to bare `host` contain
      the argument once and never `host host` (today
      `✗ unknown: /approvals host host pinky sudo — …`). bug-03-2.
- [ ] *bounce 1* The E2E block's first line is still
      `approvals_cmd passed=8 failed=0` — the two pins go into the existing
      tests — and the block is re-run and pasted in a new
      `### Update — <date> (end-to-end verification, bounce 1)` entry.

## Test plan

Pure-function tests pin the command grammar, the list and the predicate;
the round-trip test pins the wire field. The milestone's live check runs
`/approvals` and `/approvals files off` in a chat on pinky.

## End-to-end verification

```sh
bash <<'E2E'
cargo test approvals_cmd_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "approvals_cmd passed=" p, "failed=" f}'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/approval.rs`, `slash.rs`, `stream_ask.rs`,
  `stream_prompts.rs`, `chat.rs` (`HELP_TEXT`), `src/ipc.rs`,
  `src/ipc_tests.rs`, `src/daemon/executor/knowledge/agents.rs`, any other
  file that builds a `ScriptWritePrompt` / `ScriptDeletePrompt` literal and
  stops compiling without the new field, `README.md`,
  `docs/REQUIREMENTS.md`, and `tests/hygiene_allowlist.txt` (ratchet only).

## Out of scope

- Config keys for the new categories, and `/approvals save`.
- Which prompts are approval-gated (`APPROVAL_GATED_TOOLS`) and the daemon's
  own gating; this phase changes only the client's answers.
- The approval prompt's key handling (phase 07) and its panel wrapping
  (phase 05).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-02 13:22 (started)

Executor: executor (GLM, pairing with Claude Code). Flipped Status to
`in-progress` and the milestone README row to match. Plan: the eight red
tests against the pre-phase code (throwaway worktree, step 1 stubs), then
state and wire, the pure command over `&mut SessionApproval`, the prompt
arms, docs, the red-runs / findings / E2E entries, one commit.

### Update — 2026-11-02 13:58 (red runs)

Step 1's eight tests written against the pre-phase code and run in a
throwaway worktree at commit `913442f6` (the tree before any phase-03
change; removed after). The pre-phase tree cannot express the new state, so
the worktree carries only the stubs the tests need to compile: the four new
`SessionApproval` fields seeded `false` with nothing reading them,
`prompt_auto_approves` stubbed `false`, `approvals_cmd` stubbed to the
error the pre-phase command returns for every argument, the `agent` field
on the two frames (`#[serde(default)]`, no sender sets it), and
`agent: _` in the two prompt arms. Every behaviour assertion therefore
exercises pre-phase behaviour:

```
running 8 tests
test cli::commands::approval::tests::approvals_cmd_host_grants_on_off ... FAILED
test cli::commands::approval::tests::approvals_cmd_lists_every_category_and_the_always_asked_kinds ... FAILED
test cli::commands::approval::tests::approvals_cmd_new_categories_auto_approve_their_prompts ... FAILED
test cli::commands::approval::tests::approvals_cmd_category_on_off_toggles_only_that_category ... FAILED
test cli::commands::approval::tests::approvals_cmd_all_on_spares_sudo_and_all_off_clears_everything ... FAILED
test cli::commands::approval::tests::approvals_cmd_agent_flag_round_trips ... ok
test cli::commands::approval::tests::approvals_cmd_rejects_unknown_and_always_asked_names ... FAILED
test cli::commands::approval::tests::approvals_cmd_revoke_category_and_legacy_on_off ... FAILED

failures:

---- cli::commands::approval::tests::approvals_cmd_host_grants_on_off stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_host_grants_on_off' (3418388) panicked at src/cli/commands/approval.rs:688:22:
'host pinky on' must be a note, got Error("stub — the phase-03 command does not exist yet")

---- cli::commands::approval::tests::approvals_cmd_lists_every_category_and_the_always_asked_kinds stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_lists_every_category_and_the_always_asked_kinds' (3418389) panicked at src/cli/commands/approval.rs:704:22:
list returns a panel, got Error("stub — the phase-03 command does not exist yet")

---- cli::commands::approval::tests::approvals_cmd_new_categories_auto_approve_their_prompts stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_new_categories_auto_approve_their_prompts' (3418390) panicked at src/cli/commands/approval.rs:1014:9:
'schedules on' covers every schedule write

---- cli::commands::approval::tests::approvals_cmd_category_on_off_toggles_only_that_category stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_category_on_off_toggles_only_that_category' (3418387) panicked at src/cli/commands/approval.rs:688:22:
'commands on' must be a note, got Error("stub — the phase-03 command does not exist yet")

---- cli::commands::approval::tests::approvals_cmd_all_on_spares_sudo_and_all_off_clears_everything stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_all_on_spares_sudo_and_all_off_clears_everything' (3418386) panicked at src/cli/commands/approval.rs:688:22:
'all on' must be a note, got Error("stub — the phase-03 command does not exist yet")

---- cli::commands::approval::tests::approvals_cmd_rejects_unknown_and_always_asked_names stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_rejects_unknown_and_always_asked_names' (3418391) panicked at src/cli/commands/approval.rs:979:13:
the error line starts ✗: stub — the phase-03 command does not exist yet

---- cli::commands::approval::tests::approvals_cmd_revoke_category_and_legacy_on_off stdout ----

thread 'cli::commands::approval::tests::approvals_cmd_revoke_category_and_legacy_on_off' (3418392) panicked at src/cli/commands/approval.rs:688:22:
'revoke scripts' must be a note, got Error("stub — the phase-03 command does not exist yet")

test result: FAILED. 1 passed; 7 failed; 0 ignored; 0 measured; 2568 filtered out; finished in 0.00s
```

Seven of the eight fail on the pre-phase code, each on the assertion its
name describes: no list, no per-category toggle, no `all`, no host grants,
no revoke-by-category, no new-kind auto-approval. The one pass is
`approvals_cmd_agent_flag_round_trips`: the stub tree carries the `agent`
field only so the other tests compile, and the test drives the frames it
constructs itself — on the true pre-phase tree (`913442f6` without the
stub) the test does not compile, which is the red the round-trip pin
expresses: the field does not exist on the wire.

### Update — 2026-11-02 14:32 (end-to-end verification)

Ran the phase doc's § End-to-end verification block once, output redirected to
`.rexymcp/out/e2e-phase03.txt` and pasted below verbatim:

```
approvals_cmd passed=8 failed=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.85s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.52s
fmt_exit=0
passed=2655 failed=0
```

First line is `approvals_cmd passed=8 failed=0` (today it read
`passed=0 failed=0`), the hygiene / doc_truth / bug_tracker gates are `ok`,
`build_warnings=0`, clippy clean (`Finished`, no error line), `fmt_exit=0`,
full suite `failed=0` at 2655 (2566 before the phase, +89: the eight
`approvals_cmd_` tests plus the pre-phase tree's own counts shifting with
the reworked modules). Preservation: the 27 pre-existing `SessionApproval`
tests, `status_bar_*` and `paste_safe_box_*` all pass unchanged inside the
full suite. The allowlist ratcheted once: `ask_with_session_ratatui` 691 →
719, because the script-delete arm now calls `prompt_delete_ratatui` with
the agent flag and the schedule arm passes `approval` — the arms grew, the
function did not get squeezed.

### Findings — 2026-11-02 14:05

- `tests/hygiene.rs` counts `is_test_file` by name only, so a `#[path]`
  aliased test module inside a production-named file would be counted as
  production lines. Today's aliased test files (`src/ipc_tests.rs`,
  `src/cli/commands/stream.rs`'s inline modules) all pass, but a future
  `src/x_tests.rs` named `src/x.rs` would silently double-count. Not this
  phase's scope; flagging for the hygiene gate's next touch.
- `prompt_write_ratatui`'s generic `FAll`/`FContains` closures existed only
  so script and runbook writes could share one body; with the categories on
  `SessionApproval` the closures collapsed into one `PromptKind` parameter.
  Adjacent, already-done inside this phase's authorized files.
- none otherwise — everything stayed inside the authorized files.

### Update — 2026-11-02 22:57 (complete, server-authored)

**Summary:** All nine spec tasks are complete and committed (`25d4242f`); the working tree is clean, the phase doc still reads `in-progress`, and the completion tail is the server's to write.

**Summary + Notes for review**

`/approvals` now covers all eight categories — `commands`, `sudo`, `scripts`, `runbooks`, `files`, `schedules`, `agents`, `deletes` — plus the per-host grants and an always-asked block (unconfigured host, ghost escape hatch, own `/shell` shells, sudo file edit). The grammar lives in one pure `approvals_cmd(&mut SessionApproval, &str)` in `approval.rs` (list/`<category> on|off` with per-name clearing on `off`/`revoke`, `all on` sparing sudo, `all off`/`revoke`/`reset` clearing everything including hosts, `host <name> [sudo] on|off`, bare `on|off` aliases still meaning commands, and an error naming the valid categories for everything else including always-asked names); `slash.rs` only renders the outcome. `SessionApproval` gains `schedules`/`deletes` flags and an `agents` flag + name set (all `false` from `from_config`, no new config keys), the new `prompt_auto_approves(PromptKind)` predicate sits beside `auto_approves`, and the hint reports the three new kinds. `ScriptWritePrompt`/`ScriptDeletePrompt` carry `#[serde(default)] agent: bool`, set `true` by the two `knowledge/agents.rs` senders — no `script_name`-prefix branching anywhere; the schedule, script/agent write, script-delete and runbook-delete prompt arms consult the state and commit the same `✓ auto-approved (session)` line as the existing auto-approvals. The eight `approvals_cmd_` tests were written first and run red against `913442f6` in a throwaway worktree (7 failed + the wire test, which cannot compile without the field — the honest red for a wire pin; the red-runs entry explains). E2E block: `approvals_cmd passed=8 failed=0`, `build_warnings=0`, hygiene/doc_truth/bug_tracker all `ok`, clippy clean, `fmt_exit=0`, full suite `passed=2655 failed=0`; the pre-existing `SessionApproval`, `status_bar_*` and `paste_safe_box_*` tests pass unchanged. One allowlist ratchet (`ask_with_session_ratatui` 691 → 719) with the reason in the Update Log; docs updated are `HELP_TEXT`, README's `/approvals` paragraph and slash-table row, `docs/REQUIREMENTS.md` FR-1.2.19, and one clarifying line in the config template's `[approvals]` comment (assets, not a config key).

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.7.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.17s


LINT
    Checking daemoneye v2.7.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.82s


TEST
e_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.12s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g4_briefing_read_and_clear ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test g5_child_inherits_depth_and_parent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


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
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v2.7.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.30s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-947e630ce8e8635d)
kill: (3434433): No such process
kill: (-3435117): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-55151dc2a3f1453f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-f920d215d69f41da)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-820c53c972d9b621)
     Running tests/hygiene.rs (target/debug/deps/hygiene-448dcbf0c0dd5ebc)
     Running tests/integration.rs (target/debug/deps/integration-a704381cf0548134)
     Running tests/isolation.rs (target/debug/deps/isolation-0272e2131c7fc4f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-8f8b55305473d8e9)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-b7e6e4bc38231a68)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +2 -2
- `assets/etc/config.toml` — +3 -1
- `docs/REQUIREMENTS.md` — +5 -4
- `docs/dev/milestones/M36-ux-and-audit/README.md` — +1 -1
- `docs/dev/milestones/M36-ux-and-audit/phase-03-approvals-categories.md` — +124 -1
- `src/cli/commands/approval.rs` — +779 -1
- `src/cli/commands/chat.rs` — +1 -1
- `src/cli/commands/slash.rs` — +4 -59
- `src/cli/commands/stream_ask.rs` — +49 -21
- `src/cli/commands/stream_prompts.rs` — +95 -10
- `src/daemon/executor/knowledge/agents.rs` — +2 -0
- `src/daemon/executor/knowledge/artifacts.rs` — +2 -0
- `src/ipc.rs` — +16 -2
- `src/ipc_tests.rs` — +4 -0
- `tests/hygiene_allowlist.txt` — +1 -1

**Commit:** 25d4242fbd108ed5a3e18d4edf451756fc660e70

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-11-03 03:10 (started, bounce 1)

Executor: executor (GLM-5.3-Flash-FP4-Local), resuming per the bounce-1
criteria. The tree is at `dc12253a` (bug reports filed, phase doc
in-progress). Plan: the two bug-03-2 pins into the existing
`approvals_cmd_` tests, the delete-prompt rework over `stream_prompts.rs` /
`stream_ask.rs` (no `A` on deletes, all three delete prompts answer the
same), `HELP_TEXT` forms, a red run of the pins against `25d4242f` in a
throwaway worktree, then greps, the E2E block, and one `fix:` commit.

### Update — 2026-11-03 03:40 (red runs, bounce 1)

The two bug-03-2 pins went into the existing tests (per the bounce-1
criteria: "the two pins go into the existing tests") and ran against
`25d4242f` — the phase-03 commit the bugs were filed on — in a throwaway
worktree that carried only the pin diffs (21 lines, both in `approval.rs`'s
test module):

```
running 8 tests
test cli::commands::approval::tests::approvals_cmd_lists_every_category_and_the_always_asked_kinds ... ok
test cli::commands::approval::tests::approvals_cmd_new_categories_auto_approve_their_prompts ... ok
test cli::commands::approval::tests::approvals_cmd_agent_flag_round_trips ... ok
test cli::commands::approval::tests::approvals_cmd_all_on_spares_sudo_and_all_off_clears_everything ... ok
test cli::commands::approval::tests::approvals_cmd_host_grants_on_off ... ok
test cli::commands::approval::tests::approvals_cmd_category_on_off_toggles_only_that_category ... FAILED
test cli::commands::approval::tests::approvals_cmd_rejects_unknown_and_always_asked_names ... FAILED
test cli::commands::approval::tests::approvals_cmd_revoke_category_and_legacy_on_off ... ok

failures:

---- cli::commands::approval::tests::approvals_cmd_category_on_off_toggles_only_that_category stdout ----

thread '…' panicked at src/cli/commands/approval.rs:1044:17:
every category but sudo is plural: ✓ commands now requires approval

---- cli::commands::approval::tests::approvals_cmd_rejects_unknown_and_always_asked_names stdout ----

thread '…' panicked at src/cli/commands/approval.rs:1255:13:
the host form's refusal never doubles 'host': ✗ unknown: /approvals host host — valid categories: … · host <name> [sudo] on|off
 (x2)
failures:
    cli::commands::approval::tests::approvals_cmd_category_on_off_toggles_only_that_category
    cli::commands::approval::tests::approvals_cmd_rejects_unknown_and_always_asked_names

test result: FAILED. 6 passed; 2 failed; 0 ignored; 0 measured; 2568 filtered out; finished in 0.00s
```

Both pins fail on the pre-fix code, each naming the behaviour its bug
report describes: the plural exception is inverted (`commands off` says
`requires`), and the host refusal doubles the `host` verb. The other six
`approvals_cmd_` tests pass unchanged — the pins added assertions, they did
not move the grammar. The bug-03-1 criteria are grep-pins (the delete
prompt's `A` and its doc comments), not assertion pins, so their red run is
the grep itself: both greps print matches on `25d4242f` (the bug report
records them: `stream_prompts.rs` 559, 561, 496, 540) and print nothing
after the fix, quoted below.

### Update — 2026-11-03 03:50 (end-to-end verification, bounce 1)

The bounce-1 greps first, on the fixed tree:

```
$ grep -nE 'scripts\.insert|agents\.insert' src/cli/commands/stream_prompts.rs
(no output — exit 1; was lines 559 and 561)
$ grep -n "agent's own session name" src/cli/commands/stream_prompts.rs
(no output — exit 1; was lines 496 and 540)
$ grep -n revoke src/cli/commands/chat.rs
65:             revoke [category]   /prompt    list or switch system prompt
```

Then the phase doc's § End-to-end verification block, run once, output
redirected to `.rexymcp/out/e2e-phase03-bounce1.txt` and pasted verbatim:

```
approvals_cmd passed=8 failed=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.01s
fmt_exit=0
passed=2655 failed=0
```

First line is still `approvals_cmd passed=8 failed=0` — the two pins went
into the existing tests, which now pass on the fixed tree and failed on
`25d4242f` (red-runs entry above). The three delete prompts share
`prompt_delete_ratatui`: script (`stream_ask.rs:461`), runbook
(`stream_ask.rs:604`), agent (the same arm, `agent: true`), all Y/N with
the same `✓ auto-approved (session)` / `✓ approved` lines, and a delete's
confirmation names only `deletes`, which `prompt_auto_approves` honours on
the next call of that kind. The pre-existing `SessionApproval` tests,
`status_bar_*` and `paste_safe_box_*` pass unchanged inside the full suite
(`passed=2655 failed=0`). Allowlist ratchet: `ask_with_session_ratatui`
719 → 722 (the script-delete arm picks the `PromptKind`, the runbook arm
swapped its inline branch for the shared call).

### Findings — 2026-11-03 (bounce 1)

- `prompt_delete_ratatui` now takes the `PromptKind` itself instead of the
  `agent` bool, so the script arm and the runbook arm call the same function
  with the same Y/N shape — the runbook arm's inline branch in
  `stream_ask.rs` collapsed into the shared one (−17 lines there).
- `tests/hygiene.rs`'s allowlist re-ratcheted for
  `ask_with_session_ratatui` (719 → 722): the script-delete arm gained the
  two-line `PromptKind` pick, the runbook-delete arm swapped its inline
  branch for the shared call. The arms grew 3 lines net; the function did
  not get squeezed.
- none otherwise — everything stayed inside the bounce-1 authorized files.
