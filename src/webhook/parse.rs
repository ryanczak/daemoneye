use std::collections::HashMap;

use serde_json::Value;

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
    /// Structured metadata from the upstream alert. Kept for future use (e.g.
    /// filtering, grouping, or alert dedup enhancements).
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
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AlertStatus::Firing => "firing",
            AlertStatus::Resolved => "resolved",
        }
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
        .map(|a| {
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

            InternalAlert {
                alert_name,
                status,
                severity,
                summary,
                description,
                labels,
                fingerprint,
                source: "alertmanager",
            }
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
        .unwrap_or({
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
    if body["state"].is_string()
        && let Some(a) = parse_grafana_legacy(body)
    {
        return vec![a];
    }

    // Generic fallback.
    parse_generic(body).map(|a| vec![a]).unwrap_or_default()
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
}
