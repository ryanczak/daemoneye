# Phase 03 — approvals-categories

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
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
