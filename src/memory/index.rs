//! G5: FTS5 memory index.

use anyhow::{Context, Result};
use std::io::BufRead;

/// Bump when the FTS5 schema changes. A database at any other version is
/// dropped and recreated — the index is derived, so rebuilding is always safe.
pub const SCHEMA_VERSION: i64 = 2;

/// Open (creating if absent) the FTS5 memory index, applying the schema.
pub fn open_index() -> Result<rusqlite::Connection> {
    let dir = crate::config::var_index_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating index directory {}", dir.display()))?;
    let path = crate::config::memory_index_path();
    let conn = rusqlite::Connection::open(&path)
        .with_context(|| format!("opening index database {}", path.display()))?;
    ensure_schema(&conn)?;
    Ok(conn)
}

/// Apply the schema to an already-open connection, dropping and recreating
/// the table if the stored `user_version` is not `SCHEMA_VERSION`.
pub fn ensure_schema(conn: &rusqlite::Connection) -> Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .with_context(|| "reading user_version")?;

    if current != 0 && current != SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS memories;
             DROP TABLE IF EXISTS artifacts;
             DROP TABLE IF EXISTS epochs;
             DROP TABLE IF EXISTS turns;
             DROP TABLE IF EXISTS turns_map;
             DROP TABLE IF EXISTS events;
             DROP TABLE IF EXISTS events_map;",
        )
        .with_context(|| "dropping stale index tables")?;
    }

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS memories USING fts5(
            key,
            namespace UNINDEXED,
            category UNINDEXED,
            tags,
            summary,
            body,
            tokenize = 'porter unicode61 remove_diacritics 2'
        );",
    )
    .with_context(|| "creating memories FTS5 table")?;

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS artifacts USING fts5(
            kind UNINDEXED,
            name,
            tags,
            body,
            tokenize = 'porter unicode61 remove_diacritics 2'
        );",
    )
    .with_context(|| "creating artifacts FTS5 table")?;

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS epochs USING fts5(
            session_id UNINDEXED,
            seq UNINDEXED,
            kind UNINDEXED,
            body,
            tokenize = 'porter unicode61 remove_diacritics 2'
        );",
    )
    .with_context(|| "creating epochs FTS5 table")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS turns_map (
            id         INTEGER PRIMARY KEY,
            session_id TEXT NOT NULL,
            turn       INTEGER NOT NULL,
            offset     INTEGER NOT NULL
        );",
    )
    .with_context(|| "creating turns_map table")?;

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS turns USING fts5(
            body,
            content='', contentless_delete=1,
            tokenize = 'porter unicode61 remove_diacritics 2'
        );",
    )
    .with_context(|| "creating turns FTS5 table")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events_map (
            id      INTEGER PRIMARY KEY,
            segment TEXT NOT NULL,
            offset  INTEGER NOT NULL
        );",
    )
    .with_context(|| "creating events_map table")?;

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS events USING fts5(
            event,
            body,
            content='', contentless_delete=1,
            tokenize = 'porter unicode61 remove_diacritics 2'
        );",
    )
    .with_context(|| "creating events FTS5 table")?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .with_context(|| "setting user_version")?;

    Ok(())
}

/// Turn arbitrary user text into a safe FTS5 MATCH expression.
/// Returns `None` when the input yields no usable terms.
fn build_match_expr(query: &str) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut terms: Vec<String> = Vec::new();

    for token in query.split_whitespace() {
        if token.chars().all(|c| !c.is_alphanumeric()) {
            continue;
        }
        let escaped = token.replace('"', "\"\"");
        let lower = escaped.to_lowercase();
        if !seen.iter().any(|s| s == &lower) {
            seen.push(lower);
            if seen.len() > 32 {
                break;
            }
            terms.push(format!("\"{}\"", escaped));
        }
    }

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Search the FTS5 index. Returns up to `limit` hits as
/// `(namespace, key, bm25_score)`, best match first.
///
/// Best-effort: any failure returns an empty `Vec` after logging. The index is
/// a derived cache and search degrading to "no hits" must never be fatal.
pub fn fts5_search(query: &str, limit: usize, namespaces: &[&str]) -> Vec<(String, String, f64)> {
    if namespaces.is_empty() {
        return Vec::new();
    }

    let Some(expr) = build_match_expr(query) else {
        return Vec::new();
    };

    // Reconcile an empty index on first search
    let conn = {
        let conn = match open_index() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("memory index open failed: {e:#}");
                return Vec::new();
            }
        };

        let count: i64 = conn
            .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
            .unwrap_or(0);
        if count == 0 {
            if let Err(e) = reconcile_index() {
                log::warn!("memory index reconcile failed: {e:#}");
            }
            // Re-open the connection after reconcile (which may have dropped/recreated)
            match open_index() {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("memory index re-open after reconcile failed: {e:#}");
                    return Vec::new();
                }
            }
        } else {
            conn
        }
    };

    let placeholders = (0..namespaces.len())
        .map(|i| format!("?{}", i + 3))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT namespace, key, bm25(memories) FROM memories
         WHERE memories MATCH ?1 AND namespace IN ({placeholders})
         ORDER BY bm25(memories) LIMIT ?2"
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(expr), Box::new(limit as i64)];
    for ns in namespaces {
        params.push(Box::new(ns.to_string()));
    }

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("memory index prepare failed: {e:#}");
            return Vec::new();
        }
    };

    let rows = match stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|b| &**b)),
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
            ))
        },
    ) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("memory index query failed: {e:#}");
            return Vec::new();
        }
    };

    rows.filter_map(|r| r.ok()).collect()
}

/// Read the memory file for (namespace, category, key), parse it, and upsert
/// the row. A missing file is not an error — it is treated as a delete.
pub fn index_memory_file(
    key: &str,
    category: crate::memory::MemoryCategory,
    namespace: &str,
) -> Result<()> {
    let path = super::memory_dir_for_namespace(namespace, &category).join(format!("{key}.md"));
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Missing file → treat as delete
            return remove_from_index(key, category, namespace);
        }
        Err(e) => return Err(e).with_context(|| format!("reading memory file {}", path.display())),
    };
    let (fm, body) = super::parse_memory_frontmatter(&raw);
    let tags = fm.tags.join(" ");
    let summary = fm.summary.unwrap_or_default();
    let cat_name = category.canonical_name();

    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "DELETE FROM memories WHERE key = ?1 AND namespace = ?2 AND category = ?3",
        (&key, &namespace, &cat_name),
    )
    .context("deleting old row")?;
    tx.execute(
        "INSERT INTO memories (key, namespace, category, tags, summary, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (&key, &namespace, &cat_name, &tags, &summary, &body),
    )
    .context("inserting row")?;
    tx.commit().context("committing transaction")
}

