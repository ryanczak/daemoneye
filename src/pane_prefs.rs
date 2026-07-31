//! Persistent per-session foreground pane preferences.
//!
//! Stores the user's chosen target pane (for foreground command execution) keyed
//! by tmux session name in `var/run/pane_prefs.json` (under the daemon data
//! directory).  Survives daemon restarts so the user is never asked to pick a
//! pane more than once per session.
//!
//! Each entry carries a fingerprint (window name + working directory) so that a
//! recycled pane ID from a previous tmux server is not accepted silently.

use std::collections::HashMap;

/// Fingerprint stored alongside a pane ID to validate that the pane is still
/// the one the user chose.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PanePreference {
    pub pane_id: String,
    pub window_name: String,
    pub current_path: String,
}

/// The three live pane facts the fingerprint check needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePane {
    pub pane_id: String,
    pub window_name: String,
    pub current_path: String,
}

fn prefs_path() -> std::path::PathBuf {
    crate::config::var_run_dir().join("pane_prefs.json")
}

fn load_all() -> HashMap<String, PanePreference> {
    let path = prefs_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        // Try new format first.
        if let Ok(map) = serde_json::from_str::<HashMap<String, PanePreference>>(&text) {
            return map;
        }
        // Old format: {session: "pane_id"}. Entries are treated as absent
        // (silently discarded) — the user gets prompted once and a new entry
        // is written.
        if serde_json::from_str::<HashMap<String, String>>(&text).is_ok() {
            return HashMap::new();
        }
    }
    HashMap::new()
}

fn save_all(prefs: &HashMap<String, PanePreference>) {
    let path = prefs_path();
    if let Ok(json) = serde_json::to_string(prefs) {
        let _ = std::fs::write(&path, json);
    }
}

/// Fetch the live pane facts from tmux, returning only the fields the
/// fingerprint check needs.
fn live_panes() -> Option<Vec<LivePane>> {
    Some(
        crate::tmux::list_panes_detailed()
            .ok()?
            .into_iter()
            .map(|p| LivePane {
                pane_id: p.pane_id,
                window_name: p.window_name,
                current_path: p.current_path,
            })
            .collect(),
    )
}

/// Check whether a live pane matches a stored fingerprint.
///
/// This is the pure comparison behind the safety property: a stored preference
/// is accepted only when the live pane's window name and working directory
/// still match what was recorded.
pub fn matches(
    stored: &PanePreference,
    pane_id: &str,
    window_name: &str,
    current_path: &str,
) -> bool {
    stored.pane_id == pane_id
        && stored.window_name == window_name
        && stored.current_path == current_path
}

/// Pure pruning: keep only entries whose pane still exists and matches its
/// fingerprint.
pub fn prune_map(
    prefs: &HashMap<String, PanePreference>,
    panes: &[LivePane],
) -> HashMap<String, PanePreference> {
    let mut kept = HashMap::new();
    for (session, stored) in prefs {
        if let Some(live) = panes.iter().find(|p| p.pane_id == stored.pane_id)
            && matches(stored, &live.pane_id, &live.window_name, &live.current_path)
        {
            kept.insert(session.clone(), stored.clone());
        }
    }
    kept
}

/// Pure lookup: return the stored pane ID only if the live pane still matches
/// the recorded fingerprint.
pub fn get_from(
    prefs: &HashMap<String, PanePreference>,
    session_name: &str,
    panes: &[LivePane],
) -> Option<String> {
    let stored = prefs.get(session_name)?;
    let live = panes.iter().find(|p| p.pane_id == stored.pane_id)?;
    if matches(stored, &live.pane_id, &live.window_name, &live.current_path) {
        Some(stored.pane_id.clone())
    } else {
        None
    }
}

/// Save the preferred target pane for a tmux session.
///
/// Fetches the live pane's fingerprint from tmux at call time, so callers only
/// need to supply the session name and pane ID. Prunes stale entries in the
/// same pass since live pane data is already available and we are already
/// writing.
pub fn save(session_name: &str, pane_id: &str) {
    // Fetch the fingerprint from the live pane. If the pane is gone by the
    // time we save, skip — there is nothing meaningful to record.
    let panes = match crate::tmux::list_panes_detailed() {
        Ok(p) => p,
        Err(_) => return,
    };

    let pref = match panes.iter().find(|p| p.pane_id == pane_id) {
        Some(p) => PanePreference {
            pane_id: pane_id.to_string(),
            window_name: p.window_name.clone(),
            current_path: p.current_path.clone(),
        },
        None => return,
    };

    // Convert to LivePane for the pure prune.
    let live: Vec<LivePane> = panes
        .into_iter()
        .map(|p| LivePane {
            pane_id: p.pane_id,
            window_name: p.window_name,
            current_path: p.current_path,
        })
        .collect();

    // Prune stale entries, then insert the new one.
    let mut prefs = prune_map(&load_all(), &live);
    prefs.insert(session_name.to_string(), pref);
    save_all(&prefs);
}

