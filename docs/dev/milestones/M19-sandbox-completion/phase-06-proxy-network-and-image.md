# Phase 06: The egress proxy image, its lock, and the network sweep

**Milestone:** M19 — Sandbox Completion
**Status:** todo
**Depends on:** none (independent of 01–05; first of the 06 → 07 → 08 chain)
**Estimated diff:** ~260 lines including tests, plus two new files under `containers/proxy/`
**Tags:** language=rust, kind=feature, size=m

## Goal

A profile that declares `network = "proxy"` needs three things that do not
exist: a proxy **image**, a way to **build and pin** it like the agent image,
and a way to **reclaim** the per-job networks it will run on. This phase
builds exactly those three, as a runnable end state on its own: after it,
`daemoneye sandbox build` produces two locked images, and a daemon start
sweeps leaked egress networks along with leaked containers and volumes.

**What it deliberately does not do:** it adds no `network create`, `proxy run`
or `network connect` argv builders, even though the prototype measured all
three. With no caller until phase-07 they would be dead code under
`-D warnings`, and this milestone already retired one `#[allow(dead_code)]`
that exact mistake had created. Phase-07 adds the builders beside the code
that calls them.

## Architecture references

- `docs/design/agent-container-sandboxing.md` § "D5 — Network policy", the
  corrected mechanism: *"the egress proxy is itself a container. For a
  profile declaring `network = "proxy"`, the daemon runs a proxy container on
  a dedicated user-defined network and attaches the agent container to that
  network only."* This phase is the image half of that sentence.
- `docs/design/agent-container-sandboxing.md` § "Image lifecycle (supply
  chain)" — one locally built image, digest recorded in a lock, refuse on
  mismatch. The proxy image gets the same treatment in its own lock.
- `docs/dev/milestones/M19-sandbox-completion/README.md` § Phases, **06
  intent** — *"egress is HTTP(S) only"* for M19, a deferral not a decision;
  the proxy image must not assume HTTP is forever, and `Upstream` is never
  set.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-30, commit `c6bc50e`). **The
whole change was prototyped end-to-end before this doc was written** — every
count in § Acceptance criteria was read off that prototype.

- `cargo test --lib` → **1485 passed; 0 failed; 4 ignored**. All four gates
  green.
- `ls containers/` → `Dockerfile` only. `containers/proxy/` does not exist.
- `grep -c 'proxy_image' src/config/types.rs` → **0**;
  `grep -c 'proxy_image' assets/etc/config.toml` → **0**.
  `tests/doc_truth.rs::seeded_config_template_documents_every_config_field`
  fails the build the moment a `SandboxConfig` field exists that the seeded
  `assets/etc/config.toml` does not show — **measured on the prototype**, the
  panic names `[sandbox] proxy_image`. Task 2 adds both together.
- `grep -c 'fn sweep_network_list_args(' src/daemon/executor/container.rs` →
  **0**; `grep -c 'fn network_rm_args(' …` → **0**;
  `grep -c 'fn proxy_lock_path(' …` → **0**.
- `grep -c '"label=de.sandbox=1".to_string(),' src/daemon/executor/container.rs`
  → **2** (the container-list sweep filter and the ghost-teardown filter).
- The lock functions are hard-wired to one path (`container.rs:230-252`):

  ```rust
  pub fn lock_path() -> std::path::PathBuf {
      crate::config::etc_dir().join("sandbox.lock")
  }

  pub fn read_lock() -> Option<SandboxLock> {
      let text = std::fs::read_to_string(lock_path()).ok()?;
      parse_lock(&text)
  }

  pub fn write_lock(lock: &SandboxLock) -> std::io::Result<()> {
      use std::io::Write;
      let path = lock_path();
      if let Some(parent) = path.parent() {
          std::fs::create_dir_all(parent)?;
      }
      let mut f = std::fs::File::create(&path)?;
      f.write_all(render_lock(lock).as_bytes())?;
      f.flush()
  }
  ```

  `parse_lock` (`:196`) returns `None` for *"an unknown key set"*, so the
  proxy id **cannot** be a new key in `sandbox.lock` — every existing lock
  would stop parsing. It is a second file, `proxy.lock`, read and written
  through path-taking variants the old names delegate to.
- The sweep (`sweep_sandbox_leftovers`, `container.rs:~800`) removes
  containers then volumes and never mentions networks:
  `grep -c 'removed_networks' src/daemon/executor/container.rs` → **0**.
  The volume section it will be inserted before begins at the unique line
  `    cmd.args(sweep_volume_list_args(cfg));` and the closing log line is:

  ```rust
  log::info!(
      "sandbox sweep removed {} orphaned container(s) and {} stale staging volume(s)",
      removed_containers,
      removed_volumes
  );
  ```

