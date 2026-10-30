//! Querying the index: BM25-ranked search over memories, turns, artifacts,
//! events and epochs.

use super::*;

/// As [`fts5_search`], but restricted to one memory category when
/// `category` is `Some`. The value must be the category's **canonical** name
/// (`"incident"`, not the `"incidents"` directory name) — that is what
/// `index_memory_file` stores.
pub fn fts5_search_in_category(
    query: &str,
    limit: usize,
    namespaces: &[&str],
    category: Option<&str>,
) -> Vec<(String, String, f64)> {
    if namespaces.is_empty() {
        return Vec::new();
    }

    let Some(expr) = build_match_expr(query) else {
        return Vec::new();
    };

    let conn = match open_and_reconcile_if_empty("memories") {
        Some(c) => c,
        None => return Vec::new(),
    };

    let placeholders = (0..namespaces.len())
        .map(|i| format!("?{}", i + 3))
        .collect::<Vec<_>>()
        .join(",");
    let cat_clause = if category.is_some() {
        format!(" AND category = ?{}", 3 + namespaces.len())
    } else {
        String::new()
    };
    let sql = format!(
        "SELECT namespace, key, bm25(memories) FROM memories
         WHERE memories MATCH ?1 AND namespace IN ({placeholders}){cat_clause}
         ORDER BY bm25(memories) LIMIT ?2"
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(expr), Box::new(limit as i64)];
    for ns in namespaces {
        params.push(Box::new(ns.to_string()));
    }
    if let Some(cat) = category {
        params.push(Box::new(cat.to_string()));
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

/// Search the FTS5 index. Returns up to `limit` hits as
/// `(namespace, key, bm25_score)`, best match first.
///
/// Best-effort: any failure returns an empty `Vec` after logging. The index is
/// a derived cache and search degrading to "no hits" must never be fatal.
pub fn fts5_search(query: &str, limit: usize, namespaces: &[&str]) -> Vec<(String, String, f64)> {
    fts5_search_in_category(query, limit, namespaces, None)
}

/// A hit from a turns FTS search.
pub struct TurnHit {
    pub session_id: String,
    pub turn: i64,
    pub offset: i64,
    pub score: f64,
}

/// Search the `turns` FTS corpus. Returns up to `limit` hits ordered by BM25
/// (best first). When `session_id` is `Some`, restricts to that session; when
/// `None`, searches every session.
///
/// Best-effort: any failure logs and returns an empty `Vec`. Search degrading
/// to "no hits" must never be fatal.
pub fn search_turns(query: &str, limit: usize, session_id: Option<&str>) -> Vec<TurnHit> {
    let Some(expr) = build_match_expr(query) else {
        return Vec::new();
    };

    let conn = match open_and_reconcile_if_empty("turns") {
        Some(c) => c,
        None => return Vec::new(),
    };

    let (sql, params): (&str, Vec<String>) = if let Some(sid) = session_id {
        (
            "SELECT m.session_id, m.turn, m.offset, bm25(turns)
             FROM turns t JOIN turns_map m ON m.id = t.rowid
             WHERE turns MATCH ?1 AND m.session_id = ?2
             ORDER BY bm25(turns)
             LIMIT ?3",
            vec![expr, sid.to_string(), limit.to_string()],
        )
    } else {
        (
            "SELECT m.session_id, m.turn, m.offset, bm25(turns)
             FROM turns t JOIN turns_map m ON m.id = t.rowid
             WHERE turns MATCH ?1
             ORDER BY bm25(turns)
             LIMIT ?2",
            vec![expr, limit.to_string()],
        )
    };

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("search_turns prepare failed: {e:#}");
            return Vec::new();
        }
    };

    let rows = match stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|b| &**b)),
        |r| {
            Ok(TurnHit {
                session_id: r.get(0)?,
                turn: r.get(1)?,
                offset: r.get(2)?,
                score: r.get(3)?,
            })
        },
    ) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("search_turns query failed: {e:#}");
            return Vec::new();
        }
    };

    rows.filter_map(|r| r.ok()).collect()
}

/// A hit from an artifact FTS search.
pub struct ArtifactHit {
    pub name: String,
}

/// A hit from an events FTS search.
pub struct EventHit {
    pub segment: String,
    pub offset: u64,
}

