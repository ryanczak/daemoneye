use super::*;
use crate::memory::UpdateMemoryArgs;
use crate::util::UnpoisonExt;
use std::env;

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct TmpHome(std::path::PathBuf);
impl TmpHome {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("de_mem_test_{}_{}", std::process::id(), n));
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

fn temp_home() -> TmpHome {
    TmpHome::new()
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

#[test]
fn add_and_read_memory() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "user_prefs",
            "Prefers verbose output",
            MemoryCategory::Session,
            "global",
        )
        .unwrap();
        let val = read_memory("user_prefs", MemoryCategory::Session, "global").unwrap();
        assert_eq!(val, "Prefers verbose output");
    });
}

#[test]
fn add_memory_upsert() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("key1", "first", MemoryCategory::Knowledge, "global").unwrap();
        add_memory("key1", "second", MemoryCategory::Knowledge, "global").unwrap();
        let val = read_memory("key1", MemoryCategory::Knowledge, "global").unwrap();
        assert_eq!(val, "second");
    });
}

#[test]
fn delete_memory_removes_file() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "to_delete",
            "some content",
            MemoryCategory::Session,
            "global",
        )
        .unwrap();
        delete_memory("to_delete", MemoryCategory::Session, "global").unwrap();
        assert!(read_memory("to_delete", MemoryCategory::Session, "global").is_err());
    });
}

#[test]
fn list_memories_returns_all_categories() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("sess_key", "s", MemoryCategory::Session, "global").unwrap();
        add_memory("know_key", "k", MemoryCategory::Knowledge, "global").unwrap();
        add_memory("inc_key", "i", MemoryCategory::Incident, "global").unwrap();
        let all = list_memories(None, &["global"]).unwrap();
        let cats: Vec<_> = all.iter().map(|(_, c, _)| c.as_str()).collect();
        assert!(cats.contains(&"session"));
        assert!(cats.contains(&"knowledge"));
        assert!(cats.contains(&"incident"));
    });
}

#[test]
fn memory_frontmatter_tags_parsed() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "tagged-key",
            "---\ntags: [postgres, production]\n---\nActual content",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        let info = infos
            .iter()
            .find(|m| m.key == "tagged-key")
            .expect("key not found");
        assert!(
            info.tags.contains(&"postgres".to_string()),
            "tag missing: {:?}",
            info.tags
        );
        assert!(
            info.tags.contains(&"production".to_string()),
            "tag missing: {:?}",
            info.tags
        );
    });
}

#[test]
fn memory_without_frontmatter_has_no_tags() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "plain-key",
            "Just plain content",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        let info = infos
            .iter()
            .find(|m| m.key == "plain-key")
            .expect("key not found");
        assert!(info.tags.is_empty(), "expected no tags: {:?}", info.tags);
    });
}

#[test]
fn list_memories_with_tags_returns_all() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("k1", "v1", MemoryCategory::Knowledge, "global").unwrap();
        add_memory(
            "k2",
            "---\ntags: [foo]\n---\nv2",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        assert_eq!(infos.len(), 2);
        let k2 = infos.iter().find(|m| m.key == "k2").unwrap();
        assert_eq!(k2.tags, vec!["foo"]);
    });
}

#[test]
fn memory_frontmatter_summary_parsed() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "meta-key",
            "---\ntags: [foo]\nsummary: \"A useful description\"\nrelates_to: [other-key, runbook-x]\ncreated: \"2026-01-01T00:00:00Z\"\nupdated: \"2026-03-31T12:00:00Z\"\nexpires: \"2026-12-31\"\n---\nBody content",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        let info = infos
            .iter()
            .find(|m| m.key == "meta-key")
            .expect("key not found");
        assert_eq!(info.summary.as_deref(), Some("A useful description"));
        assert_eq!(info.relates_to, vec!["other-key", "runbook-x"]);
        assert_eq!(info.created.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(info.updated.as_deref(), Some("2026-03-31T12:00:00Z"));
        assert_eq!(info.expires.as_deref(), Some("2026-12-31"));
    });
}

