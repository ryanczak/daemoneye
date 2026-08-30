# Phase 07: Honouring `network = "proxy"` — profile resolution and the per-job egress proxy

**Milestone:** M19 — Sandbox Completion
**Status:** in-progress
**Depends on:** phase-06 (the proxy image, `proxy.lock`, the network sweep)
**Estimated diff:** ~470 lines including tests, across two existing files
**Tags:** language=rust, kind=feature, size=m

## Goal

`[sandbox.profile.<name>]` has parsed a `network` field and a `proxy_allow`
list since M18, and **nothing has ever read them.** `ExecSpec.network` is the
literal `"none"` at both production call sites. This phase makes the profile
real: a job whose profile declares `network = "proxy"` gets a dedicated
`--internal` network carrying its own proxy container, is attached to that
network **only**, and reaches the proxy through `HTTP(S)_PROXY`.

After this phase a `network = "proxy"` job runs with a **deny-all** proxy —
the filter file is mounted empty, which tinyproxy treats as "refuse
everything" (measured). That is the correct fail-closed end state: phase-08
fills the filter from `proxy_allow` and writes the audit records. Every
`network = "none"` job — that is, every job today — is byte-for-byte
unchanged.

**The negative direction is the whole phase.** Egress is a capability being
granted; every path that cannot positively identify a profile asking for it
must resolve to `None`. That is what `resolve_network_mode` is for and what
its mutation pair proves.

## Architecture references

- `docs/design/agent-container-sandboxing.md` § "D5 — Network policy", the
  corrected mechanism: *"the egress proxy is itself a container. For a profile
  declaring `network = "proxy"`, the daemon runs a proxy container on a
  dedicated user-defined network and attaches the agent container to that
  network only. The agent gets `HTTP(S)_PROXY` pointing at the proxy's service
  name."* This phase is that sentence, minus the allowlist.
- Same section, the constraint that outlives this phase: *"`--disable-host-loopback`
  stays on, and the agent container still has no route to the host or the wider
  LAN — the proxy is the only door, and it is audited."*
- `docs/dev/milestones/M19-sandbox-completion/README.md` § Phases, **07
  intent** — *"Credentials are **not** passed here — not as `-e`, not as a
  file."* Nothing in this phase puts a secret in a container.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-30, commit `5cbdaed`). **The
whole change was prototyped end-to-end before this doc was written, built,
linted, tested and mutated, then reverted** — every block in § Spec is that
prototype after `cargo fmt --all`, and every count in § Acceptance criteria
was read off it.

- `cargo test --lib` → **1491 passed; 0 failed; 4 ignored**. All four gates
  green.
- **`SandboxProfile` is parsed, validated, and read by nothing.** The only
  mentions outside `src/config/` are in that module's own tests:
  `grep -rn 'SandboxProfile\|proxy_allow' src/ | grep -v '^src/config/'`
  returns **nothing**.
- `ExecSpec` already carries a `network: &'a str` field, and `run_args`
  already passes it through to `--network`. Both production call sites pass
  the literal `"none"`:
  `grep -c 'network: "none",' src/daemon/background/run.rs` → **1**,
  and the same in `respawn.rs` → **1**. This phase changes the `run.rs` one
  only.
- Every new symbol is absent today. `grep -c` on
  `src/daemon/executor/container.rs` for each of `fn resolve_network_mode(`,
  `fn proxy_network_name(`, `fn proxy_container_name(`,
  `fn proxy_filter_path(`, `fn network_create_args(`, `fn proxy_run_args(`,
  `fn network_connect_args(`, `fn proxy_rm_args(`, `fn proxy_env_args(`,
  `fn start_proxy(`, `fn remove_proxy(`, `fn proxy_step(`, `enum NetworkMode`
  and `PROXY_PORT` → **0**, fourteen for fourteen.
- The profile name a job runs under is reachable at the `run.rs` call site
  without a signature change: `SessionEntry.ghost_config` is an
  `Option<GhostConfig>` and `GhostConfig.agent` is an `Option<String>`. The
  existing `entry_is_ghost` binding a few lines above shows the exact
  `with_sessions(&sessions, |store| store.get(id)...)` shape to copy.
- **Two completion sites already do the shape this phase needs.** Both
  `run.rs` paths reclaim the staging volume inside a `spawn_blocking`
  (`:409` and `:549` before this change). Proxy teardown goes **inside those
  same closures**, not in new ones.
- `network_rm_args` (phase-06) already exists and is reused here for the
  per-job network — this phase adds no second network-removal builder.
- `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'` → **6**.

### Live measurements (architect, rootless Docker on the daemon host)

Every line below was run against real containers on 2026-08-30 and the
resources removed afterwards (`docker ps -a --filter label=de.sandbox=1` →
empty, `docker network ls --filter label=de.sandbox=1` → empty). **Nothing in
this phase spawns docker during tests.**

1. **tinyproxy refuses to start when its `Filter` path does not exist.** This
   is the single fact that shapes `start_proxy`:

   ```
   $ docker run --rm daemoneye-egress-proxy
   filter file: No such file or directory
   ```

   The container is dead on arrival. Phase-06's conf sets
   `Filter "/etc/tinyproxy/filter"` unconditionally, so **a filter file must be
   written and mounted before the proxy starts**, in this phase, even though
   its contents are phase-08's. An **empty** file is valid and means deny-all.

