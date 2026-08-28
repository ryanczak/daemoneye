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

/// Read and parse the lock. `None` when the file is absent or malformed —
/// the caller distinguishes "no lock yet" from "bad lock" by its own logic.
pub fn read_lock() -> Option<SandboxLock> {
    let text = std::fs::read_to_string(lock_path()).ok()?;
    parse_lock(&text)
}

/// Write `lock` to `lock_path()`, creating `etc/` if needed.
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

/// Per-run staging volume name for `job_id`: `de-stage-<job_id>`.
pub fn stage_volume_name(job_id: &str) -> String {
    format!("de-stage-{job_id}")
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
        "run".to_string(),
        "--rm".to_string(),
        "--user".to_string(),
        "0:0".to_string(),
        "-v".to_string(),
        format!("{volume}:/stage"),
        cfg.image.clone(),
        "sh".to_string(),
        "-c".to_string(),
        shell_line,
    ]
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
pub fn run_args(cfg: &SandboxConfig, spec: &ExecSpec) -> Vec<String> {
    let Some((uid, gid)) = split_run_as(&cfg.run_as) else {
        return Vec::new();
    };
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--user".to_string(),
        cfg.run_as.clone(),
        "--network".to_string(),
        spec.network.to_string(),
        "--memory".to_string(),
        cfg.limits.memory.clone(),
        "--pids-limit".to_string(),
        cfg.limits.pids.to_string(),
        "--cpus".to_string(),
        cfg.limits.cpus.to_string(),
        "--tmpfs".to_string(),
        format!(
            "{}:rw,size={},mode=0700,uid={},gid={}",
            cfg.workdir, cfg.limits.scratch, uid, gid
        ),
        "-v".to_string(),
        format!("{}:/de/scripts:ro", stage_volume_name(spec.job_id)),
    ];
    if spec.is_ghost {
        args.push("--label".to_string());
        args.push("de.ghost=1".to_string());
    }
    args.push("--workdir".to_string());
    args.push(cfg.workdir.clone());
    args.push(cfg.image.clone());
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(spec.command.to_string());
    args
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
            run_args(&cfg, &spec),
            vec![
                "run",
                "--rm",
                "--user",
                "1000:1000",
                "--network",
                "none",
                "--memory",
                "1g",
                "--pids-limit",
                "256",
                "--cpus",
                "2",
                "--tmpfs",
                "/de/work:rw,size=2g,mode=0700,uid=1000,gid=1000",
                "-v",
                "de-stage-j1:/de/scripts:ro",
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
        );
        let position = args
            .windows(2)
            .position(|pair| pair == ["--label", "de.ghost=1"]);
        assert!(
            position.is_some(),
            "ghost vector lacks --label de.ghost=1: {args:?}"
        );
        let plain = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
        );
        assert!(
            !plain
                .iter()
                .any(|arg| arg == "--label" || arg == "de.ghost=1"),
            "non-ghost vector carries label: {plain:?}"
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
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn sandbox_exec_stage_args_run_as_root_and_chown_to_the_sandbox_uid() {
        let cfg = SandboxConfig::default();
        let args = stage_args(&cfg, "j1", "myscript.sh");
        assert_eq!(
            &args[..6],
            &["run", "--rm", "--user", "0:0", "-v", "de-stage-j1:/stage"]
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
}
