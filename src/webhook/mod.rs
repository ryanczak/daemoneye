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

mod active;
mod parse;
mod process;
mod server;

pub use parse::*;
pub use process::*;
pub use server::*;

pub use active::ACTIVE_ALERT_TTL_SECS;
pub use active::active_severity_rank;
pub use active::{active_alerts_firing, active_alerts_resolved, active_alerts_snapshot};
