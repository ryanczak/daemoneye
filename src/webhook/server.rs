use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use axum::{
    Router,
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

/// The most alerts one POST may enqueue (S10). Distinct alertname labels
/// defeat dedup, so an unbounded array would fan out hundreds of thousands
/// of concurrent `process_alert` tasks from a single authenticated POST.
const MAX_ALERTS_PER_POST: usize = 100;

/// Keep at most [`MAX_ALERTS_PER_POST`] of `alerts`, returning the kept
/// alerts and how many were dropped. Pure, so the cap is unit-testable
/// without an HTTP round trip.
fn cap_alerts(alerts: Vec<InternalAlert>) -> (Vec<InternalAlert>, usize) {
    let dropped = alerts.len().saturating_sub(MAX_ALERTS_PER_POST);
    (
        alerts.into_iter().take(MAX_ALERTS_PER_POST).collect(),
        dropped,
    )
}

/// Shared state passed to every Axum handler.
pub struct WebhookState {
    /// The daemon's live config handle — the same `Arc` `run_daemon`
    /// installs, so `daemoneye reload` reaches every reader here. Read it
    /// through `current()` and take one snapshot per code path.
    pub config: Arc<crate::config::ConfigHandle>,
    pub sessions: SessionStore,
    pub cache: Arc<crate::tmux::cache::SessionCache>,
    pub schedule_store: Arc<crate::scheduler::ScheduleStore>,
    /// The notice bus the producers publish on. Production passes
    /// [`crate::bus::global`]; a test passes [`crate::daemon::notice_sinks::test_bus`]
    /// so its notices reach only its own sinks.
    pub bus: Arc<crate::bus::Bus>,
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
/// Entries from windows other than the one being admitted are dropped, so
/// the map holds at most one window's worth of source IPs.
fn admit_request(state: &WebhookState, ip: IpAddr) -> bool {
    let mut limits = state.ip_limits.lock().unwrap_or_log();
    let window = now_secs() / IP_WINDOW_SECS;
    limits.retain(|_, (entry_window, _)| *entry_window == window);
    let entry = limits.entry(ip).or_insert((window, 0));
    if entry.1 >= IP_WINDOW_MAX {
        return false;
    }
    entry.1 += 1;
    true
}

// ---------------------------------------------------------------------------
// HTTP handler
// ---------------------------------------------------------------------------

/// Byte-equality with no early return on contents, so the comparison does not
/// leak how many leading bytes matched. The length may leak; the bytes may not.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

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
    let expected = format!("Bearer {}", secret);
    constant_time_eq(auth.as_bytes(), expected.as_bytes())
}

/// Axum's old default body cap before phase 09a moved the read past auth.
const MAX_WEBHOOK_BODY_BYTES: usize = 2 * 1024 * 1024;

async fn handle_webhook(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    body: axum::body::Body,
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

    if !is_authorized(&state.config.current().webhook.secret, &headers) {
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

    let bytes = match axum::body::to_bytes(body, MAX_WEBHOOK_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            log::warn!(
                "Webhook: body over {} bytes or unreadable",
                MAX_WEBHOOK_BODY_BYTES
            );
            crate::daemon::stats::record_webhook_rejected();
            crate::daemon::utils::log_event(
                "webhook_discarded",
                serde_json::json!({
                    "reason": "body_too_large",
                    "alert_name": "-",
                }),
            );
            return StatusCode::PAYLOAD_TOO_LARGE;
        }
    };

    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
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
    };

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

    // One task per POST, processing the kept alerts in turn: the array's
    // length is attacker-controlled, so an unbounded per-alert spawn would
    // fan out unboundedly from a single authenticated request (S10).
    let (alerts, dropped) = cap_alerts(alerts);
    if dropped > 0 {
        crate::daemon::utils::log_event(
            "webhook_discarded",
            serde_json::json!({
                "reason": "too_many_alerts",
                "dropped": dropped,
            }),
        );
    }
    tokio::spawn(async move {
        let state2 = Arc::clone(&state);
        for alert in alerts {
            process_alert(alert, state2.clone()).await;
        }
    });

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
pub(crate) fn validate_webhook_bind(
    bind_ip: &std::net::IpAddr,
    secret: &str,
) -> Result<(), String> {
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
    validate_webhook_bind(&bind_ip, &config.webhook.secret).map_err(anyhow::Error::msg)?;
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
    daemon: Arc<crate::daemon::DaemonState>,
) -> anyhow::Result<()> {
    let state = Arc::new(WebhookState {
        config: Arc::clone(&daemon.config),
        sessions: daemon.sessions.clone(),
        cache: Arc::clone(&daemon.tmux_cache),
        schedule_store: Arc::clone(&daemon.schedules),
        bus: Arc::clone(&daemon.bus),
        dedup: Mutex::new(HashMap::new()),
        rate_limit: Mutex::new(HashMap::new()),
        ip_limits: Mutex::new(HashMap::new()),
    });
    crate::webhook::trigger::register_ghost_trigger(Arc::clone(&state));

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

    #[test]
    fn constant_time_eq_matches_only_equal_bytes() {
        assert!(constant_time_eq(b"", b""), "two empty slices match");
        assert!(
            constant_time_eq(b"daemon-eye", b"daemon-eye"),
            "equal slices match"
        );
        assert!(
            !constant_time_eq(b"xismatch", b"oismatch"),
            "a difference at the first byte is caught"
        );
        assert!(
            !constant_time_eq(b"middle-diff", b"middlf-diff"),
            "a difference in the middle is caught"
        );
        assert!(
            !constant_time_eq(b"tail-end", b"tail-enx"),
            "a difference at the last byte is caught"
        );
        assert!(
            !constant_time_eq(b"short", b"shorter"),
            "a length difference is caught"
        );
    }

    // ── Auth and the body cap run before the body is read ──────────────────

    #[tokio::test]
    async fn webhook_refuses_before_reading_the_body() {
        let _guard = crate::test_home();
        let mut cfg = crate::config::Config::default();
        cfg.webhook.secret = "s3cret".into();
        let handle = std::sync::Arc::new(crate::config::ConfigHandle::new(cfg));
        let state = std::sync::Arc::new(test_state_from(handle));
        let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();

        let oversized: Vec<u8> = vec![b'x'; 3 * 1024 * 1024];

        // Wrong token, 3 MiB body: refused before the body is read.
        let resp = handle_webhook(
            State(std::sync::Arc::clone(&state)),
            headers_with_bearer("wrong"),
            ConnectInfo(addr),
            axum::body::Body::from(oversized.clone()),
        )
        .await;
        assert_eq!(
            resp,
            StatusCode::UNAUTHORIZED,
            "a wrong token is refused without the 3 MiB body being parsed"
        );

        // Right token, same 3 MiB body: over the 2 MiB cap → 413.
        let resp = handle_webhook(
            State(std::sync::Arc::clone(&state)),
            headers_with_bearer("s3cret"),
            ConnectInfo(addr),
            axum::body::Body::from(oversized),
        )
        .await;
        assert_eq!(
            resp,
            StatusCode::PAYLOAD_TOO_LARGE,
            "a 3 MiB body exceeds the 2 MiB cap"
        );

        // Right token, unparseable body → 400.
        let resp = handle_webhook(
            State(std::sync::Arc::clone(&state)),
            headers_with_bearer("s3cret"),
            ConnectInfo(addr),
            axum::body::Body::from("not json"),
        )
        .await;
        assert_eq!(
            resp,
            StatusCode::BAD_REQUEST,
            "a non-JSON body is refused as unparseable"
        );

        // Right token, valid JSON with no alerts → 400 (unchanged path).
        let resp = handle_webhook(
            State(std::sync::Arc::clone(&state)),
            headers_with_bearer("s3cret"),
            ConnectInfo(addr),
            axum::body::Body::from("{}"),
        )
        .await;
        assert_eq!(
            resp,
            StatusCode::BAD_REQUEST,
            "a JSON body with no parseable alerts is refused"
        );
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
    fn alerts_over_the_cap_are_dropped_and_counted() {
        let make = |i: usize| InternalAlert {
            alert_name: format!("alert-{i}"),
            status: crate::webhook::AlertStatus::Firing,
            severity: "warning".to_string(),
            summary: format!("alert {i}"),
            description: String::new(),
            labels: HashMap::from([("alertname".to_string(), format!("alert-{i}"))]),
            fingerprint: format!("fp-{i}"),
            source: "alertmanager",
        };
        let (kept, dropped) = cap_alerts((0..150).map(make).collect());
        assert_eq!(kept.len(), 100, "the first 100 alerts are kept");
        assert_eq!(dropped, 50, "the 50 alerts past the cap are dropped");
        assert_eq!(kept[0].alert_name, "alert-0", "the first alert survives");
        assert_eq!(
            kept[99].alert_name, "alert-99",
            "the alert at the cap boundary survives"
        );

        let (kept, dropped) = cap_alerts((0..3).map(make).collect());
        assert_eq!(kept.len(), 3, "a payload under the cap keeps every alert");
        assert_eq!(dropped, 0, "a payload under the cap drops nothing");
    }

    #[tokio::test]
    async fn webhook_post_over_the_cap_logs_one_discard() {
        let _guard = crate::test_home();
        let mut cfg = crate::config::Config::default();
        cfg.webhook.secret = "s3cret".into();
        let handle = std::sync::Arc::new(crate::config::ConfigHandle::new(cfg));
        let state = std::sync::Arc::new(test_state_from(handle));
        let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();

        let alerts: Vec<String> = (0..150)
            .map(|i| {
                serde_json::json!({
                    "status": "firing",
                    "labels": {
                        "alertname": format!("CapAlert{i}"),
                        "fingerprint": format!("fp-cap-{i}"),
                    },
                    "annotations": {"summary": format!("alert {i}")},
                })
                .to_string()
            })
            .collect();
        let body = format!(r#"{{"alerts": [{}]}}"#, alerts.join(","));
        assert!(!body.is_empty());

        let resp = handle_webhook(
            State(std::sync::Arc::clone(&state)),
            headers_with_bearer("s3cret"),
            ConnectInfo(addr),
            axum::body::Body::from(body),
        )
        .await;
        assert_eq!(
            resp,
            StatusCode::OK,
            "a POST over the cap still answers 200"
        );

        let discards = std::cell::RefCell::new(Vec::new());
        crate::daemon::utils::for_each_event_between(None, None, &mut |v| {
            if v.get("event").and_then(|e| e.as_str()) == Some("webhook_discarded")
                && v.get("reason").and_then(|r| r.as_str()) == Some("too_many_alerts")
            {
                discards.borrow_mut().push(v.clone());
            }
        });
        let discards = discards.into_inner();
        assert_eq!(
            discards.len(),
            1,
            "exactly one webhook_discarded event for the whole POST: {discards:?}"
        );
        assert_eq!(
            discards[0].get("dropped").and_then(|d| d.as_u64()),
            Some(50),
            "the event counts the 50 dropped alerts"
        );
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
            config: std::sync::Arc::new(crate::config::ConfigHandle::new(Config::default())),
            sessions: SessionStore::new(),
            cache: Arc::new(crate::tmux::cache::SessionCache::new("unused")), // unused in these tests
            schedule_store: Arc::new(crate::scheduler::ScheduleStore::new_empty()),
            bus: crate::bus::global().clone(),
            dedup: Mutex::new(HashMap::new()),
            rate_limit: Mutex::new(HashMap::new()),
            ip_limits: Mutex::new(HashMap::new()),
        }
    }

    /// A state on a caller-supplied handle, so a test can drive a reload
    /// through it. Same private-bus shape as [`test_state`].
    fn test_state_from(config: std::sync::Arc<crate::config::ConfigHandle>) -> WebhookState {
        WebhookState {
            config,
            sessions: SessionStore::new(),
            cache: Arc::new(crate::tmux::cache::SessionCache::new("unused")),
            schedule_store: Arc::new(crate::scheduler::ScheduleStore::new_empty()),
            bus: crate::bus::global().clone(),
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
        // Age the entry into a past window: the next admit drops it and
        // starts a fresh one.
        let window = now_secs() / IP_WINDOW_SECS - 1;
        state.ip_limits.lock().unwrap().insert(ip, (window, 0));
        assert!(admit_request(&state, ip));
    }

    /// P8: an entry the limiter moves into a past window is the only kind
    /// that accumulates, so admission is unchanged while the map stops
    /// holding one entry per source IP for ever.
    #[test]
    fn admit_request_drops_entries_from_past_windows() {
        let state = test_state();
        let ip: IpAddr = "10.0.0.3".parse().unwrap();
        for _ in 0..IP_WINDOW_MAX {
            assert!(admit_request(&state, ip), "the early window admits");
        }
        assert!(!admit_request(&state, ip), "the window is exhausted");

        let window = now_secs() / IP_WINDOW_SECS - 1;
        state.ip_limits.lock().unwrap().insert(ip, (window, 3));

        assert!(admit_request(&state, ip), "a new window admits again");
        assert_eq!(
            state.ip_limits.lock().unwrap().len(),
            1,
            "the stale-window entry was dropped, not kept beside the new one"
        );
        let fresh = state.ip_limits.lock().unwrap().get(&ip).copied().unwrap();
        assert_eq!(
            fresh,
            (now_secs() / IP_WINDOW_SECS, 1),
            "the kept entry is this window's, with its first bump"
        );
    }

    // ── Live config (M39 phase 13) ────────────────────────────────────────

    #[test]
    fn webhook_state_sees_a_reloaded_model() {
        let (_guard, _home) = crate::test_home();
        crate::config::Config::ensure_dirs().ok();
        let config_path = crate::config::etc_dir().join("config.toml");
        std::fs::write(
            &config_path,
            "[models.default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let handle = std::sync::Arc::new(crate::config::ConfigHandle::new(Config::load().unwrap()));
        let state = test_state_from(handle.clone());

        std::fs::write(
            &config_path,
            "[models.default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n\n[models.late]\nprovider = \"anthropic\"\nmodel = \"claude-opus-4-6\"\n",
        )
        .unwrap();
        handle.reload();

        assert!(
            state.config.current().models.contains_key("late"),
            "a reload must reach the webhook state's handle: {:?}",
            state.config.current().models.keys().collect::<Vec<_>>()
        );
    }
}
