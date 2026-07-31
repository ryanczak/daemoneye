//! Path-audit gate for agent-facing assets.
//!
//! Extracts backtick-delimited path literals from prompt and knowledge-memory
//! text, normalises them, and checks them against a hand-maintained inventory
//! that records each path's status (`Current` or `Legacy`).
//!
//! This module is production code — phase 04 ships `daemoneye audit-prompts`
//! and reuses it without inventing a second extractor.

/// Whether a path in the inventory is still valid for agent-facing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStatus {
    /// The path is constructed by the running daemon and may appear in assets.
    Current,
    /// The path is no longer the authoritative one. Assets must not present it
    /// as the current location.
    Legacy { reason: &'static str },
}

/// A single entry in the path inventory.
#[derive(Debug, Clone)]
pub struct InventoryEntry {
    /// Runtime-relative path (no `~/.daemoneye/` prefix, no trailing slash).
    pub path: &'static str,
    /// Whether this path is still valid.
    pub status: PathStatus,
    /// Short note naming what constructs it (function or source line).
    pub source: &'static str,
}

/// The authoritative table of known runtime-relative paths.
///
/// Every path the `config::` module constructs must appear here. Paths that
/// appear in agent-facing assets but are not listed are flagged as unknown.
pub static INVENTORY: &[InventoryEntry] = &[
    InventoryEntry {
        path: "etc",
        status: PathStatus::Current,
        source: "config::etc_dir()",
    },
    InventoryEntry {
        path: "etc/prompts",
        status: PathStatus::Current,
        source: "config::prompts_dir()",
    },
    InventoryEntry {
        path: "etc/config.toml",
        status: PathStatus::Current,
        source: "config::seeds (seeded from assets/etc/config.toml)",
    },
    InventoryEntry {
        path: "etc/prompts/sre.toml",
        status: PathStatus::Current,
        source: "config::seeds (seeded from assets/prompts/sre.toml)",
    },
    InventoryEntry {
        path: "var/run",
        status: PathStatus::Current,
        source: "config::var_run_dir()",
    },
    InventoryEntry {
        path: "var/run/schedules.json",
        status: PathStatus::Current,
        source: "Config::schedules_path()",
    },
    InventoryEntry {
        path: "var/run/daemoneye.sock",
        status: PathStatus::Current,
        source: "config::default_socket_path()",
    },
    InventoryEntry {
        path: "var/run/daemoneye.pid",
        status: PathStatus::Current,
        source: "config::default_pid_path()",
    },
    InventoryEntry {
        path: "var/log",
        status: PathStatus::Current,
        source: "config::var_log_dir()",
    },
    InventoryEntry {
        path: "var/log/daemon.log",
        status: PathStatus::Current,
        source: "config::default_log_path()",
    },
    InventoryEntry {
        path: "var/log/events",
        status: PathStatus::Current,
        source: "config::events_dir()",
    },
    InventoryEntry {
        path: "var/log/panes",
        status: PathStatus::Current,
        source: "config::pane_logs_dir()",
    },
    InventoryEntry {
        path: "var/log/pipe",
        status: PathStatus::Current,
        source: "config::pipe_log_dir()",
    },
    InventoryEntry {
        path: "var/log/sessions",
        status: PathStatus::Current,
        source: "config::sessions_dir()",
    },
    InventoryEntry {
        path: "var/sessions",
        status: PathStatus::Current,
        source: "session_store.rs:51",
    },
    InventoryEntry {
        path: "var/sessions/index.json",
        status: PathStatus::Current,
        source: "session_store.rs (index.json inside var/sessions)",
    },
    InventoryEntry {
        path: "bin",
        status: PathStatus::Current,
        source: "config::bin_dir()",
    },
    InventoryEntry {
        path: "memory",
        status: PathStatus::Current,
        source: "memory::memory_dir_for_namespace(\"global\", …)",
    },
    InventoryEntry {
        path: "runbooks",
        status: PathStatus::Current,
        source: "Config::runbooks_dir()",
    },
    InventoryEntry {
        path: "scripts",
        status: PathStatus::Current,
        source: "Config::scripts_dir()",
    },
    InventoryEntry {
        path: "agents",
        status: PathStatus::Current,
        source: "agents::agents_dir()",
    },
    // Legacy: superseded by dated segments (current_event_segment_path);
    // retained only as a compatibility read at event_log.rs:93.
    InventoryEntry {
        path: "var/log/events.jsonl",
        status: PathStatus::Legacy {
            reason: "superseded by dated segments (current_event_segment_path); retained only as a compatibility read at event_log.rs:93",
        },
        source: "config::events_path()",
    },
];