/// Return the stored target pane for a tmux session, if the fingerprint still
/// matches the live pane.
///
/// Returns `None` when:
/// - No preference is stored for the session.
/// - The stored pane no longer exists.
/// - The stored pane exists but its window name or working directory has changed.
pub fn get(session_name: &str) -> Option<String> {
    // A read. Pruning happens in `save()`, which already holds live pane data
    // and is already writing — so a lookup never mutates the store.
    let prefs = load_all();
    let panes = live_panes()?;
    get_from(&prefs, session_name, &panes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pref(pane_id: &str, window_name: &str, current_path: &str) -> PanePreference {
        PanePreference {
            pane_id: pane_id.to_string(),
            window_name: window_name.to_string(),
            current_path: current_path.to_string(),
        }
    }

    // --- matches() ---

    #[test]
    fn matches_when_all_fields_equal() {
        let stored = make_pref("%3", "main", "/home/user");
        assert!(matches(&stored, "%3", "main", "/home/user"));
    }

    #[test]
    fn does_not_match_when_window_name_changed() {
        let stored = make_pref("%3", "main", "/home/user");
        assert!(!matches(&stored, "%3", "edited", "/home/user"));
    }

    #[test]
    fn does_not_match_when_current_path_changed() {
        let stored = make_pref("%3", "main", "/home/user");
        assert!(!matches(&stored, "%3", "main", "/tmp"));
    }

    #[test]
    fn does_not_match_when_pane_id_changed() {
        let stored = make_pref("%3", "main", "/home/user");
        assert!(!matches(&stored, "%99", "main", "/home/user"));
    }

    // --- old-format tolerance ---

    #[test]
    fn load_all_tolerates_old_format() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Create the var/run dir and write an old-format file.
        let run_dir = tmp.path().join(".daemoneye/var/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let old_json = r#"{"my-session":"%3","other":"%0"}"#;
        std::fs::write(run_dir.join("pane_prefs.json"), old_json).unwrap();

        // Should load without panicking and return an empty map (old entries
        // treated as absent).
        let result = load_all();
        assert!(result.is_empty());

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn load_all_tolerates_garbage_json() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let run_dir = tmp.path().join(".daemoneye/var/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("pane_prefs.json"), "not json at all").unwrap();

        let result = load_all();
        assert!(result.is_empty());

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn load_all_tolerates_missing_file() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let result = load_all();
        assert!(result.is_empty());

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn load_all_reads_new_format() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let run_dir = tmp.path().join(".daemoneye/var/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let new_json =
            r#"{"my-session":{"pane_id":"%3","window_name":"main","current_path":"/home/user"}}"#;
        std::fs::write(run_dir.join("pane_prefs.json"), new_json).unwrap();

        let result = load_all();
        assert_eq!(result.len(), 1);
        let pref = result.get("my-session").unwrap();
        assert_eq!(pref.pane_id, "%3");
        assert_eq!(pref.window_name, "main");
        assert_eq!(pref.current_path, "/home/user");

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn get_does_not_mutate_stored_map() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let run_dir = tmp.path().join(".daemoneye/var/run");
        std::fs::create_dir_all(&run_dir).unwrap();

        // Write a prefs file with one matching entry and one stale entry.
        let prefs: HashMap<String, PanePreference> = HashMap::from([
            (
                "my-session".to_string(),
                make_pref("%3", "main", "/home/user"),
            ),
            (
                "stale-session".to_string(),
                make_pref("%99", "gone", "/nowhere"),
            ),
        ]);
        save_all(&prefs);

        // Snapshot the file contents before calling get_from.
        let before = std::fs::read_to_string(prefs_path()).unwrap();

        // Build a LivePane list by hand — only the matching pane exists.
        let panes = vec![LivePane {
            pane_id: "%3".to_string(),
            window_name: "main".to_string(),
            current_path: "/home/user".to_string(),
        }];

        // get_from is pure — it should return the matching pane.
        let result = get_from(&prefs, "my-session", &panes);
        assert_eq!(result.as_deref(), Some("%3"));

        // The on-disk file must be byte-identical — get_from never writes.
        let after = std::fs::read_to_string(prefs_path()).unwrap();
        assert_eq!(before, after, "get_from must not modify the on-disk file");

        // Separately verify prune_map drops the stale entry.
        let pruned = prune_map(&prefs, &panes);
        assert_eq!(pruned.len(), 1, "prune_map should drop stale entries");
        assert!(
            pruned.contains_key("my-session"),
            "prune_map should keep matching entries"
        );
        assert!(
            !pruned.contains_key("stale-session"),
            "prune_map should drop entries whose pane no longer exists"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn save_roundtrips_fingerprint() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // save() calls list_panes_detailed() which needs tmux — so we can't
        // fully exercise save() without tmux. Instead, verify that save_all
        // and load_all roundtrip correctly.
        let run_dir = tmp.path().join(".daemoneye/var/run");
        std::fs::create_dir_all(&run_dir).unwrap();

        let prefs: HashMap<String, PanePreference> = HashMap::from([
            ("sess1".to_string(), make_pref("%3", "main", "/home/user")),
            ("sess2".to_string(), make_pref("%7", "edit", "/tmp")),
        ]);
        save_all(&prefs);

        let loaded = load_all();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("sess1").unwrap().pane_id, "%3");
        assert_eq!(loaded.get("sess2").unwrap().current_path, "/tmp");

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
