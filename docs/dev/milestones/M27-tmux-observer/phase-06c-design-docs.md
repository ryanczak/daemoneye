# Phase 06c: design-docs — the design docs say commands run in PTY shells and tmux only observes

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
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
