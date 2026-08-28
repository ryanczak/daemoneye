# M18 design — Container-sandboxed Agents

**Scoped:** 2026-08-28. Settled design for running daemoneye agent *command
execution* inside rootless Docker containers while the daemon (the broker)
remains native. Revised 2026-08-28 after architect review: tool disposition
table added; credential mount removed; network default hardened to `none`;
ghost escape-hatch policy resolved; IPC respecified as socket enums.
**Revised again 2026-08-28 after standing up the runtime and measuring it** —
D1's uid model was wrong and is corrected (with it, the run-as-root
decision), D4's script bind-mount is replaced by per-run staging, and D5's
host-bound proxy is replaced by a containerized one. Every fact in D1/D4/D5
marked *measured* was executed on scrappy, not reasoned about.

## Problem

DaemonEye agents — the `chat` agent, ghost shells, and the named
`analyst/architect/researcher/sysadmin` profiles — are LLM-driven loops whose
effect on the system is carried out *directly on the host* through
`run_terminal_command` and the script executor. That gives each agent the same
privilege the daemon has: the daemon's user (host uid 1000 = `matt`), host
process tree, host filesystem, tmux, sudoers, and network.

The blast radius of a *compromised agent* is therefore the blast radius of the
daemon itself. Today the only mitigations are behavioural: the approval UI,
sudoers gating (`daemoneye install-sudoers`), and masking. None of these stop a
degenerate-but-approved command, a script that writes outside its lane, or an
LLM prompt-injection that yields a host command the operator approves while
misreading its effect.

**The fix is architectural, not procedural**: separate the *control plane*
(the daemon) from the *work plane* (each agent), and give the agent execution
an explicit, bounded sandbox. Prompt-injection then becomes
container-breakout-targeted — a much smaller, better-understood surface.

### Framing correction (important)

In DaemonEye the agent LLM loop *is* the daemon — `run_conversation_loop`
runs in-process, and only **tool effects** touch the host. So this design does
not "put agents in containers"; it re-homes the *execution backend* of the
command-shaped tools into containers. Most of the 36 AI tools are broker-side
bookkeeping (tmux inspection, knowledge CRUD, scheduling) and are unaffected.
The disposition table in D0 makes this explicit, tool by tool.

## Goals / Non-Goals

**Goals**
- Every *background* command and script an agent triggers runs inside an
  ephemeral, resource-limited container **as container uid 1000, never as
  container root** — see D1: under rootless Docker container root *is* host
  `matt`, which is precisely the blast radius this design exists to remove.
- The daemon (native) keeps full ownership of tmux, webhook, scheduling,
  approval, and the socket — sandboxed execution happens only through the
  `ContainerExec` backend at the executor choke point.
- Ghost shells get a disposable container per run; chat sessions get a
  long-lived container that can be restarted independently of the daemon.
- Mount policy is minimal and read-mostly: **only the approved script for
  this run** (staged, ro), runbook ro, empty scratch volume. **No credentials
  enter the sandbox.**
- Default network is `none`. Workloads that need egress get it only through a
  **containerized** proxy on a shared user-defined network, per profile — a
  host-bound proxy is unreachable (D5).
- Host-level ops happen ONLY via an explicit escape-hatch classification —
  interactively approved in chat, allowlist-pre-approved or parked for
  unattended ghosts.

**Non-Goals**
- Sandboxing the daemon itself (this is that design's complement, not a
  substitute — the daemon is intentionally native so it can mediate tmux and
  sudoers).
- Replacing the approval UI — approvals still gate foreground commands,
  file edits, and escape-hatch ops exactly as today.
- Sandboxing **foreground** execution. Commands injected into the user's pane
  via `send-keys` are host-level *by design*: user-visible, user-approved,
  running in the user's own shell. They keep the existing approval flow and
  are out of sandbox scope (see D0).
- Sandboxing **remote** execution. Commands targeting ssh/mosh panes
  (`target_pane`) execute on the remote host and are foreground-classified;
  a container runtime on a remote host belongs to a *separate* daemon
  instance.

## Status

