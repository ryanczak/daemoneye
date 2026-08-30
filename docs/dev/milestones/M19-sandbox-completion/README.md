# M19 — Sandbox Completion

**Goal:** Finish what M18 started — scripts reach sandboxed commands, ghost
shells actually run in containers, a sandboxed agent gets audited network
egress through a proxy it cannot bypass, an operator can see and steer sandbox
state from the chat surface, and a command that genuinely needs the host has
one explicit, audited way out.

**Status:** in-progress — **opened 2026-08-29 on PE sign-off.**

**Depends on:** M18 — Container-sandboxed Agents (closed 2026-08-29, 10 phases
done).

**Exit criteria:**

- A sandboxed background command can execute a script from
  `~/.daemoneye/scripts/` — `stage_args` has a caller, and
  `src/daemon/executor/mod.rs`'s module-level `#[allow(dead_code)]` is
  **removed** with `cargo clippy --all-targets --all-features -- -D warnings`
  still green (repo-wide count 7 → 6).
- A ghost shell's background command runs in a container carrying
  `de.ghost=1`, and a ghost-scoped teardown reclaims only that ghost's
  containers — verified live, not only in tests. **With the sandbox enabled a
  ghost has no unsandboxed door out**: neither `background=false` nor
  `retry_in_pane` reaches the host uncontained (phase-03).
- `is_ghost` is derived by a **pure, directly tested predicate**: mutating it
  to a constant fails a named test. (Today hardcoding `is_ghost: true` leaves
  all 1454 tests green.)
- `daemoneye status` reports sandbox state — runtime reachable, image id vs
  lockfile, live sandboxed containers — and `Request::ContainerStatus` carries
  it over IPC.
- A profile declaring `network = "proxy"` runs the agent container attached
  **only** to a dedicated user-defined network carrying a proxy container;
  the agent reaches an allowlisted host through it, a non-allowlisted host is
  refused, and every request is recorded in `events.jsonl` **with the rule
  that matched and the decision**. The negative case is load-bearing: with the
  proxy in place the container must still reach **neither** the host loopback
  nor the wider LAN — `--disable-host-loopback` stays on. **A credential the
  proxy profile needs never enters the container**: the agent holds a
  sentinel, the proxy substitutes the real value on the way out, and
  `docker inspect` / the container's environment show only the sentinel
  (phase-08, adopted from Docker Sandboxes 2026-08-30).
- A profile declaring `workspace = "clone"` runs the command over a
  **read-only, uid-1000-owned copy** of the pane's working directory, staged
  by the same root helper that stages scripts; `workspace = "none"` (the
  default) mounts nothing, exactly as today. A sandboxed `cargo test` in the
  user's repo succeeds under `clone` and fails loudly under `none` (phase-12).
- Every sandboxed container run is recorded in `events.jsonl` at spawn —
  job id, session, image id, network mode — so a live check can be anchored
  to a record rather than to a `docker ps` snapshot (phase-11).
- A command the operator explicitly escalates runs on the host with the
  escape recorded in `events.jsonl`; one that is not escalated cannot.
