use anyhow::{Context, Result};
use std::time::Instant;
use crate::util::UnpoisonExt;

use crate::daemon::session::{SessionEntry, SessionStore};
use crate::runbook::Runbook;
use crate::ai::Message;
use crate::tmux::ensure_incident_session;

/// Orchestrates the lifecycle of an autonomous Ghost Session.
pub struct GhostManager;

impl GhostManager {
    /// Start a new Ghost Session for a specific alert and runbook.
    ///
    /// 1. Ensures a host tmux session exists (active or detached).
    /// 2. Initializes a new ghost `SessionEntry` with the alert as the first user turn.
    ///    Background windows (`de-incident-*`) are created lazily on the first tool call.
    /// 3. Returns the session ID for use by `trigger_ghost_turn`.
    pub async fn start_session(
        sessions: SessionStore,
        runbook: &Runbook,
        alert_msg: &str,
    ) -> Result<String> {
        let alert_name = &runbook.name;
        
        // 1. Ensure host tmux session exists (active or detached)
        let tmux_session = ensure_incident_session()
            .context("GhostManager: failed to ensure incident session")?;
        
        // 2. Initialize ghost session entry
        let session_id = format!("ghost-{}-{}", alert_name, uuid::Uuid::new_v4().simple());
        
        let mut messages = Vec::new();

        // The alert payload is the first user turn.  Ghost behavioral instructions
        // (autonomous mode, background-only execution, no human present) live in the
        // system prompt assembled by `trigger_ghost_turn`, not here.  Putting them in
        // an assistant-role message causes the Anthropic API to reject the request
        // because conversations must begin with a user turn.
        let user_msg = Message {
            role: "user".to_string(),
            content: format!("Incoming alert:\n{}", alert_msg),
            tool_calls: None,
            tool_results: None,
        };
        messages.push(user_msg);

        let entry = SessionEntry {
            messages,
            last_accessed: Instant::now(),
            chat_pane: None,
            default_target_pane: None, // Ghost sessions use background windows exclusively
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: tmux_session.clone(),
            last_detach: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: true,
            ghost_config: Some(runbook.ghost_config.clone()),
        };

        {
            let mut store = sessions.lock().unwrap_or_log();
            store.insert(session_id.clone(), entry);
        }

        crate::daemon::stats::inc_ghosts_launched();

        log::info!(
            "Ghost Session started: {} (alert: {}, session: {})",
            session_id,
            alert_name,
            tmux_session
        );

        Ok(session_id)
    }
}
