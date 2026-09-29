# M21 — Host model & SSH

**Goal:** a configured remote host is an execution target the AI can name —
`open_shell { host }`, `run_terminal_command { host }`, `read_file` /
`edit_file { host }` — reached through an SSH session **in a local PTY shell**
using the user's own keys and agent, with every credential prompt (host key,
password, passphrase, **remote sudo**) surfacing through the approval client
and never reaching the shell log.

**Status:** in-progress — scoped 2026-09-18, **PE sign-off on the breakdown
2026-09-18** with two decisions: the live-verification host is **`pinky`**, and
`host?` on `spawn_ghost_shell` is in scope as row 08b. Rows 01a–01c and 04c–05 are `done`. **06 split into 06a/06b/06c at
drafting** (2026-09-23) — it was four features in one row. **06a is `done`** (2026-09-23,
`approved_first_try`): the approval panel names the host. **06b is `done`** (2026-09-23,
`approved_first_try`) — per-host session scope. **06c is `done`** (2026-09-25,
`approved_first_try`) — unconfigured hosts are gated rather than refused.
**All four 06 rows are `done`** — 06d closed 2026-09-26
(`approved_first_try`), so the approval and ghost-boundary half of the
milestone is complete. **07 split into 07a/07b at drafting** — the read path is one
`target_pane` branch, the write path is four. **07a is `done`** (2026-09-26,
`approved_first_try`) and **07b is `done`** (2026-09-27, `approved_after_1`) — `target_pane` is gone from the tool surface. **08 split into 08a/08b/08c at drafting** (2026-09-27, PE decisions in Notes); **08a is `done`** (2026-09-27, `approved_first_try`). **Two substrate defects surfaced at the next drafting** and got rows of their own ahead of the ghost move (PE 2026-09-27): **08b is `done`** (2026-09-27, `approved_first_try`; shell exit detection). **08f inserted ahead of 08c at 08c drafting** (2026-09-27, PE): the PTY password-prompt path matches its sentinel on the command's echo and re-sends a correct password into the shell — see Notes. **08f is `done`** (2026-09-28, `approved_after_1`). **08c is `done`** (2026-09-28, `approved_first_try`). **08d is `done`** (2026-09-29, `approved_first_try`; PE: both `hosts:` lists fence a spawn host, refused at spawn if unlisted or unconfigured, and a ghost's own command `host` is routed in the same row); **08e is `done`** (2026-09-29, `approved_after_1`). **09 split into 09a/09b/09c at drafting** (2026-09-29): `edit_file` has no sudo path, so the README's exit wording ("a remote `sudo` edit via `edit_file { host }`") could not be met. **PE: add sudo to `edit_file` first** (09b), then the live close (09c). Drafting also measured that **every edit widens the file's mode** (`0600` → `0644` locally, `0664` on pinky), which a sudo edit would make worse, so that fix is 09a. **09a is drafted** (2026-09-29). *Numbering note:* 08a's phase doc and review verdict say "08b" for the ghost PTY move and "08c" for spawn-ghost-host — those are now **08c** and **08d**. 06c owns making an unconfigured host allowed-but-gated;
06d owns ghost `hosts:` enforcement.

**Depends on:** M20 (the PTY shell substrate every remote shell is), M22
(`open_shell` / `list_shells` / the `[SHELLS]` block, and the two `host?`
deferrals this milestone closes). Not on M23 or M25.

**Exit criteria:**

- **The design's stated exit, live:** a remote `sudo` edit via `edit_file
  { host }` on **`pinky`** (the PE-named host), from a `daemoneye chat` turn, evidenced by
  the host shell's cast log carrying the marker-delimited edit **with the
  password absent from every `"i"` event**. Architect-run per the M14
  convention; the evidence anchor is the cast log plus the session JSONL.
- **`target_pane` is gone from the tool surface.** `grep -c 'name:
  "target_pane"' src/ai/tools/defs.rs` → **0** (now 2, on `read_file` and
  `edit_file` — M22's deferral). `host?` replaces it on both, and is added to
  `open_shell` and `run_terminal_command`. **42 tools, 28 core + 14 deferred,
  unchanged**; `tests/doc_truth.rs`, `CLAUDE.md` and `README.md` in sync.
- **`[hosts.<name>]` is real config:** parsed into `Config`, documented in
  `assets/etc/config.toml` so `seeded_config_template_documents_every_config_field`
  stays green, and rejected with a named error when `ssh` is missing.
- **ControlMaster is measured, not assumed:** the second shell to a configured
  host reuses the control socket under `var/run/ssh/` — connect time and the
  absence of a second auth prompt recorded in the phase's Update Log.
- **The approval panel names the host** for any remote command, and carries a
  distinct classification for a **new (unconfigured) host** — M22's other
  deferral, plus the design's "new host" approval class.
- **Local `sudo` on the default `pty` backend prompts through
  `CredentialPrompt`** — the pre-requisite for remote sudo, and a defect on the
  shipped default today (see Notes).
- **Ghost `hosts:` is enforced:** a runbook may open shells only on listed
  hosts; an unlisted host is refused and recorded, never opened.