#[test]
fn memory_without_frontmatter_has_empty_metadata() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("bare", "Just content", MemoryCategory::Knowledge, "global").unwrap();
        let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        let info = infos
            .iter()
            .find(|m| m.key == "bare")
            .expect("key not found");
        assert!(info.summary.is_none());
        assert!(info.relates_to.is_empty());
        assert!(info.expires.is_none());
    });
}

#[test]
fn build_frontmatter_roundtrip() {
    let tags = vec!["postgres".to_string(), "database".to_string()];
    let relates_to = vec!["runbook-x".to_string()];
    let fm = build_frontmatter(
        "global",
        &tags,
        Some("Primary DB hosts"),
        &relates_to,
        Some("2026-01-01T00:00:00Z"),
        Some("2026-03-31T00:00:00Z"),
        None,
    );
    assert!(fm.starts_with("---\n"));
    assert!(fm.contains("tags:"));
    assert!(fm.contains("postgres"));
    assert!(fm.contains("summary:"));
    assert!(fm.contains("Primary DB hosts"));
    assert!(fm.contains("relates_to:"));
    assert!(fm.contains("runbook-x"));
    assert!(fm.contains("created:"));
    assert!(
        !fm.contains("expires:"),
        "expires should be omitted when None"
    );
    // Round-trip: parse what we built
    let full = format!("{}Body text", fm);
    let (parsed, body) = parse_memory_frontmatter(&full);
    assert_eq!(parsed.tags, tags);
    assert_eq!(parsed.summary.as_deref(), Some("Primary DB hosts"));
    assert_eq!(parsed.relates_to, relates_to);
    assert_eq!(body, "Body text");
}

#[test]
fn build_frontmatter_empty_returns_empty_string() {
    let fm = build_frontmatter("global", &[], None, &[], None, None, None);
    assert!(fm.is_empty());
}

#[test]
fn update_memory_creates_new_entry() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let tags = vec!["foo".to_string()];
        update_memory(UpdateMemoryArgs {
            key: "new-key",
            category: MemoryCategory::Knowledge,
            body: Some("initial body"),
            append: false,
            tags: Some(&tags),
            summary: Some("A summary"),
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .unwrap();
        let raw = read_memory("new-key", MemoryCategory::Knowledge, "global").unwrap();
        assert!(raw.contains("initial body"));
        assert!(raw.contains("foo"));
        assert!(raw.contains("A summary"));
        assert!(raw.contains("created:"));
        assert!(raw.contains("updated:"));
    });
}

#[test]
fn update_memory_partial_update_preserves_other_fields() {
    let tmp = temp_home();
    with_home(&tmp, || {
        // Write initial entry with full frontmatter.
        add_memory(
            "existing",
            "---\ntags: [alpha, beta]\nsummary: \"original summary\"\nrelates_to: [\"other\"]\n---\nOriginal body",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        // Update only summary.
        update_memory(UpdateMemoryArgs {
            key: "existing",
            category: MemoryCategory::Knowledge,
            body: None,
            append: false,
            tags: None,
            summary: Some("new summary"),
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .unwrap();
        let raw = read_memory("existing", MemoryCategory::Knowledge, "global").unwrap();
        // Tags and relates_to preserved.
        assert!(raw.contains("alpha"), "tags should be preserved: {raw}");
        assert!(
            raw.contains("other"),
            "relates_to should be preserved: {raw}"
        );
        // Summary updated.
        assert!(
            raw.contains("new summary"),
            "summary should be updated: {raw}"
        );
        // Body preserved.
        assert!(
            raw.contains("Original body"),
            "body should be preserved: {raw}"
        );
    });
}

#[test]
fn update_memory_append_mode() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "append-key",
            "First line",
            MemoryCategory::Session,
            "global",
        )
        .unwrap();
        update_memory(UpdateMemoryArgs {
            key: "append-key",
            category: MemoryCategory::Session,
            body: Some("Second line"),
            append: true,
            tags: None,
            summary: None,
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .unwrap();
        let raw = read_memory("append-key", MemoryCategory::Session, "global").unwrap();
        assert!(raw.contains("First line"), "original body missing: {raw}");
        assert!(raw.contains("Second line"), "appended body missing: {raw}");
    });
}

#[test]
fn update_memory_replace_body() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("replace-key", "Old body", MemoryCategory::Session, "global").unwrap();
        update_memory(UpdateMemoryArgs {
            key: "replace-key",
            category: MemoryCategory::Session,
            body: Some("New body"),
            append: false,
            tags: None,
            summary: None,
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .unwrap();
        let raw = read_memory("replace-key", MemoryCategory::Session, "global").unwrap();
        assert!(!raw.contains("Old body"), "old body should be gone: {raw}");
        assert!(raw.contains("New body"), "new body missing: {raw}");
    });
}

