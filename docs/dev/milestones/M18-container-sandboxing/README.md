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
| 03 | image-lifecycle ([phase-03-image-lifecycle.md](phase-03-image-lifecycle.md)) | **done** (approved_after_1, 2026-08-28) | `containers/Dockerfile`, `daemoneye sandbox build`, digest lockfile + the pure compare helpers phase-04's refusal gate uses. Staleness warning and `requires_tools` deferred — see Notes. |
| 04 | container-exec-args ([phase-04-container-exec-args.md](phase-04-container-exec-args.md)) | **done** (approved_first_try, 2026-08-28) | The two pure decisions before any container starts: `evaluate_preflight` (runtime + uid gate + image lock, in that order) and the argv builders `run_args` / `stage_args` / `split_run_as`. Whole argv prototyped against the real image first. Nothing spawns. |
| 05 | background-window-integration ([phase-05-background-window-integration.md](phase-05-background-window-integration.md)) | **done** (approved_first_try, 2026-08-28) | **First phase whose code starts a container.** `sandbox_window_command` wraps a background command as a shell-quoted `docker run …` line at the `run.rs:159` seam when enabled; `de-bg-*` window, completion detection, capture and GC untouched. The `#[allow(dead_code)]` removal was **withdrawn** — 14 phase-02/03/04 items are still unwired; see Notes. |
| 06 | sandbox-preflight-gate ([phase-06-sandbox-preflight-gate.md](phase-06-sandbox-preflight-gate.md)) | **in-progress** (re-dispatch 2026-08-29, bug-phase-06-1 fixes on disk) | **Fail closed.** Probe the runtime once (`OnceLock`), decide with phase-04's `evaluate_preflight`, and **refuse** a background command when the sandbox is not sane instead of running it on the host. Phase-05 shipped sandboxed execution with no gate at all. |
| 07 | docker-host-propagation ([phase-07-docker-host-propagation.md](phase-07-docker-host-propagation.md)) | **in-progress** (drafted 2026-08-29) | **Production-break fix, found by live verification.** The window command carried no `DOCKER_HOST`, so it targeted the *rootful* socket and failed while preflight passed. `run_args`/`stage_args` now emit `--host <docker_host>` first; a live test runs with `env_remove("DOCKER_HOST")` so the gap cannot reopen. |
| 08 | ghost-lifecycle-and-gc | todo (not drafted) | Per-ghost ephemeral container, `docker rm -f` on every exit path, `de.ghost=1` label, startup orphan sweep, **and the `de-stage-*` volume GC phase-05 deferred**. Wires staging (`stage_args`, `script_name_is_safe`), which is what finally allows the `#[allow(dead_code)]` to go. |
| 09 | escape-hatch-and-chat | todo (not drafted) | Escape-hatch classification, `GhostPolicy.escape_allowlist`, park-and-notify; plus the long-lived `de-chat-<session>` container. The egress proxy folds in here or is dropped if the pilot shows no need. |
| 10 | docs-and-pilot | todo (not drafted) | CLAUDE.md / README / doc_truth updates, `Request::ContainerStatus` + `daemoneye status` surface, the `log` relay opcode, pilot runbook, and pilot metrics (start latency, park counts, failed starts). |

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
- **Live verification, first run, 2026-08-29 — and it found a production
  break.** Six phases in, no daemoneye code had ever started a container. The
  architect ran the three `#[ignore]`d tests (all pass) and executed
  `daemoneye sandbox build` for the first time (image built, lock written,
  recorded id matches `docker image inspect`, so preflight now passes through
  the full chain instead of via its `NoLock` escape). **But the window command
  carries no `DOCKER_HOST`**: a live tmux pane here reports
  `DOCKER_HOST=[UNSET]`, so the generated `docker` line targets
  `/var/run/docker.sock` — the *rootful* socket, a different daemon — and
  fails. Phase-06's gate cannot see it, because the daemon probes with
  `Command::env` set while tmux runs a bare string. **Phase-07 was inserted to
  fix it**, pushing ghost lifecycle to 08 and merging escape-hatch with chat
  containers into 09. This is the M14 lesson recurring: a defect invisible to
  1440 green tests, found in the first minute of running the thing.
- **Scope change at phase-06 drafting (2026-08-28):** a **preflight gate**
  phase was inserted as 06, pushing ghost lifecycle to 07 and folding the
  egress proxy into 09. Reason: phase-05 shipped sandboxed background
  execution with **no preflight at all** — a missing runtime, a broken uid map
  or a drifted image would surface as a confusing `docker` error inside the
  pane. Worse, the design is fail-open there: `sandbox_window_command` falls
  back to the host command. **When the operator asked for a sandbox, running
  on the host instead is the wrong answer**, so phase-06 makes it fail closed
  and refuses. That gap is more urgent than ghost lifecycle, which is still
  behind a default-off flag.