/// Remove the row for (namespace, category, key). Removing a row that is not
/// there is a no-op, not an error.
pub fn remove_from_index(
    key: &str,
    category: crate::memory::MemoryCategory,
    namespace: &str,
) -> Result<()> {
    let conn = open_index()?;
    let cat_name = category.canonical_name();
    conn.execute(
        "DELETE FROM memories WHERE key = ?1 AND namespace = ?2 AND category = ?3",
        (&key, &namespace, &cat_name),
    )
    .context("deleting row from index")?;
    Ok(())
}

/// What a reconcile pass changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Rows present in the index at the start of the pass.
    pub rows_before: usize,
    /// Rows present after the rebuild.
    pub rows_after: usize,
    /// Per-corpus row counts after the rebuild, in a stable order:
    /// memories, artifacts, epochs, turns, events.
    pub per_corpus: Vec<(String, usize)>,
}

/// Rebuild the whole index from the memory files on disk.
pub fn reconcile_index() -> anyhow::Result<ReconcileReport> {
    let mut conn = open_index()?;

    // ── count rows across all corpora ────────────────────────────────────────

    fn count_table(conn: &rusqlite::Connection, table: &str) -> usize {
        conn.query_row(format!("SELECT COUNT(*) FROM {table}").as_str(), [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    let rows_before = count_table(&conn, "memories")
        + count_table(&conn, "artifacts")
        + count_table(&conn, "epochs")
        + count_table(&conn, "turns")
        + count_table(&conn, "events");

    let namespaces: Vec<String> = {
        let mut ns = vec!["global".to_string()];
        let agents = crate::agents::list_agents()?;
        for a in agents {
            ns.push(a.name);
        }
        ns
    };

    let categories = [
        crate::memory::MemoryCategory::Session,
        crate::memory::MemoryCategory::Knowledge,
        crate::memory::MemoryCategory::Incident,
    ];

    let tx = conn
        .transaction()
        .context("beginning reconcile transaction")?;
    tx.execute("DELETE FROM memories", [])
        .context("clearing memories index")?;
    tx.execute("DELETE FROM artifacts", [])
        .context("clearing artifacts index")?;
    tx.execute("DELETE FROM epochs", [])
        .context("clearing epochs index")?;
    tx.execute("DELETE FROM turns", [])
        .context("clearing turns index")?;
    tx.execute("DELETE FROM turns_map", [])
        .context("clearing turns_map index")?;
    tx.execute("DELETE FROM events", [])
        .context("clearing events index")?;
    tx.execute("DELETE FROM events_map", [])
        .context("clearing events_map index")?;

    // ── memories corpus ──────────────────────────────────────────────────────

    for namespace in &namespaces {
        for category in &categories {
            let dir = super::memory_dir_for_namespace(namespace, category);
            if !dir.exists() {
                continue;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Ok(raw) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Skip expired memories
                let (fm, _) = super::parse_memory_frontmatter(&raw);
                let info = crate::memory::MemoryInfo {
                    key: stem.to_string(),
                    category: category.canonical_name().to_string(),
                    namespace: namespace.clone(),
                    tags: fm.tags.clone(),
                    summary: fm.summary.clone(),
                    relates_to: fm.relates_to,
                    created: fm.created.clone(),
                    updated: fm.updated.clone(),
                    expires: fm.expires.clone(),
                    pinned: None,
                };
                if info.is_expired() {
                    continue;
                }
                let (fm, body) = super::parse_memory_frontmatter(&raw);
                let tags = fm.tags.join(" ");
                let summary = fm.summary.unwrap_or_default();
                let cat_name = category.canonical_name();
                tx.execute(
                    "INSERT INTO memories (key, namespace, category, tags, summary, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    (stem, namespace, cat_name, &tags, &summary, &body),
                )
                .with_context(|| format!("indexing {}", path.display()))?;
            }
        }
    }

    // ── artifacts corpus (runbooks + scripts) ────────────────────────────────

    for rb in crate::runbook::list_runbooks().unwrap_or_default() {
        let rb_path = crate::runbook::runbooks_dir().join(format!("{}.md", rb.name));
        let Ok(body) = std::fs::read_to_string(&rb_path) else {
            continue;
        };
        let tags = rb.tags.join(" ");
        tx.execute(
            "INSERT INTO artifacts (kind, name, tags, body) VALUES (?1, ?2, ?3, ?4)",
            ("runbook", &rb.name, &tags, &body),
        )
        .with_context(|| format!("indexing runbook {}", rb.name))?;
    }

    for (script, tags) in crate::scripts::list_scripts_with_tags().unwrap_or_default() {
        let Ok(body) = crate::scripts::read_script(&script.name) else {
            continue;
        };
        let tags = tags.join(" ");
        tx.execute(
            "INSERT INTO artifacts (kind, name, tags, body) VALUES (?1, ?2, ?3, ?4)",
            ("script", &script.name, &tags, &body),
        )
        .with_context(|| format!("indexing script {}", script.name))?;
    }

    // ── epochs corpus ────────────────────────────────────────────────────────

    if let Ok(sessions) = crate::config::sessions_dir().read_dir() {
        for entry in sessions.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.ends_with(".epochs.jsonl") {
                continue;
            }
            let session_id = &name_str[..name_str.len() - ".epochs.jsonl".len()];
            let records = crate::daemon::context::epochs::read_epochs(session_id);
            for rec in records {
                let mut body_parts: Vec<&str> = Vec::new();
                if let Some(ref narrative) = rec.narrative {
                    body_parts.push(narrative.as_str());
                }
                for (cmd, _) in &rec.tally.failed_cmds {
                    body_parts.push(cmd.as_str());
                }
                for art in &rec.artifacts {
                    body_parts.push(art.as_str());
                }
                let body = body_parts.join(" ");
                tx.execute(
                    "INSERT INTO epochs (session_id, seq, kind, body) VALUES (?1, ?2, ?3, ?4)",
                    (session_id, rec.seq as i64, &rec.kind, &body),
                )
                .with_context(|| format!("indexing epoch {} seq {}", session_id, rec.seq))?;
            }
        }
    }

    // ── turns corpus ──────────────────────────────────────────────────────────
    if let Ok(sessions) = crate::config::sessions_dir().read_dir() {
        for entry in sessions.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.ends_with(".archive.jsonl") {
                continue;
            }
            let session_id = &name_str[..name_str.len() - ".archive.jsonl".len()];
            let path = entry.path();
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let mut reader = std::io::BufReader::new(file);
            let mut offset: u64 = 0;
            let mut line = String::new();
            loop {
                line.clear();
                let n = match reader.read_line(&mut line) {
                    Ok(n) => n,
                    Err(e) => {
                        log::warn!("skipping {} at offset {offset}: {e}", path.display());
                        break;
                    }
                };
                if n == 0 {
                    break;
                }
                if let Ok(msg) = serde_json::from_str::<crate::ai::types::Message>(line.trim_end())
                    && let Some(turn) = msg.turn
                {
                    let mut body = msg.content.clone();
                    if let Some(ref tool_results) = msg.tool_results {
                        for tr in tool_results {
                            body.push(' ');
                            body.push_str(&tr.content);
                        }
                    }
                    let body = crate::ai::mask_sensitive(&body);
                    tx.execute(
                        "INSERT INTO turns_map (session_id, turn, offset) VALUES (?1, ?2, ?3)",
                        (session_id, turn as i64, offset as i64),
                    )
                    .with_context(|| {
                        format!("inserting turn map row for {} turn {}", session_id, turn)
                    })?;
                    let rid = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO turns (rowid, body) VALUES (?1, ?2)",
                        (rid, &body),
                    )
                    .with_context(|| {
                        format!("inserting turn row for {} turn {}", session_id, turn)
                    })?;
                }
                offset += n as u64;
            }
        }
    }

    // ── events corpus ─────────────────────────────────────────────────────────
    let event_paths = crate::daemon::utils::event_segment_paths_between(None, None);
    for path in event_paths {
        let legacy_path = crate::config::events_path();
        let segment = if path == legacy_path {
            "legacy".to_string()
        } else {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut reader = std::io::BufReader::new(file);
        let mut offset: u64 = 0;
        let mut line = String::new();
        loop {
            line.clear();
            let n = match reader.read_line(&mut line) {
                Ok(n) => n,
                Err(e) => {
                    log::warn!("skipping {} at offset {offset}: {e}", path.display());
                    break;
                }
            };
            if n == 0 {
                break;
            }
            let trimmed = line.trim_end();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
                && let Some(event_val) = v.get("event").and_then(|e| e.as_str())
            {
                let body = crate::search::json_to_readable(trimmed);
                let body = crate::ai::mask_sensitive(&body);
                tx.execute(
                    "INSERT INTO events_map (segment, offset) VALUES (?1, ?2)",
                    (segment.as_str(), offset as i64),
                )
                .with_context(|| format!("inserting event map row for segment {}", segment))?;
                let rid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO events (rowid, event, body) VALUES (?1, ?2, ?3)",
                    (rid, event_val, &body),
                )
                .with_context(|| format!("inserting event row for segment {}", segment))?;
            }
            offset += n as u64;
        }
    }

    tx.commit().context("committing reconcile transaction")?;

    let memories_count = count_table(&conn, "memories");
    let artifacts_count = count_table(&conn, "artifacts");
    let epochs_count = count_table(&conn, "epochs");
    let turns_count = count_table(&conn, "turns");
    let events_count = count_table(&conn, "events");

    let rows_after = memories_count + artifacts_count + epochs_count + turns_count + events_count;

    Ok(ReconcileReport {
        rows_before: rows_before as usize,
        rows_after,
        per_corpus: vec![
            ("memories".into(), memories_count),
            ("artifacts".into(), artifacts_count),
            ("epochs".into(), epochs_count),
            ("turns".into(), turns_count),
            ("events".into(), events_count),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_index_creates_database_and_schema() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let path = crate::config::memory_index_path();
        assert!(!path.exists(), "fresh HOME must not already have an index");
        let conn = open_index().expect("open_index should succeed");
        assert!(path.exists(), "open_index must create {}", path.display());

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'memories'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "memories table should exist");
    }

    #[test]
    fn open_index_sets_schema_version() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let conn = open_index().expect("open_index should succeed");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .expect("query user_version");
        assert_eq!(version, SCHEMA_VERSION, "schema version should be set");
    }

    #[test]
    fn open_index_is_idempotent() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        open_index().expect("first open_index should succeed");
        let conn = open_index().expect("second open_index should succeed");

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'memories'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "should have exactly one memories table");
    }

    #[test]
    fn stale_schema_version_is_recreated() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");

        // Create a deliberately wrong table and set a stale version.
        conn.execute_batch(
            "CREATE TABLE memories (id INTEGER PRIMARY KEY);
             PRAGMA user_version = 999;",
        )
        .expect("setup stale schema");

        ensure_schema(&conn).expect("ensure_schema should succeed on stale version");

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .expect("query user_version");
        assert_eq!(version, SCHEMA_VERSION, "version should be updated");
    }

    #[test]
    fn fts5_is_available_and_matches() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        ensure_schema(&conn).expect("ensure_schema should succeed");

        conn.execute(
            "INSERT INTO memories (key, namespace, category, tags, summary, body)
             VALUES (?, ?, ?, ?, ?, ?)",
            [
                "test-key",
                "global",
                "knowledge",
                "test",
                "A test memory",
                "the daemon is running quickly",
            ],
        )
        .expect("insert test row");

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE memories MATCH 'run'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("FTS5 MATCH query");
        assert_eq!(
            count, 1,
            "porter stemming should find 'running' when searching 'run'"
        );
    }

    #[test]
    fn unindexed_columns_filter_but_do_not_match() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        ensure_schema(&conn).expect("ensure_schema should succeed");

        conn.execute(
            "INSERT INTO memories (key, namespace, category, tags, summary, body)
             VALUES (?, ?, ?, ?, ?, ?)",
            [
                "key-a",
                "agent-x",
                "knowledge",
                "tag1",
                "Summary A",
                "hello world",
            ],
        )
        .expect("insert row 1");
        conn.execute(
            "INSERT INTO memories (key, namespace, category, tags, summary, body)
             VALUES (?, ?, ?, ?, ?, ?)",
            [
                "key-b",
                "global",
                "knowledge",
                "tag2",
                "Summary B",
                "goodbye world",
            ],
        )
        .expect("insert row 2");

        // UNINDEXED column should NOT be searchable via MATCH.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE memories MATCH '\"agent-x\"'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH on unindexed namespace");
        assert_eq!(
            count, 0,
            "namespace is UNINDEXED and should not be found by MATCH"
        );

        // But it should be filterable via WHERE.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE memories MATCH 'world' AND namespace = ?",
                ["agent-x"],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH with namespace filter");
        assert_eq!(
            count, 1,
            "namespace filter should narrow results to the matching row"
        );
    }

    #[test]
    fn add_memory_indexes_the_row() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "zephyr-fact",
            "The zephyr blows softly",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE memories MATCH 'zephyr'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH query");
        assert_eq!(count, 1, "should find exactly 1 row matching 'zephyr'");

        let key: String = conn
            .query_row(
                "SELECT key FROM memories WHERE memories MATCH 'zephyr'",
                [],
                |r| r.get::<_, String>(0),
            )
            .expect("fetch key");
        assert_eq!(key, "zephyr-fact");
    }

    #[test]
    fn update_memory_replaces_the_row_not_duplicates_it() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "replace-me",
            "old body with zebra",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        crate::memory::update_memory(crate::memory::UpdateMemoryArgs {
            key: "replace-me",
            category: crate::memory::MemoryCategory::Knowledge,
            body: Some("new body with quokka"),
            append: false,
            tags: None,
            summary: None,
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .expect("update_memory should succeed");

        let conn = open_index().expect("open_index should succeed");
        let total: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE key = 'replace-me'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("count rows for key");
        assert_eq!(total, 1, "should have exactly 1 row after update, not 2");

        let old_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE memories MATCH 'zebra'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH old body");
        assert_eq!(old_count, 0, "old body text should no longer be indexed");
    }

    #[test]
    fn delete_memory_removes_the_row() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "delete-me",
            "gone with the wind",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        crate::memory::delete_memory(
            "delete-me",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("delete_memory should succeed");

        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE key = 'delete-me'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("count rows for key");
        assert_eq!(count, 0, "deleted memory should have 0 rows");
    }

    #[test]
    fn same_key_in_two_namespaces_is_two_rows() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Create an agent directory so the agent namespace exists
        let agent_dir = tmp.path().join(".daemoneye/agents/agent-x");
        std::fs::create_dir_all(&agent_dir).unwrap();

        crate::memory::add_memory(
            "shared-key",
            "global content",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add global memory");

        crate::memory::add_memory(
            "shared-key",
            "agent content",
            crate::memory::MemoryCategory::Knowledge,
            "agent-x",
        )
        .expect("add agent memory");

        let conn = open_index().expect("open_index should succeed");
        let total: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE key = 'shared-key'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("count rows for key");
        assert_eq!(total, 2, "same key in two namespaces should be 2 rows");

        // Delete the global one
        crate::memory::delete_memory(
            "shared-key",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("delete global memory");

        let agent_row: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE key = 'shared-key' AND namespace = 'agent-x'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("count agent rows");
        assert_eq!(agent_row, 1, "agent row should survive global delete");
    }

    #[test]
    fn index_failure_does_not_fail_add_memory() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Create a file named `var/index` where the directory is expected —
        // create_dir_all then fails, so open_index() errors.
        let bad_index = tmp.path().join(".daemoneye/var/index");
        std::fs::create_dir_all(bad_index.parent().unwrap()).unwrap();
        std::fs::write(&bad_index, "not a directory").unwrap();

        let result = crate::memory::add_memory(
            "resilient-key",
            "I still get written",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        );
        assert!(result.is_ok(), "add_memory must return Ok when index fails");

        // The memory file must still exist on disk
        let mem_path = tmp
            .path()
            .join(".daemoneye/memory/knowledge/resilient-key.md");
        assert!(
            mem_path.exists(),
            "memory file must be written despite index failure"
        );
    }

    #[test]
    fn reconcile_rebuilds_from_disk() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Write memory files directly, bypassing add_memory
        let knowledge_dir = tmp.path().join(".daemoneye/memory/knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();
        std::fs::write(
            knowledge_dir.join("alpha.md"),
            "---\nnamespace: global\n---\nalpha body text",
        )
        .unwrap();
        std::fs::write(
            knowledge_dir.join("beta.md"),
            "---\nnamespace: global\n---\nbeta body text",
        )
        .unwrap();

        let report = reconcile_index().expect("reconcile should succeed");
        assert_eq!(report.rows_after, 2, "reconcile should find 2 rows");
    }

    #[test]
    fn reconcile_after_incremental_writes_is_a_no_op() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Add three memories across two categories
        crate::memory::add_memory(
            "k1",
            "first knowledge",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        crate::memory::add_memory(
            "k2",
            "second knowledge",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();
        crate::memory::add_memory(
            "s1",
            "first session",
            crate::memory::MemoryCategory::Session,
            "global",
        )
        .unwrap();

        // Update one
        crate::memory::update_memory(crate::memory::UpdateMemoryArgs {
            key: "k1",
            category: crate::memory::MemoryCategory::Knowledge,
            body: Some("updated knowledge"),
            append: false,
            tags: None,
            summary: None,
            relates_to: None,
            expires: None,
            namespace: "global",
        })
        .unwrap();

        // Delete one
        crate::memory::delete_memory("k2", crate::memory::MemoryCategory::Knowledge, "global")
            .unwrap();

        // Now reconcile — should find exactly what the incremental hooks left
        let report = reconcile_index().expect("reconcile should succeed");
        assert_eq!(
            report.rows_before, report.rows_after,
            "reconcile after incremental writes should be a no-op (rows_before == rows_after)"
        );
    }

    #[test]
    fn expired_memory_is_not_indexed() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Write a memory file with a past expiration date
        let knowledge_dir = tmp.path().join(".daemoneye/memory/knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();
        std::fs::write(
            knowledge_dir.join("expired.md"),
            "---\nnamespace: global\nexpires: \"2020-01-01\"\n---\nold expired content",
        )
        .unwrap();

        let report = reconcile_index().expect("reconcile should succeed");
        assert_eq!(
            report.rows_after, 0,
            "expired memory should not contribute any row"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 08: FTS5 search tests
    // -----------------------------------------------------------------------

    #[test]
    fn search_finds_text_hit_when_tags_miss() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "zephyr-fact",
            "The zephyr blows softly through the canyon",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        let results = fts5_search("zephyr", 10, &["global"]);
        assert_eq!(
            results.len(),
            1,
            "should find memory whose body mentions 'zephyr' even though tags do not"
        );
        assert_eq!(results[0].0, "global");
        assert_eq!(results[0].1, "zephyr-fact");
    }

    #[test]
    fn search_ranks_better_match_first() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Long body where "zephyr" is buried in filler
        crate::memory::add_memory(
            "zephyr-weak",
            "the quick brown fox jumps over the lazy dog many times and then zephyr appears once at the end of this long sentence with lots of other words that have nothing to do with zephyr at all",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add weak memory");

        // Short body where "zephyr" dominates
        crate::memory::add_memory(
            "zephyr-strong",
            "zephyr zephyr zephyr",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add strong memory");

        let results = fts5_search("zephyr", 10, &["global"]);
        assert!(results.len() >= 2, "should find both memories");
        assert_eq!(
            results[0].1, "zephyr-strong",
            "the memory where 'zephyr' dominates should rank first"
        );
        assert!(
            results[0].2 < results[1].2,
            "bm25 score for strong match should be more negative (better)"
        );
    }

    #[test]
    fn hyphenated_query_does_not_error() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "runtime-layout",
            "the runtime layout includes var and log directories",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        // Must not raise "no such column: layout"
        let results = fts5_search("runtime-layout", 10, &["global"]);
        assert!(
            !results.is_empty(),
            "hyphenated query should find the memory without error"
        );
    }

    #[test]
    fn operator_words_are_treated_as_text() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "and-memory",
            "a and b are both present in this memory",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        // Must not be interpreted as a boolean AND expression
        let results = fts5_search("a AND b", 10, &["global"]);
        assert!(
            !results.is_empty(),
            "'a AND b' should be treated as text terms, not a boolean operator"
        );
    }

    #[test]
    fn empty_query_returns_no_hits() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "some-memory",
            "some content here",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        let results = fts5_search("", 10, &["global"]);
        assert!(results.is_empty(), "empty query should return no hits");

        let results = fts5_search("   ?  ", 10, &["global"]);
        assert!(
            results.is_empty(),
            "punctuation-only query should return no hits"
        );
    }

    #[test]
    fn namespace_filter_excludes_other_namespaces() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "shared-key",
            "quokka is a marsupial",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add global memory");

        crate::memory::add_memory(
            "shared-key",
            "quokka is a marsupial",
            crate::memory::MemoryCategory::Knowledge,
            "agent-x",
        )
        .expect("add agent-x memory");

        let results = fts5_search("quokka", 10, &["global"]);
        assert_eq!(results.len(), 1, "should find exactly the global row");
        assert_eq!(results[0].0, "global");
        assert_eq!(results[0].1, "shared-key");
    }

    #[test]
    fn fresh_index_is_reconciled_on_first_search() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Seed a fresh HOME with config dirs and knowledge memories, but do NOT
        // call add_memory (so the index is never touched).
        crate::config::Config::ensure_dirs().expect("ensure_dirs should succeed");

        // The index file does not exist yet — nothing has called open_index().
        let index_path = crate::config::memory_index_path();
        assert!(
            !index_path.exists(),
            "fresh HOME must not already have an index file"
        );

        // Search for a word present in a seeded knowledge memory.
        // "webhook" appears in webhook-setup.md but no index exists yet.
        let results = fts5_search("webhook", 10, &["global"]);
        assert!(
            !results.is_empty(),
            "search should find seeded knowledge memory after reconcile-on-empty"
        );
        assert_eq!(
            results[0].1, "webhook-setup",
            "should find the webhook-setup memory"
        );

        // Assert the index now holds 9 rows (7 knowledge + 2 session).
        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
            .expect("count rows");
        assert_eq!(
            count, 9,
            "reconciled index should have 9 rows (7 knowledge + 2 session)"
        );
    }

    #[test]
    fn ftsearch_memories_preserves_rank_order() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Add two memories where one has a much stronger match for "quokka"
        crate::memory::add_memory(
            "quokka-strong",
            "quokka quokka quokka",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add strong memory");

        crate::memory::add_memory(
            "quokka-weak",
            "the quick brown fox jumps over the lazy dog and then quokka appears once",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add weak memory");

        let results = crate::daemon::memory_prompt::ftsearch_memories("quokka", 10, &["global"]);
        assert!(results.len() >= 2, "should find both memories");
        assert_eq!(
            results[0].key, "quokka-strong",
            "ftsearch_memories should preserve BM25 rank order: strong match first"
        );
    }

    #[test]
    fn multi_word_query_matches_non_adjacent_terms() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        crate::memory::add_memory(
            "pg-tuning",
            "increase shared_buffers when the working set grows",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add_memory should succeed");

        // A realistic user turn: the words are scattered, not a contiguous phrase.
        // Whole-query phrase quoting returns 0 here; per-term OR finds the memory.
        let results = fts5_search(
            "how do I tune shared_buffers for postgres?",
            10,
            &["global"],
        );
        assert!(
            !results.is_empty(),
            "a multi-word user turn must match on individual terms, not as one phrase"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 02a: schema v2 tests
    // -----------------------------------------------------------------------

    #[test]
    fn schema_v2_creates_every_table() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let conn = open_index().expect("open_index should succeed");

        let expected_tables = [
            "memories",
            "artifacts",
            "epochs",
            "turns",
            "turns_map",
            "events",
            "events_map",
        ];
        for tbl in &expected_tables {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    [tbl],
                    |r| r.get::<_, i64>(0),
                )
                .expect("query sqlite_master");
            assert_eq!(count, 1, "table '{tbl}' should exist");
        }

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .expect("query user_version");
        assert_eq!(version, 2, "schema version should be 2");
    }

    #[test]
    fn stale_v1_database_is_dropped_and_recreated() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");

        // Create a v1 schema with a row
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memories USING fts5(
                key, namespace UNINDEXED, category UNINDEXED,
                tags, summary, body,
                tokenize = 'porter unicode61 remove_diacritics 2'
            );
             PRAGMA user_version = 1;",
        )
        .expect("setup v1 schema");
        conn.execute(
            "INSERT INTO memories (key, namespace, category, tags, summary, body)
             VALUES (?, ?, ?, ?, ?, ?)",
            [
                "stale-key",
                "global",
                "knowledge",
                "",
                "stale",
                "stale body",
            ],
        )
        .expect("insert stale row");

        // Now upgrade
        ensure_schema(&conn).expect("ensure_schema should succeed");

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .expect("query user_version");
        assert_eq!(version, 2, "version should be 2 after upgrade");

        let stale: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE key = 'stale-key'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("count stale rows");
        assert_eq!(stale, 0, "stale v1 row should be gone");

        // Verify all 7 tables exist
        let expected_tables = [
            "memories",
            "artifacts",
            "epochs",
            "turns",
            "turns_map",
            "events",
            "events_map",
        ];
        for tbl in &expected_tables {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    [tbl],
                    |r| r.get::<_, i64>(0),
                )
                .expect("query sqlite_master");
            assert_eq!(count, 1, "table '{tbl}' should exist after upgrade");
        }
    }

    #[test]
    fn reconcile_indexes_runbook_and_script_bodies() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Seed a runbook with unique body text
        let rb_dir = tmp.path().join(".daemoneye/runbooks");
        std::fs::create_dir_all(&rb_dir).unwrap();
        std::fs::write(
            rb_dir.join("test-rb.md"),
            "---\ntags: [test]\n---\nThis runbook covers the quokka deployment procedure.",
        )
        .unwrap();

        // Seed a script with unique body text
        let sc_dir = tmp.path().join(".daemoneye/scripts");
        std::fs::create_dir_all(&sc_dir).unwrap();
        std::fs::write(
            sc_dir.join("test-sc.sh"),
            "#!/bin/sh\n# This script handles the wombat migration\nset -euo pipefail\necho done",
        )
        .unwrap();

        let report = reconcile_index().expect("reconcile should succeed");

        // Two artifacts rows
        let artifacts_count: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "artifacts")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(
            artifacts_count, 2,
            "should have 2 artifact rows (1 runbook + 1 script)"
        );

        // FTS query matching text in a runbook body
        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM artifacts WHERE artifacts MATCH 'quokka'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH query on artifacts");
        assert_eq!(count, 1, "should find runbook by body text 'quokka'");

        // FTS query matching text in a script body
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM artifacts WHERE artifacts MATCH 'wombat'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH query on artifacts");
        assert_eq!(count, 1, "should find script by body text 'wombat'");

        // Verify runbooks-executed stat did NOT change
        let before = crate::daemon::stats::get_runbooks_executed();
        let _ = reconcile_index();
        let after = crate::daemon::stats::get_runbooks_executed();
        assert_eq!(
            before, after,
            "reconcile must not increment runbooks-executed stat"
        );

        // Negative: searching memories for an artifacts-only term returns nothing
        let mem_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE memories MATCH 'quokka'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH query on memories");
        assert_eq!(
            mem_count, 0,
            "corpora must not bleed: 'quokka' in artifacts should not appear in memories"
        );
    }

    #[test]
    fn reconcile_indexes_epoch_narrative_and_failed_cmds() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Write an epochs file with two records
        let sessions_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let epochs_path = sessions_dir.join("test-sess.epochs.jsonl");
        let rec1 = crate::daemon::context::epochs::EpochRecord {
            seq: 1,
            kind: "epoch".into(),
            turn_start: 0,
            turn_end: 5,
            ts_start: chrono::Utc::now(),
            ts_end: chrono::Utc::now(),
            msg_count: 3,
            narrative: Some("The ferret was found in the garden".into()),
            tally: crate::daemon::context::epochs::EpochTally {
                commands_fail: 1,
                failed_cmds: vec![("rm -rf /tmp/bad".to_string(), -1)],
                ..Default::default()
            },
            artifacts: vec!["runbook:deploy".to_string()],
            covers: None,
        };
        let rec2 = crate::daemon::context::epochs::EpochRecord {
            seq: 2,
            kind: "epoch".into(),
            turn_start: 6,
            turn_end: 12,
            ts_start: chrono::Utc::now(),
            ts_end: chrono::Utc::now(),
            msg_count: 4,
            narrative: Some("The gerbil escaped".into()),
            tally: crate::daemon::context::epochs::EpochTally {
                commands_fail: 1,
                failed_cmds: vec![("curl http://example.com".to_string(), 0)],
                ..Default::default()
            },
            artifacts: vec![],
            covers: None,
        };

        // Write directly as JSONL (bypassing append_epoch to avoid masking)
        use std::io::Write;
        let mut f = std::fs::File::create(&epochs_path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&rec1).unwrap()).unwrap();
        writeln!(f, "{}", serde_json::to_string(&rec2).unwrap()).unwrap();

        let report = reconcile_index().expect("reconcile should succeed");

        let epochs_count: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "epochs")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(epochs_count, 2, "should have 2 epoch rows");

        // Query matching narrative text
        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM epochs WHERE epochs MATCH 'ferret'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH query on epochs");
        assert_eq!(count, 1, "should find epoch by narrative text 'ferret'");

        // Query matching failed_cmds text
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM epochs WHERE epochs MATCH 'bad'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .expect("MATCH query on epochs for failed_cmds");
        assert_eq!(count, 1, "should find epoch by failed_cmds text 'bad'");
    }

    #[test]
    fn reconcile_leaves_contentless_corpora_empty() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let report = reconcile_index().expect("reconcile should succeed");

        let turns_count: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(
            turns_count, 0,
            "turns should be empty (populated in phase 02b)"
        );

        let events_count: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "events")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(
            events_count, 0,
            "events should be empty (populated in phase 02b)"
        );
    }

    #[test]
    fn reconcile_report_per_corpus_sums_to_total() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Seed one memory
        crate::memory::add_memory(
            "test-mem",
            "test content",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();

        let report = reconcile_index().expect("reconcile should succeed");

        let per_corpus_sum: usize = report.per_corpus.iter().map(|(_, c)| c).sum();
        assert_eq!(
            report.rows_after, per_corpus_sum,
            "rows_after must equal the sum of per-corpus counts"
        );

        // Must have exactly 5 corpus entries
        assert_eq!(report.per_corpus.len(), 5, "should have 5 corpus entries");
    }

    #[test]
    fn second_reconcile_reports_no_change() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Seed a runbook
        let runbooks_dir = crate::config::runbooks_dir();
        std::fs::create_dir_all(&runbooks_dir).unwrap();
        std::fs::write(
            runbooks_dir.join("test-runbook.md"),
            "---\ntags: test\n---\n\n# Test Runbook\n\nThis runbook body contains unique searchable text for verification.",
        )
        .unwrap();

        // Seed a script
        let scripts_dir = crate::config::scripts_dir();
        std::fs::create_dir_all(&scripts_dir).unwrap();
        std::fs::write(scripts_dir.join("test-script.sh"), "#!/bin/sh\necho hello").unwrap();

        // First reconcile
        let report1 = reconcile_index().expect("first reconcile should succeed");
        let total1 = report1.rows_after;
        assert!(total1 > 0, "first reconcile should have rows");

        // Second reconcile — nothing changed on disk
        let report2 = reconcile_index().expect("second reconcile should succeed");
        assert_eq!(
            report2.rows_before, report2.rows_after,
            "second reconcile should report no change (rows_before == rows_after), \
             got before={} after={}",
            report2.rows_before, report2.rows_after
        );
        assert_eq!(
            report2.rows_before, total1,
            "rows_before on second reconcile should equal rows_after from first"
        );

        // rows_before must equal the sum of per_corpus
        let per_corpus_sum: usize = report2.per_corpus.iter().map(|(_, c)| c).sum();
        assert_eq!(
            report2.rows_before, per_corpus_sum,
            "rows_before must equal the sum of per-corpus counts"
        );
    }

    #[test]
    fn reconcile_indexes_archive_turns() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Create a session archive with three turn-numbered messages
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let archive = sessions_dir.join("abc123.archive.jsonl");
        let line1 = serde_json::json!({"role":"user","content":"hello","turn":0});
        let line2 = serde_json::json!({"role":"assistant","content":"world","turn":1});
        let line3 = serde_json::json!({"role":"user","content":"goodbye","turn":2});
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&line1).unwrap(),
            serde_json::to_string(&line2).unwrap(),
            serde_json::to_string(&line3).unwrap()
        );
        std::fs::write(&archive, &content).unwrap();

        let report = reconcile_index().expect("reconcile should succeed");

        let turns_count: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(turns_count, 3, "should have 3 turn rows");

        // Verify map rows too
        let conn = open_index().expect("open_index should succeed");
        let map_count: i64 = conn
            .query_row("SELECT count(*) FROM turns_map", [], |r| r.get(0))
            .unwrap();
        assert_eq!(map_count, 3, "should have 3 turns_map rows");
    }

    #[test]
    fn turns_map_offsets_point_at_the_right_line() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let archive = sessions_dir.join("offset-test.archive.jsonl");
        // Use multi-byte UTF-8 to stress offset calculation
        let line1 = serde_json::json!({"role":"user","content":"hello café","turn":0});
        let line2 = serde_json::json!({"role":"assistant","content":"world","turn":1});
        let line3 = serde_json::json!({"role":"user","content":"goodbye","turn":2});
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&line1).unwrap(),
            serde_json::to_string(&line2).unwrap(),
            serde_json::to_string(&line3).unwrap()
        );
        std::fs::write(&archive, &content).unwrap();

        reconcile_index().expect("reconcile should succeed");

        let conn = open_index().expect("open_index should succeed");
        let mut stmt = conn
            .prepare("SELECT turn, offset FROM turns_map ORDER BY offset")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        // For each row, seek to the offset and verify the line's turn matches
        for (expected_turn, offset) in &rows {
            use std::io::Seek;
            let mut reader = std::io::BufReader::new(std::fs::File::open(&archive).unwrap());
            reader
                .seek(std::io::SeekFrom::Start(*offset as u64))
                .unwrap();
            let mut line = String::new();
            let n = reader.read_line(&mut line).unwrap();
            assert!(n > 0, "should read a line at offset {}", offset);
            let parsed: crate::ai::types::Message = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(
                parsed.turn,
                Some(*expected_turn as usize),
                "line at offset {} should have turn {}",
                offset,
                expected_turn
            );
        }
    }

    #[test]
    fn turns_body_includes_tool_result_text() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let archive = sessions_dir.join("toolres.archive.jsonl");
        // The term "tool_output_42" appears ONLY in tool_results, not in content
        let msg = serde_json::json!({
            "role": "assistant",
            "content": "here is the result",
            "turn": 0,
            "tool_results": [
                {"tool_call_id": "t1", "tool_name": "grep", "content": "tool_output_42 found"}
            ]
        });
        let content = format!("{}\n", serde_json::to_string(&msg).unwrap());
        std::fs::write(&archive, &content).unwrap();

        reconcile_index().expect("reconcile should succeed");

        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'tool_output_42'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "should find turn by tool_result text");
    }

    #[test]
    fn turns_skips_messages_without_turn_numbers() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let archive = sessions_dir.join("noturn.archive.jsonl");
        let line1 = serde_json::json!({"role":"user","content":"no turn here"});
        let line2 = serde_json::json!({"role":"user","content":"has turn","turn":1});
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&line1).unwrap(),
            serde_json::to_string(&line2).unwrap()
        );
        std::fs::write(&archive, &content).unwrap();

        reconcile_index().expect("reconcile should succeed");

        let conn = open_index().expect("open_index should succeed");
        let count: i64 = conn
            .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "should have only 1 turn row (turn:None skipped)");
    }

    #[test]
    fn reconcile_indexes_event_segments() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();
        let segment = events_dir.join("events-20260803.jsonl");
        let line1 = serde_json::json!({"event":"webhook_alert","level":"warn","msg":"disk full"});
        let line2 = serde_json::json!({"event":"cron_tick","level":"info","msg":"ok"});
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&line1).unwrap(),
            serde_json::to_string(&line2).unwrap()
        );
        std::fs::write(&segment, &content).unwrap();

        reconcile_index().expect("reconcile should succeed");

        let conn = open_index().expect("open_index should succeed");

        // Check row count
        let count: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "should have 2 event rows");

        // Check segment label is the file stem
        let seg: String = conn
            .query_row("SELECT segment FROM events_map LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seg, "events-20260803", "segment should be the file stem");

        // Column-scoped match on event column
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'event:webhook_alert'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "should find webhook_alert by event column");
    }

    #[test]
    fn legacy_event_file_is_indexed_as_legacy_segment() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let legacy_path = crate::config::events_path();
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        let line = serde_json::json!({"event":"startup","level":"info","msg":"daemon started"});
        let content = format!("{}\n", serde_json::to_string(&line).unwrap());
        std::fs::write(&legacy_path, &content).unwrap();

        reconcile_index().expect("reconcile should succeed");

        let conn = open_index().expect("open_index should succeed");

        let count: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "should have 1 event row from legacy file");

        let seg: String = conn
            .query_row("SELECT segment FROM events_map LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seg, "legacy", "legacy file should have segment='legacy'");
    }

    #[test]
    fn contentless_bodies_are_masked() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Archive with an AWS key in content
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let archive = sessions_dir.join("mask-test.archive.jsonl");
        let msg = serde_json::json!({
            "role": "user",
            "content": "my key is AKIAIOSFODNN7EXAMPLE please help",
            "turn": 0
        });
        let content = format!("{}\n", serde_json::to_string(&msg).unwrap());
        std::fs::write(&archive, &content).unwrap();

        // Event with the same key
        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();
        let segment = events_dir.join("events-20260804.jsonl");
        let ev = serde_json::json!({"event":"api_call","level":"info","msg":"key AKIAIOSFODNN7EXAMPLE used"});
        let ev_content = format!("{}\n", serde_json::to_string(&ev).unwrap());
        std::fs::write(&segment, &ev_content).unwrap();

        reconcile_index().expect("reconcile should succeed");

        let conn = open_index().expect("open_index should succeed");

        // The raw canary should NOT be matchable in turns (proves masking)
        let turn_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'AKIAIOSFODNN7EXAMPLE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            turn_count, 0,
            "raw AWS key should not be searchable in turns"
        );

        // The masked placeholder should be matchable (proves masking happened)
        let turn_masked: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'AWS_KEY'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            turn_masked, 1,
            "masked placeholder should be searchable in turns"
        );

        // Same for events
        let ev_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'AKIAIOSFODNN7EXAMPLE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ev_count, 0,
            "raw AWS key should not be searchable in events"
        );

        let ev_masked: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'AWS_KEY'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ev_masked, 1,
            "masked placeholder should be searchable in events"
        );
    }

    #[test]
    fn second_reconcile_does_not_duplicate_contentless_rows() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Create a session archive
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let archive = sessions_dir.join("idem.archive.jsonl");
        let msg = serde_json::json!({"role":"user","content":"hello","turn":0});
        let content = format!("{}\n", serde_json::to_string(&msg).unwrap());
        std::fs::write(&archive, &content).unwrap();

        // Create an event segment
        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();
        let segment = events_dir.join("events-20260805.jsonl");
        let ev = serde_json::json!({"event":"tick","level":"info","msg":"ok"});
        let ev_content = format!("{}\n", serde_json::to_string(&ev).unwrap());
        std::fs::write(&segment, &ev_content).unwrap();

        let report1 = reconcile_index().expect("first reconcile should succeed");
        let turns1: usize = report1
            .per_corpus
            .iter()
            .find(|(n, _)| n == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        let events1: usize = report1
            .per_corpus
            .iter()
            .find(|(n, _)| n == "events")
            .map(|(_, c)| *c)
            .unwrap_or(0);

        let report2 = reconcile_index().expect("second reconcile should succeed");
        let turns2: usize = report2
            .per_corpus
            .iter()
            .find(|(n, _)| n == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        let events2: usize = report2
            .per_corpus
            .iter()
            .find(|(n, _)| n == "events")
            .map(|(_, c)| *c)
            .unwrap_or(0);

        assert_eq!(
            turns1, turns2,
            "turns count should not change on second reconcile"
        );
        assert_eq!(
            events1, events2,
            "events count should not change on second reconcile"
        );
        assert_eq!(
            report2.rows_before, report2.rows_after,
            "second reconcile should report no change"
        );
    }

    #[test]
    fn malformed_line_is_skipped_and_later_offsets_stay_correct() {
        use std::io::Write;

        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Write an archive with 3 lines: valid, malformed, valid
        let archive = sessions_dir.join("s1.archive.jsonl");
        let mut f = std::fs::File::create(&archive).unwrap();
        writeln!(f, r#"{{"role":"user","content":"first","turn":1}}"#).unwrap();
        writeln!(f, "not json at all").unwrap();
        writeln!(f, r#"{{"role":"user","content":"third","turn":3}}"#).unwrap();
        drop(f);

        let report = reconcile_index().unwrap();
        let turns: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(
            turns, 2,
            "malformed line should be skipped, 2 valid rows expected"
        );

        // Verify offsets by seeking and re-reading
        let db_path = crate::config::memory_index_path();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        let rows: Vec<(i64, i64)> = db
            .prepare("SELECT turn, offset FROM turns_map ORDER BY offset")
            .unwrap()
            .query_map([], |r| Ok((r.get(0).unwrap(), r.get(1).unwrap())))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(rows.len(), 2, "should have exactly 2 map rows");
        for (expected_turn, stored_offset) in &rows {
            let mut fh = std::fs::File::open(&archive).unwrap();
            use std::io::Seek;
            fh.seek(std::io::SeekFrom::Start(*stored_offset as u64))
                .unwrap();
            let mut reader = std::io::BufReader::new(fh);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
            let actual_turn: usize = parsed["turn"].as_u64().unwrap() as usize;
            assert_eq!(
                actual_turn as i64, *expected_turn,
                "offset {stored_offset} should point to turn {expected_turn}"
            );
        }
    }

    #[test]
    fn invalid_utf8_file_does_not_abort_reconcile() {
        use std::io::Write;

        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Write a valid archive with one message
        let valid_archive = sessions_dir.join("good.archive.jsonl");
        std::fs::write(
            &valid_archive,
            r#"{"role":"user","content":"hello world","turn":1}"#,
        )
        .unwrap();

        // Write a corrupt archive whose first line is raw invalid UTF-8
        // (no valid lines before the error, so zero rows from this file)
        let bad_archive = sessions_dir.join("bad.archive.jsonl");
        let mut f = std::fs::File::create(&bad_archive).unwrap();
        f.write_all(&[0xff, 0xfe, 0x80]).unwrap();
        drop(f);

        // reconcile_index() should return Ok, not Err
        let report = reconcile_index().unwrap();
        let turns: usize = report
            .per_corpus
            .iter()
            .find(|(n, _)| n == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(
            turns, 1,
            "only the valid archive should contribute rows (1 from 'good', 0 from 'bad')"
        );

        // Verify the row came from the good archive
        let db_path = crate::config::memory_index_path();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        let session_id: String = db
            .query_row("SELECT session_id FROM turns_map LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            session_id, "good",
            "the indexed row should come from the valid archive"
        );
    }
}