- `src/cli/commands/sandbox.rs` (`run_sandbox_build`, `:21-88`) builds one
  image inline — spawn, check status, validate id, compare to the previous
  lock, write, print. Its imports are exactly:

  ```rust
  use crate::daemon::executor::container::{
      SandboxLock, is_valid_image_id, lock_path, read_lock, write_lock,
  };
  ```

  `grep -c 'build_image(' src/cli/commands/sandbox.rs` → **0**.
- The seeded config's sandbox image line is `assets/etc/config.toml:246`:
  `# image       = "daemoneye-agent-base"   # tag pinned by digest in sandbox.lock`.
- `SandboxConfig` has a **manual** `impl Default` (`src/config/types.rs:~560`)
  that names every field — a new field must be added there too or the build
  fails with `missing field`. Measured.

### Live measurements (architect, rootless Docker on the daemon host)

Everything below was run against real containers on 2026-08-30 and removed
afterwards. It is the evidence for the design; nothing in this phase spawns
docker except `sandbox build`.

1. **An `--internal` user-defined network is the isolation mechanism.** An
   agent container attached to it reaches the proxy container **by service
   name** and nothing else:

   ```
   $ docker network create --internal --label de.sandbox=1 de-egress-probe
   $ docker run --rm --network de-egress-probe daemoneye-agent-base sh -lc '…'
   PROXY_REACHABLE                    # nc -z de-px-probe 8888
   192.168.50.90:8888 BLOCKED         # the LAN AI backend
   172.18.0.1:8888 BLOCKED            # the network's own gateway
   1.1.1.1:443 BLOCKED
   172.17.0.1:9393 BLOCKED            # the host's docker0 side
   ICMP_LAN_BLOCKED
   ```

   Name resolution inside the agent goes to docker's embedded DNS
   (`127.0.0.11`), which answers for **containers on the network only** —
   `example.com` does not resolve there. With `HTTP(S)_PROXY` set the agent
   never needs to resolve anything itself.
2. **The proxy gets its egress leg by a second attachment**, and that leg
   still cannot reach the host:

   ```
   $ docker network connect bridge de-px-probe
   # from inside the proxy:
   10.0.2.2:9393 BLOCKED              # slirp host loopback
   172.17.0.1:9393 BLOCKED
   192.168.50.90:8888 REACHABLE       # the LAN — this is its job
   ```

   `--disable-host-loopback` holds on the proxy exactly as it does on every
   other container; the exit criterion's "neither the host loopback nor the
   wider LAN" is about the **agent**, which the internal network satisfies.
3. **tinyproxy with `FilterDefaultDeny Yes` and an fnmatch filter does the
   allowlist, and a refusal is observable.** With `example.com` in the filter:

   ```
   http://example.com/   → 200      https://example.com/   → 200 (CONNECT)
   http://httpbin.org/   → HTTP/1.1 403 Filtered   (+ an HTML body)
   https://httpbin.org/  → CONNECT answered "HTTP/1.1 403 Filtered"; curl reports 000
   ```

   and the proxy's own log carries the audit raw material phase-08 will
   consume — one line per request, one per refusal:

   ```
   CONNECT … Request (file descriptor 4): GET http://example.com/ HTTP/1.1
   CONNECT … Request (file descriptor 4): CONNECT httpbin.org:443 HTTP/1.1
   NOTICE  … Proxying refused on filtered domain "httpbin.org"
   ```

   An **empty** filter file refuses everything (`403`) — so a profile with no
   `proxy_allow` is deny-all by construction, not by a special case.
4. **The image built from Task 1 runs unprivileged and sees no capabilities**:
   `docker build` → 2.0 s, 3.8 MB; inside, `id -u` → `1000`, `CapEff:
   0000000000000000`; `Starting main loop. Accepting connections.` on the
   first log line; a deny-all run answers `403`.
5. **Teardown order is forced by the runtime.** `docker network rm` on a
   network with an attached container exits 1 — *"has active endpoints"* —
   and succeeds once the container is removed. The sweep therefore removes
   containers **first**, then networks, then volumes.
6. **Cost, cold:** `network create` 22 ms, proxy `run -d` 169 ms, `network
   connect bridge` 55 ms — ~250 ms per job before the agent container starts.
   Recorded for phase-07's per-job-vs-long-lived decision; not decided here.
7. `docker network ls -q --filter label=de.sandbox=1` lists exactly the
   labelled networks and nothing of the user's; an empty match exits 0.

## Gotchas

