# Webhook Setup

## Endpoint

- Default port: 9393 (configure via `webhook.port` in config.toml)
- Endpoint: `http://<daemon-host>:9393/webhook`
- Health check: `http://<daemon-host>:9393/health`
- Authentication: set `webhook.secret` in config.toml; send as `Authorization: Bearer <secret>`

## Enabling

Add to `~/.daemoneye/config.toml` and restart the daemon:

```toml
[webhook]
enabled = true
port = 9393
secret = ""
```

Check current state:

```
grep -A5 '\[webhook\]' ~/.daemoneye/config.toml || echo 'not configured'
```

## Prometheus Alert Rule

Create `/etc/prometheus/rules/<topic>.yml` on the Prometheus host. Alert name must be CamelCase:

```yaml
groups:
  - name: daemoneye
    rules:
      - alert: HighDiskUsage
        expr: >
          (node_filesystem_size_bytes{fstype!~"tmpfs|overlay"}
           - node_filesystem_free_bytes{fstype!~"tmpfs|overlay"})
          / node_filesystem_size_bytes{fstype!~"tmpfs|overlay"} > 0.90
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Disk usage above 90% on {{ $labels.instance }}"
          description: "{{ $labels.mountpoint }} is at {{ $value | humanizePercentage }}"
```

Reload: `curl -X POST http://localhost:9090/-/reload`

## Alertmanager Receiver

Add to `/etc/prometheus/alertmanager.yml` on the Alertmanager host:

```yaml
receivers:
  - name: daemoneye
    webhook_configs:
      - url: 'http://<daemon-host>:9393/webhook'
        send_resolved: true
        # http_config:
        #   authorization:
        #     credentials: '<secret>'

route:
  receiver: daemoneye
  routes:
    - matchers:
        - severity =~ "warning|critical"
      receiver: daemoneye
```

Reload: `curl -X POST http://localhost:9093/-/reload`
Verify: `amtool check-config /etc/alertmanager/alertmanager.yml`

## Grafana Unified Alerting (Grafana 9+)

In Alerting → Contact points, create a Webhook contact point:

- URL: `http://<daemon-host>:9393/webhook`
- Method: POST
- If secret set: add `Authorization: Bearer <secret>` custom header

Via API on the Grafana host:

```bash
curl -X POST http://localhost:3000/api/v1/provisioning/contact-points \
  -H 'Content-Type: application/json' -u admin:admin \
  -d '{"name":"daemoneye","type":"webhook","settings":{"url":"http://<daemon-host>:9393/webhook","httpMethod":"POST"}}'
```

## Grafana Legacy Alerting (< 9)

Alert Rules → Notifications → add Webhook channel with URL `http://<daemon-host>:9393/webhook`.
Legacy payloads (top-level `"state"` field) are detected and parsed automatically.

## Test the Pipeline

```bash
curl -s -X POST http://<daemon-host>:9393/webhook \
  -H 'Content-Type: application/json' \
  -d '{"alerts":[{"status":"firing","labels":{"alertname":"TestAlert","severity":"warning"},"annotations":{"summary":"Integration test","description":"Verify DaemonEye webhook is working"},"fingerprint":"test-001"}]}'
```

Expected: `200` response, tmux overlay in chat pane, `webhook_alert` in `~/.daemoneye/events.jsonl`.

Check recent events: `search_repository("webhook_alert", kind:"events")`

## Self-Setup Workflow

1. `write_runbook("kebab-alertname", content)` — create before the alert rule
2. Create Prometheus alert rule (CamelCase alertname)
3. Configure Alertmanager or Grafana to route to DaemonEye webhook
4. Optionally add `schedule_command` watchdog as fallback
5. Send test alert to confirm end-to-end delivery
