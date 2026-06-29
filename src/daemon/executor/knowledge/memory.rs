use super::super::ToolCallOutcome;
use super::{ArtifactCtx, track_artifact};
use crate::ai::filter::mask_sensitive;
use crate::daemon::utils::log_event;

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

pub fn add_memory(
    key: &str,
    value: &str,
    category: &str,
    artifact_ctx: &ArtifactCtx<'_>,
) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    if value.trim().is_empty() {
        return "Error: memory value cannot be empty.".to_string();
    }
    let stamped = match artifact_ctx.saved_name {
        Some(origin) => crate::header::inject_yaml_session_origin(value, origin),
        None => value.to_string(),
    };
    let namespace = artifact_ctx.namespaces.first().copied().unwrap_or("global");
    match crate::memory::add_memory(key, &stamped, cat, namespace) {
        Ok(()) => {
            log_event(
                "memory_write",
                serde_json::json!({ "session": artifact_ctx.session_id, "op": "add", "category": category, "key": key }),
            );
            track_artifact(artifact_ctx, "memory", key);
            format!("Memory '{}' stored in {} ({})", key, category, namespace)
        }
        Err(e) => format!("Error storing memory: {}", e),
    }
}

pub struct UpdateMemoryRequest<'a> {
    pub key: &'a str,
    pub category: &'a str,
    pub body: Option<&'a str>,
    pub append: bool,
    pub tags: Option<&'a [String]>,
    pub summary: Option<&'a str>,
    pub relates_to: Option<&'a [String]>,
    pub expires: Option<&'a str>,
}

pub fn update_memory(
    req: UpdateMemoryRequest<'_>,
    session_id: Option<&str>,
    namespaces: &[&str],
) -> String {
    let UpdateMemoryRequest {
        key,
        category,
        body,
        append,
        tags,
        summary,
        relates_to,
        expires,
    } = req;
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    let namespace = namespaces.first().copied().unwrap_or("global");
    match crate::memory::update_memory(crate::memory::UpdateMemoryArgs {
        key,
        category: cat,
        body,
        append,
        tags,
        summary,
        relates_to,
        expires,
        namespace,
    }) {
        Ok(()) => {
            log_event(
                "memory_write",
                serde_json::json!({ "session": session_id, "op": "update", "category": category, "key": key }),
            );
            let mut updated_fields: Vec<&str> = Vec::new();
            if body.is_some() {
                updated_fields.push(if append { "body (appended)" } else { "body" });
            }
            if tags.is_some() {
                updated_fields.push("tags");
            }
            if summary.is_some() {
                updated_fields.push("summary");
            }
            if relates_to.is_some() {
                updated_fields.push("relates_to");
            }
            if expires.is_some() {
                updated_fields.push("expires");
            }
            if updated_fields.is_empty() {
                format!(
                    "Memory '{}' [{}] updated (timestamp refreshed).",
                    key, category
                )
            } else {
                format!(
                    "Memory '{}' [{}] updated: {}.",
                    key,
                    category,
                    updated_fields.join(", ")
                )
            }
        }
        Err(e) => format!("Error updating memory '{}': {}", key, e),
    }
}

pub fn delete_memory(
    key: &str,
    category: &str,
    session_id: Option<&str>,
    namespaces: &[&str],
) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    let namespace = namespaces.first().copied().unwrap_or("global");
    match crate::memory::delete_memory(key, cat, namespace) {
        Ok(()) => {
            log_event(
                "memory_write",
                serde_json::json!({ "session": session_id, "op": "delete", "category": category, "key": key }),
            );
            format!("Memory '{}' deleted from {} ({})", key, category, namespace)
        }
        Err(e) => format!("Error deleting memory: {}", e),
    }
}

pub fn read_memory(key: &str, category: &str, namespaces: &[&str]) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    for ns in namespaces {
        if let Ok(content) = crate::memory::read_memory(key, cat, ns) {
            crate::daemon::stats::inc_memories_recalled();
            return mask_sensitive(&content);
        }
    }
    format!(
        "Error reading memory '{}': not found in namespaces: {:?}",
        key, namespaces
    )
}