1. **Adding a `SandboxConfig` field breaks two things at once, and only one
   of them is the compiler.** The manual `impl Default` fails with `missing
   field` (build), and `tests/doc_truth.rs` fails until
   `assets/etc/config.toml` shows the field (test). Task 2 does all three
   edits; do not stop at the first green build.

2. **`proxy.lock` is a separate file, not a new key.** `parse_lock` rejects
   an unknown key set by design; adding `proxy_image_id` to `sandbox.lock`
   would make every existing lock parse as `None` and refuse every sandboxed
   command on the next daemon start. Do not touch `render_lock`,
   `parse_lock` or `SandboxLock`.

3. **Networks are swept between containers and volumes, not before
   containers** (§ Live measurement 5). Insert the block exactly where Task 4
   says. A network the sweep cannot remove because a container is still on
   it is logged at `warn` and left for the next start — never retried in a
   loop.

4. **Do not add `network create` / `proxy run` / `network connect`
   builders here.** They have no caller until phase-07 and clippy will name
   them dead. If you find yourself writing one, stop — it is phase-07's.

5. **The proxy's egress leg is `network connect bridge`, not a second
   `--network`.** `docker run` accepts one `--network`; the second attachment
   is a separate command after the container exists. This is phase-07's
   problem to sequence, but the conf in Task 1 must not assume a single
   interface (it listens on `0.0.0.0`, measured working across both).

6. **`include_str!` paths are relative to the source file.**
   `src/daemon/executor/container.rs` reaches the repo root with
   `../../../`, so the conf is `include_str!("../../../containers/proxy/tinyproxy.conf")`.
   A wrong depth is a compile error naming the path — fix the path, not the
   test.

7. **No `Upstream` directive, ever.** The proxy is the only door, not a hop
   to another one; a test pins its absence.

## Spec

### Task 1 — The proxy image: two new files under `containers/proxy/`

`containers/proxy/Dockerfile`, exactly:

```dockerfile
FROM alpine:3.22
RUN apk add --no-cache tinyproxy
RUN adduser -D -u 1000 -g '' de
COPY tinyproxy.conf /etc/tinyproxy/tinyproxy.conf
USER 1000:1000
ENTRYPOINT ["tinyproxy", "-d", "-c", "/etc/tinyproxy/tinyproxy.conf"]
```

`containers/proxy/tinyproxy.conf`, exactly:

```
# daemoneye egress proxy. Static; the per-profile allowlist is mounted at
# /etc/tinyproxy/filter by the daemon (M19 phase-08 renders it).
Port 8888
Listen 0.0.0.0
Timeout 30
LogLevel Info
Allow 0.0.0.0/0
FilterDefaultDeny Yes
FilterType fnmatch
Filter "/etc/tinyproxy/filter"
MaxClients 20
```

Both are byte-for-byte the files the § Live measurements were taken with.

### Task 2 — `proxy_image` in the config, in three places

**`src/config/types.rs`** — directly after the `image` field of
`SandboxConfig`:

```rust
    /// Image the egress proxy container runs. Default: "daemoneye-egress-proxy".
    #[serde(default = "default_sandbox_proxy_image")]
    pub proxy_image: String,
```

its default fn directly after `default_sandbox_image`:

```rust
fn default_sandbox_proxy_image() -> String {
    "daemoneye-egress-proxy".to_string()
}
```

and in the manual `impl Default for SandboxConfig`, directly after
`            image: default_sandbox_image(),`:

```rust
            proxy_image: default_sandbox_proxy_image(),
```

**`assets/etc/config.toml`** — directly after line 246
(`# image       = "daemoneye-agent-base"   # tag pinned by digest in sandbox.lock`):

```toml
# proxy_image = "daemoneye-egress-proxy"  # egress proxy image, pinned in proxy.lock (M19)
```

### Task 3 — Path-taking lock functions in `src/daemon/executor/container.rs`

Replace the two hard-wired functions (`read_lock`, `write_lock`) with
delegates plus path-taking variants, and add the proxy lock path. The
resulting block, in full, replacing everything from the `/// Read and parse
the lock.` doc comment through the end of `write_lock`:

```rust
/// Path to the egress proxy image's lock: `etc/proxy.lock`. A second file
/// rather than a second key, because `parse_lock` rejects an unknown key set
/// and every existing `sandbox.lock` must keep parsing.
pub fn proxy_lock_path() -> std::path::PathBuf {
    crate::config::etc_dir().join("proxy.lock")
}

/// Read and parse the lock. `None` when the file is absent or malformed —
/// the caller distinguishes "no lock yet" from "bad lock" by its own logic.
pub fn read_lock() -> Option<SandboxLock> {
    read_lock_from(&lock_path())
}

/// [`read_lock`] for an arbitrary lock file.
pub fn read_lock_from(path: &std::path::Path) -> Option<SandboxLock> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_lock(&text)
}

/// Write `lock` to `lock_path()`, creating `etc/` if needed.
pub fn write_lock(lock: &SandboxLock) -> std::io::Result<()> {
    write_lock_to(&lock_path(), lock)
}

/// [`write_lock`] for an arbitrary lock file.
pub fn write_lock_to(path: &std::path::Path, lock: &SandboxLock) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(render_lock(lock).as_bytes())?;
    f.flush()
}
```

