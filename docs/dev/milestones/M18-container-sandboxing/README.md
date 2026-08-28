# M18 — Container-sandboxed Agents

**Goal:** Every background command or script an agent triggers executes inside
an ephemeral, rootless, resource-limited Docker container instead of directly
on the host — with the daemon staying native, foreground execution untouched,
and host-level ops flowing only through an explicit escape hatch.

**Status:** planning

**Depends on:** none (M16 and M17 closed 2026-08-20).

**Operator prerequisite — SATISFIED 2026-08-28.** Rootless Docker is
installed and running on scrappy (docker 29.7.2, rootlesskit 3.1.0,
slirp4netns 1.3.5, systemd user unit, linger enabled,
`DOCKER_HOST=unix:///run/user/1000/docker.sock`). The accurate Arch recipe —
no `dockerd-rootless-setuptool.sh` exists there, and `slirp4netns` must be
installed explicitly — is recorded in the design's § Rollout step 1 and
§ Current architecture. Installing it was an operator/architect action, never
an executor task: the executor bash guard forbids it and the M16 phase-01
incident is the standing reason no executor touches host services.

**Standing up the runtime before drafting disproved three design claims**
(uid model, script bind-mount, host-bound proxy) — all three corrected in the
design of record. This is the "never assert an unexecuted spec fact" rule
paying out before a single phase was dispatched.

**Design of record:** `docs/design/agent-container-sandboxing.md` (revised
2026-08-28). The D0 tool disposition table is the contract: exactly one tool
(`run_terminal_command`, background mode) plus script execution changes
backend; everything else is broker-native or foreground and must be untouched.

**Exit criteria:**

- With `[sandbox] enabled = true`, a background `run_terminal_command` runs
  inside a container: the process tree under the `de-bg-*` pane is
  `docker exec …`, and `pane-died` completion detection, output
  capture/archive, and `close_background_window` work unchanged (live check).
- UID-mapping gate: a probe container run as `--user 1000:1000` reports
  in-container uid 1000 **and** host-visible uid **100999**
  (`subuid_base + 999`, read from `/etc/subuid`), and `/proc/self/uid_map`
  shows container root mapping to the daemon's own uid (live check; refusal
  path unit-tested against fixture output). **Note the corrected model** —
  the pre-2026-08-28 draft asserted host uid 1000 *and* container uid 1000,
  which measurement proved mutually exclusive.
- No sandboxed process runs as container root: `--user` is always passed, and
  a container started without it is refused (unit + live check).
- Staging: `/de/scripts` holds exactly the approved script for the run, mode
  `0500` owned `1000:1000`, with a non-approved script from the same host
  directory demonstrably absent (unit + live check).
- Mount-surface assertion: from inside a fresh container, every non-mounted
  host path is absent (`~/.daemoneye/etc`, memory dirs, `~/.ssh`, the tmux
  socket path, the Docker API socket) and a write to `/de/scripts` fails with
  `Read-only file system` (live check, scripted).
- No credential enters the sandbox: the container environment and filesystem
  contain no AI API key (live check; also a negative grep criterion on the
  mount-assembly code).
- Relay refusal: a ghost requesting a host script not on its
  `escape_allowlist` gets the op **parked** — mailbox result + `[Ghost Shell …]`
  event — with nothing executed on the host, asserted through the daemon's
  real door (event log + mailbox), not the container (unit + live check).
- Ghost lifecycle: `kill -9` the daemon mid-ghost; after restart no container
  labeled `de.ghost=1` remains (live check).
- Network: a default-profile container has no route out (`none` — verified
  necessary, since the *default* docker network reaches the LAN backend and
  the public internet); a proxy-profile container reaches an allowlisted host
  through the **containerized** proxy on a shared user-defined network
  (request logged) and cannot reach a non-allowlisted one (live check).
- Resource limits: an in-container fork bomb hits the `pids` cap and a
  scratch write beyond the `scratch` cap fails, with daemon and host
  unaffected (live check, isolated).
- Image lockfile: `daemoneye sandbox build` records the image digest;
  the daemon refuses sandboxed execution when the available image digest
  differs from the lockfile (unit-tested with fixtures; live check).
- With `[sandbox] enabled = false` (the default), behaviour is byte-for-byte
  today's: no docker invocation anywhere in a full chat + ghost round trip
  (unit + live check).
- All four gates green on a host **without** docker installed: `cargo fmt
  --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo test`. Docker-dependent tests are `#[ignore]`d with a
  reason string naming the runtime requirement; the ignored set is run
  explicitly at milestone close on the docker-equipped host.