/// Search the `artifacts` FTS corpus. Returns up to `limit` hits ordered by
/// BM25 (best first). When `kind` is `Some`, restricts to that kind
/// (`"runbook"` or `"script"` — the singular labels `index_artifact` stores).
///
/// Best-effort: any failure logs and returns an empty `Vec`.
pub fn search_artifacts(query: &str, limit: usize, kind: Option<&str>) -> Vec<ArtifactHit> {
    let Some(expr) = build_match_expr(query) else {
        return Vec::new();
    };

    let conn = match open_and_reconcile_if_empty("artifacts") {
        Some(c) => c,
        None => return Vec::new(),
    };

    let (sql, params): (&str, Vec<String>) = match kind {
        Some(k) => (
            "SELECT kind, name, bm25(artifacts)
             FROM artifacts
             WHERE artifacts MATCH ?1 AND kind = ?2
             ORDER BY bm25(artifacts)
             LIMIT ?3",
            vec![expr, k.to_string(), limit.to_string()],
        ),
        None => (
            "SELECT kind, name, bm25(artifacts)
             FROM artifacts
             WHERE artifacts MATCH ?1
             ORDER BY bm25(artifacts)
             LIMIT ?2",
            vec![expr, limit.to_string()],
        ),
    };

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("search_artifacts prepare failed: {e:#}");
            return Vec::new();
        }
    };

    let rows = match stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|b| &**b)),
        |r| Ok(ArtifactHit { name: r.get(1)? }),
    ) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("search_artifacts query failed: {e:#}");
            return Vec::new();
        }
    };

    rows.filter_map(|r| r.ok()).collect()
}

/// Search the `events` FTS corpus. Returns up to `limit` hits ordered by BM25
/// (best first). Joins `events` to `events_map` on `m.id = e.rowid` to get
/// the segment and offset for each hit.
///
/// Best-effort: any failure logs and returns an empty `Vec`.
pub fn search_events(query: &str, limit: usize) -> Vec<EventHit> {
    let Some(expr) = build_phrase_expr(query) else {
        return Vec::new();
    };

    let conn = match open_and_reconcile_if_empty("events") {
        Some(c) => c,
        None => return Vec::new(),
    };

    // Select only `events_map` columns: the `events` table is contentless
    // (`content=''`), so its own columns read back NULL, and decoding one
    // into a `String` failed on every row, which `filter_map` then dropped.
    let sql = "SELECT m.segment, m.offset
               FROM events e JOIN events_map m ON m.id = e.rowid
               WHERE events MATCH ?1
               ORDER BY bm25(events)
               LIMIT ?2";
    let params = [expr, limit.to_string()];

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("search_events prepare failed: {e:#}");
            return Vec::new();
        }
    };

    let rows = match stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|b| &**b)),
        |r| {
            Ok(EventHit {
                segment: r.get(0)?,
                offset: r.get::<_, i64>(1)? as u64,
            })
        },
    ) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("search_events query failed: {e:#}");
            return Vec::new();
        }
    };

    rows.filter_map(|r| r.ok()).collect()
}

/// A hit from an epochs FTS search.
pub struct EpochHit {
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub body: String,
}

/// Search the `epochs` FTS corpus. Returns up to `limit` hits ordered by BM25
/// (best first). The epochs table is stored-content, so `body` is selected
/// directly — no offset, no file round-trip.
///
/// Best-effort: any failure logs and returns an empty `Vec`.
pub fn search_epochs(query: &str, limit: usize) -> Vec<EpochHit> {
    let Some(expr) = build_match_expr(query) else {
        return Vec::new();
    };

    let conn = match open_and_reconcile_if_empty("epochs") {
        Some(c) => c,
        None => return Vec::new(),
    };

    let sql = "SELECT session_id, seq, kind, body, bm25(epochs)
               FROM epochs
               WHERE epochs MATCH ?1
               ORDER BY bm25(epochs)
               LIMIT ?2";
    let params = [expr, limit.to_string()];

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("search_epochs prepare failed: {e:#}");
            return Vec::new();
        }
    };

    let rows = match stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|b| &**b)),
        |r| {
            Ok(EpochHit {
                session_id: r.get(0)?,
                seq: r.get(1)?,
                kind: r.get(2)?,
                body: r.get(3)?,
            })
        },
    ) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("search_epochs query failed: {e:#}");
            return Vec::new();
        }
    };

    rows.filter_map(|r| r.ok()).collect()
}
