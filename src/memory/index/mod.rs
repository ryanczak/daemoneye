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

    // Split on non-alphanumeric characters so "runtime-layout" becomes
    // "runtime" and "layout", which FTS5 can match independently.
    for token in query.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        let lower = token.to_lowercase();
        if !seen.iter().any(|s| s == &lower) {
            seen.push(lower.clone());
            if seen.len() > 32 {
                break;
            }
            terms.push(lower);
        }
    }

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Read the single line beginning at `offset` bytes into `path`.
///
/// The inverse of the byte offsets this module stores in `turns_map` and
/// `events_map`, which is why it lives here: the append-only invariant that
/// makes those offsets stable is documented in this module. Returns an empty
/// string when the file is missing, unreadable, or has no line at that offset —
/// callers treat that as "no excerpt", never as an error.
pub fn read_line_at_offset(path: &std::path::Path, offset: u64) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let reader = std::io::BufReader::new(file);
    let mut current_offset: u64 = 0;
    for line in std::io::BufRead::lines(reader) {
        let Ok(l) = line else {
            break;
        };
        if current_offset == offset {
            return l;
        }
        current_offset += l.len() as u64 + 1; // +1 for newline
    }
    String::new()
}

/// Open the index connection and reconcile the given corpus table if it is
/// empty. Returns the connection on success, or logs and returns `None` on
/// failure. The caller should use the returned connection for its query.
fn open_and_reconcile_if_empty(table: &str) -> Option<rusqlite::Connection> {
    let conn = match open_index() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("memory index open failed: {e:#}");
            return None;
        }
    };

    let count_sql = format!("SELECT count(*) FROM {table}");
    let count: i64 = conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0);
    if count == 0 {
        let Some(corpus) = Corpus::from_table(table) else {
            log::warn!(
                "table '{}' is not a recognised corpus — skipping reconcile",
                table
            );
            return Some(conn);
        };
        if let Err(e) = reconcile_corpus(corpus) {
            log::warn!("memory index reconcile failed: {e:#}");
        }
        // Re-open because reconcile may have dropped and recreated the DB
        return match open_index() {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("memory index re-open after reconcile failed: {e:#}");
                None
            }
        };
    }
    Some(conn)
}

mod reconcile;
mod search;
pub use reconcile::*;
pub use search::*;
// `reconcile` reaches these memory items as `super::…`, through this import.
use super::{memory_dir_for_namespace, parse_memory_frontmatter};

/// Scan one archive file and insert its turn rows into the index.
///
/// Takes a `&rusqlite::Connection` so it can be called with either a
/// `&rusqlite::Transaction` (from reconcile) or a plain `&Connection`
/// (from a hook), since `Transaction` derefs to `Connection`.
pub fn index_archive_file(
    conn: &rusqlite::Connection,
    session_id: &str,
    path: &std::path::Path,
) -> Result<()> {
    let file =
        std::fs::File::open(path).with_context(|| format!("opening archive {}", path.display()))?;
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
            conn.execute(
                "INSERT INTO turns_map (session_id, turn, offset) VALUES (?1, ?2, ?3)",
                (session_id, turn as i64, offset as i64),
            )
            .with_context(|| format!("inserting turn map row for {} turn {}", session_id, turn))?;
            let rid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO turns (rowid, body) VALUES (?1, ?2)",
                (rid, &body),
            )
            .with_context(|| format!("inserting turn row for {} turn {}", session_id, turn))?;
        }
        offset += n as u64;
    }
    Ok(())
}

/// Index a single event segment file. Best-effort: per-file read errors are
/// logged and break the loop, never propagated.
#[cfg(test)]
pub fn index_event_segment(segment: &str) -> Result<()> {
    let path = crate::config::events_dir().join(format!("{segment}.jsonl"));
    let Ok(file) = std::fs::File::open(&path) else {
        return Ok(());
    };
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
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
                (segment, offset as i64),
            )?;
            let rid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO events (rowid, event, body) VALUES (?1, ?2, ?3)",
                (rid, event_val, &body),
            )?;
        }
        offset += n as u64;
    }
    tx.commit()
        .context("committing event segment index transaction")
}