| State | Value |
|-------|-------|
| Runtime decision | **Docker (rootless)** — made 2026-08-28 |
| Enforcement point | executor backend (`ContainerExec`), not prompts |
| Execution identity | **container uid 1000** (host-visible 100999), never container root |
| Network default | `none`; egress only via a containerized proxy, per profile |
| Security posture | defence-in-depth; the audit trail remains the primary control |
| Runtime verified | 2026-08-28 on scrappy — docker 29.7.2 rootless, rootlesskit 3.1.0, slirp4netns 1.3.5 |

## Current architecture (what we build on)

- **Daemon** runs natively as `matt` (`daemoneye daemon --console`), owns the
  unix socket `~/.daemoneye/var/run/daemoneye.sock` (mode 0700),
  webhook `:9393`, and scheduling.
- **Agents** request operations; the daemon executes them
  (`src/daemon/executor/…`). The executor already fans out by namespace
  (`file_ops/`, `knowledge/`, `foreground.rs`, `schedule.rs`) — this is the
  natural insertion point for a `container/` backend.
- **Background execution** today *is* tmux: `run_background_in_window()`
  creates a `de-bg-*` window on the daemon host, monitors via `pane-died`,
  GCs via `gc_bg_windows()`. This visibility model is preserved (D3): the
  sandboxed process runs *inside* the `de-bg-*` window via `docker exec`, so
  the user can still watch a background job in a pane.
- **Ghost shells** (`spawn_ghost_shell`) already carry runbook-scoped policy
  (`GhostPolicy`: approved scripts, sudo, turn budget) — they are the easiest
  place to prove the sandbox because their lifecycle is already bounded.
- **Sudoers** are host-level NOPASSWD rules installed per script
  (`daemoneye install-sudoers` → `/etc/sudoers.d/daemoneye-<name>`). A
  rootless container cannot exercise host sudo at all, so sudo-needing
  scripts are escape-hatch ops under this design (D6) — there is no
  "container-level caps" middle ground.
- **AI backend** is remote and HTTP (brain `:8888` / vLLM) — and it is the
  *daemon* that talks to it. The sandbox needs no AI egress and no API
  credential (D4/D5).

Verified facts from the host — **all measured 2026-08-28, not assumed**:
- Rootless Docker is now **installed and running** on scrappy: docker 29.7.2,
  rootlesskit 3.1.0, slirp4netns 1.3.5, as a systemd **user** unit
  (`systemctl --user … docker.service`), socket
  `/run/user/1000/docker.sock`. `docker info` reports
  `["name=seccomp,profile=builtin","name=rootless","name=cgroupns"]`.
- **Arch ships no `dockerd-rootless-setuptool.sh`** — `docker-rootless-extras`
  provides the systemd user unit and `99-docker-rootless.conf` instead, so the
  upstream setup tool is replaced, not missing.
- **`slirp4netns` is not a dependency and must be installed explicitly.**
  rootlesskit embeds no network driver (`--net` needs an external binary for
  every non-`host` value); without it `dockerd-rootless.sh` exhausts its
  slirp4netns → pasta → vpnkit → gvisor-tap-vsock chain and fails to start.
- **`loginctl enable-linger matt` is required.** Rootless dockerd is a user
  service; without linger it dies with the last login session, while the
  daemoneye daemon persists — the failure would present as a daemoneye bug.
- Client gotcha: a stale `~/.docker/config.json` from a prior Docker Desktop
  install carried `"credsStore": "desktop"`, breaking every `docker pull`
  with `docker-credential-desktop: executable file not found`. Removed
  (`auths` was empty; backup at `config.json.bak-20260828`).
- tmux socket: `/tmp/tmux-1000/default`, mode `srwxrwx---`, owned by uid 1000
  (`matt`). Container must never need to touch it; daemon does.
- Daemon binary: `~/.daemoneye/bin/daemoneye` (mode 0700, 25.5 MB, rust,
  static-ish).
- Agents dir: `~/.daemoneye/agents/` holds `analyst`, `architect`,
  `researcher`, `sysadmin`.

## Design decisions

