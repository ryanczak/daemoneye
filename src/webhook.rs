//! Webhook alert ingestion for DaemonEye.
//!
//! Listens on an HTTP port for alert payloads from Prometheus Alertmanager,
//! Grafana unified alerting, or a generic JSON format.  Received alerts are:
//!
//! 1. Deduplicated by fingerprint within a configurable window.
//! 2. Masked for sensitive data.
//! 3. Logged to `events.jsonl`.
//! 4. Injected into every active AI session history.
//! 5. Displayed via `tmux display-message` in all active chat panes.
//! 6. Optionally trigger runbook-based AI analysis (when a matching runbook exists).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::Value;

use crate::ai::{AiEvent, Message};
use crate::config::Config;
use crate::daemon::ghost::GhostManager;
use crate::daemon::session::{SessionStore, append_session_message};
use crate::daemon::utils::{UnpoisonExt, fire_notification, log_event};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Normalised representation of a single alert from any supported format.
#[derive(Debug, Clone)]
pub struct InternalAlert {
    pub alert_name: String,
    pub status: AlertStatus,
    /// "critical" | "warning" | "info" | ""
    pub severity: String,
    pub summary: String,
    pub description: String,
    #[allow(dead_code)]
    pub labels: HashMap<String, String>,
    /// Stable identity key used for deduplication.
    pub fingerprint: String,
    /// Original payload format: "alertmanager" | "grafana" | "generic"
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlertStatus {
    Firing,
    Resolved,
}

impl AlertStatus {
    fn as_str(&self) -> &'static str {
        match self {
            AlertStatus::Firing => "firing",
            AlertStatus::Resolved => "resolved",
        }
    }
}

/// Shared state passed to every Axum handler.
pub struct WebhookState {
    pub config: Config,
    pub sessions: SessionStore,
    pub cache: Arc<crate::tmux::cache::SessionCache>,
    pub schedule_store: Arc<crate::scheduler::ScheduleStore>,
    /// Fingerprint → last-seen timestamp (seconds since UNIX epoch).
    pub dedup: Mutex<HashMap<String, u64>>,
    /// Alert-name → last-analysis timestamp for rate-limiting AI analysis.
    pub rate_limit: Mutex<HashMap<String, u64>>,
}

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
// Alert parsers
// ---------------------------------------------------------------------------