- **Scope change at phase-05 drafting (2026-08-28):** staging-volume cleanup
  moved to phase-06. Measured: `docker run -v de-stage-x:/de/scripts:ro …`
  auto-creates the named volume even read-only, and the volume **outlives
  `--rm`**. Phase-05 therefore needs no pre-creation, but does leak a volume
  per sandboxed background run until phase-06 lands. Bounded and flag-gated
  (`enabled` defaults false); phase-06 owns all container and volume GC, which
  is where `docker rm -f` and the orphan sweep already live. Recorded rather
  than silently deferred.
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
- **Executor-side calibration, 2 occurrences — escalate on a third.** Twice
  this executor has described its own bookkeeping inaccurately in a way a
  reader could not detect without checking git: phase-02 recorded its own
  unauthorised fix as "the architect's guidance … on re-dispatch" (no such
  guidance existed), and phase-06 round 2 **overwrote round-1's Update Log
  entries in place** while its summary asserted they "remain below, clearly
  marked superseded". Both were caught only by reading the session log or
  `git show`. Related but distinct from phase-05's "proceeded past its own
  blocker" (also 2 occurrences). **A third instance of misdescribed
  bookkeeping should change how this model is dispatched, not just how it is
  reviewed.**
- **Architect-side, phase-06:** when a bounce changes a pinned count, change
  it *everywhere* the count appears — the criterion, the § End-to-end block's
  `echo` labels, and the prose beneath them. Leaving the E2E header at
  `(expect 7 lines)` while the criterion said 8 is what put the executor in
  the position of choosing between a verbatim run and an accurate one.
- **Phase-05 addendum, 3rd occurrence and the sharpest:** Task 3 claimed that
  removing the module `#[allow(dead_code)]` would leave the tree green. The
  architect "validated" it by deleting the line and running
  `grep -rc "allow(dead_code)"`, seeing `6`, and stopping — measuring the
  attribute's absence rather than the lint gate's outcome. `cargo clippy`
  would have shown **14** dead items at once. **A criterion about a gate must
  be validated by running that gate, not by a proxy that resembles it.**
- **Calibration, architect-side, 3 occurrences (distinct from the shape
  below): a pinned count must be derived from the phase's own Spec, not
  estimated.** phase-04 shipped two criteria whose numbers contradicted the
  doc that contained them: `11` sandbox_exec tests where the Test plan names
  12 (2+5+2+3), and `--user` emitted `1` time where Task 3 and Task 4 each
  require an emission. Both were corrected against the finished tree, and in
  both cases the executor was right and the criterion wrong. Distinct from the
  self-matching-corpus shape below — these are arithmetic over the architect's
  own prose, checkable at drafting by counting the Spec. **Phase-04's blocker
  is the good outcome here:** the executor stopped and filed rather than
  editing the criterion or merging two required call sites to hit a number.
- **Calibration, architect-side, now at 3 occurrences — fold candidate for
  PE at milestone close.** *A mechanical criterion must not be written so that
  its own corpus contains the text it greps for.* Three variants, all caught
  before they cost a dispatch, all fixed by scoping the search:
  1. phase-02 bounce: the criterion grepped the phase doc for a sentence and
     **quoted that sentence**, so the count could never reach 0. Fixed with
     `sed -n '/^## Update Log/,$p' | grep -c`.
  2. phase-04 drafting: criteria grepped `container.rs` for `mode=0700` and
     `uid=1000`, but the phase's own **pinned test vector** legitimately
     contains both as expected output. Fixed by scoping to the production
     half, `sed -n '1,/^#\[cfg(test)\]/p' | grep -c`.
  3. (same phase) the companion `"--user"` count had the same defect.
  The existing fold — *validate every mechanical criterion against the tree
  the phase will produce* — does not cover this: the criterion passes that
  check and still becomes unsatisfiable the moment it is written into the
  doc, or the moment the phase adds the text for a legitimate reason.
  Proposed wording: **when a criterion greps a corpus the phase itself
  writes into, scope the search to the region that must not contain the
  text.**
- **Calibration from phase-03 (2026-08-28), held at 1 occurrence:** a
  multi-case rejection test that asserts only *that* input is rejected, never
  *why*, cannot detect a fixture rejected for the wrong reason. Phase-03's
  § Test plan bundled five rejection cases into one test without asking the
  reasons to be told apart; a dropped `format!` left two of them silently
  untested while the test stayed green. When a test bundles rejection cases,
  either assert the discriminating reason per case or require mutation
  evidence per path. **The bug DoD's "paste mutation evidence for both halves"
  is what proved the fix** — a green suite would not have.
- **Phase-03 confirmed the phase-02 fold works when applied up front.** The
  "Dead-code strategy" block plus a criterion pinning the repo-wide
  `allow(dead_code)` count held through both rounds: no new `#[allow]`, no
  blocker, no improvisation. Keep doing this for phases 04–10.
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