#[test]
fn update_memory_sets_updated_timestamp() {
    let tmp = temp_home();
    with_home(&tmp, || {
        update_memory(UpdateMemoryArgs {
            key: "ts-key",
            category: MemoryCategory::Knowledge,
            body: Some("body"),
            append: false,
            tags: None,
            summary: None,
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .unwrap();
        let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        let info = infos
            .iter()
            .find(|m| m.key == "ts-key")
            .expect("key not found");
        assert!(info.updated.is_some(), "updated timestamp should be set");
        assert!(info.created.is_some(), "created timestamp should be set");
    });
}

#[test]
fn session_memory_block_respects_cap() {
    let tmp = temp_home();
    with_home(&tmp, || {
        // Write many large entries to exceed SESSION_MEMORY_CAP (32 768 bytes)
        for i in 0..50 {
            let content = "x".repeat(1000);
            add_memory(
                &format!("entry_{:02}", i),
                &content,
                MemoryCategory::Session,
                "global",
            )
            .unwrap();
        }
        let block = load_session_memory_block(&["global"]);
        assert!(
            block.len() <= 32_768 + 200,
            "block should be capped near 32 KB"
        );
        assert!(block.contains("omitted"), "should mention omitted entries");
        assert!(
            block.contains("entry_"),
            "should name at least one omitted key"
        );
    });
}

// ── Namespace tests ──────────────────────────────────────────────────────────

#[test]
fn frontmatter_defaults_to_global() {
    let raw = "---\ntags: [foo]\n---\nBody";
    let (fm, body) = parse_memory_frontmatter(raw);
    assert_eq!(fm.namespace, "global");
    assert_eq!(body, "Body");
}

#[test]
fn frontmatter_parses_namespace() {
    let raw = "---\nnamespace: \"postgres-dba\"\ntags: [db]\n---\nBody";
    let (fm, body) = parse_memory_frontmatter(raw);
    assert_eq!(fm.namespace, "postgres-dba");
    assert_eq!(body, "Body");
}

#[test]
fn build_frontmatter_includes_non_global_namespace() {
    let fm = build_frontmatter("postgres-dba", &[], None, &[], None, None, None);
    assert!(fm.contains("namespace: \"postgres-dba\""));
}

#[test]
fn build_frontmatter_omits_global_namespace() {
    let fm = build_frontmatter("global", &[], None, &[], None, None, None);
    assert!(fm.is_empty());
}

#[test]
fn write_agent_reads_agent() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "agent-key",
            "agent value",
            MemoryCategory::Knowledge,
            "analyst",
        )
        .unwrap();
        let val = read_memory("agent-key", MemoryCategory::Knowledge, "analyst").unwrap();
        assert_eq!(val, "agent value");
    });
}

#[test]
fn write_agent_invisible_to_global() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("secret", "agent-only", MemoryCategory::Knowledge, "analyst").unwrap();
        // Reading from global should fail.
        assert!(read_memory("secret", MemoryCategory::Knowledge, "global").is_err());
    });
}

#[test]
fn fallback_to_global() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "shared",
            "global value",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        // Agent writes to its own namespace.
        add_memory(
            "agent-only",
            "agent value",
            MemoryCategory::Knowledge,
            "analyst",
        )
        .unwrap();
        // List with agent+global namespaces should find both.
        let infos =
            list_memories_with_tags(Some(MemoryCategory::Knowledge), &["analyst", "global"])
                .unwrap();
        let shared = infos
            .iter()
            .find(|m| m.key == "shared")
            .expect("shared not found");
        assert_eq!(shared.namespace, "global");
        let agent_only = infos
            .iter()
            .find(|m| m.key == "agent-only")
            .expect("agent-only not found");
        assert_eq!(agent_only.namespace, "analyst");
    });
}

