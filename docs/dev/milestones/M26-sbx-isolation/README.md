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
| 01b | baseline-green ([phase-01b-baseline-green.md](phase-01b-baseline-green.md)) — fix the 4 clippy 1.97 lints and make `bounded_output_with` kill the child's process group on timeout (see § Baseline red on pinky) | todo |
| 02 | sbx-argv — `src/daemon/executor/sbx.rs`: pure argv builders (`create`/`exec`/`stop`/`rm`/`ls --json`/`policy allow|deny --sandbox`), `ls --json` parser, sandbox naming, profile → flags mapping, preflight (`sbx version`, `sbx daemon status`) cached and fail-closed | todo |
| 03 | sbx-config — `[sandbox] runtime = "sbx"`; `limits.memory`/`cpus` → `--memory`/`--cpus`; `image` → `--template`; docker-only keys (`run_as`, `docker_host`, `proxy_image`, `limits.pids`, `limits.scratch`, `ghost_defaults.mount_scripts`) warn and are ignored | todo |
| 04 | sbx-shell — `ShellRecord` carries `sandbox: Option<String>`; `spawn_for` launches the shell-host with child argv `sbx exec -it -w <workdir> <sandbox> bash`; adoption at startup recovers the sandbox name (this row measures it against the real shell-host); reuse is sandbox-keyed (never hand a sandbox shell to a host caller, or the reverse) | todo |
| 05 | sbx-signals — pause/resume/cancel for a sandbox shell signal the **in-VM** foreground job through a side `sbx exec`; cancel also kills in-VM (killing the client orphans the job — measured); pause-vs-deadline decision lands here | todo |
| 06 | sbx-ghost — ghost start creates its sandbox (one per run), local commands route to the sandbox shell, script staging, teardown on every exit path; startup sweep of leaked `de-*` sandboxes | todo |
| 07 | sbx-chat-background — chat `wait: false` in a sandbox shell when enabled; sandbox lifetime per chat session (see Q5) | todo |
| 08 | sbx-status — `Request::ContainerStatus` / the `SANDBOX` status section / `daemoneye sandbox status` read `sbx ls --json` + `sbx policy log --json`; per-run egress audit into the run record | todo |
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

Proposed: one row **01b — baseline-green**, dispatched to the executor
before row 02.

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
| Leaked sandboxes hold a VM each (memory default: 50 % of host RAM, clamped) | 03 sets `--memory`/`--cpus` from `[sandbox.limits]`; 06 sweep; live close checks `sbx ls` is empty after teardown |
| A `proxy` profile is wider than an M18 allowlist | Q1/Q2 decisions; documented in README and `security.md` |
