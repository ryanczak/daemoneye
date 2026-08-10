use anyhow::Result;
use std::collections::HashMap;
use std::process::Command;

/// Summary of another tmux session returned by [`list_sessions`].
pub struct OtherSessionInfo {
    pub name: String,
    pub windows: usize,
    /// Unix timestamp of last activity across any pane in this session.
    pub last_activity: u64,
    /// True when at least one tmux client is currently attached.
    pub attached: bool,
    /// True when any window in this session is holding an uncleared bell (`!`).
    pub has_bell: bool,
    /// True when any window in this session has unseen activity (`#`).
    pub has_activity: bool,
}

/// Return a list of all tmux sessions visible to the server.
///
/// Uses a single `list-sessions` call.  Returns an empty Vec when tmux is
/// unavailable or no sessions exist.
pub fn list_sessions() -> Vec<OtherSessionInfo> {
    let out = match crate::tmux::bounded_output(Command::new("tmux").args([
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_windows}\t#{session_activity}\t#{session_attached}",
    ])) {
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
            if p.len() < 4 {
                return None;
            }
            Some(OtherSessionInfo {
                name: p[0].to_string(),
                windows: p[1].parse().unwrap_or(0),
                last_activity: p[2].parse().unwrap_or(0),
                attached: p[3] == "1",
                has_bell: false,
                has_activity: false,
            })
        })
        .collect()
}

/// Query bell (`!`) and activity (`#`) flags for every window across all
/// sessions in a single `list-windows -a` call.
///
/// Returns a map of `session_name → (has_bell, has_activity)`.
/// Empty map when tmux is unavailable.
fn list_session_flags() -> HashMap<String, (bool, bool)> {
    let out = match crate::tmux::bounded_output(Command::new("tmux").args([
        "list-windows",
        "-a",
        "-F",
        "#{session_name}\t#{window_flags}",
    ])) {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };
    let mut map: HashMap<String, (bool, bool)> = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let session = match parts.next() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let flags = parts.next().unwrap_or("");
        let entry = map.entry(session.to_string()).or_insert((false, false));
        if flags.contains('!') {
            entry.0 = true;
        }
        if flags.contains('#') {
            entry.1 = true;
        }
    }
    map
}

/// Build a `[OTHER SESSIONS]` context line for the AI, omitting `current_session`.
///
/// Returns an empty string when no other sessions exist.
pub fn other_sessions_context(current_session: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut sessions = list_sessions();
    // Enrich with bell/activity flags from a single list-windows -a call.
    let flags = list_session_flags();
    for s in &mut sessions {
        if let Some(&(bell, activity)) = flags.get(&s.name) {
            s.has_bell = bell;
            s.has_activity = activity;
        }
    }
    format_other_sessions(current_session, &sessions, now)
}

/// Pure formatting helper — separated from tmux I/O for testability.
pub(crate) fn format_other_sessions(
    current_session: &str,
    sessions: &[OtherSessionInfo],
    now_secs: u64,
) -> String {
    let others: Vec<_> = sessions
        .iter()
        .filter(|s| s.name != current_session)
        .collect();

    if others.is_empty() {
        return String::new();
    }

    let parts: Vec<String> = others
        .iter()
        .map(|s| {
            let age = if s.last_activity > 0 && now_secs >= s.last_activity {
                let secs = now_secs - s.last_activity;
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
            let mut alerts = Vec::new();
            if s.has_bell {
                alerts.push("bell!");
            }
            if s.has_activity {
                alerts.push("activity");
            }
            let alert_part = if alerts.is_empty() {
                String::new()
            } else {
                format!(", {}", alerts.join(", "))
            };
            format!(
                "{} ({} window{}, {}, {}{})",
                s.name,
                s.windows,
                if s.windows == 1 { "" } else { "s" },
                age,
                attach_state,
                alert_part,
            )
        })
        .collect();

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

    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "show-environment",
        "-t",
        session,
    ]))?;

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
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        session_name,
        "-p",
        "#{pane_id}",
    ]))?;

    if !output.status.success() {
        anyhow::bail!("Failed to get active pane for session '{}'", session_name);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Return the name of the current tmux session, or `None` if not inside tmux.
pub fn current_session_name() -> Option<String> {
    let out =
        crate::tmux::bounded_output(Command::new("tmux").args(["display-message", "-p", "#S"]))
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
    let out = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        session_name,
        "-p",
        "#{client_width}\t#{client_height}",
    ]));
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