`lock_path` itself does not change.

### Task 4 — Network sweep, same file

Insert the two builders directly **before** the `/// argv listing every
volume name known to the runtime.` doc comment of `sweep_volume_list_args`:

```rust
/// argv listing every egress network this daemon's sandbox created. The
/// label filter is the whole selector: a user's own networks carry no
/// `de.sandbox` label and must never be listed here.
pub fn sweep_network_list_args(cfg: &SandboxConfig) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "network".to_string(),
        "ls".to_string(),
        "-q".to_string(),
        "--filter".to_string(),
        "label=de.sandbox=1".to_string(),
    ]
}

/// argv removing the given networks. Must run **after** their containers are
/// gone: docker refuses to remove a network with active endpoints.
pub fn network_rm_args(cfg: &SandboxConfig, ids: &[String]) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "network".to_string(),
        "rm".to_string(),
    ];
    args.extend(ids.iter().cloned());
    args
}
```

Then in `sweep_sandbox_leftovers`, directly **before** the two lines

```rust
    cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_volume_list_args(cfg));
```

insert:

```rust
    // Networks go after containers and before volumes: a network with an
    // attached container cannot be removed (measured — "has active endpoints").
    cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_network_list_args(cfg));
    let network_ids: Vec<String> = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Err(e) => {
            log::warn!("sandbox network sweep list failed: {e}");
            Vec::new()
        }
    };
    let removed_networks = if network_ids.is_empty() {
        0
    } else {
        cmd = Command::new(&cfg.runtime);
        cmd.args(network_rm_args(cfg, &network_ids));
        match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
            Ok(_) => network_ids.len(),
            Err(e) => {
                log::warn!("sandbox network sweep remove failed: {e}");
                network_ids.len()
            }
        }
    };

```

and change the closing log call to:

```rust
    log::info!(
        "sandbox sweep removed {} orphaned container(s), {} egress network(s) and {} stale staging volume(s)",
        removed_containers,
        removed_networks,
        removed_volumes
    );
```

### Task 5 — `sandbox build` builds and locks both images, in `src/cli/commands/sandbox.rs`

Replace the import block with:

```rust
use crate::daemon::executor::container::{
    SandboxLock, is_valid_image_id, lock_path, proxy_lock_path, read_lock, read_lock_from,
    write_lock, write_lock_to,
};
```

Directly after `format_build_result` add its proxy twin:

```rust
/// Same wording for the proxy image, pointing at its own lock.
fn format_proxy_build_result(image: &str, image_id: &str, rebuilt: bool) -> String {
    let action = if rebuilt { "Rebuilt" } else { "Built" };
    format!(
        "{action} image '{image}' (id {image_id}).\nRecorded in {}",
        proxy_lock_path().display()
    )
}
```

Then replace `run_sandbox_build` **in its entirety** (doc comment included)
with the following three functions. `build_image` is today's inline spawn /
status check / id validation lifted out unchanged; the two call sites are the
only new logic:

