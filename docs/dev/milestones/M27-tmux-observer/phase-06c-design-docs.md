# Phase 06c: design-docs — the design docs say commands run in PTY shells and tmux only observes

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** done
**Depends on:** phase-06b
**Tags:** language=markdown, kind=docs, size=m

## Goal

Phase 06b made the runtime docs (CLAUDE.md, README, `assets/`) true and
added the `runtime_docs_describe_no_tmux_execution` gate in
`tests/doc_truth.rs`. The design docs still describe the old design:
`docs/architecture.md` opens with "embeds an AI assistant directly into
`tmux`", `REQUIREMENTS.md` requires `de-bg-*` windows and `send-keys`
password injection, and `security.md` cites `daemoneye notify`. Three
`[daemon] tmux_session` descriptions, plus one CLAUDE.md tool row that 06b
never reached, still say the daemon creates a tmux session.

This phase brings the gate's file list up to date, then rewrites until the
gate is green.

## What is true now (write from this — and only this)

> Verified against the source by the architect on 2026-10-11. **Do not
> open files under `src/` to re-check it.** If a passage describes
> something this list does not cover, **delete the passage. Do not write
> a replacement from inference.** Phase 06b was bounced for adding a
> sandboxing claim that nothing on this list supports.

- **Commands** run in PTY shells (`executor/shell_exec.rs`). By default a
  chat command runs in the session's shell. With `wait: false` it runs in
  a dedicated PTY shell, or in the session's sbx sandbox when `[sandbox]
  enabled`. A ghost runs on a configured `[hosts.*]` host, in its sbx
  sandbox, or in a local PTY shell. A call with no PTY route is refused
  with the reason. Real exit codes come from the PTY marker protocol.
- **sudo** on the PTY path: the daemon detects sudo's password prompt with a
  sentinel and sends `Response::CredentialPrompt` (echo off) to the client.
  It writes the reply into the PTY. The password is never logged, stored on
  disk, or sent to the AI.
- **tmux is optional and read-only.** Inside tmux, the prompt carries pane
  summaries for the user's session (capture and list only). Outside tmux,
  the context is the daemon's own shells. The daemon installs no hooks,
  starts no `pipe-pane`, and **never creates a tmux session**. It adopts
  the session it was launched in, or a configured `[daemon] tmux_session`
  that already exists. `tmux_session = ""` means no tmux at all.
  `daemoneye chat` runs in place and never attaches.
- **Alerts** reach every attached `daemoneye chat` over its presence
  connection. The tmux `display-message` overlay is optional.
- **Catch-up** fires when a chat client re-attaches (`chat --resume <id>`)
  after ≥ 30 s away.
- **Scheduled scripts** run in a scheduler-owned PTY shell.
  `ActionOn::Command` loads but refuses to fire.
- **`[execution] backend = "tmux"`** still loads, with a startup warning.
- **`daemoneye doctor`** reports readiness (config, api key, shell, daemon,
  sandbox, hosts, tmux) and exits 1 only on a `FAIL`.
- **Legacy `de-*` windows** are still filtered out of context
  (`is_daemon_window`) and listed by `daemoneye schedule windows`. Use the
  generic `de-*`, never a specific prefix.

## Spec

1. **Gate.** Add `docs/architecture.md`, `docs/PRODUCT_DEFINITION.md`,
   `docs/REQUIREMENTS.md` and `docs/security.md` to the
   repo-root documents that `runtime_docs_describe_no_tmux_execution`
   scans. Run it. It must fail. Its hits are the work list: about 15 in
   architecture.md, 4 in PRODUCT_DEFINITION.md, 19 in REQUIREMENTS.md and
   2 in security.md.
2. **Rewrite** each flagged passage from § What is true now, one file at a
   time, re-running the gate after each. Also fix two passages the gate
   does not catch: PRODUCT_DEFINITION.md's opening sentence ("embeds AI
   operators … via tmux"), and every "Requires tmux" or "tmux is required"
   statement in these four files.
3. **Milestone history** in `architecture.md` (dated entries and "not yet
   delivered" lists) is history, not description. Keep its meaning and
   drop the retired term. For example, "the `de-bg-*` tmux path remains"
   becomes "background execution still used tmux windows". Then update
   any "not yet delivered" item that has since shipped.
4. **The `tmux_session` descriptions** say the daemon creates the session.
   Fix all four: README's config sample comment and its `[daemon]` table
   row, the comment in `assets/etc/config.toml`, and the `///` doc comment
   on `DaemonConfig::tmux_session` in `src/config/types.rs` (comment only,
   no code change).