2. **The empty-filter proxy starts and stays up**, with the file bind-mounted
   read-only from the daemon host:

   ```
   $ : > $W/filter
   $ docker network create --internal --label de.sandbox=1 de-probe-net
   $ docker run -d --name de-probe-px --network de-probe-net --label de.sandbox=1 \
       -v $W/filter:/etc/tinyproxy/filter:ro daemoneye-egress-proxy
   NOTICE  Initializing tinyproxy ...
   INFO    Added address [0.0.0.0] to listen addresses.
   $ docker ps --filter name=de-probe-px --format '{{.Names}} {{.Status}}'
   de-probe-px Up 2 seconds
   ```

   A host-file bind mount works here. (D4 replaced the *script* bind-mount
   with per-run staging for a different reason; a single read-only file
   mounted into the proxy is not affected by that finding.)

3. **The end state this phase produces, measured in full.** Proxy on the
   internal network with the bridge egress leg attached; agent on the internal
   network only, with all four `HTTP(S)_PROXY` spellings set:

   ```
   -- proxy reachable:                       PROXY_REACHABLE
   -- http through proxy (deny-all filter):  403
   -- https through proxy (CONNECT):         000   (curl_rc=56)
   -- direct LAN (must be blocked):          LAN_BLOCKED
   -- host loopback via slirp gw:            HOST_BLOCKED
   -- direct public bypassing proxy:         PUBLIC_BLOCKED
   -- does example.com resolve locally?      ** server can't find example.com: SERVFAIL
   ```

   The refusal is a real `403`, not a dropped connection, and the agent has no
   DNS of its own — it cannot resolve a name to bypass the proxy even if it
   tried.

4. **The positive control works too**, confirming the wiring is complete and
   only the filter's contents are missing. With `example.com` written into the
   same filter file and the proxy restarted:

   ```
   allowed http  example.com: 200
   allowed https example.com: 200
   refused http  httpbin.org: 403
   ```

   This is phase-08's behaviour arriving for free once it renders the file —
   nothing further is needed from the wiring.

5. **Teardown order is forced by the runtime**, exactly as phase-06 measured
   for the sweep:

   ```
   $ docker network rm de-probe-net
   Error response from daemon: error while removing network:
   network de-probe-net has active endpoints (name:"de-probe-px" ...)
   ```

   `remove_proxy` therefore removes the container first, then the network.

## Gotchas

1. **Do not add a field to `ExecSpec`.** It is constructed at **26** sites in
   this repo — **24** of them in `container.rs`'s own test module (measured:
   `grep -c 'ExecSpec {'` reads 24 there and **0** in that file's production
   half), plus one each in `run.rs` and `respawn.rs`. The proxy endpoint is
   derived from the fields it already has — `spec.network != "none"` is the condition and
   `spec.job_id` names the proxy — so this phase changes **zero** existing
   tests. If you find yourself editing a test's `ExecSpec { .. }` literal,
   stop: something has gone wrong.

2. **`resolve_network_mode` must fail closed, and the catch-all arm is how.**
   `_ => NetworkMode::None` covers four distinct ways in: no profile name, a
   name with no config entry, a `network` value that is not exactly `"proxy"`,
   and a case-differing value. Do not "simplify" it into an `unwrap_or` that
   defaults the other way, and do not make the lookup case-insensitive.

3. **Write the filter before starting the proxy** (§ Live measurement 1). A
   proxy started without one dies immediately and the job hangs against a
   container that is already gone.

4. **Teardown removes the container before the network** (§ Live measurement
   5). Reversing them leaves the network behind for the phase-06 sweep to
   collect at the next daemon start — not a leak forever, but not this
   phase's job either.

5. **Proxy teardown goes inside the two existing `spawn_blocking` closures**
   beside `remove_stage_volume`, guarded by `proxy_started`. Do not add new
   `spawn_blocking` calls, and do not call `remove_proxy` unconditionally: a
   `--network=none` job has no proxy, and two pointless `docker` spawns per
   background command is a real cost.

6. **`respawn.rs` is out of scope and must not be touched.** Its
   `network: "none"` stays. A retried-in-pane command losing its proxy is
   strictly *more* restrictive, not a bypass — it gets no network at all.
   A criterion pins that file unchanged.

7. **No credential of any kind enters a container in this phase** — not
   `-e`, not a mount, not argv. The only `-e` pairs added are the four
   `HTTP(S)_PROXY` spellings, whose value is a container name and a port.

8. **The four env spellings are not redundant.** `curl` reads lowercase,
   many Rust and Python tools read uppercase; the measurement in § Live 3 set
   all four. Setting two would half-work in a way no unit test would catch.

## Spec

### Task 1 — Pure decision logic and argv builders, in `src/daemon/executor/container.rs`

Insert the following block **directly before** the line

```rust
/// argv listing every container this daemon's sandbox created, running or not.
```

(the doc comment of `sweep_container_list_args`, a unique line in the file).
This is the prototype verbatim, after `cargo fmt --all`:

```rust
/// The port the proxy image's tinyproxy listens on. Baked into
/// `containers/proxy/tinyproxy.conf` as `Port 8888`; the agent reaches it
/// through `HTTP(S)_PROXY`, so the two must agree.
pub const PROXY_PORT: u16 = 8888;

/// Which network a job's container is attached to, resolved from its profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// `--network=none` — the default, and categorically stronger than any
    /// filtering.
    None,
    /// A dedicated `--internal` network carrying an egress proxy container.
    Proxy,
}

/// Resolve a job's network mode from its profile name.
///
/// **Fails closed.** No profile name, a name with no `[sandbox.profile.*]`
/// entry, and any `network` value other than `"proxy"` all resolve to
/// [`NetworkMode::None`]: a job that cannot be matched to a profile must not
/// silently acquire egress.
pub fn resolve_network_mode(cfg: &SandboxConfig, profile: Option<&str>) -> NetworkMode {
    let Some(name) = profile else {
        return NetworkMode::None;
    };
    match cfg.profile.get(name) {
        Some(p) if p.network == "proxy" => NetworkMode::Proxy,
        _ => NetworkMode::None,
    }
}

/// Per-job egress network name: `de-net-<job_id>`.
pub fn proxy_network_name(job_id: &str) -> String {
    format!("de-net-{job_id}")
}

/// Per-job proxy container name: `de-px-<job_id>`. This doubles as the host
/// name the agent reaches it by — docker's embedded DNS answers for
/// containers on the same user-defined network, and for nothing else.
pub fn proxy_container_name(job_id: &str) -> String {
    format!("de-px-{job_id}")
}

/// Daemon-side path of the per-job allowlist mounted into the proxy at
/// `/etc/tinyproxy/filter`. Phase-08 renders its contents; an empty file is
/// deny-all, which is why this phase can mount one safely.
pub fn proxy_filter_path(job_id: &str) -> std::path::PathBuf {
    crate::config::var_run_dir()
        .join("proxy")
        .join(job_id)
        .join("filter")
}

/// argv creating the job's egress network. `--internal` is the isolation
/// mechanism: measured, a container on it reaches the proxy by name and
/// nothing else — not the LAN, not the gateway, not the host loopback.
pub fn network_create_args(cfg: &SandboxConfig, job_id: &str) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "network".to_string(),
        "create".to_string(),
        "--internal".to_string(),
        "--label".to_string(),
        "de.sandbox=1".to_string(),
        proxy_network_name(job_id),
    ]
}

/// argv running the job's proxy container on its network.
///
/// The labels mirror [`run_args`] exactly, because ghost teardown selects on
/// `de.sandbox=1` **and** `de.ghost=1` **and** `de.session=<id>`: a proxy
/// missing any of them survives its own ghost's exit.
pub fn proxy_run_args(
    cfg: &SandboxConfig,
    job_id: &str,
    filter_path: &std::path::Path,
    is_ghost: bool,
    session_id: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "run".to_string(),
        "-d".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        proxy_container_name(job_id),
        "--network".to_string(),
        proxy_network_name(job_id),
        "--user".to_string(),
        cfg.run_as.clone(),
        "-v".to_string(),
        format!("{}:/etc/tinyproxy/filter:ro", filter_path.display()),
    ];
    args.push("--label".to_string());
    args.push("de.sandbox=1".to_string());
    if is_ghost {
        args.push("--label".to_string());
        args.push("de.ghost=1".to_string());
    }
    if let Some(sid) = session_id {
        args.push("--label".to_string());
        args.push(format!("de.session={sid}"));
    }
    args.push(cfg.proxy_image.clone());
    args
}

/// argv giving the proxy its egress leg. `docker run` takes one `--network`,
/// so the second attachment is a separate command after the container exists.
/// Measured: with it the proxy reaches the LAN and the internet, and still
/// reaches neither the host loopback nor `172.17.0.1`.
pub fn network_connect_args(cfg: &SandboxConfig, job_id: &str) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "network".to_string(),
        "connect".to_string(),
        "bridge".to_string(),
        proxy_container_name(job_id),
    ]
}

/// argv removing the job's proxy container. Must precede the network's
/// removal: docker refuses to remove a network with active endpoints.
pub fn proxy_rm_args(cfg: &SandboxConfig, job_id: &str) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "rm".to_string(),
        "-f".to_string(),
        proxy_container_name(job_id),
    ]
}

/// The `-e` pairs pointing an agent container at its job's proxy. All four
/// spellings are set because the tools an agent runs disagree about case.
pub fn proxy_env_args(job_id: &str) -> Vec<String> {
    let url = format!("http://{}:{}", proxy_container_name(job_id), PROXY_PORT);
    let mut args = Vec::new();
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        args.push("-e".to_string());
        args.push(format!("{key}={url}"));
    }
    args
}

```

### Task 2 — The lifecycle, same file, same insertion point

Immediately after the block from Task 1 (still before
`sweep_container_list_args`' doc comment) add the three impure functions.
`proxy_step` is the module's one-spawn-site-per-operation idiom; `start_proxy`
and `remove_proxy` are the only functions in this phase that start a process:

```rust
/// Run one docker subcommand for the proxy lifecycle, mapping a spawn failure
/// or a non-zero exit to an operator-facing message.
fn proxy_step(cfg: &SandboxConfig, what: &str, args: Vec<String>) -> Result<(), String> {
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(args);
    match bounded_output_with(&mut cmd, Duration::from_secs(60)) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "sandbox egress {what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("sandbox egress {what} failed: {e}")),
    }
}

/// Stand up the job's egress network and proxy container.
///
/// Order is forced by the runtime: the network exists before the proxy joins
/// it, and the proxy exists before its egress leg can be attached. The filter
/// is written **before** the proxy starts because tinyproxy refuses to boot
/// when its `Filter` path is absent (measured: `filter file: No such file or
/// directory`, container dead on arrival). An empty file is deny-all.
///
/// Fails closed: on any error the partial state is reclaimed and the caller
/// refuses the command rather than running it unproxied.
pub fn start_proxy(
    cfg: &SandboxConfig,
    job_id: &str,
    is_ghost: bool,
    session_id: Option<&str>,
) -> Result<(), String> {
    let filter = proxy_filter_path(job_id);
    if let Some(parent) = filter.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Err(format!("sandbox egress filter directory failed: {e}"));
    }
    if let Err(e) = std::fs::write(&filter, b"") {
        return Err(format!("sandbox egress filter write failed: {e}"));
    }
    proxy_step(cfg, "network create", network_create_args(cfg, job_id))?;
    let started = proxy_step(
        cfg,
        "proxy run",
        proxy_run_args(cfg, job_id, &filter, is_ghost, session_id),
    )
    .and_then(|()| proxy_step(cfg, "network connect", network_connect_args(cfg, job_id)));
    if let Err(message) = started {
        remove_proxy(cfg, job_id);
        return Err(message);
    }
    Ok(())
}