- **`[SHELLS]` and `list_shells` name each shell's host.**
- All four gates green: `cargo fmt --all`, `cargo build`, `cargo clippy
  --all-targets --all-features -- -D warnings`, `cargo test`.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 "Host model" — the `[hosts]` shape,
  the PTY-as-transport decision, ControlMaster flags, new-host approval, remote
  file ops through the host's shell. § 3 "Security model deltas" — identity,
  `"i"`-event scrubbing, the four approval classifications, runbook `hosts:`.
  § 8 PE decision 3: *PTY for everything remote, including file ops.*
- `docs/architecture.md` § 2.4 — the remote execution model, **unchanged**:
  the daemon host stores every managed artifact; remotes are execution targets
  only.
- `docs/security.md` § 2 — shell logs at rest are raw and 0600; every read
  toward a model or client goes through `mask_sensitive`.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — `host?` on four tools is
  four passes through steps 4, 9, 10, 11 without a count change.

## Phases

Proposed, pre-measurement — **each will be re-derived and may split at
drafting on a measured seam**, as every M20 and M22 row did. **Twenty-five rows**
(01a, 01b, 01c, 02, 03, 04a, 04b, 04c, 04d, 05, 06a, 06b, 06c, 06d, 07a,
07b, 08a, 08b, 08f, 08c, 08d, 08e, 09a, 09b, 09c) — 04 has split **three times** at drafting, 06 once, 08 twice and 09 once, every time on a
measured seam, and 01c was inserted when the test gate went deterministically
red. Ordering: the
credential mechanism the PTY path lacks comes first because remote sudo, host
keys and passphrases all ride on it; config and the argv-carrying spawn path
are pure and unblock the SSH shell; the SSH shell unblocks everything remote;
the live close is last.