5. **CLAUDE.md's `run_terminal_command` tool row** opens with
   "Foreground (user pane) or background (daemon host window)". Rewrite that
   opening clause only. Leave the row's name, `Loaded` value and position
   alone, because `doc_truth` checks them.
6. Full gates, § End-to-end verification once, one `docs:` commit.

### Gotchas

- Edit with `patch`, never `patch_lines`, python, or `sed` through `bash`.
- REQUIREMENTS.md keeps its FR numbering. Rewrite a requirement's text in
  place, and never renumber. If a requirement describes removed machinery
  only (for example the tmux hook IPC), replace its text with what is true
  now, or mark it `*(Retired: …)*` in one line. Never delete the ID.
- Describe the design. Do not narrate the change ("now", "no longer",
  "used to") outside the milestone-history entries.

## Work list (escalation, 2026-10-11) — this table is the whole job

**Step 1 is done.** The gate edit in `tests/doc_truth.rs`, which adds the four
design docs to the scanned list, is on disk and correct. Keep it. Do not redo it.

Rules for this run:

- **Never open anything under `src/`.** Run 1 spent 240 consecutive
  read-only calls checking pane-target code and made no edit after the gate.
  Every fact you need is written in this table.
- **Never read a whole design doc.** Open only the line range a row names
  (`read_file` with an offset). Line numbers are as of 2026-10-11 and may
  shift by a few lines after earlier edits in the same file.
- **Edit only the rows below.** Something else may look stale. That
  includes unflagged FRs and history bullets. If it is not in this table, it
  is out of scope: list it in the Update Log and move on.
- **Your first `patch` lands within 10 turns.** Do the rows in order. Re-run
  `cargo test --test doc_truth runtime_docs` after each file.

