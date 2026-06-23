use anyhow::Result;

use crate::ai::{AiEvent, Message, make_client};
use crate::config::Config;
use crate::daemon::session::SessionStore;
use crate::util::UnpoisonExt;

/// Maximum number of recent messages to include in the briefing prompt.
const MAX_BRIEFING_MESSAGES: usize = 50;

/// Generate a briefing summary from the session messages and save it to the
/// agent's briefing file.
///
/// This is best-effort: failures are logged but do not affect the caller.
pub async fn generate_and_save_briefing(
    agent_name: &str,
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
) {
    let (messages, model_key) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(session_id) else {
            log::warn!(
                "Briefing: session '{}' not found for agent '{}'",
                session_id,
                agent_name
            );
            return;
        };
        (entry.messages.clone(), entry.active_model.clone())
    };

    if messages.is_empty() {
        return;
    }

    let result = match do_generate_briefing(&messages, config, model_key.as_deref()).await {
        Ok(summary) => summary,
        Err(e) => {
            log::warn!(
                "Briefing generation failed for agent '{}': {}",
                agent_name,
                e
            );
            return;
        }
    };

    // Apply masking filter before writing to disk.
    let masked = crate::ai::filter::mask_sensitive(&result);

    let path = crate::agents::briefing_path(agent_name);
    if let Err(e) = std::fs::write(&path, &masked) {
        log::warn!(
            "Failed to write briefing for agent '{}' to {}: {}",
            agent_name,
            path.display(),
            e
        );
        return;
    }

    log::info!(
        "Briefing written for agent '{}' to {} ({} bytes)",
        agent_name,
        path.display(),
        masked.len()
    );

    crate::daemon::utils::log_event(
        "briefing_written",
        serde_json::json!({
            "agent": agent_name,
            "session_id": session_id,
            "path": path.display().to_string(),
            "bytes": masked.len(),
        }),
    );
}

async fn do_generate_briefing(
    messages: &[Message],
    config: &Config,
    model_key: Option<&str>,
) -> Result<String> {
    let model_entry = config.resolve_model(model_key);
    let client = make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
    );

    let system_prompt = "Summarize this session in ≤ 500 tokens for the next invocation of this agent. \
        Include: key findings, actions taken, open questions, and what the next run should prioritize. \
        Be concise and factual. Use bullet points where appropriate.";

    // Take the last N messages as the transcript.
    let recent: Vec<Message> = messages
        .iter()
        .rev()
        .take(MAX_BRIEFING_MESSAGES)
        .rev()
        .cloned()
        .collect();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

    tokio::spawn(async move {
        if let Err(e) = client
            .chat(system_prompt, recent, tx, false, Vec::new())
            .await
        {
            log::error!("Briefing AI call failed: {}", e);
        }
    });

    let mut summary = String::new();
    while let Some(ev) = rx.recv().await {
        match ev {
            AiEvent::Token(t) => summary.push_str(&t),
            AiEvent::Done(_) => break,
            AiEvent::Error(e) => anyhow::bail!("AI error during briefing generation: {}", e),
            _ => {}
        }
    }

    if summary.is_empty() {
        anyhow::bail!("briefing generation returned empty summary");
    }

    Ok(summary.trim().to_string())
}

/// Read the briefing file for an agent, if it exists.
///
/// Returns `None` when no briefing has been written yet.
pub fn read_briefing(agent_name: &str) -> Option<String> {
    let path = crate::agents::briefing_path(agent_name);
    std::fs::read_to_string(&path).ok()
}

/// Delete the briefing file for an agent. No-op if it does not exist.
pub fn clear_briefing(agent_name: &str) {
    let path = crate::agents::briefing_path(agent_name);
    let _ = std::fs::remove_file(&path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::UnpoisonExt;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("de_briefing_test_{}_{}", std::process::id(), n));
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

    #[test]
    fn read_briefing_returns_none_when_absent() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            assert!(read_briefing("test-agent").is_none());
        });
    }

    #[test]
    fn read_briefing_returns_content_when_present() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let path = crate::agents::briefing_path("test-agent");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "Test briefing content").unwrap();
            assert_eq!(
                read_briefing("test-agent"),
                Some("Test briefing content".to_string())
            );
        });
    }

    #[test]
    fn clear_briefing_removes_file() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let path = crate::agents::briefing_path("test-agent");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "content").unwrap();
            clear_briefing("test-agent");
            assert!(read_briefing("test-agent").is_none());
        });
    }

    #[test]
    fn clear_briefing_is_noop_when_absent() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            clear_briefing("nonexistent-agent");
        });
    }
}
