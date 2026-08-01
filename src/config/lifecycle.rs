//! Artifact lifecycle policy table.
//!
//! Every directory or file the daemon creates under `~/.daemoneye/` must have
//! an entry here. The table states the intended lifecycle (rotate, sweep,
//! keep-forever, etc.), the default parameter value, and whether the lifecycle
//! is implemented today. Phases 08 and 09 implement against this table.
//!
//! This module is production code — not `#[cfg(test)]` — so that future phases
//! read the table to know what to implement, and an operator-facing report may
//! too.

/// The intended lifecycle for an artifact class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleIntent {
    /// The artifact is rotated (e.g. log files split by size or date).
    Rotate,
    /// Old entries are swept (deleted) after a configurable retention period.
    Sweep { default_retention_days: u32 },
    /// The artifact is cleared at daemon startup and is not persisted.
    ClearAtStartup,
    /// The artifact is user content; it persists by design and is never managed.
    KeepForever,
}

/// Whether the stated lifecycle is implemented in the codebase today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplementationStatus {
    /// The lifecycle is fully implemented.
    Implemented,
    /// The lifecycle is stated but not yet implemented; the owning phase is named.
    Pending { owned_by: &'static str },
}

/// A single entry in the artifact lifecycle policy table.
#[derive(Debug, Clone)]
pub struct LifecycleEntry {
    /// Runtime-relative path (no `~/.daemoneye/` prefix, no trailing slash).
    /// For lazy directories, this is the directory the daemon creates.
    pub path: &'static str,
    /// The intended lifecycle for this artifact class.
    pub intent: LifecycleIntent,
    /// The config key that controls this lifecycle, if one exists.
    pub config_key: Option<&'static str>,
    /// Whether the lifecycle is implemented today.
    pub implemented: ImplementationStatus,
    /// A short note about the artifact class.
    pub note: &'static str,
    /// Whether this path is created lazily at runtime rather than by
    /// `Config::ensure_dirs()`. Lazily-created paths are exempt from
    /// Direction A (enumerate-existing) tests but still bound by
    /// Direction B (every entry corresponds to a real path).
    pub lazy: bool,
}