```rust
/// Build one image and return its id, or exit with the operator-facing error.
fn build_image(image: &str, dockerfile: &str, context: &str, docker_host: &str) -> String {
    let mut cmd = Command::new("docker");
    cmd.args(["build", "-q", "-t", image, "-f", dockerfile, context])
        .env("DOCKER_HOST", docker_host);
    let output = match bounded_output_with(&mut cmd, Duration::from_secs(600)) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("Failed to spawn docker: {}", e);
            std::process::exit(1);
        }
    };
    if !output.status.success() {
        eprintln!(
            "docker build failed for image '{}' (runtime {}):\n{}",
            image,
            docker_host,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        std::process::exit(1);
    }
    let raw_stdout = String::from_utf8_lossy(&output.stdout);
    let image_id = raw_stdout.trim();
    if !is_valid_image_id(image_id) {
        eprintln!(
            "docker build printed an invalid image id ({:?}); refusing to write a lock.",
            image_id
        );
        std::process::exit(1);
    }
    image_id.to_string()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `daemoneye sandbox build` — build the agent image and the egress proxy
/// image, recording each id in its own lock.
pub fn run_sandbox_build() {
    let cfg = Config::load().unwrap_or_default();
    let image = cfg.sandbox.image.clone();
    let docker_host = cfg.sandbox.docker_host.clone();

    let image_id = build_image(&image, "containers/Dockerfile", "containers", &docker_host);
    let rebuilt = read_lock()
        .map(|lock| lock.image_id != image_id)
        .unwrap_or(false);
    if let Err(e) = write_lock(&SandboxLock {
        image: image.clone(),
        image_id: image_id.clone(),
        built_at: now_secs(),
    }) {
        eprintln!("Failed to write lock file at {}: {}", lock_path().display(), e);
        std::process::exit(1);
    }
    println!("{}", format_build_result(&image, &image_id, rebuilt));

    let proxy_image = cfg.sandbox.proxy_image.clone();
    let proxy_id = build_image(
        &proxy_image,
        "containers/proxy/Dockerfile",
        "containers/proxy",
        &docker_host,
    );
    let proxy_path = proxy_lock_path();
    let proxy_rebuilt = read_lock_from(&proxy_path)
        .map(|lock| lock.image_id != proxy_id)
        .unwrap_or(false);
    if let Err(e) = write_lock_to(
        &proxy_path,
        &SandboxLock {
            image: proxy_image.clone(),
            image_id: proxy_id.clone(),
            built_at: now_secs(),
        },
    ) {
        eprintln!("Failed to write lock file at {}: {}", proxy_path.display(), e);
        std::process::exit(1);
    }
    println!("{}", format_proxy_build_result(&proxy_image, &proxy_id, proxy_rebuilt));
}
```

The existing `mod tests` in this file is untouched.

### Task 6 — Tests in `container.rs`'s existing `mod tests`

Six tests, named exactly as below, appended at the end of the module. Every
name begins `sandbox_proxy_`.

```rust
#[test]
fn sandbox_proxy_network_list_args_filter_by_label() {
    let cfg = SandboxConfig::default();
    let args = sweep_network_list_args(&cfg);
    assert_eq!(args.first().map(String::as_str), Some("--host"), "{args:?}");
    assert!(args.iter().any(|a| a == "network"), "{args:?}");
    assert!(args.iter().any(|a| a == "ls"), "{args:?}");
    assert!(
        args.iter().any(|a| a == "label=de.sandbox=1"),
        "without the label filter a user's own networks would be swept: {args:?}"
    );
}

#[test]
fn sandbox_proxy_network_rm_args_are_empty_for_an_empty_slice() {
    let cfg = SandboxConfig::default();
    assert!(network_rm_args(&cfg, &[]).is_empty());
    let args = network_rm_args(&cfg, &["n1".to_string(), "n2".to_string()]);
    assert!(args.iter().any(|a| a == "network"), "{args:?}");
    assert!(args.iter().any(|a| a == "rm"), "{args:?}");
    assert_eq!(&args[args.len() - 2..], &["n1".to_string(), "n2".to_string()]);
}

#[test]
fn sandbox_proxy_lock_lives_beside_the_image_lock() {
    let a = lock_path();
    let b = proxy_lock_path();
    assert_ne!(a, b, "two images, two locks");
    assert_eq!(a.parent(), b.parent(), "same etc/ directory");
    assert!(b.ends_with("proxy.lock"), "{b:?}");
}

#[test]
fn sandbox_proxy_lock_round_trips_through_an_arbitrary_path() {
    let dir = std::env::temp_dir().join(format!("de-proxy-lock-{}", std::process::id()));
    let path = dir.join("nested").join("proxy.lock");
    let lock = SandboxLock {
        image: "daemoneye-egress-proxy".to_string(),
        image_id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
        built_at: 42,
    };
    write_lock_to(&path, &lock).expect("write creates parents");
    let back = read_lock_from(&path).expect("parses back");
    assert_eq!(back.image, lock.image);
    assert_eq!(back.image_id, lock.image_id);
    assert_eq!(back.built_at, 42);
    assert!(read_lock_from(&dir.join("absent.lock")).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sandbox_proxy_conf_denies_by_default_and_reads_the_mounted_filter() {
    // The conf is baked into the image; the allowlist is mounted per job.
    // Measured 2026-08-30: an empty filter file with these settings answers
    // every request `403 Filtered`, for GET and for CONNECT alike.
    let conf = include_str!("../../../containers/proxy/tinyproxy.conf");
    assert!(conf.lines().any(|l| l.trim() == "FilterDefaultDeny Yes"), "{conf}");
    assert!(conf.lines().any(|l| l.trim() == "FilterType fnmatch"), "{conf}");
    assert!(
        conf.lines().any(|l| l.trim() == "Filter \"/etc/tinyproxy/filter\""),
        "the daemon mounts the allowlist at exactly this path: {conf}"
    );
    assert!(conf.lines().any(|l| l.trim() == "Port 8888"), "{conf}");
    assert!(
        !conf.contains("Upstream"),
        "no upstream: the proxy is the only door, not a hop"
    );
}

#[test]
fn sandbox_proxy_dockerfile_runs_unprivileged_and_bakes_the_conf() {
    let df = include_str!("../../../containers/proxy/Dockerfile");
    assert!(df.lines().any(|l| l.trim() == "USER 1000:1000"), "{df}");
    assert!(df.contains("tinyproxy.conf /etc/tinyproxy/tinyproxy.conf"), "{df}");
    assert!(df.contains("apk add --no-cache tinyproxy"), "{df}");
}
```

