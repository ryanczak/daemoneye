use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::Value;

use crate::config::Config;
use crate::daemon::session::SessionStore;

use super::*;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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
}
