# M21 — Host model & SSH

**Goal:** a configured remote host is an execution target the AI can name —
`open_shell { host }`, `run_terminal_command { host }`, `read_file` /
`edit_file { host }` — reached through an SSH session **in a local PTY shell**
using the user's own keys and agent, with every credential prompt (host key,
password, passphrase, **remote sudo**) surfacing through the approval client
and never reaching the shell log.

**Status:** in-progress — scoped 2026-09-18, **PE sign-off on the breakdown
2026-09-18** with two decisions: the live-verification host is **`pinky`**, and
`host?` on `spawn_ghost_shell` is in scope as row 08b. No phase drafted yet;
`/rexymcp:architect next` drafts 01a.

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
drafting on a measured seam**, as every M20 and M22 row did. **Ten rows** (01a, 01b, 02–08, 08b, 09). Ordering: the
credential mechanism the PTY path lacks comes first because remote sudo, host
keys and passphrases all ride on it; config and the argv-carrying spawn path
are pure and unblock the SSH shell; the SSH shell unblocks everything remote;
the live close is last.

| # | Phase | Status |
|----|-------|--------|
| 01a | credential-prompt-on-pty ([phase-01a-credential-prompt-on-pty.md](phase-01a-credential-prompt-on-pty.md)) — detect a `sudo` password prompt on a **local** PTY shell and round-trip it through `Response::CredentialPrompt` / `CredentialResponse`, the mechanism the tmux path has and the PTY path has **none** of. **Drafting probe confirmed the defect live:** real `sudo` on a PTY shell emits `[sudo] password for matt: ` and **no end marker ever arrives** | done |
| 01b | input-scrub-on-prompt ([phase-01b-input-scrub-on-prompt.md](phase-01b-input-scrub-on-prompt.md)) — the shell host writes `[redacted]` instead of the credential's bytes while the screen shows a secret prompt. **Drafting measured the race:** the pump broadcasts to the daemon *before* it feeds the screen, so a screen-based check loses to a daemon that answers immediately — the flag is armed from the chunk, ahead of the broadcast | in-progress |
| 02 | host-config — `src/host.rs`: `HostId`, `HostSpec { ssh, tags, sudo_scripts, sandbox_profile }`, `[hosts.<name>]` as a free-form map on `Config` (the `[models.<name>]` subtable mechanism `doc_truth` already handles), template docs, validation. Runbook `hosts: [...]` frontmatter → `GhostConfig.hosts`. Pure, additive | todo (not drafted) |
| 03 | shell-spawn-argv — the spawn path carries a program **and its arguments** end to end (`shell-host` clap → `launch::spawn_detached` → `PtyShell::spawn`), and `ShellRecord` gains `host: HostId` (the 9-site struct change 04a measured). `list_shells` and `[SHELLS]` show the host. Local behaviour byte-identical | todo (not drafted) |
| 04 | ssh-shell — `Host::Ssh` spawn: `ssh -tt -o ControlMaster=auto -o ControlPath=<var/run/ssh/%C> -o ControlPersist=10m <dest>` as the shell program; the `var/run/ssh/` path constructor + lifecycle row; readiness = remote prompt; the marker protocol over ssh (the remote-shell exit-variable decision is **measured here**). `open_shell { host? }` and `run_terminal_command { host? }`. Live: one command on the configured host, in the cast log. ControlMaster reuse measured | todo (not drafted) |
| 05 | ssh-prompt-detection — host-key (`continue connecting`), password, passphrase and `Permission denied` patterns extend 01a's detector; 01b's scrub covers them. **Remote sudo** is 01a's detector over the ssh PTY — verified live | todo (not drafted) |
| 06 | new-host-approval — `Response::ToolCallPrompt` gains a `classification` (`command` / `remote_shell` / `new_host` / `escape_hatch`); an unconfigured destination is allowed but gated as `new_host` with an offer to persist to `[hosts]`; the client renders the class and names the host; session-scope `A` is per class per host. Ghost `hosts:` enforced — an unlisted host is refused and recorded (the M18 D6 "parked escape-hatch" pattern). The re-homed M19 phase-09 lands here as a classification | todo (not drafted) |
| 07 | remote-file-ops-over-pty — `read_file` / `edit_file` gain `host?`; the four `target_pane` sites in `file_ops/ops.rs` route the existing hex-encoded python/perl one-liners through an idle shell on the host (or a new one — never into an `interactive` shell) with **marker-delimited** results instead of pane scraping; `target_pane` retired from both tools. Closes M22's deferral. `CLAUDE.md` / `README.md` rows | todo (not drafted) |
| 08 | ghost-host-mapping — `GhostPolicy.ssh_target` maps onto `HostId` unchanged in meaning: `wrap_remote`'s `ssh <target> '<cmd>'` string-wrap becomes "run in a shell on that host"; `resolve_command`, `remote_stream_cmd` and `remote_materialize_cmd` keep their semantics; `HostSpec.sudo_scripts` is the 1.x remote-sudoers model. The re-homed M19 phase-12 (`sandbox_profile` on a host) is parsed here, not enforced | todo (not drafted) |
| 08b | spawn-ghost-host — `spawn_ghost_shell { host? }` (§ 2.5): a coordinator names the host a specialist's shells open on; the value must be in the runbook's `hosts:` list (row 06's enforcement) or it is refused as an unlisted host. Rides 08's mapping; **PE added at sign-off** | todo (not drafted) |
| 09 | live-close — the exit criterion end to end: a remote sudo edit from chat on the configured host; cast log shows the edit, no `"i"` event carries the password; ControlMaster reuse timed. Architect-run; the phase exists so the obligation is a tracked `## Spec` task, not a close-out promise | todo (not drafted) |

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
