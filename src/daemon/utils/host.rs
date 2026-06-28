/// Return the hostname of the machine running the daemon.
pub fn daemon_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Check whether a tmux pane's foreground process is SSH or mosh.
/// Returns a human-readable description if the pane is on a remote host.
pub fn get_pane_remote_host(pane_id: &str) -> Option<String> {
    let out = std::process::Command::new("tmux")
        .args([
            "display-message",
            "-t",
            pane_id,
            "-p",
            "#{pane_current_command}",
        ])
        .output()
        .ok()?;
    let cmd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match cmd.as_str() {
        "ssh" | "mosh-client" | "mosh" => Some(format!("remote (via {})", cmd)),
        _ => None,
    }
}
