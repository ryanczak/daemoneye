use crate::config::SandboxConfig;
use crate::tmux::bounded_output_with;
use std::process::Command;
use std::time::Duration;

/// Why sandboxed execution is unavailable. Each variant maps to a different
/// operator fix, so they must stay distinct.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeUnavailable {
    /// The runtime binary is not on PATH (spawn failed with NotFound).
    NotInstalled { runtime: String },
    /// The binary ran but could not reach its daemon.
    DaemonUnreachable { docker_host: String, stderr: String },
    /// `[sandbox] runtime` names something this build does not support.
    UnsupportedRuntime { runtime: String },
}

/// Result of the D1 UID-mapping gate.
#[derive(Debug, Clone, PartialEq)]
pub enum UidGateOutcome {
    /// Sandboxed execution may proceed.
    Ok { container_uid: u32, host_uid: u32 },
    /// The container process is container root, which maps to the daemon's
    /// own host uid — the sandbox would not reduce the blast radius.
    ContainerRoot { host_uid: u32 },
    /// The container uid is not covered by any range in the uid map.
    Unmapped { container_uid: u32 },
    /// `/proc/self/uid_map` could not be parsed.
    MalformedMap,
}

/// One `container_start host_start length` range from `/proc/self/uid_map`.
#[derive(Debug, Clone, PartialEq)]
pub struct UidRange {
    pub container_start: u32,
    pub host_start: u32,
    pub length: u32,
}

/// Parse `/proc/self/uid_map` content into its ranges.
/// Blank lines are skipped. A line that does not yield exactly three
/// whitespace-separated `u32` fields makes the whole parse fail (`None`) —
/// a partially-understood map must never be treated as authoritative.
pub fn parse_uid_map(text: &str) -> Option<Vec<UidRange>> {
    let mut ranges = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() != 3 {
            return None;
        }
        let container_start = fields[0].parse::<u32>().ok()?;
        let host_start = fields[1].parse::<u32>().ok()?;
        let length = fields[2].parse::<u32>().ok()?;
        ranges.push(UidRange {
            container_start,
            host_start,
            length,
        });
    }
    Some(ranges)
}

/// Host uid for `container_uid` under `ranges`, or `None` when no range
/// covers it. A range covers `[container_start, container_start + length)`
/// and maps to `host_start + (container_uid - container_start)`.
pub fn host_uid_for(container_uid: u32, ranges: &[UidRange]) -> Option<u32> {
    for range in ranges {
        let end = range.container_start.checked_add(range.length)?;
        if container_uid >= range.container_start && container_uid < end {
            return Some(range.host_start + (container_uid - range.container_start));
        }
    }
    None
}

/// Decide the D1 gate from the two inputs the probe collects: the container's
/// own uid (`id -u` inside it) and its `/proc/self/uid_map` content.
/// Both are needed — the map alone cannot distinguish a root container from a
/// non-root one.
pub fn evaluate_uid_gate(container_uid: u32, uid_map: &str) -> UidGateOutcome {
    let Some(ranges) = parse_uid_map(uid_map) else {
        return UidGateOutcome::MalformedMap;
    };
    let Some(host_uid) = host_uid_for(container_uid, &ranges) else {
        return UidGateOutcome::Unmapped { container_uid };
    };
    if container_uid == 0 {
        UidGateOutcome::ContainerRoot { host_uid }
    } else {
        UidGateOutcome::Ok {
            container_uid,
            host_uid,
        }
    }
}

/// Classify the outcome of running `<runtime> version --format '{{.Server.Version}}'`.
/// `spawn_kind` is `Some(ErrorKind)` when the spawn itself failed.
pub fn classify_version_probe(
    runtime: &str,
    docker_host: &str,
    spawn_kind: Option<std::io::ErrorKind>,
    exit_ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<String, RuntimeUnavailable> {
    match spawn_kind {
        Some(std::io::ErrorKind::NotFound) => {
            return Err(RuntimeUnavailable::NotInstalled {
                runtime: runtime.to_string(),
            });
        }
        Some(_) => {}
        None => {}
    }
    if !exit_ok {
        return Err(RuntimeUnavailable::DaemonUnreachable {
            docker_host: docker_host.to_string(),
            stderr: stderr.trim().to_string(),
        });
    }
    let version = stdout.trim();
    if version.is_empty() {
        return Err(RuntimeUnavailable::DaemonUnreachable {
            docker_host: docker_host.to_string(),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(version.to_string())
}

/// Run the runtime's version probe. The only impure function in this module:
/// it shells out and hands the raw results to `classify_version_probe`.
pub fn probe_runtime(cfg: &SandboxConfig) -> Result<String, RuntimeUnavailable> {
    if cfg.runtime != "docker" {
        return Err(RuntimeUnavailable::UnsupportedRuntime {
            runtime: cfg.runtime.clone(),
        });
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(["version", "--format", "{{.Server.Version}}"])
        .env("DOCKER_HOST", &cfg.docker_host);
    match bounded_output_with(&mut cmd, Duration::from_secs(10)) {
        Ok(out) => classify_version_probe(
            &cfg.runtime,
            &cfg.docker_host,
            None,
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(e) => classify_version_probe(
            &cfg.runtime,
            &cfg.docker_host,
            Some(e.kind()),
            false,
            "",
            "",
        ),
    }
}

/// The recorded identity of the agent image, persisted at
/// `~/.daemoneye/etc/sandbox.lock`. Phase-04 refuses to run a container whose
/// image id differs from this.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxLock {
    pub image: String,
    pub image_id: String,
    pub built_at: u64,
}

/// True when `s` is a well-formed docker image id: literal "sha256:" followed
/// by exactly 64 lowercase hex characters.
pub fn is_valid_image_id(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Serialize a lock to the on-disk form (see below).
pub fn render_lock(lock: &SandboxLock) -> String {
    format!(
        "image = {}\nimage_id = {}\nbuilt_at = {}\n",
        lock.image, lock.image_id, lock.built_at
    )
}

/// Parse the on-disk form. `None` for a malformed record, an unknown key set,
/// or an `image_id` that fails `is_valid_image_id`.
pub fn parse_lock(text: &str) -> Option<SandboxLock> {
    let mut image: Option<String> = None;
    let mut image_id: Option<String> = None;
    let mut built_at: Option<u64> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        let value = value.trim();
        if key == "image" && image.is_none() {
            image = Some(value.to_string());
        } else if key == "image_id" && image_id.is_none() {
            image_id = Some(value.to_string());
        } else if key == "built_at" && built_at.is_none() {
            built_at = Some(value.parse::<u64>().ok()?);
        } else {
            return None;
        }
    }
    let image_id = image_id?;
    if !is_valid_image_id(&image_id) {
        return None;
    }
    Some(SandboxLock {
        image: image?,
        image_id,
        built_at: built_at?,
    })
}

/// Path to the lock file: `crate::config::etc_dir().join("sandbox.lock")`.
pub fn lock_path() -> std::path::PathBuf {
    crate::config::etc_dir().join("sandbox.lock")
}

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

/// Compare a live image id against the lock. Phase-04's refusal gate.
pub fn check_image_matches(lock: &SandboxLock, live_image_id: &str) -> ImageCheck {
    if !is_valid_image_id(live_image_id) {
        return ImageCheck::MalformedLive {
            live: live_image_id.to_string(),
        };
    }
    if lock.image_id == live_image_id {
        ImageCheck::Match
    } else {
        ImageCheck::Mismatch {
            locked: lock.image_id.clone(),
            live: live_image_id.to_string(),
        }
    }
}

/// Result of comparing a live image id against a lock.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageCheck {
    Match,
    Mismatch {
        locked: String,
        live: String,
    },
    /// The live id is not a well-formed image id at all.
    MalformedLive {
        live: String,
    },
}

/// Split a `"uid:gid"` string into its numeric halves.
/// `None` when either half is missing or non-numeric. Used for both the
/// `--user` flag and the tmpfs `uid=`/`gid=` options, so the two can never
/// disagree.
pub fn split_run_as(run_as: &str) -> Option<(u32, u32)> {
    let halves: Vec<&str> = run_as.split(':').collect();
    if halves.len() != 2 {
        return None;
    }
    let uid = halves[0].trim();
    let gid = halves[1].trim();
    if uid.is_empty() || gid.is_empty() {
        return None;
    }
    Some((uid.parse().ok()?, gid.parse().ok()?))
}

/// Why sandboxed execution cannot proceed. One operator-facing reason,
/// collapsed from the three independent checks.
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxUnavailable {
    /// The runtime is missing, unreachable, or unsupported.
    Runtime(RuntimeUnavailable),
    /// The uid gate did not return `Ok` — carries the outcome that failed.
    UidGate(UidGateOutcome),
    /// No `sandbox.lock` exists; `daemoneye sandbox build` has not been run.
    NoLock,
    /// The live image does not match the lock — carries the failing check.
    Image(ImageCheck),
    /// `run_as` is not a parseable `uid:gid` pair.
    BadRunAs { run_as: String },
}

/// Decide whether sandboxed execution may proceed, from inputs the caller has
/// already collected. Pure: it starts no process and reads no file.
pub fn evaluate_preflight(
    run_as: &str,
    version: &Result<String, RuntimeUnavailable>,
    gate: &UidGateOutcome,
    lock: Option<&SandboxLock>,
    live_image_id: &str,
) -> Result<(), SandboxUnavailable> {
    if split_run_as(run_as).is_none() {
        return Err(SandboxUnavailable::BadRunAs {
            run_as: run_as.to_string(),
        });
    }
    if let Err(err) = version {
        return Err(SandboxUnavailable::Runtime(err.clone()));
    }
    match gate {
        UidGateOutcome::Ok { .. } => {}
        other => return Err(SandboxUnavailable::UidGate(other.clone())),
    }
    let lock = match lock {
        Some(l) => l,
        None => return Err(SandboxUnavailable::NoLock),
    };
    let check = check_image_matches(lock, live_image_id);
    if check != ImageCheck::Match {
        return Err(SandboxUnavailable::Image(check));
    }
    Ok(())
}

/// Split the combined probe's stdout into `(container_uid, uid_map)`.
/// The probe prints the uid, a line containing only `---`, then the map.
/// `None` when the sentinel is missing or the uid line is not a `u32`.
pub fn parse_probe_output(text: &str) -> Option<(u32, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let sentinel = lines.iter().position(|line| line.trim() == "---")?;
    let uid = lines[..sentinel]
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())?
        .parse::<u32>()
        .ok()?;
    let map = lines[sentinel + 1..].join("\n");
    Some((uid, map.trim_matches('\n').to_string()))
}

/// Run the combined probe of § Gotchas 1 and reduce its output to the uid gate.
fn probe_uid_gate(cfg: &SandboxConfig) -> UidGateOutcome {
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args([
        "run",
        "--rm",
        "--user",
        &cfg.run_as,
        "--network",
        "none",
        &cfg.image,
        "sh",
        "-c",
        "id -u; echo ---; cat /proc/self/uid_map",
    ])
    .env("DOCKER_HOST", &cfg.docker_host);
    match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            match parse_probe_output(&text) {
                Some((uid, map)) => evaluate_uid_gate(uid, &map),
                None => UidGateOutcome::MalformedMap,
            }
        }
        Err(_) => UidGateOutcome::MalformedMap,
    }
}

/// The live image id via `image inspect`, or the empty string when it cannot
/// be read — `evaluate_preflight` treats a malformed live id correctly.
fn probe_live_image_id(cfg: &SandboxConfig) -> String {
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(["image", "inspect", &cfg.image, "--format", "{{.Id}}"])
        .env("DOCKER_HOST", &cfg.docker_host);
    match bounded_output_with(&mut cmd, Duration::from_secs(15)) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// Run the combined probe and the image inspection, then decide.
/// Impure: starts one container and runs one `image inspect`.
fn collect_preflight(cfg: &SandboxConfig) -> Result<(), SandboxUnavailable> {
    let version = probe_runtime(cfg);
    let gate = probe_uid_gate(cfg);
    let live_id = probe_live_image_id(cfg);
    evaluate_preflight(&cfg.run_as, &version, &gate, read_lock().as_ref(), &live_id)
}

/// Cached sandbox verdict — probed once per daemon lifetime.
pub fn sandbox_preflight(cfg: &SandboxConfig) -> Result<(), SandboxUnavailable> {
    if !cfg.enabled {
        return Ok(());
    }
    static SANDBOX_VERDICT: SandboxVerdictCell = SandboxVerdictCell::new();
    SANDBOX_VERDICT
        .get_or_init(|| collect_preflight(cfg))
        .clone()
}

type SandboxVerdictCell = std::sync::OnceLock<Result<(), SandboxUnavailable>>;

/// Operator-facing explanation of why sandboxed execution is unavailable,
/// including the concrete fix. Used as the tool result the AI sees.
pub fn describe_unavailable(reason: &SandboxUnavailable) -> String {
    match reason {
        SandboxUnavailable::Runtime(RuntimeUnavailable::NotInstalled { runtime }) => format!(
            "sandbox unavailable: runtime `{runtime}` is not installed — install it (e.g. `docker`) and try again"
        ),
        SandboxUnavailable::Runtime(RuntimeUnavailable::DaemonUnreachable {
            docker_host, ..
        }) => format!(
            "sandbox unavailable: cannot reach the container runtime at `{docker_host}` — start the user docker service (e.g. `systemctl --user start docker.socket`) and try again"
        ),
        SandboxUnavailable::Runtime(RuntimeUnavailable::UnsupportedRuntime { runtime }) => format!(
            "sandbox unavailable: runtime `{runtime}` is not supported (only `docker` is) — set `[sandbox] runtime = \"docker\"` and try again"
        ),
        SandboxUnavailable::UidGate(UidGateOutcome::ContainerRoot { .. }) => {
            "sandbox unavailable: the container would run as root, which maps to the daemon's own host uid — set `[sandbox] run_as` to an unprivileged numeric uid:gid and rebuild the image, then try again".to_string()
        }
        SandboxUnavailable::UidGate(UidGateOutcome::Unmapped { container_uid }) => format!(
            "sandbox unavailable: container uid {container_uid} is not covered by any range in the uid map — adjust `[sandbox] run_as` or the rootless subuid allocation, then retry"
        ),
        SandboxUnavailable::UidGate(UidGateOutcome::MalformedMap) => {
            "sandbox unavailable: the container's uid map could not be parsed — check the rootless docker installation and retry".to_string()
        }
        SandboxUnavailable::UidGate(UidGateOutcome::Ok { .. }) => {
            "sandbox unavailable: uid gate passed but preflight still failed (internal inconsistency)".to_string()
        }
        SandboxUnavailable::NoLock => {
            "sandbox unavailable: no sandbox.lock exists — run `daemoneye sandbox build` to create the image lock, then try again".to_string()
        }
        SandboxUnavailable::Image(ImageCheck::Mismatch { live, .. }) => format!(
            "sandbox unavailable: the live image ({live}) differs from the lock — run `daemoneye sandbox build` to rebuild and re-lock, then try again"
        ),
        SandboxUnavailable::Image(ImageCheck::MalformedLive { live }) => format!(
            "sandbox unavailable: the live image id `{live}` is malformed — run `daemoneye sandbox build` to rebuild and re-lock, then try again"
        ),
        SandboxUnavailable::Image(ImageCheck::Match) => {
            "sandbox unavailable: image matched the lock but preflight still failed (internal inconsistency)".to_string()
        }
        SandboxUnavailable::BadRunAs { run_as } => format!(
            "sandbox unavailable: run_as `{run_as}` is not a numeric uid:gid pair — set `[sandbox] run_as` (e.g. \"1000:1000\") and try again"
        ),
    }
}

/// Per-run staging volume name for `job_id`: `de-stage-<job_id>`.
pub fn stage_volume_name(job_id: &str) -> String {
    format!("de-stage-{job_id}")
}

/// The job id for a pane's sandboxed run: the pane number without tmux's `%`
/// sigil, then the run's unix timestamp. Both background paths build it here
/// so the container the command runs in and the volume staged for it always
/// name the same job.
pub fn job_id_for(pane_id: &str, unix_ts: i64) -> String {
    format!("{}-{}", pane_id.trim_start_matches('%'), unix_ts)
}

fn script_name_is_safe(script_name: &str) -> bool {
    if script_name.is_empty() || script_name.contains("..") {
        return false;
    }
    script_name.chars().all(|c| {
        !c.is_whitespace() && !matches!(c, '/' | ';' | '&' | '|' | '$' | '`' | '\'' | '"' | '\n')
    })
}

/// argv for the short-lived helper that stages one approved script into the
/// per-run volume. Runs as **container root** (`--user 0:0`) because it must
/// read the 0700 originals and chown the copy — it never runs agent-supplied
/// code, only this fixed shell line.
pub fn stage_args(cfg: &SandboxConfig, job_id: &str, script_name: &str) -> Vec<String> {
    if !script_name_is_safe(script_name) {
        return Vec::new();
    }
    let Some((uid, gid)) = split_run_as(&cfg.run_as) else {
        return Vec::new();
    };
    let volume = stage_volume_name(job_id);
    let shell_line = format!(
        "cp /de/src/{script_name} /stage/{script_name} && chmod 0500 /stage/{script_name} && chown {uid}:{gid} /stage/{script_name}"
    );
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "run".to_string(),
        "--rm".to_string(),
        "--user".to_string(),
        "0:0".to_string(),
        "-v".to_string(),
        format!("{}:/de/src:ro", crate::config::scripts_dir().display()),
        "-v".to_string(),
        format!("{volume}:/stage"),
        "--label".to_string(),
        "de.sandbox=1".to_string(),
        cfg.image.clone(),
        "sh".to_string(),
        "-c".to_string(),
        shell_line,
    ]
}

/// The daemon-host script a sandboxed background command invokes, if any,
/// as `(name, args_tail)` from [`crate::scripts::parse_script_invocation`].
///
/// Pure: `script_exists` answers whether `~/.daemoneye/scripts/<name>` is a
/// real script (production passes `resolve_script(..).is_ok()`; tests pass a
/// closure). `ls -la` parses as a candidate but does not exist, so it is an
/// ordinary command. A command under `sudo` is never staged — sudo inside the
/// sandbox is the escape hatch's business, not staging's.
pub fn sandbox_script_invocation(
    cmd: &str,
    script_exists: impl Fn(&str) -> bool,
) -> Option<(String, String)> {
    if crate::daemon::utils::command_has_sudo(cmd) {
        return None;
    }
    let (name, args_tail) = crate::scripts::parse_script_invocation(cmd)?;
    if !script_exists(&name) {
        return None;
    }
    Some((name, args_tail))
}

/// The in-container command for a staged script: its path under the
/// `/de/scripts` mount `run_args` provides, then the verbatim argument tail.
pub fn staged_script_command(script_name: &str, args_tail: &str) -> String {
    format!("/de/scripts/{script_name}{args_tail}")
}

/// Stage one script into this job's volume by spawning the helper
/// [`stage_args`] describes. Blocking — call it off the async runtime.
/// Fails closed: every error is an operator-facing reason and the caller
/// must not run the command.
pub fn stage_script(cfg: &SandboxConfig, job_id: &str, script_name: &str) -> Result<(), String> {
    let args = stage_args(cfg, job_id, script_name);
    if args.is_empty() {
        return Err(format!(
            "sandbox staging refused: `{script_name}` is not a stageable script name"
        ));
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(args);
    match bounded_output_with(&mut cmd, Duration::from_secs(60)) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "sandbox staging failed for `{script_name}`: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("sandbox staging failed for `{script_name}`: {e}")),
    }
}

/// Remove this job's staging volume once the job is over. Best-effort: a
/// failure is logged, never surfaced — the startup sweep reclaims leftovers.
pub fn remove_stage_volume(cfg: &SandboxConfig, job_id: &str) {
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_volume_rm_args(cfg, &[stage_volume_name(job_id)]));
    if let Err(e) = bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        log::warn!("sandbox stage volume remove failed for job {job_id}: {e}");
    }
}

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