/// Reclaim the job's proxy container, its network and its filter directory.
/// Best-effort and idempotent — it runs on completion paths, where a failure
/// must not mask the job's own result. Container first: docker refuses to
/// remove a network that still has an active endpoint.
pub fn remove_proxy(cfg: &SandboxConfig, job_id: &str) {
    if let Err(message) = proxy_step(cfg, "proxy remove", proxy_rm_args(cfg, job_id)) {
        log::debug!("{message}");
    }
    if let Err(message) = proxy_step(
        cfg,
        "network remove",
        network_rm_args(cfg, &[proxy_network_name(job_id)]),
    ) {
        log::warn!("{message}");
    }
    if let Some(dir) = proxy_filter_path(job_id).parent()
        && let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("sandbox egress filter cleanup failed for job {job_id}: {e}");
    }
}

```

### Task 3 — `run_args` points a proxied agent at its proxy, same file

In `run_args`, **directly before** the two lines

```rust
    args.push("--workdir".to_string());
    args.push(cfg.workdir.clone());
```

insert:

```rust
    if spec.network != "none" {
        args.extend(proxy_env_args(spec.job_id));
    }
```

That guard is the whole of the change to `run_args`. Nothing else in the
function moves, and `spec.network` continues to reach `--network` exactly as
it does today.

### Task 4 — Resolve the profile in `src/daemon/background/run.rs`

Directly after the existing line

```rust
    let is_ghost = crate::daemon::resolve_is_ghost(session_id.as_deref(), entry_is_ghost);
```

insert:

```rust

    // The sandbox profile this job runs under is the ghost's agent name; an
    // interactive session has none and so resolves to the default profile.
    let profile_name = session_id.as_deref().and_then(|id| {
        with_sessions(&sessions, |store| {
            store
                .get(id)
                .and_then(|e| e.ghost_config.as_ref().and_then(|g| g.agent.clone()))
        })
    });
```

### Task 5 — Stand the proxy up, same file

**Replace** this block — the opening of the `sandboxed_cmd` binding, which
occurs once:

```rust
    let sandboxed_cmd;
    let cmd: &str = {
        if config.sandbox.enabled {
            let spec = crate::daemon::executor::container::ExecSpec {
                job_id: &job_id,
                network: "none",
                is_ghost,
                command: cmd,
            };
```

with:

```rust
    // Stand up this job's egress proxy when its profile asks for one. Same
    // fail-closed shape as staging above: a command whose proxy cannot be
    // started is refused and its window reclaimed, never run unproxied.
    let mut proxy_started = false;
    let network = if config.sandbox.enabled
        && crate::daemon::executor::container::resolve_network_mode(
            &config.sandbox,
            profile_name.as_deref(),
        ) == crate::daemon::executor::container::NetworkMode::Proxy
    {
        let (cfg_p, job_p, sid_p) = (config.sandbox.clone(), job_id.clone(), session_id.clone());
        let started = tokio::task::spawn_blocking(move || {
            crate::daemon::executor::container::start_proxy(
                &cfg_p,
                &job_p,
                is_ghost,
                sid_p.as_deref(),
            )
        })
        .await
        .unwrap_or_else(|e| Err(format!("sandbox egress task failed: {e}")));
        if let Err(message) = started {
            log::warn!("refusing sandboxed background command: {message}");
            let (s6, wn6) = (session.to_string(), win_name.clone());
            let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s6, &wn6))
                .await;
            return message;
        }
        proxy_started = true;
        crate::daemon::executor::container::proxy_network_name(&job_id)
    } else {
        "none".to_string()
    };

    let sandboxed_cmd;
    let cmd: &str = {
        if config.sandbox.enabled {
            let spec = crate::daemon::executor::container::ExecSpec {
                job_id: &job_id,
                network: &network,
                is_ghost,
                command: cmd,
            };
```

The refusal arm is deliberately the same shape as the staging refusal ~40
lines above it in the same function — read that one first; this is its twin.

### Task 6 — Reclaim the proxy at both completion sites, same file

Two edits, each inside an **existing** `spawn_blocking` closure. Change

```rust
                    crate::daemon::executor::container::remove_stage_volume(&cfg_v, &job_v)
                });
```

to

```rust
                    crate::daemon::executor::container::remove_stage_volume(&cfg_v, &job_v);
                    if proxy_started {
                        crate::daemon::executor::container::remove_proxy(&cfg_v, &job_v);
                    }
                });
```

and change

```rust
                        crate::daemon::executor::container::remove_stage_volume(
                            &sandbox_bg,
                            &job_id_bg,
                        )
                    });
```

to

```rust
                        crate::daemon::executor::container::remove_stage_volume(
                            &sandbox_bg,
                            &job_id_bg,
                        );
                        if proxy_started {
                            crate::daemon::executor::container::remove_proxy(
                                &sandbox_bg,
                                &job_id_bg,
                            );
                        }
                    });