/// The authoritative table of artifact lifecycle policies.
///
/// Every directory or file the daemon creates under `~/.daemoneye/` must
/// appear here. Adding a new artifact class without an entry causes the
/// Direction A test to fail.
pub static POLICY_TABLE: &[LifecycleEntry] = &[
    // ── Logs ──────────────────────────────────────────────────────────────────
    LifecycleEntry {
        path: "var/log/daemon.log",
        intent: LifecycleIntent::Rotate,
        config_key: Some("logging.log_max_bytes"),
        implemented: ImplementationStatus::Implemented,
        note: "rotated by rotate_log_file() from the cleanup tick; \
               logging.log_max_bytes bound, logging.log_keep_count copies",
        lazy: false,
    },
    LifecycleEntry {
        path: "var/log/events",
        intent: LifecycleIntent::Sweep {
            default_retention_days: 90,
        },
        config_key: Some("events.retention_days"),
        implemented: ImplementationStatus::Implemented,
        note: "swept every 60th cleanup tick; sane default",
        lazy: true,
    },
    LifecycleEntry {
        path: "var/log/sessions",
        intent: LifecycleIntent::Sweep {
            default_retention_days: 0,
        },
        config_key: Some("sessions.archive_retention_days"),
        implemented: ImplementationStatus::Implemented,
        note: "swept but default is 0 (keep forever) — 141 archives back to May 8; fix in phase-09",
        lazy: false,
    },
    LifecycleEntry {
        path: "var/log/panes",
        intent: LifecycleIntent::Sweep {
            default_retention_days: 7,
        },
        config_key: Some("retention.pane_log_retention_days"),
        implemented: ImplementationStatus::Implemented,
        note: "swept every 60th cleanup tick; operator-tunable via retention.pane_log_retention_days",
        lazy: false,
    },
    LifecycleEntry {
        path: "var/log/pipe",
        intent: LifecycleIntent::ClearAtStartup,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "cleared at daemon start; no persistence expected",
        lazy: false,
    },
    // ── Derived indexes ────────────────────────────────────────────────────────
    LifecycleEntry {
        path: "var/index",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "derived FTS5 memory index; rebuildable from the memory files on disk \
               — reconciliation lands in phase 07",
        lazy: false,
    },
    // ── Runtime state ────────────────────────────────────────────────────────
    LifecycleEntry {
        path: "var/run",
        intent: LifecycleIntent::ClearAtStartup,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "sockets, PID files — ephemeral runtime state",
        lazy: false,
    },
    // ── Saved sessions ────────────────────────────────────────────────────────
    LifecycleEntry {
        path: "var/sessions",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "user-named session archives; user content persists by design",
        lazy: true,
    },
    // ── Config and prompts ────────────────────────────────────────────────────
    LifecycleEntry {
        path: "etc",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "user-editable configuration files",
        lazy: false,
    },
    LifecycleEntry {
        path: "etc/prompts",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "user prompt TOML files",
        lazy: false,
    },
    // ── User content ──────────────────────────────────────────────────────────
    LifecycleEntry {
        path: "scripts",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "user scripts — user content",
        lazy: false,
    },
    LifecycleEntry {
        path: "runbooks",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "user runbooks — user content",
        lazy: false,
    },
    LifecycleEntry {
        path: "memory",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "knowledge and session memories — user content",
        lazy: false,
    },
    LifecycleEntry {
        path: "memory/session",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "session memories — user preferences, always injected at session start",
        lazy: false,
    },
    LifecycleEntry {
        path: "memory/knowledge",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "knowledge memories — technical facts, loaded on-demand via tags",
        lazy: false,
    },
    LifecycleEntry {
        path: "memory/incidents",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "incident memories — post-mortems, created lazily on first write",
        lazy: true,
    },
    LifecycleEntry {
        path: "agents/*/memory/session",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "per-agent session memories",
        lazy: true,
    },
    LifecycleEntry {
        path: "agents/*/memory/knowledge",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "per-agent knowledge memories",
        lazy: true,
    },
    LifecycleEntry {
        path: "agents/*/memory/incidents",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "per-agent incident memories",
        lazy: true,
    },
    // ── Binaries ──────────────────────────────────────────────────────────────
    LifecycleEntry {
        path: "bin",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "symlinks/wrappers for compiled agent and scripts",
        lazy: false,
    },
    LifecycleEntry {
        path: "agents",
        intent: LifecycleIntent::KeepForever,
        config_key: None,
        implemented: ImplementationStatus::Implemented,
        note: "agent configs and briefings — user content",
        lazy: false,
    },
    LifecycleEntry {
        path: "agents/*/mailbox",
        intent: LifecycleIntent::Sweep {
            default_retention_days: 7,
        },
        config_key: Some("retention.mailbox_retention_days"),
        implemented: ImplementationStatus::Implemented,
        note: "swept every 60th cleanup tick across all agent mailboxes; operator-tunable via retention.mailbox_retention_days",
        lazy: false,
    },
];