### D0 — Tool disposition table (the contract)

Every AI tool is classified. **Sandboxed** = effect runs inside the container
via `ContainerExec`. **Broker-native** = effect stays in the daemon process,
unchanged by this design (existing approval gates still apply). **Foreground**
= host-level by design, user-approved, out of sandbox scope. **Escape-hatch**
= host op mediated by D6.

| Tool(s) | Disposition | Notes |
|---|---|---|
| `run_terminal_command` (background mode) | **Sandboxed** | the core change; runs via `docker exec` inside the `de-bg-*` window (D3) |
| `run_terminal_command` (foreground mode) | **Foreground** | `send-keys` into the user's pane; user-visible, user-approved, host-level by design. Remote (`target_pane` over ssh/mosh) is a sub-case of this. |
| script execution (via ghost/scheduler) | **Sandboxed** | non-sudo scripts run in-container from the ro mount; sudo scripts are escape-hatch (D6) |
| `edit_file`, `read_file` | **Broker-native** | daemon-host filesystem with existing approval + diff + path blocks. Rationale: their targets are the operator's own files (configs, code) — moving them in-container would gut the tool. Container-scratch access (`/de/work`) goes through `container:run`, not these tools. |
| `get_terminal_context`, `list_panes`, `read_pane`, `find_in_panes`, `watch_pane`, `tmux_control`, `close_background_window` | **Broker-native** | tmux is the daemon's exclusive property; the container never sees the tmux socket |
| `search_repository`, `recall_context`, `load_tools` | **Broker-native** | daemon-side state |
| `write_script`, `delete_script`, `read_script`, `list_scripts` | **Broker-native** | scripts live on the daemon host; the sandbox sees them ro at `/de/scripts` |
| `write_runbook`, `delete_runbook`, `read_runbook`, `list_runbooks` | **Broker-native** | |
| `add_memory`, `read_memory`, `update_memory`, `list_memories`, `delete_memory` | **Broker-native** | memory dirs are never mounted |
| `schedule_command`, `list_schedules`, `cancel_schedule`, `delete_schedule` | **Broker-native** | scheduled *commands* execute sandboxed when they fire |
| `spawn_ghost_shell`, `await_agent_result` | **Broker-native** | spawn allocates the ghost's container (D3) |
| `create_agent`, `read_agent`, `list_agents`, `delete_agent` | **Broker-native** | |

Net effect: exactly one tool changes execution backend. That is the honest
size of this design — small choke-point change, large blast-radius change.

### D1 — Runtime: Docker, rootless, user-namespace-mapped execution

Docker selected over Podman per user decision 2026-08-28. Execution is
**rootless**.

**Correction (measured 2026-08-28 — the first draft of this section was
wrong).** The draft claimed the userns "maps host uid 1000 → container uid
1000". It does not. The real map, read from `/proc/self/uid_map` inside a
container:

```
         0       1000          1        # container root  → host 1000 (matt)
         1     100000      65536        # container 1..65536 → host 100000..165535
```

Measured both directions: a default (container-root) process is
**host-visible as uid 1000 `matt`**; a `--user 1000:1000` process is
**host-visible as uid 100999**.

**This inverts the decision.** Container root *is* host `matt` — the exact
identity whose blast radius this whole design exists to shrink. A breakout
from a container running as root lands you back at the starting threat model.
A breakout from `--user 1000:1000` lands on host uid 100999, a subuid that
owns no file on the system.

**Decision: every sandboxed process runs as `--user 1000:1000`.** Container
root is used only by the short-lived staging helper (D4), which never runs
agent-controlled code.

Failure modes to guard against, each of which silently returns execution to
host `matt`: omitting `--user`, `--userns=host`, `--privileged`, or any
`USER root` in the image. The D1 gate (§ Testing) exists solely to catch
this, and it asserts the *host-visible* uid, because that is the number the
threat model is about.

**New attack surface, stated plainly**: the rootless Docker API socket
(`$XDG_RUNTIME_DIR/docker.sock`) is reachable by any process running as
`matt`, and controlling it controls every container. This is the same trust
boundary as today (any `matt` process can already do anything `matt` can),
so it is accepted — but it is listed in Risks, and the socket is never
mounted into any container.