| # | Phase | Status |
|----|-------|--------|
| 01a | credential-prompt-on-pty ([phase-01a-credential-prompt-on-pty.md](phase-01a-credential-prompt-on-pty.md)) — detect a `sudo` password prompt on a **local** PTY shell and round-trip it through `Response::CredentialPrompt` / `CredentialResponse`, the mechanism the tmux path has and the PTY path has **none** of. **Drafting probe confirmed the defect live:** real `sudo` on a PTY shell emits `[sudo] password for matt: ` and **no end marker ever arrives** | done |
| 01b | input-scrub-on-prompt ([phase-01b-input-scrub-on-prompt.md](phase-01b-input-scrub-on-prompt.md)) — the shell host writes `[redacted]` instead of the credential's bytes while the screen shows a secret prompt. **Drafting measured the race:** the pump broadcasts to the daemon *before* it feeds the screen, so a screen-based check loses to a daemon that answers immediately — the flag is armed from the chunk, ahead of the broadcast | done           |
| 01c | prompt-echo-race ([phase-01c-prompt-echo-race.md](phase-01c-prompt-echo-race.md)) — `cargo test` is **deterministically red on master**: 01b's scrub test fails every full-suite run and passes in isolation, so no phase can meet the DoD until it is fixed. **Measured at drafting:** the secret detector arms on bash echoing the fixture's own command line, which contains the prompt text, and the 50 ms sleep that papered over it stopped sufficing when 04b added eight tests. Two candidate fixes prototyped and both failed — recorded in the spec so they are not repeated. **Dispatched ahead of 04c** | done        |
| 02 | host-config ([phase-02-host-config.md](phase-02-host-config.md)) — `src/host.rs` with `HostId`, `[hosts.<name>]` as a free-form `HostEntry` map on `Config`, `validate_hosts`, runbook `hosts:` frontmatter, template docs. Pure and additive — nothing executes. **Drafting probe decided the struct name:** `doc_truth` discovers config sections by a `Config`/`Entry` type suffix, so `HostSpec` is invisible to the gate and `HostEntry` is not | done      |
| 03 | shell-spawn-argv ([phase-03-shell-spawn-argv.md](phase-03-shell-spawn-argv.md)) — the spawn path carries a program **and its argv** end to end (`shell-host` clap → `run_host` → `PtyBackend::spawn` → `PtyShell::spawn` → `CommandBuilder`), and `ShellRecord` gains `host: HostId` across its nine sites. Local behaviour byte-identical. **Drafting probed the clap shape:** `last = true, num_args = 0.., allow_hyphen_values = true` round-trips a real ssh argv and still parses with no `--` | done      |
| 04a | ssh-shell ([phase-04a-ssh-shell.md](phase-04a-ssh-shell.md)) — `Host::Ssh` spawn through 03's argv path: `ssh_argv` builder, `ssh_dir()` + its lifecycle/audit rows, and the remote exit-variable decision. **No tool changes.** All the hard facts measured live against `pinky` at drafting: the marker protocol works over `ssh -tt`, exit codes 0/1/42 propagate, ControlMaster is 157 ms cold vs 4–5 ms warm | done      |
| 04b | ssh-tool-surface ([phase-04b-ssh-tool-surface.md](phase-04b-ssh-tool-surface.md)) — `open_shell { host? }`, plus the registry plumbing that makes it mean anything: `spawn_for_on_host` (additive) and a reuse scan that never hands a remote shell to a local command. **Split again at drafting**, on a measured seam: `spawn_ssh_host` has zero callers, every `ShellRecord.host` is hard-coded `Local`, and `marker_shell_name` / `list_shells` already read the field. With `open_shell { host }` the existing `run_terminal_command { shell }` already reaches the remote shell, so the one-shot `host?` form is its own row | done |
| 04c | host-keyed-reuse ([phase-04c-host-keyed-reuse.md](phase-04c-host-keyed-reuse.md)) — the plumbing `run_terminal_command { host }` needs, with **no tool surface**: `shell_for_owner` takes the host to reuse on, `AiEvent::ToolCall` becomes a struct variant before 04d adds a second adjacent `Option<String>`, and 04b's `open_shell_test_seam` becomes a real RAII guard. **Split at drafting on a measured defect:** `default_shell_for` calls itself a pure mirror of `shell_for_owner`'s reuse predicate but never reads `row.host`, so `[DEFAULT SHELL]` names a remote shell as where a bare command lands — probed `Some("s1")` on remote-only rows, and it is false | done |
| 04d | run-command-host ([phase-04d-run-command-host.md](phase-04d-run-command-host.md)) — `run_terminal_command { host? }` on 04c's plumbing: the `ParamDef`, the `PendingCall` field, the `AiEvent` field, the dispatch arm and the three doc rows. **Decides what `shell` + `host` together mean** — an eight-row table whose load-bearing line is that a `shell` id on a *different* host is refused, never relocated. **Measured at drafting:** the spawn closure is monomorphic, so the remote arm cannot share the local call — `open_shell`'s two-arm split is the quoted pattern. **42 tools unchanged**; `host?` is a parameter, and `doc_truth` is the gate on that | done |
| 05 | ssh-prompt-detection ([phase-05-ssh-prompt-detection.md](phase-05-ssh-prompt-detection.md)) — `PromptKind` gains `SshHostKey` / `SshPassword` / `SshPassphrase`; the answerable two reuse 01a's `CredentialPrompt` round-trip, the host key fails fast (row 06 owns the trust flow) and `Permission denied` ends the run instead of hanging. **All four prompt strings captured live off `pinky` at drafting** — each hangs to a 124 timeout today. **Measured hazard:** the host-key prompt contains the word *fingerprint* twice, so broadening `is_fingerprint_prompt` would misclassify every one; a negative test and a mutation pin it. **Measured blocker for row 09:** `pinky` is `(ALL) NOPASSWD: ALL`, so remote sudo there never prompts | done |
| 06a | prompt-classification ([phase-06a-prompt-classification.md](phase-06a-prompt-classification.md)) — `Response::ToolCallPrompt` gains a `classification` (`command` / `remote_shell` / `new_host` / `escape_hatch`), `ApprovalRequest` carries the host, and the client renders the class **naming the host** — the milestone's approval-panel exit criterion. Classifies and renders only; no gating change. **Measured at drafting:** `prompt_and_await_approval` runs *before* host resolution, so today you approve a command whose destination you were never shown and only then get `unknown host` | done |
| 06b | per-host-approval ([phase-06b-per-host-approval.md](phase-06b-per-host-approval.md)) — session `A` becomes per class **per host**, and `new_host` / `escape_hatch` never auto-approve. **Measured at drafting:** 06a threaded `Classification` into the prompt but it is read only for the panel body — `auto_approved` still consults the bare `regular`/`sudo` flags, and `commands = true` is the shipped default, so today a remote command is named in the panel and then auto-approved with no chance to refuse. Changes who is asked, not what is reachable | done |
| 06c | new-host-allowed ([phase-06c-new-host-allowed.md](phase-06c-new-host-allowed.md)) — a well-formed unconfigured host is **gated, not refused**: the malformed case is refused *before* the prompt, the unconfigured case reaches the panel as `new_host` and on approval connects using the bare name as its own ssh destination (`HostEntry.ssh` already documents a bare alias as valid, and `[hosts.pinky] ssh = "pinky"` proves it). `open_shell` keeps the **strict** lookup — a deliberate boundary with its own criterion. **The persist offer is deferred:** measured at drafting, the daemon has no config-write path and `toml` (not `toml_edit`) would discard 96 of the config's 193 lines | done |
| 06d | ghost-hosts-enforced ([phase-06d-ghost-hosts-enforced.md](phase-06d-ghost-hosts-enforced.md)) — a runbook's `hosts:` list becomes a boundary: `GhostPolicy` carries it, `may_use_host` decides, **both** doors are gated (the `Foreground` arm and `PendingCall::OpenShell`), and an unlisted host is refused naming the allowed set and recorded via `log_event`. **Empty stays unrestricted** — every runbook on disk omits `hosts:`, a mutation pins it, and `src/ipc.rs`'s "enforced in a later phase" note is deleted in the same row. Closes the milestone's ghost-`hosts:` exit criterion | done |
| 07a | read-file-host ([phase-07a-read-file-host.md](phase-07a-read-file-host.md)) — `read_file { host? }` reads through a **shell on the host** with marker-delimited output instead of scraping a pane; `target_pane` retired from `read_file` (2 → 1). Builds the shell transport 07b reuses. **Measured at drafting:** the marker protocol already exists (`build_remote_read_cmd` emits `__DE_S__`, `extract_marked` cuts it) — only the transport is wrong, `send-keys` then poll `capture-pane` every 300 ms | done |
| 07b | edit-file-host ([phase-07b-edit-file-host.md](phase-07b-edit-file-host.md)) — the **four** write branches (`ops.rs:35, 247, 391, 545`) move onto 07a's `run_in_host_shell`; `target_pane` retired from `edit_file`, taking the tool surface to **0** and closing that exit criterion. Deletes the pane era — **but not `get_pane_remote_host`**, which the grep at drafting showed still has two live callers in `prompt.rs` and `foreground.rs` (07a's doc got this wrong; corrected here). A mutation pins that the host path still prompts for approval. Bounced once ([bug-07b-1](bugs/bug-07b-1.md), closed); `approved_after_1` | done |
| 08a | ghost-target-resolution ([phase-08a-ghost-target-resolution.md](phase-08a-ghost-target-resolution.md)) — at ghost start a runbook's `ssh_target` is **resolved against `[hosts]`** (key, else a unique `ssh`-destination match; ambiguity resolves to neither) and recorded as `ghost_ssh_target_resolved`. On today's transport, `HostEntry.sudo_scripts` becomes **enforced** (empty = unrestricted, mutation-pinned) and a non-empty `hosts:` must grant the resolved host. **Split at drafting on measured seams** (PE decisions 2026-09-27): half the old row was already done — `sudo_scripts` / `sandbox_profile` have parsed since 02 — `ssh_target`'s documented `user@host` cannot parse as a `HostId`, and ghosts never touch the PTY substrate. **Transport unchanged here**. `approved_first_try` | done |
| 08b | shell-exit-detection ([phase-08b-shell-exit-detection.md](phase-08b-shell-exit-detection.md)) — a command sent to a shell whose program has exited **fails in seconds, saying so**, instead of waiting out its timeout and being reported as "may still be running". **Inserted at 08c drafting on a live probe:** a real `ssh -o BatchMode=yes` with an unknown host key died in 17 ms and the daemon's `run_command` waited the full 20 s. One shared loop (`read_to_marker`) polls `Status`; `ShellExited` is a typed error; both model-facing texts stop claiming the command may still run | done        |
| 08f | prompt-on-the-real-prompt ([phase-08f-prompt-on-the-real-prompt.md](phase-08f-prompt-on-the-real-prompt.md)) — a PTY password prompt is recognised only when really on screen, answered **once**, and cancelled with `^C` when nobody can answer. **Inserted ahead of 08c on a live probe** (PE 2026-09-27): the command is wrapped with `SUDO_PROMPT` twice, so the classifier matches the shell's **echo**; a never-prompting command is given up; after a correct answer later output **re-sends the password into the shell** (13 plaintext copies in one cast, `command not found` twice); the `\n` give-up re-prompts on both sudo 1.9.17 and pinky's sudo-rs, eating the next command. Prototype measured on pinky and locally before drafting. **Dispatched ahead of 08c** | done |
| 08c | ghost-remote-pty ([phase-08c-ghost-remote-pty.md](phase-08c-ghost-remote-pty.md)) — a ghost command whose `ssh_target` resolved (08a) runs in a **PTY shell on that host** instead of `ssh <target> '<cmd>'` in a tmux window: the first production `Owner::Ghost`, ControlMaster, the entry's own `ssh` destination, and ghost SSH shells opened with **`-o BatchMode=yes`** so ssh never prompts (PE 2026-09-27; measured on pinky: key auth 378 ms, unknown key fails in 17 ms, password-only in 59 ms) — chat keeps prompting and `ssh_argv_does_not_set_batch_mode` stays pinned. Rides 08b's exit detection so a refused connection fails fast, and **08f's prompt fix** so a ghost's unanswerable password prompt fails fast and leaves its shell usable. **PE 2026-09-27:** a remote ghost command is synchronous with a **300 s** cap (`GHOST_TURN_TIMEOUT_SECS`), and a ghost's shells are **closed at ghost exit** (TERM ends an idle ssh shell host in ~100 ms, measured). `resolve_command` / `remote_stream_cmd` / `remote_materialize_cmd` keep their semantics. An unresolved `ssh_target` keeps the 1.x wrap. **Sandbox:** a remote ghost command is allowed while `[sandbox] enabled` — `hosts:` is the fence (PE 2026-09-27). **Carries two 08a review residuals:** revert `ghost_host_refusal` to private, and replace `refusal_gate_tests`' `HomeGuard` (leaks a tempdir per run) with `test_home_guard()`. **Carries one 08b review residual:** the `run_in_shell_with_prompts` doc comment (`shell_exec.rs:76`) still opens with the self-contradictory *"The non-chat entry point: the entry point …"* — make it open "The entry point `executor/mod.rs` calls with a pub/sub transport." | done |
| 08d | spawn-ghost-host ([phase-08d-spawn-ghost-host.md](phase-08d-spawn-ghost-host.md)) — `spawn_ghost_shell { host? }` (§ 2.5): a coordinator names the host a specialist's shells open on; the value must be in the runbook's `hosts:` list (row 06's enforcement) or it is refused as an unlisted host. Rides 08a/08c's mapping; also closes the ghost's dropped `host` argument (`ghost.rs:829-845`); **PE added at sign-off** | done |
| 08e | remote-connect-handshake ([phase-08e-remote-connect-handshake.md](phase-08e-remote-connect-handshake.md)) — a **fresh** remote shell's connection-time prompt (password, passphrase, unknown host key) is handled **before** the first command is written. **Measured at 08c drafting:** a plain command's marker-wrapped bytes are typed straight into ssh's `password:` prompt and the run waits out its timeout; routing through the classifier cannot fix it, because the command is written before any prompt is classified. **Reopens phase-05:** its password/passphrase round trip was verified only by running `ssh` directly under `script`, never through a daemon shell. Needs a live-probed readiness handshake before drafting **Carries one 08d review residual:** an unconfigured ghost command host reads `Error: Error: …` — `for_host`'s message already carries the prefix and `run_background` adds another **Drafted 2026-09-29 on live probes (PE):** connect first — `ssh -M -N -f` in a daemon-driven PTY before any shell spawns (81 ms key-auth; `ssh -O check` 2–3 ms decides warm), prompts answered via the caller's transport; all four callers may prompt, reversing 07a's no-prompt read rule **Bounced 2026-09-28:** bug-08e-1 (connect timeout never restarts), bug-08e-2 (ghost edit_file connects before refusal), bug-08e-3 (Mutation A unpinned). **Done 2026-09-29** (`approved_after_1`) | done        |
| 09a | edit-keeps-file-mode ([phase-09a-edit-keeps-file-mode.md](phase-09a-edit-keeps-file-mode.md)) — an `edit_file` edit keeps the file's **mode** (and its owner where the writer may set it) on the local path and both remote interpreter branches. **Measured at 09 drafting:** `0600` → `0644` locally, `0600` → **`0664`** on pinky (PTY shell umask `0002`, so group-writable); the stat→chmod→chown→rename fix run on both interpreters and on pinky | in-progress |
| 09b | remote-sudo-edit — `edit_file { host, sudo: true }` edits a root-owned file through the PTY sudo path (`run_in_shell_with_prompts`), the password answered via `CredentialPrompt`; the approval panel says "as root", and a sudo edit is **never** auto-approved or session-approved. **PE 2026-09-29:** add sudo to `edit_file` before the live close. Facts measured at 09 drafting are in Notes | todo (not drafted) |
| 09c | live-close — the exit criterion end to end: a remote sudo edit via `edit_file { host }` from chat on pinky; the cast log shows the edit, and no `"i"` event carries the password; ControlMaster reuse timed. Architect-run; the phase exists so the obligation is a tracked `## Spec` task, not a close-out promise | todo (not drafted) |

## Notes

### Measured before scoping — the facts the breakdown rests on

Every row above was shaped by one of these. Each was read from the tree on
2026-09-18 at HEAD `16741432`; none is inferred from the design.

- **It is greenfield.** No `src/host.rs`, no `HostId`, no `[hosts]` section,
  no `ControlMaster` / `ControlPath` string anywhere in `src/` or `assets/`, no
  `var/run/ssh/` path constructor. The design's `ExecutionPolicy::wrap_remote`
  is a name drift — the type is **`GhostPolicy`** (`src/daemon/policy.rs`).

- **The shell host spawns one program token, no arguments.** `daemoneye
  shell-host --shell <SHELL>` → `launch::spawn_detached(id, shell, …)` →
  `PtyShell::spawn(shell)` → `CommandBuilder::new(shell)` with nothing else.
  An SSH shell is a program **plus argv** (`-tt`, three `-o` flags, a
  destination). Row 03 exists because the spawn path cannot carry it today.

- **The marker protocol is interpreted by whatever shell sits on the far side
  of the PTY.** `wrap_command(cmd, nonce, shell_name)` closes with
  `exit_var(shell_name)` — `$status` for fish/csh/tcsh, `$?` otherwise — and
  `shell_name` is `login_shell()`, i.e. the **daemon host's** `$SHELL`. Over
  ssh the *remote* login shell runs it, and that shell is unknown at spawn.
  `$?` is right for sh/bash/zsh/dash/ksh and wrong for fish. Row 04 measures
  which to pin (probe the remote shell, or pin `$?` and document fish remotes
  as unsupported); it does not guess.

- **The PTY path has no credential-prompt handling at all.** `grep -n
  'sudo\|Credential\|password' src/daemon/executor/shell_exec.rs
  src/daemon/executor/bg_shell.rs src/shell/client.rs` → **nothing**. The tmux
  path has a complete mechanism: `SUDO_PROMPT='[de-sudo-prompt]'`
  (`background/run.rs:212`), `wait_for_sudo_prompt_and_inject`
  (`run.rs:364`), `sudo_sentinel` / `with_sudo_sentinel` and
  `Response::CredentialPrompt` (`executor/foreground.rs`). The design says
  remote prompts surface "the same way sudo prompts are today" — **on the PTY
  path they do not surface today**. Two consequences: row 01a is a
  pre-requisite, not a nicety; and **a chat-session `sudo` on the shipped
  default backend almost certainly hangs to its 45 s timeout right now**. **Confirmed live at 01a drafting
  (2026-09-18)**, driving a real `daemoneye shell-host` over its socket: `sudo
  -k; sudo true` emits `[sudo] password for matt: ` and the end marker **never
  arrives** — the command runs to its deadline and the user is never asked.
  A second finding from the same probe: a shell parked at a prompt **eats the
  next command's bytes**, the same poisoning `shell_exec.rs:219` already guards
  for interactive sessions, which is why 01a's give-up path must un-park the
  shell rather than just return.

- **No ssh prompt pattern exists.** `continue connecting`, `passphrase`,
  `Permission denied`, `Host key` — zero hits in `src/` outside the two
  client-side `"  Password: "` render strings. `utils/sudo.rs`
  (`command_has_sudo`, `is_fingerprint_prompt`, the sentinel helpers) is the
  extension point; row 05 extends it.

- **Cast-log `"i"` events are written raw.** `CastWriter::write_event(at, "i",
  bytes)` (`log.rs:62`) with no scrub, redact or mask anywhere in `log.rs` /
  `backend.rs`. § 3's requirement — nothing typed during a password prompt
  reaches the log — is unimplemented, and the exit criterion depends on it.
  Row 01b is host-process work (the writer lives in `daemoneye shell-host`),
  which is why it is split from 01a's daemon-side detector.

- **`ShellRecord` has no host, and is built at 9 sites in 3 files**
  (`registry.rs`, `session.rs`, `knowledge/shell.rs`). `ShellResponse::Status`
  carries `state, rows, cols, pid` — no program, no host. This is the 10-site
  change M22 phase 04a measured and deferred `open_shell { host?, label? }` on.
  Row 03 makes it.

- **Remote file ops today are pane scraping.** Four `if let Some(pane) =
  target_pane` sites in `file_ops/ops.rs` build hex-encoded python/perl
  one-liners and call `remote_run_and_capture(pane, cmd, 30)`. The one-liners
  are sound and stay; the transport and the result capture are what row 07
  replaces. PE decision 3 (§ 8): *PTY for everything remote, including file
  ops.*

- **`GhostConfig.ssh_target` → `GhostPolicy::from_config` → `wrap_remote`**
  wraps as `ssh <target> '<cmd>'` via `sh_single_quote`; `resolve_command`
  already resolves scripts to `~/.daemoneye/scripts/<name>` for the remote;
  `scripts.rs` has `remote_stream_cmd` and `remote_materialize_cmd`. Row 08
  keeps every one of those semantics and changes only *where the command
  runs*.

- **`Response::ToolCallPrompt` is `{ id, command, background, target_pane }`.**
  No classification field. Rendered at `cli/commands/stream.rs:427` and
  `cli/commands/ask.rs:118`. The four approval classes in § 3 need a wire field
  and a renderer — row 06.

- **The user typing `ssh` into a shell is already a recognised state.**
  `is_interactive_command` / `interactive_destination` (`utils/shell.rs`)
  parse an ssh destination with a flag table, and `shell_exec.rs:219` marks
  the shell `interactive`. That is a *user-driven* remote session and stays
  distinct from a daemon-spawned `Host::Ssh` shell; row 07's "never type into
  an interactive shell" rule is what keeps them apart.

- **The config template's mechanism already fits `[hosts.<name>]`.** Sections
  are documented as commented `# [section]` blocks, and
  `seeded_config_template_documents_every_config_field` treats free-form maps
  as `[section.key]` subtables — it handles `[models.<name>]` today. Row 02
  rides that; no `doc_truth` change is expected.

- **The re-homed M19 phases have no spec to import.** M19's phase-09 (escape
  hatch) and phase-12 (workspace mount) were never drafted — no files exist
  under `docs/dev/milestones/M19-sandbox-completion/`. What is re-homed is the
  design's one-line intent plus M18's D6 ("sudo scripts are escape-hatch
  ops"). Row 06 lands the classification; row 08 parses `sandbox_profile`
  without enforcing it.

- **Live environment:** OpenSSH 10.5 on the daemon host. **The
  live-verification host is `pinky`** (PE decision 2026-09-18). Resolved with
  `ssh -G pinky`, without connecting: `hostname pinky.home.planetfoo.org`,
  `port 22`, `user matt`, six `identityfile` entries, and **`controlmaster
  false`** — so the ControlMaster behaviour rows 04 and 09 measure comes
  entirely from the flags the daemon passes, not from the user's config.
  Rows 04, 05, 08b and 09 all target it.

- **User-facing docs that go stale:** `README.md` documents `ssh_target` twice
  (the runbook field table and the script-dispatch note) and "Remote script
  streaming" once; `CLAUDE.md`'s `edit_file` row says "remote via
  `target_pane`". PE rule: README current at every 2.0 milestone close and on
  any user-visible change — rows 07 and 08 carry those edits.

- **The config struct is `HostEntry`, not `HostSpec` — probed at 02 drafting
  (2026-09-19).** `tests/doc_truth.rs` discovers config sections by walking
  `Config`'s fields and keeping those whose **type name ends with `Config` or
  `Entry`**. Measured both ways against the real gate: named `HostSpec`,
  `doc_truth` reports **14 passed** and the section is invisible — a
  user-facing config table would ship wholly undocumented with nothing
  complaining. Named `HostEntry`, the gate **fails** naming `"[hosts] ssh"`
  and `"[hosts] tags"` as undocumented, which is the gate working. The design
  and this README's earlier rows said `HostSpec`; 02 supersedes that. A second
  probe found the phantom-key half skips any section containing a `.`, so
  `[hosts.web01]` keys go unchecked until `doc_truth` gains a `hosts`
  special-case beside its `models` one — 02 adds it and mutation-proves it.

- **Row 04 split into 04a/04b at drafting (2026-09-20), and the substrate's
  hard questions were answered live against `pinky` first.** Every risk the
  scoping README listed for this row is now measured, not assumed: the marker
  protocol works unchanged over `ssh -tt` (exit codes 0, 1 and 42 all
  propagate); ControlMaster is worth its flags at **157 ms cold vs 4–5 ms
  warm**; `pinky` runs bash so `$?` is right there; the remote shell's OSC
  shell-integration sequences do **not** break `parse_outcome`; and a bare
  `exit N` kills the session, so the probe uses a subshell.

  One real defect surfaced that no test would have caught: `shell_exec.rs`
  passes `login_shell()` — the **daemon's** `$SHELL` — as the name whose
  exit-variable syntax the marker uses, but over SSH the **remote** shell
  interprets it. A fish-using operator on a bash host would emit `$status`,
  which bash expands to empty, and no exit code would parse. 04a pins remote
  shells to POSIX `$?` and tests it with `SHELL` set to fish.

- **Row 08 split into 08a/08b/08c at drafting (2026-09-27), on four PE
  decisions.** Measured at HEAD `ee70e420` before any were asked:
  - *Half the row was already done.* `HostEntry.sudo_scripts` and
    `sandbox_profile` have parsed since 02 (`config/mod.rs:901-914`). Nothing
    reads `sudo_scripts`.
  - *Ghosts never touch the PTY substrate.* Every ghost `run_terminal_command`
    becomes `PendingCall::Background` with `host` dropped on purpose
    (`ghost.rs:829-845`), then runs **synchronously** in a tmux `de-gs-bg-*`
    window whatever the backend (`executor/mod.rs:660`, `uses_pty &&
    !is_ghost`). A remote one is `ssh <target> '<cmd>'` there — no
    ControlMaster, no credential or host-key detection. `Owner::Ghost` exists
    and has **no production user**.
  - *`ssh_target` is not a host name.* Its documented form is `user@host`, and
    `HostId::parse` rejects `@`. The design's `open_shell(host:
    "ops@10.0.0.9")` example drifts from 02's grammar the same way.
  - *Where resolution can run.* `GhostPolicy::from_config` is called per tool
    call under the sessions lock; every ghost start funnels through
    `start_session_with_config`. Resolution happens once there.

  **PE decisions:** (1) `ssh_target` resolves against `[hosts]` — by key, else
  by a unique `ssh`-destination match — and anything unresolved keeps the 1.x
  wrap byte for byte; (2) split, with the pure resolution first (08a) and the
  move onto a host PTY shell second (08b); (3) `sudo_scripts` is **enforced**,
  empty = unrestricted; (4) with the sandbox enabled a ghost may still run a
  **remote** command — the container protects the daemon host, and the
  runbook's `hosts:` list is the fence.

  **Probed at 08a drafting:** a copy of `webhook_ghost_e2e_deterministic` that
  writes `[hosts.e2e-host]` to `etc_dir()/config.toml` and gives the runbook
  `ssh_target: e2e-host` passes in 0.09 s and loads the host, so 08a's door
  test rides the existing isolation harness. Unprobed and recorded for 08b:
  with `[sandbox] enabled`, a ghost's background command runs in a
  `--network=none` container, where an `ssh <target>` wrap would have no
  network — read from `background/run.rs`, not executed.

- **Two substrate defects under the ghost move, measured live at its
  drafting (2026-09-27).** A throwaway test hosted a real `ssh` in an
  in-process shell host and drove it with the daemon's real `run_command`:
  - **Dead shell → full timeout (D2 → row 08b).** `ssh -o BatchMode=yes`
    with an unknown host key exits 255 in **17 ms**; `run_command` waited
    **20.0 s** and reported a timeout. The shell host stays alive after its
    program dies and just sends nothing; nothing in the one shared read loop
    asks `Status`, which already reports `Exited`.
  - **Connection prompt eats the first command (D1 → row 08e).** With a
    password-only login and no `BatchMode` (the chat configuration), the
    screen showed `matt@pinky…'s password: ` followed by the command's
    marker-wrapped bytes, and the run waited out its timeout. Phase-05 wired
    ssh prompt classification into the **sudo** loop only, and its live
    check ran `ssh` directly under `script`, never through a daemon shell,
    so its password round trip is unverified end to end.

  **PE decisions:** a new row ahead of the ghost move, then split on a
  second measurement — 08b is exit detection only (all a `BatchMode` ghost
  needs, since ssh then dies instead of prompting), and 08e takes the
  connection-time handshake, drafted only after probing; ghost SSH shells
  use `-o BatchMode=yes` (08c), chat keeps prompting.