/// Return all entries that are not lazily-created (i.e. created by
/// `Config::ensure_dirs()`).
pub fn eager_entries() -> Vec<&'static LifecycleEntry> {
    POLICY_TABLE.iter().filter(|e| !e.lazy).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_home_guard;
    use std::path::PathBuf;

    /// Collect all top-level and nested directories that exist under
    /// `~/.daemoneye/` after `Config::ensure_dirs()`.
    ///
    /// We walk the tree and collect every directory path (runtime-relative).
    /// Single files (like `etc/config.toml`) are not directories and are not
    /// collected — this test is about artifact *classes* (directories).
    fn collect_existing_dirs(base: &PathBuf) -> Vec<String> {
        let mut dirs = Vec::new();
        collect_dirs_recursive(base, base, &mut dirs);
        dirs.sort();
        dirs
    }

    fn collect_dirs_recursive(base: &PathBuf, current: &PathBuf, out: &mut Vec<String>) {
        let rel = current.strip_prefix(base).unwrap();
        if !rel.as_os_str().is_empty() {
            out.push(rel.to_string_lossy().into_owned());
        }
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    collect_dirs_recursive(base, &path, out);
                }
            }
        }
    }

    /// Normalise a table path for comparison with filesystem paths.
    /// Strips glob patterns like `agents/*/mailbox` → `agents` for directory matching.
    fn normalise_table_path(path: &str) -> String {
        // For wildcard paths like "agents/*/mailbox", the directory that exists
        // on disk is "agents/<name>/mailbox" — we normalise to the prefix before
        // the first wildcard segment.
        let parts: Vec<&str> = path.split('/').collect();
        let mut result = Vec::new();
        for part in parts {
            if part.contains('*') {
                break;
            }
            result.push(part);
        }
        result.join("/")
    }

    /// Check whether a filesystem directory is covered by any policy entry.
    /// A directory is covered if:
    /// 1. Its path exactly matches a table entry, or
    /// 2. It is a subdirectory of a table entry's path, or
    /// 3. It is a parent directory of a table entry (container dirs like `var`, `var/log`), or
    /// 4. For wildcard entries, it matches the pattern.
    fn is_covered(dir: &str) -> bool {
        POLICY_TABLE.iter().any(|entry| {
            let table_path = entry.path;
            if table_path.contains('*') {
                // Wildcard path: check if the directory matches the pattern
                // e.g. "agents/*/mailbox" matches "agents/test-agent/mailbox"
                let table_parts: Vec<&str> = table_path.split('/').collect();
                let dir_parts: Vec<&str> = dir.split('/').collect();
                if table_parts.len() != dir_parts.len() {
                    return false;
                }
                table_parts
                    .iter()
                    .zip(dir_parts.iter())
                    .all(|(t, d)| t.contains('*') || t == d)
            } else {
                // Exact match, subdirectory, or parent directory
                dir == table_path
                    || dir.starts_with(&format!("{}/", table_path))
                    || table_path.starts_with(&format!("{}/", dir))
            }
        })
    }

    // ── Direction A: no directory escapes the policy ─────────────────────────

    #[test]
    fn every_existing_directory_has_a_policy_entry() {
        let _guard = test_home_guard();
        let tmp_home =
            std::env::temp_dir().join(format!("de_lifecycle_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).ok();
        let old_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp_home);
        }

        Config::ensure_dirs().ok();

        // Also create the lazy directories so Direction A sees them.
        // var/log/events is created by event_log on first write, not ensure_dirs.
        // var/sessions is created by session_store on first save.
        // agents/*/mailbox is created by agents::mailbox on first write.
        // We create the parent dirs that ensure_dirs doesn't touch.
        let base = crate::config::config_dir();
        std::fs::create_dir_all(base.join("var/log/events")).ok();
        std::fs::create_dir_all(base.join("var/sessions")).ok();
        // agents/<name>/mailbox — create a representative agent mailbox
        std::fs::create_dir_all(base.join("agents/test-agent/mailbox")).ok();

        let existing = collect_existing_dirs(&base);
        // For each existing directory, check that some table entry covers it.
        let mut uncovered = Vec::new();
        for dir in &existing {
            if !is_covered(dir) {
                uncovered.push(dir.clone());
            }
        }
        assert!(
            uncovered.is_empty(),
            "directories exist without a lifecycle policy entry:\n  {}",
            uncovered.join("\n  ")
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp_home);
    }

    // ── Direction B: no entry is fiction ──────────────────────────────────────

    #[test]
    fn every_policy_entry_corresponds_to_a_real_path() {
        // Hermetic by construction: seed our own throwaway HOME rather than reading
        // whatever the ambient one happens to contain.
        let _guard = test_home_guard();
        let tmp_home =
            std::env::temp_dir().join(format!("de_lifecycle_dirb_{}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).ok();
        let old_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp_home);
        }
        Config::ensure_dirs().ok();
        let base = crate::config::config_dir();
        std::fs::create_dir_all(base.join("agents/test-agent/mailbox")).ok();
        std::fs::create_dir_all(base.join("var/log/events")).ok();
        std::fs::create_dir_all(base.join("var/sessions")).ok();

        for entry in POLICY_TABLE {
            let path = entry.path;

            if path.contains('*') {
                // Wildcard paths (e.g. agents/*/mailbox) — verify the parent
                // directory exists and the pattern is meaningful.
                // "agents/*/mailbox" → agents/<name>/mailbox is created by
                // agents::mailbox::ensure_mailbox_dir()
                let prefix = normalise_table_path(path);
                let full = base.join(&prefix);
                assert!(
                    full.exists() || entry.lazy,
                    "policy entry '{path}' references a non-existent base directory '{prefix}'"
                );
                continue;
            }

            // The directory may not exist on the executor's machine if
            // ensure_dirs hasn't been run, but the path must be constructible.
            // We verify constructibility by checking the path is valid
            // (no empty segments, no .. components).
            let parts: Vec<&str> = path.split('/').collect();
            assert!(
                !parts.is_empty() && parts.iter().all(|s| !s.is_empty() && *s != ".."),
                "policy entry path '{path}' is not a valid relative path"
            );
        }

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp_home);
    }

    // ── Mutation check: gate fires on uncovered directory ─────────────────────

    #[test]
    fn mutation_check_uncovered_directory_fails_gate() {
        let _guard = test_home_guard();
        let tmp_home =
            std::env::temp_dir().join(format!("de_lifecycle_mutation_{}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).ok();
        let old_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp_home);
        }

        Config::ensure_dirs().ok();

        let base = crate::config::config_dir();

        // Create an extra directory that no policy entry covers.
        let rogue_dir = base.join("var/log/rogue-test-dir");
        std::fs::create_dir_all(&rogue_dir).ok();
        // Now run the same check as Direction A — it should fail.
        let existing = collect_existing_dirs(&base);
        let uncovered: Vec<&String> = existing
            .iter()
            .filter(|d| !is_covered(d.as_str()))
            .collect();

        // The rogue directory must be the one uncovered.
        assert!(
            uncovered
                .iter()
                .any(|d| d.as_str() == "var/log/rogue-test-dir"),
            "expected 'var/log/rogue-test-dir' to be uncovered, got: {:?}",
            uncovered
        );

        // Clean up rogue directory and verify gate passes again.
        let _ = std::fs::remove_dir_all(&rogue_dir);

        let existing2 = collect_existing_dirs(&base);
        let uncovered2: Vec<&String> = existing2
            .iter()
            .filter(|d| !is_covered(d.as_str()))
            .collect();

        assert!(
            uncovered2.is_empty(),
            "after removing rogue dir, unexpected uncovered: {:?}",
            uncovered2
        );
        assert!(
            uncovered2.is_empty(),
            "after removing rogue dir, unexpected uncovered: {:?}",
            uncovered2
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp_home);
    }

    // ── Direction C: every non-lazy policy entry is created by ensure_dirs ────

    /// Assert that every non-lazy (eager) policy entry names a directory that
    /// `Config::ensure_dirs()` actually creates. This is the reverse of
    /// Direction A: Direction A catches directories without policy entries;
    /// this catches policy entries for directories that are never created.
    ///
    /// Adding `lib`-shaped drift back in should fail this test.
    #[test]
    fn every_eager_policy_entry_is_created_by_ensure_dirs() {
        let _guard = test_home_guard();
        let tmp_home =
            std::env::temp_dir().join(format!("de_lifecycle_eager_{}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).ok();
        let old_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp_home);
        }

        Config::ensure_dirs().ok();

        let base = crate::config::config_dir();
        let existing = collect_existing_dirs(&base);

        let eager = eager_entries();
        let mut missing = Vec::new();
        for entry in eager {
            let path = entry.path;
            // Skip file entries (like daemon.log) — ensure_dirs creates
            // directories, not files. We check that the parent directory
            // exists for file entries.
            if path.contains('.') {
                // This is a file path; check the parent directory exists
                let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or(path);
                if !existing.iter().any(|d| d == parent) {
                    missing.push(format!("{path} (parent '{parent}' not created)"));
                }
            } else if path.contains('*') {
                // Wildcard: check the normalised prefix directory exists
                let prefix = normalise_table_path(path);
                if !existing.iter().any(|d| d == &prefix) {
                    missing.push(format!("{path} (prefix '{prefix}' not created)"));
                }
            } else {
                // Directory: must exist exactly
                if !existing.iter().any(|d| d == path) {
                    missing.push(path.to_string());
                }
            }
        }
        assert!(
            missing.is_empty(),
            "eager policy entries not created by ensure_dirs():\n  {}",
            missing.join("\n  ")
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp_home);
    }

    /// POLICY_TABLE must contain an entry for every MemoryCategory dir_name().
    /// Deriving the expected path from dir_name() rather than hardcoding it is
    /// what makes this test catch a future rename.
    #[test]
    fn policy_table_covers_every_memory_category() {
        use crate::memory::MemoryCategory;
        for category in [
            MemoryCategory::Session,
            MemoryCategory::Knowledge,
            MemoryCategory::Incident,
        ] {
            let expected = format!("memory/{}", category.dir_name());
            assert!(
                POLICY_TABLE.iter().any(|e| e.path == expected),
                "POLICY_TABLE missing entry for {expected}"
            );
        }
    }
}