/// Literals the audit skips because they are real defects owned by a named phase.
///
/// This list is currently empty. When a phase introduces a known defect (e.g. a
/// stale path in an asset), it adds the normalised literal here as a temporary
/// quarantine and owns its removal. A non-empty entry is a temporary quarantine
/// owned by a named phase.
pub static PENDING_FIX: &[&str] = &[];

/// A reason why a path literal was flagged by the audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingReason {
    /// The normalised literal is not in the inventory.
    Unknown,
    /// The literal is in the inventory but marked as legacy.
    Legacy { reason: &'static str },
}

/// A single audit finding for a bad path literal.
#[derive(Debug, Clone)]
pub struct Finding {
    /// The original literal as it appeared in the text (after trimming).
    pub literal: String,
    /// Why it was flagged.
    pub reason: FindingReason,
}

/// Leading segments that identify a backtick-delimited span as a path literal.
const PATH_PREFIXES: &[&str] = &[
    "~/.daemoneye/",
    "etc/",
    "var/",
    "bin/",
    "memory/",
    "runbooks/",
    "scripts/",
    "prompts/",
    "agents/",
    "sessions/",
];

/// Extract backtick-delimited path literals from text.
///
/// A span is kept only if, after trimming whitespace, it starts with one of
/// the recognised path prefixes. Everything else (slash commands, shebangs,
/// tool names, etc.) is discarded.
pub fn extract_path_literals(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = text.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c == '`' {
            // Consume the opening backtick
            chars.next();
            let mut span = String::new();
            let mut on_line = true;
            while let Some(&ch) = chars.peek() {
                if ch == '`' {
                    chars.next(); // consume closing backtick
                    break;
                }
                if ch == '\n' {
                    on_line = false;
                    break;
                }
                span.push(ch);
                chars.next();
            }
            if on_line {
                let trimmed = span.trim();
                if PATH_PREFIXES.iter().any(|&pfx| trimmed.starts_with(pfx)) {
                    found.push(trimmed.to_string());
                }
            }
        } else {
            chars.next();
        }
    }

    found
}

/// Normalise a path literal for inventory lookup.
///
/// Returns `None` if the result is the runtime root (empty string after
/// normalisation), meaning the literal refers to the root itself and is
/// always valid.
fn normalise(literal: &str) -> Option<String> {
    let mut s = literal.to_string();

    // 1. Strip leading ~/.daemoneye/ (with or without trailing slash)
    if s.starts_with("~/.daemoneye/") {
        s = s["~/.daemoneye/".len()..].to_string();
    } else if s == "~/.daemoneye" {
        // Bare root without trailing slash
        return None;
    }

    // 2. Drop trailing /
    s = s.trim_end_matches('/').to_string();

    // 3. Truncate at first segment containing < or >
    let mut result = Vec::new();
    for seg in s.split('/') {
        if seg.contains('<') || seg.contains('>') {
            break;
        }
        result.push(seg);
    }

    let normalised = result.join("/");

    // Empty string means runtime root — always valid, no finding
    if normalised.is_empty() {
        None
    } else {
        Some(normalised)
    }
}
/// Classification of a single path literal against the inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathClassification {
    /// The literal normalises to a `Current` inventory entry.
    Current,
    /// The literal normalises to a `Legacy` inventory entry.
    Superseded { reason: &'static str },
    /// The literal normalises to something not in the inventory at all.
    Unknown,
}

/// Classify every extracted path literal in `text` against the inventory.
///
/// Returns one entry per extracted literal — including the good ones — so the
/// caller can render a full report. Literals that normalise to `None` (the bare
/// runtime root, e.g. `~/.daemoneye`) are omitted because they are always valid.
pub fn classify_text(text: &str) -> Vec<(String, PathClassification)> {
    let literals = extract_path_literals(text);
    let mut results = Vec::new();

    for literal in literals {
        let normalised = match normalise(&literal) {
            Some(n) => n,
            None => continue, // runtime root — always valid, omitted
        };

        let classification = match INVENTORY.iter().find(|e| e.path == normalised) {
            None => PathClassification::Unknown,
            Some(entry) => match entry.status {
                PathStatus::Current => PathClassification::Current,
                PathStatus::Legacy { reason } => PathClassification::Superseded { reason },
            },
        };

        results.push((literal, classification));
    }

    results
}

