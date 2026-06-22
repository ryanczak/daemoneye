# M1 — Agent Tooling Improvements

**Goal:** Make every DaemonEye agent tool that touches files or runs commands work
correctly and safely on **both the local daemon host and SSH-connected remote
hosts**, fix the bugs and prompt gaps that prevent agents from using tools
effectively, leverage tmux features where they add reliability, and close the
security holes found in the tool-execution path.

**Status:** planning

**Depends on:** none

**Exit criteria:**

- `read_file`, `edit_file` (all operations), `write_script`, `write_runbook`,
  and the matching delete tools work against a remote SSH pane *and* the local
  daemon host, verified end-to-end.
- A remote-bound script (`ssh_target` set) is transferred to the remote host
  before execution; remote script execution succeeds end-to-end.
- No agent-supplied string (command, script name, path, ssh target) reaches a
  shell, an `ssh` invocation, or a `/etc/sudoers.d` rule without correct
  escaping/quoting. The injection cases below have regression tests.
- The credential-file blocklist and `~/.daemoneye/` write-block cannot be
  bypassed via symlinks or non-existent-path canonicalization.
- Memory/search namespace access is enforced against the calling agent's
  allowlist, not the caller-supplied namespace list.
- `assets/prompts/sre.toml` documents every shipped tool, the local-vs-remote
  (`target_pane`) decision, and the approval/ghost rules; tool-def schemas carry
  enum constraints and match their `PendingCall` variants.
- `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features
  -- -D warnings`, and `cargo test` all pass.

## Architecture references

- `docs/architecture.md#13-ai-provider-layer` — the `TOOLS` slice and tool model.
- `docs/architecture.md#3-the-ghost-shell-subsystem` — policy gates that tools run under.
- `docs/architecture.md#4-non-goals` — "file tools must work local + SSH" extends the
  remote-via-tmux model; cross-host stays runbook/pane-mediated.
- `docs/design-reference.md` — deep implementation detail (foreground/background
  completion, load-buffer/save-buffer remote reads, hook formats).

## Phases

Phases are subsystem-scoped; each carries its own security hardening (per the
"interleave" decision in Notes). Drafted on demand via `/rexymcp:architect next`.

| #  | Phase                                                              | Status |
|----|-------------------------------------------------------------------|--------|
| 01 | safe-remote-command-foundation (SSH escaping + shared helper)     | done   |
| 02 | remote-file-op-parity ([phase-02](phase-02-remote-file-op-parity.md))   | done   |
| 03 | script-exec-hardening ([phase-03](phase-03-script-exec-hardening.md)) — sudoers quoting + script-name allowlist | done   |
| 04 | remote-script-transfer (ghost `ssh_target` script push)           | todo   |
| 05 | write-tool-target-pane-parity (write/delete script + runbook)     | todo   |
| 06 | namespace-access-control (memory/search ACL)                      | todo   |
| 07 | execution-robustness-and-tmux (completion, exit code, tmux verbs) | todo   |
| 08 | prompt-and-tooldef-fixes (sre.toml + schema constraints)          | todo   |

## Notes

### Milestone design decisions (2026-06-21)

- **Scope:** all four reviewed workstreams are in — remote/SSH parity, security
  hardening, agent prompt + tool-def fixes, tmux leverage + execution bugs.
- **Security sequencing = interleave.** Each subsystem's security fix lands in
  the phase that already touches that subsystem (SSH escaping in 01, path/symlink
  guards in 02, sudoers + script-name allowlist in 03, namespace ACL in 06),
  rather than a single
  up-front hardening phase. The two highest-severity bugs exist *today* and are
  pulled as early as possible (01 and 03) to minimize their window.
- **Remote model:** file/command tools reach remote hosts *through an existing
  SSH/mosh tmux pane* (`target_pane`) — DaemonEye does not open its own SSH
  connections for interactive tools. Ghost shells are the exception: they use
  `GhostPolicy.ssh_target` to wrap commands in `ssh <target> …`. This milestone
  must make both paths safe and complete.

### Confirmed findings inventory (pre-injection source for phase drafts)

Verified against code during the M1 review. Cite these `file:line`s when drafting
phases; re-verify line numbers at draft time (the tree moves).

**Phase 01 — safe remote command foundation**
- `policy.rs:148` `wrap_remote` — `format!("ssh {} '{}'", target, cmd)` wraps in
  single quotes with **no escaping**; a `'` in `cmd` breaks out → arbitrary
  remote execution (**CRITICAL, exists today**). A `shell_escape_arg()` helper
  already exists in the codebase (used for hook session names) — reuse/extend it.
- `policy.rs:116-135` `resolve_command` builds remote tilde paths; its output is
  later fed to `wrap_remote` unescaped (`foreground.rs:886`). The single-quote
  wrap is the one fix point.

**Phase 02 — remote file-op parity & correctness**
- `file_ops.rs:36-44` `extract_marked` uses `rposition(|l| l.contains(end))` —
  file content containing the sentinel (`__DE_E__`) silently truncates output
  (**major**). Use exact-line match or unguessable markers.