```

Note the `remove_stage_volume(...)` call gains a trailing `;` in both — it was
the closure's tail expression and is now a statement.

### Task 7 — Tests, appended to `container.rs`'s existing `mod tests`

Eight tests plus one helper, appended at the end of the module. Every test
name begins `sandbox_egress_`. This is the prototype verbatim after
`cargo fmt --all`:

```rust
    fn cfg_with_profile(network: &str) -> SandboxConfig {
        let mut cfg = SandboxConfig::default();
        cfg.profile.insert(
            "researcher".to_string(),
            crate::config::SandboxProfile {
                network: network.to_string(),
                proxy_allow: vec!["example.com".to_string()],
            },
        );
        cfg
    }

    #[test]
    fn sandbox_egress_mode_is_proxy_only_for_a_profile_that_asks_for_it() {
        let cfg = cfg_with_profile("proxy");
        assert_eq!(
            resolve_network_mode(&cfg, Some("researcher")),
            NetworkMode::Proxy
        );
    }

    #[test]
    fn sandbox_egress_mode_fails_closed_for_every_other_input() {
        // The negative cases are the point: each of these is a way a job could
        // silently acquire egress it was never granted.
        let cfg = cfg_with_profile("proxy");
        assert_eq!(resolve_network_mode(&cfg, None), NetworkMode::None);
        assert_eq!(
            resolve_network_mode(&cfg, Some("analyst")),
            NetworkMode::None,
            "a profile name with no config entry must not inherit another's network"
        );
        assert_eq!(resolve_network_mode(&cfg, Some("")), NetworkMode::None);
        assert_eq!(
            resolve_network_mode(&cfg, Some("RESEARCHER")),
            NetworkMode::None,
            "profile lookup is exact, not case-insensitive"
        );
        for network in ["none", "Proxy", "proxy ", "bridge", ""] {
            assert_eq!(
                resolve_network_mode(&cfg_with_profile(network), Some("researcher")),
                NetworkMode::None,
                "network = {network:?} must not enable egress"
            );
        }
    }

    #[test]
    fn sandbox_egress_names_are_distinct_and_job_scoped() {
        assert_eq!(proxy_network_name("42-1712937600"), "de-net-42-1712937600");
        assert_eq!(proxy_container_name("42-1712937600"), "de-px-42-1712937600");
        assert_ne!(proxy_network_name("7-1"), proxy_container_name("7-1"));
        assert_ne!(proxy_network_name("7-1"), proxy_network_name("7-2"));
        assert!(
            proxy_filter_path("7-1").ends_with("proxy/7-1/filter"),
            "{:?}",
            proxy_filter_path("7-1")
        );
        assert_ne!(proxy_filter_path("7-1"), proxy_filter_path("7-2"));
    }

    #[test]
    fn sandbox_egress_network_is_created_internal_and_labelled() {
        let cfg = SandboxConfig::default();
        let args = network_create_args(&cfg, "7-1");
        assert!(
            args.iter().any(|a| a == "--internal"),
            "without --internal the agent reaches the LAN directly: {args:?}"
        );
        assert!(args.iter().any(|a| a == "de.sandbox=1"), "{args:?}");
        assert_eq!(
            args.last().map(String::as_str),
            Some("de-net-7-1"),
            "{args:?}"
        );
    }

    #[test]
    fn sandbox_egress_proxy_labels_mirror_the_agent_containers() {
        let cfg = SandboxConfig::default();
        let filter = std::path::Path::new("/tmp/de/filter");
        let ghost = proxy_run_args(&cfg, "7-1", filter, true, Some("ghost-abc"));
        // Ghost teardown ANDs all three; a proxy missing any one outlives its
        // own ghost.
        assert!(ghost.iter().any(|a| a == "de.sandbox=1"), "{ghost:?}");
        assert!(ghost.iter().any(|a| a == "de.ghost=1"), "{ghost:?}");
        assert!(
            ghost.iter().any(|a| a == "de.session=ghost-abc"),
            "{ghost:?}"
        );
        assert!(
            ghost
                .iter()
                .any(|a| a == "/tmp/de/filter:/etc/tinyproxy/filter:ro"),
            "the filter must be mounted read-only or the proxy will not start: {ghost:?}"
        );
        assert_eq!(
            ghost.last().map(String::as_str),
            Some("daemoneye-egress-proxy")
        );

        let interactive = proxy_run_args(&cfg, "7-1", filter, false, None);
        assert!(
            !interactive.iter().any(|a| a == "de.ghost=1"),
            "an interactive job's proxy must not be reclaimable by a ghost teardown: {interactive:?}"
        );
        assert!(
            !interactive.iter().any(|a| a.starts_with("de.session=")),
            "{interactive:?}"
        );
    }

    #[test]
    fn sandbox_egress_leg_and_teardown_target_the_proxy_container() {
        let cfg = SandboxConfig::default();
        let connect = network_connect_args(&cfg, "7-1");
        assert!(connect.iter().any(|a| a == "connect"), "{connect:?}");
        assert!(
            connect.iter().any(|a| a == "bridge"),
            "the egress leg is the bridge network: {connect:?}"
        );
        assert_eq!(connect.last().map(String::as_str), Some("de-px-7-1"));

        let rm = proxy_rm_args(&cfg, "7-1");
        assert!(rm.iter().any(|a| a == "-f"), "{rm:?}");
        assert_eq!(rm.last().map(String::as_str), Some("de-px-7-1"));
    }

    #[test]
    fn sandbox_egress_env_names_the_proxy_in_all_four_spellings() {
        let args = proxy_env_args("7-1");
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            assert!(
                args.iter()
                    .any(|a| a == &format!("{key}=http://de-px-7-1:8888")),
                "{key} missing or wrong: {args:?}"
            );
        }
        assert_eq!(args.iter().filter(|a| *a == "-e").count(), 4, "{args:?}");
    }

    #[test]
    fn sandbox_egress_env_reaches_the_agent_only_on_a_proxy_network() {
        let cfg = SandboxConfig::default();
        let proxied = run_args(
            &cfg,
            &ExecSpec {
                job_id: "7-1",
                network: "de-net-7-1",
                is_ghost: false,
                command: "true",
            },
            None,
        );
        assert!(
            proxied
                .iter()
                .any(|a| a == "HTTP_PROXY=http://de-px-7-1:8888"),
            "{proxied:?}"
        );
        assert!(
            proxied
                .windows(2)
                .any(|w| w[0] == "--network" && w[1] == "de-net-7-1"),
            "{proxied:?}"
        );

        let isolated = run_args(
            &cfg,
            &ExecSpec {
                job_id: "7-1",
                network: "none",
                is_ghost: false,
                command: "true",
            },
            None,
        );
        assert!(
            !isolated.iter().any(|a| a.starts_with("HTTP_PROXY=")),
            "a --network=none job has no proxy to point at: {isolated:?}"
        );
        assert!(!isolated.iter().any(|a| a == "-e"), "{isolated:?}");
    }
