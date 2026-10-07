# M26 — Agent isolation on Docker Sandboxes (sbx)

**Goal:** Run autonomous ghost shells and chat `wait: false` commands inside
**Docker Sandboxes** (`sbx`) microVMs on the PTY substrate, and delete the
M18/M19 `docker run` sandbox it supersedes.

**Status:** in-progress. Rescoped 2026-10-05 from "tmux removal & 2.0 close";
**PE signed off on scope 2026-10-05**, accepting every recommendation on the
six questions (§ Decisions on the open questions).

**Depends on:** M20 (shell engine; shell-host child argv), M21 (host model;
`sandbox_profile` parsed on `[hosts.<name>]`), M23 (the carried 10d row:
sandboxed ghost in a PTY shell + `Ctrl-p`), M24 (runs; dedicated shells for
`wait: false`).

**Exit criteria:**

- **A sandboxed ghost, end to end, live.** With `[sandbox] enabled = true`, a
  webhook ghost on the daemon host gets a sandbox named for its run, every
  local command it runs lands in a PTY shell whose child is
  `sbx exec -it <sandbox> bash`, and the real exit code comes back through the
  marker protocol. When the ghost exits, on every path including failure and
  cancel, `sbx ls --json` no longer lists its sandbox. Evidence: the run turn
  log, the shell's `.cast`, and `sbx ls` before and after.
- **`Ctrl-p` freezes the sandboxed job.** Attached to that ghost, `Ctrl-p`
  stops the in-VM foreground job: a tick loop's counter is unchanged across 2 s
  while paused and advances after resume. (The 10d defect: stopping the host
  client does **not** freeze the in-VM process — measured below.)
- **Egress is policy-enforced and audited.** A command to a host the profile
  does not allow fails, and the denial appears in `sbx policy log <sandbox>`
  with reason `default deny`/the deny rule; DaemonEye's run record names it.
- **Chat `wait: false` runs in sbx** when the sandbox is enabled, returning the
  shell id immediately as today; foreground chat commands are unchanged.
- **Fails closed.** On a host where `sbx` is missing, or `sbx daemon status`
  is not `running`, a sandboxed command or ghost is refused with an
  operator-facing reason; it never falls back to the host.
- **Survives a daemon restart.** A sandboxed ghost's shell started before
  `daemoneye daemon` restarts is adopted afterwards with its sandbox name, and
  the startup sweep removes only `de-*` sandboxes that no live record owns.
- **The docker runtime is gone.** `src/daemon/executor/container.rs`,
  `containers/`, `sandbox.lock`/proxy lockfile, `daemoneye sandbox build` and
  every `docker --host` argv are deleted; `[sandbox]` keys that only meant
  something to docker produce a loud load-time warning, not a parse error.
- **Docs current.** README (sandbox section, CLI table, config table, ghost
  prose), CLAUDE.md § Container sandbox, `security.md`, the seeded
  `ghost-shell-guide` memory (still describes `de-gs-*` windows), and
  `docs/architecture.md` describe sbx; `tests/doc_truth.rs` passes.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.9 (sandbox integration — rewritten for
  sbx 2026-10-05), § 3 (security deltas), § 6 (roadmap — M26 renumbered).
- `docs/design/agent-container-sandboxing.md` — the M18 design. Its threat
  model (D1 identity, no credentials in the work plane, fail closed) still
  holds; its mechanism (rootless `docker run`, uid gate, egress-proxy
  container) is what this milestone replaces.
- `docs/architecture.md` § "DaemonEye 2.0 — M20 Shell Engine" (shell hosts,
  marker protocol, registry adoption, signal lifecycles).

## Phases

Estimated 9–10 rows. Only phase 01 is drafted on scope sign-off; the rest are
drafted on demand and may split.