- `file_ops.rs:657-666` remote `create` Perl fallback omits parent-dir creation
  (Python path has `os.makedirs`); nested-path create fails when Python3 absent
  (**major**).
- `file_ops.rs:197/370/992` path-traversal guard is a plain `contains("..")`;
  `canonicalize()` only runs when the path exists, so new-file/symlink paths skip
  symlink resolution → credential-blocklist / `~/.daemoneye/` write-block can be
  bypassed (**major, security**). Canonicalize the parent dir.
- `file_ops.rs:1000-1006` remote `copy` has a TOCTOU check→cp gap (**minor**).
- Binary/non-UTF-8 files: `read_to_string` fails locally; remote uses
  `from_utf8_lossy` (silent corruption) (**minor** — document or base64 fallback).

**Phase 03 — script-exec hardening** (sudoers quoting + script-name allowlist)
- `scripts.rs:127` `sudoers_rule` inserts `script_path` unquoted/unescaped into
  the NOPASSWD rule (**high, security**). Escape sudoers-special characters in the
  path so no path component can terminate the command or inject a directive.
- `scripts.rs:112-121` `validate_script_name` only rejects empty, `/`, NUL, `.`,
  `..` — it allows spaces and shell metacharacters. Tighten to a strict
  `[A-Za-z0-9._-]` allowlist (the script name is the agent-/user-controlled part
  that flows into the path used by both the sudoers rule and shell execution).

**Phase 04 — remote script transfer** (ghost `ssh_target` script push)
- `policy.rs:116-122` + `knowledge.rs:52-119` — remote script gap: `resolve_command`
  emits `~/.daemoneye/scripts/<name>` for `ssh_target`, but `write_script` only
  writes to the daemon host; the script never reaches the remote → remote script
  execution silently broken (**critical functionality gap**). Add transfer (scp /
  `ssh host 'cat > …'`) before execution.

**Phase 05 — write-tool target_pane parity**
- `tools.rs` — `write_script`/`write_runbook`/`delete_script`/`delete_runbook`
  lack `target_pane`; only `read_file`/`edit_file`/`run_terminal_command` have it.
  Add for local+SSH parity. Wide blast radius: 4 `PendingCall` variants across
  `types.rs`/`tools.rs`/`stream.rs`/`ghost.rs`/`executor/mod.rs` + 3 backends.

**Phase 06 — namespace access control**
- `knowledge.rs:521-537` (`read_memory`) and `knowledge.rs:604-607`
  (`search_repository`) trust the caller-supplied `namespaces` slice; no check
  against the agent's identity → an agent can read another agent's namespace
  (**medium, security**). `agents/mod.rs` has an unused `read_namespaces` field —
  populate and enforce it.

**Phase 07 — execution robustness + tmux leverage**
- `foreground.rs:650-689` local completion: `saw_child`/PID-return loop can
  false-early-exit on very fast commands and has a too-short start window
  (**high**). Consider `pane_dead_status` (tmux already tracks `pane_dead`).
- `foreground.rs:734` exit-code capture falls back to `0` when the shell hook
  didn't write `DE_EXIT_*` → wrong success reports (**high**).
- `mod.rs:856-868` manual pane-selection list does not exclude the chat pane →
  agent/user can run a command in the chat pane (**medium**).
- `foreground.rs:193-221` C3b stale-pane guard reads the (up to 2 s stale) cache;
  query tmux directly for the specific pane (**medium**).
- `foreground.rs:482-484` sudo-prompt timeout leaves the remote `sudo` waiting;
  send `C-c` on timeout (**medium**).
- `knowledge.rs:796-878` `watch_pane` removes its tmux hook only inside the spawn
  body — cancellation leaks zombie hooks; use a `Drop` guard (**medium**).
- tmux leverage opportunities: `wait-for` for completion signalling, `set-buffer`/
  `paste-buffer` for large/binary remote transfer, `copy-mode`+`send-keys -X` for
  scrollback extraction, `if-shell` for atomic file-existence checks. Apply where
  they replace fragile polling — not as blanket rewrites.

**Phase 08 — prompt + tool-def fixes**
- `assets/prompts/sre.toml` omits 9 shipped tools: `create_agent`/`read_agent`/
  `list_agents`/`delete_agent`, `read_script`/`list_scripts`, `read_runbook`/
  `list_runbooks` (**high — undiscoverable**).
- No JSON-schema `enum` constraint for `edit_file.operation`, `search_repository.kind`,
  `add_memory.category` — descriptions only; add an `enum_values` to the param
  renderer (**low**).
- `tools.rs` `auto_approve_scripts` typed `Str` but documented "JSON array" —
  clarify the accepted format (**low**).
- Prompt under-explains the local-vs-remote `target_pane` decision for file tools
  and the ghost background/approval rules (**medium**).
- Cross-check every `ToolDef` param against its `PendingCall` variant in
  `types.rs` for drift while here.

### Retrospective

(Filled in at milestone close, before any M2 phase 01.)
