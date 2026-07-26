use std::sync::Arc;

use crate::ai::{AiEvent, Message};
use crate::daemon::ghost::GhostManager;
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};
use crate::daemon::utils::{UnpoisonExt, fire_notification, log_event};

use super::*;

// ---------------------------------------------------------------------------
// Severity ranking
// ---------------------------------------------------------------------------

fn severity_rank(s: &str) -> u8 {
    match s.to_lowercase().as_str() {
        "critical" => 3,
        "warning" | "warn" => 2,
        "info" | "informational" => 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Alert processing pipeline
// ---------------------------------------------------------------------------

/// Returns the current time as seconds since UNIX epoch.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub async fn process_alert(alert: InternalAlert, state: Arc<WebhookState>) {
    crate::daemon::stats::record_webhook();
    let cfg = &state.config.webhook;

    // 1. Deduplication check.
    {
        // Clamp the window to a safe range: 1 s minimum (0 would disable dedup
        // entirely), 86400 s maximum (prevents unbounded HashMap growth from a
        // misconfigured very-long window keeping entries alive indefinitely).
        let window = cfg.dedup_window_secs.clamp(1, 86400);
        let mut dedup = state.dedup.lock().unwrap_or_log();
        let now = now_secs();
        if let Some(&last_seen) = dedup.get(&alert.fingerprint)
            && now.saturating_sub(last_seen) < window
        {
            log::debug!(
                "Webhook: suppressed duplicate alert '{}' (fingerprint: {})",
                alert.alert_name,
                &alert.fingerprint[..alert.fingerprint.len().min(16)]
            );
            return;
        }
        // Cap the dedup map to 10,000 entries. When the cap is reached, evict
        // the oldest entry (smallest timestamp) to prevent unbounded growth
        // from alert storms with high fingerprint cardinality.
        const DEDUP_MAP_CAP: usize = 10_000;
        if dedup.len() >= DEDUP_MAP_CAP
            && let Some(oldest_key) = dedup
                .iter()
                .min_by_key(|&(_, &ts)| ts)
                .map(|(k, _)| k.clone())
        {
            dedup.remove(&oldest_key);
        }
        dedup.insert(alert.fingerprint.clone(), now);
    }

    // 2. Mask sensitive data in summary + description.
    let summary = crate::ai::filter::mask_sensitive(&alert.summary);
    let description = crate::ai::filter::mask_sensitive(&alert.description);

    // 3. Format human-readable alert message.
    let fp_short = &alert.fingerprint[..alert.fingerprint.len().min(8)];
    let formatted = if description.is_empty() || description == summary {
        format!(
            "[{}] {} — {} ({})",
            alert.status.as_str().to_uppercase(),
            alert.alert_name,
            summary,
            alert.source,
        )
    } else {
        format!(
            "[{}] {} — {}\n{} ({})",
            alert.status.as_str().to_uppercase(),
            alert.alert_name,
            summary,
            description,
            alert.source,
        )
    };

    // 4. Log to events.jsonl.
    log_event(
        "webhook_alert",
        serde_json::json!({
            "alert_name": alert.alert_name,
            "status": alert.status.as_str(),
            "severity": alert.severity,
            "summary": summary,
            "fingerprint": fp_short,
            "source": alert.source,
        }),
    );

    log::info!(
        "Webhook alert: '{}' [{}] severity={} source={}",
        alert.alert_name,
        alert.status.as_str(),
        alert.severity,
        alert.source
    );

    // 5. Inject into all active AI sessions (disk + in-memory).
    let alert_msg = Message {
        role: "user".to_string(),
        content: format!("[Webhook Alert]\n{}", formatted),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    inject_into_sessions(&state.sessions, &alert_msg);

    // 6. Notify chat panes via tmux display-message.
    let first_line = formatted.lines().next().unwrap_or(&formatted).to_string();
    notify_chat_panes(&state.sessions, &first_line);

    // 7. Severity gate: fire notification + optionally trigger AI analysis.
    let threshold_rank = severity_rank(&cfg.severity_threshold);
    let alert_rank = severity_rank(&alert.severity);

    if alert_rank >= threshold_rank || threshold_rank == 0 {
        fire_notification(&alert.alert_name, &formatted, &state.config);

        if cfg.auto_analyze {
            maybe_analyze_alert(&alert, &formatted, &state).await;
        }
    }
}

/// Append the alert message to every active session — both the on-disk JSONL
/// file and the in-memory entry (so it appears in the next AI turn regardless
/// of whether the session was idle or active).
pub(crate) fn inject_into_sessions(sessions: &SessionStore, msg: &Message) {
    let ids: Vec<String> = with_sessions(sessions, |store| store.keys().cloned().collect());

    // In-memory is intentionally NOT updated here — the next Ask request
    // will re-read from disk when the in-memory history is stale.
    // For sessions currently in flight this means the alert appears in the
    // turn after the one already in progress, which is acceptable.
    for sid in &ids {
        append_session_message(sid, msg);
    }
}

/// Send a one-line alert notification to every active chat pane.
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let panes: Vec<String> = with_sessions(sessions, |store| {
        store.values().filter_map(|e| e.chat_pane.clone()).collect()
    });

    // Unlocked phase: everything blocking happens out here.
    for pane in &panes {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "8000", "-t", pane, msg])
            .output();
    }
}