### Task 7 — Mutation pair M1: the network sweep's label filter is real

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them.** Append each marker and run to `/tmp/e2e-06.txt`. Run the gates
(§ End-to-end verification) only **after** both pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`. **The two-line
   filter alone is NOT unique** — `"label=de.sandbox=1".to_string(),` occurs
   three times on the finished tree. The `old_str` below reaches up to
   `"ls"`, which only the network builder has (measured: one occurrence):

   ```
           "ls".to_string(),
           "-q".to_string(),
           "--filter".to_string(),
           "label=de.sandbox=1".to_string(),
       ]
   ```

   `new_str`:

   ```
           "ls".to_string(),
           "-q".to_string(),
       ]
   ```

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-06.txt
   cargo test --lib sandbox_proxy 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-06.txt
   grep -c '"label=de.sandbox=1".to_string(),' src/daemon/executor/container.rs >> /tmp/e2e-06.txt
   ```
   Measured on the prototype: **exactly 1 failed**, naming
   `sandbox_proxy_network_list_args_filter_by_label`, and the `grep -c`
   prints `2`. A green suite here means a daemon start would remove every
   network on the host — record a blocker.

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M1 RESTORED ==" >> /tmp/e2e-06.txt
   cargo test --lib sandbox_proxy 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-06.txt
   grep -c '"label=de.sandbox=1".to_string(),' src/daemon/executor/container.rs >> /tmp/e2e-06.txt
   ```
   The tests pass and the `grep -c` prints `3`.

### Task 8 — Mutation pair M2: deny-by-default is pinned in the conf

Only after M1 is restored. This mutation is to a **non-Rust file**; `patch`
works on it the same way.

1. **Apply.** `patch` `containers/proxy/tinyproxy.conf`:
   - `old_str`: `FilterDefaultDeny Yes`
   - `new_str`: `FilterDefaultDeny No`

   Then:
   ```sh
   echo "== M2 APPLIED ==" >> /tmp/e2e-06.txt
   cargo test --lib sandbox_proxy 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-06.txt
   grep -c '^FilterDefaultDeny Yes$' containers/proxy/tinyproxy.conf >> /tmp/e2e-06.txt
   ```
   Measured: **exactly 1 failed**, naming
   `sandbox_proxy_conf_denies_by_default_and_reads_the_mounted_filter`, and
   the `grep -c` prints `0`. (`include_str!` re-reads the file at compile
   time, so the test binary rebuilds — that is expected, not a stale cache.)

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M2 RESTORED ==" >> /tmp/e2e-06.txt
   cargo test --lib sandbox_proxy 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-06.txt
   grep -c '^FilterDefaultDeny Yes$' containers/proxy/tinyproxy.conf >> /tmp/e2e-06.txt
   ```
   The `grep -c` prints `1`.

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the wrong line fails silently, and a mutation that never
applied certifies a vacuous guard. **Both failure counts above were measured,
not estimated.** If a mutation fails a different number of tests than stated,
do not adjust a test to match — record a blocker naming the criterion.

### Task 9 — Capture the end-to-end evidence

**The § End-to-end block appends (`>> /tmp/e2e-06.txt`). If you need to run it
a second time — for any reason — `rm -f /tmp/e2e-06.txt` first and run the
whole sequence again from Task 7.** Two executions otherwise leave two copies
in the file, the paste holds one, and the self-check prints `PASTE MISMATCH`.
**Never edit `/tmp/e2e-06.txt` or the pasted block to reconcile them** — the
`PASTE MATCH` check is worth nothing if either side can be adjusted until they
agree, and what an edit removes is usually the failing line that mattered. Run
`cargo fmt --all` **before** the block so `fmt_exit` is a real `0`, not one
produced by a later fix.

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-06.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. **The entry ends with the
self-check's verdict line, `PASTE MATCH`, bare on its own line after the
fenced block** — a tick in your final summary is not that line.

## Acceptance criteria

**Every count below was read off the architect's prototype of this exact
change, not derived from the spec text.**

