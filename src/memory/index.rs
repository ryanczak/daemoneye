//! G5: FTS5 memory index.

#![allow(dead_code)]

use anyhow::{Context, Result};

/// Bump when the FTS5 schema changes. A database at any other version is
/// dropped and recreated — the index is derived, so rebuilding is always safe.
pub const SCHEMA_VERSION: i64 = 1;

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
        conn.execute_batch("DROP TABLE IF EXISTS memories")
            .with_context(|| "dropping stale memories table")?;
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

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .with_context(|| "setting user_version")?;

    Ok(())
}

/// Search the FTS5 index. Returns empty results until the index is implemented.
pub fn fts5_search(_query: &str, _limit: usize) -> Vec<(String, f64)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn open_index_creates_database_and_schema() {
        let _guard = crate::test_home_guard();
        unsafe { std::env::set_var("HOME", env::temp_dir().join("daemoneye-test-index-1")) };

        let conn = open_index().expect("open_index should succeed");
        let path = crate::config::memory_index_path();
        assert!(
            path.exists(),
            "database file should exist at {}",
            path.display()
        );

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
        unsafe { std::env::set_var("HOME", env::temp_dir().join("daemoneye-test-index-2")) };

        let conn = open_index().expect("open_index should succeed");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .expect("query user_version");
        assert_eq!(version, SCHEMA_VERSION, "schema version should be set");
    }

    #[test]
    fn open_index_is_idempotent() {
        let _guard = crate::test_home_guard();
        unsafe { std::env::set_var("HOME", env::temp_dir().join("daemoneye-test-index-3")) };

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
}