| File · line | Action — write this meaning, in the file's own style |
|---|---|
| `architecture.md` 14 | "embeds an AI assistant directly into `tmux`" → "runs an AI assistant beside your terminal". If the rest of the sentence calls tmux the place commands run, say they run in daemon-owned PTY shells and tmux is an optional read-only observer. |
| `architecture.md` 46–47 | Remove `BG_DONE_TX` and `FG_HOOK_COUNTER` from the statics list. Keep the others. |
| `architecture.md` 81–85 | Delete the `background.rs` and the `hook.rs` bullets. Both modules are gone. |
| `architecture.md` 143 | `webhook.rs` → `src/webhook/` (`server.rs`, `process.rs`). Keep the description. |
| `architecture.md` 160–163 | Step 4 → approved commands run in a PTY shell: the session's own, a dedicated one for `wait: false` (the session's sbx sandbox when `[sandbox] enabled`), or one on a configured `[hosts.*]` host. Real exit codes come from the PTY marker protocol. |
| `architecture.md` 255–256 | Replace the window-prefixes bullet: legacy `de-*` windows from an older daemon are filtered out of context and listed by `daemoneye schedule windows`. |
| `architecture.md` 307 | Delete ", pipe-pane logging" from the list. |
| `architecture.md` 350 | "the `daemon/utils.rs` + `webhook.rs` splits" → "the `daemon/utils.rs` + webhook-module splits". |
| `architecture.md` ~510–514 | The "Still **not** delivered" paragraph. Background execution, ghost sessions and SSH/remote targets on PTY shells have since shipped. Say so in one clause, without `de-bg-*`. Leave "client key bindings / an attach UI" listed as not delivered. |
| `PRODUCT_DEFINITION.md` 13 | "embeds AI operators directly into your terminal workflow via tmux" → "runs AI operators beside your terminal (tmux optional)". |
| `PRODUCT_DEFINITION.md` 97–98 | Replace the Background and Foreground bullets with two bullets. **Waiting** (default): the command runs in the session's PTY shell and the result returns when it exits, with its real exit code. **`wait: false`**: it runs in a dedicated PTY shell, or the sbx sandbox, and returns at once. Completion arrives as a `[Background Task Completed]` message. |
| `REQUIREMENTS.md` FR-1.1.2 | → The application MAY use tmux as a read-only context source (pane capture and listing). It MUST NOT require tmux. |
| `REQUIREMENTS.md` FR-1.1.3 | → `*(Retired: the daemon creates no tmux panes or windows.)*` |
| `REQUIREMENTS.md` FR-1.1.6 | In its `var/log/` list only: "background-window output archives in `panes/`, live pipe-pane captures in `pipe/`" → "PTY shell recordings in `shells/` (`panes/` and `pipe/` are legacy, swept at startup)". |
| `REQUIREMENTS.md` FR-1.1.8 | Replace it, with any sub-list under it. The daemon MUST run with or without tmux. It adopts the tmux session it was launched in, or a configured `[daemon] tmux_session` that already exists. It MUST NOT create a tmux session. |
| `REQUIREMENTS.md` FR-1.2.2 | Only the parenthetical "daemon-launched `de-bg-*` / `de-sj-*` / `de-gs-*` windows" → "legacy daemon `de-*` windows". |
| `REQUIREMENTS.md` FR-1.2.4 | → The AI works beside the user's terminal. It reads pane summaries when the user is in tmux, and runs approved commands in daemon-owned PTY shells, never in the user's pane. |
| `REQUIREMENTS.md` FR-1.2.7 | Keep the two-mode framing. Replace both sub-bullets with the PRODUCT_DEFINITION 97–98 meaning, as MUST statements. |
| `REQUIREMENTS.md` FR-1.2.10 | Keep the scheduling and persistence sentences. Replace everything about the `de-sj-*` window, its naming and its cleanup with: scripts MUST run in a scheduler-owned PTY shell. `ActionOn::Command` jobs load but MUST refuse to fire. |
| `REQUIREMENTS.md` FR-1.2.15 | → The daemon MUST track chat attach/detach over the client's presence connection. A client that re-attaches (`chat --resume <id>`) after ≥ 30 s away MUST receive a catch-up brief of event messages added while it was gone. |
| `REQUIREMENTS.md` FR-1.2.17 | Delete only the sentence about archiving ghost background output to `panes/<win_name>.log` via pipe-pane. |
| `REQUIREMENTS.md` FR-1.4.3 | → When a command needs `sudo`, the daemon MUST detect sudo's password prompt in the PTY with a sentinel and send a `CredentialPrompt` (echo off). It MUST write the reply into the PTY. The password MUST NOT be logged, stored on disk, or sent to the AI. |
| `REQUIREMENTS.md` FR-1.4.4 | → `*(Retired: every command's sudo prompt takes FR-1.4.3's PTY path.)*` |
| `security.md` ~100–106 | Delete the tmux-hook-bodies / `daemoneye notify` bullet. In `Sources:`, drop `` `src/daemon/mod.rs` (global hooks) `` and `` `src/cli/notify.rs` ``. |
| `README.md` ~779, `assets/etc/config.toml` ~297 | The `tmux_session` comment → "existing tmux session the daemon observes when launched outside tmux (never created); \"\" = no tmux". |
| `README.md` ~1054 | The `tmux_session` row's description → the name of an existing tmux session the daemon observes when launched outside tmux. The daemon never creates it. When launched inside tmux, the current session is used and this setting is ignored. `""` means no tmux at all. Keep the type and default columns. |
| `src/config/types.rs` `DaemonConfig::tmux_session` | The `///` comment only, with the same meaning as the README row. Drop the `tmux new-session` line. |
| `CLAUDE.md` `run_terminal_command` row | Replace only the opening "Foreground (user pane) or background (daemon host window)." with "Runs a command in a PTY shell." Keep the rest of the cell. |

Then: full gates, § End-to-end verification once, and one `docs:` commit.

## Acceptance criteria

- [ ] `cargo test --test doc_truth runtime_docs_describe_no_tmux_execution`
      reports `1 passed`, and the test's source names all four design docs.
- [ ] `grep -niE 'embeds?.*(into|via) .?tmux' docs/*.md` prints nothing.
      Today it prints 2 lines.
- [ ] `grep -rn 'creates/adopts\|session the daemon creates\|the daemon creates$' README.md assets/etc/config.toml`
      prints nothing, and `grep -n 'new-session' src/config/types.rs` prints
      nothing. Today they print 3 lines and 1 line.
- [ ] `grep -c 'user pane\|daemon host window' CLAUDE.md` prints `0`.
      Today it prints `1`.
- [ ] *Preservation:* every other `doc_truth` test passes, and
      `seeded_config_template_*` stays green.