### D2 — Enforcement point = executor backend

The agent prompts are not trusted; the *executor* is. Today the quiet
inheritance of privilege is the risk. The change: background execution of
`cmd`/`script` requests travels through a new `ContainerExec` backend instead
of local `Command`. That backend is the single choke point where every
sandboxed op is namespaced: which image, which mounts, which network, which
user, which resource limits.

### D3 — Container lifecycle

- **Chat sessions**: one long-lived `de-chat-<session>` container per chat
  session, created lazily on first sandboxed op, restarted independently of
  the daemon. GC'd when the session ends or on daemon restart sweep.
- **Ghost shells**: one ephemeral container per `spawn_ghost_shell`,
  destroyed on ghost completion regardless of success (`docker rm -f` on
  every exit path, including panic). Labeled `de.ghost=1` for the orphan
  sweep.
- **Named agents**: **one base image, per-profile policy** — not per-profile
  images. A profile (analyst, researcher, sysadmin, …) selects mount set,
  network mode, and resource limits from config; the image is shared.
  Rationale: four images plus base plus tools is a real maintenance surface
  (rebuild cadence, pinning, drift vs. runbook expectations) with no security
  win over mount/network policy. Revisit only if a pilot shows a profile
  needs tooling the base image shouldn't carry.
- **Background-window integration**: the `de-bg-*` tmux window model is kept.
  `run_background_in_window()` launches `docker exec …` *as the window's
  command*, so `pane-died` completion detection, output capture/archive,
  `close_background_window`, the 5-window cap, and the user's ability to
  watch the job in a pane all survive unchanged. The process inside the pane
  is sandboxed; the pane itself is still the daemon's.
- **Warm pool**: deliberately **not** designed now. Measure container start
  latency in the ghost pilot first; a `docker create` pre-warm is a
  contained follow-up if the numbers demand it.

### D4 — Mount policy

**A bind-mount of `~/.daemoneye/scripts/` does not work and is not used.**
Measured 2026-08-28: that directory is `drwx------ matt matt` and its scripts
are `-rwx------` (the chmod-700 property `src/scripts.rs` enforces). Under the
D1 decision the process is host uid 100999, so the mount reads `READ DENIED`.
Relaxing those permissions to make a bind-mount work would trade a real
host-side security property for container convenience — rejected.

Instead the daemon **stages** into a per-run named volume. Verified end to
end 2026-08-28:

1. Daemon creates a per-run volume (`de-stage-<job_id>`).
2. A short-lived helper container (container root = host `matt`, so it can
   read the 0700 originals) copies in **only the script approved for this
   run**, `chmod 0500`, `chown 1000:1000`.
3. The agent container mounts that volume at `/de/scripts:ro` and runs as
   `--user 1000:1000` — the script executes; a write to the mount fails with
   `Read-only file system`.
4. The volume is removed with the container.

This is strictly tighter than the bind-mount it replaces: the sandbox sees
one vetted script, not the whole library.

| Source | Container path | Mode | Rationale |
|--------|----------------|------|-----------|
| staged volume (approved script only) | `/de/scripts` | RO | vetted automation, staged per run — never a bind-mount of the 0700 dir |
| staged volume (per-runbook file) | `/de/runbook.md` | RO | context, not mutable |
| `--tmpfs` sized by `[sandbox.limits] scratch` | `/de/work` | RW | agent scratch, destroyed with container; verified writable as uid 1000 |
| *(no log mount — relay only)* | *(none)* | — | agents hand log lines to the daemon via a `log` opcode; daemon appends to the real event log |

**No credential mount.** The daemon runs the LLM loop and talks to the AI
backend; nothing inside the sandbox needs the API key, and it is precisely
the secret a compromised workload must not hold. (The earlier draft mounted
`/de/cred:ro` — removed.)

Explicitly NOT mounted: `etc/config.toml`, memory dirs, `var/run` (socket),
the rootless Docker socket, `.ssh`, shell rc. If a workload needs host
state, it asks the relay; it does not reach through the filesystem.