| #  | Phase | Status |
|----|-------|--------|
| 01 | sbx-measure ([phase-01-sbx-measure.md](phase-01-sbx-measure.md)) — architect-run probes on pinky (`probes/`): in-VM foreground group for pause, resize, cancel, teardown under a live shell, `sbx cp` staging, limits, error shapes, the per-sandbox policy mapping; results quoted in this README | done (approved_first_try, 2026-10-05) |
| 01b | baseline-green ([phase-01b-baseline-green.md](phase-01b-baseline-green.md)) — fix the 4 clippy 1.97 lints and make `bounded_output_with` kill the child's process group on timeout (see § Baseline red on pinky) | done (escalated, 2026-10-06) |
| 02 | sbx-argv ([phase-02-sbx-argv.md](phase-02-sbx-argv.md)) — `src/daemon/executor/sbx.rs`: pure argv builders (`create`/`exec`/`stop`/`rm`/`ls --json`/`policy allow|deny --sandbox`), `ls --json` parser, sandbox naming, profile → flags mapping, preflight (`sbx version`, `sbx daemon status`) cached and fail-closed | done (approved_first_try, 2026-10-06) |
| 02b | backend-flake ([phase-02b-backend-flake.md](phase-02b-backend-flake.md)) — make `input_after_a_prompt_logs_a_redaction_marker` use a real `read -rs` prompt and re-read the cast while polling (see § Flaky test on pinky) | done (approved_first_try, 2026-10-06) |
| 03 | sbx-config ([phase-03-sbx-config.md](phase-03-sbx-config.md)) — `[sandbox] runtime = "sbx"` accepted; a pure `SandboxConfig::warnings()` flags docker-only keys (`image`, `proxy_image`, `workdir`, `run_as`, `docker_host`, `limits.pids`, `limits.scratch`, `ghost_defaults.mount_scripts`) as ignored under sbx and says a `proxy` profile is additive (Q1). No sbx template key in M26 | done (approved_after_1, 2026-10-06) |
| 04a | sbx-shell ([phase-04a-sbx-shell.md](phase-04a-sbx-shell.md)) — `ShellRecord.sandbox`; `spawn_for_in_sandbox`; `which_sbx` + `spawn_sbx_host` (child `sbx exec -it <sandbox> bash`); `shell_for_owner` never reuses a sandbox shell; `shell_marker_name` gives `"sh"` inside a sandbox | done (approved_first_try, 2026-10-06) |
| 04b | sbx-adopt ([phase-04b-sbx-adopt.md](phase-04b-sbx-adopt.md)) — a `<id>.sandbox` sidecar written by `spawn_sbx_host`, read by `adopt_at_startup` (an invalid sidecar fails closed as `dedicated`), removed by `sweep_stale`; no PID/`/proc` recovery (the lock PID is diagnostic only) | done (approved_first_try, 2026-10-06) |
| 05a | sbx-signal-plan ([phase-05a-sbx-signal-plan.md](phase-05a-sbx-signal-plan.md)) — tag each sandbox shell (`-e DE_SHELL=<id>`); the measured in-VM `FIND/STOP/CONT/HUP` script as `VM_SIGNAL_SCRIPT`; `vm_signal_args`, `parse_vm_find`, `vm_op_outcome`, `sbx_target_from_host` | done (escalated, 2026-10-06) |
| 05b | sbx-signal-host ([phase-05b-sbx-signal-host.md](phase-05b-sbx-signal-host.md)) — `SbxBackend` wraps `PtyBackend`; the shell host of a tagged sbx child routes STOP/CONT/close into the VM, sends INT/TERM as a PTY `^C` (an in-VM `kill -INT` does not end a prompt loop — measured), and reports status from `FIND` | done (approved_first_try, 2026-10-06) |
| 05c | pause-deadline ([phase-05c-pause-deadline.md](phase-05c-pause-deadline.md)) — Q6: a paused command's deadline clock stops while it is paused (all shells, not only sandbox ones) | done (approved_first_try, 2026-10-07) |
| 06a | sbx-lifecycle ([phase-06a-sbx-lifecycle.md](phase-06a-sbx-lifecycle.md)) — `sbx_lifecycle`: `preflight`, `create_sandbox` (profile → plan, limits, allows; a failed allow tears down), `teardown_sandbox` (rm → stop fallback for the locked keychain), `sweep_daemon_sandboxes`; plus the 04b orphan-sidecar fix | done (approved_first_try, 2026-10-07) |
| 06b | sbx-shell-lookup ([phase-06b-sbx-shell-lookup.md](phase-06b-sbx-shell-lookup.md)) — `shell_for_owner_in_sandbox` (reuse/create the owner's shell in a named sandbox); `run_in_shell_with_prompts` takes the marker from the record (`shell_marker_name`), so a sandbox shell targeted by id uses POSIX `$?` | done (approved_first_try, 2026-10-07) |
| 06c | sbx-ghost ([phase-06c-sbx-ghost.md](phase-06c-sbx-ghost.md)) — `ghost_sandbox`: `ensure_ghost_sandbox` (fail-closed preflight, create once per run, profile = the ghost's agent), `run_ghost_in_sandbox`; routed in `run_background`; teardown in the ghost exit path; startup sweep after adoption | done (approved_first_try, 2026-10-07) |
| 06d | sbx-script-stage ([phase-06d-sbx-script-stage.md](phase-06d-sbx-script-stage.md)) — Q3: `sbx cp` the one script a ghost runs into its sandbox | done (approved_first_try, 2026-10-07) |
| 07 | sbx-chat-background ([phase-07-sbx-chat-background.md](phase-07-sbx-chat-background.md)) — chat `wait: false` in a sandbox shell when enabled; sandbox lifetime per chat session (see Q5) | done (approved_first_try, 2026-10-07) |
| 08a | sbx-status ([phase-08a-sbx-status.md](phase-08a-sbx-status.md)) — `Request::ContainerStatus` reads the sbx preflight + `sbx ls --json` when `runtime = "sbx"`; the SANDBOX section is shared with a new `daemoneye sandbox status`; wire type unchanged until row 09 | in-progress |
| 08b | sbx-egress-audit — read `sbx policy log <name> --json` just before each teardown; a ghost run's allowed/blocked hosts go into its turn log as a `Note` and into events.jsonl (`sandbox_egress`); chat sandboxes log the event at reap. Measured 2026-10-07: the log survives `rm`, and accumulates across sandboxes reusing a name, so read before teardown | todo |
| 09 | docker-removal — delete `container.rs`, `containers/`, lockfiles, `sandbox build`, the tmux-window sandbox wiring in `background/run.rs`/`ghost.rs`; docs + `ghost-shell-guide` rewrite | todo |
| 10 | live-close — architect-run live verification of every exit criterion on pinky | todo |

## Notes

### Why rescope (2026-10-05)

The M18/M19 sandbox is a hand-built isolation stack — rootless `docker run`,
a uid gate, `--network=none`, a separate egress-proxy container with our own
allowlist and audit, image lockfiles. **Docker Sandboxes** ships that stack as
a product, with a stronger boundary: a **microVM per sandbox** (own kernel,
own Docker engine), a host-side proxy with a deny-by-default **policy engine**
and per-request logs, and **secret injection** where the sandbox only sees a
sentinel and the proxy substitutes the real credential after the request has
left the VM. pinky, the new development host, has `sbx` installed. The 10d
row M23 carried forward ("sandboxed ghost in a PTY shell, `docker pause` for
`Ctrl-p`") was the only planned sandbox work; it now targets sbx.

### PE decisions (2026-10-05)

1. **Numbering.** M26 = sbx isolation. "tmux removal & 2.0 close" moves to
   **M27**; telemetry, MCP surface and fleet follow-ons become M28–M30
   (`daemoneye-2.0.md` § 6 updated).
2. **Replace, don't add.** sbx is the only sandbox runtime. The docker path
   is deleted (row 09), not kept beside it.
3. **One sandbox per ghost run**, created at ghost start and removed at exit;
   no state carries between runs.
4. **Coverage:** local ghost shells and chat `wait: false` commands.
   Foreground chat commands and `[hosts.<name>] sandbox_profile` enforcement
   on remote hosts are **out of scope**.

### Measured on pinky, 2026-10-05 (sbx v0.46.0, sandboxd running)

Architect probes with throwaway `shell` sandboxes (`de-probe-*`, all removed).
Quote these, don't re-derive them.

| Fact | Result |
|---|---|
| `sbx create --name X -q shell` (mountless) | 3.1 s; status `running` |
| `sbx exec X true`, warm | 0.4 s; after `sbx stop`, exec restarts it in 1.5 s |
| Identity inside | `uid=1000(agent) gid=1000(agent) groups=1000(agent),27(sudo),1001(docker)` — passwordless sudo and a private Docker engine **inside the VM**; the VM is the boundary, not the uid |
| Guest | Ubuntu 26.04.1, kernel 7.0.14; `bash`, `sudo`, `docker`, `curl` present; workdir `/home/agent/workspace` |
| Exit codes | `sbx exec X sh -c 'exit 7'` → host `$?` = 7; with `-t` also passes through |
| Marker protocol over `sbx exec -it X bash --norc --noprofile` from a host PTY | `\x1fDE_BEG x` / `\x1fDE_END x 1` arrive intact (bracketed-paste `\x1b[?2004h/l` around them); `^C` (0x03) written to the PTY interrupts the in-VM `sleep`; `TERM=xterm-256color` |
| `kill -STOP` the host `sbx exec` client | **does not freeze** the in-VM job (tick 13 → 25 over 2 s) — the 10d failure mode, unchanged by sbx |
| `sbx exec X kill -STOP -- -<pgid>` in-VM | **freezes** it (29 → 29 over 2 s); `-CONT` resumes |
| `kill -TERM` the host client | in-VM job **keeps running** (orphan) — cancel must kill in-VM |
| Workspace mount `sbx create … shell <dir>` | appears at the **same absolute path**; host files read; a file written in-VM lands on the host as `uid 1000, gid 991 (kvm)` |
| Host files outside mounts | `/home/matt` absent; no `~/.ssh` in the VM |
| Default egress (operator's global `local-policy`, 194 allow rules) | `github.com` 200; `example.org` 403; unknown host 403; logged in `sbx policy log` with rule + reason |
| Raw TCP to a denied `1.1.1.1:22` | bash `/dev/tcp` connect **succeeds** (transparent proxy accepts) but the flow is denied in the log — **a successful connect is not evidence of egress** |
| `sbx create --deny-network '**'` | github, api.github.com, api.anthropic.com all 403 — a true "none" |
| `--deny-network '**'` + `sbx policy allow network --sandbox X example.org` | still **403** — deny beats allow, so "deny all, allow a list" cannot be built per sandbox |
| Service secrets | every sandbox env carries sentinels for every built-in service (`ANTHROPIC_API_KEY=proxy-managed`, `GH_TOKEN=gho_sbxproxy…`, OpenAI, Google, …) **whether or not a secret is stored** — pinky stores none; per the sbx docs the proxy substitutes a stored value on egress to the matching host (not measured: nothing is stored here) |
| Limits | `--memory 1g --cpus 2` → in-VM `nproc=2`, MemTotal 981 MiB |
| Error shapes | exec/rm on a missing sandbox: exit 1, `error: sandbox '<n>' not found`; duplicate create: exit 1, `error: sandbox '<n>' already exists; connect to the existing sandbox` |
| Name limit | 63 characters (`error: sandbox name cannot exceed 63 characters`) — `de-<kind>-<run id>` names must fit |
| Host env | not forwarded by `sbx exec` (`FOO_HOST_ONLY` absent in the VM) |
| Other `HOME` | `sbx` under a different `HOME` (with or without the real `XDG_STATE_HOME`) → exit 1 `Not authenticated to Docker`: the throwaway-`HOME` harness cannot reach sbx, so rows 02–08 test through an injected command runner. Pointing only `XDG_CONFIG_HOME` at the real one starts a second sandboxd — never do that |
| `sbx cp` staging | file lands mode `700`, owner `agent`; runs |
| Policy mapping | `--sandbox` allow: 403 → 200 for that sandbox only (`de-probe-b` stays 403); `--deny-network api.github.com`: 403 while `github.com` stays 200; `--deny-network '**'`: 403; rules gone after `sbx rm` |
| `sbx policy log --json` | top keys `allowed_hosts`, `blocked_hosts`; entry keys `count_since, host, last_seen, proxy_type, rule, since, vm_name` (+ `reason` on blocked) |
| PTY marker + resize | markers round-trip under bash's default rc with exit status intact; `stty size` 24 80 → 40 132 after a host `TIOCSWINSZ`; bash's bracketed-paste escapes precede output lines, so parse by pattern, not line prefix |
| Foreground group | idle: `ps -o tpgid=` of the in-VM bash equals its pid; with a job running it differs |
| Pause | STOP of the job alone does **not** freeze a loop typed at the prompt; STOP shell **then** job freezes it; CONT job then shell resumes — the host rule in `pty.rs`, unchanged |
| Cancel | `^C` through the PTY ends the loop and the shell stays usable; in-VM `TERM` to the foreground group → status 143 |
| Teardown | `sbx rm -f` under a live shell ends the host client, exit 137 |
| `sbx ls --json`, empty | `{"sandboxes": []}`, exit 0 — never the human "No sandboxes found." text |
| `sbx version` / `sbx daemon status` | exit 0; stdout `sbx version: v0.46.0 <sha>` and first line `Status: running` |
| `--cpus` | integer only: `--cpus 1.5` → exit 1 `invalid argument "1.5" for "--cpus" flag` |
| Names | underscores rejected (`sandbox name cannot contain underscores`); upper case and dots accepted |
| **Locked OS keychain** (2026-10-06, an SSH session with gnome-keyring locked) | `sbx create` with the default `--pull always` → exit 1 `encode registry auth: … docker login service unavailable`; with `--pull missing` it succeeds with a `WARN: failed to list stored credentials … the OS keychain is locked` on stderr. **`sbx rm -f` fails** (`snapshot associated scoped secrets: … the OS keychain is locked`) while **`sbx stop` succeeds**. A headless daemon can therefore create but not remove sandboxes while the keyring is locked |
| Sandbox init `sleep` | every sandbox runs a `sleep` as **pid 3** (SigIgn `0x6`), so `pgrep -x sleep` in a VM is never empty; probes and tests must match their own command line exactly (`pgrep -fx 'sleep 300'`) |
| Live host, end to end (05b review) | a real `daemoneye shell-host` with a tagged sbx child: Status Idle/Running/Paused correct; STOP froze a tick loop, CONT resumed, INT (`^C`) ended it, Close made the host exit 0 and ended an in-VM `sleep 300` |

### Decisions on the open questions (PE, 2026-10-05 — all recommendations accepted)

- **Q1 — `network = "proxy"` semantics.** `none` → `--deny-network '**'`.
  `proxy` → the operator's global sbx policy, narrowed by the profile's
  `proxy_deny` (`--deny-network`), with `proxy_allow` **adding** per-sandbox
  allows (`sbx policy allow network --sandbox`). This is wider than M18's
  strict allowlist: sbx cannot express "deny all, allow a list" per sandbox
  (deny beats allow, measured). README and `security.md` say so, config load
  warns that `proxy_allow` is additive, and status shows the effective rule
  count.
- **Q2 — service secrets.** A profile gets `secrets = []` by default. Each
  sbx service the profile does not list is denied at the network layer for
  that sandbox (`--deny-network` on the service's hosts), so a ghost cannot
  make authenticated calls with the operator's stored credentials unless its
  profile opts in. Note the sentinel env vars are present in every sandbox
  even when no secret is stored (pinky has none: `sbx secret ls` → "No
  secrets found"), so their presence proves nothing.
- **Q3 — script staging.** `sbx cp` the one script into the sandbox; the
  scripts directory is never mounted.
- **Q4 — workspace.** Ghost sandboxes are mountless in M26; no runbook option.
- **Q5 — chat `wait: false`.** One sandbox per chat session, created on first
  use and removed when the session's shells are reaped.
  *Refined at row-07 drafting (2026-10-07):* dedicated background shells are
  never closed and nothing closes chat shells, so "reaped" alone would keep a
  chat sandbox up for the daemon's lifetime. Row 07 therefore closes a
  sandboxed command's dedicated shell after its completion message lands, and
  the hourly reaper removes a chat sandbox that no shell record names once it
  has been unused for 600 s.
- **Q6 — pause vs deadline.** A pause suspends the command's deadline clock,
  as prompt time already does.

### Baseline red on pinky (found at phase-01 review, 2026-10-05) — blocks row 02

`master` at `c1613db4` does not pass two gates on pinky, the new development
host. Row 02 must not be dispatched until both are fixed; the executor's
gates would fail on code it never touched.

- **`cargo clippy --all-targets --all-features -- -D warnings`: 4 errors**,
  lints new in rustc/clippy 1.97.1: `unneeded pattern … the .. pattern can
  match that element` at `src/cli/viewer.rs:263` and
  `src/daemon/executor/foreground.rs:142`; `use of Some(x).filter(|_|
  predicate)` at `src/shell/pty.rs:508` and `:517`.
- **`cargo test`: `tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child`
  fails every time** (30.0 s, three runs). pinky's `/bin/sh` is dash, and
  `sh -c 'sleep 30'` **forks** `sleep` (measured: `sleep 30` runs as a child
  of `sh -c sleep 30`). `bounded_output_with` (`src/tmux/mod.rs:73`) kills
  only the direct child, then joins the pipe readers, which wait for the
  grandchild that still holds the pipes. The test exposes a real defect:
  a timed-out tmux call whose child forked does not return on time.

Fixed by row **01b** (`998bb9d4`, done 2026-10-06): `master` is green on
pinky with rustc/clippy 1.97.1.

### Flaky test on pinky (found at phase-02 review, 2026-10-06)

`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
(`src/shell/backend.rs`, last touched by M23) fails on roughly half of full
`cargo test` runs on pinky. It failed 118 times in the 01b runaway, and twice
of three at the phase-02 review. It passes solo and with its `shell::backend`
siblings. When it fails, cargo stops before the other test binaries, so the
pass count drops too. Every phase now carries a "run at most twice" rule
because of it. **Cause (measured 2026-10-06):** the test fakes the prompt with `printf`,
so readline echoes the credential back as an `"o"` event, and it reads the
cast once, before polling. It passes only when that one read beats the echo.
The product is correct. Row **02b** gives it the `read -rs` fixture its
sibling already uses; measured at 4/4 green full library runs.
**Fixed 2026-10-06** (`96e061b7`): 3/3 executor runs and the reviewer's
first full run green. Later phases no longer need the run-twice rule.

### Out of scope

- Foreground chat commands in a sandbox.
- Enforcing `sandbox_profile` on `[hosts.<name>]` remote hosts.
- `sbx --cloud` sandboxes; sbx kits/templates authored by DaemonEye; the sbx
  MCP gateway.
- Hosts without sbx: there is no docker fallback after row 09. On such a host
  the sandbox stays disabled, and enabling it fails closed.
- tmux removal (M27).

### Risks

| Risk | Retired in |
|---|---|
| sbx is pre-1.0 (v0.46.0) and its CLI surface moves | 02 — one module owns every argv; preflight records `sbx version` in the run record; doctor warns on an untested major/minor |
| Finding the in-VM foreground job for `Ctrl-p` | 01 measures (e.g. `ps -o tpgid=` of the in-VM bash recorded at shell start); 05 implements |
| sbx needs the operator's unlocked OS keychain: with it locked, `rm` fails (measured), so a ghost's teardown cannot complete | 02 classifies the error; 06 falls back to `sbx stop` and leaves removal to the startup/periodic sweep; status names the locked keychain |
| Leaked sandboxes hold a VM each (memory default: 50 % of host RAM, clamped) | 03 sets `--memory`/`--cpus` from `[sandbox.limits]`; 06 sweep; live close checks `sbx ls` is empty after teardown |
| A `proxy` profile is wider than an M18 allowlist | Q1/Q2 decisions; documented in README and `security.md` |