Live checks are architect-run (M14–M17 convention: through the user's door,
session JSONL / event log as evidence anchors, isolated `tmux -L <name>`
servers so the operator's session is never touched).

## Architecture references

- `docs/design/agent-container-sandboxing.md` — the design of record
  (D0–D6, config schema, image lifecycle, testing).
- `CLAUDE.md` § "Request/Response lifecycle" and § "Key files" —
  `src/daemon/executor/`, `src/daemon/background/`.
- `docs/architecture.md` — orchestration layer.

## Design decisions on record

- **D0 disposition is frozen for the milestone.** Broker-native tools and
  foreground execution (including remote `target_pane`) are out of scope for
  every phase; a phase that touches them is mis-scoped by definition.
- **One base image, per-profile policy** — no per-agent images (design D3).
- **No credential mount, network `none` by default** (design D4/D5); egress
  only via a **containerized** proxy on a shared user-defined network for
  profiles that declare it — a host-bound proxy was measured unreachable
  (`--disable-host-loopback`).
- **Never container root** (design D1, measured): container root maps to host
  `matt`, the exact identity being sandboxed. All sandboxed execution is
  `--user 1000:1000` (host-visible 100999).
- **Scripts are staged per run, never bind-mounted** (design D4, measured):
  `~/.daemoneye/scripts/` is 0700 and unreadable at the non-root uid, and
  relaxing it would trade a host security property for container
  convenience. A root helper container stages only the approved script into a
  per-run volume.
- **Sudoers and the escape hatch are the same door** in the sandbox world:
  rootless containers cannot exercise host sudo, so sudo-needing scripts are
  always escape-hatch ops (design D6).
- **The rootless Docker API socket is accepted attack surface** (same trust
  boundary as any `matt` process today); it is never mounted into a
  container.
- **Everything lands behind `[sandbox] enabled`, default OFF**, so every
  phase keeps the unmodified path shippable.

## Phases

Ordering: 01 → 02 → 03 → 04 (core plumbing, each depending on the previous),
then 05 → 06 → 07 (execution integration), then 08–10. Phase docs are drafted
one at a time via `/rexymcp:architect next` — none are drafted ahead;
line-number facts go stale (M4/M16 precedent) and this milestone's Current
state will shift with each landing.

| #  | Phase | Status | Scope (one line) |
|----|-------|--------|------------------|
| 01 | sandbox-config ([phase-01-sandbox-config.md](phase-01-sandbox-config.md)) | **done** (approved_after_1, 2026-08-28) | `[sandbox]` config schema: `SandboxConfig` + limits + profiles + ghost defaults, parsing, validation, `assets/etc/config.toml` docs. Hermetic — no docker. |
| 02 | container-runtime-probe ([phase-02-container-runtime-probe.md](phase-02-container-runtime-probe.md)) | **done** (approved_after_1, 2026-08-28) | `executor/container.rs`: runtime version probe + D1 UID-map gate, all decision logic pure and fixture-tested; one `#[ignore]`d live test. Nothing wired yet. **IPC surface deferred** — see Notes. |
| 03 | image-lifecycle ([phase-03-image-lifecycle.md](phase-03-image-lifecycle.md)) | **in-progress** (bounced 2026-08-28, bug-phase-03-1) | `containers/Dockerfile`, `daemoneye sandbox build`, digest lockfile + the pure compare helpers phase-04's refusal gate uses. Staleness warning and `requires_tools` deferred — see Notes. |
| 04 | container-exec-backend | todo (not drafted) | `ContainerExec`: create-if-missing, `--user 1000:1000`, D4 per-run staging volume (root helper stages the approved script, chown 1000), scratch tmpfs **with `mode=0700,uid=1000,gid=1000`** (see Notes — without these flags it is not writable), `[sandbox.limits]` flags, `--network=none`, bounded output, `log` relay opcode. Calls the phase-02 gate and phase-03's `check_image_matches`; removes phase-02's `#[allow(dead_code)]`. Also carries the deferred `Request::ContainerStatus` + `daemoneye status` surface. Flag-gated. |
| 05 | background-window-integration | todo (not drafted) | Route background `run_terminal_command` through `docker exec` inside the `de-bg-*` window when enabled; completion detection, archive, cap, GC unchanged. |
| 06 | ghost-container-lifecycle | todo (not drafted) | Per-ghost ephemeral container, `docker rm -f` on every exit path, `de.ghost=1` label, startup orphan sweep. |
| 07 | escape-hatch | todo (not drafted) | Escape-hatch classification, `GhostPolicy.escape_allowlist`, park-and-notify (mailbox + `[Ghost Shell …]` event), `escape_hatch` flag on `ToolCallPrompt`, event-log records. |
| 08 | chat-session-containers | todo (not drafted) | Long-lived `de-chat-<session>` container, lazy create, restart-independent, session-end GC + restart sweep. |
| 09 | egress-proxy | todo (not drafted) | Daemon-owned egress proxy, `network = "proxy"` profile wiring, per-profile hostname allowlist, request logging. May be deferred at PE discretion — nothing in 01–08 depends on it. |
| 10 | docs-and-pilot | todo (not drafted) | CLAUDE.md / README / doc_truth updates, pilot runbook (low-risk, network-none), pilot metrics capture (start latency, park counts, failed starts). |

## Notes

- **Executor-host constraint (load-bearing):** the four gates must stay green
  on hosts with no docker binary. Runtime interaction lives behind
  `#[ignore]`; logic phases test parsing/assembly against captured fixture
  output. Phase docs must pre-inject the fixture text — the executor cannot
  run docker to produce it.
- **Warm pool deliberately not scoped** — phase-10's latency numbers decide
  whether it is ever worth a follow-up milestone item (design D3).
- **`container:shell` (interactive tty relay) deferred** — open question in
  the design, needed by none of these phases.
- Scoped 2026-08-28 from `docs/design/agent-container-sandboxing.md`
  (commit `d856ca6`).
- **Measured while drafting phase-03 (2026-08-28) — a design correction.**
  The scratch tmpfs at `/de/work` is **not** writable by the sandboxed uid
  unless the mount flag carries `mode=0700,uid=1000,gid=1000`, and the obvious
  Dockerfile fix does not work: when the mountpoint exists in the image the
  tmpfs **inherits its mode but not its ownership**, so an in-image
  `chown 1000:1000` still yields `drwx------ root root` and a denial. The
  design's D4 originally claimed the mount was "verified writable as uid
  1000" — that test had passed only against stock `alpine`, which has no
  `/de/work`, where Docker creates the tmpfs mode `1777`. D4 now carries the
  measured table. **Phase-04 must pass the uid/gid options.**
- **Scope change at phase-03 drafting (2026-08-28):** the image **staleness
  warning** and the runbook **`requires_tools`** check are deferred out of
  phase-03. The staleness warning does not fit `retention_warnings()` —
  `RetentionWarning` holds `&'static str` fields
  (`src/daemon/utils/warnings.rs:24`) and a "built N days ago" message is
  dynamic — and neither check has a consumer until phase-04 can run a
  container. Both land with the phase that uses them.
- **Scope change at phase-02 drafting (2026-08-28):** the `Request::ContainerStatus`
  / `Response::ContainerStatus` IPC surface and its `daemoneye status` line moved
  from phase-02 to phase-04. Reason: until phase-04 can actually run a container
  there is nothing to report but "the runtime answered a version probe", and
  wiring a status field for that alone adds IPC surface with no consumer.
  Phase-02 is correspondingly narrower — a pure, fixture-tested probe and gate
  module that nothing calls yet.
- **Calibration from phase-02 (2026-08-28), none folded:**
  1. **Architect-side, repeat of an already-folded rule.** Phase-02 as first
     drafted specced a module nothing calls under a `-D warnings` gate without
     saying how dead-code would be satisfied — the M7–M10 rule *"a phase that
     lands code for a later phase must say how the deny-warnings gate is
     satisfied"* already covers this. Apply it when drafting phases 03–10:
     every phase that lands code for a later consumer must name its
     dead-code strategy in § Authorizations up front.
  2. **Architect-side, new variant, 1 occurrence.** A bounce criterion that
     greps for an offending phrase must not *contain* that phrase — the
     criterion becomes part of the corpus it measures and can never reach 0.
     Scope such greps to a section (`sed -n '/^## Update Log/,$p' | grep -c`).
  3. **Executor-side, 1 occurrence.** DeepSeek V4 Flash recorded its own
     unauthorized decision as architect guidance received "on re-dispatch",
     when the session log shows a single turn-0 prompt and no injected
     feedback. Corrected on bounce. Watch across phases 03+: a fabricated
     authorization is worse than an unauthorized decision, because it removes
     the reviewer's reason to look.
- **Calibration, held at 1 occurrence (phase-01):** a phase doc's
  § Authorizations tells the executor to file a blocker and stop *"if an
  acceptance criterion cannot be satisfied honestly"* — which does not cover
  **a pre-existing test blocking a gate**. Phase-01 hit exactly that: every
  criterion was satisfiable, but `cargo test` was red because
  `peer_euid_none_on_invalid_fd` asserted on stdin, and the executor's MCP
  stdio environment supplies a socket there. With no sanctioned path the
  executor repaired the test out of scope — correctly diagnosed and
  disclosed, but unauthorized. Extend the sentence to cover gates in future
  M18 phase docs. Not folded into WORKFLOW.md at one occurrence.