- **The PTY password-prompt path is broken three ways — measured at 08c
  drafting (2026-09-27), row 08f inserted ahead of 08c (PE).** A throwaway
  in-crate probe drove the real `run_in_shell_with_prompts` against real
  `ssh -o BatchMode=yes pinky` and a local bash:
  - *Echo match.* `run_in_shell_with_prompts` and `run_sudo_command` each
    prepend `SUDO_PROMPT`, so the typed line carries sentinels N+1 then N;
    bash gives the program N and the classifier looks for N+1 — present only
    in the shell's echo. `sudo -k` (never prompts) failed in 17 ms with a
    false "did not complete within the 20s limit". Only the first simple
    command gets the variable, so `cd x && sudo y` is never classified.
  - *Re-sent credential (security).* The callback sees the whole buffer, so
    after a correct answer any output re-triggers it: with a fake password
    program and an immediately-answering transport, the secret was typed
    three times, ran twice as a shell command, and the cast held 13
    plaintext copies. In chat the user is asked again. 01a's live check
    passed by human latency.
  - *Give-up.* `\n` at a real prompt re-prompts on sudo 1.9.17 **and**
    sudo-rs 0.2.13 (pinky); `^C` cancels on both, but sudo-rs then abandons
    the rest of the line (no end marker) and eats bytes that arrive in the
    same millisecond — a 300 ms quiet wait fixes it.

  Also measured for 08c: BatchMode+ControlMaster to pinky 470 ms cold / 12 ms
  on reuse; a 25 KB `remote_stream_cmd` typed into the interactive remote
  shell runs with the right exit code.

