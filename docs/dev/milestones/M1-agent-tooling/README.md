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
- `assets/prompts/sre.toml` documents the **core** tool set, the local-vs-remote
  (`target_pane`) decision, and the approval/ghost rules; tool-def schemas carry
  enum constraints and match their `PendingCall` variants. Rarely-used tools are
  **deferred** — discoverable and loadable on demand via `load_tools` (phase-11),
  not resident in every request.
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
| 05 | interactive-remote-script-exec ([phase-05](phase-05-interactive-remote-script-exec.md)) — daemon-host script streamed into a remote *user* pane; the non-ghost analogue of 04 | done   |
| 06 | namespace-access-control ([phase-06](phase-06-namespace-access-control.md)) — lock the memory/search ACL with regression tests (already enforced by construction) | done   |
| 07a | pane-targeting-and-cleanup-safety ([phase-07a](phase-07a-pane-targeting-and-cleanup-safety.md)) — chat-pane exclusion, live stale-pane guard, sudo-cancel C-c, watch_pane hook Drop guard | done   |
| 07b | completion-and-exit-code-correctness ([phase-07b](phase-07b-completion-and-exit-code-correctness.md)) — local completion via the `DE_EXIT` latch + non-zero exit surfacing (tmux-verb leverage split out, later renumbered → phase-10) | done   |
| 08 | prompt-and-tooldef-fixes ([phase-08](phase-08-prompt-and-tooldef-fixes.md)) — sre.toml teaches the § 2.4 model + enum schema constraints + `auto_approve_scripts` dual-format (re-scoped 2026-06-22: tool re-documentation moved to phase-11) | done   |
| 09 | error-suppress-audit ([phase-09](phase-09-error-suppress-audit.md)) — unwrap/expect/panic!/unsafe/#[allow] cleanup | done   |
| 10 | tmux-surface-and-safe-verbs ([phase-10](phase-10-tmux-surface-and-safe-verbs.md)) — stand-alone tmux-integration phase (was 07c): centralize inline buffer calls into `src/tmux/`, adopt `tmux wait-for` at the one daemon-host-local sentinel-poll site (`read_file` local buffer read); foreground path left untouched | done   |
| 11 | on-demand-tool-loading ([phase-11](phase-11-on-demand-tool-loading.md)) — split `TOOLS` into core + deferred via a self-declaring `ToolDef.deferred_group`; default render emits core only; a new `load_tools` tool pulls a deferred group into the session on demand (deferred schemas no longer ship every request) | done   |

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

### 07b/07c split (2026-06-22)

At 07b draft time the architect split the remaining phase-07 work again. 07b's
spec covers **only** the two `high` items — local completion detection (via the
`DE_EXIT` latch) and exit-code surfacing. The third bullet of the original 07b
scope — open-ended **tmux-verb leverage** (`wait-for`, `set-buffer`/`paste-buffer`,
`copy-mode -X`, `if-shell`) — was moved to a new **07c**, deferred and drafted on
demand. Rationale: each verb is a genuine redesign of a send/capture path (e.g.
`wait-for` requires wrapping the sent command so it signals a channel, which
changes what the user sees typed into their pane and breaks the interactive
path), and bundling an exploratory rewrite with the delicate completion-detection
change is exactly the risk that motivated the 07a/07b split in the first place.
**Update (2026-06-22):** 07c was subsequently promoted to a drafted stand-alone
tmux-integration phase and **renumbered → phase-10** (to put execution order in
numeric order, since it sequences after 08/09), scoped down to the safe slice
(buffer-call centralization + `wait-for` at the one daemon-host-local read site)
with the risky verbs deferred with reasons — see the phase-10 row and "→ Phase 10"
below.

### Tool discoverability: prose → on-demand loading (2026-06-22)

The principal engineer corrected the phase-08 premise. Phase-08 was drafted to
re-add the nine tools absent from `sre.toml` and add a test forcing *every* `TOOLS`
entry to be documented. But those tools were pulled **deliberately** to cut
context, and the real cost is the tool **JSON schemas**: all three backends send
the full `TOOLS` slice on every request (`body["tools"] = get_tool_definition()` →
`render_anthropic(TOOLS)`), so prose changes do not touch it. Re-documenting them
would re-bloat the prompt without addressing the schema cost.

Resolution:
- **Phase-08 re-scoped** to schema-correctness only (enum constraints +
  `auto_approve_scripts` dual-format + § 2.4 prose + ghost note). Its two
  discoverability tasks (re-document nine; `every_tool_is_named_in_sre_prompt`)
  were **removed**.
- **Phase-11 (on-demand-tool-loading) added** for true deferred loading: a
  self-declaring `ToolDef.deferred_group` splits `TOOLS` into core (always
  rendered) and deferred (omitted by default); a new core `load_tools` tool pulls a
  deferred group into the session, whose schemas then ship only on subsequent
  turns. Designed data-driven (adding a tool is a one-liner, compiler-forced to set
  the field) and unload-ready (a future `unload_tools` mirrors the arg shape).
  See [phase-11](phase-11-on-demand-tool-loading.md).

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

