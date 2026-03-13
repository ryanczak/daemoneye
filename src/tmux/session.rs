use anyhow::Result;
use std::process::Command;
use std::collections::HashMap;

/// Summary of another tmux session returned by [`list_sessions`].
pub struct OtherSessionInfo {
    pub name: String,
    pub windows: usize,
    /// Unix timestamp of last activity across any pane in this session.
    pub last_activity: u64,
    /// True when at least one tmux client is currently attached.
    pub attached: bool,
}

/// Return a list of all tmux sessions visible to the server.
///
/// Uses a single `list-sessions` call.  Returns an empty Vec when tmux is
/// unavailable or no sessions exist.
pub fn list_sessions() -> Vec<OtherSessionInfo> {
    let out = match Command::new("tmux")
        .args([
            "list-sessions", "-F",
            "#{session_name}\t#{session_windows}\t#{session_activity}\t#{session_attached}",
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let p: Vec<&str> = line.splitn(4, '\t').collect();
            if p.len() < 4 { return None; }
            Some(OtherSessionInfo {
                name:          p[0].to_string(),
                windows:       p[1].parse().unwrap_or(0),
                last_activity: p[2].parse().unwrap_or(0),
                attached:      p[3] == "1",
            })
        })
        .collect()
}

/// Build a `[OTHER SESSIONS]` context line for the AI, omitting `current_session`.
///
/// Returns an empty string when no other sessions exist.
pub fn other_sessions_context(current_session: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let sessions = list_sessions();
    let others: Vec<_> = sessions.iter()
        .filter(|s| s.name != current_session)
        .collect();

    if others.is_empty() {
        return String::new();
    }

    let parts: Vec<String> = others.iter().map(|s| {
        let age = if s.last_activity > 0 && now >= s.last_activity {
            let secs = now - s.last_activity;
            if secs < 60 {
                format!("active {}s ago", secs)
            } else if secs < 3600 {
                format!("active {}m ago", secs / 60)
            } else {
                format!("idle {}h{}m", secs / 3600, (secs % 3600) / 60)
            }
        } else {
            "unknown activity".to_string()
        };
        let attach_state = if s.attached { "attached" } else { "detached" };
        format!(
            "{} ({} window{}, {}, {})",
            s.name,
            s.windows,
            if s.windows == 1 { "" } else { "s" },
            age,
            attach_state,
        )
    }).collect();

    format!("[OTHER SESSIONS] {}\n", parts.join(", "))
}

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



/// Return the name of the current tmux session, or `None` if not inside tmux.
pub fn current_session_name() -> Option<String> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Query the dimensions of the terminal client currently attached to `session`.
///
/// Returns `(width, height)` in columns × rows.  Returns `(0, 0)` when no
/// client is attached or when tmux is unavailable — callers should treat
/// `(0, 0)` as "unknown" and skip viewport-sensitive formatting.
pub fn client_dimensions(session_name: &str) -> (u16, u16) {
    let out = Command::new("tmux")
        .args([
            "display-message", "-t", session_name, "-p",
            "#{client_width}\t#{client_height}",
        ])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        _ => return (0, 0),
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    let mut parts = s.splitn(2, '\t');
    let w = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let h = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (w, h)
}

/// List all pane IDs in a tmux session (across all windows).
pub fn list_pane_ids_in_session(session: &str) -> Result<Vec<String>> {
    let out = Command::new("tmux")
        .args(["list-panes", "-s", "-t", session, "-F", "#{pane_id}"])
        .output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}