### Measured at 09 drafting (2026-09-29) — for 09b

- **`edit_file` has no sudo path.** Every remote write runs through
  `run_in_host_shell`'s plain `run_command`, which never prompts. A root-owned
  file cannot be edited with it, which is why row 09 split.
- **`command_has_sudo` misses a `sudo` inside `if … then …`.** Its regex is
  `(?:^|[;&|])\s*sudo\b`. The built edit command is
  `if command -v python3 …; then python3 …; else perl …; fi`, so putting
  `sudo` inside the branches is **not** detected (measured: `false`). A sudo
  edit line must **start** with `sudo` (measured: `true`), so the interpreter
  has to be chosen before the line is built.
- **The line shape works on pinky through the daemon's own path.**
  `sudo python3 -c "…" 2>&1; echo '__DE_DONE__'` through
  `run_in_shell_with_prompts` with nobody attached was classified on
  sudo-rs's real prompt and gave up in **315 ms** with the "nobody is
  attached" sentence. The next command in the same shell ran in 17 ms.
  pinky has `/usr/bin/python3` and `/usr/bin/perl`.
- **The client auto-approves `EditFilePrompt`** by exact `path` string or by
  the session-wide `approval.file_edits_all` flag (`src/cli/commands/stream.rs`,
  `prompt_edit_file_ratatui`). A sudo edit needs a signal on the prompt
  (an IPC field) so neither shortcut applies. `Response::EditFilePrompt` is
  built at **9** sites in `file_ops/ops.rs`, 1 in `ipc.rs`, and matched in
  `cli/commands/{stream,ask}.rs`.
