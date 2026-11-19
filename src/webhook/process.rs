use std::sync::Arc;

use crate::ai::{AiEvent, Message};
use crate::daemon::ghost::GhostManager;
use crate::daemon::session::{
    SessionStore, append_session_message, presence_broadcast, presence_session_ids, with_sessions,
};
use crate::daemon::utils::{UnpoisonExt, log_event};

use super::{parse::InternalAlert, server::WebhookState};

// ---------------------------------------------------------------------------
// Severity ranking
// ---------------------------------------------------------------------------

/// Rank a severity string: 3 = critical, 2 = warning/warn, 1 =
/// info/informational, `None` for anything else (unrankable). `pub(crate)`
/// so the active-alert registry (`active.rs`) sorts with the same table
/// rather than a copy of it.
pub(crate) fn severity_rank(s: &str) -> Option<u8> {
    match s.to_lowercase().as_str() {
        "critical" => Some(3),
        "warning" | "warn" => Some(2),
        "info" | "informational" => Some(1),
        _ => None,
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

/// `now_secs` for sibling modules (`active.rs`'s snapshot): the same clock,
/// re-exported rather than duplicated.
pub(crate) fn now_secs_pub() -> u64 {
    now_secs()
}

/// The dedup half of `process_alert`'s step 1 (M37 phase 01 extraction, for
/// the function-length cap): clamp the window to 1–86400 s, discard a
/// fingerprint seen inside the window, and cap the map at 10,000 entries by
/// evicting the oldest. Returns `true` when the alert is a duplicate (the
/// caller returns immediately).
fn is_duplicate(
    alert: &InternalAlert,
    dedup: &std::sync::Mutex<std::collections::HashMap<String, u64>>,
    window_cfg: u64,
) -> bool {
    // Clamp the window to a safe range: 1 s minimum (0 would disable dedup
    // entirely), 86400 s maximum (prevents unbounded HashMap growth from a
    // misconfigured very-long window keeping entries alive indefinitely).
    let window = window_cfg.clamp(1, 86400);
    let mut dedup = dedup.lock().unwrap_or_log();
    let now = now_secs();
    if let Some(&last_seen) = dedup.get(&alert.fingerprint)
        && now.saturating_sub(last_seen) < window
    {
        log::debug!(
            "Webhook: suppressed duplicate alert '{}' (fingerprint: {})",
            alert.alert_name,
            crate::util::truncate_chars(&alert.fingerprint, 16)
        );
        crate::daemon::utils::log_event(
            "webhook_discarded",
            serde_json::json!({
                "reason": "duplicate",
                "alert_name": alert.alert_name,
                "fingerprint": crate::util::truncate_chars(&alert.fingerprint, 16),
            }),
        );
        return true;
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
    false
}

pub async fn process_alert(alert: InternalAlert, state: Arc<WebhookState>) {
    crate::daemon::stats::record_webhook();
    let cfg = &state.config.webhook;

    // 1. Deduplication check: a duplicate is discarded here.
    if is_duplicate(&alert, &state.dedup, cfg.dedup_window_secs) {
        return;
    }

    // 2. Mask sensitive data in summary + description.
    let summary = crate::ai::filter::mask_sensitive(&alert.summary);
    let description = crate::ai::filter::mask_sensitive(&alert.description);

    // 3. Format human-readable alert message.
    let fp_short = crate::util::truncate_chars(&alert.fingerprint, 8);
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

    // 5. Publish the alert notice (M38 phase 03): the bus fans it out to the
    //    sinks — history, chat notice, command, alert registry —
    //    each behind its own routes. The severity the gate ranks is the
    //    notice's parsed severity; an unrankable raw string degrades to
    //    `Debug`, which sits below every threshold (the M37 fail-closed rule).
    //    The title's first line is the chat-notice overlay; the body is the
    //    remaining formatted lines.
    let notice_severity = crate::bus::Severity::from_external(&alert.severity);
    let (title, body) = split_first_line(&formatted);
    let notice = crate::bus::Notice::new(
        notice_severity,
        crate::bus::Source::Webhook,
        &alert.alert_name,
        format!("[Webhook Alert] {}", title),
    )
    .source_id(alert.source)
    .status(match alert.status {
        crate::webhook::parse::AlertStatus::Firing => crate::bus::NoticeStatus::Firing,
        crate::webhook::parse::AlertStatus::Resolved => crate::bus::NoticeStatus::Resolved,
    })
    .fingerprint(&alert.fingerprint)
    .body(body)
    .label("severity", &alert.severity)
    .label("kind", "alert");
    let published = state.bus.publish(notice);

    // 6. Analyze gate — the shared predicate the ghost-trigger consumer
    //    applies; the producer no longer calls the analysis. The config is
    //    read per alert so `daemoneye reload` applies route changes. An
    //    alert whose severity cannot be ranked degrades to `Debug`, which
    //    sits below `warning` — the fail-closed rule.
    if !crate::daemon::notice_sinks::analyze_routed(
        &crate::config::current().notifications,
        &published,
    ) {
        log::warn!(
            "Webhook: alert '{}' discarded — severity '{}' below threshold 'warning'",
            alert.alert_name,
            alert.severity
        );
        crate::daemon::utils::log_event(
            "webhook_discarded",
            serde_json::json!({
                "reason": "below_threshold",
                "alert_name": alert.alert_name,
                "severity": alert.severity,
                "threshold": "warning",
            }),
        );
    }
}

/// Split a formatted message into its first line and the rest: the title and
/// body of the notice built from it. The remaining lines keep their newlines.
fn split_first_line(text: &str) -> (String, String) {
    match text.split_once('\n') {
        Some((first, rest)) => (first.to_string(), rest.to_string()),
        None => (text.to_string(), String::new()),
    }
}

/// Append the alert message to every active session — both the on-disk JSONL
/// file and the in-memory entry (so it appears in the next AI turn regardless
/// of whether the session was idle or active).
pub(crate) fn inject_into_sessions(sessions: &SessionStore, msg: &Message) {
    // The union of the store's resident ids and every session with a live
    // presence connection. Since M29 phase 05 an attach inserts the entry
    // from disk, so every attached chat is resident: the union still guards
    // a session whose entry was evicted while its connection is live.
    let mut ids: Vec<String> = with_sessions(sessions, |store| store.keys().cloned().collect());
    ids.extend(presence_session_ids());
    ids.sort();
    ids.dedup();

    // Every target gets the file append; resident entries get the in-memory
    // push too so the next turn sees the alert without a disk re-read.
    for sid in &ids {
        append_session_message(sid, msg);
    }
    with_sessions(sessions, |store| {
        for entry in store.values_mut() {
            entry.push_event(msg.clone());
        }
    });
}

/// Send a one-line alert notification to every active chat pane, and push the
/// same line to every chat attached over the presence connection (M27 phase
/// 03b). The tmux overlay stays as the optional extra — an attached chat
/// without tmux still sees the alert.
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let panes: Vec<String> = with_sessions(sessions, |store| {
        store
            .values()
            .filter_map(|e| e.chat_pane().map(str::to_string))
            .collect()
    });

    // Unlocked phase: everything blocking happens out here.
    for pane in &panes {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "8000", "-t", pane, msg])
            .output();
    }
    presence_broadcast(msg);
}