- [ ] `ls containers/proxy | wc -l` prints `2` (**before: the directory does
      not exist**); `grep -c '^FilterDefaultDeny Yes$' containers/proxy/tinyproxy.conf`
      prints `1` and `grep -c '^USER 1000:1000$' containers/proxy/Dockerfile`
      prints `1`.
- [ ] `grep -c 'proxy_image' src/config/types.rs` prints `4` (**before: 0**)
      — field, serde attr, default fn, `Default` impl — and
      `grep -c 'proxy_image' assets/etc/config.toml` prints `1` (**before:
      0**).
- [ ] `grep -c 'fn sweep_network_list_args(' src/daemon/executor/container.rs`,
      `grep -c 'fn network_rm_args(' …`, `grep -c 'fn proxy_lock_path(' …`,
      `grep -c 'fn read_lock_from(' …` and `grep -c 'fn write_lock_to(' …`
      each print `1` (**before: 0**).
- [ ] `grep -c '"label=de.sandbox=1".to_string(),' src/daemon/executor/container.rs`
      prints `3` (**before: 2**).
- [ ] `grep -c 'cmd.args(sweep_network_list_args(cfg));' src/daemon/executor/container.rs`
      prints `1` and `grep -c 'removed_networks' …` prints `2` (**before: 0,
      0**) — the sweep calls the builder and reports the count.
- [ ] `grep -c 'build_image(' src/cli/commands/sandbox.rs` prints `3`
      (**before: 0**) — the definition and both call sites — and
      `grep -c 'proxy_lock_path' src/cli/commands/sandbox.rs` prints `3`.
- [ ] `cargo test --lib sandbox_proxy 2>&1 | grep -c "^test .* ok$"` prints
      `6`. A count, not an exit status.