- **Adding a field to `edit_file` touches 11 variant sites:** `ai/types/pending.rs`
  (enum `:84`, `to_tool_call` `:556`, two test literals `:1347`, `:1634`),
  `ai/types/events.rs:75`, `ai/tools/args.rs` (`EditFileArgs` `:157`,
  `to_event` `:587`), `daemon/stream.rs:481`, `daemon/ghost.rs:1123`,
  `daemon/executor/mod.rs:1019`, `file_ops/write.rs` `EditArgs`. The tree
  does not compile until the last site is done (M22 03a's lesson: batch the
  sites, and expect the verifier to fail until then).

### Not in this milestone

- **The `[HOSTS]` context block** — M23, which owns the context blocks. M21
  gives it a `HostId` to render; it does not render one.
- **Chat outside tmux, `/shells`, `/tail`, the attach picker** — M23.
- **Enforcing `sandbox_profile` on a host** — parsed in row 08, enforced when
  the container work meets the host model (M26 per the roadmap).
- **The `backend = "tmux"` opt-out path** gets no host features. It remains
  byte-for-byte the 1.x behaviour, including `ssh_target` string-wrapping,
  until it is deleted.

### Risks, and where each is retired

| Risk | Retired in |
|---|---|
| `ssh -tt` readiness: the marker protocol needs a prompt, and "connected" is not "at a prompt" (banner, MOTD, slow auth) | 04 — probe before drafting; readiness may need a sentinel `echo` rather than prompt-pattern matching |
| Remote shell exit variable (`$?` vs `$status`) | 04 — measured, pinned, and the unsupported case named |
| Scrub race: the host writes `"i"` as bytes arrive, but the daemon detects the prompt from output — the first keystroke may land before the scrub engages | 01b — measured with a real prompt; the scrub may need to be host-side pattern detection on the output stream, not a daemon signal |
| ControlMaster + `-tt` + a `ControlPersist` master outliving the daemon | 04 — a stale master is swept with the shell sockets, or `ControlPersist` is shortened; measured |
| Local sudo on the default backend is broken today and users hit it before 01a lands | 01a is row one for this reason |
