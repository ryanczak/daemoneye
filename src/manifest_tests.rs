use super::*;
use crate::util::UnpoisonExt;
use std::env;

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct TmpHome(std::path::PathBuf);
impl TmpHome {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("de_mf_test_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&p).unwrap();
        TmpHome(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TmpHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
    let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
    let old = env::var("HOME").ok();
    unsafe {
        env::set_var("HOME", tmp.path());
    }
    f();
    match old {
        Some(v) => unsafe {
            env::set_var("HOME", v);
        },
        None => unsafe {
            env::remove_var("HOME");
        },
    }
}

const SAMPLE_RUNBOOK: &str = "# Runbook: disk-check\n\n## Alert Criteria\n- Usage > 90%\n";

// --- Phase 1 tests ---

#[test]
fn manifest_empty_stores() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        let m = build_knowledge_manifest();
        assert!(m.is_empty(), "expected empty manifest, got: {m:?}");
    });
}

#[test]
fn manifest_runbooks_with_tags() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook("high-disk-usage", "---\ntags: [disk, linux]\n---\n# Runbook: high-disk-usage\n\n## Alert Criteria\n- usage > 90%\n").unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("high-disk-usage"), "runbook name missing: {m}");
        assert!(m.contains("disk"), "tag missing: {m}");
        assert!(m.contains("linux"), "tag missing: {m}");
        assert!(m.contains("## Available Knowledge"), "header missing: {m}");
    });
}

#[test]
fn manifest_scripts_listed() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::scripts::write_script("cleanup-logs.sh", "#!/bin/bash\necho done").unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("cleanup-logs.sh"), "script name missing: {m}");
    });
}

#[test]
fn manifest_knowledge_and_incidents() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::memory::add_memory(
            "prod-db-hosts",
            "db1, db2",
            crate::memory::MemoryCategory::Knowledge,
        )
        .unwrap();
        crate::memory::add_memory(
            "2026-02-incident",
            "db failover",
            crate::memory::MemoryCategory::Incident,
        )
        .unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("prod-db-hosts"), "knowledge key missing: {m}");
        assert!(m.contains("2026-02-incident"), "incident key missing: {m}");
        assert!(
            m.contains("Knowledge memories"),
            "knowledge section missing: {m}"
        );
        assert!(m.contains("Incidents"), "incidents section missing: {m}");
    });
}

#[test]
fn manifest_excludes_session_memories() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::memory::add_memory(
            "my-session-pref",
            "dark mode",
            crate::memory::MemoryCategory::Session,
        )
        .unwrap();
        let m = build_knowledge_manifest();
        assert!(
            !m.contains("my-session-pref"),
            "session memory should not appear: {m}"
        );
        // Should be empty since only session memory exists
        assert!(m.is_empty(), "expected empty manifest: {m:?}");
    });
}

#[test]
fn manifest_caps_at_1kb() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        // Write many knowledge memories to exceed 1KB
        for i in 0..50 {
            crate::memory::add_memory(
                &format!("very-long-key-name-for-testing-{:02}", i),
                "value",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
        }
        let m = build_knowledge_manifest();
        assert!(m.len() <= 1024, "manifest exceeds 1KB: {} bytes", m.len());
        assert!(m.contains("(+"), "should contain truncation marker: {m}");
        assert!(m.contains("more)"), "should contain '(+N more)': {m}");
    });
}

#[test]
fn manifest_mixed_stores() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook(
            "deploy-rollback",
            "# Runbook: deploy-rollback\n\n## Alert Criteria\n- deploy failed\n",
        )
        .unwrap();
        crate::scripts::write_script("rotate-certs.sh", "#!/bin/bash\necho done").unwrap();
        crate::memory::add_memory(
            "monitoring-stack",
            "prometheus+grafana",
            crate::memory::MemoryCategory::Knowledge,
        )
        .unwrap();
        crate::memory::add_memory(
            "2026-01-outage",
            "details",
            crate::memory::MemoryCategory::Incident,
        )
        .unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("Runbooks"), "missing runbooks section: {m}");
        assert!(m.contains("Scripts"), "missing scripts section: {m}");
        assert!(
            m.contains("Knowledge memories"),
            "missing knowledge section: {m}"
        );
        assert!(m.contains("Incidents"), "missing incidents section: {m}");
    });
}

// --- Phase 4 tests ---