- [ ] `cargo test --lib` reports **1491** passing and `0 failed`
      (**before: 1485**), with `4 ignored` unchanged; and **`cargo test`
      (all targets)** is green — `tests/doc_truth.rs` is the one that checks
      the seeded config (§ Gotchas 1).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**unchanged**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0`, and the same on `src/cli/commands/sandbox.rs` prints `0`
      (**before: 0, 0**).
- [ ] The § End-to-end entry shows `== M1 APPLIED ==` and `== M2 APPLIED ==`
      each failing **exactly one** named test — the two names in Tasks 7 and
      8 — both `RESTORED` runs passing, with a `grep -c` line after each
      direction reading the value that task states.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks):
      `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-06-proxy-network-and-image.md`
      prints `1`.

## Test plan

Six unit tests in `container.rs`'s `mod tests`, given in full in Task 6. No
new test file; no existing test changes.

**The negative cases are the phase.**
`sandbox_proxy_network_list_args_filter_by_label` pins the one thing that
makes the network sweep safe on a shared docker host — without the label
filter a daemon start would remove the user's own networks — and M1 proves it
is live. `sandbox_proxy_conf_denies_by_default_and_reads_the_mounted_filter`
pins the four conf lines the whole D5 mechanism rests on (deny by default, the
mount path phase-08 will write to, the port phase-07 will point
`HTTP(S)_PROXY` at, no `Upstream`), through `include_str!` so the pin is on
the real file and not a copy; M2 proves it.
`sandbox_proxy_lock_round_trips_through_an_arbitrary_path` is what makes the
second lock trustworthy, including the create-parents case a fresh install
hits.

`sweep_sandbox_leftovers`' new network step and `run_sandbox_build`'s second
build both spawn `docker` and are **not** unit-tested, matching how the rest
of the sweep and the first build are treated; the sweep order (§ Live
measurement 5) is verified live at milestone close. Behaviour is unchanged
with the sandbox disabled: the sweep returns early on `!cfg.enabled` before
any of this runs. **If an existing test requires a change to pass, stop and
record a blocker.**

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 7 and 8 have
appended their mutation markers to `/tmp/e2e-06.txt` and both pairs are
restored.

```sh
{
echo "== A. named tests (expect 6 ok) =="
cargo test --lib sandbox_proxy 2>&1 | grep -E "^test |^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. full suite, all targets =="
cargo test 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== D. structural greps =="
echo -n "containers/proxy files (2):     "; ls containers/proxy | wc -l
echo -n "conf deny-by-default (1):       "; grep -c '^FilterDefaultDeny Yes$' containers/proxy/tinyproxy.conf
echo -n "dockerfile USER 1000 (1):       "; grep -c '^USER 1000:1000$' containers/proxy/Dockerfile
echo -n "proxy_image types.rs (4):       "; grep -c 'proxy_image' src/config/types.rs
echo -n "proxy_image config.toml (1):    "; grep -c 'proxy_image' assets/etc/config.toml
echo -n "fn sweep_network_list_args (1): "; grep -c 'fn sweep_network_list_args(' src/daemon/executor/container.rs
echo -n "fn network_rm_args (1):         "; grep -c 'fn network_rm_args(' src/daemon/executor/container.rs
echo -n "fn proxy_lock_path (1):         "; grep -c 'fn proxy_lock_path(' src/daemon/executor/container.rs
echo -n "fn read_lock_from (1):          "; grep -c 'fn read_lock_from(' src/daemon/executor/container.rs
echo -n "fn write_lock_to (1):           "; grep -c 'fn write_lock_to(' src/daemon/executor/container.rs
echo -n "label filter lines (3):         "; grep -c '"label=de.sandbox=1".to_string(),' src/daemon/executor/container.rs
echo -n "sweep calls network list (1):   "; grep -c 'cmd.args(sweep_network_list_args(cfg));' src/daemon/executor/container.rs
echo -n "removed_networks (2):           "; grep -c 'removed_networks' src/daemon/executor/container.rs
echo -n "build_image sites (3):          "; grep -c 'build_image(' src/cli/commands/sandbox.rs
echo -n "proxy_lock_path in cli (3):     "; grep -c 'proxy_lock_path' src/cli/commands/sandbox.rs
echo -n "allow total (6):                "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "prod unwrap container.rs (0):   "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('
echo -n "prod unwrap sandbox.rs (0):     "; sed -n '1,/^#\[cfg(test)\]/p' src/cli/commands/sandbox.rs | grep -c '\.unwrap()\|\.expect('
} >> /tmp/e2e-06.txt 2>&1
cat /tmp/e2e-06.txt
```

Paste the whole of `/tmp/e2e-06.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-06-proxy-network-and-image.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06.txt
diff /tmp/pasted-06.txt /tmp/e2e-06.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Create `containers/proxy/Dockerfile` and `containers/proxy/tinyproxy.conf`.
  Edit `src/daemon/executor/container.rs`, `src/cli/commands/sandbox.rs`,
  `src/config/types.rs` and `assets/etc/config.toml` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- No `#[allow(...)]` may be added or removed, and no `#[ignore]` may be added
  or removed.
- **Do not change any existing test's assertions**, and do not touch
  `SandboxLock`, `render_lock` or `parse_lock` (§ Gotchas 2).
- **Do not run `docker`, `podman`, or any container command** — including
  `daemoneye sandbox build` — and do not start, stop or query a system
  service. The image and every network behaviour this phase depends on were
  measured by the architect (§ Live measurements) and are re-verified at
  milestone close.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it would discard this round's own uncommitted work.
- **Do not edit any other source file, and do not edit any doc other than this
  phase doc's Update Log.**
- **Append to the Update Log; never edit or delete an existing entry.** When
  flipping this doc's `Status:` line, change **only** that line — the line
  above it is `**Milestone:** M19 — Sandbox Completion` and must survive (a
  mis-anchored status patch ate it in phase-03; see `bugs/bug-phase-03-1.md`).
  After the flip, `grep -c '^\*\*Status:\*\*' <this doc>` must print `1` and
  `grep -c '^\*\*Milestone:\*\*' <this doc>` must print `1`.
- **Never edit `/tmp/e2e-06.txt` or the pasted evidence block after capture,
  for any reason** (Task 9). On a `PASTE MISMATCH`, delete the artifact and
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
  describe the end-to-end artifact — paste it and let it speak.

## Out of scope

- **`network create`, `proxy run` and `network connect` argv builders, and
  every per-job network/proxy lifecycle call** — phase-07, beside their
  callers (§ Gotchas 4). The measured facts they need are in § Live
  measurements 1, 2, 5 and 6.
- **Preflight for the proxy image** — refusing a `network = "proxy"` command
  when `proxy.lock` is absent or mismatched. Phase-07, where the profile is
  first honoured; here nothing reads `proxy.lock` but `sandbox build`.
- **Rendering the per-profile filter file** and the audit record — phase-08.
- **Raw TCP / SSH egress** — deferred past M19 (README § 06 intent). The conf
  listens on `0.0.0.0` and sets no `Upstream`; nothing here forecloses it.
- **Per-job vs long-lived proxy** — § Live measurement 6 gives phase-07 the
  cold-start number; the decision is made there.
- **Hardening flags on the proxy container** (`--read-only`, `--cap-drop`) —
  phase-11, which owns those for every sandbox container.
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep. The
  `CLAUDE.md` "Container sandbox" section should gain the second image and
  lock; `daemoneye sandbox build`'s README line becomes "builds both".

## Update Log

<!-- entries appended below this line -->