/// Index a single archived turn message. Best-effort: any failure is logged
/// and returned as `Err` so the caller can swallow it.
pub fn index_turn(session_id: &str, turn: usize, offset: u64, body: &str) -> Result<()> {
    let body = crate::ai::mask_sensitive(body);
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "INSERT INTO turns_map (session_id, turn, offset) VALUES (?1, ?2, ?3)",
        (session_id, turn as i64, offset as i64),
    )
    .context("inserting turn map row")?;
    let rid = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO turns (rowid, body) VALUES (?1, ?2)",
        (rid, &body),
    )
    .context("inserting turn row")?;
    tx.commit().context("committing turn index transaction")
}

/// Index a single event log entry. Best-effort.
pub fn index_event(segment: &str, offset: u64, event: &str, body: &str) -> Result<()> {
    let body = crate::ai::mask_sensitive(body);
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "INSERT INTO events_map (segment, offset) VALUES (?1, ?2)",
        (segment, offset as i64),
    )
    .context("inserting event map row")?;
    let rid = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO events (rowid, event, body) VALUES (?1, ?2, ?3)",
        (rid, event, &body),
    )
    .context("inserting event row")?;
    tx.commit().context("committing event index transaction")
}

/// Index a single epoch record. Best-effort.
pub fn index_epoch(session_id: &str, seq: u32, kind: &str, body: &str) -> Result<()> {
    let conn = open_index()?;
    conn.execute(
        "INSERT INTO epochs (session_id, seq, kind, body) VALUES (?1, ?2, ?3, ?4)",
        (session_id, seq as i64, kind, body),
    )
    .context("inserting epoch row")?;
    Ok(())
}

/// Index (or replace) an artifact row. Deletes any existing row for
/// `(kind, name)` before inserting, so repeated writes do not accumulate.
/// Best-effort.
pub fn index_artifact(kind: &str, name: &str, tags: &str, body: &str) -> Result<()> {
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "DELETE FROM artifacts WHERE kind = ?1 AND name = ?2",
        (kind, name),
    )
    .context("deleting old artifact row")?;
    tx.execute(
        "INSERT INTO artifacts (kind, name, tags, body) VALUES (?1, ?2, ?3, ?4)",
        (kind, name, tags, body),
    )
    .context("inserting artifact row")?;
    tx.commit().context("committing artifact index transaction")
}

/// Remove an artifact row. Best-effort.
pub fn remove_artifact(kind: &str, name: &str) -> Result<()> {
    let conn = open_index()?;
    conn.execute(
        "DELETE FROM artifacts WHERE kind = ?1 AND name = ?2",
        (kind, name),
    )
    .context("deleting artifact row from index")?;
    Ok(())
}

/// Remove all turns rows belonging to a session. Must delete FTS rows
/// before map rows so the subquery in the FTS delete still sees the ids.
pub fn remove_session_turns(session_id: &str) -> Result<()> {
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "DELETE FROM turns WHERE rowid IN (SELECT id FROM turns_map WHERE session_id = ?1)",
        (session_id,),
    )
    .context("deleting turns FTS rows")?;
    tx.execute("DELETE FROM turns_map WHERE session_id = ?1", (session_id,))
        .context("deleting turns_map rows")?;
    tx.commit().context("committing turns removal")
}

/// Remove all events rows belonging to a segment. Must delete FTS rows
/// before map rows so the subquery in the FTS delete still sees the ids.
pub fn remove_event_segment(segment: &str) -> Result<()> {
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "DELETE FROM events WHERE rowid IN (SELECT id FROM events_map WHERE segment = ?1)",
        (segment,),
    )
    .context("deleting events FTS rows")?;
    tx.execute("DELETE FROM events_map WHERE segment = ?1", (segment,))
        .context("deleting events_map rows")?;
    tx.commit().context("committing events removal")
}

/// Construct a test message for indexing. Used by daemon::utils sweep tests.
#[cfg(test)]
#[doc(hidden)]
pub fn make_test_message_for_index(
    role: &str,
    content: &str,
    turn: Option<usize>,
) -> crate::ai::Message {
    crate::ai::Message {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_results: None,
        turn,
    }
}

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

#[cfg(test)]
mod incremental_tests;
#[cfg(test)]
mod tests;