/// Whether the attached tmux client can receive 24-bit color escapes.
///
/// tmux only passes `38;2;R;G;B` SGR sequences through to the outer terminal
/// when it believes that terminal supports them; otherwise it drops or
/// approximates them, which renders as monotone text. That belief comes from
/// three places, checked in order:
///
/// 1. the client's declared feature list (tmux ≥ 3.2 `client_termfeatures`,
///    containing `RGB` — set by the `terminal-features` option matching);
/// 2. the client terminal's terminfo entry (`Tc` / `RGB` flags or the
///    `setrgbf`/`setrgbb` strings) — this is how e.g. `xterm-ghostty` or
///    `xterm-direct` get truecolor, and it is **not** reflected in
///    `client_termfeatures`, so it must be probed separately via `infocmp -x`;
/// 3. an explicit user grant in the `terminal-features` /
///    `terminal-overrides` server options (e.g. the classic
///    `set -ga terminal-overrides ',*:Tc'`).
///
/// On any failure or on an older tmux report `false` so callers fall back to
/// 256-color output, which tmux always handles (reducing further for the
/// client if needed).
pub fn client_supports_rgb() -> bool {
    let features = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-p",
        "#{client_termfeatures}\t#{client_termname}",
    ]));
    let mut termname = String::new();
    if let Ok(out) = features
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        let s = s.trim();
        let (feats, name) = s.split_once('\t').unwrap_or((s, ""));
        if termfeatures_have_rgb(feats) {
            return true;
        }
        termname = name.trim().to_string();
    }

    if !termname.is_empty()
        && let Ok(out) =
            crate::tmux::bounded_output(Command::new("infocmp").args(["-x", &termname]))
        && out.status.success()
        && terminfo_advertises_rgb(&String::from_utf8_lossy(&out.stdout))
    {
        return true;
    }

    for opt in ["terminal-features", "terminal-overrides"] {
        let out =
            crate::tmux::bounded_output(Command::new("tmux").args(["show-options", "-gv", opt]));
        if let Ok(o) = out
            && o.status.success()
        {
            let v = String::from_utf8_lossy(&o.stdout);
            if v.contains("Tc") || v.contains("RGB") {
                return true;
            }
        }
    }
    false
}

/// Does a tmux `client_termfeatures` list (comma-separated) include `RGB`?
fn termfeatures_have_rgb(features: &str) -> bool {
    features
        .split(',')
        .any(|f| f.trim().eq_ignore_ascii_case("RGB"))
}

/// Does an `infocmp -x` dump advertise truecolor? Matches the `Tc` / `RGB`
/// boolean flags and the `setrgbf` / `setrgbb` capability strings as whole
/// comma-separated capability tokens, not substrings.
fn terminfo_advertises_rgb(infocmp_output: &str) -> bool {
    infocmp_output.split([',', '\n']).map(str::trim).any(|cap| {
        cap == "Tc" || cap == "RGB" || cap.starts_with("setrgbf") || cap.starts_with("setrgbb")
    })
}

/// Default name for the headless tmux session used to host ghost incidents.
pub const INCIDENT_SESSION_NAME: &str = "daemoneye-incidents";