```

### Task 8 — Mutation pair M1: `--internal` is what isolates the agent

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them.** Append each marker and run to `/tmp/e2e-07.txt`. Run the gates
(§ End-to-end verification) only **after** all three pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`:
     ```
             "create".to_string(),
             "--internal".to_string(),
             "--label".to_string(),
     ```
   - `new_str`:
     ```
             "create".to_string(),
             "--label".to_string(),
     ```

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-07.txt
   cargo test --lib sandbox_egress 2>&1 | grep -E "FAILED|^test result:" | sed 's/; finished in .*//' >> /tmp/e2e-07.txt
   grep -c '"--internal".to_string(),' src/daemon/executor/container.rs >> /tmp/e2e-07.txt
   ```
   Measured on the prototype: **exactly 1 failed**, naming
   `sandbox_egress_network_is_created_internal_and_labelled`, and the `grep -c`
   prints `0`. A green suite here would mean the agent container is on a
   routable network with the LAN reachable — record a blocker.

2. **Restore.** The inverse `patch`, then the same three lines with the marker
   `== M1 RESTORED ==`. The tests pass (`8 passed`) and the `grep -c` prints
   `1`.

### Task 9 — Mutation pair M2: the profile resolution fails closed

Only after M1 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `        _ => NetworkMode::None,`
   - `new_str`: `        _ => NetworkMode::Proxy,`

   Then, with the marker `== M2 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -c '_ => NetworkMode::None,' src/daemon/executor/container.rs >> /tmp/e2e-07.txt
   ```
   Measured: **exactly 1 failed**, naming
   `sandbox_egress_mode_fails_closed_for_every_other_input`, and the `grep -c`
   prints `0`.

2. **Restore.** The inverse `patch`, marker `== M2 RESTORED ==`, same two
   commands. `8 passed` and the `grep -c` prints `1`.

### Task 10 — Mutation pair M3: the env guard keeps `none` jobs clean

Only after M2 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`:
     ```
         if spec.network != "none" {
             args.extend(proxy_env_args(spec.job_id));
         }
     ```
   - `new_str`:
     ```
         args.extend(proxy_env_args(spec.job_id));
     ```

   Then, with the marker `== M3 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -c 'if spec.network != "none" {' src/daemon/executor/container.rs >> /tmp/e2e-07.txt
   ```
   Measured: **exactly 1 failed**, naming
   `sandbox_egress_env_reaches_the_agent_only_on_a_proxy_network`, and the
   `grep -c` prints `0`.

2. **Restore.** The inverse `patch`, marker `== M3 RESTORED ==`, same two
   commands. `8 passed` and the `grep -c` prints `1`.

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the wrong line fails silently, and a mutation that never
applied certifies a vacuous guard. **All three failure counts above were
measured, not estimated.** If a mutation fails a different number of tests
than stated, do not adjust a test to match — record a blocker naming the
criterion.

### Task 11 — Capture the end-to-end evidence

**The § End-to-end block appends (`>> /tmp/e2e-07.txt`). If you need to run it
a second time — for any reason — `rm -f /tmp/e2e-07.txt` first and run the
whole sequence again from Task 8.** Two executions otherwise leave two copies
in the file, the paste holds one, and the self-check prints `PASTE MISMATCH`.
**Never edit `/tmp/e2e-07.txt` or the pasted block to reconcile them** — the
`PASTE MATCH` check is worth nothing if either side can be adjusted until they
agree, and what an edit removes is usually the failing line that mattered. Run
`cargo fmt --all` **before** the block so `fmt_exit` is a real `0`, not one
produced by a later fix.

Every `test result:` line in this phase's evidence is piped through
`sed 's/; finished in .*//'`, in the mutation tasks and in the block below
alike. Phase-06's run hit a `PASTE MISMATCH` caused **only** by per-run test
durations differing between two identical executions; stripping the timing
suffix removes that failure mode. Do not add the suffix back, and do not
strip anything else.

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-07.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. **The entry ends with the
self-check's verdict line, `PASTE MATCH`, bare on its own line after the
fenced block** — a tick in your final summary is not that line.

## Acceptance criteria

**Every count below was read off the architect's prototype of this exact
change, not derived from the spec text.**