/// One parsed egress rule from `proxy_allow` / `proxy_deny`.
///
/// The variants are exactly what tinyproxy's filter can express, measured
/// 2026-08-30: a filter line is an fnmatch pattern tested against the **host
/// alone**, so `example.com` matches only `example.com`, `*.example.com`
/// matches its subdomains but **not** the apex, and a `host:port` line
/// matches nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyRule {
    /// Exactly this host.
    Host(String),
    /// Every subdomain of this domain — not the domain itself.
    Subdomains(String),
    /// Cannot be enforced; carries the operator-facing reason.
    Unsupported(String),
}

/// Parse one rule. Fails closed: anything not certainly expressible becomes
/// [`ProxyRule::Unsupported`] and is dropped from the rendered filter rather
/// than approximated into a broader grant.
///
/// A `host:port` suffix is accepted for ports **80** and **443** only — the
/// two the proxy can actually reach (`ConnectPort 443`/`563` caps CONNECT,
/// measured). Any other port is unsupported: rendering just the host would
/// silently grant more than was asked for.
pub fn parse_proxy_rule(rule: &str) -> ProxyRule {
    let text = rule.trim();
    if text.is_empty() {
        return ProxyRule::Unsupported("empty rule".to_string());
    }
    if text.contains('/') || text.split_whitespace().count() > 1 {
        return ProxyRule::Unsupported(format!(
            "{text:?} is not a hostname — rules are hosts, not URLs"
        ));
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h, Some(port)),
            Err(_) => {
                return ProxyRule::Unsupported(format!("{text:?} has an unparseable port"));
            }
        },
        None => (text, None),
    };
    if let Some(port) = port
        && port != 80
        && port != 443
    {
        return ProxyRule::Unsupported(format!(
            "{text:?} names port {port}; only 80 and 443 are reachable through the proxy"
        ));
    }
    if host.is_empty() {
        return ProxyRule::Unsupported(format!("{text:?} has no host"));
    }
    if let Some(domain) = host.strip_prefix("*.") {
        if domain.is_empty() || domain.contains('*') {
            return ProxyRule::Unsupported(format!("{text:?} is not a usable wildcard"));
        }
        return ProxyRule::Subdomains(domain.to_string());
    }
    if host.contains('*') {
        return ProxyRule::Unsupported(format!(
            "{text:?} — the only wildcard form is a leading \"*.\""
        ));
    }
    ProxyRule::Host(host.to_string())
}

/// Is `host` a strict subdomain of `domain`?
fn is_subdomain_of(host: &str, domain: &str) -> bool {
    host.len() > domain.len() + 1 && host.ends_with(domain) && {
        let boundary = host.len() - domain.len() - 1;
        host.as_bytes()[boundary] == b'.'
    }
}

/// Does `deny` forbid anything `allow` would grant?
///
/// A deny that lands **inside** a wildcard allow returns true, which drops the
/// whole wildcard. tinyproxy's filter is an allow list with no exception form,
/// so a narrower grant cannot be expressed — losing the wildcard is the only
/// way "deny beats allow" can be honoured without leaking the denied host.
fn deny_covers(deny: &ProxyRule, allow: &ProxyRule) -> bool {
    match (deny, allow) {
        (ProxyRule::Host(d), ProxyRule::Host(a)) => d == a,
        (ProxyRule::Host(d), ProxyRule::Subdomains(a)) => is_subdomain_of(d, a),
        (ProxyRule::Subdomains(d), ProxyRule::Host(a)) => is_subdomain_of(a, d),
        (ProxyRule::Subdomains(d), ProxyRule::Subdomains(a)) => a == d || is_subdomain_of(a, d),
        _ => false,
    }
}

/// Render a profile's rules into the file mounted at `/etc/tinyproxy/filter`.
///
/// One fnmatch pattern per line, in the order the allow list gave them, with
/// duplicates removed. An empty result is **deny-all**, which is what an empty
/// `proxy_allow`, an all-unsupported list, and a fully-denied list each
/// correctly produce.
pub fn render_proxy_filter(allow: &[String], deny: &[String]) -> String {
    let denials: Vec<ProxyRule> = deny.iter().map(|r| parse_proxy_rule(r)).collect();
    for rule in &denials {
        if let ProxyRule::Unsupported(why) = rule {
            log::warn!("sandbox egress deny rule ignored: {why}");
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for rule in allow.iter().map(|r| parse_proxy_rule(r)) {
        let line = match &rule {
            ProxyRule::Host(h) => h.clone(),
            ProxyRule::Subdomains(d) => format!("*.{d}"),
            ProxyRule::Unsupported(why) => {
                log::warn!("sandbox egress allow rule ignored: {why}");
                continue;
            }
        };
        if denials.iter().any(|d| deny_covers(d, &rule)) {
            log::warn!("sandbox egress allow rule {line:?} dropped: a deny rule covers it");
            continue;
        }
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("{}\n", lines.join("\n"))
}

/// The filter text for the profile `name`, or deny-all when it has none.
pub fn filter_for_profile(cfg: &SandboxConfig, name: Option<&str>) -> String {
    match name.and_then(|n| cfg.profile.get(n)) {
        Some(p) => render_proxy_filter(&p.proxy_allow, &p.proxy_deny),
        None => String::new(),
    }
}

/// Which profile rule governed a host, if any.
///
/// Deny beats allow, exactly as [`render_proxy_filter`] renders it, so a host
/// covered by both reports the deny.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleMatch {
    /// The `proxy_allow` entry, verbatim, that permits this host.
    Allow(String),
    /// The `proxy_deny` entry, verbatim, that forbids it.
    Deny(String),
    /// Nothing in either list names it — the deny-by-default case.
    None,
}

impl RuleMatch {
    /// The audit record's `rule` field: `"allow:<rule>"`, `"deny:<rule>"` or
    /// `"none"`. One string, so a reader can grep a rule out of the log.
    pub fn label(&self) -> String {
        match self {
            RuleMatch::Allow(r) => format!("allow:{r}"),
            RuleMatch::Deny(r) => format!("deny:{r}"),
            RuleMatch::None => "none".to_string(),
        }
    }
}

/// Does one parsed rule name `host`?
///
/// Mirrors what tinyproxy's filter actually matches (measured 2026-08-30):
/// [`ProxyRule::Host`] is exact, [`ProxyRule::Subdomains`] excludes the apex,
/// and [`ProxyRule::Unsupported`] never reaches the filter so it never matches.
fn rule_names_host(rule: &ProxyRule, host: &str) -> bool {
    match rule {
        ProxyRule::Host(h) => h == host,
        ProxyRule::Subdomains(d) => is_subdomain_of(host, d),
        ProxyRule::Unsupported(_) => false,
    }
}

/// The rule that governed `host`, for the audit record.
///
/// Deny is checked first because [`render_proxy_filter`] drops any allow a
/// deny covers; reporting the allow would name a line that was never written
/// to the filter.
pub fn match_proxy_rule(host: &str, allow: &[String], deny: &[String]) -> RuleMatch {
    for rule in deny {
        if rule_names_host(&parse_proxy_rule(rule), host) {
            return RuleMatch::Deny(rule.trim().to_string());
        }
    }
    for rule in allow {
        if rule_names_host(&parse_proxy_rule(rule), host) {
            return RuleMatch::Allow(rule.trim().to_string());
        }
    }
    RuleMatch::None
}

/// One audited egress request.
///
/// Deliberately **host, port and method only** — never the path or query.
/// Measured 2026-08-30: the proxy logs the full absolute URI
/// (`GET http://example.com/secret?token=abc HTTP/1.1`), so keeping the target
/// would turn `events.jsonl` into a secret sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyAudit {
    /// `GET`, `CONNECT`, … as the client sent it.
    pub method: String,
    /// Destination host, with any userinfo stripped.
    pub host: String,
    /// Destination port, defaulted from the scheme when the URI omits it.
    pub port: u16,
    /// `"allowed"` or `"denied"`.
    pub decision: &'static str,
    /// `"allowed"`, `"filtered"` (host not in the filter) or `"port"`
    /// (CONNECT to a port `ConnectPort` does not permit).
    pub reason: &'static str,
    /// The governing rule, or `RuleMatch::None`.
    pub rule: RuleMatch,
    /// How many identical consecutive requests this record stands for; 1 for
    /// a single request.
    pub repeats: u32,
}

impl ProxyAudit {
    /// The `events.jsonl` payload for this record.
    ///
    /// `proxy_type` is `"forward"` and exists from the first release so that a
    /// later transparent proxy is a new value rather than a schema change
    /// (M19 README, 06 intent).
    pub fn to_event(&self, job_id: &str, session: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "session": session.unwrap_or("-"),
            "job_id": job_id,
            "proxy_type": "forward",
            "method": self.method,
            "host": self.host,
            "port": self.port,
            "decision": self.decision,
            "reason": self.reason,
            "rule": self.rule.label(),
            "repeats": self.repeats,
        })
    }
}

