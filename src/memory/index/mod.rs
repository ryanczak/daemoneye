//! G5: FTS5 memory index.

use anyhow::{Context, Result};
use std::io::BufRead;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, OnceLock};

use crate::util::UnpoisonExt;

/// Bump when the FTS5 schema changes. A database at any other version is
/// dropped and recreated — the index is derived, so rebuilding is always safe.
pub const SCHEMA_VERSION: i64 = 2;

/// The process's one cached index connection, keyed by the path it was
/// opened for. `rusqlite::Connection` is not `Sync`, so the connection is
/// reached only through the mutex, and a holder keeps the guard for one
/// whole synchronous index operation. The slot is filled by the first
/// successful open — a slot cannot exist without its connection — and a
/// changed `memory_index_path()` (each test points `HOME` at its own
/// tempdir) makes the next open replace it with that path's database.
static INDEX_CONN: OnceLock<Mutex<IndexSlot>> = OnceLock::new();

/// The cached connection and the path it was opened for.
struct IndexSlot {
    path: std::path::PathBuf,
    conn: rusqlite::Connection,
}

// `(index path, corpus table)` pairs this process already reconciled to no
// avail — empty before and after — so the per-turn searches stop rebuilding
// them. Global: the connection slot is global too, and a pair keyed by the
// absolute path cannot collide across test HOMEs.
static INDEX_RECONCILED_EMPTY: OnceLock<
    Mutex<std::collections::HashSet<(std::path::PathBuf, String)>>,
> = OnceLock::new();

fn reconciled_empty() -> &'static Mutex<std::collections::HashSet<(std::path::PathBuf, String)>> {
    INDEX_RECONCILED_EMPTY.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[cfg(test)]
static INDEX_OPEN_ATTEMPTS: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();

#[cfg(test)]
fn attempts_slot() -> &'static std::sync::atomic::AtomicU64 {
    INDEX_OPEN_ATTEMPTS.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// How many times the process has opened an index database. Bumped in
/// `open_index`, read by the perf tests.
#[cfg(test)]
pub(crate) fn index_open_attempts() -> u64 {
    attempts_slot().load(std::sync::atomic::Ordering::Relaxed)
}

/// Re-key the cached index connection away from `path`, so a test observes
/// the next open for it actually reopening (and re-checking the schema); the
/// stale connection closes when that open replaces it.
#[cfg(test)]
pub(crate) fn close_index_conn(path: &std::path::Path) {
    if let Some(slot) = INDEX_CONN.get() {
        let mut guard = slot.lock().unwrap_or_log();
        if guard.path == path {
            guard.path = std::path::PathBuf::new();
        }
    }
}

#[cfg(test)]
static CORPUS_RECONCILE_COUNT: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();

#[cfg(test)]
fn corpus_slot() -> &'static std::sync::atomic::AtomicU64 {
    CORPUS_RECONCILE_COUNT.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// How many per-corpus rebuilds this process has run. Test-only probe.
#[cfg(test)]
pub(crate) fn corpus_reconcile_count() -> u64 {
    corpus_slot().load(std::sync::atomic::Ordering::Relaxed)
}

/// An open index connection, checked out for one synchronous operation.
/// A holder always has its connection: either the guard on the process's
/// cached slot, released for the next caller when the holder drops, or a
/// transient connection of its own, opened because the slot was already
/// held (by another thread, or by this one — a reconcile re-opens the index
/// while its caller still holds it) and closed when the holder drops.
pub struct IndexConn(Held);

enum Held {
    Cached(std::sync::MutexGuard<'static, IndexSlot>),
    Own(rusqlite::Connection),
}

impl Deref for IndexConn {
    type Target = rusqlite::Connection;

    fn deref(&self) -> &rusqlite::Connection {
        match &self.0 {
            Held::Cached(slot) => &slot.conn,
            Held::Own(conn) => conn,
        }
    }
}

impl DerefMut for IndexConn {
    fn deref_mut(&mut self) -> &mut rusqlite::Connection {
        match &mut self.0 {
            Held::Cached(slot) => &mut slot.conn,
            Held::Own(conn) => conn,
        }
    }
}

/// Open (creating if absent) the FTS5 memory index: the process's one cached
/// connection for the current `memory_index_path()`, schema-checked once per
/// path rather than on every open. The connection is checked out for one
/// whole synchronous index operation — take it, use it, drop it; holding it
/// across an `.await` would hand every other index caller a transient
/// connection, with the schema check re-run for each.
pub fn open_index() -> Result<IndexConn> {
    let path = crate::config::memory_index_path();
    let dir = crate::config::var_index_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating index directory {}", dir.display()))?;
    let slot = match INDEX_CONN.get() {
        Some(slot) => slot,
        None => {
            // The first open in this process fills the slot. Open first, so
            // a failure is this call's error rather than an empty slot; if
            // another thread filled the slot meanwhile, its connection stays
            // and this one closes.
            let conn = open_at(&path)?;
            INDEX_CONN.get_or_init(|| {
                Mutex::new(IndexSlot {
                    path: path.clone(),
                    conn,
                })
            })
        }
    };
    let mut guard = match slot.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            log::error!("Recovering from poisoned Mutex lock");
            poisoned.into_inner()
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            return Ok(IndexConn(Held::Own(open_at(&path)?)));
        }
    };
    if guard.path != path {
        *guard = IndexSlot {
            path: path.clone(),
            conn: open_at(&path)?,
        };
    }
    Ok(IndexConn(Held::Cached(guard)))
}

/// Open the database at `path` and bring its schema to `SCHEMA_VERSION`.
/// Every index database this process opens comes through here, which is
/// what the test-only open counter counts.
fn open_at(path: &std::path::Path) -> Result<rusqlite::Connection> {
    #[cfg(test)]
    attempts_slot().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let conn = rusqlite::Connection::open(path)
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

/// The query's terms as one FTS5 phrase: the same tokens as
/// [`build_match_expr`], in order, quoted together so they must appear
/// adjacent. The events search wants the line holding `idx=0`, not every
/// line holding `idx` or `0`, which the OR form returns. Tokens are
/// alphanumeric, so none can close the quotes.
fn build_phrase_expr(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(32)
        .map(str::to_lowercase)
        .collect();
    (!terms.is_empty()).then(|| format!("\"{}\"", terms.join(" ")))
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
///
/// The reconcile is **once per path**: a corpus that is still empty after
/// its reconcile — `epochs` for a user who has never compacted — is recorded
/// in `reconciled_empty()` and never rebuilt again for that database, rather
/// than rebuilt on every turn.
fn open_and_reconcile_if_empty(table: &str) -> Option<IndexConn> {
    let path = crate::config::memory_index_path();
    let done = reconciled_empty()
        .lock()
        .unwrap_or_log()
        .contains(&(path.clone(), table.to_string()));
    if done {
        return open_index().ok();
    }
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
        let reopened = match open_index() {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("memory index re-open after reconcile failed: {e:#}");
                None
            }
        };
        // Still empty after the rebuild (never-compacted epochs): mark the
        // (path, table) done so a later turn does not build it again.
        if let Some(c) = reopened.as_ref() {
            let still: i64 = c.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0);
            if still == 0 {
                reconciled_empty()
                    .lock()
                    .unwrap_or_log()
                    .insert((path, table.to_string()));
            }
        }
        return reopened;
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
mod perf_tests;
#[cfg(test)]
mod tests;