- [ ] Each of `fn resolve_network_mode(`, `fn proxy_network_name(`,
      `fn proxy_container_name(`, `fn proxy_filter_path(`,
      `fn network_create_args(`, `fn proxy_run_args(`,
      `fn network_connect_args(`, `fn proxy_rm_args(`, `fn proxy_env_args(`,
      `fn start_proxy(`, `fn remove_proxy(`, `fn proxy_step(` and
      `enum NetworkMode` appears exactly **1** time in
      `src/daemon/executor/container.rs` (**before: 0** for all thirteen), and
      `grep -c 'PROXY_PORT' …` prints **2** (**before: 0**) — the definition
      and its one use.
- [ ] `grep -c '"--internal".to_string(),' src/daemon/executor/container.rs`
      prints `1`, `grep -c '_ => NetworkMode::None,' …` prints `1`, and
      `grep -c 'if spec.network != "none" {' …` prints `1` (**before: 0, 0,
      0**).
- [ ] In `src/daemon/background/run.rs`: `grep -c 'network: "none",'` prints
      **0** (**before: 1**) and `grep -c 'network: &network,'` prints `1`;
      `grep -c 'profile_name'` prints `2`, `grep -c 'proxy_started'` prints
      `4`, `grep -c 'remove_proxy'` prints `2`, `grep -c 'start_proxy'` prints
      `1` and `grep -c 'resolve_network_mode'` prints `1` (**before: 0** for
      all six).
- [ ] `grep -c 'network: "none",' src/daemon/background/respawn.rs` prints
      `1` (**unchanged**) — § Gotchas 6, that file is not touched.
- [ ] `cargo test --lib sandbox_egress 2>&1 | grep -c "^test .* ok$"` prints
      `8`. A count, not an exit status.
- [ ] `cargo test --lib` reports **1499** passing and `0 failed`
      (**before: 1491**), with `4 ignored` unchanged; and **`cargo test`
      (all targets)** is green.
- [ ] No existing test was edited. `git diff --name-only | wc -l` prints `2`
      — exactly `src/daemon/executor/container.rs` and
      `src/daemon/background/run.rs`. In `container.rs`,
      `grep -c 'ExecSpec {'` prints **26** (**before: 24**) and
      `grep -c 'network: "none",'` prints **25** (**before: 24**): the new
      test in Task 7 adds one `ExecSpec` with `network: "none"` and one with
      `network: "de-net-7-1"`, and **nothing else in that module moves**
      (§ Gotchas 1). Any other value on either count means an existing
      literal was rewritten — record a blocker rather than adjusting a test.
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**unchanged**) — every new function has a caller, so none
      needs one.
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0`, and the same on `src/daemon/background/run.rs` prints `0`
      (**before: 0, 0**).
- [ ] The § End-to-end entry shows `== M1 APPLIED ==`, `== M2 APPLIED ==` and
      `== M3 APPLIED ==` each failing **exactly one** named test — the three
      names in Tasks 8, 9 and 10 — all three `RESTORED` runs passing, with a
      `grep -c` line after each direction reading the value that task states.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks):
      `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-07-proxy-profile-wiring.md`
      prints `1`.

## Test plan

Eight unit tests in `container.rs`'s `mod tests`, given in full in Task 7. No
new test file; **no existing test changes** — the phase is designed around
that constraint (§ Gotchas 1).

**The negative cases are the phase.** `sandbox_egress_mode_fails_closed_for_every_other_input`
walks nine distinct ways a job could reach `NetworkMode::Proxy` without a
profile that asks for it — absent name, unknown name, empty name, wrong case,
and five `network` values including `"Proxy"` and `"proxy "` — because each
one is a silent grant of egress; M2 proves the arm is live.
`sandbox_egress_network_is_created_internal_and_labelled` pins `--internal`,
the single token standing between the agent and the LAN (§ Live measurement
3), and M1 proves it. `sandbox_egress_env_reaches_the_agent_only_on_a_proxy_network`
pins both directions of the `run_args` guard — the proxied job gets the
endpoint, the `none` job gets **no `-e` at all** — and M3 proves it.
`sandbox_egress_proxy_labels_mirror_the_agent_containers` exists because a
proxy that omits `de.ghost=1` or `de.session=<id>` outlives its own ghost's
teardown, which no other test in the repo would notice.

`start_proxy`, `remove_proxy` and `proxy_step` spawn `docker` and are **not**
unit-tested, matching how `stage_script`, `teardown_ghost_containers` and the
sweep are treated in the same module; their argv is tested through the pure
builders they call, and their live behaviour is § Live measurements 1–5,
re-verified at milestone close. Behaviour is unchanged with the sandbox
disabled and unchanged for every `network = "none"` profile, which is every
profile that exists today. **If an existing test requires a change to pass,
stop and record a blocker.**

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 8, 9 and 10 have
appended their mutation markers to `/tmp/e2e-07.txt` and all three pairs are
restored.

```sh
{
echo "== A. named tests (expect 8 ok) =="
cargo test --lib sandbox_egress 2>&1 | grep -E "^test |^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. full suite, all targets =="
cargo test 2>&1 | grep -E "^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== D. structural greps =="
C=src/daemon/executor/container.rs
R=src/daemon/background/run.rs
for f in resolve_network_mode proxy_network_name proxy_container_name proxy_filter_path \
         network_create_args proxy_run_args network_connect_args proxy_rm_args \
         proxy_env_args start_proxy remove_proxy proxy_step; do
  echo -n "fn $f (1): "; grep -c "fn $f(" "$C"
done
echo -n "enum NetworkMode (1):           "; grep -c 'enum NetworkMode' "$C"
echo -n "PROXY_PORT (2):                 "; grep -c 'PROXY_PORT' "$C"
echo -n "--internal (1):                 "; grep -c '"--internal".to_string(),' "$C"
echo -n "fail-closed arm (1):            "; grep -c '_ => NetworkMode::None,' "$C"
echo -n "env guard (1):                  "; grep -c 'if spec.network != "none" {' "$C"
echo -n "ExecSpec sites in container (26):"; grep -c 'ExecSpec {' "$C"
echo -n "ExecSpec none literals (25):    "; grep -c 'network: "none",' "$C"
echo -n "run.rs none literals (0):       "; grep -c 'network: "none",' "$R"
echo -n "run.rs &network (1):            "; grep -c 'network: &network,' "$R"
echo -n "run.rs profile_name (2):        "; grep -c 'profile_name' "$R"
echo -n "run.rs proxy_started (4):       "; grep -c 'proxy_started' "$R"
echo -n "run.rs remove_proxy (2):        "; grep -c 'remove_proxy' "$R"
echo -n "run.rs start_proxy (1):         "; grep -c 'start_proxy' "$R"
echo -n "run.rs resolve_network_mode (1):"; grep -c 'resolve_network_mode' "$R"
echo -n "respawn untouched (1):          "; grep -c 'network: "none",' src/daemon/background/respawn.rs
echo -n "allow total (6):                "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "prod unwrap container.rs (0):   "; sed -n '1,/^#\[cfg(test)\]/p' "$C" | grep -c '\.unwrap()\|\.expect('
echo -n "prod unwrap run.rs (0):         "; sed -n '1,/^#\[cfg(test)\]/p' "$R" | grep -c '\.unwrap()\|\.expect('
echo -n "files changed (2):              "; git diff --name-only | wc -l
} >> /tmp/e2e-07.txt 2>&1
cat /tmp/e2e-07.txt
```

Paste the whole of `/tmp/e2e-07.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-07-proxy-profile-wiring.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07.txt
diff /tmp/pasted-07.txt /tmp/e2e-07.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit `src/daemon/executor/container.rs` and `src/daemon/background/run.rs`
  only. **No other source file, and no doc other than this phase doc's Update
  Log.**
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- No `#[allow(...)]` may be added or removed, and no `#[ignore]` may be added
  or removed.