/// Pull `(method, host, port)` out of one tinyproxy `Request` line.
///
/// The two shapes, measured verbatim:
///
/// ```text
/// CONNECT   Aug 31 03:32:20.545 [1]: Request (file descriptor 4): GET http://example.com/ HTTP/1.1
/// CONNECT   Aug 31 03:32:20.590 [1]: Request (file descriptor 4): CONNECT example.com:443 HTTP/1.1
/// ```
fn parse_request_line(line: &str) -> Option<(String, String, u16)> {
    let rest = line.split_once("]: Request (file descriptor ")?.1;
    let rest = rest.split_once("): ")?.1;
    let mut parts = rest.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let (authority, default_port) = if method == "CONNECT" {
        (target, 443u16)
    } else {
        let after_scheme = match target.split_once("://") {
            Some(("https", rest)) => return split_authority(method, rest, 443),
            Some((_, rest)) => rest,
            None => target,
        };
        (after_scheme, 80u16)
    };
    split_authority(method, authority, default_port)
}

/// Split `[user:pw@]host[:port][/path]` into host and port.
fn split_authority(
    method: &str,
    authority: &str,
    default_port: u16,
) -> Option<(String, String, u16)> {
    let authority = authority.split(['/', '?', '#']).next()?;
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h, port),
            Err(_) => (authority, default_port),
        },
        None => (authority, default_port),
    };
    if host.is_empty() {
        return None;
    }
    Some((method.to_string(), host.to_string(), port))
}

/// The decision for a request, read off the line that follows it.
///
/// Measured 2026-08-30 under twelve-way concurrency: a refusal is emitted
/// **immediately** after its own `Request` line, because the filter and port
/// checks are synchronous, while the allow path's lines interleave freely.
/// Both refusal forms are guarded by host or port, so a refusal belonging to a
/// different request cannot be mis-attributed unless it names the same host or
/// port — in which case the decision would have been the same anyway.
fn decision_for(next: Option<&str>, host: &str, port: u16) -> (&'static str, &'static str) {
    let Some(next) = next else {
        return ("allowed", "allowed");
    };
    if next.ends_with(&format!("Proxying refused on filtered domain \"{host}\"")) {
        return ("denied", "filtered");
    }
    if next.ends_with(&format!("Refused CONNECT method on port {port}")) {
        return ("denied", "port");
    }
    ("allowed", "allowed")
}

/// Parse a job proxy's whole log into audit records, collapsing identical
/// consecutive requests into one record with a `repeats` count.
///
/// Lines that are not requests — the boot banner, `Connect (file descriptor
/// …)`, `opensock`, `Closed connection` — produce nothing.
pub fn parse_proxy_log(log: &str) -> Vec<ProxyAudit> {
    let lines: Vec<&str> = log.lines().collect();
    let mut out: Vec<ProxyAudit> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((method, host, port)) = parse_request_line(line) else {
            continue;
        };
        let (decision, reason) = decision_for(lines.get(i + 1).copied(), &host, port);
        if let Some(last) = out.last_mut()
            && last.method == method
            && last.host == host
            && last.port == port
            && last.decision == decision
            && last.reason == reason
        {
            last.repeats += 1;
            continue;
        }
        out.push(ProxyAudit {
            method,
            host,
            port,
            decision,
            reason,
            rule: RuleMatch::None,
            repeats: 1,
        });
    }
    out
}

/// Parse the log and attribute each record to the rule that governed it.
pub fn audit_proxy_log(log: &str, allow: &[String], deny: &[String]) -> Vec<ProxyAudit> {
    let mut records = parse_proxy_log(log);
    for record in &mut records {
        record.rule = match_proxy_rule(&record.host, allow, deny);
    }
    records
}

/// argv reading the job proxy's log. Must run **before** [`remove_proxy`] —
/// a removed container's log is gone with it.
pub fn proxy_logs_args(cfg: &SandboxConfig, job_id: &str) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "logs".to_string(),
        proxy_container_name(job_id),
    ]
}

/// Read the job proxy's log and return its audit records.
///
/// The one spawn site for the audit; a docker failure yields no records and a
/// warning rather than failing the job, because the command has already run.
pub fn collect_proxy_audit(
    cfg: &SandboxConfig,
    job_id: &str,
    allow: &[String],
    deny: &[String],
) -> Vec<ProxyAudit> {
    let out = std::process::Command::new(&cfg.runtime)
        .args(proxy_logs_args(cfg, job_id))
        .output();
    match out {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            audit_proxy_log(&text, allow, deny)
        }
        Err(e) => {
            log::warn!("sandbox egress audit unavailable for {job_id}: {e}");
            Vec::new()
        }
    }
}

/// The `proxy_allow` / `proxy_deny` lists for the profile `name`.
pub fn proxy_rules_for_profile(
    cfg: &SandboxConfig,
    name: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    match name.and_then(|n| cfg.profile.get(n)) {
        Some(p) => (p.proxy_allow.clone(), p.proxy_deny.clone()),
        None => (Vec::new(), Vec::new()),
    }
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
    filter: &str,
) -> Result<(), String> {
    let path = proxy_filter_path(job_id);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Err(format!("sandbox egress filter directory failed: {e}"));
    }
    if let Err(e) = std::fs::write(&path, filter.as_bytes()) {
        return Err(format!("sandbox egress filter write failed: {e}"));
    }
    proxy_step(cfg, "network create", network_create_args(cfg, job_id))?;
    let started = proxy_step(
        cfg,
        "proxy run",
        proxy_run_args(cfg, job_id, &path, is_ghost, session_id),
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

/// argv listing every container this daemon's sandbox created, running or not.
pub fn sweep_container_list_args(cfg: &SandboxConfig) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "ps".to_string(),
        "-aq".to_string(),
        "--filter".to_string(),
        "label=de.sandbox=1".to_string(),
    ]
}

/// argv force-removing the given container ids.
pub fn sweep_container_rm_args(cfg: &SandboxConfig, ids: &[String]) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "rm".to_string(),
        "-f".to_string(),
    ];
    args.extend(ids.iter().cloned());
    args
}

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

/// argv listing every volume name known to the runtime.
pub fn sweep_volume_list_args(cfg: &SandboxConfig) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "volume".to_string(),
        "ls".to_string(),
        "-q".to_string(),
    ]
}

/// argv removing the given volumes.
pub fn sweep_volume_rm_args(cfg: &SandboxConfig, names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "volume".to_string(),
        "rm".to_string(),
    ];
    args.extend(names.iter().cloned());
    args
}

/// The subset of `names` that are sandbox staging volumes.
/// Prefix match only — docker's own name filter is a substring match and
/// would select a user volume that merely contains the string.
pub fn stale_stage_volumes(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| name.starts_with("de-stage-"))
        .cloned()
        .collect()
}

/// Whether a ghost's containers should be destroyed when it exits: the
/// sandbox must be on, and `[sandbox.ghost_defaults] destroy_on_exit` must
/// not have been turned off.
pub fn should_teardown_ghost(cfg: &SandboxConfig) -> bool {
    cfg.enabled && cfg.ghost_defaults.destroy_on_exit
}

/// argv listing the containers one ghost session owns, running or not.
///
/// All three filters are load-bearing and docker ANDs them: `de.sandbox=1`
/// keeps it to this daemon's containers, `de.ghost=1` makes an interactive
/// session's container unmatchable, and `de.session=<id>` is an **exact**
/// value match, so a sibling ghost whose id merely shares a prefix is not
/// selected.
pub fn ghost_teardown_list_args(cfg: &SandboxConfig, session_id: &str) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "ps".to_string(),
        "-aq".to_string(),
        "--filter".to_string(),
        "label=de.sandbox=1".to_string(),
        "--filter".to_string(),
        "label=de.ghost=1".to_string(),
        "--filter".to_string(),
        format!("label=de.session={session_id}"),
    ]
}

/// Remove every container belonging to one ghost session. Blocking — call it
/// off the async runtime. Best-effort: every failure is logged and none is
/// propagated, because this runs on a ghost's exit path.
pub fn teardown_ghost_containers(cfg: &SandboxConfig, session_id: &str) {
    if !should_teardown_ghost(cfg) {
        return;
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(ghost_teardown_list_args(cfg, session_id));
    let listed = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("ghost container teardown list failed for {session_id}: {e}");
            return;
        }
    };
    let ids: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return;
    }
    let count = ids.len();
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_container_rm_args(cfg, &ids));
    match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(_) => log::info!("ghost teardown removed {count} container(s) for {session_id}"),
        Err(e) => log::warn!("ghost container teardown remove failed for {session_id}: {e}"),
    }
}

/// The `--format` template `status_inspect_args` uses. Each container becomes
/// one line: `<id> <state> <image> <labels-as-json>`. The labels are JSON, not
/// a comma-joined string, because a `de.session` value carries a
/// webhook-supplied alert name — `docker ps`'s own `{{.Labels}}` joins pairs
/// with `,` and is irrecoverably ambiguous once a value contains one.
pub const STATUS_INSPECT_FORMAT: &str =
    "{{.Id}} {{.State.Status}} {{.Config.Image}} {{json .Config.Labels}}";

/// argv inspecting the given containers. **Empty when `ids` is empty** —
/// `docker inspect` with no arguments is a usage error (exit 1), and the empty
/// case is the common one.
pub fn status_inspect_args(cfg: &SandboxConfig, ids: &[String]) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "inspect".to_string(),
        "--format".to_string(),
        STATUS_INSPECT_FORMAT.to_string(),
    ];
    args.extend(ids.iter().cloned());
    args
}

/// Parse `status_inspect_args`' output into one record per container.
///
/// Pure. Splits each line into exactly four fields — the last is the whole
/// remaining text, so a label value containing a space, comma or `=` cannot
/// shift the parse. A line whose JSON does not decode is skipped rather than
/// guessed at.
pub fn parse_container_records(text: &str) -> Vec<crate::ipc::ContainerInfo> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim_end().splitn(4, ' ');
            let id = parts.next()?;
            let state = parts.next()?;
            let image = parts.next()?;
            let labels: std::collections::HashMap<String, String> =
                serde_json::from_str(parts.next()?).ok()?;
            Some(crate::ipc::ContainerInfo {
                id: id.chars().take(12).collect(),
                state: state.to_string(),
                image: image.to_string(),
                session: labels.get("de.session").cloned(),
                is_ghost: labels.contains_key("de.ghost"),
            })
        })
        .collect()
}

/// Runtime and image health plus every sandbox container this daemon owns.
/// Blocking — call it off the async runtime. Never fails: an unreachable
/// runtime is reported, not raised.
pub fn collect_container_status(cfg: &SandboxConfig) -> crate::ipc::ContainerStatusReport {
    let (runtime_ok, runtime_detail) = match probe_runtime(cfg) {
        Ok(version) => (true, version),
        Err(reason) => (
            false,
            describe_unavailable(&SandboxUnavailable::Runtime(reason)),
        ),
    };
    let image_detail = match read_lock() {
        None => format!("no lockfile at {}", lock_path().display()),
        Some(lock) => match check_image_matches(&lock, &probe_live_image_id(cfg)) {
            ImageCheck::Match => format!("{} ({})", cfg.image, lock.image_id),
            other => format!("{other:?}"),
        },
    };
    if !runtime_ok {
        return crate::ipc::ContainerStatusReport {
            enabled: cfg.enabled,
            runtime_ok,
            runtime_detail,
            image_detail,
            containers: Vec::new(),
        };
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_container_list_args(cfg));
    let ids: Vec<String> = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Err(e) => {
            log::warn!("container status list failed: {e}");
            Vec::new()
        }
    };
    let inspect = status_inspect_args(cfg, &ids);
    let containers = if inspect.is_empty() {
        Vec::new()
    } else {
        let mut cmd = Command::new(&cfg.runtime);
        cmd.args(inspect);
        match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
            Ok(out) => parse_container_records(&String::from_utf8_lossy(&out.stdout)),
            Err(e) => {
                log::warn!("container status inspect failed: {e}");
                Vec::new()
            }
        }
    };
    crate::ipc::ContainerStatusReport {
        enabled: cfg.enabled,
        runtime_ok,
        runtime_detail,
        image_detail,
        containers,
    }
}

/// Remove orphaned sandbox containers and staging volumes. Best-effort:
/// every failure is logged and none is fatal — a sweep that cannot run must
/// never stop the daemon from starting.
pub fn sweep_sandbox_leftovers(cfg: &SandboxConfig) {
    if !cfg.enabled {
        return;
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_container_list_args(cfg));
    let listed = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("sandbox container sweep list failed: {e}");
            return;
        }
    };
    let container_ids: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let removed_containers = if container_ids.is_empty() {
        0
    } else {
        cmd = Command::new(&cfg.runtime);
        cmd.args(sweep_container_rm_args(cfg, &container_ids));
        match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
            Ok(_) => container_ids.len(),
            Err(e) => {
                log::warn!("sandbox container sweep remove failed: {e}");
                container_ids.len()
            }
        }
    };

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

    cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_volume_list_args(cfg));
    let listed_volumes = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("sandbox volume sweep list failed: {e}");
            return;
        }
    };
    let volume_names: Vec<String> = String::from_utf8_lossy(&listed_volumes.stdout)
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let stale = stale_stage_volumes(&volume_names);
    let removed_volumes = if stale.is_empty() {
        0
    } else {
        cmd = Command::new(&cfg.runtime);
        cmd.args(sweep_volume_rm_args(cfg, &stale));
        match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
            Ok(_) => stale.len(),
            Err(e) => {
                log::warn!("sandbox volume sweep remove failed: {e}");
                stale.len()
            }
        }
    };

    log::info!(
        "sandbox sweep removed {} orphaned container(s), {} egress network(s) and {} stale staging volume(s)",
        removed_containers,
        removed_networks,
        removed_volumes
    );
}

/// One sandboxed job's identity and payload.
pub struct ExecSpec<'a> {
    pub job_id: &'a str,
    pub network: &'a str,
    pub is_ghost: bool,
    pub command: &'a str,
}