### D5 — Network policy

**Default: `--network=none`.** With the credential mount gone (D4), the
common case — run a command over staged data, return output — needs no
network at all, and `none` is categorically stronger than any filtering.

This default is load-bearing, not decorative. Measured 2026-08-28 on the
**default** docker network, a container reached the LAN AI backend
(`192.168.50.90:8888/v1/models`), answered ICMP across the LAN, and fetched
`https://example.com` — full LAN + internet egress. Under `--network=none`
the same probes report `NET_BLOCKED` with only `lo` present.

**Correction (measured): a host-bound proxy is unreachable, so the earlier
"daemon-owned proxy on the host" mechanism does not work.**
`dockerd-rootless.sh` runs slirp4netns with `--disable-host-loopback`
(confirmed in the live process args). A container probing the slirp gateway
`10.0.2.2` reached **neither** a 127.0.0.1-bound listener nor a
0.0.0.0-bound one — both `BLOCKED`. Disabling that flag to make a host proxy
reachable would re-open every host loopback service to every container, which
is a strictly worse trade.

**Mechanism, corrected: the egress proxy is itself a container.** For a
profile declaring `network = "proxy"`, the daemon runs a proxy container on a
dedicated user-defined network and attaches the agent container to that
network only. The agent gets `HTTP(S)_PROXY` pointing at the proxy's service
name; the proxy enforces the per-profile hostname allowlist and logs every
request to the event log. `--disable-host-loopback` stays on, and the agent
container still has no route to the host or the wider LAN — the proxy is the
only door, and it is audited.

Host services (webhook `:9393`, grafana) are reachable by the daemon, never
by a container.

Per-profile setting: `network = "none" | "proxy"` in the sandbox profile.

### D6 — Escape-hatch protocol (host-level ops)

Some ops legitimately need host access (cachy-update, zpool, sudoers install,
sudo-requiring scripts, tmux moves). These are mediated — never automatic —
and the policy differs by whether anyone is watching:

**Interactive (chat):**
1. Agent requests host-level op → daemon flags it **escape-hatch**, a
   distinct classification in the approval prompt (not just "run this").
2. Approval runs the host command **outside the sandbox**.
3. Every usage is logged with agent identity, container id, command, and
   reason — an event accessible by search (future alert target).

**Unattended (ghost shells, scheduled jobs)** — there is no operator to
approve at 03:00, and remediation runbooks are exactly where host ops occur,
so silence here would make ghosts useless or make the hatch a fiction:
1. `GhostPolicy` grows `escape_allowlist: Vec<String>` — script names (from
   `~/.daemoneye/scripts/`, the already-vetted set) that this runbook may run
   on the host without live approval. Populated in runbook frontmatter;
   installing the runbook is the approval moment. Arbitrary host *commands*
   are never allowlistable — scripts only, so the vetted-artifact property
   is preserved.
2. An escape-hatch request **not** on the allowlist does not fail the ghost:
   the op is **parked**, written to the ghost's mailbox result and injected
   as a `[Ghost Shell …]` event so it surfaces in the catch-up brief; the
   operator runs or dismisses it later.
3. Both paths log identically to the interactive case.

Sudo-requiring scripts (the `install-sudoers` set) are always escape-hatch:
rootless containers cannot exercise host sudo, so the sudoers mechanism and
the hatch are now the same door.

## Proposed changes

### Config schema

```toml
[sandbox]
enabled  = false                       # feature flag; default OFF for rollout
runtime  = "docker"                    # only supported value today
image    = "daemoneye-agent-base"      # tag pinned by digest in lockfile, see Image lifecycle
workdir  = "/de/work"
run_as   = "1000:1000"                 # D1: never container root; host-visible 100999
docker_host = "unix:///run/user/1000/docker.sock"

[sandbox.limits]
memory     = "1g"
pids       = 256
cpus       = 2.0
scratch    = "2g"                      # /de/work tmpfs size

[sandbox.profile.researcher]
network = "proxy"                      # default profile network is "none"
proxy_allow = ["crates.io", "docs.rs"]

[sandbox.ghost_defaults]
destroy_on_exit = true                 # always true today; reserved
mount_scripts   = "ro"
```