/// Inject a ghost shell lifecycle event into all active user sessions and
/// send a tmux display-message notification to every open chat pane.
///
/// The `content` string is stored in session history so it shows up in the
/// N15 catch-up brief when the user re-attaches after being away.
pub(crate) fn inject_ghost_event(sessions: &SessionStore, content: &str) {
    let msg = crate::ai::Message {
        role: "user".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    inject_into_sessions(sessions, &msg);
    // One-liner for the tmux display-message overlay (strip newlines).
    let one_liner = content.lines().next().unwrap_or(content);
    notify_chat_panes(sessions, one_liner);
    // Always mirror ghost lifecycle events to events.jsonl for troubleshooting.
    crate::daemon::utils::log_event("ghost_lifecycle", serde_json::json!({ "content": content }));
}

// ---------------------------------------------------------------------------
// Runbook auto-analysis (Phase 4)
// ---------------------------------------------------------------------------

/// Convert CamelCase to kebab-case, e.g. "HighDiskUsage" → "high-disk-usage".
fn camel_to_kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.char_indices() {
        if ch.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Parse the structured `GHOST_TRIGGER: YES/NO` field from a watchdog AI response.
///
/// Scans lines in reverse so the field is found quickly even in long responses.
/// Returns `Some(true)` for YES, `Some(false)` for NO, `None` if absent.
pub(crate) fn parse_ghost_trigger(response: &str) -> Option<bool> {
    const PREFIX: &str = "GHOST_TRIGGER:";
    for line in response.lines().rev() {
        let upper = line.trim().to_uppercase();
        if let Some(rest) = upper.strip_prefix(PREFIX) {
            let val = rest.trim();
            if val.starts_with("YES") {
                return Some(true);
            }
            if val.starts_with("NO") {
                return Some(false);
            }
        }
    }
    None
}

/// Evaluate a watchdog AI response to determine whether action should be taken.
///
/// Returns `(should_act, trigger_reason)`.  Prefers the structured
/// `GHOST_TRIGGER: YES/NO` field; falls back to the legacy `ALERT` keyword for
/// responses that predate the structured format.  Pass `api_error=true` when the
/// API call itself failed so the reason string is informative.
pub(crate) fn evaluate_watchdog_response(response: &str, api_error: bool) -> (bool, &'static str) {
    if api_error && response.is_empty() {
        (false, "api_error — response empty")
    } else if response.is_empty() {
        (false, "empty_response — model returned no tokens")
    } else {
        match parse_ghost_trigger(response) {
            Some(true) => (true, "GHOST_TRIGGER: YES"),
            Some(false) => (false, "GHOST_TRIGGER: NO"),
            None => {
                if response.to_uppercase().contains("ALERT") {
                    (true, "legacy ALERT keyword (no GHOST_TRIGGER line found)")
                } else {
                    (
                        false,
                        "no GHOST_TRIGGER line and no ALERT keyword in response",
                    )
                }
            }
        }
    }
}

/// Try to find a runbook whose name matches the alert name in several variants.
fn find_runbook_for_alert(alert_name: &str) -> Option<crate::runbook::Runbook> {
    let kebab = camel_to_kebab(alert_name);
    let lower = alert_name.to_lowercase();
    for name in [kebab.as_str(), lower.as_str(), alert_name] {
        if let Ok(rb) = crate::runbook::load_runbook(name) {
            return Some(rb);
        }
    }
    None
}

/// Run runbook-based AI analysis for the alert, rate-limited per alert name.
async fn maybe_analyze_alert(alert: &InternalAlert, formatted_msg: &str, state: &WebhookState) {
    // Rate limit: skip if we analysed the same alert_name within dedup_window_secs.
    {
        let mut rl = state.rate_limit.lock().unwrap_or_log();
        let now = now_secs();
        if let Some(&last) = rl.get(&alert.alert_name)
            && now.saturating_sub(last) < state.config.webhook.dedup_window_secs
        {
            return;
        }
        rl.insert(alert.alert_name.clone(), now);
    }

    let Some(rb) = find_runbook_for_alert(&alert.alert_name) else {
        log::debug!(
            "Webhook: no runbook found for alert '{}' (tried kebab-case, lowercase, exact match) — skipping analysis",
            alert.alert_name
        );
        return;
    };

    log::info!(
        "Webhook: running runbook analysis for '{}' using runbook '{}'",
        alert.alert_name,
        rb.name
    );

    let system = crate::runbook::watchdog_system_prompt(&rb);
    let msgs = vec![Message {
        role: "user".to_string(),
        content: format!("Incoming alert:\n{}", formatted_msg),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }];

    let model_entry = state.config.resolve_model(None);
    let client = crate::ai::make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
    );

    let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
    let api_err = if let Err(e) = client.chat(&system, msgs, ai_tx, false, Vec::new()).await {
        log::error!(
            "Webhook: runbook analysis API call failed for '{}': {}",
            alert.alert_name,
            e
        );
        Some(e.to_string())
    } else {
        None
    };

    let mut response = String::new();
    while let Some(ev) = ai_rx.recv().await {
        if let AiEvent::Token(t) = ev {
            response.push_str(&t);
        }
    }

    let (should_act, trigger_reason) = evaluate_watchdog_response(&response, api_err.is_some());

    log::info!(
        "Webhook: analysis for '{}' complete — should_act={} reason='{}' ghost_enabled={} (response: {} chars)",
        alert.alert_name,
        should_act,
        trigger_reason,
        rb.ghost_config.enabled,
        response.len()
    );
    if !should_act && rb.ghost_config.enabled {
        log::info!(
            "Webhook: ghost shell NOT triggered for '{}' — reason: {}",
            alert.alert_name,
            trigger_reason
        );
    }
    log::debug!(
        "Webhook: analysis response for '{}':\n{}",
        alert.alert_name,
        response.trim()
    );

    log_event(
        "webhook_analysis",
        serde_json::json!({
            "alert_name": alert.alert_name,
            "runbook": rb.name,
            "ghost_trigger": should_act,
            "trigger_reason": trigger_reason,
            "ghost_enabled": rb.ghost_config.enabled,
        }),
    );

    if should_act {
        // If the runbook has ghost mode enabled, trigger a ghost shell.
        if rb.ghost_config.enabled {
            if !crate::daemon::ghost::check_ghost_capacity(&state.config) {
                log::warn!(
                    "Webhook: ghost shell skipped for '{}' — concurrency limit ({}) reached",
                    alert.alert_name,
                    state.config.ghost.max_concurrent_ghosts
                );
                inject_ghost_event(
                    &state.sessions,
                    &format!(
                        "[Ghost Shell Skipped] Concurrency limit reached for alert: {}",
                        alert.alert_name
                    ),
                );
            } else {
                log::info!("Webhook: triggering Ghost Shell for '{}'", alert.alert_name);
                let sessions = state.sessions.clone();
                let alert_msg = formatted_msg.to_string();
                let rb_clone = rb.clone();
                let config_clone = state.config.clone();
                let cache_clone = state.cache.clone();
                let schedule_store_clone = state.schedule_store.clone();

                tokio::spawn(async move {
                    let merged_config = crate::agents::merge_runbook_ghost_config(&rb_clone);
                    match GhostManager::start_session_with_config(
                        sessions.clone(),
                        &rb_clone,
                        &merged_config,
                        &alert_msg,
                        crate::daemon::GS_BG_WINDOW_PREFIX,
                        config_clone.approvals.ghost_commands,
                    )
                    .await
                    {
                        Ok(sid) => {
                            let session_log = crate::daemon::session::session_file(&sid)
                                .display()
                                .to_string();
                            inject_ghost_event(
                                &sessions,
                                &format!(
                                    "[Ghost Shell Started] Autonomous remediation triggered for alert: {} — session log: {}",
                                    rb_clone.name, session_log
                                ),
                            );

                            match crate::daemon::ghost::trigger_ghost_turn(
                                &sid,
                                &sessions,
                                &config_clone,
                                &cache_clone,
                                &schedule_store_clone,
                            )
                            .await
                            {
                                Ok(()) => {
                                    inject_ghost_event(
                                        &sessions,
                                        &format!(
                                            "[Ghost Shell Completed] Autonomous remediation finished for alert: {} — session log: {}",
                                            rb_clone.name, session_log
                                        ),
                                    );
                                }
                                Err(e) => {
                                    log::error!("Ghost Turn: failed for {}: {}", sid, e);
                                    crate::daemon::stats::inc_ghosts_failed();
                                    inject_ghost_event(
                                        &sessions,
                                        &format!(
                                            "[Ghost Shell Failed] Autonomous remediation failed for alert: {} — {} — session log: {}",
                                            rb_clone.name, e, session_log
                                        ),
                                    );
                                }
                            }
                        }
                        Err(e) => log::error!("Ghost Shell: failed to start: {}", e),
                    }
                });
            } // end else (capacity check)
        }

        let analysis = format!(
            "[Webhook Analysis] {}: {}",
            alert.alert_name,
            response.trim()
        );
        let analysis_msg = Message {
            role: "user".to_string(),
            content: format!("[Webhook Alert]\n{}", analysis),
            tool_calls: None,
            tool_results: None,
            turn: None,
        };
        inject_into_sessions(&state.sessions, &analysis_msg);
        let first_line = analysis.lines().next().unwrap_or(&analysis).to_string();
        notify_chat_panes(&state.sessions, &first_line);
        fire_notification(&alert.alert_name, &analysis, &state.config);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Severity ranking ──────────────────────────────────────────────────

    #[test]
    fn severity_rank_ordering() {
        assert!(severity_rank("critical") > severity_rank("warning"));
        assert!(severity_rank("warning") > severity_rank("info"));
        assert!(severity_rank("info") > severity_rank("unknown"));
    }

    #[test]
    fn severity_rank_case_insensitive() {
        assert_eq!(severity_rank("CRITICAL"), severity_rank("critical"));
        assert_eq!(severity_rank("Warning"), severity_rank("warning"));
    }

    // ── camel_to_kebab ────────────────────────────────────────────────────

    #[test]
    fn camel_to_kebab_basic() {
        assert_eq!(camel_to_kebab("HighDiskUsage"), "high-disk-usage");
    }

    #[test]
    fn camel_to_kebab_already_lowercase() {
        assert_eq!(camel_to_kebab("alert"), "alert");
    }

    #[test]
    fn camel_to_kebab_single_word() {
        assert_eq!(camel_to_kebab("Alert"), "alert");
    }

    #[test]
    fn camel_to_kebab_consecutive_uppercase() {
        // "CPUHigh" → "c-p-u-high" (each caps gets a dash)
        assert_eq!(camel_to_kebab("CPUHigh"), "c-p-u-high");
    }

    // ── parse_ghost_trigger ───────────────────────────────────────────────

    #[test]
    fn ghost_trigger_yes_detected() {
        let r = "ALERT: disk usage at 95%.\nGHOST_TRIGGER: YES";
        assert_eq!(parse_ghost_trigger(r), Some(true));
    }

    #[test]
    fn ghost_trigger_no_detected() {
        let r = "OK: disk usage is normal.\nGHOST_TRIGGER: NO";
        assert_eq!(parse_ghost_trigger(r), Some(false));
    }

    #[test]
    fn ghost_trigger_case_insensitive() {
        assert_eq!(parse_ghost_trigger("ghost_trigger: yes"), Some(true));
        assert_eq!(parse_ghost_trigger("Ghost_Trigger: No"), Some(false));
    }

    #[test]
    fn ghost_trigger_absent_returns_none() {
        assert_eq!(parse_ghost_trigger("ALERT: something is wrong."), None);
        assert_eq!(parse_ghost_trigger("OK: everything fine."), None);
        assert_eq!(parse_ghost_trigger(""), None);
    }

    #[test]
    fn ghost_trigger_scans_last_occurrence() {
        // If the field appears more than once, the last one wins.
        let r = "GHOST_TRIGGER: NO\nSome analysis.\nGHOST_TRIGGER: YES";
        assert_eq!(parse_ghost_trigger(r), Some(true));
    }

    #[test]
    fn ghost_trigger_whitespace_trimmed() {
        assert_eq!(parse_ghost_trigger("  GHOST_TRIGGER: YES  "), Some(true));
        assert_eq!(parse_ghost_trigger("  GHOST_TRIGGER: NO  "), Some(false));
    }

    // ── evaluate_watchdog_response ─────────────────────────────────────────

    #[test]
    fn evaluate_ghost_trigger_yes() {
        let (act, reason) = evaluate_watchdog_response("ALERT\nGHOST_TRIGGER: YES", false);
        assert!(act);
        assert_eq!(reason, "GHOST_TRIGGER: YES");
    }

    #[test]
    fn evaluate_ghost_trigger_no() {
        let (act, reason) = evaluate_watchdog_response("OK\nGHOST_TRIGGER: NO", false);
        assert!(!act);
        assert_eq!(reason, "GHOST_TRIGGER: NO");
    }

    #[test]
    fn evaluate_legacy_alert_keyword() {
        let (act, reason) = evaluate_watchdog_response("ALERT: disk is full", false);
        assert!(act);
        assert_eq!(reason, "legacy ALERT keyword (no GHOST_TRIGGER line found)");
    }

    #[test]
    fn evaluate_no_trigger_no_alert() {
        let (act, reason) = evaluate_watchdog_response("OK: everything looks fine", false);
        assert!(!act);
        assert_eq!(
            reason,
            "no GHOST_TRIGGER line and no ALERT keyword in response"
        );
    }

    #[test]
    fn evaluate_empty_response_no_api_error() {
        let (act, reason) = evaluate_watchdog_response("", false);
        assert!(!act);
        assert_eq!(reason, "empty_response — model returned no tokens");
    }

    #[test]
    fn evaluate_api_error_empty_response() {
        let (act, reason) = evaluate_watchdog_response("", true);
        assert!(!act);
        assert_eq!(reason, "api_error — response empty");
    }

    #[test]
    fn evaluate_api_error_with_partial_response_uses_content() {
        // If api_error=true but there IS a response, still evaluate the content.
        let (act, reason) = evaluate_watchdog_response("GHOST_TRIGGER: YES", true);
        assert!(act);
        assert_eq!(reason, "GHOST_TRIGGER: YES");
    }
}