**Phase 06 — namespace access control** (re-scoped 2026-06-22)
- Original finding (**withdrawn as a live bug**): "`read_memory`
  (`knowledge.rs:521`) / `search_repository` (`knowledge.rs:604`) trust the
  caller-supplied `namespaces` slice → an agent can read another agent's namespace."
  Re-verification shows this is **not reachable**: none of the read tools
  (`read_memory`/`list_memories`/`search_repository`) expose a namespace parameter,
  and the slice is built **server-side** by `build_memory_namespaces()`
  (`executor/mod.rs:78`) from the agent's own config (own namespace +
  `read_namespaces` + `"global"`). `read_namespaces` is **not** unused —
  `build_memory_namespaces:95` already grants it (wired in G2, commit `b7025ba`,
  predating the M1 review). The ACL is enforced by construction.
- Re-scoped (principal engineer, "lock with tests, no code change"): phase-06 is now
  **test-only** — regression tests that pin three negative properties (no namespace
  tool param; storage layer reads only the supplied slice; `build_memory_namespaces`
  excludes foreign namespaces). No production change. See
  [phase-06](phase-06-namespace-access-control.md).

**Phase 07 — execution robustness + tmux leverage** (split 2026-06-22 into 07a/07b;
six findings + open-ended tmux leverage is too large/risky for one executor session,
and it mixes safe mechanical fixes with delicate completion-detection changes).

*Line numbers below were re-verified against the current tree at the 07a draft
(2026-06-22); the tree moved since the M1 review — phases 04/05 reshaped
`foreground.rs`.*

**→ Phase 07a — pane-targeting & cleanup safety** (the four `medium` mechanical fixes):
- `mod.rs:856-868` `find_best_target_pane` builds the manual pane-selection list
  from the cache **without excluding the chat pane** → the user can pick (or the
  default can become) the chat pane, running a command in the conversation pane
  (**medium**).
- `foreground.rs:193-221` C3b stale-pane guard reads the (up to 2 s stale) cache
  (`cache.panes.read()`); a pane closed < 2 s ago still passes, and a just-created
  pane is falsely rejected. Query tmux directly via `crate::tmux::pane_exists(tp)`
  (**medium**).