#[test]
fn auto_search_matches_runbook_name() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook("high-disk-usage", SAMPLE_RUNBOOK).unwrap();
        let result = auto_search_context("disk is filling up", "");
        assert!(
            result.contains("high-disk-usage"),
            "runbook name missing: {result}"
        );
        assert!(
            result.contains("## Auto-loaded Knowledge"),
            "header missing: {result}"
        );
    });
}

#[test]
fn auto_search_matches_runbook_tag() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook(
            "ssl-renewal",
            "---\ntags: [certs, ssl]\n---\n# Runbook: ssl-renewal\n\n## Alert Criteria\n- cert expires\n",
        ).unwrap();
        let result = auto_search_context("ssl certificate is expiring", "");
        assert!(
            result.contains("ssl-renewal"),
            "runbook name missing from tag match: {result}"
        );
    });
}

#[test]
fn auto_search_matches_memory_key() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::memory::add_memory(
            "prod-db-hosts",
            "db1.internal, db2.internal",
            crate::memory::MemoryCategory::Knowledge,
        )
        .unwrap();
        let result = auto_search_context("connect to prod-db-hosts", "");
        assert!(
            result.contains("prod-db-hosts"),
            "memory key missing: {result}"
        );
    });
}

#[test]
fn auto_search_respects_4kb_cap() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        // Write a large runbook
        let big_content = format!(
            "# Runbook: big-runbook\n\n## Alert Criteria\n- x\n\n{}",
            "detailed content\n".repeat(500)
        );
        crate::runbook::write_runbook("big-runbook", &big_content).unwrap();
        let result = auto_search_context("big-runbook", "");
        assert!(
            result.len() <= 4096 + 200,
            "output exceeds 4KB cap: {} bytes",
            result.len()
        );
    });
}

#[test]
fn auto_search_empty_on_no_match() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook("memory-leak", SAMPLE_RUNBOOK).unwrap();
        let result = auto_search_context("disk space is low", "");
        assert!(result.is_empty(), "expected no match: {result}");
    });
}

#[test]
fn auto_search_deduplicates() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook(
            "disk-check",
            "---\ntags: [disk]\n---\n# Runbook: disk-check\n\n## Alert Criteria\n- usage > 90%\n",
        ).unwrap();
        // "disk" matches both name and tag — should appear only once
        let result = auto_search_context("disk is full", "");
        let count = result.matches("### Runbook: disk-check").count();
        assert_eq!(count, 1, "runbook should appear exactly once: {result}");
    });
}

#[test]
fn auto_search_max_three_items() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        for i in 0..5 {
            crate::memory::add_memory(
                &format!("key-{i}"),
                &format!("content for key-{i}"),
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
        }
        let result = auto_search_context("key-0 key-1 key-2 key-3 key-4", "");
        let count = result.matches("### Knowledge:").count();
        assert!(
            count <= 3,
            "should load at most 3 items, got {count}: {result}"
        );
    });
}

// --- Phase 5 tests ---

#[test]
fn related_hints_matches_runbook_name() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook("high-disk-usage", SAMPLE_RUNBOOK).unwrap();
        let hints = related_knowledge_hints(
            "Disk usage is at 95% on /dev/sda1. high-disk-usage threshold exceeded.",
        );
        assert!(
            hints.contains("runbook \"high-disk-usage\""),
            "hint missing: {hints}"
        );
        assert!(
            hints.starts_with("[Related knowledge:"),
            "wrong format: {hints}"
        );
    });
}

#[test]
fn related_hints_matches_tags() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook(
            "disk-check",
            "---\ntags: [disk, storage]\n---\n# Runbook: disk-check\n\n## Alert Criteria\n- usage > 90%\n",
        ).unwrap();
        let hints = related_knowledge_hints("disk usage is high on the storage volume");
        assert!(
            hints.contains("runbook \"disk-check\""),
            "tag-based hint missing: {hints}"
        );
    });
}

#[test]
fn related_hints_caps_at_three() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        for i in 0..5 {
            crate::memory::add_memory(
                &format!("key-{i}"),
                "v",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
        }
        let output = "key-0 key-1 key-2 key-3 key-4";
        let hints = related_knowledge_hints(output);
        // Count occurrences of "memory \""
        let count = hints.matches("memory \"").count();
        assert!(
            count <= 3,
            "hints should cap at 3 items, got {count}: {hints}"
        );
    });
}