/// Audit `text` against the inventory, skipping literals listed in `pending`.
///
/// Tests pass `&[]` to reproduce the unquarantined (red) audit.
pub fn audit_text_with(text: &str, pending: &[&str]) -> Vec<Finding> {
    let classified = classify_text(text);
    let mut findings = Vec::new();

    for (literal, classification) in classified {
        // Determine the normalised form for quarantine lookup
        let normalised = match normalise(&literal) {
            Some(n) => n,
            None => continue,
        };

        // Skip literals in the quarantine list (owned by phase 03)
        if pending.iter().any(|&p| p == normalised) {
            continue;
        }

        match classification {
            PathClassification::Current => {}
            PathClassification::Superseded { reason } => {
                findings.push(Finding {
                    literal,
                    reason: FindingReason::Legacy { reason },
                });
            }
            PathClassification::Unknown => {
                findings.push(Finding {
                    literal,
                    reason: FindingReason::Unknown,
                });
            }
        }
    }

    findings
}

/// Audit `text` with the standing quarantine list applied.
pub fn audit_text(text: &str) -> Vec<Finding> {
    audit_text_with(text, PENDING_FIX)
}

/// Audit a single asset by name, returning the extracted literal count and findings.
pub fn audit_asset(_name: &str, text: &str) -> (usize, Vec<Finding>) {
    let literals = extract_path_literals(text);
    let findings = audit_text(text);
    (literals.len(), findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::config::seeds::{
        AGENT_RUNTIME_LAYOUT_MEMORY, GHOST_SHELL_GUIDE_MEMORY, RUNBOOK_FORMAT_MEMORY,
        RUNBOOK_GHOST_TEMPLATE_MEMORY, SCHEDULING_GUIDE_MEMORY, SCRIPTS_AND_SUDOERS_MEMORY,
        SRE_PROMPT_TOML, WEBHOOK_SETUP_MEMORY,
    };
    use std::path::PathBuf;

    // ── Extraction: positive cases ──────────────────────────────────────────

    #[test]
    fn extracts_real_path_spans() {
        let must_extract = [
            "~/.daemoneye/",
            "var/log/events.jsonl",
            "~/.daemoneye/config.toml",
            "var/log/daemon.log",
            "~/.daemoneye/daemon.log",
            "var/log/panes/<name>.log",
            "~/.daemoneye/events.jsonl",
            "var/log/pipe/<id>.log",
            "~/.daemoneye/pane_logs/",
            "var/log/sessions/<id>.jsonl",
            "~/.daemoneye/pane_logs/<win_name>.log",
            "var/run/schedules.json",
            "~/.daemoneye/schedules.json",
            "var/sessions/index.json",
            "~/.daemoneye/scripts/",
            "var/sessions/<name>/",
            "~/.daemoneye/agents/<name>/briefing.md",
            "etc/config.toml",
            "agents/<name>/mailbox/<job_id>.json",
            "etc/prompts/sre.toml",
            "memory/",
            "runbooks/",
            "scripts/",
            "bin/",
            "var/run/",
            "var/log/pipe/",
        ];

        for span in &must_extract {
            let text = format!("see `{span}` for details");
            let found = extract_path_literals(&text);
            assert!(
                found.iter().any(|s| s == span),
                "expected `{span}` to be extracted, got {found:?}",
            );
        }
    }

    // ── Extraction: negative cases ──────────────────────────────────────────

    #[test]
    fn rejects_slash_commands_and_shebangs() {
        let must_not_extract = [
            "/clear",
            "/refresh",
            "/limits",
            "/limits reset",
            "/prompt <name>",
            "/session list",
            "/session save <name> [desc]",
            "/session load <name>",
            "/approvals revoke commands",
            "//",
            "#!/usr/bin/env python3",
            "[Ghost Shell Completed/Failed]",
        ];

        for span in &must_not_extract {
            let text = format!("try `{span}`");
            let found = extract_path_literals(&text);
            assert!(
                !found.iter().any(|s| s == span),
                "expected `{span}` to NOT be extracted, got {found:?}",
            );
        }
    }

    // ── Normalisation ───────────────────────────────────────────────────────

    #[test]
    fn normalisation_collapses_placeholder_segments() {
        assert_eq!(
            normalise("var/log/panes/<name>.log"),
            Some("var/log/panes".to_string())
        );
        assert_eq!(
            normalise("var/sessions/<name>/"),
            Some("var/sessions".to_string())
        );
        assert_eq!(
            normalise("agents/<name>/mailbox/<job_id>.json"),
            Some("agents".to_string())
        );
        // Bare runtime root → None (always valid)
        assert_eq!(normalise("~/.daemoneye/"), None);
        assert_eq!(normalise("~/.daemoneye"), None);
    }

    #[test]
    fn normalisation_strips_prefix_and_trailing_slash() {
        assert_eq!(
            normalise("~/.daemoneye/scripts/"),
            Some("scripts".to_string())
        );
        assert_eq!(
            normalise("~/.daemoneye/var/log/daemon.log"),
            Some("var/log/daemon.log".to_string())
        );
        assert_eq!(
            normalise("etc/prompts/sre.toml"),
            Some("etc/prompts/sre.toml".to_string())
        );
        assert_eq!(normalise("var/run/"), Some("var/run".to_string()));
    }

    // ── Legacy detection ────────────────────────────────────────────────────

    #[test]
    fn legacy_entry_is_reported() {
        let findings = audit_text_with("see `var/log/events.jsonl` for events", &[]);
        assert_eq!(findings.len(), 1, "expected one finding, got {findings:?}");
        assert!(
            matches!(findings[0].reason, FindingReason::Legacy { .. }),
            "expected Legacy, got {:?}",
            findings[0].reason
        );
    }

    // ── Inventory completeness ──────────────────────────────────────────────
    #[test]
    fn inventory_contains_all_config_constructors() {
        // Every path the config:: module constructs must appear in INVENTORY.
        // We derive the runtime-relative form by stripping the config_dir prefix.
        let constructors: Vec<fn() -> PathBuf> = vec![
            crate::config::etc_dir,
            crate::config::var_run_dir,
            crate::config::var_log_dir,
            crate::config::pipe_log_dir,
            crate::config::pane_logs_dir,
            crate::config::bin_dir,
            crate::config::default_log_path,
            crate::config::default_socket_path,
            crate::config::default_pid_path,
            crate::config::events_path,
            crate::config::events_dir,
            crate::config::prompts_dir,
            crate::config::sessions_dir,
            Config::scripts_dir,
            Config::runbooks_dir,
            Config::schedules_path,
        ];

        let base = crate::config::config_dir();
        for ctor in &constructors {
            let full = ctor();
            let rel = full
                .strip_prefix(&base)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Normalise: drop trailing /, truncate at placeholder segments
            let rel = normalise(&rel).unwrap_or(rel);
            assert!(
                INVENTORY.iter().any(|e| e.path == rel),
                "config constructor produces '{rel}' but it is not in INVENTORY",
            );
        }

        // current_event_segment_path produces a dynamic filename (events-YYYYMMDD.jsonl).
        // Its parent directory is var/log/events, which is already in INVENTORY.
        let segment = crate::config::current_event_segment_path();
        let rel = segment
            .strip_prefix(&base)
            .map(|p| p.parent().unwrap().to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = normalise(&rel).unwrap_or(rel);
        assert!(
            INVENTORY.iter().any(|e| e.path == rel),
            "current_event_segment_path parent '{rel}' not in INVENTORY",
        );
    }

    // ── Shipped assets audit ────────────────────────────────────────────────

    /// The 7 historical path literals that were quarantined in PENDING_FIX.
    /// Frozen as test data so the extractor's detection capability is preserved
    /// even after the real assets are corrected.
    const HISTORICAL_STALE_LITERALS: &[&str] = &[
        "`var/log/events.jsonl`",
        "`~/.daemoneye/config.toml`",
        "`~/.daemoneye/daemon.log`",
        "`~/.daemoneye/events.jsonl`",
        "`~/.daemoneye/pane_logs/`",
        "`~/.daemoneye/schedules.json`",
        "`~/.daemoneye/sessions/ghost-<name>-<uuid>.jsonl`",
    ];

    #[test]
    fn all_assets_audit_clean_with_empty_quarantine() {
        let assets: &[(&str, &str)] = &[
            ("SRE_PROMPT_TOML", SRE_PROMPT_TOML),
            ("webhook-setup", WEBHOOK_SETUP_MEMORY),
            ("runbook-format", RUNBOOK_FORMAT_MEMORY),
            ("runbook-ghost-template", RUNBOOK_GHOST_TEMPLATE_MEMORY),
            ("ghost-shell-guide", GHOST_SHELL_GUIDE_MEMORY),
            ("scheduling-guide", SCHEDULING_GUIDE_MEMORY),
            ("scripts-and-sudoers", SCRIPTS_AND_SUDOERS_MEMORY),
            ("agent-runtime-layout", AGENT_RUNTIME_LAYOUT_MEMORY),
        ];

        for (name, text) in assets {
            let findings = audit_text_with(text, &[]);
            assert!(
                findings.is_empty(),
                "asset '{name}' has findings (empty quarantine):\n{}",
                findings
                    .iter()
                    .map(|f| format!("  {} → {:?}", f.literal, f.reason))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }

    #[test]
    fn extractor_detects_historical_stale_literals() {
        // Freeze the 7 historical defects as a synthetic corpus so the
        // extractor's detection capability is preserved after the real
        // assets are corrected.
        let corpus = HISTORICAL_STALE_LITERALS.join(" ");
        let findings = audit_text_with(&corpus, &[]);
        assert_eq!(
            findings.len(),
            7,
            "expected 7 findings from historical corpus, got {}",
            findings.len()
        );

        let mut flagged: Vec<String> = findings
            .iter()
            .filter_map(|f| normalise(&f.literal))
            .collect();
        flagged.sort();

        let mut expected = vec![
            "config.toml".to_string(),
            "daemon.log".to_string(),
            "events.jsonl".to_string(),
            "pane_logs".to_string(),
            "schedules.json".to_string(),
            "sessions".to_string(),
            "var/log/events.jsonl".to_string(),
        ];
        expected.sort();

        assert_eq!(
            flagged, expected,
            "flagged normalised literals must match the 7 historical defects"
        );
    }

    // ── classify_text ────────────────────────────────────────────────────────

    #[test]
    fn classify_text_returns_current_for_good_literal() {
        let text = "see `var/log/daemon.log` for details";
        let results = classify_text(text);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "var/log/daemon.log");
        assert_eq!(results[0].1, PathClassification::Current);
    }

    #[test]
    fn classify_text_returns_superseded_with_reason() {
        // `var/log/events.jsonl` is the Legacy entry in INVENTORY.
        let text = "see `var/log/events.jsonl` for details";
        let results = classify_text(text);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "var/log/events.jsonl");
        assert!(matches!(
            results[0].1,
            PathClassification::Superseded { .. }
        ));
        if let PathClassification::Superseded { reason } = results[0].1 {
            assert!(!reason.is_empty(), "superseded reason must not be empty");
        }
    }

    #[test]
    fn classify_text_returns_unknown_for_invented_path() {
        let text = "see `var/log/does-not-exist.log` for details";
        let results = classify_text(text);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "var/log/does-not-exist.log");
        assert_eq!(results[0].1, PathClassification::Unknown);
    }

    #[test]
    fn classify_text_omits_bare_runtime_root() {
        let text = "see `~/.daemoneye` for details";
        let results = classify_text(text);
        assert!(
            results.is_empty(),
            "bare runtime root should be omitted, got {results:?}",
        );
    }

    #[test]
    fn classify_text_reports_all_literals_including_good_ones() {
        // Mix of current, superseded, and unknown
        let text = concat!(
            "see `var/log/daemon.log` ",
            "and `var/log/events.jsonl` ",
            "and `var/log/does-not-exist.log`"
        );
        let results = classify_text(text);
        assert_eq!(results.len(), 3);

        // Current
        let current = results.iter().find(|(l, _)| l == "var/log/daemon.log");
        assert!(current.is_some(), "current literal missing");
        assert_eq!(current.unwrap().1, PathClassification::Current);

        // Superseded
        let superseded = results.iter().find(|(l, _)| l == "var/log/events.jsonl");
        assert!(superseded.is_some(), "superseded literal missing");
        assert!(matches!(
            superseded.unwrap().1,
            PathClassification::Superseded { .. }
        ));

        // Unknown
        let unknown = results
            .iter()
            .find(|(l, _)| l == "var/log/does-not-exist.log");
        assert!(unknown.is_some(), "unknown literal missing");
        assert_eq!(unknown.unwrap().1, PathClassification::Unknown);
    }
}
