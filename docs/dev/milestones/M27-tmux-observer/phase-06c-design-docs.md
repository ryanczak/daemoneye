# Phase 06c: design-docs — the design docs say commands run in PTY shells and tmux only observes

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
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
meaning to write. Read nothing else.

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
