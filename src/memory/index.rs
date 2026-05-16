//! G5: FTS5 memory index (stub — not yet implemented).
//!
//! Returns empty results until the SQLite FTS5 index is wired in.

/// Placeholder result type for FTS5 search.
#[allow(dead_code)]
pub struct Fts5Result {
    pub key: String,
    pub score: f64,
}

/// Search the FTS5 index. Returns empty results until the index is implemented.
pub fn fts5_search(_query: &str, _limit: usize) -> Vec<(String, f64)> {
    Vec::new()
}
