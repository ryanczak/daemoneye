use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::Value;

use crate::config::Config;
use crate::daemon::session::SessionStore;
use crate::daemon::utils::UnpoisonExt;

use super::*;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Fixed-window rate limit per source IP (H2). A flood of webhook posts
/// would otherwise spawn an AI analysis (and potentially a ghost shell) per
/// request, burning CPU and API budget.
const IP_WINDOW_SECS: u64 = 60;
const IP_WINDOW_MAX: u32 = 30;

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
    /// Source IP → (window start epoch, request count in window).
    pub ip_limits: Mutex<HashMap<IpAddr, (u64, u32)>>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Check the per-IP fixed window; returns `true` when the request should be
/// admitted (and bumps the counter), `false` when the window is exhausted.
fn admit_request(state: &WebhookState, ip: IpAddr) -> bool {
    let mut limits = state.ip_limits.lock().unwrap_or_log();
    let window = now_secs() / IP_WINDOW_SECS;
    let entry = limits.entry(ip).or_insert((window, 0));
    if entry.0 != window {
        *entry = (window, 0);
    }
    if entry.1 >= IP_WINDOW_MAX {
        return false;
    }
    entry.1 += 1;
    true
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
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Json(body): Json<Value>,
) -> StatusCode {
    let source_ip = addr.ip();
    if !admit_request(&state, source_ip) {
        log::warn!("Webhook: rate limit exceeded for {source_ip}");
        crate::daemon::stats::record_webhook_rejected();
        crate::daemon::utils::log_event(
            "webhook_discarded",
            serde_json::json!({
                "reason": "rate_limited",
                "alert_name": "-",
            }),
        );
        return StatusCode::TOO_MANY_REQUESTS;
    }

    if !is_authorized(&state.config.webhook.secret, &headers) {
        log::warn!("Webhook: rejected request — invalid or missing Bearer token");
        crate::daemon::stats::record_webhook_rejected();
        crate::daemon::utils::log_event(
            "webhook_discarded",
            serde_json::json!({
                "reason": "unauthorized",
                "alert_name": "-",
            }),
        );
        return StatusCode::UNAUTHORIZED;
    }

    let alerts = parse_payload(&body);
    if alerts.is_empty() {
        log::warn!("Webhook: received payload with no parseable alerts");
        crate::daemon::utils::log_event(
            "webhook_discarded",
            serde_json::json!({
                "reason": "unparseable",
                "alert_name": "-",
            }),
        );
        return StatusCode::BAD_REQUEST;
    }

    // Process each alert asynchronously so we return 200 immediately.
    for alert in alerts {
        let state2 = Arc::clone(&state);
        tokio::spawn(async move {
            let _handle = process_alert(alert, state2).await;
            // Handle is dropped here — the HTTP handler must return 200
            // without waiting for the ghost to finish.
        });
    }

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Axum router + entry point
// ---------------------------------------------------------------------------

/// Bind the webhook listener. Fatal at startup: a port already in use is the
/// strongest available signal that another daemon is running
/// (`docs/design/daemon-instance.md` § 4.2).
/// Fail-closed guard for the webhook listener (H2). Returns `Err` when the
/// configured bind would expose an unauthenticated webhook to the network.
fn validate_webhook_bind(bind_ip: &std::net::IpAddr, secret: &str) -> Result<(), String> {
    // Every webhook post can launch an autonomous ghost shell that runs
    // arbitrary commands on this host. Exposing that to the network without
    // a bearer token is an unauthenticated remote-code-execution vector, so
    // refuse to start rather than serve it.
    if !bind_ip.is_loopback() && secret.trim().is_empty() {
        return Err(format!(
            "refusing to start webhook listener on {bind_ip}: exposing DaemonEye's \
             webhook (which can launch ghost shells that run arbitrary commands) \
             to the network without a bearer token is unsafe. Set webhook.secret \
             in ~/.daemoneye/etc/config.toml or bind to 127.0.0.1."
        ));
    }
    Ok(())
}

pub async fn bind(config: &Config) -> anyhow::Result<tokio::net::TcpListener> {
    let port = config.webhook.port;
    let bind_ip: std::net::IpAddr = config
        .webhook
        .bind_addr
        .parse()
        .unwrap_or_else(|_| std::net::Ipv4Addr::LOCALHOST.into());
    validate_webhook_bind(&bind_ip, &config.webhook.secret)
        .map_err(anyhow::Error::msg)?;
    if config.webhook.enabled && config.webhook.secret.trim().is_empty() {
        log::warn!(
            "webhook listener on {bind_ip}:{port} requires NO auth — set webhook.secret \
             in ~/.daemoneye/etc/config.toml to require a Bearer token"
        );
    }
    tokio::net::TcpListener::bind(std::net::SocketAddr::new(bind_ip, port))
        .await
        .with_context(|| {
            format!(
                "failed to bind the webhook listener on {bind_ip}:{port} \
                 (is another daemon or another process already using it?)"
            )
        })
}

/// Serve on an already-bound listener. Runs until the process exits.
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: Config,
    sessions: SessionStore,
    cache: Arc<crate::tmux::cache::SessionCache>,
    schedule_store: Arc<crate::scheduler::ScheduleStore>,
) -> anyhow::Result<()> {
    let state = Arc::new(WebhookState {
        config,
        sessions,
        cache,
        schedule_store,
        dedup: Mutex::new(HashMap::new()),
        rate_limit: Mutex::new(HashMap::new()),
        ip_limits: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state);
    log::info!(
        "Webhook server listening on {}",
        listener
            .local_addr()
            .unwrap_or_else(|_| std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 0))
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── Fail-closed bind validation (H2) ───────────────────────────────────

    #[test]
    fn bind_rejects_non_loopback_without_secret() {
        let err = validate_webhook_bind(&std::net::IpAddr::V4("0.0.0.0".parse().unwrap()), "")
            .unwrap_err();
        assert!(err.contains("unsafe"), "got: {err}");
    }

    #[test]
    fn bind_rejects_any_external_addr_without_secret() {
        let ip: std::net::IpAddr = "192.168.1.10".parse().unwrap();
        assert!(validate_webhook_bind(&ip, "").is_err());
        let v6: std::net::IpAddr = "fd00::1".parse().unwrap();
        assert!(validate_webhook_bind(&v6, "").is_err());
    }

    #[test]
    fn bind_accepts_external_addr_with_secret() {
        let ip: std::net::IpAddr = "0.0.0.0".parse().unwrap();
        assert!(validate_webhook_bind(&ip, "s3cret").is_ok());
    }

    #[test]
    fn bind_accepts_loopback_without_secret() {
        // Local-only no-auth webhook is the historical default and stays
        // permitted (with a warning), since it cannot be reached remotely.
        let v4: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        let v6: std::net::IpAddr = "::1".parse().unwrap();
        assert!(validate_webhook_bind(&v4, "").is_ok());
        assert!(validate_webhook_bind(&v6, "").is_ok());
    }

    // ── Rate limiting (H2) ────────────────────────────────────────────────

    fn test_state() -> WebhookState {
        WebhookState {
            config: Config::default(),
            sessions: SessionStore::new(),
            cache: Arc::new(crate::tmux::cache::SessionCache::new("unused")), // unused in these tests
            schedule_store: Arc::new(crate::scheduler::ScheduleStore::new_empty()),
            dedup: Mutex::new(HashMap::new()),
            rate_limit: Mutex::new(HashMap::new()),
            ip_limits: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn rate_limit_allows_under_window() {
        let state = test_state();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..IP_WINDOW_MAX {
            assert!(admit_request(&state, ip));
        }
        assert!(!admit_request(&state, ip));
    }

    #[test]
    fn rate_limit_resets_next_window() {
        let state = test_state();
        let ip: IpAddr = "10.0.0.2".parse().unwrap();
        for _ in 0..IP_WINDOW_MAX {
            admit_request(&state, ip);
        }
        assert!(!admit_request(&state, ip));
        // Jump the internal clock a full window ahead by pretending the
        // window index moved:
        let window = now_secs() / IP_WINDOW_SECS + 100;
        *state.ip_limits.lock().unwrap().get_mut(&ip).unwrap() = (window, 0);
        assert!(admit_request(&state, ip));
    }
}