/// argv for the sandboxed run. Pure — the caller prepends the runtime binary
/// and spawns it.
///
/// `session_id` becomes a `de.session=<id>` label, which is what lets a
/// ghost's own containers be reclaimed on exit without touching a sibling
/// ghost's or an interactive session's. `None` emits no such label.
pub fn run_args(cfg: &SandboxConfig, spec: &ExecSpec, session_id: Option<&str>) -> Vec<String> {
    let Some((uid, gid)) = split_run_as(&cfg.run_as) else {
        log::warn!(
            "sandbox enabled but run_as={:?} unparseable; falling back to running on the host",
            cfg.run_as
        );
        return Vec::new();
    };
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "run".to_string(),
        "--rm".to_string(),
        "--user".to_string(),
        cfg.run_as.clone(),
        "--network".to_string(),
        spec.network.to_string(),
        "--memory".to_string(),
        cfg.limits.memory.clone(),
        // Docker defaults --memory-swap to 2x --memory, so the documented 1g
        // cap actually permits 2 GiB. Equal values disable swap entirely.
        "--memory-swap".to_string(),
        cfg.limits.memory.clone(),
        "--pids-limit".to_string(),
        cfg.limits.pids.to_string(),
        "--cpus".to_string(),
        cfg.limits.cpus.to_string(),
        // The image's filesystem is not a scratch space: /de/work and /tmp are
        // the only writable paths, both tmpfs, both owned by the run_as uid.
        "--read-only".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        // The process is already uid 1000, but Alpine ships setuid busybox
        // links; this is what closes that escalation path.
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        // sandbox_preflight caches its verdict for the daemon's lifetime, so an
        // image deleted after startup would otherwise be resolved against a
        // registry. Fail locally instead.
        "--pull".to_string(),
        "never".to_string(),
        "--tmpfs".to_string(),
        format!(
            "{}:rw,size={},mode=0700,uid={},gid={}",
            cfg.workdir, cfg.limits.scratch, uid, gid
        ),
        "--tmpfs".to_string(),
        format!(
            "/tmp:rw,size={},mode=1777,uid={uid},gid={gid}",
            cfg.limits.scratch
        ),
        "-v".to_string(),
        format!("{}:/de/scripts:ro", stage_volume_name(spec.job_id)),
    ];
    args.push("--label".to_string());
    args.push("de.sandbox=1".to_string());
    if spec.is_ghost {
        args.push("--label".to_string());
        args.push("de.ghost=1".to_string());
    }
    if let Some(sid) = session_id {
        args.push("--label".to_string());
        args.push(format!("de.session={sid}"));
    }
    if spec.network != "none" {
        args.extend(proxy_env_args(spec.job_id));
    }
    args.push("--workdir".to_string());
    args.push(cfg.workdir.clone());
    args.push(cfg.image.clone());
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(spec.command.to_string());
    args
}

/// The `container_run` event payload for a sandboxed job, or `None` when the
/// sandbox is off and no container will exist.
///
/// The image id comes from the **lockfile**, not from a fresh `image inspect`:
/// [`sandbox_preflight`] refuses to run anything when the live image differs
/// from the lock, so whenever a job reaches this point the two agree, and the
/// lock is readable without spawning a process.
///
/// This is the audit anchor a live check binds to — a `docker ps` snapshot
/// races the `--rm` teardown, a record does not.
pub fn container_run_event(
    cfg: &SandboxConfig,
    lock: Option<&SandboxLock>,
    job_id: &str,
    job_name: &str,
    network: &str,
    session: Option<&str>,
) -> Option<serde_json::Value> {
    if !cfg.enabled {
        return None;
    }
    Some(serde_json::json!({
        "session": session.unwrap_or("-"),
        "job_id": job_id,
        "job_name": job_name,
        "image": cfg.image,
        "image_id": lock.map_or("unknown", |l| l.image_id.as_str()),
        "network": network,
    }))
}