- **Live checks, architect-run at close** (the M14/M18 convention — through
  the user's door, session JSONL as the evidence anchor): the startup sweep
  runs through a real daemon; an AI-driven background command completes in a
  container from a real `daemoneye chat` turn.
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Architecture references

- `docs/design/agent-container-sandboxing.md` — the design of record. § D0 is
  the tool disposition table; § D4 (staging) and § D5 (egress) were both
  **corrected from measurement** during M18 and are the versions to build to.
- `CLAUDE.md` § "Container sandbox" — what M18 actually shipped.
- `docs/dev/milestones/M18-container-sandboxing/README.md` § Retrospective —
  the carries, and the four defects that live measurement found.

## Phases

Proposed decomposition; each drafted on demand via `/rexymcp:architect next`.

| #  | Phase | Status |
|----|-------|--------|
| 01 | is-ghost-predicate ([phase-01-is-ghost-predicate.md](phase-01-is-ghost-predicate.md)) | **done** (approved_first_try, 2026-08-29) |
| 02 | staging-integration ([phase-02-staging-integration.md](phase-02-staging-integration.md)) | **done** (approved_after_1, 2026-08-29) |
| 03 | ghost-container-execution ([phase-03-ghost-container-execution.md](phase-03-ghost-container-execution.md)) | **done** (approved_first_try, 2026-08-29) |
| 04 | ghost-scoped-teardown ([phase-04-ghost-scoped-teardown.md](phase-04-ghost-scoped-teardown.md)) | **done** (approved_after_1, 2026-08-30) |
| 05 | container-status-ipc ([phase-05-container-status-ipc.md](phase-05-container-status-ipc.md)) | **done** (approved_first_try, 2026-08-30) |
| 06 | proxy-network-and-image | todo (not drafted) |
| 07 | proxy-profile-wiring | todo (not drafted) |
| 08 | proxy-allowlist-and-audit | todo (not drafted) |
| 09 | escape-hatch | todo (not drafted) |
| 10 | live-verification-and-close | todo (not drafted) |
| 11 | container-hardening-flags | todo (not drafted; **taken into scope 2026-08-30**) |
| 12 | workspace-mount-policy | todo (not drafted; **added 2026-08-30**) |

**Ordering.** 01 is first and deliberately small: it closes a *known* coverage
gap before 03/04 start depending on the value it produces. 02 is independent
of 01. 03 depends on 01; 04 depends on 03. 05 is independent of everything
else. 06 → 07 → 08 is a hard chain (no wiring without a network; no allowlist
without wiring). 09 is independent but scheduled late deliberately. 10 is the
close-out.

**11 and 12 were added after the original decomposition** (PE decision
2026-08-30, from the Docker Sandboxes comparison in § Notes) and sit last in
the table only because they were added last. Both run **before** 10, which
stays the close-out. 11 is independent of everything. 12 depends on 02's
staging helper (it is the same root-copy mechanism applied to a directory)
and on nothing else; it is deliberately **not** on the proxy chain.

Phase intents:

- **01 is-ghost-predicate** — extract `is_ghost_session()` as a pure function
  with its own tests, and call it from `src/daemon/background/run.rs:187`.
  Small, mechanical, and it converts an untested expression into a mutable
  seam **before** teardown starts trusting it.
- **02 staging-integration** — give `stage_args` a caller so a sandboxed
  command can run a script, then **remove** the module `#[allow(dead_code)]`.
  Removal is the phase's real acceptance test.
- **03 ghost-container-execution** — close the two paths by which a ghost
  reaches the host with no container: `background=false` (foreground execution
  ignores `is_ghost` entirely) and `retry_in_pane` (`respawn.rs` has **zero**
  sandbox code). **Corrected from measurement at drafting, 2026-08-29:** the
  original intent line here said *"today ghosts are labelled, not sandboxed"* —
  false. A ghost's ordinary background command *is* containerized;
  `run_background_in_window` wraps every enabled-sandbox command and phase-01
  already routes the `de.ghost=1` decision through `resolve_is_ghost`. The two
  bypasses above are the actual hole.
- **04 ghost-scoped-teardown** — reclaim one ghost's containers on exit, using
  `de.ghost=1` plus a new `de.session=<session_id>` label, and wire
  `[sandbox.ghost_defaults] destroy_on_exit`. Must not touch another ghost's or
  an interactive session's containers; the negative case is the point.
  **Refined from measurement at drafting (2026-08-29):** the intended selector
  was "a per-job label" — a *session* label is the right grain, because the
  hook fires once when the ghost exits, not once per job. Live-measured on the
  daemon host: `--filter label=k=v` is an **exact** value match (a sibling
  ghost sharing a name prefix does not match), repeated `--filter` clauses are
  ANDed, and `rm -f` reclaims a still-running container.
- **05 container-status-ipc** — `Request::ContainerStatus` +
  `daemoneye status` output. **Measured while drafting (2026-08-30):** the
  obvious listing (`docker ps --format '{{.Labels}}'`) joins label pairs with
  `,` and cannot be split back, so a ghost whose alert name contains a comma
  is silently mis-attributed; `docker inspect --format '… {{json
  .Config.Labels}}'` is unambiguous and keeps a newline-bearing value on one
  line. `docker inspect` with no arguments is a usage error, and the empty
  list is the common case. The report adds `enabled` and `image_detail` to the
  design's named shape, because the milestone's exit criterion asks for the
  lockfile comparison and it has nowhere else to live.
- **06 proxy-network-and-image** — a dedicated user-defined docker network
  and a proxy container image, plus the argv builders that create and tear
  them down. Pure-decision-logic-first, exactly as `container.rs` is built.
  **Contract decided 2026-08-30: egress is HTTP(S) only.** The agent reaches
  the proxy through `HTTP(S)_PROXY` and never resolves a name itself, so
  there is no DNS path out; raw TCP (`ssh`, `git@`), UDP and ICMP are not
  forwarded and the proxy image need not try. Docker Sandboxes forwards raw
  TCP per port rule; we are choosing not to **for M19**, and the design doc
  must say so rather than leave it implied. **PE note 2026-08-30: HTTP-only is
  a deferral, not a decision — SSH and other TCP protocols will be needed
  later.** Two consequences for how 06–08 are built now, so the later
  extension is additive rather than a rewrite: (1) the profile's rule
  syntax must accept `host:port` from the start (08 already does), because a
  TCP rule is a host:port rule; (2) the audit record's `proxy_type` field
  must exist from the start with the single value `forward`, so a later
  `transparent` value is a new enum arm, not a schema change. Nothing else
  about the proxy image should assume HTTP is forever.
- **07 proxy-profile-wiring** — `network = "none" | "proxy"` in the sandbox
  profile; when `proxy`, attach the agent container to the proxy network
  **only** and set `HTTP(S)_PROXY` to the proxy's service name. Credentials
  are **not** passed here — not as `-e`, not as a file. A key in the
  container's environment is visible to `docker inspect` and to every process
  in it; phase-08's sentinel mechanism is the only door.
- **08 proxy-allowlist-and-audit** — per-profile hostname allowlist enforced
  in the proxy, every request logged to `events.jsonl`. A refused host must
  be observably refused, not silently dropped. **Three decisions adopted from
  Docker Sandboxes' policy model (2026-08-30):**
  1. **Rule syntax and precedence.** Exact host, `*.domain` wildcard,
     `host:port` suffix; a `proxy_deny` list beside `proxy_allow`, and **deny
     always beats allow**. Do not invent a fourth form.
  2. **Audit record shape.** Each `events.jsonl` record carries the
     destination host, the **rule that matched** (or `none`), the decision and
     its reason, a `proxy_type` (only `forward` in M19 — see the 06 note on
     raw TCP), and a repeat count for identical consecutive requests —
     the matched rule is what makes a refusal debuggable. Blocked and allowed
     are the same record with a different decision, never two formats.
  3. **Sentinel credential injection.** A profile may declare
     `[sandbox.profile.<name>.credentials]` entries mapping a destination
     domain to a daemon-side secret. The container is given a **sentinel**
     value (`de-cred-<rand>`), never the secret; the proxy rewrites the
     matching header on the way out. The real value appears in no container
     environment, argv, file or `docker inspect`. This is strictly weaker than
     the default profile — which has no credential at all — and strictly
     stronger than an env var, and it is the only way an agent that needs an
     API gets one.
- **09 escape-hatch** — `GhostPolicy.escape_allowlist`, the `escape_hatch`
  flag on `ToolCallPrompt`, park-and-notify, and an `events.jsonl` record.
  The highest-risk phase in the milestone: it is the one feature whose bug
  runs a command on the host.
- **10 live-verification-and-close** — the two unrun M18 live checks plus this
  milestone's own, then the doc sweep and retrospective. **Two measurements
  added 2026-08-30, both deciding questions the design deferred:**
  1. **Cold container start latency** per sandboxed command, through a real
     `daemoneye chat` turn. D3 promised a long-lived `de-chat-<session>`
     container with `docker exec` and said to measure before building it;
     measured 2026-08-30, **it is not built** — `grep -rn "docker exec"
     src/` is empty and every job is `docker run --rm`. The number decides
     whether a per-session container is M19's last phase or a later
     milestone's; it also decides a usability question, since only a
     persistent container lets an agent `pip install` something and then use
     it.
  2. **gVisor under rootless.** `--runtime=runsc` is a one-flag middle ground
     between a shared kernel and Docker Sandboxes' microVM. Spend an hour
     measuring whether it runs under rootless dockerd on the daemon host and
     what it costs; record the numbers; decide nothing else here.

- **11 container-hardening-flags** (in scope 2026-08-30) — four `docker run`
  flags `run_args` does not set today. Prompted by a read of
  `docker/compose-for-agents` (2026-08-30) and then **measured against the real
  `daemoneye-agent-base` image on the daemon host**, because that repo turned
  out to be an orchestration showcase rather than a hardening reference: across
  its 25 compose files, `read_only`/`cap_drop` appear in 3 — all the
  third-party Sock Shop demo app, not one agent service — `security_opt`,
  `no-new-privileges`, `pids_limit`, `ulimits` and `tmpfs` appear **zero**
  times, and two services run `privileged: true`. Nothing there is worth
  copying. The gaps below are ours, found by looking:

  1. **`--memory-swap`.** Docker defaults it to 2× `--memory`, so today's
     `--memory 1g` permits 2 GiB total. Measured:
     `docker run --memory 512m …` → `MemorySwap=1073741824`. Set it equal to
     `limits.memory`.
  2. **`--read-only` plus a `/tmp` tmpfs.** Measured working with the current
     image and `sh -lc`: `/de/work` stays writable, the root filesystem is not
     (`touch /rootfs-probe` → `Read-only file system`).
  3. **`--cap-drop=ALL` and `--security-opt=no-new-privileges`.** Measured
     effective in-kernel, not merely accepted by the CLI:
     `CapBnd: 0000000000000000`, `NoNewPrivs: 1`. The second is the one with
     teeth — the process is already uid 1000, but Alpine ships setuid busybox
     links, and this closes that escalation path.
  4. **`--pull=never`.** Without it docker resolves a missing image against
     `docker.io` (measured: *"failed to resolve reference
     docker.io/library/…"*). `sandbox_preflight` does fail closed on a missing
     image, but its verdict is cached in a `OnceLock` for the daemon's
     lifetime, so an image deleted *after* startup leaves a window where a run
     would reach the registry. This closes it locally at no cost.

  **Cost:** ~40 lines plus tests, all inside `run_args`. It edits
  `sandbox_exec_run_args_match_the_prototyped_vector`'s pinned vector, which is
  the phase's real acceptance test — that expectation is *supposed* to change
  here, unlike in phases 04 and 05 where it was pinned as unchanged.

  **Three more items folded in 2026-08-30**, each a design-doc promise the
  code does not keep (measured: the greps are empty):
  5. **`FROM alpine@sha256:…`** — `containers/Dockerfile` is `FROM alpine:3.22`,
     a tag; the design says the base image is pinned by digest.
  6. **A `container_run` event at spawn** (job id, session, image id,
     network mode). `events.jsonl` today carries only `job_start`,
     `job_complete` and `gc_window` for a background job — nothing records
     that a container ran, or which. This is the audit anchor phase-10's live
     checks need, and it is also exit-criterion material above.
  7. **The >90-day image staleness warning** in `retention_warnings()`, and
     `requires_tools` runbook frontmatter with a fail-fast check — both named
     in the design's image-lifecycle section, neither present.

  **One anti-pattern worth naming, from the same read:** every gateway in that
  repo sets `use_api_socket: true` — the Docker API socket mounted into a
  container, which is root-equivalent on the host. An agent sandbox must never
  grant it, and it is presented there as the ordinary way to do this.

- **12 workspace-mount-policy** (added 2026-08-30) — the biggest usability gap
  the Docker Sandboxes comparison exposed. A sandboxed background command
  today lands in an empty `/de/work` tmpfs with **no host path mounted at
  all**: the AI's `cd ~/src/foo && cargo test` does not fail loudly, it runs in
  a container that has never heard of `~/src/foo`. D4 anticipates a
  "per-profile mount set" and nothing implements it. Adds
  `workspace = "none" | "clone" | "direct"` to the sandbox profile:
  - **`none`** (default) — exactly today. Nothing changes for a profile that
    does not opt in.
  - **`clone`** — the pane's working directory is copied into the job's
    staging volume by the same root helper `stage_args` already uses for
    scripts, `chown 1000:1000`, mounted **read-only** at a fixed path, and the
    command's cwd is set there. The volume is removed with the job like the
    script volume is. Build this first, and pin it with a live measurement:
    a real `cargo test` in a real checkout succeeds under `clone`.
  - **`direct`** — a read-write bind of the host directory. **Do not draft
    this without measuring the uid mapping first.** Under rootless, a host
    directory owned by host uid 1000 appears *root-owned* inside the
    container, so the uid-1000 agent almost certainly cannot write it; that is
    the exact reason `clone` exists and why it comes first. If the measurement
    confirms it, `direct` is out of scope for M19 and this bullet says so.
  Docker Sandboxes ships the equivalent as `direct` (default) and `--clone`,
  and its own security page calls the direct default the critical gap (git
  hooks, Makefiles and CI files are live-editable). We invert the default.
  Depends on phase-02's staging mechanism; independent of the proxy chain.

## Notes

- **PE DECISION 2026-08-29 — the egress proxy IS in scope.** I had scoped it
  out, arguing a containerized proxy is a network-architecture piece rather
  than a completion task; the PE overruled that and it is in, as phases 06–08.
  Recorded because the reasoning that argued against it is still the risk to
  manage: it is the largest unbuilt piece here, so it gets **three** phases
  rather than one, and the chain is ordered so each has a runnable end state.
  Design of record is § D5, whose mechanism was already corrected from
  measurement — a host-bound proxy is unreachable under
  `--disable-host-loopback`, and disabling that flag to reach one would expose
  every host loopback service to every container.

- **Gap found drafting phase-02 (2026-08-29): scheduled script jobs are not
  sandboxed.** `ActionOn::Script` runs the script's host path in a `de-sj-*`
  host shell (`src/daemon/scheduled.rs`) and never enters
  `run_background_in_window`, so neither the sandbox wrap nor the staging
  phase-02 adds reaches it. D0 says *"scheduled commands execute sandboxed
  when they fire"*; today they do not. Not in any phase's scope — the design
  claim needs correcting in the phase-10 doc sweep, or a phase adding.

- **Gap found drafting phase-03 (2026-08-29): `[sandbox.ghost_defaults]` is
  parsed and consulted by nothing.** `grep -rn ghost_defaults src/` returns
  only `src/config/mod.rs` tests. `destroy_on_exit` is phase-04's business;
  **`mount_scripts` has no phase** — staging always mounts `/de/scripts:ro`,
  so a config that sets `mount_scripts = "rw"` is silently ignored. Either
  wire it or delete the field in the phase-10 sweep; a config key that does
  nothing is worse than no key.

- **Docker Sandboxes comparison, 2026-08-30 — PE decision: incorporate.**
  `docs.docker.com/ai/sandboxes/` was read in full (architecture, security,
  policy, credentials, monitoring, MCP gateway, CLI) and each claim checked
  against this repo's code. The two are different shapes: sbx is a **microVM
  per agent** (separate kernel, `sudo` inside, its own dockerd, host workspace
  bind-mounted read-write by default, Docker-Desktop-shaped, needs a
  hypervisor and an interactive user); daemoneye is a **rootless container per
  command** (shared kernel, uid-mapped, non-root, no host path mounted,
  `--network=none`, headless). A microVM is not an option for a daemon on a
  server, and the shared kernel is the weaker boundary against a kernel
  exploit — which is why phase-11's container-level hardening matters more
  here than it would there. Things daemoneye already does **better** than
  sbx's defaults, so nobody regresses them: no network by default (sbx's
  "Balanced" preset ships `*.googleapis.com`); zero host filesystem exposure;
  non-root with a uid-mapping gate; digest lockfile with refuse-on-mismatch;
  one shared image cache where sbx's per-sandbox caches grow disk unbounded.
  What was taken, and where it landed: sentinel credential injection → 08;
  rule syntax, deny-over-allow, audit record shape → 08; HTTP-only egress
  stated as the contract → 06; workspace mount policy → new phase-12; cold
  start latency and gVisor measurements → 10; digest pin, `container_run`
  event, staleness/`requires_tools` → 11. **What was deliberately not taken:**
  `sudo` inside the sandbox, root-run kit installs, and a workspace mounted
  read-write by default. sbx's MCP gateway keeps local stdio servers on the
  host *outside* isolation and says so; daemoneye's broker-native tools have
  the same posture, already documented in `CLAUDE.md` — a parallel, not a
  gap.

- **`container:shell` (interactive tty relay) stays deferred** — still an open
  question in the design doc, and nothing in M18 settled it.

- **Method carried forward from M18, because it worked:** stand the thing up
  and *run* it before writing the phase that specifies it. Four defects that
  every green test missed were found that way, three of them in code that
  passed all four gates. Budget drafting time for a live pass per phase.

- **Executor model:** `deepseek-v4-flash-0731` via rexyMCP (`brain:8888`).
  M18 record: 6 first-try, 4 approved_after_1, none escalated. Two calibration
  items were **held at 2 occurrences each** — proceeding past its own filed
  blocker, and misdescribing its own bookkeeping in a way a reader cannot
  detect without `git show`.
  **The third of the latter arrived at phase-03 (2026-08-29)** — the
  completion summary claimed the M1 mutation failed exactly one test while
  the artifact it had just pasted showed two — so, per the note that
  anticipated it, the change is now on the **dispatch** side:
  **every phase doc dispatched to this model gets a mechanically checkable
  criterion for each claim its summary will make.** A count the reviewer can
  `grep` cannot be misdescribed; prose about a count can, and twice now has.
  Concretely: pin the *set* of expected failing test names, not a count of
  them; and where a summary claim has no grep, do not ask for the claim.
  A second, narrower fold from the same phase: the status flip is the one
  edit this model makes that **no** `grep -c` guards — it mis-anchored and
  ate the `**Milestone:**` line (bug-phase-03-1). Reviewers check the
  phase-doc header (one `**Status:**`, one `**Milestone:**`) as part of the
  DoD walk. Standing hazard from
  M16: this model has acted destructively when a gate kept bouncing, so the
  phase doc's criteria and the gate must agree *before* dispatch.

- **Template drift, checked at milestone start 2026-08-29 — and the recorded
  method is wrong.** The practice on file is to `comm` the `^#{2,3} ` headings
  both ways. Done: 5 local-only headings, 1 upstream-only. But local
  `WORKFLOW.md` is **1802 lines against upstream's 1259** — 543 lines the
  heading diff cannot see, because most folds (including both landed at M18
  close) are *paragraphs inside existing sections*. Probing by content:
  `PIPESTATUS` 2/0, `validate every mechanical criterion` 1/0, and both M18
  folds 1/0. **Heading `comm` badly understates drift; probe by content for
  anything folded into an existing section.**

- **The upstream-only section was approved for pulling, and on inspection
  there is nothing to pull — my claim that we lacked it was wrong.**
  § "The E2E block: runnable, complete, and seeded as a Spec task" was folded
  *upstream from DaemonEye* on 2026-08-09; local `WORKFLOW.md` carries all
  three of its rules in **fuller** form, inside § "End-to-end verification"
  rather than under that heading — the runnable-block rule, the mutation-pairs
  -as-`## Spec`-tasks rule with its `patch` worked example, and the
  seeded-capture-task rule naming `executor/src/agent/tasks.rs`. Local also
  carries three things upstream does not: the `${PIPESTATUS[0]}` requirement,
  the no-unpastable-bytes clause, and the last-entry-anchored PASTE MATCH
  recipe. Pasting the upstream copy would have inserted a weaker duplicate of
  guidance already here.
  **How I got it wrong is the useful part:** I concluded "we do not have it"
  from a *heading* diff, in the same breath as documenting that heading diffs
  cannot see paragraph-level folds. My content probe then compounded it —
  `grep -ci "Prove it applied"` returned 0 because local words that rule as
  "a `grep -c` of the mutated text after each direction". A blind instrument
  reporting clean is § "Run every count criterion"'s own second corollary.
