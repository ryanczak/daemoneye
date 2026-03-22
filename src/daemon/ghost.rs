use anyhow::{Context, Result};
use std::time::Instant;
use crate::util::UnpoisonExt;

use crate::daemon::session::{SessionEntry, SessionStore};
use crate::runbook::Runbook;
use crate::ai::Message;
use crate::tmux::{ensure_incident_session, create_incident_window};

/// Orchestrates the lifecycle of an autonomous Ghost Session.
pub struct GhostManager;

impl GhostManager {
    /// Start a new Ghost Session for a specific alert and runbook.
    ///
    /// 1. Ensures a host tmux session exists.
    /// 2. Creates a dedicated `de-incident-*` window.
    /// 3. Initializes a new ghost `SessionEntry`.
    /// 4. Injects the initial alert context and system instructions.
    pub async fn start_session(
        sessions: SessionStore,
        runbook: &Runbook,
        alert_msg: &str,
    ) -> Result<String> {
        let alert_name = &runbook.name;
        
        // 1. Ensure host tmux session
        let tmux_session = ensure_incident_session()
            .context("GhostManager: failed to ensure incident session")?;
        
        // 2. Create incident window
        let (_window_idx, pane_id) = create_incident_window(&tmux_session, alert_name)
            .context("GhostManager: failed to create incident window")?;
        
        // 3. Initialize ghost session entry
        let session_id = format!("ghost-{}-{}", alert_name, uuid::Uuid::new_v4().simple());
        
        let mut messages = Vec::new();
        
        // System instruction for Ghost Session
        let system_msg = Message {
            role: "assistant".to_string(), // Injected as "system" context but using assistant/user turns
            content: format!(
                "[System] You are operating in an unattended Ghost Session responding to: {}\n\n\
                 Investigate and remediate autonomously using the provided runbook. \
                 You must use pre-approved scripts for destructive or sudo actions. \
                 No human user is present to approve commands or answer questions.",
                alert_msg
            ),
            tool_calls: None,
            tool_results: None,
        };
        messages.push(system_msg);

        let entry = SessionEntry {
            messages,
            last_accessed: Instant::now(),
            chat_pane: None, // No interactive chat pane
            default_target_pane: Some(pane_id.clone()),
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

            "Ghost Session started: {} (window: {}, pane: {})",
            session_id,
            alert_name,
            pane_id
        );

        Ok(session_id)
    }
}