/// The kind of ghost shell lifecycle event a caller reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostEvent {
    Started,
    Completed,
    Failed,
    Skipped,
}

/// The `Ghost` notice for a ghost shell lifecycle event: severity follows the
/// kind, the title is the content's first line and the body the rest, so
/// `history_text()` equals the content byte for byte. Pure so the mapping is
/// testable without a bus.
pub fn ghost_event_notice(kind: GhostEvent, name: &str, content: &str) -> crate::bus::Notice {
    let severity = match kind {
        GhostEvent::Started | GhostEvent::Completed => crate::bus::Severity::Info,
        GhostEvent::Failed => crate::bus::Severity::Error,
        GhostEvent::Skipped => crate::bus::Severity::Warning,
    };
    let kind_label = match kind {
        GhostEvent::Started => "started",
        GhostEvent::Completed => "completed",
        GhostEvent::Failed => "failed",
        GhostEvent::Skipped => "skipped",
    };
    let (title, body) = split_first_line(content);
    crate::bus::Notice::new(severity, crate::bus::Source::Ghost, name, title)
        .body(body)
        .label("kind", kind_label)
}

/// Publish a ghost shell lifecycle event on the bus and mirror it to
/// `ghost_lifecycle` in events.jsonl for troubleshooting.
pub fn publish_ghost_event(bus: &crate::bus::Bus, kind: GhostEvent, name: &str, content: &str) {
    bus.publish(ghost_event_notice(kind, name, content));
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
/// Returns `(should_act, trigger_reason)`.  Only the structured
/// `GHOST_TRIGGER: YES/NO` field can trigger action; anything without it is a
/// hard no (the legacy `ALERT` keyword heuristic was removed in H3 because a
/// model remarking "no ALERT condition" could spuriously launch a ghost).
/// Pass `api_error=true` when the API call itself failed so the reason string
/// is informative.
pub(crate) fn evaluate_watchdog_response(response: &str, api_error: bool) -> (bool, &'static str) {
    if api_error && response.is_empty() {
        (false, "api_error — response empty")
    } else if response.is_empty() {
        (false, "empty_response — model returned no tokens")
    } else {
        match parse_ghost_trigger(response) {
            Some(true) => (true, "GHOST_TRIGGER: YES"),
            Some(false) => (false, "GHOST_TRIGGER: NO"),
            None => (
                false,
                "no GHOST_TRIGGER line — refusing to act (strict mode)",
            ),
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

/// Outcome of a single-turn, tool-free watchdog analysis against a runbook.
pub(crate) struct WatchdogResult {
    pub response: String,
    pub api_error: Option<String>,
    pub should_act: bool,
    pub trigger_reason: &'static str,
}

/// Run a single-turn, tool-free watchdog LLM query against `runbook` with
/// `user_content`, emit its `watchdog` cost attribution, and evaluate its
/// structured `GHOST_TRIGGER: YES/NO` verdict. Shared by webhook alert
/// analysis and scheduled script watchdog analysis.
pub(crate) async fn run_watchdog_query(
    config: &crate::config::Config,
    runbook: &crate::runbook::Runbook,
    user_content: String,
) -> WatchdogResult {
    let system = crate::runbook::watchdog_system_prompt(runbook);
    let msgs = vec![Message {
        role: "user".to_string(),
        content: user_content,
        tool_calls: None,
        tool_results: None,
        turn: None,
    }];
    let model_entry = config.resolve_model(None);
    let client = crate::ai::make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
        model_entry.effective_max_tokens(),
    );

    let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
    let api_error = client
        .chat(&system, msgs, ai_tx, false, Vec::new())
        .await
        .err()
        .map(|e| e.to_string());

    let mut response = String::new();
    while let Some(ev) = ai_rx.recv().await {
        match ev {
            AiEvent::Token(t) => response.push_str(&t),
            AiEvent::Done(usage) => {
                crate::cost::emit_cost(crate::cost::CostCall {
                    model_entry,
                    usage: &usage,
                    attribution: &crate::cost::side_call_attribution("watchdog"),
                    session_id: "-",
                    run_id: "",
                    now: chrono::Utc::now(),
                });
            }
            _ => {}
        }
    }

    let (should_act, trigger_reason) = evaluate_watchdog_response(&response, api_error.is_some());
    WatchdogResult {
        response,
        api_error,
        should_act,
        trigger_reason,
    }
}

/// Run runbook-based AI analysis for the alert, rate-limited per alert name.
pub(crate) async fn maybe_analyze_alert(notice: &crate::bus::Notice, state: &WebhookState) {
    // Rate limit: skip if we analysed the same alert_name within dedup_window_secs.
    {
        let mut rl = state.rate_limit.lock().unwrap_or_log();
        let now = now_secs();
        if let Some(&last) = rl.get(&notice.name)
            && now.saturating_sub(last) < state.config.webhook.dedup_window_secs
        {
            return;
        }
        rl.insert(notice.name.clone(), now);
    }

    let Some(rb) = find_runbook_for_alert(&notice.name) else {
        log::debug!(
            "Webhook: no runbook found for alert '{}' (tried kebab-case, lowercase, exact match) — skipping analysis",
            notice.name
        );
        return;
    };

    // The daily cap gates the analysis before the model call: over
    // `[budget] max_cost_per_day` the analysis is skipped and logged, the
    // alert is still logged and the session goes on, nothing is retried.
    if let Err(message) =
        crate::daemon::spend::cost_gate(&state.config, crate::daemon::spend::SpendPath::Webhook)
    {
        log::warn!(
            "Webhook: analysis for '{}' skipped — daily cost cap: {message}",
            notice.name
        );
        return;
    }

    log::info!(
        "Webhook: running runbook analysis for '{}' using runbook '{}'",
        notice.name,
        rb.name
    );

    // The analysis reads the alert from the notice: the formatted text is
    // the notice's history text without its `[Webhook Alert] ` prefix.
    let alert_msg = notice
        .history_text()
        .strip_prefix("[Webhook Alert] ")
        .unwrap_or(&notice.history_text())
        .to_string();

    let wd = run_watchdog_query(&state.config, &rb, format!("Incoming alert:\n{alert_msg}")).await;
    if let Some(ref e) = wd.api_error {
        log::error!(
            "Webhook: runbook analysis API call failed for '{}': {}",
            notice.name,
            e
        );
    }

    log::info!(
        "Webhook: analysis for '{}' complete — should_act={} reason='{}' ghost_enabled={} (response: {} chars)",
        notice.name,
        wd.should_act,
        wd.trigger_reason,
        rb.ghost_config.enabled,
        wd.response.len()
    );
    if !wd.should_act && rb.ghost_config.enabled {
        log::info!(
            "Webhook: ghost shell NOT triggered for '{}' — reason: {}",
            notice.name,
            wd.trigger_reason
        );
    }
    log::debug!(
        "Webhook: analysis response for '{}':\n{}",
        notice.name,
        wd.response.trim()
    );

    log_event(
        "webhook_analysis",
        serde_json::json!({
            "alert_name": notice.name,
            "runbook": rb.name,
            "ghost_trigger": wd.should_act,
            "trigger_reason": wd.trigger_reason,
            "ghost_enabled": rb.ghost_config.enabled,
        }),
    );

    if wd.should_act {
        if rb.ghost_config.enabled && !spawn_webhook_ghost(&notice.name, &alert_msg, &rb, state) {
            return;
        }
        let analysis = analysis_notice(notice, &wd.response);
        state.bus.publish(analysis);
    }
}

/// Spawn a webhook-triggered Ghost Shell session if capacity allows.
/// Returns `false` when the concurrency limit is reached and the ghost is skipped.
fn spawn_webhook_ghost(
    alert_name: &str,
    alert_msg: &str,
    rb: &crate::runbook::Runbook,
    state: &WebhookState,
) -> bool {
    if !crate::daemon::ghost::check_ghost_capacity(&state.config) {
        log::warn!(
            "Webhook: ghost shell skipped for '{}' — concurrency limit ({}) reached",
            alert_name,
            state.config.ghost.max_concurrent_ghosts
        );
        publish_ghost_event(
            &state.bus,
            GhostEvent::Skipped,
            alert_name,
            &format!(
                "[Ghost Shell Skipped] Concurrency limit reached for alert: {}",
                alert_name
            ),
        );
        return false;
    }
    log::info!("Webhook: triggering Ghost Shell for '{}'", alert_name);
    let sessions = state.sessions.clone();
    let alert_msg = alert_msg.to_string();
    let rb_clone = rb.clone();
    let config_clone = state.config.clone();
    let cache_clone = state.cache.clone();
    let schedule_store_clone = state.schedule_store.clone();
    let bus = state.bus.clone();

    let _handle = tokio::spawn(async move {
        let merged_config = crate::agents::merge_runbook_ghost_config(&rb_clone);
        match GhostManager::start_session_with_config(
            sessions.clone(),
            &rb_clone,
            &merged_config,
            &alert_msg,
            crate::daemon::GhostTrigger::Webhook,
            config_clone.approvals.ghost_commands,
        )
        .await
        {
            Ok(sid) => {
                let session_log = crate::daemon::session::session_file(&sid)
                    .display()
                    .to_string();
                publish_ghost_event(
                    &bus,
                    GhostEvent::Started,
                    &rb_clone.name,
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
                        publish_ghost_event(
                            &bus,
                            GhostEvent::Completed,
                            &rb_clone.name,
                            &format!(
                                "[Ghost Shell Completed] Autonomous remediation finished for alert: {} — session log: {}",
                                rb_clone.name, session_log
                            ),
                        );
                    }
                    Err(e) => {
                        log::error!("Ghost Turn: failed for {}: {}", sid, e);
                        publish_ghost_event(
                            &bus,
                            GhostEvent::Failed,
                            &rb_clone.name,
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
    true
}

/// The `[Webhook Analysis]` notice: the alert's severity (parsed from the
/// notice's raw `severity` label), source `Webhook`, name the notice's name,
/// status `Event`, labels `severity` (raw) and `kind` = `analysis`. The
/// title is `"[Webhook Analysis] <name>: "` plus the response's first line;
/// the body is the rest. Pure so the split is testable without a bus.
pub fn analysis_notice(alert: &crate::bus::Notice, response: &str) -> crate::bus::Notice {
    let trimmed = response.trim();
    let (first, rest) = split_first_line(trimmed);
    crate::bus::Notice::new(
        alert.severity,
        crate::bus::Source::Webhook,
        &alert.name,
        format!("[Webhook Analysis] {}: {}", alert.name, first),
    )
    .body(rest)
    .label(
        "severity",
        alert
            .labels
            .get("severity")
            .map(String::as_str)
            .unwrap_or(alert.severity.as_str()),
    )
    .label("kind", "analysis")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session::SessionEntry;
    use crate::ipc::Response;

    // ── Severity ranking ──────────────────────────────────────────────────

    #[test]
    fn severity_rank_ordering() {
        assert_eq!(severity_rank("critical"), Some(3));
        assert_eq!(severity_rank("warning"), Some(2));
        assert_eq!(severity_rank("warn"), Some(2));
        assert_eq!(severity_rank("info"), Some(1));
        assert_eq!(severity_rank("informational"), Some(1));
        assert_eq!(severity_rank("unknown"), None);
        assert_eq!(severity_rank(""), None);
        assert_eq!(severity_rank("  "), None);
        assert_eq!(severity_rank("banana"), None);
    }

    #[test]
    fn severity_rank_case_insensitive() {
        assert_eq!(severity_rank("CRITICAL"), Some(3));
        assert_eq!(severity_rank("Warning"), Some(2));
        assert_eq!(severity_rank("INFO"), Some(1));
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
    fn evaluate_alert_keyword_alone_does_not_act() {
        // The legacy ALERT-keyword heuristic is gone (H3): a model remarking
        // "…no ALERT condition…" must not launch a ghost.  Without an explicit
        // final GHOST_TRIGGER: YES line, nothing happens — strict mode.
        let (act, reason) = evaluate_watchdog_response("ALERT: disk is full", false);
        assert!(!act);
        assert_eq!(
            reason,
            "no GHOST_TRIGGER line — refusing to act (strict mode)"
        );
    }

    #[test]
    fn evaluate_no_trigger_no_alert() {
        let (act, reason) = evaluate_watchdog_response("OK: everything looks fine", false);
        assert!(!act);
        assert_eq!(
            reason,
            "no GHOST_TRIGGER line — refusing to act (strict mode)"
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

    // ── presence push (M27 phase 03b) ─────────────────────────────────────

    fn msg(content: &str) -> Message {
        Message {
            role: "system".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    #[test]
    fn inject_into_sessions_updates_resident_history() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            let mut e = SessionEntry::new_interactive("test");
            e.history.messages = vec![msg("first"), msg("second")];
            store.insert("resident-1".to_string(), e);
        });

        let alert = msg("[Webhook Alert] disk usage at 95%");
        inject_into_sessions(&sessions, &alert);

        with_sessions(&sessions, |store| {
            let entry = store.get("resident-1").expect("the entry is resident");
            assert_eq!(
                entry.history.messages.len(),
                3,
                "the injected alert lands in the resident history"
            );
            assert_eq!(
                entry.history.messages.last().map(|m| m.content.as_str()),
                Some("[Webhook Alert] disk usage at 95%"),
                "the alert is the last message"
            );
        });
    }

    #[test]
    fn inject_reaches_a_presence_attached_session_that_is_not_resident() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();
        let sessions = SessionStore::new();

        let (push_tx, mut push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection {
            push: push_tx.clone(),
        };
        let sid = "reconnected-after-restart";
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn.clone()),
            1,
            "the reconnected chat is attached over presence"
        );
        assert!(
            !with_sessions(&sessions, |store| store.contains_key(sid)),
            "precondition: the reattached chat has no in-memory entry yet"
        );

        let alert = msg("[Webhook Alert] disk usage at 95%");
        inject_into_sessions(&sessions, &alert);

        let file = crate::config::sessions_dir().join(format!("{sid}.jsonl"));
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("the session file {file:?} exists: {e}"));
        assert!(
            text.contains("[Webhook Alert] disk usage at 95%"),
            "the alert lands in the session file of the non-resident reattached chat"
        );
        assert!(
            with_sessions(&sessions, |store| !store.contains_key(sid)),
            "no empty in-memory entry is created — handle_ask would read it instead of the file"
        );
        assert!(
            push_rx.try_recv().is_err(),
            "inject_into_sessions does not push over presence; notify_chat_panes does"
        );

        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the presence connection is released"
        );
        assert!(
            !crate::daemon::session::presence_session_ids().contains(&sid.to_string()),
            "this test's presence connection is released"
        );
    }

    #[tokio::test]
    async fn notify_chat_panes_pushes_over_presence() {
        let (_home, _dir) = crate::test_home();
        let sessions = SessionStore::new();
        let (mut rx, _client_w, task) = drive_presence(sessions.clone(), "webhook-notify-1").await;
        let replies = read_presence_responses(&mut rx, 1).await;
        assert!(
            matches!(replies.as_slice(), [Response::Ok]),
            "no detach state → Ok alone, got {replies:?}"
        );

        notify_chat_panes(&sessions, "WEBHOOK ALERT: disk usage at 95%");

        let pushed = read_until_text(&mut rx, "WEBHOOK ALERT: disk usage at 95%").await;
        assert!(
            matches!(pushed.last(), Some(Response::SystemMsg(m)) if m == "WEBHOOK ALERT: disk usage at 95%"),
            "the attached chat receives the alert line as a SystemMsg: {pushed:?}"
        );

        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            drop(_client_w);
            task.await
        })
        .await;
        let _ = ended.map(|r| r.expect("handler joins").expect("clean EOF"));
    }

    /// Attach one live presence connection over a duplex, with the daemon side
    /// in a task. Unique session id per test; delivery is observed by reading
    /// the client.
    async fn drive_presence(
        sessions: SessionStore,
        session_id: &str,
    ) -> (
        tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server);
        let sid = session_id.to_string();
        let task = tokio::spawn(async move {
            let mut rx = tokio::io::BufReader::new(server_r);
            let mut tx = server_w;
            crate::daemon::server::handle_chat_attach(sessions, sid, &mut rx, &mut tx).await
        });
        let (client_r, client_w) = tokio::io::split(client);
        (tokio::io::BufReader::new(client_r), client_w, task)
    }

    /// Read exactly `n` response lines from the client side, each within 5 s.
    async fn read_presence_responses(
        rx: &mut tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        n: usize,
    ) -> Vec<Response> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let mut line = String::new();
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::io::AsyncBufReadExt::read_line(rx, &mut line),
            )
            .await
            .expect("a response line within 5 s")
            .expect("the attach handler is still writing");
            assert!(read > 0, "the attach handler wrote a non-empty line");
            out.push(serde_json::from_str(line.trim()).expect("a response line parses"));
        }
        out
    }

    /// Read response lines until one carries `text` — a concurrent test's
    /// global broadcast can interleave its own SystemMsg on this connection's
    /// stream, so delivery is observed by content, not by count or position.
    async fn read_until_text(
        rx: &mut tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        text: &str,
    ) -> Vec<Response> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !left.is_zero(),
                "the pushed line '{text}' never arrived within 5 s: {out:?}"
            );
            let mut line = String::new();
            let read =
                tokio::time::timeout(left, tokio::io::AsyncBufReadExt::read_line(rx, &mut line))
                    .await
                    .expect("a response line within the deadline")
                    .expect("the attach handler is still writing");
            assert!(read > 0, "the attach handler wrote a non-empty line");
            let resp: Response = serde_json::from_str(line.trim()).expect("a response line parses");
            let hit = matches!(&resp, Response::SystemMsg(m) if m == text);
            out.push(resp);
            if hit {
                return out;
            }
        }
    }

    // ── ghost notice builder (M38 phase 05) ──────────────────────────────

    fn resident_entry() -> SessionEntry {
        SessionEntry::new_interactive("")
    }

    #[test]
    fn ghost_event_notice_maps_kind_to_severity() {
        let cases = [
            (GhostEvent::Started, "started", crate::bus::Severity::Info),
            (
                GhostEvent::Completed,
                "completed",
                crate::bus::Severity::Info,
            ),
            (GhostEvent::Failed, "failed", crate::bus::Severity::Error),
            (
                GhostEvent::Skipped,
                "skipped",
                crate::bus::Severity::Warning,
            ),
        ];
        for (kind, label, severity) in cases {
            let n = ghost_event_notice(kind, "rotate-logs", "[Ghost Shell X] body");
            assert_eq!(n.severity, severity, "{label} severity");
            assert_eq!(n.source, crate::bus::Source::Ghost, "{label} source");
            assert_eq!(
                n.labels.get("kind").map(String::as_str),
                Some(label),
                "{label} kind label"
            );
            assert_eq!(n.name, "rotate-logs", "{label} name");
        }
    }

    #[test]
    fn ghost_event_notice_history_text_is_the_content() {
        let one_line = ghost_event_notice(
            GhostEvent::Started,
            "rotate-logs",
            "[Ghost Shell Started] Autonomous remediation triggered",
        );
        assert_eq!(
            one_line.history_text(),
            "[Ghost Shell Started] Autonomous remediation triggered",
            "a one-line content round-trips unchanged"
        );
        assert_eq!(
            one_line.title, "[Ghost Shell Started] Autonomous remediation triggered",
            "a one-line content is all title"
        );

        let content = "[Ghost Shell Failed] Autonomous remediation failed for alert: disk-full — exit 1 — session log: /tmp/s.jsonl";
        let multi = ghost_event_notice(GhostEvent::Failed, "disk-full", content);
        assert_eq!(
            multi.history_text(),
            content,
            "the multi-line content round-trips byte for byte"
        );
        assert_eq!(
            multi.title,
            "[Ghost Shell Failed] Autonomous remediation failed for alert: disk-full — exit 1 — session log: /tmp/s.jsonl"
                .lines()
                .next()
                .expect("at least one line"),
            "the title is the content's first line"
        );
    }

    #[tokio::test]
    async fn publish_ghost_event_reaches_every_session() {
        let _guard = crate::test_home();
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            store.insert("a".to_string(), resident_entry());
            store.insert("b".to_string(), resident_entry());
        });

        let content =
            "[Ghost Shell Completed] Autonomous remediation finished for alert: disk-full";
        let bus =
            crate::daemon::notice_sinks::test_bus(sessions.clone(), crate::bus::Severity::Warning);
        publish_ghost_event(&bus, GhostEvent::Completed, "disk-full", content);

        for id in ["a", "b"] {
            let content_there = with_sessions(&sessions, |store| {
                store.get(id).map(|e| e.history.messages.len()).unwrap_or(0)
            });
            assert_eq!(content_there, 1, "session {id} gains exactly one message");
            let last = with_sessions(&sessions, |store| {
                store
                    .get(id)
                    .and_then(|e| e.history.messages.last().map(|m| m.content.clone()))
            });
            assert_eq!(
                last.as_deref(),
                Some(content),
                "session {id}'s message is the published string"
            );
        }
    }
}
