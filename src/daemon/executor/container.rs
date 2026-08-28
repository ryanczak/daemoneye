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

#[cfg(test)]
mod tests {
    use super::*;

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

        let missing_key = "image_id = {id}\nbuilt_at = 1787900000\n";
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
            missing_key,
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
}