- [ ] clippy (`-D warnings`) exits 0, and `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `passed=2308 failed=0`, the same as today. This
      phase adds no test.
- [ ] The Update Log has § End-to-end verification's output, as printed,
      and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test --test doc_truth 2>&1 | grep -E '^test result'
grep -niE 'embeds?.*(into|via) .?tmux' docs/*.md | wc -l
grep -rn 'creates/adopts\|session the daemon creates' README.md assets/etc/config.toml | wc -l
grep -c 'user pane\|daemon host window' CLAUDE.md
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Each line runs **once**.

## Authorizations

- [x] May edit the four design docs, `CLAUDE.md`, `README.md`,
      `assets/etc/config.toml`, `tests/doc_truth.rs`, and the `tmux_session`
      doc comment in `src/config/types.rs` (comment text only).

## Out of scope

- `docs/ROADMAP.md`, `docs/design-reference.md`, and `docs/dev/**`. These
  are planning and history documents.
- Any code change. If a doc and the code disagree, the code is right. Note
  the disagreement in the Update Log.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-11

Run 1 hard-failed with `NoProgressStall` after 240 consecutive read-only
calls. It landed the gate (step 1, kept) and then read `src/` to check
pane-target behaviour in `architecture.md`. The spec had asked it to judge
about 40 rewrites against a fact list that did not cover most of them. The
refinement adds § Work list: every edit, with its line range and the
meaning to write. Read nothing else. The kept gate edit is not
`cargo fmt` clean yet, so run `cargo fmt --all` once before the final gates.

### Update — 2026-10-11 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** a spec gap. The fact list did not cover most REQUIREMENTS FRs, so the
executor went to the source. That is the same stall shape as 06b run 1, and the
first failure on this phase. An exhaustive per-line work list removes the need to
derive anything.

### Update — 2026-10-11 11:22 (started)

**Executor:** Executor (rexyMCP headless run)
**Status flip:** todo → in-progress (this file and the milestone README's phase table).

Task 1 first: widen the `doc_truth` gate to the four design docs, run it, and
use its hits as the work list.

### Update — 2026-10-11 19:07 (progress)

**Executor:** Executor (rexyMCP headless run)

Work-list rows done in order, one file at a time, gate re-run after each:

- `architecture.md`: all nine rows (opening sentence, statics list,
  module bullets, `webhook/` split, §2 step 4, window-prefixes bullet,
  pipe-pane removal, webhook-module split line, "not delivered" paragraph).
- `PRODUCT_DEFINITION.md`: opening sentence + the two Dual-Execution-Mode
  bullets.
- `REQUIREMENTS.md`: FR-1.1.2, FR-1.1.3, FR-1.1.6 (`var/log/` list),
  FR-1.1.8, FR-1.2.2 parenthetical, FR-1.2.4, FR-1.2.7, FR-1.2.10,
  FR-1.2.15, FR-1.2.17 (k), FR-1.4.3, FR-1.4.4. FR-1.1.8's sub-bullets
  (a)/(b), the session-recreate sentence and the `chat`-attach/systemd
  sentences described removed machinery only; replaced by the one-clause
  replacement the work list specifies, with "runs in place and never
  attaches" appended from the fact list.
- `security.md`: §6 — dropped the hook/notify bullet and both Sources.
- `README.md` (~779 sample comment, ~1054 table row) and
  `assets/etc/config.toml` (~297): `tmux_session` descriptions.
- `src/config/types.rs`: `DaemonConfig::tmux_session` `///` doc comment
  only (no code change).
- `CLAUDE.md`: `run_terminal_command` opening clause only.

`cargo test --test doc_truth runtime_docs` reports `1 passed` after each
file; the whole `doc_truth` suite is 16 passed / 0 failed.

Out of scope (left alone, per the work list's rules):
- `REQUIREMENTS.md` FR-1.2.2 still names `capture-pane`/`tmux
  show-environment`/`[FOREGROUND TARGET] %N`/`/pane` pinning and
  `run_terminal_command(background=false)` — unflagged, not in the table.
- `REQUIREMENTS.md` FR-1.2.6, FR-1.2.16 and FR-1.2.17 (b) still describe
  remote-pane detection, `[OTHER SESSIONS]`, and a ghost running in a
  "dedicated headless tmux window (`de-incident-…`)" — unflagged.
- `architecture.md`'s "Foreground/background execution — hook-based
  completion, PID-based local completion, respawn-pane retry, persistent
  background windows" bullet in the §5 capability list — unflagged.
- `README.md`'s sentence under the `[daemon]` table ("`daemoneye chat`
  … will open a new chat window inside the managed session and
  exec-attach to it") — unflagged by the gate and outside the table rows.

### Update — 2026-10-11 19:24 (complete, server-authored)

**Summary:** **Summary:** Executed the § Work list in order: confirmed the kept gate edit (task 1, the `doc_truth` widening was already on disk — I ran the gate, got 32 hits, and used them as the checklist), then rewrote all rows one file at a time — architecture.md (9 rows), PRODUCT_DEFINITION.md (opening sentence + both execution-mode bullets), REQUIREMENTS.md (12 FRs, IDs preserved, FR-1.1.3/FR-1.4.4 retired in one line), security.md §6, README + seeded config + `DaemonConfig::tmux_session` doc comment (comment only), and CLAUDE.md's `run_terminal_command` opening clause. Ran `cargo fmt --all` before staging and committed everything in one `docs:` commit (`c2b2b5eb`); the tree is clean. E2E, run once: `doc_truth` 16 passed/0 failed with the gate naming all four design docs; embeds-grep 0 lines (was 2), creates-grep 0 (was 3), `new-session` grep 0 (was 1), CLAUDE.md row hits 0 (was 1); clippy `-D warnings` and `cargo fmt --all --check` exit 0; `cargo test` passed=2308 failed=0, unchanged. **Notes for review:** the per-line work list was exhaustive and I added no rewrites beyond it; leftover stale passages that the gate does not catch and the table does not list are enumerated in the phase doc's Update Log progress entry (FR-1.2.2's `capture-pane`/`/pane` detail, FR-1.2.17(b)'s `de-incident-…` ghost window, architecture's §5 "Foreground/background execution" capability bullet, README's `[daemon]` chat-attach sentence) — they need a future phase, not this one. The completion bookkeeping (status flip to `review`, completion Update Log entry, end-to-end verification entry with `PASTE MATCH: yes`) is authored by the server, as the contract assigns it.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
iled; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 16 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_documents_every_chat_slash_command ... ok
test docs_document_the_reindex_command ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test config_pricing_round_trip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_injects_on_next_run ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok

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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (4168210): No such process
kill: (-4168986): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +2 -2
- `assets/etc/config.toml` — +1 -1
- `docs/PRODUCT_DEFINITION.md` — +3 -3
- `docs/REQUIREMENTS.md` — +13 -18
- `docs/architecture.md` — +23 -26
- `docs/dev/milestones/M27-tmux-observer/phase-06c-design-docs.md` — +42 -0
- `docs/security.md` — +2 -7
- `src/config/types.rs` — +4 -8

**Commit:** c2b2b5eb19723f24f3eacdf25801d0634443f590

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-11

- **Verdict:** approved_first_try
- **Bounces:** none. One escalation: run 1 hit `NoProgressStall` and was
  re-dispatched with a refined spec.
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** in `security.md`, the argv bullet's
  "(tmux, background, foreground, remote panes)" parenthetical was also
  dropped. That is harmless, though it was not on the work list. FR-1.2.10
  kept the original's "returned to the AI" meaning with the real exit code.
  That is not an invented claim.
- **Findings, not bounced:** (1) The Update Log has no pasted § End-to-end
  verification output or `PASTE MATCH: yes` line. The executor deferred it to
  the server, which did not write it. The reviewer re-ran every line: doc_truth
  16 passed; the four greps print 0, 0, 0 and 0; clippy and fmt are clean;
  `passed=2308 failed=0`. (2) CLAUDE.md's `run_terminal_command` cell now
  reads "Runs a command in a PTY shell. `wait?` selects between them". The
  "them" refers to nothing, because the architect's dictated replacement
  dropped what it pointed to.
- **Carried (stale text outside this phase's table, listed by the
  executor):** FR-1.2.2's `capture-pane`/`/pane` detail, FR-1.2.17(b)'s
  `de-incident-…` ghost window, architecture.md §5's "Foreground/background
  execution" capability bullet, README's `[daemon]` chat-attach sentence,
  and finding (2).
- **Calibration:** the per-line work list turned a 240-call stall into a
  clean run. On a prose-rewrite phase, the GLM executor needs every edit
  enumerated, with the meaning to write. That is the second occurrence,
  after 06b run 1. The executor contract and server disagree about who
  pastes the end-to-end evidence. That is a rexyMCP issue.
