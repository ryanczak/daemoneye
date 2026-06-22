# M1 — Agent Tooling Improvements

**Goal:** Give DaemonEye agents the full agency of a human operator on **both the
local daemon host and SSH-connected remote hosts**, under one principle: the daemon
host stores all of DaemonEye's managed artifacts; remotes are *execution* targets,
never *storage* targets (architecture § 2.4). Make every tool correct and safe on
both sides of that line, fix the bugs and prompt gaps that block effective tool use,
leverage tmux features where they add reliability, and close the security holes in
the tool-execution path.

**Status:** planning

**Depends on:** none

**Exit criteria:**

- `read_file` and `edit_file` (all operations) — the operator-filesystem tools —
  work against a remote SSH pane *and* the local daemon host, verified end-to-end.
- Managed-artifact tools (`write_script` / `write_runbook` and the matching
  delete/read/list tools) are **daemon-host only by design** (§ 2.4): they carry no
  `target_pane` and never write to a remote. This is verified as a *negative*
  property (the tool schemas expose no remote target).
- A daemon-host script runs on a remote host via the **streamed, no-remote-disk**
  mechanism by default, and via the **persistent sudoers-path materialize** when the
  invocation requires `sudo`; both verified end-to-end at the wire-string level.
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
- `docs/architecture.md#24-remote-host-execution-model` — the daemon-host-storage
  principle and the three tool classes (managed-artifact / operator-filesystem /
  execution). This is the spine of the milestone.
- `docs/architecture.md#3-the-ghost-shell-subsystem` — policy gates that tools run under.
- `docs/architecture.md#4-non-goals` — no far-side daemon, no remote artifact storage;
  operator parity is the goal, remote-resident DaemonEye state is not.
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
| 04 | remote-script-execution ([phase-04](phase-04-remote-script-transfer.md)) — ghost `ssh_target`: stream by default, persistent materialize for sudo | done   |
| 05 | interactive-remote-script-exec ([phase-05](phase-05-interactive-remote-script-exec.md)) — daemon-host script streamed into a remote *user* pane; the non-ghost analogue of 04 | review   |
| 06 | namespace-access-control (memory/search ACL)                      | todo   |
| 07 | execution-robustness-and-tmux (completion, exit code, tmux verbs) | todo   |
| 08 | prompt-and-tooldef-fixes (sre.toml teaches the § 2.4 model + schema constraints) | todo   |
| 09 | error-suppress-audit ([phase-09](phase-09-error-suppress-audit.md)) — unwrap/expect/panic!/unsafe/#[allow] cleanup | todo   |

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

### Remote-execution model redirection (2026-06-22)

Mid-milestone the principal engineer reset the remote model (architecture § 2.4):
**the daemon host is the only place DaemonEye stores managed artifacts; remotes are
execution targets, never storage targets.** Rationale the model must survive: the
daemon may lack remote write privileges, the remote FS may be read-only, or its only
writable storage may be volatile. The goal is operator parity — an agent does on a
remote whatever a human at that pane could — *without* leaving DaemonEye state there.

Consequences for the phase plan:
- **`write_script` / `write_runbook` / `delete_script` / `delete_runbook` do NOT get
  `target_pane`.** They are daemon-host-only by design. The original phase-05
  ("write-tool target_pane parity") is **dropped** — there is nothing to build; the
  current daemon-host-only behavior is now correct-by-design. Verify it as a negative
  property only.
- **Phase 04 was reopened** (done → in-progress). Its persistent
  `~/.daemoneye/scripts/<name>` materialize assumed a writable, persistent remote
  home — exactly what the new constraints forbid as a *default*. Revised mechanism:
  **stream** hex-decoded content to `bash -s -- <args>` (no remote disk) by default;
  fall back to the **persistent materialize only when `sudo` is required** (a NOPASSWD
  sudoers rule needs a fixed authorized path — streamed stdin and `mktemp` cannot be
  pre-authorized). `remote_materialize_cmd` is retained but demoted to the sudo case.
- **Phase 05 repurposed** to *interactive* remote script execution: streaming a
  daemon-host script into a remote *user* pane (`send-keys`), the non-ghost analogue
  of phase 04. (Tentative — draft on demand; confirm scope at draft time.)
- **Phases 06/07 unchanged** in scope (ACL; execution robustness). **Phase 08** grows:
  `sre.toml` must teach the three tool classes and the local-vs-remote decision.

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

**Phase 04 — remote script execution** (ghost `ssh_target`; reopened 2026-06-22)
- Original gap (closed): `resolve_command` emitted `~/.daemoneye/scripts/<name>` for
  `ssh_target` but `write_script` only writes the daemon host, so the remote script
  never existed. Phase 04 (v1) added a persistent hex-materialize before execution.
- Reopen reason (model § 2.4): persistent remote materialize must not be the
  *default* — it assumes a writable, persistent remote home. Revised mechanism:
  **stream** hex-decoded content to `bash -s -- <args>` (no remote disk) by default;
  use the persistent `remote_materialize_cmd` **only when `sudo` is required** (fixed
  sudoers-authorized path). See phase-04 doc Spec for the worked design.

**Phase 05 — interactive remote script execution** (repurposed 2026-06-22)
- The original "write-tool target_pane parity" finding is **withdrawn**: per § 2.4,
  managed-artifact tools are daemon-host-only and do not get `target_pane`. Nothing
  to build; current behavior is correct-by-design.
- New scope: when a *user* pane is SSH'd to a remote, invoking a daemon-host script
  via `run_terminal_command` send-keys the bare name into the remote shell where the
  file does not exist (**functionality gap**, the interactive analogue of phase 04).
  Stream the script into the pane via the same hex-decode-to-`bash -s` idiom. Re-scope
  and confirm at draft time.

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