- `foreground.rs` local sudo flow: on the **Cancelled (password-prompt timeout)**
  path (`SudoFail::Cancelled`, ~line 545) `sudo` is left sitting at the password
  prompt in the user's pane. Send `C-c` to the target pane before returning the
  error so the pane returns to a clean shell (**medium**). (The `AuthExhausted`
  path needs no C-c: `sudo` exits itself after 3 wrong attempts. The remote path
  switches focus and never injects, so there is no injected prompt to abort —
  finding's original "remote" wording is stale vs. the current tree.)
- `knowledge.rs:876-878` `watch_pane` uninstalls its `pane-title-changed[@de_wp_N]`
  hook only inside the spawned task body, *after* the `timeout(...).await`; a task
  abort or panic leaks the hook. Mirror the existing `FgHookGuard`
  (`foreground.rs:50-84`) with a `Drop` guard moved into the task (**medium**).

**→ Phase 07b — completion & exit-code correctness** (the two `high` items; drafted
2026-06-22 — [phase-07b](phase-07b-completion-and-exit-code-correctness.md)). The
open-ended tmux-leverage bullet was **split out to 07c** at draft time (see Notes
§ "07b/07c split"):
- `foreground.rs:696-735` local completion: the `saw_child`/PID-return loop can
  false-early-exit on very fast commands and has a too-short start window
  (`LOCAL_CHILD_START_WINDOW = 300ms`) (**high**). The `DE_EXIT_<pane>` env var
  written by the shell hook is a more reliable completion signal than PID-return
  for the user's (non-`remain-on-exit`) foreground pane; `pane_dead_status` does
  **not** apply to a live user pane. → 07b clears the latch before send and polls
  for its reappearance as the exact primary signal, PID-return as the no-hook
  fallback; widens the start window to 750ms.
- `foreground.rs:780` exit-code capture (`read_pane_exit_status(...).unwrap_or(0)`)
  fabricates `0` when the hook didn't write `DE_EXIT_*`, and the captured code is
  only fed to `finish_command` (stats) — it never reaches the AI, so the model
  cannot tell a failed command from a clean one (**high**). → 07b annotates the
  `ToolResult` with the real non-zero code (local pane only; unknown/clean stay
  silent — never fabricate success).

**→ Phase 10 — tmux-surface-and-safe-verbs** (drafted 2026-06-22 as a stand-alone
tmux-integration phase, renumbered from 07c —
[phase-10](phase-10-tmux-surface-and-safe-verbs.md)).
A code survey (recorded in the phase doc's Current state) found most tmux polling
lives on the foreground completion path 07a/07b just hardened — high-risk to touch
for marginal gain — so the phase takes the low-blast-radius slice: (1) centralize
the three inline `tmux` buffer subprocess calls (`save-buffer`/`delete-buffer`) in
`file_ops.rs` into `src/tmux/` wrappers, and (2) replace the one **daemon-host-local**
sentinel-poll loop (the `read_file` local buffer read) with native `tmux wait-for`,
designed so a lost/raced signal degrades to the prior behavior. **Deferred with
reasons** (in the phase's Out of scope): `wait-for` on the foreground path (touches
hardened 07a/07b code); `if-shell` for existence checks (they run on the *remote*
host's shell, unreachable by the daemon's tmux server); `set-buffer`/`paste-buffer`
(no consumer — tmux buffers are daemon-host-local so they don't fix remote
binary/large transfer, which is its own future phase); the remote read sentinel
(remote shell can't signal the daemon's tmux server).

**Phase 08 — prompt + tool-def fixes** (discoverability finding moved to phase-11)
- `assets/prompts/sre.toml` omits 9 shipped tools:
  `create_agent`/`read_agent`/`list_agents`/`delete_agent`,
  `read_script`/`list_scripts`, `read_runbook`/`list_runbooks`, **`delete_memory`**.
  Originally framed as "undiscoverable, re-document them"; **re-scoped 2026-06-22**
  — these are the deferred set handled by **phase-11** (on-demand loading), not
  re-documented in 08. See the "Tool discoverability" note above.
- No JSON-schema `enum` constraint for `edit_file.operation`, `search_repository.kind`,
  `add_memory.category` — descriptions only; add an `enum_values` to the param
  renderer (**low**).
- `tools.rs` `auto_approve_scripts` typed `Str` but documented "JSON array" —
  clarify the accepted format (**low**).
- Prompt under-explains the local-vs-remote `target_pane` decision for file tools
  and the ghost background/approval rules (**medium**).
- Cross-check every `ToolDef` param against its `PendingCall` variant in
  `types.rs` for drift while here.

### Retrospective (2026-06-23)

**Outcome.** All eleven phases (01–11) landed. M1 delivered: remote/SSH command +
file parity (01–05), the namespace ACL (06), execution-correctness hardening
(07a/07b), the error-suppression audit (09), the tmux-surface refactor (10), the
prompt/tool-def fixes (08), and on-demand tool loading (11).

**What worked.**
- **Interleaved security sequencing.** Landing each subsystem's security fix in the
  phase that already touched it (SSH escaping in 01, path/symlink guards in 02,
  sudoers + allowlist in 03, namespace ACL in 06) kept diffs coherent and pulled the
  two highest-severity bugs to the front (01, 03). No regressions traced back to a
  deferred-hardening gap.
- **Mid-milestone model reset absorbed cleanly.** The 2026-06-22 remote-execution
  redirection (daemon-host is the only artifact store) reshaped 04/05 and *dropped*
  work (write-tool `target_pane` parity) rather than bolting it on. Reopening 04 to
  stream-by-default + sudo-only materialize was the right call.
- **Splitting risky phases paid off.** 07a/07b (and the 07c→phase-10 promotion) kept
  the delicate completion-detection change away from exploratory tmux-verb work.
- **Compiler-forced completeness in 11.** Making `deferred_group` a required field
  meant the core/deferred split could never be silently incomplete.

**What broke — calibration data.**
- **Phase 09 bounced once** (bug-phase-09-1) — a real but bounded miss, fixed on
  re-dispatch. One occurrence.
- **Phase 11 hard_failed and was taken over.** The local-LLM executor left a
  non-compiling tree *and*, more insidiously, two inert call sites: both the
  interactive (`stream.rs`) and ghost (`ghost.rs`) loops passed `Vec::new()` to
  `chat()` instead of the session's `loaded_tools`. The tree compiled and the
  partial unit tests (on the *pure* helpers) passed — so the headline feature did
  nothing while looking green. The takeover fixed both sites and added a seam test
  (`loaded_group_names_render_their_schemas`).

**Trend to watch (not yet a fix).** Two of eleven phases needed architect
intervention (one bounce, one takeover), both on the *back half* of the milestone
(09, 11) and both involving **the gap between "compiles + unit tests pass" and "the
feature actually runs"** — 09 was suppression cleanup, 11 was a value threaded
through a trait but ignored at the loop call site. One occurrence is data, two is a
trend. The candidate WORKFLOW.md fold: *when a phase threads a new value through a
trait/signature, an acceptance criterion must be a live end-to-end seam test that
fails if the loop call site drops the value — not just unit tests on the pure
helpers it feeds.* **Not yet folded** — per WORKFLOW.md this needs a third occurrence
or explicit user sign-off; raised here for the human's milestone-close review.

**Boundary.** M1 is complete. Human sign-off required before M2 is drafted.