#[test]
fn migrate_namespace_adds_missing() {
    let tmp = temp_home();
    with_home(&tmp, || {
        // Write a memory file without namespace.
        let dir = memory_dir_for_namespace("global", &MemoryCategory::Knowledge);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old-memory.md");
        std::fs::write(&path, "---\ntags: [old]\n---\nOld content").unwrap();

        migrate_namespace().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("namespace: global"));
    });
}

#[test]
fn migrate_namespace_skips_already_migrated() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = memory_dir_for_namespace("global", &MemoryCategory::Knowledge);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("migrated.md");
        std::fs::write(&path, "---\nnamespace: global\ntags: [x]\n---\nBody").unwrap();

        migrate_namespace().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        // Should not have double namespace.
        let count = raw.matches("namespace:").count();
        assert_eq!(count, 1);
    });
}

#[test]
fn list_memories_scopes_to_namespaces() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("g1", "global", MemoryCategory::Knowledge, "global").unwrap();
        add_memory("a1", "agent", MemoryCategory::Knowledge, "analyst").unwrap();

        // Global-only listing.
        let global_only = list_memories(Some(MemoryCategory::Knowledge), &["global"]).unwrap();
        let keys: Vec<_> = global_only.iter().map(|(_, _, k)| k.as_str()).collect();
        assert_eq!(keys, vec!["g1"]);

        // Agent-only listing.
        let agent_only = list_memories(Some(MemoryCategory::Knowledge), &["analyst"]).unwrap();
        let keys: Vec<_> = agent_only.iter().map(|(_, _, k)| k.as_str()).collect();
        assert_eq!(keys, vec!["a1"]);

        // Combined listing.
        let combined =
            list_memories(Some(MemoryCategory::Knowledge), &["analyst", "global"]).unwrap();
        let keys: Vec<_> = combined.iter().map(|(_, _, k)| k.as_str()).collect();
        assert!(keys.contains(&"g1"));
        assert!(keys.contains(&"a1"));
    });
}

#[test]
fn delete_memory_deletes_from_correct_path() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory(
            "to-del",
            "agent value",
            MemoryCategory::Knowledge,
            "analyst",
        )
        .unwrap();
        add_memory(
            "to-del",
            "global value",
            MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();

        // Delete from agent namespace only.
        delete_memory("to-del", MemoryCategory::Knowledge, "analyst").unwrap();

        // Agent version gone.
        assert!(read_memory("to-del", MemoryCategory::Knowledge, "analyst").is_err());
        // Global version still present.
        let val = read_memory("to-del", MemoryCategory::Knowledge, "global").unwrap();
        assert_eq!(val, "global value");
    });
}

/// The storage layer must read only the namespaces it is handed.
/// A memory written only in namespace `victim` is invisible to a scan
/// over `["analyst","global"]` and visible only when `victim` is in the slice.
#[test]
fn memory_scan_is_confined_to_supplied_namespaces() {
    let tmp = temp_home();
    with_home(&tmp, || {
        add_memory("secret", "v", MemoryCategory::Knowledge, "victim").unwrap();

        // Negative (read): the key exists only in `victim`, so an `analyst`-scoped read must fail.
        assert!(
            read_memory("secret", MemoryCategory::Knowledge, "analyst").is_err(),
            "read_memory must not find a key in a different namespace"
        );

        // Negative (scan): `["analyst","global"]` must not include the `victim`-only key.
        let results =
            list_memories_with_tags(Some(MemoryCategory::Knowledge), &["analyst", "global"])
                .unwrap();
        assert!(
            !results.iter().any(|e| e.key == "secret"),
            "scan over [analyst, global] must not include victim-only key"
        );

        // Positive control: `["victim"]` must include it (proves the memory was written
        // and the namespace slice is the only boundary).
        let results =
            list_memories_with_tags(Some(MemoryCategory::Knowledge), &["victim"]).unwrap();
        assert!(
            results.iter().any(|e| e.key == "secret"),
            "scan over [victim] must include the key"
        );
    });
}