/// The command string a `de-bg-*` window should run for `raw_cmd`.
///
/// With the sandbox disabled this is `raw_cmd` unchanged. With it enabled the
/// result is a fully shell-quoted `docker run …` line that carries `raw_cmd`
/// as a single literal argument to the container's shell, so nothing in it is
/// interpreted by the host shell.
pub fn sandbox_window_command(
    cfg: &SandboxConfig,
    spec: &ExecSpec,
    raw_cmd: &str,
    session_id: Option<&str>,
) -> String {
    if !cfg.enabled {
        return raw_cmd.to_string();
    }
    let run = run_args(cfg, spec, session_id);
    if run.is_empty() {
        return raw_cmd.to_string();
    }
    std::iter::once(cfg.runtime.clone())
        .chain(run)
        .map(|arg| crate::daemon::utils::sh_single_quote(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SandboxLimits;

    const UID_MAP: &str = "         0       1000          1\n         1     100000      65536\n";

    #[test]
    fn sandbox_runtime_parses_the_real_uid_map() {
        let ranges = parse_uid_map(UID_MAP).expect("real map parses");
        assert_eq!(
            ranges
                .iter()
                .map(|r| (r.container_start, r.host_start, r.length))
                .collect::<Vec<_>>(),
            vec![(0, 1000, 1), (1, 100000, 65536)]
        );
    }

    #[test]
    fn sandbox_runtime_uid_map_rejects_malformed_lines() {
        assert_eq!(parse_uid_map("0 1000"), None);
        assert_eq!(parse_uid_map("0 1000 1 2"), None);
        assert_eq!(parse_uid_map("a b c"), None);
        let with_blank = format!("{}\n", UID_MAP);
        assert!(parse_uid_map(&with_blank).is_some());
        let blank_between =
            "         0       1000          1\n\n         1     100000      65536\n";
        assert!(parse_uid_map(blank_between).is_some());
    }

    #[test]
    fn sandbox_runtime_translates_container_uids_to_host_uids() {
        let ranges = parse_uid_map(UID_MAP).unwrap();
        assert_eq!(host_uid_for(0, &ranges), Some(1000));
        assert_eq!(host_uid_for(1, &ranges), Some(100000));
        assert_eq!(host_uid_for(1000, &ranges), Some(100999));
        assert_eq!(host_uid_for(65536, &ranges), Some(165535));
    }

    #[test]
    fn sandbox_runtime_translation_rejects_uids_outside_every_range() {
        let ranges = parse_uid_map(UID_MAP).unwrap();
        assert_eq!(host_uid_for(65537, &ranges), None);
        assert_eq!(host_uid_for(70000, &ranges), None);
    }

    #[test]
    fn sandbox_runtime_gate_passes_for_container_uid_1000() {
        assert_eq!(
            evaluate_uid_gate(1000, UID_MAP),
            UidGateOutcome::Ok {
                container_uid: 1000,
                host_uid: 100999
            }
        );
    }

    #[test]
    fn sandbox_runtime_gate_rejects_container_root() {
        assert_eq!(
            evaluate_uid_gate(0, UID_MAP),
            UidGateOutcome::ContainerRoot { host_uid: 1000 }
        );
    }

    #[test]
    fn sandbox_runtime_gate_reports_unmapped_uid() {
        assert_eq!(
            evaluate_uid_gate(70000, UID_MAP),
            UidGateOutcome::Unmapped {
                container_uid: 70000
            }
        );
    }

    #[test]
    fn sandbox_runtime_gate_reports_malformed_map() {
        assert_eq!(
            evaluate_uid_gate(1000, "garbage"),
            UidGateOutcome::MalformedMap
        );
        assert_eq!(
            evaluate_uid_gate(0, "garbage"),
            UidGateOutcome::MalformedMap
        );
    }

    #[test]
    fn sandbox_runtime_version_probe_classifies_healthy() {
        assert_eq!(
            classify_version_probe(
                "docker",
                "unix:///run/user/1000/docker.sock",
                None,
                true,
                "29.7.2\n",
                ""
            ),
            Ok("29.7.2".to_string())
        );
    }

    #[test]
    fn sandbox_runtime_version_probe_distinguishes_missing_binary_from_dead_daemon() {
        assert_eq!(
            classify_version_probe(
                "docker",
                "unix:///run/user/1000/docker.sock",
                Some(std::io::ErrorKind::NotFound),
                false,
                "",
                ""
            ),
            Err(RuntimeUnavailable::NotInstalled {
                runtime: "docker".to_string()
            })
        );
        let stderr = "failed to connect to the docker API at unix:///nonexistent/docker.sock; check if the path is correct and if the daemon is running: dial unix /nonexistent/docker.sock: connect: no such file or directory";
        assert_eq!(
            classify_version_probe(
                "docker",
                "unix:///nonexistent/docker.sock",
                None,
                false,
                "",
                stderr
            ),
            Err(RuntimeUnavailable::DaemonUnreachable {
                docker_host: "unix:///nonexistent/docker.sock".to_string(),
                stderr: stderr.trim().to_string()
            })
        );
        assert_eq!(
            classify_version_probe(
                "docker",
                "unix:///run/user/1000/docker.sock",
                None,
                true,
                "  \n",
                ""
            ),
            Err(RuntimeUnavailable::DaemonUnreachable {
                docker_host: "unix:///run/user/1000/docker.sock".to_string(),
                stderr: "".to_string()
            })
        );
    }

    #[test]
    #[ignore = "requires a running rootless Docker daemon"]
    fn sandbox_runtime_probe_reaches_a_real_docker() {
        let result = probe_runtime(&SandboxConfig::default());
        assert!(result.is_ok(), "probe failed: {:?}", result);
    }

    #[test]
    fn sandbox_lock_accepts_a_well_formed_image_id() {
        let fixture = format!("sha256:{}", "a".repeat(64));
        assert!(is_valid_image_id(&fixture));
    }

    #[test]
    fn sandbox_lock_rejects_malformed_image_ids() {
        let hex64 = "a".repeat(64);
        let cases = [
            hex64.clone(),
            format!("md5:{}", hex64),
            format!("sha256:{}", "a".repeat(63)),
            format!("sha256:{}", "a".repeat(65)),
            format!("sha256:{}", "A".repeat(64)),
            String::new(),
        ];
        for case in &cases {
            assert!(!is_valid_image_id(case), "accepted: {case:?}");
        }
    }

    #[test]
    fn sandbox_lock_render_parse_round_trip() {
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: format!("sha256:{}", "b".repeat(64)),
            built_at: 1_787_900_000,
        };
        let parsed = parse_lock(&render_lock(&lock)).expect("round trip parses");
        assert_eq!(parsed, lock);
    }

    #[test]
    fn sandbox_lock_parse_tolerates_whitespace_and_blank_lines() {
        let text = format!(
            "\n\n  image   =  daemoneye-agent-base  \n\n  image_id = sha256:{}\nbuilt_at   =  1787900000\n\n",
            "c".repeat(64)
        );
        let parsed = parse_lock(&text).expect("whitespace-tolerant parse");
        assert_eq!(parsed.image, "daemoneye-agent-base");
        assert_eq!(parsed.image_id, format!("sha256:{}", "c".repeat(64)));
        assert_eq!(parsed.built_at, 1_787_900_000);
    }

    #[test]
    fn sandbox_lock_parse_rejects_bad_records() {
        let id = format!("sha256:{}", "d".repeat(64));
        let valid =
            format!("image = daemoneye-agent-base\nimage_id = {id}\nbuilt_at = 1787900000\n");
        assert!(parse_lock(&valid).is_some());

        let missing_image = format!("image_id = {id}\nbuilt_at = 1787900000\n");
        let missing_built_at = format!("image = daemoneye-agent-base\nimage_id = {id}\n");
        let duplicated_key = format!(
            "image = daemoneye-agent-base\nimage = agent\nimage_id = {id}\nbuilt_at = 1787900000\n"
        );
        let unknown_key = format!(
            "image = daemoneye-agent-base\nimage_id = {id}\nbuilt_at = 1787900000\nrevision = 3\n"
        );
        let non_numeric_built_at =
            format!("image = daemoneye-agent-base\nimage_id = {id}\nbuilt_at = soon\n");
        let malformed_image_id = format!(
            "image = daemoneye-agent-base\nimage_id = sha256:{}\nbuilt_at = 1787900000\n",
            "z".repeat(64)
        );
        for text in [
            &missing_image,
            &missing_built_at,
            &duplicated_key,
            &unknown_key,
            &non_numeric_built_at,
            &malformed_image_id,
        ] {
            assert!(parse_lock(text).is_none(), "accepted: {text}");
        }
    }

    #[test]
    fn sandbox_lock_check_reports_match() {
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: format!("sha256:{}", "f".repeat(64)),
            built_at: 1_787_900_000,
        };
        assert_eq!(
            check_image_matches(&lock, &lock.image_id),
            ImageCheck::Match
        );
    }

    #[test]
    fn sandbox_lock_check_reports_mismatch() {
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: format!("sha256:{}", "d".repeat(64)),
            built_at: 1_787_900_000,
        };
        let live = format!("sha256:{}", "e".repeat(64));
        let check = check_image_matches(&lock, &live);
        assert_eq!(
            check,
            ImageCheck::Mismatch {
                locked: lock.image_id.clone(),
                live: live.clone(),
            }
        );
    }

    #[test]
    fn sandbox_lock_check_reports_malformed_live_before_mismatch() {
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: format!("sha256:{}", "i".repeat(64)),
            built_at: 1_787_900_000,
        };
        let check = check_image_matches(&lock, "garbage");
        assert_ne!(
            check,
            ImageCheck::Mismatch {
                locked: "".into(),
                live: "".into()
            }
        );
        match check {
            ImageCheck::MalformedLive { live } => assert_eq!(live, "garbage"),
            other => panic!("expected MalformedLive, got {other:?}"),
        }
    }

    #[test]
    fn sandbox_exec_splits_a_valid_run_as() {
        assert_eq!(split_run_as("1000:1000"), Some((1000, 1000)));
        assert_eq!(split_run_as("10:0"), Some((10, 0)));
        assert_eq!(split_run_as("0:0"), Some((0, 0)));
    }

    #[test]
    fn sandbox_exec_rejects_malformed_run_as() {
        for input in ["1000", ":1000", "1000:", "a:b", "1000:1000:1000", ""] {
            assert_eq!(split_run_as(input), None, "accepted: {input:?}");
        }
    }

    #[test]
    fn sandbox_exec_run_args_match_the_prototyped_vector() {
        let cfg = SandboxConfig::default();
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: "echo hi",
        };
        assert_eq!(
            run_args(&cfg, &spec, None),
            vec![
                "--host",
                "unix:///run/user/1000/docker.sock",
                "run",
                "--rm",
                "--user",
                "1000:1000",
                "--network",
                "none",
                "--memory",
                "1g",
                "--memory-swap",
                "1g",
                "--pids-limit",
                "256",
                "--cpus",
                "2",
                "--read-only",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--pull",
                "never",
                "--tmpfs",
                "/de/work:rw,size=2g,mode=0700,uid=1000,gid=1000",
                "--tmpfs",
                "/tmp:rw,size=2g,mode=1777,uid=1000,gid=1000",
                "-v",
                "de-stage-j1:/de/scripts:ro",
                "--label",
                "de.sandbox=1",
                "--workdir",
                "/de/work",
                "daemoneye-agent-base",
                "sh",
                "-lc",
                "echo hi"
            ]
        );
    }

    #[test]
    fn sandbox_gc_stage_args_carry_the_sandbox_label() {
        let cfg = SandboxConfig::default();
        let args = stage_args(&cfg, "j1", "myscript.sh");
        assert!(
            args.iter().any(|arg| arg == "de.sandbox=1"),
            "stage args lack de.sandbox=1: {args:?}"
        );
    }

    #[test]
    fn sandbox_gc_every_container_carries_the_sandbox_label() {
        let cfg = SandboxConfig::default();
        let plain = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        assert!(
            plain.iter().any(|arg| arg == "de.sandbox=1"),
            "non-ghost vector lacks de.sandbox=1: {plain:?}"
        );
        assert!(
            !plain.iter().any(|arg| arg == "de.ghost=1"),
            "non-ghost vector carries ghost label: {plain:?}"
        );
        let ghost = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: true,
                command: "echo hi",
            },
            None,
        );
        assert!(
            ghost.iter().any(|arg| arg == "de.sandbox=1"),
            "ghost vector lacks de.sandbox=1: {ghost:?}"
        );
        assert!(
            ghost.iter().any(|arg| arg == "de.ghost=1"),
            "ghost vector lacks de.ghost=1: {ghost:?}"
        );
    }

    #[test]
    fn sandbox_gc_selects_only_stage_prefixed_volumes() {
        let names: Vec<String> = [
            "de-stage-a",
            "zz-de-stage-decoy",
            "de-stage-b",
            "unrelated",
            "de-stagex",
            "de-stage-",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            stale_stage_volumes(&names),
            ["de-stage-a", "de-stage-b", "de-stage-"]
        );
    }

    #[test]
    fn sandbox_gc_selects_nothing_from_an_empty_list() {
        assert!(stale_stage_volumes(&[]).is_empty());
    }

    #[test]
    fn sandbox_gc_container_list_args_filter_by_label() {
        let cfg = SandboxConfig::default();
        assert_eq!(
            sweep_container_list_args(&cfg),
            [
                "--host",
                "unix:///run/user/1000/docker.sock",
                "ps",
                "-aq",
                "--filter",
                "label=de.sandbox=1"
            ]
        );
    }

    #[test]
    fn sandbox_gc_volume_list_args_do_not_filter() {
        let cfg = SandboxConfig::default();
        let args = sweep_volume_list_args(&cfg);
        assert_eq!(
            args,
            [
                "--host",
                "unix:///run/user/1000/docker.sock",
                "volume",
                "ls",
                "-q"
            ]
        );
        assert!(
            !args.iter().any(|a| a.starts_with("--filter")),
            "volume selection must happen in Rust, not docker's filter: {args:?}"
        );
    }

    #[test]
    fn sandbox_gc_rm_args_are_empty_for_an_empty_slice() {
        let cfg = SandboxConfig::default();
        assert!(sweep_container_rm_args(&cfg, &[]).is_empty());
        assert!(sweep_volume_rm_args(&cfg, &[]).is_empty());
        assert_eq!(
            sweep_container_rm_args(&cfg, &["abc".to_string()]),
            [
                "--host",
                "unix:///run/user/1000/docker.sock",
                "rm",
                "-f",
                "abc"
            ]
        );
        assert_eq!(
            sweep_volume_rm_args(&cfg, &["de-stage-1".to_string()]),
            [
                "--host",
                "unix:///run/user/1000/docker.sock",
                "volume",
                "rm",
                "de-stage-1"
            ]
        );
    }

    #[test]
    fn sandbox_exec_run_args_label_ghost_jobs() {
        let cfg = SandboxConfig::default();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: true,
                command: "echo hi",
            },
            None,
        );
        let position = args
            .windows(2)
            .position(|pair| pair == ["--label", "de.ghost=1"]);
        assert!(
            position.is_some(),
            "ghost vector lacks --label de.ghost=1: {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "de.sandbox=1"),
            "ghost vector lacks de.sandbox=1: {args:?}"
        );
        let plain = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        assert!(
            !plain.iter().any(|arg| arg == "de.ghost=1"),
            "non-ghost vector carries ghost label: {plain:?}"
        );
        assert!(
            plain.iter().any(|arg| arg == "de.sandbox=1"),
            "non-ghost vector lacks de.sandbox=1: {plain:?}"
        );
    }

    #[test]
    fn sandbox_exec_run_args_derive_tmpfs_ids_from_run_as() {
        let cfg = SandboxConfig {
            run_as: "10:0".to_string(),
            ..SandboxConfig::default()
        };
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let tmpfs = args
            .iter()
            .position(|arg| arg == "--tmpfs")
            .map(|i| &args[i + 1])
            .expect("tmpfs value present");
        assert!(tmpfs.ends_with("mode=0700,uid=10,gid=0"), "got {tmpfs}");
        let user = args
            .iter()
            .position(|arg| arg == "--user")
            .map(|i| &args[i + 1])
            .expect("user value present");
        assert_eq!(user, "10:0");
    }

    #[test]
    fn sandbox_exec_run_args_honour_limits_and_workdir() {
        let cfg = SandboxConfig {
            limits: SandboxLimits {
                memory: "4g".to_string(),
                pids: 64,
                cpus: 1.5,
                scratch: "8g".to_string(),
            },
            workdir: "/scratch".to_string(),
            ..SandboxConfig::default()
        };
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        assert!(args.iter().any(|a| a == "--memory") && args.iter().any(|a| a == "4g"));
        assert!(args.iter().any(|a| a == "--pids-limit") && args.iter().any(|a| a == "64"));
        assert!(args.iter().any(|a| a == "--cpus") && args.iter().any(|a| a == "1.5"));
        let tmpfs = args
            .iter()
            .position(|arg| arg == "--tmpfs")
            .map(|i| &args[i + 1])
            .expect("tmpfs value present");
        assert!(tmpfs.starts_with("/scratch:rw,size=8g,"), "got {tmpfs}");
    }

    #[test]
    fn sandbox_exec_run_args_are_empty_for_bad_run_as() {
        let cfg = SandboxConfig {
            run_as: "nope".to_string(),
            ..SandboxConfig::default()
        };
        assert!(
            run_args(
                &cfg,
                &ExecSpec {
                    job_id: "j1",
                    network: "none",
                    is_ghost: false,
                    command: "echo hi",
                },
                None,
            )
            .is_empty()
        );
    }

    #[test]
    fn sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid() {
        let cfg = SandboxConfig::default();
        let args = stage_args(&cfg, "j1", "myscript.sh");
        let src_mount = format!("{}:/de/src:ro", crate::config::scripts_dir().display());
        assert_eq!(
            &args[..12],
            &[
                "--host",
                "unix:///run/user/1000/docker.sock",
                "run",
                "--rm",
                "--user",
                "0:0",
                "-v",
                src_mount.as_str(),
                "-v",
                "de-stage-j1:/stage",
                "--label",
                "de.sandbox=1"
            ]
        );
        let shell = args.last().expect("shell line present");
        assert!(shell.contains("chmod 0500 /stage/myscript.sh"));
        assert!(shell.contains("chown 1000:1000 /stage/myscript.sh"));
    }

    #[test]
    fn sandbox_exec_stage_args_reject_unsafe_script_names() {
        let cfg = SandboxConfig::default();
        for name in [
            "../etc/passwd",
            "a/b",
            "a b",
            "a;rm -rf /",
            "a$(id)",
            "a|b",
            "a\nb",
        ] {
            assert!(
                stage_args(&cfg, "j1", name).is_empty(),
                "accepted: {name:?}"
            );
        }
    }

    #[test]
    fn sandbox_exec_preflight_passes_when_everything_is_healthy() {
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: format!("sha256:{}", "f".repeat(64)),
            built_at: 1_787_900_000,
        };
        let version = Ok("Docker version 26.1.4".to_string());
        assert_eq!(
            evaluate_preflight(
                "1000:1000",
                &version,
                &UidGateOutcome::Ok {
                    container_uid: 1000,
                    host_uid: 100999
                },
                Some(&lock),
                &lock.image_id,
            ),
            Ok(())
        );
    }

    #[test]
    fn sandbox_exec_preflight_reports_each_failure() {
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: format!("sha256:{}", "f".repeat(64)),
            built_at: 1_787_900_000,
        };
        let healthy_version = Ok("Docker version 26.1.4".to_string());
        let healthy_gate = UidGateOutcome::Ok {
            container_uid: 1000,
            host_uid: 100999,
        };

        let bad_run_as = evaluate_preflight(
            "nope",
            &healthy_version,
            &healthy_gate,
            Some(&lock),
            &lock.image_id,
        );
        match bad_run_as {
            Err(SandboxUnavailable::BadRunAs { run_as }) => assert_eq!(run_as, "nope"),
            other => panic!("expected BadRunAs, got {other:?}"),
        }

        let runtime = evaluate_preflight(
            "1000:1000",
            &Err(RuntimeUnavailable::NotInstalled {
                runtime: "docker".to_string(),
            }),
            &healthy_gate,
            Some(&lock),
            &lock.image_id,
        );
        match runtime {
            Err(SandboxUnavailable::Runtime(_)) => {}
            other => panic!("expected Runtime, got {other:?}"),
        }

        let gate = evaluate_preflight(
            "1000:1000",
            &healthy_version,
            &UidGateOutcome::ContainerRoot { host_uid: 0 },
            Some(&lock),
            &lock.image_id,
        );
        match gate {
            Err(SandboxUnavailable::UidGate(UidGateOutcome::ContainerRoot { host_uid: 0 })) => {}
            other => panic!("expected UidGate, got {other:?}"),
        }

        let no_lock = evaluate_preflight(
            "1000:1000",
            &healthy_version,
            &healthy_gate,
            None,
            &lock.image_id,
        );
        match no_lock {
            Err(SandboxUnavailable::NoLock) => {}
            other => panic!("expected NoLock, got {other:?}"),
        }

        let mismatched_live = format!("sha256:{}", "d".repeat(64));
        let image_failure = evaluate_preflight(
            "1000:1000",
            &healthy_version,
            &healthy_gate,
            Some(&lock),
            &mismatched_live,
        );
        match image_failure {
            Err(SandboxUnavailable::Image(ImageCheck::Mismatch { locked, live })) => {
                assert_eq!(locked, lock.image_id);
                assert_eq!(live, mismatched_live);
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn sandbox_exec_preflight_reports_the_most_fundamental_failure_first() {
        let version = Ok("Docker version 26.1.4".to_string());
        let all_bad_run_as = evaluate_preflight(
            "nope",
            &Err(RuntimeUnavailable::NotInstalled {
                runtime: "docker".to_string(),
            }),
            &UidGateOutcome::ContainerRoot { host_uid: 0 },
            None,
            "not-an-image-id",
        );
        match all_bad_run_as {
            Err(SandboxUnavailable::BadRunAs { .. }) => {}
            other => panic!("expected BadRunAs, got {other:?}"),
        }

        let runtime = evaluate_preflight(
            "1000:1000",
            &Err(RuntimeUnavailable::NotInstalled {
                runtime: "docker".to_string(),
            }),
            &UidGateOutcome::ContainerRoot { host_uid: 0 },
            None,
            "not-an-image-id",
        );
        match runtime {
            Err(SandboxUnavailable::Runtime(_)) => {}
            other => panic!("expected Runtime, got {other:?}"),
        }

        let gate = evaluate_preflight(
            "1000:1000",
            &version,
            &UidGateOutcome::ContainerRoot { host_uid: 0 },
            None,
            "not-an-image-id",
        );
        match gate {
            Err(SandboxUnavailable::UidGate(_)) => {}
            other => panic!("expected UidGate, got {other:?}"),
        }

        let no_lock = evaluate_preflight(
            "1000:1000",
            &version,
            &UidGateOutcome::Ok {
                container_uid: 1000,
                host_uid: 100999,
            },
            None,
            "not-an-image-id",
        );
        match no_lock {
            Err(SandboxUnavailable::NoLock) => {}
            other => panic!("expected NoLock, got {other:?}"),
        }
    }

    // ── sandbox_window_command ─────────────────────────────────────────────
    //
    // The six non-ignored tests below pin the pure wrapper's behaviour; the
    // seventh is the milestone's second live ignored test.

    #[test]
    fn sandbox_window_disabled_returns_the_command_unchanged() {
        let cfg = SandboxConfig::default();
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: "echo hi",
        };
        let result = sandbox_window_command(&cfg, &spec, "echo hi", None);
        assert_eq!(result, "echo hi");
        assert_eq!(result.as_str(), "echo hi");
    }

    #[test]
    fn sandbox_window_enabled_starts_with_the_quoted_runtime() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: "echo hi",
        };
        let result = sandbox_window_command(&cfg, &spec, "echo hi", None);
        assert!(
            result
                .starts_with("'docker' '--host' 'unix:///run/user/1000/docker.sock' 'run' '--rm'"),
            "got: {result}"
        );
    }

    #[test]
    fn sandbox_window_keeps_a_hostile_command_in_one_token() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let raw = "echo inside-container; touch /tmp/PWNED";
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: raw,
        };
        let result = sandbox_window_command(&cfg, &spec, raw, None);
        assert!(
            result.ends_with("'echo inside-container; touch /tmp/PWNED'"),
            "got: {result}"
        );
    }

    #[test]
    fn sandbox_window_quotes_embedded_single_quotes() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let raw = "echo 'a'";
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: raw,
        };
        let result = sandbox_window_command(&cfg, &spec, raw, None);
        // The embedded `'` becomes `'\''` (close-quote, escaped quote, reopen)
        // — the sh_single_quote rendering — never a naive close-open `''` pair.
        let expected_tail = r"'echo '\''a'\'''";
        assert!(result.ends_with(expected_tail), "got: {result}");
        assert_eq!(
            result.matches('\'').count(),
            result.len() - result.replace('\'', "").len()
        );
    }

    #[test]
    fn sandbox_window_carries_the_job_id_into_the_volume_mount() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let spec = ExecSpec {
            job_id: "42-1712937600",
            network: "none",
            is_ghost: false,
            command: "echo hi",
        };
        let result = sandbox_window_command(&cfg, &spec, "echo hi", None);
        assert!(
            result.contains("'de-stage-42-1712937600:/de/scripts:ro'"),
            "got: {result}"
        );
    }

    #[test]
    fn sandbox_window_falls_back_when_run_as_is_unparseable() {
        let cfg = SandboxConfig {
            enabled: true,
            run_as: "nope".to_string(),
            ..SandboxConfig::default()
        };
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: "echo hi",
        };
        let result = sandbox_window_command(&cfg, &spec, "echo hi", None);
        assert_eq!(result, "echo hi");
    }

    #[test]
    #[ignore = "requires a running rootless Docker daemon"]
    fn sandbox_window_command_line_runs_in_a_real_container() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let job_id = format!("e2e-{}", std::process::id());
        let spec = ExecSpec {
            job_id: &job_id,
            network: "none",
            is_ghost: false,
            command: "echo sandbox-ok",
        };
        let line = sandbox_window_command(&cfg, &spec, "echo sandbox-ok", None);
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&line)
            .output()
            .expect("spawning sh failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("sandbox-ok"),
            "container output lacked 'sandbox-ok': {stdout:?} stderr={:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn sandbox_gate_parses_the_real_probe_output() {
        let probe =
            "1000\n---\n         0       1000          1\n         1     100000      65536\n";
        let (uid, map) = parse_probe_output(probe).expect("real probe output must parse");
        assert_eq!(uid, 1000);
        let ranges = parse_uid_map(&map).expect("map must parse");
        assert_eq!(
            ranges,
            vec![
                UidRange {
                    container_start: 0,
                    host_start: 1000,
                    length: 1
                },
                UidRange {
                    container_start: 1,
                    host_start: 100000,
                    length: 65536
                }
            ]
        );
    }

    #[test]
    fn sandbox_gate_probe_output_rejects_malformed_input() {
        let cases: &[&str] = &[
            "1000\n     90000\n",            // no sentinel
            "abc\n---\n     0   1000   1\n", // non-numeric uid
            "",                              // empty
            "---\n     0   1000   1\n",      // sentinel, empty uid line
        ];
        for case in cases {
            assert_eq!(parse_probe_output(case), None, "input: {case:?}");
        }
    }

    #[test]
    fn sandbox_gate_probe_output_feeds_the_uid_gate() {
        let stdout =
            "1000\n---\n         0       1000          1\n         1     100000      65536\n";
        let (uid, map) = parse_probe_output(stdout).expect("real probe output must parse");
        let outcome = evaluate_uid_gate(uid, &map);
        assert_eq!(
            outcome,
            UidGateOutcome::Ok {
                container_uid: 1000,
                host_uid: 100999
            }
        );
    }

    #[test]
    fn sandbox_gate_describes_every_unavailable_variant() {
        use std::collections::HashSet;
        let cases = [
            SandboxUnavailable::BadRunAs { run_as: "x".into() },
            SandboxUnavailable::Runtime(RuntimeUnavailable::NotInstalled {
                runtime: "docker".into(),
            }),
            SandboxUnavailable::Runtime(RuntimeUnavailable::DaemonUnreachable {
                docker_host: "unix:///tmp/docker.sock".into(),
                stderr: String::new(),
            }),
            SandboxUnavailable::Runtime(RuntimeUnavailable::UnsupportedRuntime {
                runtime: "podman".into(),
            }),
            SandboxUnavailable::UidGate(UidGateOutcome::ContainerRoot { host_uid: 1000 }),
            SandboxUnavailable::NoLock,
            SandboxUnavailable::Image(ImageCheck::Mismatch {
                locked: format!("sha256:{}", "d".repeat(64)),
                live: format!("sha256:{}", "b".repeat(64)),
            }),
        ];
        let mut messages = HashSet::new();
        for case in cases {
            let text = describe_unavailable(&case);
            assert!(
                text.starts_with("sandbox unavailable: "),
                "message must start with the prefix: {text:?}"
            );
            messages.insert(text);
        }
        assert_eq!(
            messages.len(),
            7,
            "every variant must have a distinct message"
        );
    }

    #[test]
    fn sandbox_gate_describes_image_mismatch_with_a_single_prefix() {
        let live = format!("sha256:{}", "b".repeat(64));
        let text = describe_unavailable(&SandboxUnavailable::Image(ImageCheck::Mismatch {
            locked: format!("sha256:{}", "a".repeat(64)),
            live: live.clone(),
        }));
        assert_eq!(
            text.matches("sha256:").count(),
            1,
            "rendered message: {text}"
        );
        assert!(text.contains(&live), "missing id in: {text}");
    }

    #[test]
    fn sandbox_gate_describes_nolock_with_the_build_command() {
        let text = describe_unavailable(&SandboxUnavailable::NoLock);
        assert!(text.contains("sandbox build"), "got: {text}");
    }

    #[test]
    fn sandbox_gate_describes_bad_run_as_with_the_offending_value() {
        let text = describe_unavailable(&SandboxUnavailable::BadRunAs {
            run_as: "nope".into(),
        });
        assert!(text.contains("nope"), "got: {text}");
    }

    #[test]
    fn sandbox_gate_disabled_config_is_ok_without_probing() {
        let cfg = SandboxConfig::default();
        assert!(sandbox_preflight(&cfg).is_ok());
    }

    #[test]
    #[ignore = "requires a running rootless Docker daemon"]
    fn sandbox_gate_preflight_reaches_a_real_runtime() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        match sandbox_preflight(&cfg) {
            Ok(()) => {}
            Err(SandboxUnavailable::NoLock) => {}
            Err(other) => panic!("unexpected preflight failure: {other:?}"),
        }
    }

    #[test]
    fn sandbox_host_run_args_start_with_the_configured_endpoint() {
        let cfg = SandboxConfig::default();
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: false,
            command: "echo hi",
        };
        let args = run_args(&cfg, &spec, None);
        let first_three = args.iter().take(3).cloned().collect::<Vec<_>>();
        assert_eq!(
            first_three,
            vec![
                "--host".to_string(),
                "unix:///run/user/1000/docker.sock".to_string(),
                "run".to_string()
            ]
        );
        assert_eq!(
            args.iter().filter(|a| *a == "--host").count(),
            1,
            "`--host` must appear exactly once: {args:?}"
        );
        assert_ne!(args[0], "run");
    }

    #[test]
    fn sandbox_host_stage_args_start_with_the_configured_endpoint() {
        let cfg = SandboxConfig {
            docker_host: "unix:///tmp/alt.sock".to_string(),
            ..SandboxConfig::default()
        };
        let args = stage_args(&cfg, "j1", "myscript.sh");
        let first_three = args.iter().take(3).cloned().collect::<Vec<_>>();
        assert_eq!(
            first_three,
            vec![
                "--host".to_string(),
                "unix:///tmp/alt.sock".to_string(),
                "run".to_string()
            ]
        );
    }

    #[test]
    fn sandbox_host_window_command_carries_the_endpoint() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let result = sandbox_window_command(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            "echo hi",
            None,
        );
        let prefix = "'--host' 'unix:///run/user/1000/docker.sock'";
        let run_at = result.find("'run'").expect("run must be present");
        let host_at = result
            .find(prefix)
            .expect("endpoint prefix must be present");
        assert!(
            host_at < run_at,
            "`--host` must precede `run` in the window command: {result}"
        );
        assert_eq!(
            result.matches("'--host'").count(),
            1,
            "`--host` must appear exactly once in the window command: {result}"
        );
    }

    #[test]
    #[ignore = "requires a running rootless Docker daemon"]
    fn sandbox_host_command_runs_with_no_ambient_docker_host() {
        let cfg = SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        };
        let job_id = format!("scrub-{}", std::process::id());
        let spec = ExecSpec {
            job_id: &job_id,
            network: "none",
            is_ghost: false,
            command: "echo scrubbed-ok",
        };
        let line = sandbox_window_command(&cfg, &spec, "echo scrubbed-ok", None);
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&line)
            .env_remove("DOCKER_HOST")
            .output()
            .expect("spawning sh failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("scrubbed-ok"),
            "command without DOCKER_HOST failed: {stdout:?} stderr={:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn sandbox_stage_args_mount_the_script_source_read_only() {
        let cfg = SandboxConfig::default();
        let args = stage_args(&cfg, "j1", "myscript.sh");
        let src = format!("{}:/de/src:ro", crate::config::scripts_dir().display());
        let stage = "de-stage-j1:/stage";
        let src_at = args.iter().position(|a| a == &src);
        let stage_at = args.iter().position(|a| a == stage);
        assert!(src_at.is_some(), "missing source mount: {args:?}");
        assert!(stage_at.is_some(), "missing stage volume: {args:?}");
        assert!(
            src_at < stage_at,
            "source mount must precede stage: {args:?}"
        );
    }

    #[test]
    fn sandbox_stage_args_keep_the_root_helper_and_chown() {
        let cfg = SandboxConfig::default();
        let args = stage_args(&cfg, "j1", "myscript.sh");
        let user_at = args.iter().position(|a| a == "--user").expect("--user");
        assert_eq!(args.get(user_at + 1).map(String::as_str), Some("0:0"));
        let shell = args
            .iter()
            .find(|a| a.starts_with("cp /de/src/"))
            .expect("shell line present");
        assert!(shell.contains("chmod 0500"), "got: {shell}");
        assert!(shell.contains("chown 1000:1000"), "got: {shell}");
    }

    #[test]
    fn sandbox_stage_args_still_reject_unsafe_script_names() {
        let cfg = SandboxConfig::default();
        assert!(stage_args(&cfg, "j1", "../etc/passwd").is_empty());
    }

    #[test]
    fn sandbox_stage_ghost_spec_carries_both_labels() {
        let cfg = SandboxConfig::default();
        let ghost = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: true,
                command: "echo hi",
            },
            None,
        );
        assert!(ghost.iter().any(|a| a == "de.sandbox=1"));
        assert!(ghost.iter().any(|a| a == "de.ghost=1"));

        let ordinary = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        assert!(ordinary.iter().any(|a| a == "de.sandbox=1"));
        assert!(!ordinary.iter().any(|a| a == "de.ghost=1"));
    }

    #[test]
    fn sandbox_staging_detects_a_script_the_predicate_knows() {
        let known = |n: &str| n == "myscript.sh";
        assert_eq!(
            sandbox_script_invocation("myscript.sh --flag one", known),
            Some(("myscript.sh".to_string(), " --flag one".to_string()))
        );
        assert_eq!(
            sandbox_script_invocation("~/.daemoneye/scripts/myscript.sh", known),
            Some(("myscript.sh".to_string(), String::new()))
        );
    }

    #[test]
    fn sandbox_staging_ignores_commands_that_are_not_scripts() {
        let known = |n: &str| n == "myscript.sh";
        assert_eq!(
            sandbox_script_invocation("ls -la", known),
            None,
            "a basename that is not a script is an ordinary command"
        );
        assert_eq!(
            sandbox_script_invocation("myscript.sh", |_| false),
            None,
            "the predicate is the authority, not the name shape"
        );
        assert_eq!(
            sandbox_script_invocation("/home/op/.daemoneye/scripts/myscript.sh", |_| true),
            None,
            "an absolute path is never a script invocation (foreground parity)"
        );
        assert_eq!(
            sandbox_script_invocation("", |_| true),
            None,
            "empty command"
        );
    }

    #[test]
    fn sandbox_staging_never_stages_under_sudo() {
        assert_eq!(
            sandbox_script_invocation("sudo myscript.sh", |_| true),
            None,
            "leading sudo"
        );
        assert_eq!(
            sandbox_script_invocation("myscript.sh && sudo reboot", |_| true),
            None,
            "sudo later in the line"
        );
    }

    #[test]
    fn sandbox_staging_rewrites_to_the_staged_path() {
        assert_eq!(
            staged_script_command("myscript.sh", " --flag one"),
            "/de/scripts/myscript.sh --flag one"
        );
        assert_eq!(
            staged_script_command("myscript.sh", ""),
            "/de/scripts/myscript.sh"
        );
    }

    #[test]
    fn sandbox_staging_refuses_unstageable_names_without_spawning() {
        let cfg = SandboxConfig {
            runtime: "/nonexistent/de-runtime".to_string(),
            ..Default::default()
        };
        let err = stage_script(&cfg, "j1", "../etc/passwd").expect_err("refused");
        assert!(err.contains("not a stageable script name"), "got: {err}");
    }

    #[test]
    fn sandbox_staging_reports_a_helper_that_cannot_run() {
        let cfg = SandboxConfig {
            runtime: "/nonexistent/de-runtime".to_string(),
            ..Default::default()
        };
        let err = stage_script(&cfg, "j1", "myscript.sh").expect_err("spawn fails");
        assert!(
            err.starts_with("sandbox staging failed for `myscript.sh`"),
            "got: {err}"
        );
    }

    #[test]
    fn job_id_for_strips_the_pane_sigil() {
        assert_eq!(job_id_for("%42", 1712937600), "42-1712937600");
        assert_eq!(
            job_id_for("42", 1712937600),
            "42-1712937600",
            "a pane number with no sigil is already the job id's first half"
        );
    }

    #[test]
    fn job_id_for_names_the_volume_the_container_mounts() {
        let job = job_id_for("%42", 17);
        assert_eq!(stage_volume_name(&job), "de-stage-42-17");
    }

    #[test]
    fn job_id_for_distinguishes_a_retry_from_its_original_run() {
        assert_ne!(
            job_id_for("%42", 100),
            job_id_for("%42", 101),
            "a retry in the same pane must not reuse the original job's volume name"
        );
    }

    #[test]
    fn sandbox_session_label_is_absent_without_a_session() {
        let cfg = SandboxConfig::default();
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: true,
            command: "echo hi",
        };
        assert!(
            !run_args(&cfg, &spec, None)
                .iter()
                .any(|a| a.starts_with("de.session=")),
            "no session label without a session"
        );
    }

    #[test]
    fn sandbox_session_label_rides_beside_the_ghost_label() {
        let cfg = SandboxConfig::default();
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: true,
            command: "echo hi",
        };
        let args = run_args(&cfg, &spec, Some("ghost-aaa"));
        assert!(args.iter().any(|a| a == "de.ghost=1"), "{args:?}");
        assert!(args.iter().any(|a| a == "de.session=ghost-aaa"), "{args:?}");
        let label = args
            .iter()
            .position(|a| a == "de.session=ghost-aaa")
            .expect("label");
        let image = args
            .iter()
            .position(|a| a == "daemoneye-agent-base")
            .expect("image");
        assert!(
            label < image,
            "the label must precede the image or docker hands it to the container: {args:?}"
        );
    }

    #[test]
    fn sandbox_session_label_keeps_a_value_containing_an_equals_sign() {
        // Ghost ids embed the alert name (`ghost-<alert>-<uuid>`), and docker
        // splits `--label k=v` on the first `=` only — measured, not assumed.
        let cfg = SandboxConfig::default();
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: true,
            command: "echo hi",
        };
        let args = run_args(&cfg, &spec, Some("ghost-a=b-1"));
        assert!(
            args.iter().any(|a| a == "de.session=ghost-a=b-1"),
            "{args:?}"
        );
    }

    #[test]
    fn sandbox_session_label_reaches_the_window_command() {
        let cfg = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let spec = ExecSpec {
            job_id: "j1",
            network: "none",
            is_ghost: true,
            command: "echo hi",
        };
        let line = sandbox_window_command(&cfg, &spec, "echo hi", Some("ghost-aaa"));
        assert!(line.contains("de.session=ghost-aaa"), "{line}");
        assert!(
            !sandbox_window_command(&cfg, &spec, "echo hi", None).contains("de.session"),
            "no session means no label in the window command either"
        );
    }

    #[test]
    fn ghost_teardown_selects_one_session_and_not_its_neighbours() {
        let cfg = SandboxConfig::default();
        let args = ghost_teardown_list_args(&cfg, "ghost-aaa");
        assert!(
            args.iter().any(|a| a == "label=de.session=ghost-aaa"),
            "{args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "label=de.session=ghost-aaa-extra"),
            "a sibling ghost must never be named: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "label=de.session=ghost-bbb"),
            "another ghost must never be named: {args:?}"
        );
    }

    #[test]
    fn ghost_teardown_is_scoped_to_this_daemons_ghosts() {
        let cfg = SandboxConfig::default();
        let args = ghost_teardown_list_args(&cfg, "ghost-aaa");
        assert!(args.iter().any(|a| a == "label=de.sandbox=1"), "{args:?}");
        assert!(
            args.iter().any(|a| a == "label=de.ghost=1"),
            "without the ghost filter an interactive session's container could match: {args:?}"
        );
        assert_eq!(args.first().map(String::as_str), Some("--host"), "{args:?}");
        assert!(
            args.iter().any(|a| a == "-aq"),
            "stopped containers count too: {args:?}"
        );
    }

    #[test]
    fn ghost_teardown_honours_destroy_on_exit_and_the_sandbox_flag() {
        let on = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(
            should_teardown_ghost(&on),
            "default destroy_on_exit is true"
        );

        let off = SandboxConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(
            !should_teardown_ghost(&off),
            "sandbox off means nothing to reclaim"
        );

        let no_destroy = SandboxConfig {
            enabled: true,
            ghost_defaults: crate::config::SandboxGhostDefaults {
                destroy_on_exit: false,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            !should_teardown_ghost(&no_destroy),
            "the operator turned it off"
        );
    }

    #[test]
    fn container_status_inspect_args_are_empty_without_ids() {
        let cfg = SandboxConfig::default();
        assert!(
            status_inspect_args(&cfg, &[]).is_empty(),
            "docker inspect with no arguments is a usage error, not an empty result"
        );
    }

    #[test]
    fn container_status_inspect_args_carry_the_json_label_format() {
        let cfg = SandboxConfig::default();
        let args = status_inspect_args(&cfg, &["abc".to_string(), "def".to_string()]);
        assert_eq!(args.first().map(String::as_str), Some("--host"), "{args:?}");
        assert!(args.iter().any(|a| a == "inspect"), "{args:?}");
        assert!(args.iter().any(|a| a == STATUS_INSPECT_FORMAT), "{args:?}");
        assert!(
            args.iter().any(|a| a.contains("json .Config.Labels")),
            "labels must come back as JSON, not docker's comma-joined string: {args:?}"
        );
        assert_eq!(
            &args[args.len() - 2..],
            &["abc".to_string(), "def".to_string()]
        );
    }

    #[test]
    fn container_status_parses_a_ghost_and_an_interactive_record() {
        let text = concat!(
            "39c2a88ad4137144 running alpine:3.22 {\"de.sandbox\":\"1\",\"de.session\":\"sess-plain\"}\n",
            "a1997c9929c48003 exited alpine:3.22 {\"de.ghost\":\"1\",\"de.sandbox\":\"1\",\"de.session\":\"ghost-x\"}\n"
        );
        let got = parse_container_records(text);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].id, "39c2a88ad413", "id is truncated for display");
        assert_eq!(got[0].state, "running");
        assert_eq!(got[0].session.as_deref(), Some("sess-plain"));
        assert!(!got[0].is_ghost);
        assert!(got[1].is_ghost);
        assert_eq!(got[1].session.as_deref(), Some("ghost-x"));
    }

    #[test]
    fn container_status_survives_a_session_id_with_spaces_and_commas() {
        // A ghost id is `ghost-<alert>-<uuid>` and the alert name comes from a
        // webhook, so it can hold spaces, commas and `=`. Measured: docker's
        // own `{{.Labels}}` joins pairs with `,` and cannot be split back.
        let text = "abcdef0123456789 running img {\"de.ghost\":\"1\",\"de.session\":\"ghost-disk full,x=1-uuid\"}\n";
        let got = parse_container_records(text);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(
            got[0].session.as_deref(),
            Some("ghost-disk full,x=1-uuid"),
            "the label value must survive the split intact"
        );
        assert_eq!(got[0].image, "img");
    }

    #[test]
    fn container_status_skips_a_line_it_cannot_decode() {
        let text = concat!(
            "abcdef0123456789 running img not-json\n",
            "0123456789abcdef running img {\"de.session\":\"ok\"}\n",
            "too few fields\n"
        );
        let got = parse_container_records(text);
        assert_eq!(got.len(), 1, "only the decodable line survives: {got:?}");
        assert_eq!(got[0].session.as_deref(), Some("ok"));
    }

    #[test]
    fn container_status_reports_an_unlabelled_container_without_a_session() {
        let text = "abcdef0123456789 created img {\"de.sandbox\":\"1\"}\n";
        let got = parse_container_records(text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session, None);
        assert!(!got[0].is_ghost);
    }

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
        assert_eq!(
            &args[args.len() - 2..],
            &["n1".to_string(), "n2".to_string()]
        );
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
        assert!(
            conf.lines().any(|l| l.trim() == "FilterDefaultDeny Yes"),
            "{conf}"
        );
        assert!(
            conf.lines().any(|l| l.trim() == "FilterType fnmatch"),
            "{conf}"
        );
        assert!(
            conf.lines()
                .any(|l| l.trim() == "Filter \"/etc/tinyproxy/filter\""),
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
        assert!(
            df.contains("tinyproxy.conf /etc/tinyproxy/tinyproxy.conf"),
            "{df}"
        );
        assert!(df.contains("apk add --no-cache tinyproxy"), "{df}");
    }

    fn cfg_with_profile(network: &str) -> SandboxConfig {
        let mut cfg = SandboxConfig::default();
        cfg.profile.insert(
            "researcher".to_string(),
            crate::config::SandboxProfile {
                network: network.to_string(),
                proxy_allow: vec!["example.com".to_string()],
                proxy_deny: Vec::new(),
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

    #[test]
    fn sandbox_filter_parses_the_three_supported_rule_forms() {
        assert_eq!(
            parse_proxy_rule("example.com"),
            ProxyRule::Host("example.com".to_string())
        );
        assert_eq!(
            parse_proxy_rule("  example.com  "),
            ProxyRule::Host("example.com".to_string()),
            "surrounding whitespace is trimmed, not rejected"
        );
        assert_eq!(
            parse_proxy_rule("*.example.com"),
            ProxyRule::Subdomains("example.com".to_string())
        );
        assert_eq!(
            parse_proxy_rule("example.com:443"),
            ProxyRule::Host("example.com".to_string())
        );
        assert_eq!(
            parse_proxy_rule("*.example.com:80"),
            ProxyRule::Subdomains("example.com".to_string())
        );
    }

    #[test]
    fn sandbox_filter_refuses_every_rule_it_cannot_enforce() {
        // Each of these would otherwise be approximated into a *broader*
        // grant than the operator wrote. Measured 2026-08-30: a tinyproxy
        // filter line matches the host alone, so a port cannot be enforced
        // there, and `ConnectPort 443`/`563` is what caps CONNECT.
        for bad in [
            "",
            "   ",
            "https://example.com/",
            "example.com/path",
            "example.com example.org",
            "example.com:22",
            "example.com:8443",
            "example.com:notaport",
            "*",
            "*.",
            "ex*ple.com",
            ":443",
        ] {
            assert!(
                matches!(parse_proxy_rule(bad), ProxyRule::Unsupported(_)),
                "{bad:?} must not parse into a usable rule, got {:?}",
                parse_proxy_rule(bad)
            );
        }
    }

    #[test]
    fn sandbox_filter_renders_one_pattern_per_line_in_order() {
        let out = render_proxy_filter(
            &[
                "crates.io".to_string(),
                "*.crates.io".to_string(),
                "crates.io".to_string(),
                "docs.rs:443".to_string(),
            ],
            &[],
        );
        assert_eq!(out, "crates.io\n*.crates.io\ndocs.rs\n", "{out:?}");
    }

    #[test]
    fn sandbox_filter_deny_beats_an_exactly_matching_allow() {
        let out = render_proxy_filter(
            &["a.example.com".to_string(), "b.example.com".to_string()],
            &["a.example.com".to_string()],
        );
        assert_eq!(out, "b.example.com\n", "{out:?}");
    }

    #[test]
    fn sandbox_filter_a_deny_inside_a_wildcard_drops_the_whole_wildcard() {
        // tinyproxy's filter is an allow list with no exception form, so the
        // narrower grant cannot be expressed. Dropping the wildcard is the
        // only rendering that does not leak the denied host.
        let out = render_proxy_filter(
            &["*.example.com".to_string(), "other.org".to_string()],
            &["secret.example.com".to_string()],
        );
        assert_eq!(out, "other.org\n", "{out:?}");
        assert!(
            !out.contains("example.com"),
            "the denied host must not remain reachable through the wildcard: {out:?}"
        );
        // A deny that is merely a *sibling* of the wildcard leaves it intact.
        let unrelated = render_proxy_filter(
            &["*.example.com".to_string()],
            &["secret.example.org".to_string()],
        );
        assert_eq!(unrelated, "*.example.com\n", "{unrelated:?}");
        // The apex is not inside its own wildcard — `*.d` never matches `d`.
        let apex =
            render_proxy_filter(&["*.example.com".to_string()], &["example.com".to_string()]);
        assert_eq!(apex, "*.example.com\n", "{apex:?}");
    }

    #[test]
    fn sandbox_filter_denies_everything_when_nothing_survives() {
        // An empty filter file is deny-all (measured), so each of these is a
        // profile that can reach nothing — never an open door.
        assert_eq!(render_proxy_filter(&[], &[]), "");
        assert_eq!(
            render_proxy_filter(&["example.com:22".to_string()], &[]),
            "",
            "an all-unsupported allow list must not fall back to permitting anything"
        );
        assert_eq!(
            render_proxy_filter(&["example.com".to_string()], &["example.com".to_string()]),
            ""
        );
        assert_eq!(
            render_proxy_filter(
                &["*.example.com".to_string()],
                &["*.example.com".to_string()]
            ),
            ""
        );
    }

    #[test]
    fn sandbox_filter_for_an_unknown_profile_is_deny_all() {
        let mut cfg = cfg_with_profile("proxy");
        cfg.profile
            .get_mut("researcher")
            .expect("seeded above")
            .proxy_deny = vec!["bad.example.com".to_string()];
        assert_eq!(
            filter_for_profile(&cfg, Some("researcher")),
            "example.com\n"
        );
        assert_eq!(
            filter_for_profile(&cfg, Some("analyst")),
            "",
            "a profile with no config entry reaches nothing"
        );
        assert_eq!(filter_for_profile(&cfg, None), "");
    }

    #[test]
    fn sandbox_filter_conf_caps_connect_to_tls_ports() {
        // Without these two lines tinyproxy opens CONNECT to *any* port on an
        // allowlisted host — measured 2026-08-30, it dialled example.com:22,
        // :25 and :3306. That would make the milestone's "HTTP(S) only"
        // contract false.
        let conf = include_str!("../../../containers/proxy/tinyproxy.conf");
        assert!(
            conf.lines().any(|l| l.trim() == "ConnectPort 443"),
            "{conf}"
        );
        assert!(
            conf.lines().any(|l| l.trim() == "ConnectPort 563"),
            "{conf}"
        );
        assert!(
            !conf.lines().any(|l| l.trim() == "ConnectPort 22"),
            "{conf}"
        );
    }

    /// A verbatim excerpt of a real job proxy's log, captured on the daemon
    /// host 2026-08-31 with filter `example.com` + `*.wikipedia.org`. Every
    /// parser test below reads a slice of this rather than an invented shape.
    const PROXY_LOG: &str = concat!(
        "NOTICE    Aug 31 03:32:11.169 [1]: Initializing tinyproxy ...\n",
        "INFO      Aug 31 03:32:11.169 [1]: Starting main loop. Accepting connections.\n",
        "CONNECT   Aug 31 03:32:20.545 [1]: Connect (file descriptor 4): 172.18.0.3\n",
        "CONNECT   Aug 31 03:32:20.545 [1]: Request (file descriptor 4): GET http://example.com/ HTTP/1.1\n",
        "INFO      Aug 31 03:32:20.545 [1]: No upstream proxy for example.com\n",
        "INFO      Aug 31 03:32:20.545 [1]: opensock: opening connection to example.com:80\n",
        "CONNECT   Aug 31 03:32:20.590 [1]: Request (file descriptor 4): CONNECT example.com:443 HTTP/1.1\n",
        "INFO      Aug 31 03:32:20.590 [1]: No upstream proxy for example.com\n",
        "CONNECT   Aug 31 03:32:20.681 [1]: Request (file descriptor 4): GET http://www.example.com/ HTTP/1.1\n",
        "NOTICE    Aug 31 03:32:20.681 [1]: Proxying refused on filtered domain \"www.example.com\"\n",
        "CONNECT   Aug 31 03:32:20.683 [1]: Request (file descriptor 4): CONNECT en.wikipedia.org:443 HTTP/1.1\n",
        "INFO      Aug 31 03:32:20.683 [1]: No upstream proxy for en.wikipedia.org\n",
        "CONNECT   Aug 31 03:32:20.787 [1]: Request (file descriptor 4): CONNECT example.com:22 HTTP/1.1\n",
        "INFO      Aug 31 03:32:20.787 [1]: Refused CONNECT method on port 22\n",
    );

    #[test]
    fn sandbox_proxy_log_reads_method_host_and_port_from_every_request() {
        let records = parse_proxy_log(PROXY_LOG);
        let seen: Vec<(String, String, u16)> = records
            .iter()
            .map(|r| (r.method.clone(), r.host.clone(), r.port))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("GET".to_string(), "example.com".to_string(), 80),
                ("CONNECT".to_string(), "example.com".to_string(), 443),
                ("GET".to_string(), "www.example.com".to_string(), 80),
                ("CONNECT".to_string(), "en.wikipedia.org".to_string(), 443),
                ("CONNECT".to_string(), "example.com".to_string(), 22),
            ],
            "boot, opensock and Connect lines must produce nothing"
        );
    }

    #[test]
    fn sandbox_proxy_log_decides_each_request_from_the_line_that_follows_it() {
        let records = parse_proxy_log(PROXY_LOG);
        let seen: Vec<(&str, &str)> = records.iter().map(|r| (r.decision, r.reason)).collect();
        assert_eq!(
            seen,
            vec![
                ("allowed", "allowed"),
                ("allowed", "allowed"),
                ("denied", "filtered"),
                ("allowed", "allowed"),
                ("denied", "port"),
            ]
        );
    }

    #[test]
    fn sandbox_proxy_log_ignores_a_refusal_that_names_another_host() {
        // Guarded by host: a filtered-domain line for someone else leaves this
        // request allowed, which is what the concurrency measurement requires.
        let log = concat!(
            "CONNECT   Aug 31 03:33:16.785 [1]: Request (file descriptor 4): GET http://example.com/ HTTP/1.1\n",
            "NOTICE    Aug 31 03:33:16.785 [1]: Proxying refused on filtered domain \"blocked.test\"\n",
        );
        let records = parse_proxy_log(log);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].decision, "allowed");
    }

    #[test]
    fn sandbox_proxy_log_ignores_a_port_refusal_for_another_port() {
        let log = concat!(
            "CONNECT   Aug 31 03:33:53.374 [1]: Request (file descriptor 4): CONNECT example.com:443 HTTP/1.1\n",
            "INFO      Aug 31 03:33:53.374 [1]: Refused CONNECT method on port 22\n",
        );
        assert_eq!(parse_proxy_log(log)[0].decision, "allowed");
    }

    #[test]
    fn sandbox_proxy_log_collapses_identical_consecutive_requests() {
        let one = "CONNECT   Aug 31 03:32:20.788 [1]: Request (file descriptor 4): GET http://example.com/ HTTP/1.1\nINFO      Aug 31 03:32:20.788 [1]: No upstream proxy for example.com\n";
        let log = format!("{one}{one}{one}");
        let records = parse_proxy_log(&log);
        assert_eq!(records.len(), 1, "three identical requests collapse to one");
        assert_eq!(records[0].repeats, 3);
    }

    #[test]
    fn sandbox_proxy_log_does_not_collapse_across_a_different_request() {
        let get = "CONNECT   Aug 31 03:32:20.788 [1]: Request (file descriptor 4): GET http://example.com/ HTTP/1.1\n";
        let other = "CONNECT   Aug 31 03:32:20.789 [1]: Request (file descriptor 4): GET http://en.wikipedia.org/ HTTP/1.1\n";
        let records = parse_proxy_log(&format!("{get}{other}{get}"));
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|r| r.repeats == 1));
    }

    #[test]
    fn sandbox_proxy_log_defaults_the_port_from_the_scheme_and_keeps_an_explicit_one() {
        let log = concat!(
            "CONNECT   Aug 31 03:33:53.416 [1]: Request (file descriptor 4): GET https://example.com/ HTTP/1.1\n",
            "CONNECT   Aug 31 03:33:53.417 [1]: Request (file descriptor 4): GET http://example.com:8080/ HTTP/1.1\n",
        );
        let ports: Vec<u16> = parse_proxy_log(log).iter().map(|r| r.port).collect();
        assert_eq!(
            ports,
            vec![443, 8080],
            "8080 is reachable over plain HTTP today — the port field is what makes that visible"
        );
    }

    #[test]
    fn sandbox_proxy_audit_never_records_the_path_or_query() {
        // Measured verbatim: the proxy logs the whole absolute URI, so a token
        // in a query string is one careless field away from events.jsonl.
        let log = "CONNECT   Aug 31 03:33:53.378 [1]: Request (file descriptor 4): GET http://example.com/secret?token=abc HTTP/1.1\n";
        let records = parse_proxy_log(log);
        assert_eq!(records[0].host, "example.com");
        let event = records[0].to_event("42-1", Some("s1")).to_string();
        assert!(!event.contains("token"), "{event}");
        assert!(!event.contains("secret"), "{event}");
    }

    #[test]
    fn sandbox_proxy_audit_strips_userinfo_from_the_host() {
        let log = "CONNECT   Aug 31 03:33:53.378 [1]: Request (file descriptor 4): GET http://user:pw@example.com/ HTTP/1.1\n";
        let records = parse_proxy_log(log);
        assert_eq!(records[0].host, "example.com");
        assert!(!records[0].to_event("42-1", None).to_string().contains("pw"));
    }

    #[test]
    fn sandbox_proxy_rule_match_prefers_deny_over_allow() {
        let allow = vec!["*.example.com".to_string()];
        let deny = vec!["evil.example.com".to_string()];
        assert_eq!(
            match_proxy_rule("evil.example.com", &allow, &deny),
            RuleMatch::Deny("evil.example.com".to_string())
        );
        assert_eq!(
            match_proxy_rule("good.example.com", &allow, &deny),
            RuleMatch::Allow("*.example.com".to_string())
        );
    }

    #[test]
    fn sandbox_proxy_rule_match_reports_none_for_an_unlisted_host() {
        let allow = vec!["example.com".to_string()];
        assert_eq!(
            match_proxy_rule("elsewhere.test", &allow, &[]),
            RuleMatch::None
        );
        assert_eq!(
            match_proxy_rule("www.example.com", &allow, &[]),
            RuleMatch::None,
            "an exact rule does not cover a subdomain"
        );
        assert_eq!(RuleMatch::None.label(), "none");
    }

    #[test]
    fn sandbox_filter_lookalike_suffix_is_not_a_subdomain() {
        // Carried from phase-08's review: removing the dot-boundary check in
        // is_subdomain_of killed no test. This is that test.
        assert!(is_subdomain_of("a.example.com", "example.com"));
        assert!(!is_subdomain_of("evilexample.com", "example.com"));
        assert!(!is_subdomain_of("example.com", "example.com"));
        assert_eq!(
            match_proxy_rule("evilexample.com", &["*.example.com".to_string()], &[]),
            RuleMatch::None
        );
    }

    #[test]
    fn sandbox_proxy_audit_event_names_the_rule_and_the_proxy_type() {
        let records = audit_proxy_log(
            PROXY_LOG,
            &["example.com".to_string(), "*.wikipedia.org".to_string()],
            &[],
        );
        let denied = records
            .iter()
            .find(|r| r.host == "www.example.com")
            .expect("the filtered request is audited");
        let event = denied.to_event("42-1712937600", Some("s1"));
        assert_eq!(event["decision"], "denied");
        assert_eq!(event["reason"], "filtered");
        assert_eq!(event["rule"], "none");
        assert_eq!(event["proxy_type"], "forward");
        assert_eq!(event["job_id"], "42-1712937600");
        assert_eq!(event["session"], "s1");
        assert_eq!(event["repeats"], 1);
        let allowed = records
            .iter()
            .find(|r| r.host == "en.wikipedia.org")
            .expect("the wildcard-allowed request is audited");
        assert_eq!(
            allowed.to_event("42-1", None)["rule"],
            "allow:*.wikipedia.org"
        );
        assert_eq!(allowed.to_event("42-1", None)["session"], "-");
    }

    #[test]
    fn sandbox_proxy_logs_args_read_the_jobs_own_proxy_container() {
        let cfg = SandboxConfig {
            docker_host: "unix:///run/user/1000/docker.sock".to_string(),
            ..Default::default()
        };
        assert_eq!(
            proxy_logs_args(&cfg, "42-1712937600"),
            vec![
                "--host".to_string(),
                "unix:///run/user/1000/docker.sock".to_string(),
                "logs".to_string(),
                "de-px-42-1712937600".to_string(),
            ]
        );
    }

    #[test]
    fn sandbox_proxy_rules_for_profile_falls_back_to_no_rules() {
        let mut cfg = SandboxConfig::default();
        cfg.profile.insert(
            "web".to_string(),
            crate::config::SandboxProfile {
                network: "proxy".to_string(),
                proxy_allow: vec!["example.com".to_string()],
                proxy_deny: vec!["evil.example.com".to_string()],
            },
        );
        assert_eq!(
            proxy_rules_for_profile(&cfg, Some("web")),
            (
                vec!["example.com".to_string()],
                vec!["evil.example.com".to_string()]
            )
        );
        assert_eq!(
            proxy_rules_for_profile(&cfg, Some("absent")),
            (Vec::new(), Vec::new())
        );
        assert_eq!(
            proxy_rules_for_profile(&cfg, None),
            (Vec::new(), Vec::new())
        );
    }

    /// Every flag this phase adds, as an adjacent (flag, value) pair or a bare
    /// switch. Measured effective against the real image on the daemon host
    /// 2026-08-31 — see the phase doc's § Live measurements.
    #[test]
    fn sandbox_run_args_carry_every_hardening_flag() {
        let cfg = SandboxConfig::default();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let pair = |flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(pair("--memory-swap", "1g"), "{args:?}");
        assert!(pair("--cap-drop", "ALL"), "{args:?}");
        assert!(pair("--security-opt", "no-new-privileges"), "{args:?}");
        assert!(pair("--pull", "never"), "{args:?}");
        assert!(args.iter().any(|a| a == "--read-only"), "{args:?}");
    }

    #[test]
    fn sandbox_run_args_cap_swap_at_the_memory_limit() {
        // Docker defaults --memory-swap to 2x --memory. Measured: without the
        // flag a 1g container reports MemorySwap=2147483648; with it, 1g.
        // The value must track limits.memory, not be a second literal.
        let mut cfg = SandboxConfig::default();
        cfg.limits.memory = "512m".to_string();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let swap = args
            .windows(2)
            .find(|w| w[0] == "--memory-swap")
            .map(|w| w[1].clone());
        assert_eq!(swap.as_deref(), Some("512m"), "{args:?}");
    }

    #[test]
    fn sandbox_run_args_give_a_read_only_root_two_writable_tmpfs() {
        // --read-only without a writable /tmp breaks ordinary tooling, so the
        // two arrive together or not at all. /de/work stays 0700 and private;
        // /tmp is 1777 because that is what programs expect of it.
        let cfg = SandboxConfig::default();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let mounts: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "--tmpfs")
            .map(|w| &w[1])
            .collect();
        assert_eq!(mounts.len(), 2, "{args:?}");
        assert!(
            mounts[0].starts_with("/de/work:rw,") && mounts[0].contains("mode=0700"),
            "{mounts:?}"
        );
        assert!(
            mounts[1].starts_with("/tmp:rw,") && mounts[1].contains("mode=1777"),
            "{mounts:?}"
        );
        assert!(args.iter().any(|a| a == "--read-only"), "{args:?}");
    }

    #[test]
    fn sandbox_images_pin_their_base_by_digest() {
        // The design pins the base image by digest; a moving tag would let a
        // rebuild change the contents under a lock that still matches.
        for (name, text) in [
            ("agent", include_str!("../../../containers/Dockerfile")),
            (
                "proxy",
                include_str!("../../../containers/proxy/Dockerfile"),
            ),
        ] {
            let from = text
                .lines()
                .find(|l| l.starts_with("FROM "))
                .unwrap_or_else(|| panic!("{name} Dockerfile has no FROM line"));
            assert!(
                from.contains("@sha256:"),
                "{name} Dockerfile pins a tag, not a digest: {from}"
            );
            assert!(
                !from.contains(':') || from.split("@sha256:").count() == 2,
                "{name}: {from}"
            );
        }
    }

    #[test]
    fn sandbox_container_run_event_names_the_image_and_the_network() {
        let cfg = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: "sha256:".to_string() + &"a".repeat(64),
            built_at: 42,
        };
        let event = container_run_event(
            &cfg,
            Some(&lock),
            "42-1712937600",
            "de-bg-42-1712937600-cargo-build",
            "de-net-42-1712937600",
            Some("s1"),
        )
        .expect("an enabled sandbox produces a record");
        assert_eq!(event["session"], "s1");
        assert_eq!(event["job_id"], "42-1712937600");
        assert_eq!(event["job_name"], "de-bg-42-1712937600-cargo-build");
        assert_eq!(event["image"], "daemoneye-agent-base");
        assert_eq!(event["image_id"], lock.image_id);
        assert_eq!(event["network"], "de-net-42-1712937600");
    }

    #[test]
    fn sandbox_container_run_event_is_absent_when_the_sandbox_is_off() {
        // No container exists, so no record claims one did.
        let cfg = SandboxConfig::default();
        assert!(!cfg.enabled);
        assert!(container_run_event(&cfg, None, "j1", "de-bg-j1", "none", None).is_none());
    }

    #[test]
    fn sandbox_container_run_event_survives_a_missing_lock() {
        // The lock is normally present — preflight refuses without one — but a
        // record that panics is worse than one that says "unknown".
        let cfg = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let event = container_run_event(&cfg, None, "j1", "de-bg-j1", "none", None)
            .expect("an enabled sandbox produces a record");
        assert_eq!(event["image_id"], "unknown");
        assert_eq!(event["session"], "-");
    }
}