### Executor backend

New `src/daemon/executor/container.rs` (a sibling of `foreground.rs`, not
under `knowledge/` — it is an execution backend, not a knowledge tool)
exposing:

- `container:run` — core opcode: `docker exec` (create-if-missing) with the
  profile's image/mounts/limits; returns bounded output.
- `container:esc` — escape-hatch request; never auto-executes; interactive →
  approval round, unattended → allowlist check or park (D6).
- `container:info` — runtime health for observability (`daemoneye status`).

(The earlier draft's `container:shell` interactive REPL is deferred with the
tty open question — not needed for the ghost pilot or background commands.)

### Ghost lifecycle wiring

`spawn_ghost_shell` obtains a `job_id`/`container_id` pair. On ghost
completion the daemon destroys the container (`docker rm -f`), guaranteeing
no cross-run state even on panic/fail paths. Daemon start runs an orphan
sweep by label `de.ghost=1`.

### IPC surface

New `Request`/`Response` enum variants in `src/ipc.rs` (newline-delimited
JSON over the unix socket — this project has no HTTP API surface):

- `Request::ContainerStatus` → `Response::ContainerStatus { runtime_ok,
  containers: Vec<ContainerInfo> }` — per-session/per-ghost container state
  for `daemoneye status` and the `/panes`-style inspector.
- Escape-hatch approval reuses the existing `ToolCallPrompt` /
  `ToolCallResponse` round-trip with a new `escape_hatch: bool` field so the
  client renders the distinct classification.

### Image lifecycle (supply chain)

- One Dockerfile under `containers/` in this repo: Alpine base + curl, jq,
  git, python3, coreutils. Base image pinned by digest, not tag.
- `daemoneye sandbox build` (new subcommand) builds/rebuilds locally; the
  resulting image digest is recorded in `~/.daemoneye/etc/sandbox.lock` and
  the daemon refuses to start containers from a different digest (loud
  error, not silent fallback).
- Rebuild is an operator action (after base-image CVE or tool addition);
  nothing auto-pulls. A stale-image warning (>90 days since build) lands in
  `retention_warnings()`.
- Runbooks that assume in-container tools must name them in frontmatter
  (`requires_tools = ["jq"]`); `container:run` verifies presence once per
  container and fails fast with a clear error instead of mid-run mystery.

## Testing

- **UID mapping gate** (rewritten — the first draft asserted "host uid 1000
  and container uid 1000", which D1 measured to be mutually exclusive and so
  could never pass). At daemon start, run a probe container as
  `--user 1000:1000` and assert **both**:
  - in-container `id -u` = 1000, and
  - the **host-visible** uid of the container process = **100999**
    (i.e. `subuid_base + 999`, read from `/etc/subuid`, not hardcoded),
  and additionally assert `/proc/self/uid_map` contains a `0 <host_uid> 1`
  line — proving container root maps to the daemon's own uid, which is the
  condition that makes running as root unacceptable.
  On mismatch, refuse **sandboxed execution** (never the daemon itself).
- **Mount-surface assertion**: from inside a fresh container, assert the
  *absence* of every non-mounted host path (`~/.daemoneye/etc`, memory dirs,
  `~/.ssh`, tmux socket path, Docker socket) and that a write to
  `/de/scripts` fails with `Read-only file system`. (Replaces the earlier
  `/etc/shadow` test, which only ever read the container's own harmless
  shadow file and would pass vacuously.)
- **Staging assertion** (D4): `/de/scripts` contains **exactly** the script
  approved for this run and no other; the staged file is mode `0500` owned by
  container `1000:1000` and is executable by the agent process. Negative
  half: a second, non-approved script in `~/.daemoneye/scripts/` is absent
  from the mount.
- **Relay refusal**: a host-path request through the broker (e.g. ghost asks
  to run a non-allowlisted host script) is refused/parked without approval —
  asserted through the daemon's actual door (event log + mailbox), not the
  container.