/// Compute a stable fingerprint from sorted label key=value pairs.
fn fingerprint_from_labels(labels: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = labels.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse an Alertmanager (or Grafana unified alerting) payload.
/// Both formats use a top-level `"alerts"` array.
fn parse_alertmanager(body: &Value) -> Vec<InternalAlert> {
    let Some(alerts_arr) = body["alerts"].as_array() else {
        return Vec::new();
    };

    alerts_arr
        .iter()
        .filter_map(|a| {
            let labels: HashMap<String, String> = a["labels"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let alert_name = labels
                .get("alertname")
                .cloned()
                .unwrap_or_else(|| "UnknownAlert".to_string());

            let annotations = a["annotations"].as_object();
            let summary = annotations
                .and_then(|o| o.get("summary"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = annotations
                .and_then(|o| o.get("description").or_else(|| o.get("message")))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let severity = labels.get("severity").cloned().unwrap_or_default();

            let status = match a["status"].as_str().unwrap_or("firing") {
                "resolved" => AlertStatus::Resolved,
                _ => AlertStatus::Firing,
            };

            let fingerprint = a["fingerprint"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| fingerprint_from_labels(&labels));

            Some(InternalAlert {
                alert_name,
                status,
                severity,
                summary,
                description,
                labels,
                fingerprint,
                source: "alertmanager",
            })
        })
        .collect()
}

/// Parse the legacy Grafana webhook format (has a top-level `"state"` field,
/// no `"alerts"` array).
fn parse_grafana_legacy(body: &Value) -> Option<InternalAlert> {
    // Legacy Grafana uses "state": "alerting" | "ok" | "no_data"
    let state_str = body["state"].as_str()?;

    let alert_name = body["ruleName"]
        .as_str()
        .or_else(|| body["title"].as_str())
        .unwrap_or("GrafanaAlert")
        .to_string();

    let summary = body["title"]
        .as_str()
        .unwrap_or(alert_name.as_str())
        .to_string();
    let description = body["message"]
        .as_str()
        .or_else(|| body["description"].as_str())
        .unwrap_or("")
        .to_string();

    let status = if state_str == "ok" {
        AlertStatus::Resolved
    } else {
        AlertStatus::Firing
    };

    let labels: HashMap<String, String> = body["tags"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();

    let severity = labels.get("severity").cloned().unwrap_or_default();
    let fingerprint = fingerprint_from_labels(&labels);

    Some(InternalAlert {
        alert_name,
        status,
        severity,
        summary,
        description,
        labels,
        fingerprint,
        source: "grafana",
    })
}

/// Generic fallback: tries common key names; serialises full body if nothing matches.
fn parse_generic(body: &Value) -> Option<InternalAlert> {
    let alert_name = body["alertname"]
        .as_str()
        .or_else(|| body["name"].as_str())
        .or_else(|| body["title"].as_str())
        .or_else(|| body["alert_name"].as_str())
        .unwrap_or("GenericAlert")
        .to_string();

    let summary = body["summary"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .or_else(|| body["title"].as_str())
        .unwrap_or("")
        .to_string();

    let description = body["description"]
        .as_str()
        .or_else(|| body["details"].as_str())
        .unwrap_or_else(|| {
            // Fall back to full JSON body as description.
            ""
        })
        .to_string();

    let description = if description.is_empty() {
        serde_json::to_string_pretty(body).unwrap_or_default()
    } else {
        description
    };

    let severity = body["severity"]
        .as_str()
        .or_else(|| body["level"].as_str())
        .or_else(|| body["priority"].as_str())
        .unwrap_or("")
        .to_string();

    let status_str = body["status"]
        .as_str()
        .or_else(|| body["state"].as_str())
        .unwrap_or("firing");
    let status = if matches!(status_str, "resolved" | "ok" | "normal") {
        AlertStatus::Resolved
    } else {
        AlertStatus::Firing
    };

    let labels = HashMap::new();
    let fingerprint = format!("{}-{}", alert_name, severity);

    Some(InternalAlert {
        alert_name,
        status,
        severity,
        summary,
        description,
        labels,
        fingerprint,
        source: "generic",
    })
}

/// Top-level dispatcher: detect payload format and return parsed alerts.
pub fn parse_payload(body: &Value) -> Vec<InternalAlert> {
    // Alertmanager v4 and Grafana unified alerting both use "alerts" array.
    if body["alerts"].is_array() {
        let alerts = parse_alertmanager(body);
        if !alerts.is_empty() {
            return alerts;
        }
    }

    // Legacy Grafana webhook has a "state" field at the top level.
    if body["state"].is_string() {
        if let Some(a) = parse_grafana_legacy(body) {
            return vec![a];
        }
    }

    // Generic fallback.
    parse_generic(body).map(|a| vec![a]).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// HTTP handler
// ---------------------------------------------------------------------------

/// Returns true if the request is authorized for the given secret.
/// When `secret` is empty, all requests are allowed.
fn is_authorized(secret: &str, headers: &HeaderMap) -> bool {
    if secret.is_empty() {
        return true;
    }
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    auth == format!("Bearer {}", secret)
}

async fn handle_webhook(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> StatusCode {
    if !is_authorized(&state.config.webhook.secret, &headers) {
        log::warn!("Webhook: rejected request — invalid or missing Bearer token");
        crate::daemon::stats::record_webhook_rejected();
        return StatusCode::UNAUTHORIZED;
    }

    let alerts = parse_payload(&body);
    if alerts.is_empty() {
        log::warn!("Webhook: received payload with no parseable alerts");
        return StatusCode::BAD_REQUEST;
    }

    // Process each alert asynchronously so we return 200 immediately.
    for alert in alerts {
        let state2 = Arc::clone(&state);
        tokio::spawn(async move {
            process_alert(alert, state2).await;
        });
    }

    StatusCode::OK
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

async fn process_alert(alert: InternalAlert, state: Arc<WebhookState>) {
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
        if let Some(&last_seen) = dedup.get(&alert.fingerprint) {
            if now.saturating_sub(last_seen) < window {
                log::debug!(
                    "Webhook: suppressed duplicate alert '{}' (fingerprint: {})",
                    alert.alert_name,
                    &alert.fingerprint[..alert.fingerprint.len().min(16)]
                );
                return;
            }
        }
        // Cap the dedup map to 10,000 entries. When the cap is reached, evict
        // the oldest entry (smallest timestamp) to prevent unbounded growth
        // from alert storms with high fingerprint cardinality.
        const DEDUP_MAP_CAP: usize = 10_000;
        if dedup.len() >= DEDUP_MAP_CAP {
            if let Some(oldest_key) = dedup
                .iter()
                .min_by_key(|&(_, &ts)| ts)
                .map(|(k, _)| k.clone())
            {
                dedup.remove(&oldest_key);
            }
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
    let guard = sessions.lock().unwrap_or_log();
    for (sid, entry) in guard.iter() {
        append_session_message(sid, msg);
        // In-memory is intentionally NOT updated here — the next Ask request
        // will re-read from disk when the in-memory history is stale.
        // For sessions currently in flight this means the alert appears in the
        // turn after the one already in progress, which is acceptable.
        let _ = entry; // suppress unused-variable warning
    }
}

/// Send a one-line alert notification to every active chat pane.
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let guard = sessions.lock().unwrap_or_log();
    for entry in guard.values() {
        if let Some(ref pane) = entry.chat_pane {
            let _ = std::process::Command::new("tmux")
                .args(["display-message", "-d", "8000", "-t", pane, msg])
                .output();
        }
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
    };
    inject_into_sessions(sessions, &msg);
    // One-liner for the tmux display-message overlay (strip newlines).
    let one_liner = content.lines().next().unwrap_or(content);
    notify_chat_panes(sessions, one_liner);
    // Always mirror ghost lifecycle events to events.jsonl for troubleshooting.
    crate::daemon::utils::log_event(
        "ghost_lifecycle",
        serde_json::json!({ "content": content }),
    );
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
pub(crate) fn evaluate_watchdog_response(
    response: &str,
    api_error: bool,
) -> (bool, &'static str) {
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
                    (false, "no GHOST_TRIGGER line and no ALERT keyword in response")
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
        if let Some(&last) = rl.get(&alert.alert_name) {
            if now.saturating_sub(last) < state.config.webhook.dedup_window_secs {
                return;
            }
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
    }];

    let api_key = state.config.ai.resolve_api_key();
    let client = crate::ai::make_client(
        &state.config.ai.provider,
        api_key,
        state.config.ai.model.clone(),
        state.config.ai.effective_base_url(),
    );

    let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
    let api_err = if let Err(e) = client.chat(&system, msgs, ai_tx, false).await {
        log::error!("Webhook: runbook analysis API call failed for '{}': {}", alert.alert_name, e);
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

    let (should_act, trigger_reason) =
        evaluate_watchdog_response(&response, api_err.is_some());

    log::info!(
        "Webhook: analysis for '{}' complete — should_act={} reason='{}' ghost_enabled={} (response: {} chars)",
        alert.alert_name, should_act, trigger_reason, rb.ghost_config.enabled, response.len()
    );
    if !should_act && rb.ghost_config.enabled {
        log::info!(
            "Webhook: ghost shell NOT triggered for '{}' — reason: {}",
            alert.alert_name, trigger_reason
        );
    }
    log::debug!("Webhook: analysis response for '{}':\n{}", alert.alert_name, response.trim());

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
                match GhostManager::start_session(sessions.clone(), &rb_clone, &alert_msg, crate::daemon::GS_BG_WINDOW_PREFIX).await {
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
                        ).await {
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
        };
        inject_into_sessions(&state.sessions, &analysis_msg);
        let first_line = analysis.lines().next().unwrap_or(&analysis).to_string();
        notify_chat_panes(&state.sessions, &first_line);
        fire_notification(&alert.alert_name, &analysis, &state.config);
    }
}

// ---------------------------------------------------------------------------
// Axum router + entry point
// ---------------------------------------------------------------------------

/// Start the webhook HTTP server.  Runs until the process exits.
pub async fn start(
    config: Config,
    sessions: SessionStore,
    cache: Arc<crate::tmux::cache::SessionCache>,
    schedule_store: Arc<crate::scheduler::ScheduleStore>,
) -> anyhow::Result<()> {
    let port = config.webhook.port;
    let bind_ip: std::net::IpAddr = config
        .webhook
        .bind_addr
        .parse()
        .unwrap_or_else(|_| std::net::Ipv4Addr::LOCALHOST.into());
    let state = Arc::new(WebhookState {
        config,
        sessions,
        cache,
        schedule_store,
        dedup: Mutex::new(HashMap::new()),
        rate_limit: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::new(bind_ip, port)).await?;
    log::info!("Webhook server listening on {}:{}", bind_ip, port);
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Alertmanager parser ───────────────────────────────────────────────

    fn alertmanager_payload() -> Value {
        serde_json::json!({
            "version": "4",
            "groupKey": "{}:{alertname=\"HighDiskUsage\"}",
            "status": "firing",
            "receiver": "daemoneye",
            "alerts": [
                {
                    "status": "firing",
                    "labels": {
                        "alertname": "HighDiskUsage",
                        "severity": "critical",
                        "instance": "server01",
                        "job": "node"
                    },
                    "annotations": {
                        "summary": "Disk usage above 90%",
                        "description": "Disk /dev/sda1 is at 93% on server01"
                    },
                    "fingerprint": "abc12345"
                }
            ]
        })
    }

    #[test]
    fn alertmanager_parses_single_alert() {
        let alerts = parse_payload(&alertmanager_payload());
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.alert_name, "HighDiskUsage");
        assert_eq!(a.severity, "critical");
        assert_eq!(a.summary, "Disk usage above 90%");
        assert_eq!(a.description, "Disk /dev/sda1 is at 93% on server01");
        assert_eq!(a.status, AlertStatus::Firing);
        assert_eq!(a.fingerprint, "abc12345");
        assert_eq!(a.source, "alertmanager");
    }

    #[test]
    fn alertmanager_resolved_status() {
        let mut payload = alertmanager_payload();
        payload["alerts"][0]["status"] = serde_json::json!("resolved");
        let alerts = parse_payload(&payload);
        assert_eq!(alerts[0].status, AlertStatus::Resolved);
    }

    #[test]
    fn alertmanager_multiple_alerts() {
        let payload = serde_json::json!({
            "alerts": [
                {
                    "status": "firing",
                    "labels": { "alertname": "Alert1", "severity": "warning" },
                    "annotations": { "summary": "First alert" },
                    "fingerprint": "fp1"
                },
                {
                    "status": "firing",
                    "labels": { "alertname": "Alert2", "severity": "info" },
                    "annotations": { "summary": "Second alert" },
                    "fingerprint": "fp2"
                }
            ]
        });
        let alerts = parse_payload(&payload);
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].alert_name, "Alert1");
        assert_eq!(alerts[1].alert_name, "Alert2");
    }

    #[test]
    fn alertmanager_fingerprint_computed_from_labels_when_absent() {
        let payload = serde_json::json!({
            "alerts": [{
                "status": "firing",
                "labels": { "alertname": "Test", "severity": "warning" },
                "annotations": {}
            }]
        });
        let alerts = parse_payload(&payload);
        assert!(!alerts[0].fingerprint.is_empty());
        // Should be stable across calls.
        let alerts2 = parse_payload(&payload);
        assert_eq!(alerts[0].fingerprint, alerts2[0].fingerprint);
    }

    // ── Grafana legacy parser ─────────────────────────────────────────────

    fn grafana_legacy_payload() -> Value {
        serde_json::json!({
            "state": "alerting",
            "ruleName": "HighMemoryUsage",
            "title": "High memory usage on web01",
            "message": "Memory usage exceeded 85% threshold",
            "tags": {
                "severity": "warning",
                "team": "platform"
            }
        })
    }

    #[test]
    fn grafana_legacy_parses_firing() {
        let alerts = parse_payload(&grafana_legacy_payload());
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.alert_name, "HighMemoryUsage");
        assert_eq!(a.status, AlertStatus::Firing);
        assert_eq!(a.source, "grafana");
        assert_eq!(a.severity, "warning");
    }

    #[test]
    fn grafana_legacy_ok_maps_to_resolved() {
        let mut payload = grafana_legacy_payload();
        payload["state"] = serde_json::json!("ok");
        let alerts = parse_payload(&payload);
        assert_eq!(alerts[0].status, AlertStatus::Resolved);
    }

    // ── Generic parser ────────────────────────────────────────────────────

    #[test]
    fn generic_parses_alertname_field() {
        let payload = serde_json::json!({
            "alertname": "ServiceDown",
            "severity": "critical",
            "summary": "The payment service is down",
            "status": "firing"
        });
        let alerts = parse_payload(&payload);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].alert_name, "ServiceDown");
        assert_eq!(alerts[0].severity, "critical");
        assert_eq!(alerts[0].source, "generic");
    }

    #[test]
    fn generic_parses_name_field_fallback() {
        let payload = serde_json::json!({
            "name": "CPUHigh",
            "message": "CPU is high",
            "status": "firing"
        });
        let alerts = parse_payload(&payload);
        assert_eq!(alerts[0].alert_name, "CPUHigh");
    }

    #[test]
    fn generic_unknown_fields_uses_full_body_as_description() {
        let payload = serde_json::json!({ "foo": "bar", "baz": 42 });
        let alerts = parse_payload(&payload);
        assert_eq!(alerts.len(), 1);
        assert!(!alerts[0].description.is_empty());
    }

    #[test]
    fn generic_resolved_status() {
        let payload = serde_json::json!({
            "alertname": "Resolved",
            "status": "resolved"
        });
        let alerts = parse_payload(&payload);
        assert_eq!(alerts[0].status, AlertStatus::Resolved);
    }

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

    // ── Fingerprint stability ─────────────────────────────────────────────

    #[test]
    fn fingerprint_stable_regardless_of_label_order() {
        let mut labels1 = HashMap::new();
        labels1.insert("alertname".to_string(), "Test".to_string());
        labels1.insert("severity".to_string(), "warning".to_string());

        let mut labels2 = HashMap::new();
        labels2.insert("severity".to_string(), "warning".to_string());
        labels2.insert("alertname".to_string(), "Test".to_string());

        assert_eq!(
            fingerprint_from_labels(&labels1),
            fingerprint_from_labels(&labels2)
        );
    }

    // ── Bearer token authentication ───────────────────────────────────────

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        h
    }

    #[test]
    fn auth_empty_secret_always_allows() {
        assert!(is_authorized("", &HeaderMap::new()));
        assert!(is_authorized("", &headers_with_bearer("anything")));
    }

    #[test]
    fn auth_correct_token_allows() {
        assert!(is_authorized("mysecret", &headers_with_bearer("mysecret")));
    }

    #[test]
    fn auth_missing_header_denies() {
        assert!(!is_authorized("mysecret", &HeaderMap::new()));
    }

    #[test]
    fn auth_wrong_token_denies() {
        assert!(!is_authorized(
            "mysecret",
            &headers_with_bearer("wrongtoken")
        ));
    }

    #[test]
    fn auth_token_without_bearer_prefix_denies() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            "mysecret".parse().unwrap(),
        );
        assert!(!is_authorized("mysecret", &h));
    }

    // ── parse_ghost_trigger ──────────────────────────────────────────────────

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

    // ── evaluate_watchdog_response ───────────────────────────────────────────

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
        assert_eq!(reason, "no GHOST_TRIGGER line and no ALERT keyword in response");
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
