//! Rebuilding the index from the files on disk: `reconcile_corpus` for one
//! corpus, `reconcile_index` for all of them.

use super::*;

/// Which corpus a table belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corpus {
    Memories,
    Artifacts,
    Epochs,
    Turns,
    Events,
}

impl Corpus {
    /// The FTS table name for this corpus.
    pub(super) fn table_name(self) -> &'static str {
        match self {
            Corpus::Memories => "memories",
            Corpus::Artifacts => "artifacts",
            Corpus::Epochs => "epochs",
            Corpus::Turns => "turns",
            Corpus::Events => "events",
        }
    }

    /// Resolve a table name to its corpus.
    /// Returns `None` for map tables (`turns_map`, `events_map`) and unknown names.
    pub(super) fn from_table(name: &str) -> Option<Corpus> {
        match name {
            "memories" => Some(Corpus::Memories),
            "artifacts" => Some(Corpus::Artifacts),
            "epochs" => Some(Corpus::Epochs),
            "turns" => Some(Corpus::Turns),
            "events" => Some(Corpus::Events),
            _ => None,
        }
    }
}

fn rebuild_memories(tx: &rusqlite::Connection) -> anyhow::Result<()> {
    tx.execute("DELETE FROM memories", [])
        .context("clearing memories index")?;

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

    Ok(())
}

fn rebuild_artifacts(tx: &rusqlite::Connection) -> anyhow::Result<()> {
    tx.execute("DELETE FROM artifacts", [])
        .context("clearing artifacts index")?;

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

    Ok(())
}

fn rebuild_epochs(tx: &rusqlite::Connection) -> anyhow::Result<()> {
    tx.execute("DELETE FROM epochs", [])
        .context("clearing epochs index")?;

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

    Ok(())
}

fn rebuild_turns(tx: &rusqlite::Connection) -> anyhow::Result<()> {
    tx.execute("DELETE FROM turns", [])
        .context("clearing turns index")?;
    tx.execute("DELETE FROM turns_map", [])
        .context("clearing turns_map index")?;

    if let Ok(sessions) = crate::config::sessions_dir().read_dir() {
        for entry in sessions.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.ends_with(".archive.jsonl") {
                continue;
            }
            let session_id = &name_str[..name_str.len() - ".archive.jsonl".len()];
            let path = entry.path();
            if let Err(e) = index_archive_file(tx, session_id, &path) {
                log::warn!("indexing archive {}: {e:#}", path.display());
            }
        }
    }

    Ok(())
}

fn rebuild_events(tx: &rusqlite::Connection) -> anyhow::Result<()> {
    tx.execute("DELETE FROM events", [])
        .context("clearing events index")?;
    tx.execute("DELETE FROM events_map", [])
        .context("clearing events_map index")?;

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

    Ok(())
}

/// Reconcile a single corpus from disk, rebuilding only its own tables.
pub fn reconcile_corpus(corpus: Corpus) -> anyhow::Result<usize> {
    let mut conn = open_index()?;
    let tx = conn
        .transaction()
        .context("beginning corpus reconcile transaction")?;

    match corpus {
        Corpus::Memories => rebuild_memories(&tx)?,
        Corpus::Artifacts => rebuild_artifacts(&tx)?,
        Corpus::Epochs => rebuild_epochs(&tx)?,
        Corpus::Turns => rebuild_turns(&tx)?,
        Corpus::Events => rebuild_events(&tx)?,
    }

    tx.commit()
        .context("committing corpus reconcile transaction")?;

    let count: i64 = conn
        .query_row(
            format!("SELECT COUNT(*) FROM {}", corpus.table_name()).as_str(),
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(count as usize)
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

    let tx = conn
        .transaction()
        .context("beginning reconcile transaction")?;

    rebuild_memories(&tx)?;
    rebuild_artifacts(&tx)?;
    rebuild_epochs(&tx)?;
    rebuild_turns(&tx)?;
    rebuild_events(&tx)?;

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