/// Ensure a tmux session exists to host an incident window.
///
/// Returns the name of an existing active session if available,
/// otherwise creates a new detached session named `daemoneye-incidents`
/// and returns that name.
pub fn ensure_incident_session() -> Result<String> {
    let sessions = list_sessions();

    // 1. Try to find the most recently active attached session.
    if let Some(s) = sessions
        .iter()
        .filter(|s| s.attached)
        .max_by_key(|s| s.last_activity)
    {
        return Ok(s.name.clone());
    }

    // 2. Try to find any existing session (even detached).
    if let Some(s) = sessions.iter().max_by_key(|s| s.last_activity) {
        return Ok(s.name.clone());
    }

    // 3. Fallback: create a new detached session.
    log::info!(
        "No active tmux sessions found. Creating detached session: {}",
        INCIDENT_SESSION_NAME
    );
    let out = crate::tmux::bounded_output(Command::new("tmux").args([
        "new-session",
        "-d",
        "-s",
        INCIDENT_SESSION_NAME,
    ]))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        anyhow::bail!("Failed to create incident session: {}", err);
    }

    Ok(INCIDENT_SESSION_NAME.to_string())
}

/// Return `true` if a tmux session with this name currently exists.
pub fn session_exists(name: &str) -> bool {
    crate::tmux::bounded_output(Command::new("tmux").args(["has-session", "-t", name]))
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// List all pane IDs in a tmux session (across all windows).
pub fn list_pane_ids_in_session(session: &str) -> Result<Vec<String>> {
    let out = crate::tmux::bounded_output(Command::new("tmux").args([
        "list-panes",
        "-s",
        "-t",
        session,
        "-F",
        "#{pane_id}",
    ]))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(name: &str, windows: usize, last_activity: u64, attached: bool) -> OtherSessionInfo {
        OtherSessionInfo {
            name: name.to_string(),
            windows,
            last_activity,
            attached,
            has_bell: false,
            has_activity: false,
        }
    }

    fn sess_with_flags(
        name: &str,
        windows: usize,
        last_activity: u64,
        attached: bool,
        has_bell: bool,
        has_activity: bool,
    ) -> OtherSessionInfo {
        OtherSessionInfo {
            name: name.to_string(),
            windows,
            last_activity,
            attached,
            has_bell,
            has_activity,
        }
    }

    #[test]
    fn format_other_sessions_empty_when_only_current() {
        let sessions = vec![sess("main", 2, 1000, true)];
        assert_eq!(format_other_sessions("main", &sessions, 1060), "");
    }

    #[test]
    fn format_other_sessions_empty_when_no_sessions() {
        assert_eq!(format_other_sessions("main", &[], 1000), "");
    }

    #[test]
    fn format_other_sessions_active_seconds() {
        let sessions = vec![sess("staging", 1, 990, false)];
        let out = format_other_sessions("main", &sessions, 1000);
        assert!(out.contains("[OTHER SESSIONS]"), "missing header: {out}");
        assert!(out.contains("active 10s ago"), "wrong age format: {out}");
        assert!(out.contains("detached"), "wrong attach state: {out}");
        assert!(out.contains("1 window,"), "wrong window count: {out}");
    }

    #[test]
    fn format_other_sessions_active_minutes() {
        let sessions = vec![sess("prod", 3, 1000 - 300, true)];
        let out = format_other_sessions("current", &sessions, 1000);
        assert!(out.contains("active 5m ago"), "wrong age: {out}");
        assert!(out.contains("attached"), "wrong attach state: {out}");
        assert!(out.contains("3 windows"), "expected plural: {out}");
    }

    #[test]
    fn format_other_sessions_idle_hours() {
        // 2 hours 30 minutes ago: now=10000, last_activity=10000-9000=1000
        let sessions = vec![sess("old", 1, 1000, false)];
        let out = format_other_sessions("current", &sessions, 10000);
        assert!(out.contains("idle 2h30m"), "wrong idle format: {out}");
    }

    #[test]
    fn format_other_sessions_unknown_activity_when_zero() {
        let sessions = vec![sess("fresh", 1, 0, false)];
        let out = format_other_sessions("current", &sessions, 1000);
        assert!(out.contains("unknown activity"), "expected unknown: {out}");
    }

    #[test]
    fn format_other_sessions_excludes_current() {
        let sessions = vec![sess("current", 2, 900, true), sess("other", 1, 950, false)];
        let out = format_other_sessions("current", &sessions, 1000);
        assert!(
            !out.contains("current ("),
            "current session should be excluded: {out}"
        );
        assert!(
            out.contains("other ("),
            "other session should be included: {out}"
        );
    }

    #[test]
    fn format_other_sessions_multiple_sessions_comma_separated() {
        let sessions = vec![sess("a", 1, 990, true), sess("b", 2, 940, false)];
        let out = format_other_sessions("x", &sessions, 1000);
        // Both should appear, separated by ", "
        assert!(out.contains(", "), "expected comma-separated list: {out}");
        assert!(out.contains("a ("), "missing session a: {out}");
        assert!(out.contains("b ("), "missing session b: {out}");
    }

    #[test]
    fn format_other_sessions_ends_with_newline() {
        let sessions = vec![sess("other", 1, 990, true)];
        let out = format_other_sessions("current", &sessions, 1000);
        assert!(
            out.ends_with('\n'),
            "output should end with newline: {out:?}"
        );
    }

    #[test]
    fn format_other_sessions_bell_shown() {
        let sessions = vec![sess_with_flags("staging", 2, 990, true, true, false)];
        let out = format_other_sessions("prod", &sessions, 1000);
        assert!(out.contains("bell!"), "expected bell marker: {out}");
        assert!(!out.contains("activity"), "should not show activity: {out}");
    }

    #[test]
    fn format_other_sessions_activity_shown() {
        let sessions = vec![sess_with_flags("staging", 2, 990, false, false, true)];
        let out = format_other_sessions("prod", &sessions, 1000);
        assert!(out.contains("activity"), "expected activity marker: {out}");
        assert!(!out.contains("bell!"), "should not show bell: {out}");
    }

    #[test]
    fn format_other_sessions_bell_and_activity_shown() {
        let sessions = vec![sess_with_flags("staging", 1, 990, true, true, true)];
        let out = format_other_sessions("prod", &sessions, 1000);
        assert!(out.contains("bell!"), "expected bell: {out}");
        assert!(out.contains("activity"), "expected activity: {out}");
    }

    #[test]
    fn format_other_sessions_no_alert_markers_when_quiet() {
        let sessions = vec![sess("staging", 2, 990, true)];
        let out = format_other_sessions("prod", &sessions, 1000);
        assert!(!out.contains("bell!"), "unexpected bell: {out}");
        assert!(!out.contains("activity"), "unexpected activity: {out}");
    }

    #[test]
    fn termfeatures_rgb_token_matches() {
        assert!(termfeatures_have_rgb("256,RGB,clipboard"));
        assert!(termfeatures_have_rgb("bpaste, rgb ,title"));
        // ghostty's real feature list has no RGB token
        assert!(!termfeatures_have_rgb(
            "bpaste,ccolour,clipboard,cstyle,focus,title"
        ));
        assert!(!termfeatures_have_rgb(""));
    }

    #[test]
    fn terminfo_rgb_caps_match_as_tokens() {
        // xterm-ghostty style: extended Tc flag + setrgbf/setrgbb strings
        assert!(terminfo_advertises_rgb(
            "\tcolors#256, cols#80,\n\tTc, fullkbd,\n\tsetrgbf=\\E[38:2:%p1%d:%p2%d:%p3%dm,"
        ));
        assert!(terminfo_advertises_rgb("\tRGB, colors#0x1000000,"));
        // plain xterm advertises nothing truecolor; "smcup" must not
        // substring-match anything
        assert!(!terminfo_advertises_rgb(
            "\tcolors#8, cols#80,\n\tsmcup=\\E[?1049h, rmcup=\\E[?1049l,"
        ));
    }
}
