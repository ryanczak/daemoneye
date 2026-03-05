use anyhow::Result;
use std::process::Command;
use std::collections::HashMap;

/// Fetch the tmux session environment and return high-signal variables.
///
/// Only variables on the allowlist are returned.  Values are passed back
/// as-is; callers should run them through `mask_sensitive` before sending to
/// the AI.  Lines prefixed with `-` (unset variables) are skipped.
pub fn session_environment(session: &str) -> Result<HashMap<String, String>> {
    const ALLOWLIST: &[&str] = &[
        // Cloud / infra
        "AWS_PROFILE",
        "AWS_DEFAULT_REGION",
        "AWS_REGION",
        "KUBECONFIG",
        "KUBE_CONTEXT",
        "KUBECTL_CONTEXT",
        "VAULT_ADDR",
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        // App environment tier
        "ENVIRONMENT",
        "APP_ENV",
        "NODE_ENV",
        "RAILS_ENV",
        "RACK_ENV",
        // Language runtimes
        "VIRTUAL_ENV",
        "CONDA_DEFAULT_ENV",
        "GOPATH",
        "GOENV",
        "JAVA_HOME",
        // Locale
        "LANG",
        "LC_ALL",
    ];

    let output = Command::new("tmux")
        .args(["show-environment", "-t", session])
        .output()?;

    // Not a hard error if unavailable (e.g. session not found).
    if !output.status.success() {
        return Ok(HashMap::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut env = HashMap::new();
    for line in stdout.lines() {
        if line.starts_with('-') {
            continue; // variable unset in this session
        }
        if let Some(eq) = line.find('=') {
            let key = &line[..eq];
            let val = &line[eq + 1..];
            if ALLOWLIST.contains(&key) {
                env.insert(key.to_string(), val.to_string());
            }
        }
    }
    Ok(env)
}

/// Check if a tmux session exists.
pub fn has_session(session_name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Create a new detached tmux session.
pub fn create_session(session_name: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["new-session", "-d", "-s", session_name])
        .output()?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create tmux session '{}': {}", session_name, err);
    }

    Ok(())
}

/// Get the active pane ID in `#{pane_id}` format (e.g. `%5`).
pub fn get_active_pane(session_name: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", session_name, "-p", "#{pane_id}"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to get active pane for session '{}'", session_name);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}


pub fn install_passive_activity_hook(
    pane_id: &str,
    session: &str,
) -> Result<()> {
    // 1. Turn on monitor-activity for the window containing this pane
    let out0 = Command::new("tmux")
        .args(["set-window-option", "-t", pane_id, "monitor-activity", "on"])
        .output()?;
    if !out0.status.success() {
        anyhow::bail!("Failed to enable monitor-activity for pane '{}'", pane_id);
    }

    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    // 0 is a dummy hook index since we don't strictly need one for passive watching
    let cmd = format!(
        "run-shell -b '{} notify activity {} 0 \"{}\"'",
        exe_path, pane_id, session
    );

    let out1 = Command::new("tmux")
        .args(["set-hook", "-t", pane_id, "alert-activity", &cmd])
        .output()?;
    if !out1.status.success() {
        anyhow::bail!("Failed to install alert-activity hook for pane '{}'", pane_id);
    }

    let out2 = Command::new("tmux")
        .args(["set-hook", "-t", pane_id, "alert-silence", &cmd])
        .output()?;
    if !out2.status.success() {
        anyhow::bail!("Failed to install alert-silence hook for pane '{}'", pane_id);
    }

    Ok(())
}

pub fn remove_passive_activity_hook(pane_id: &str) -> Result<()> {
    let _ = Command::new("tmux")
        .args(["set-window-option", "-t", pane_id, "monitor-activity", "off"])
        .output();
    let _ = Command::new("tmux")
        .args(["set-hook", "-u", "-t", pane_id, "alert-activity"])
        .output();
    let _ = Command::new("tmux")
        .args(["set-hook", "-u", "-t", pane_id, "alert-silence"])
        .output();
    Ok(())
}

