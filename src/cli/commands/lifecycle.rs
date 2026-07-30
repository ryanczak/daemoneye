//! Small daemon lifecycle CLI commands: `logs`, `stop`, `ping`.

use anyhow::Result;
use std::path::PathBuf;
use tokio::io::BufReader;

use crate::config::default_pid_path;
use crate::daemon::instance::read_pid;
use crate::daemon::{DaemonLiveness, daemon_liveness};
use crate::ipc::{Request, Response};

use super::ipc_client::{connect, recv, send_request};

/// One line describing what a probe found, for `ping` / `stop` / `status`.
/// `pid` is the PID-file payload, used only to distinguish a wedged daemon from
/// an absent one.
pub fn liveness_line(liveness: DaemonLiveness, pid: Option<u32>) -> String {
    match (liveness, pid) {
        (DaemonLiveness::NotRunning, Some(p)) => {
            format!("Daemon is not running (stale PID file names PID {p}).")
        }
        (DaemonLiveness::NotRunning, None) => "Daemon is not running.".into(),
        (DaemonLiveness::Unresponsive, Some(p)) => {
            format!(
                "Daemon PID {p} is alive but not answering \
                 — it may be wedged. Check ~/.daemoneye/var/log/daemon.log."
            )
        }
        (DaemonLiveness::Unresponsive, None) => "Daemon is listening but not answering \
             — it may be wedged. Check ~/.daemoneye/var/log/daemon.log."
            .into(),
        (DaemonLiveness::Confused, Some(p)) => {
            format!("Daemon PID {p} answered with an unexpected reply.")
        }
        (DaemonLiveness::Confused, None) => "Daemon answered with an unexpected reply.".into(),
        (DaemonLiveness::Running, _) => "Daemon is running.".into(),
    }
}

pub fn run_logs(path: PathBuf) -> Result<()> {
    if !path.exists() {
        eprintln!("No log file found at {}.", path.display());
        eprintln!("The daemon writes logs there by default when started with: daemoneye daemon");
        std::process::exit(1);
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("tail")
        .args(["-f", path.to_str().unwrap_or("")])
        .exec();
    anyhow::bail!("Failed to exec tail: {}", err)
}

pub async fn run_stop() -> Result<()> {
    match connect().await {
        Err(_) => {
            let liveness = daemon_liveness().await;
            let pid = read_pid(&default_pid_path());
            eprintln!("{}", liveness_line(liveness, pid));
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Shutdown).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon stopped."),
                _ => {
                    println!("Daemon did not respond to shutdown.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ping() -> Result<()> {
    let liveness = daemon_liveness().await;
    let pid = read_pid(&default_pid_path());
    let line = liveness_line(liveness, pid);
    match liveness {
        DaemonLiveness::Running => {
            println!("{}", line);
            Ok(())
        }
        _ => {
            eprintln!("{}", line);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_line_reports_wedged_with_pid() {
        let line = liveness_line(DaemonLiveness::Unresponsive, Some(4321));
        assert!(line.contains("PID 4321"));
        assert!(line.contains("wedged"));
    }

    #[test]
    fn liveness_line_reports_wedged_without_pid() {
        let line = liveness_line(DaemonLiveness::Unresponsive, None);
        assert!(line.contains("wedged"));
        assert!(!line.contains("PID"));
    }

    #[test]
    fn liveness_line_distinguishes_stale_pid_file() {
        let with_pid = liveness_line(DaemonLiveness::NotRunning, Some(4321));
        assert!(with_pid.contains("stale PID file"));
        let without_pid = liveness_line(DaemonLiveness::NotRunning, None);
        assert_eq!(without_pid, "Daemon is not running.");
    }

    #[test]
    fn liveness_line_running_ignores_pid() {
        let with_pid = liveness_line(DaemonLiveness::Running, Some(1));
        let without_pid = liveness_line(DaemonLiveness::Running, None);
        assert_eq!(with_pid, "Daemon is running.");
        assert_eq!(without_pid, "Daemon is running.");
    }
}
