//! Persistent per-session foreground pane preferences.
//!
//! Stores the user's chosen target pane (for foreground command execution) keyed
//! by tmux session name in `~/.daemoneye/pane_prefs.json`.  Survives daemon
//! restarts so the user is never asked to pick a pane more than once per session.

use std::collections::HashMap;

fn prefs_path() -> std::path::PathBuf {
    crate::config::var_run_dir().join("pane_prefs.json")
}

fn load_all() -> HashMap<String, String> {
    let path = prefs_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        serde_json::from_str(&text).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

/// Save the preferred target pane for a tmux session.
pub fn save(session_name: &str, pane_id: &str) {
    let path = prefs_path();
    let mut prefs = load_all();
    prefs.insert(session_name.to_string(), pane_id.to_string());
    if let Ok(json) = serde_json::to_string(&prefs) {
        let _ = std::fs::write(&path, json);
    }
}

/// Return the stored target pane for a tmux session, if any.
pub fn get(session_name: &str) -> Option<String> {
    let mut all = load_all();
    all.remove(session_name)
}