#[test]
fn related_hints_empty_on_no_match() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::runbook::write_runbook("memory-leak-check", SAMPLE_RUNBOOK).unwrap();
        let hints = related_knowledge_hints("some unrelated command output");
        assert!(hints.is_empty(), "expected no hints: {hints}");
    });
}

#[test]
fn related_hints_excludes_scripts() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::scripts::write_script("cleanup-logs.sh", "#!/bin/bash\necho done").unwrap();
        let hints = related_knowledge_hints("cleanup-logs.sh ran successfully");
        assert!(
            !hints.contains("script"),
            "scripts should be excluded from hints: {hints}"
        );
    });
}

// --- Phase 6 tests (tag-aware) ---

#[test]
fn manifest_shows_memory_tags() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        // Write memory with frontmatter tags
        crate::memory::add_memory(
            "webhook-setup",
            "---\ntags: [webhook, alertmanager]\n---\nSetup instructions here",
            crate::memory::MemoryCategory::Knowledge,
        )
        .unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("webhook-setup"), "key missing: {m}");
        assert!(m.contains("webhook"), "tag missing: {m}");
        assert!(m.contains("alertmanager"), "tag missing: {m}");
    });
}

#[test]
fn manifest_shows_script_tags() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        let content = "#!/bin/bash\n\
                       # --- daemoneye ---\n\
                       # tags: [certs, ssl]\n\
                       # --- /daemoneye ---\n\
                       echo done\n";
        crate::scripts::write_script("rotate-certs.sh", content).unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("rotate-certs.sh"), "script missing: {m}");
        assert!(m.contains("certs"), "tag missing: {m}");
    });
}

#[test]
fn auto_search_matches_memory_tags() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        crate::memory::add_memory(
            "postgres-config",
            "---\ntags: [postgres, database]\n---\nConnection string: ...",
            crate::memory::MemoryCategory::Knowledge,
        )
        .unwrap();
        let result = auto_search_context("postgres is down", "");
        assert!(
            result.contains("postgres-config"),
            "tag-based memory match missing: {result}"
        );
    });
}

// --- Phase 3 tests ---

#[test]
fn manifest_shows_summary() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        // Write memory file with summary frontmatter directly
        let dir = crate::config::config_dir().join("memory").join("knowledge");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("db-quirks.md"),
            "---\nsummary: Postgres runs on port 5433 not 5432\ntags: [postgres]\n---\nSome content\n",
        ).unwrap();
        let m = build_knowledge_manifest();
        assert!(m.contains("db-quirks"), "key missing: {m}");
        assert!(m.contains("5433"), "summary missing from manifest: {m}");
    });
}

#[test]
fn auto_search_matches_summary_text() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("memory").join("knowledge");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("db-quirks.md"),
            "---\nsummary: Postgres runs on port 5433 not 5432\n---\nDetailed connection info here.\n",
        ).unwrap();
        // Query contains a word from the summary
        let result = auto_search_context("connection failed postgres port", "");
        assert!(
            result.contains("db-quirks"),
            "summary-based match missing: {result}"
        );
    });
}

#[test]
fn auto_search_follows_relates_to_links() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("memory").join("knowledge");
        std::fs::create_dir_all(&dir).unwrap();
        // Primary memory: matches query directly
        std::fs::write(
            dir.join("db-hosts.md"),
            "---\nrelates_to: [db-quirks]\n---\ndb1.internal, db2.internal\n",
        )
        .unwrap();
        // Related memory: should be pulled in via relates_to even without direct match
        std::fs::write(
            dir.join("db-quirks.md"),
            "---\n---\nPostgres runs on port 5433\n",
        )
        .unwrap();
        let result = auto_search_context("db-hosts connection string", "");
        assert!(
            result.contains("db-hosts"),
            "primary match missing: {result}"
        );
        assert!(
            result.contains("db-quirks"),
            "relates_to link not followed: {result}"
        );
    });
}

#[test]
fn hints_matches_summary_text() {
    let tmp = TmpHome::new();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("memory").join("knowledge");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("redis-config.md"),
            "---\nsummary: Redis cluster failover procedure\n---\nDetails here.\n",
        )
        .unwrap();
        // Output contains a word from the summary
        let hints = related_knowledge_hints("redis failover detected in cluster logs");
        assert!(
            hints.contains("redis-config"),
            "summary-based hint missing: {hints}"
        );
    });
}