- **Do not change any existing test's assertions**, and do not add a field to
  `ExecSpec` (§ Gotchas 1).
- **Do not run `docker`, `podman`, or any container command** — including
  `daemoneye sandbox build` — and do not start, stop or query a system
  service. Every container behaviour this phase depends on was measured by the
  architect (§ Live measurements) and is re-verified at milestone close.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it would discard this round's own uncommitted work.
- **Append to the Update Log; never edit or delete an existing entry.** When
  flipping this doc's `Status:` line, change **only** that line — the line
  above it is `**Milestone:** M19 — Sandbox Completion` and must survive (a
  mis-anchored status patch ate it in phase-03; see `bugs/bug-phase-03-1.md`).
  After the flip, `grep -c '^\*\*Status:\*\*' <this doc>` must print `1` and
  `grep -c '^\*\*Milestone:\*\*' <this doc>` must print `1`.
- **Never edit `/tmp/e2e-07.txt` or the pasted evidence block after capture,
  for any reason** (Task 11). On a `PASTE MISMATCH`, delete the artifact and
  re-run the sequence; if a mismatch survives a clean re-run, record a
  blocker. This is `bugs/bug-phase-04-1.md`.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable, a
  mutation leaves the suite green or fails a different number of tests than the
  spec states, *or* a gate is red for a reason this phase did not cause —
  record a blocker Update Log entry naming the exact criterion, and stop.
  Reporting the blocker *is* the successful outcome.** Do not proceed past a
  blocker you have filed.
- **Record what you decide, not what you wish had been decided.** Every claim
  in your completion summary must be one the reviewer can re-run as a command
  from this doc. Do not assert a count you have not just read, and do not
  describe the end-to-end artifact — paste it and let it speak. **If a
  command's output surprises you, re-run it before concluding your tools are
  broken**: phase-06's summary reported a working `grep` as flaky and
  substituted its own instrument for a pinned criterion, which the review
  disproved in one command.

## Out of scope

- **Rendering `proxy_allow` into the filter file, and the audit records** —
  phase-08. This phase mounts the file empty, which is deny-all (§ Live
  measurement 1), and § Live measurement 4 shows phase-08's positive case
  already works once the file has contents.
- **Sentinel credential injection** — phase-08. Nothing here puts a secret in
  a container (§ Gotchas 7).
- **Proxy-image preflight** — refusing a `network = "proxy"` command when
  `proxy.lock` is absent or mismatched. It belongs with the allowlist in
  phase-08: today a missing proxy image surfaces as a `start_proxy` failure
  that refuses the command, which is already fail-closed, just with a less
  precise message.
- **`respawn.rs` / `retry_in_pane`** — § Gotchas 6. A retried command runs
  with no network rather than a proxied one; strictly more restrictive.
- **Foreground and remote (`target_pane`) execution** — unsandboxed today by
  design, unchanged here.
- **A long-lived shared proxy instead of one per job** — § Live measurement 6
  of phase-06 recorded the ~250 ms cold cost; per-job is chosen here for its
  teardown story and the decision is not revisited in M19.
- **Hardening flags on the proxy container** (`--read-only`, `--cap-drop`) —
  phase-11, which owns those for every sandbox container.
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-08-30 22:40 (progress)

Started phase-07: flipping status to in-progress, then implementing Tasks 1-7 (the pure builders, lifecycle, run_args guard, run.rs resolution + proxy startup/teardown) before running the M1-M3 mutation pairs and the E2E block.