- Ghost lifecycle: kill -9 daemon mid-ghost; on restart, no orphaned ghost
  containers remain (orphan sweep by label `de.ghost=1`).
- Network: default-profile container has no route to anything (`none`);
  proxy-profile container reaches an allowlisted host through the proxy
  (logged) and fails to reach a non-allowlisted one.
- Resource limits: fork bomb in-container hits `pids` limit; scratch write
  beyond `scratch` limit fails; daemon and host unaffected.
- Background-window integration: sandboxed background job is visible in its
  `de-bg-*` pane, completion detection and output archive work unchanged.

## Rollout

1. ~~Install rootless docker~~ — **DONE 2026-08-28.** On Arch the accurate
   sequence is `pacman -S docker docker-rootless-extras slirp4netns`
   (slirp4netns is *not* pulled in and is required), then
   `systemctl --user enable --now docker.service` — there is **no**
   `dockerd-rootless-setuptool.sh` on Arch — then `loginctl enable-linger
   matt`. Set `DOCKER_HOST=unix:///run/user/1000/docker.sock`. Verified
   working; uid map and mount/network behaviour measured into D1/D4/D5.
2. Build `daemoneye-agent-base` via `daemoneye sandbox build`; commit
   Dockerfile under `containers/`; lockfile digest recorded.
3. Implement the `ContainerExec` backend + D0 routing behind
   `[sandbox] enabled`; default OFF. Broker-native tools untouched.
4. Ghost-shell pilot (flag ON) for a single low-risk, network-none runbook.
   Capture: container start latency (decides warm pool), escape-hatch
   park/allowlist counts, failed sandbox starts.
5. Chat-session containers opt-in; then default ON with escape-hatch
   observation. Foreground execution is unaffected at every step.

## Open questions

- Interactive REPLs inside the sandbox (`container:shell`, tty relay) —
  deferred; not needed for background commands or ghosts.
- GPU/ROCm for in-container inference experiments — pass /dev/kfd, /dev/dri?
  Likely yes for a future "lm-studio-in-container" ghost, but NOT in-scope
  now.
- Backup/restore of container-local runtime (`/de/work`) — intentionally
  ephemeral; decisions to persist go back through scripts.

## Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Container runs as root → host `matt` (the original blast radius) | D1: `--user 1000:1000` always; gate asserts host-visible uid 100999 and refuses sandboxed exec on mismatch |
| UID mapping breakage (kernel/docker update, `--userns=host`, `USER root` in image) | same D1 gate, run at daemon start |
| Rootless dockerd dies at logout (user service) | `loginctl enable-linger`; `container:info` surfaces runtime-down as a clear error, never a silent host fallback |
| Rootless Docker API socket reachable by any `matt` process | accepted — same trust boundary as today; socket never mounted into containers; stated here so it is not rediscovered as a surprise |
| Runaway container resources | `[sandbox.limits]`: memory, pids, cpus, scratch size; GC on ghost exit |
| Stale/drifted image breaks runbooks | digest lockfile + refuse-on-mismatch; `requires_tools` fail-fast; >90-day staleness warning |
| Egress proxy becomes a bypass | allowlist is per-profile hostnames; every request logged; default profile has no network at all |
| Agent latency (container start per op) | long-lived chat container; ghost pilot measures before any warm-pool work |
| Legitimate host ops friction | escape-hatch with distinct UX; ghost allowlist (vetted scripts only) + park-and-notify keeps unattended runbooks viable |
| Unattended ghost blocked on host op | park-and-notify (D6): op lands in mailbox + catch-up brief instead of failing the ghost |

## Notes / lessons

- Decision log (runtime choice, enforcement point) is in
  `runbooks/agent-container-sandboxing.md` — keep the two artifacts in sync.
- Milestone number: this is **M18** (M17 closed 2026-08-20, commit
  `69b7e76`); the first draft mis-titled it M17.
- Capture after first real pilot: escape-hatch counts/day (allowlisted vs.
  parked), failed sandbox starts, container start latency, UID-map breakage
  on kernel updates.
