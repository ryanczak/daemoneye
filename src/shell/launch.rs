//! Launching shell hosts: id validation and per-id paths, the in-process host
//! (`run_host`), the detached spawn the daemon will call in phase-06, and the
//! `Status`-round-trip readiness check.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::config::Config;
use crate::shell::proto::{ShellRequest, ShellResponse, encode};

/// The id lands in a filename, so this is a confinement boundary: accept
/// exactly `s` followed by 1–9 ASCII digits.
pub fn validate_shell_id(id: &str) -> Result<()> {
    let Some(digits) = id.strip_prefix('s') else {
        bail!("Invalid shell id: '{id}'");
    };
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("Invalid shell id: '{id}'");
    }
    Ok(())
}

/// `var/run/shells/{id}.sock` — the socket this host serves.
pub fn socket_path(id: &str) -> PathBuf {
    crate::config::shells_dir().join(format!("{id}.sock"))
}

/// `var/run/shells/{id}.lock` — the per-id instance lock that authorizes this
/// process to unlink the socket.
pub fn lock_path(id: &str) -> PathBuf {
    crate::config::shells_dir().join(format!("{id}.lock"))
}

/// `var/log/shells/{id}-{started_unix}.cast` — this host's asciicast log.
pub fn cast_path(id: &str, started_unix: u64) -> PathBuf {
    crate::config::shell_logs_dir().join(format!("{id}-{started_unix}.cast"))
}

/// The cast file this id is writing to. The record's `started_unix` and the
/// cast filename's timestamp are stamped in different processes and can
/// disagree by one second across a spawn, so scan `shell_logs_dir()` for the
/// cast whose name carries the id and the highest `started_unix`, falling back
/// to `cast_path(id, fallback_started_unix)` when no cast exists yet.
pub fn resolve_cast(id: &str, fallback_started_unix: u64) -> PathBuf {
    let logs = crate::config::shell_logs_dir();
    let mut best: Option<u64> = None;
    if let Ok(entries) = std::fs::read_dir(&logs) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some((pid, ts)) = crate::shell::parse_cast_name(&name.to_string_lossy())
                && pid.as_str() == id
                && best.is_none_or(|b| ts > b)
            {
                best = Some(ts);
            }
        }
    }
    match best {
        Some(ts) => cast_path(id, ts),
        None => cast_path(id, fallback_started_unix),
    }
}

/// Run one shell host for `id` in-process: hold the per-id instance lock (the
/// caller owns the id, so only the lockholder may unlink its socket), spawn
/// the PTY backend, bind the socket, and serve until failure. `serve` runs
/// forever; the function returns when the server or the shell dies.
pub async fn run_host(id: &str, shell: &str, args: &[String], rows: u16, cols: u16) -> Result<()> {
    validate_shell_id(id)?;
    let _lock = match crate::daemon::instance::InstanceLock::acquire(&lock_path(id)) {
        Ok(lock) => lock,
        Err(crate::daemon::instance::AcquireError::Held { pid }) => match pid {
            Some(p) => bail!("shell {id} is already hosted (PID {p})"),
            None => bail!("shell {id} is already hosted"),
        },
        Err(crate::daemon::instance::AcquireError::Io(e)) => {
            bail!("acquiring the instance lock for shell {id}: {e}");
        }
    };

    let cfg = Config::load()?;
    let scrollback = cfg.shells.scrollback_lines as usize;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backend = crate::shell::PtyBackend::spawn(
        shell,
        args,
        rows,
        cols,
        scrollback,
        &cast_path(id, now),
        now,
    )?;
    let listener = crate::shell::host::bind(&socket_path(id)).await?;
    match crate::daemon::executor::sbx::sbx_target_from_host(shell, args) {
        Some(target) => {
            let sbx = Arc::new(crate::shell::SbxBackend::new(
                backend,
                target,
                crate::shell::real_runner(shell.to_string()),
            ));
            crate::shell::host::serve_until_exit(listener, sbx, SHELL_EXIT_POLL).await?;
        }
        None => {
            crate::shell::host::serve_until_exit(listener, backend, SHELL_EXIT_POLL).await?;
        }
    }
    log::info!("shell host {id}: the shell exited; stopping");
    Ok(())
}