pub async fn list_memories<W>(
    category: Option<&str>,
    namespaces: &[&str],
    _tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let cat = match category {
        None => None,
        Some(s) => match crate::memory::MemoryCategory::from_str(s) {
            Some(c) => Some(c),
            None => {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    s
                )));
            }
        },
    };
    let infos = crate::memory::list_memories_with_tags(cat, namespaces).unwrap_or_default();
    let count = infos.len();
    if count == 0 {
        Ok(ToolCallOutcome::Result(
            "No memory entries found.".to_string(),
        ))
    } else {
        let lines: Vec<String> = infos
            .iter()
            .map(|info| {
                let mut line = match &info.summary {
                    Some(s) => format!(
                        "[{}] [{}] {} — {}",
                        info.namespace, info.category, info.key, s
                    ),
                    None => format!("[{}] [{}] {}", info.namespace, info.category, info.key),
                };
                let ts_opt = info.updated.as_ref().or(info.created.as_ref());
                let label = if info.updated.is_some() {
                    "updated"
                } else {
                    "created"
                };
                if let Some(ts) = ts_opt {
                    let date = ts.split('T').next().unwrap_or(ts.as_str());
                    if !date.is_empty() {
                        line.push_str(&format!(" ({} {})", label, date));
                    }
                }
                line
            })
            .collect();
        Ok(ToolCallOutcome::Result(format!(
            "{} memory entries:\n{}",
            count,
            lines.join("\n")
        )))
    }
}

// ---------------------------------------------------------------------------
// Search / context
// ---------------------------------------------------------------------------

pub fn search_repository(query: &str, kind: &str, namespaces: &[&str]) -> String {
    let results = crate::search::search_repository_with_namespaces(query, kind, 2, namespaces);
    crate::search::format_results(&results)
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{TmpHome, with_home};
    use super::*;

    fn make_ctx() -> ArtifactCtx<'static> {
        let store: &'static crate::daemon::session::SessionStore = Box::leak(Box::new(
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        ));
        let ns: &[&str] = &["global"];
        ArtifactCtx {
            session_id: None,
            sessions: store,
            saved_name: None,
            turn_count: 0,
            is_ghost: false,
            namespaces: ns,
        }
    }

    #[test]
    fn add_memory_rejects_invalid_category() {
        let ctx = make_ctx();
        let out = add_memory("k", "v", "bogus", &ctx);
        assert!(
            out.starts_with("Error: invalid category 'bogus'."),
            "got: {out}"
        );
    }

    #[test]
    fn add_memory_rejects_empty_value() {
        let ctx = make_ctx();
        let out = add_memory("k", "   ", "knowledge", &ctx);
        assert_eq!(out, "Error: memory value cannot be empty.");
    }

    #[test]
    fn add_then_read_memory_round_trips() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let ctx = make_ctx();
            let out = add_memory("mykey", "myvalue", "knowledge", &ctx);
            assert_eq!(out, "Memory 'mykey' stored in knowledge (global)");

            let read = read_memory("mykey", "knowledge", &["global"]);
            assert!(
                read.contains("myvalue"),
                "expected read to contain 'myvalue', got: {read}"
            );
        });
    }

    #[test]
    fn read_memory_not_found_reports_namespaces() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let out = read_memory("nonexistent", "session", &["global"]);
            assert!(
                out.starts_with("Error reading memory 'nonexistent': not found in namespaces:"),
                "got: {out}"
            );
        });
    }

    #[test]
    fn delete_memory_rejects_invalid_category() {
        let out = delete_memory("k", "bogus", None, &["global"]);
        assert!(
            out.starts_with("Error: invalid category 'bogus'."),
            "got: {out}"
        );
    }

    #[test]
    fn update_memory_rejects_invalid_category() {
        let out = update_memory(
            UpdateMemoryRequest {
                key: "k",
                category: "bogus",
                body: None,
                append: false,
                tags: None,
                summary: None,
                relates_to: None,
                expires: None,
            },
            None,
            &["global"],
        );
        assert!(
            out.starts_with("Error: invalid category 'bogus'."),
            "got: {out}"
        );
    }

    #[test]
    fn list_memories_empty_reports_none() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let out = rt.block_on(async {
                let mut sink = tokio::io::sink();
                list_memories(None, &["global"], &mut sink).await.unwrap()
            });
            match out {
                ToolCallOutcome::Result(s) => {
                    assert_eq!(s, "No memory entries found.", "got: {s}")
                }
                ToolCallOutcome::UserMessage(_) => {
                    panic!("unexpected UserMessage outcome")
                }
                ToolCallOutcome::SpawnGhostSession { .. } => {
                    panic!("unexpected SpawnGhostSession outcome")
                }
            }
        });
    }
}