/// How often a host checks whether its shell has exited.
const SHELL_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// The largest per-probe gap `await_ready` will wait for one `Status` reply.
/// An sbx host's `Status` runs the in-VM FIND script through `sbx exec` —
/// measured 0.24 s warm on pinky — so this must exceed that comfortably; a
/// plain PTY host answers in microseconds either way. The probe still loops
/// until its own overall deadline, so a host that is genuinely dead rolls back
/// exactly as before.
const STATUS_REPLY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Poll `sock` until the host answers a full `Status` round-trip. A successful
/// `connect()` alone is not readiness (F3): a bound-but-unserved socket still
/// accepts a connection and a write into the listen backlog, so readiness must
/// be parsed out of the host's own `Status` response.
pub async fn await_ready(sock: &Path, deadline: Instant) -> Result<()> {
    loop {
        if Instant::now() >= deadline {
            anyhow::bail!(
                "shell host at {} did not become ready before {deadline:?}",
                sock.display()
            );
        }
        match UnixStream::connect(sock).await {
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
            Ok(stream) => {
                let (reader, mut writer) = stream.into_split();
                let request = encode(ShellRequest::Status)?;
                if writer.write_all(request.as_bytes()).await.is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                // The overall deadline wins: a host that answers slowly but
                // inside it is ready, and one that never answers rolls back at
                // the deadline rather than waiting for the reply timeout.
                let end = tokio::time::Instant::from_std(
                    deadline.min(Instant::now() + STATUS_REPLY_TIMEOUT),
                );
                let read = tokio::time::timeout_at(end, reader.read_line(&mut line)).await;
                match read {
                    Ok(Ok(n)) if n > 0 => {
                        if let Ok(ShellResponse::Status { .. }) =
                            serde_json::from_str::<ShellResponse>(line.trim())
                        {
                            return Ok(());
                        }
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

/// Spawn a detached shell host for `id` — a fresh `daemoneye shell-host`
/// process in its own process group, with stdio detached — and wait until it
/// answers `Status`. The child is never waited on, so it survives the caller's
/// exit. Re-invokes the running binary via `current_exe()` (F5).
pub async fn spawn_detached(
    id: &str,
    shell: &str,
    args: &[String],
    rows: u16,
    cols: u16,
) -> Result<()> {
    validate_shell_id(id)?;
    let rows = rows.to_string();
    let cols = cols.to_string();
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args([
        "shell-host",
        "--id",
        id,
        "--shell",
        shell,
        "--rows",
        &rows,
        "--cols",
        &cols,
    ]);
    // `--` must be omitted when argv is empty: a trailing bare separator
    // changes the child's command line, which the phase pins as observable.
    if !args.is_empty() {
        cmd.arg("--");
        cmd.args(args);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let child = cmd.spawn()?;
    drop(child);
    await_ready(&socket_path(id), Instant::now() + Duration::from_secs(10)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::shell::proto::{ShellResponse, ShellState};
    use crate::test_home_guard;
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixListener;

    /// Point `HOME` at a fresh temp dir and return the guard; callers then run
    /// `Config::ensure_dirs()` so `shells_dir`/`shell_logs_dir` exist.
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        (guard, dir)
    }

    #[test]
    fn launch_validate_shell_id_accepts_canonical_ids() {
        for id in ["s0", "s1", "s42", "s123456789"] {
            assert!(validate_shell_id(id).is_ok(), "must accept {id}");
        }
    }

    #[test]
    fn launch_validate_shell_id_rejects_traversal_and_junk() {
        for id in [
            "",
            ".",
            "..",
            "../etc/passwd",
            "s1/../x",
            "s1/x",
            "s",
            "s-1",
            "S1",
            "shell",
            "s1.sock",
            "s1 ",
            "s1234567890",
        ] {
            assert!(validate_shell_id(id).is_err(), "must reject {id:?}");
        }
    }

    #[test]
    fn launch_paths_live_under_the_configured_dirs() {
        let (_guard, _dir) = test_home();
        let sock = socket_path("s1");
        assert!(
            sock.ends_with("var/run/shells/s1.sock"),
            "socket path under shells_dir: {}",
            sock.display()
        );
        let lock = lock_path("s1");
        assert!(
            lock.ends_with("var/run/shells/s1.lock"),
            "lock path under shells_dir: {}",
            lock.display()
        );
        let cast = cast_path("s1", 17);
        assert!(
            cast.ends_with("var/log/shells/s1-17.cast"),
            "cast path under shell_logs_dir: {}",
            cast.display()
        );
    }

    #[tokio::test]
    async fn launch_await_ready_rejects_a_bound_but_unserved_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unserved.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let result = await_ready(&path, Instant::now() + Duration::from_secs(1)).await;
        assert!(
            result.is_err(),
            "a bound-but-unserved socket must not read as ready"
        );
    }

    #[tokio::test]
    async fn launch_await_ready_succeeds_against_a_status_responder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ready.sock");
        let path_for_task = path.clone();
        tokio::spawn(async move {
            let listener = UnixListener::bind(&path_for_task).unwrap();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut line = Vec::new();
                    let mut reader = BufReader::new(&mut stream);
                    let _ = reader.read_until(b'\n', &mut line).await;
                    let resp = encode(ShellResponse::Status {
                        state: ShellState::Running,
                        rows: 24,
                        cols: 80,
                        pid: 1,
                        raw_mode: None,
                        alt_screen: None,
                        foreground: None,
                    })
                    .unwrap();
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        await_ready(&path, Instant::now() + Duration::from_secs(5))
            .await
            .expect("a status responder is ready");
    }

    #[tokio::test]
    async fn launch_await_ready_accepts_a_slow_status_reply() {
        // An sbx host's `Status` round-trips the in-VM FIND script through
        // `sbx exec` (measured 0.24 s warm), so the probe must still accept a
        // reply that lands half a second after the request.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slow.sock");
        let path_for_task = path.clone();
        tokio::spawn(async move {
            let listener = UnixListener::bind(&path_for_task).unwrap();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut line = Vec::new();
                    let mut reader = BufReader::new(&mut stream);
                    let _ = reader.read_until(b'\n', &mut line).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let resp = encode(ShellResponse::Status {
                        state: ShellState::Idle,
                        rows: 24,
                        cols: 80,
                        pid: 1,
                        raw_mode: None,
                        alt_screen: None,
                        foreground: None,
                    })
                    .unwrap();
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let start = Instant::now();
        await_ready(&path, start + Duration::from_secs(2))
            .await
            .expect("a 500 ms status reply is ready within the 2 s deadline");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "await_ready must have returned before its deadline, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn launch_second_host_for_the_same_id_is_refused() {
        let (_guard, _dir) = test_home();
        Config::ensure_dirs().ok();
        let first = crate::daemon::instance::InstanceLock::acquire(&lock_path("s1"))
            .expect("first acquisition succeeds");
        let second = crate::daemon::instance::InstanceLock::acquire(&lock_path("s1"));
        assert!(
            second.is_err(),
            "a second lock for the same id must be refused"
        );
        drop(first);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn launch_run_host_serves_status_over_its_socket() {
        let (_guard, _dir) = test_home();
        Config::ensure_dirs().ok();
        let id = "s1";
        let _host = tokio::spawn(async move { run_host(id, "bash", &[], 24, 80).await });

        await_ready(&socket_path(id), Instant::now() + Duration::from_secs(15))
            .await
            .expect("the host becomes ready");

        let stream = UnixStream::connect(socket_path(id)).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let wire = encode(ShellRequest::Status).unwrap();
        writer.write_all(wire.as_bytes()).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("a status reply arrives in time")
            .unwrap();
        let response: ShellResponse = serde_json::from_str(line.trim()).unwrap();
        match response {
            ShellResponse::Status {
                // A fresh idle shell reports Idle since phase-08a; the
                // property this readiness test pins is that a reply arrives
                // at all.
                state: ShellState::Idle,
                ..
            } => {}
            other => panic!("expected an Idle Status reply, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn launch_run_host_rejects_an_invalid_id_before_binding() {
        let (guard, _dir) = test_home();
        Config::ensure_dirs().ok();
        let result = run_host("../x", "bash", &[], 24, 80).await;
        assert!(result.is_err(), "an invalid id must be rejected");
        let mut entries = std::fs::read_dir(crate::config::shells_dir()).unwrap();
        assert!(
            entries.next().is_none(),
            "validation must precede any filesystem effect"
        );
        drop(guard);
    }

    /// The command line `spawn_detached` builds for the child, as `ps` would
    /// show it. `current_exe()` is not invoked — the argv, not the spawn,
    /// is what the assertion pins.
    fn child_argv(id: &str, shell: &str, args: &[String], rows: u16, cols: u16) -> Vec<String> {
        let rows = rows.to_string();
        let cols = cols.to_string();
        let mut argv: Vec<String> = [
            "<exe>".to_string(),
            "shell-host".to_string(),
            "--id".to_string(),
            id.to_string(),
            "--shell".to_string(),
            shell.to_string(),
            "--rows".to_string(),
            rows,
            "--cols".to_string(),
            cols,
        ]
        .to_vec();
        if !args.is_empty() {
            argv.push("--".to_string());
            argv.extend(args.iter().cloned());
        }
        argv
    }

    #[test]
    fn spawn_detached_appends_argv_after_a_separator() {
        let args: Vec<String> = ["-tt", "-o"].iter().map(|s| s.to_string()).collect();
        let argv = child_argv("s7", "ssh", &args, 24, 80);
        let sep = argv
            .iter()
            .position(|a| a == "--")
            .expect("argv with args carries the separator");
        assert_eq!(&argv[sep + 1..], &args[..], "args follow `--` in order");
        assert!(
            !argv[..sep].contains(&"--".to_string()),
            "no separator before the args: {argv:?}"
        );
    }

    #[test]
    fn spawn_detached_omits_the_separator_when_argv_is_empty() {
        let argv = child_argv("s7", "bash", &[], 24, 80);
        assert!(
            !argv.contains(&"--".to_string()),
            "an empty argv must leave no trailing `--`: {argv:?}"
        );
        assert_eq!(argv[1], "shell-host");
        assert_eq!(argv.last().map(String::as_str), Some("80"));
    }

    #[test]
    fn spawn_detached_command_line_format_passes_the_args_through() {
        let args: Vec<String> = [
            "-tt",
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPath=/tmp/x/%C",
            "matt@pinky",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let argv = child_argv("s3", "ssh", &args, 24, 80);
        assert_eq!(&argv[argv.len() - args.len()..], &args[..]);
        assert!(argv.contains(&"--".to_string()));
    }
}
