---
tags: [daemoneye, config, configuration, config.toml, keys, defaults, reload, restart]
summary: Every ~/.daemoneye/etc/config.toml key — type, range, default, and whether a change lands on reload or waits for a restart
relates_to: [agent-runtime-layout, webhook-setup, scripts-and-sudoers]
---

# Config Reference

Every key `~/.daemoneye/etc/config.toml` can hold, with its type, range,
default, and when a change takes effect. The seeded template
(`assets/etc/config.toml`) shows the same keys as commented examples; where a
commented example and the default differ, the default here wins.

**How a change lands.** Asked to edit the config, the model uses `edit_file`
with `operation="edit"` on `~/.daemoneye/etc/config.toml`: the replacement is
parsed as the daemon's `Config` before anything is prompted — a result that
does not parse is refused and the file is unchanged — the prompt is always
asked (never session-approved) with secrets masked, and after the approval the
daemon reloads the config, reporting the keys that changed and the keys that
wait for a restart. A hand edit of the file lands at `daemoneye reload` (or a
restart). Secrets — `models.<name>.api_key`, `webhook.secret` — show masked in
every read; they are never echoed back.

A key under a restart-only section (`daemon`, `webhook`, `sandbox`,
`execution`) keeps its running value across a reload and needs a daemon
restart to land. Everything else swaps into the running config on reload.

## `[ai]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `ai.prompt` | string | any | `sre` | reload |
| `ai.first_token_timeout_secs` | integer | ≥ 0 | 600 | reload |
| `ai.stream_idle_timeout_secs` | integer | ≥ 0 | 240 | reload |
| `ai.connect_timeout_secs` | integer | ≥ 0 | 30 | reload |

## `[models.<name>]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `models.<name>.provider` | string | `anthropic` / `openai` / `gemini` / `ollama` / `lmstudio` | `anthropic` | reload |
| `models.<name>.api_key` | string | any (masked) | none | reload |
| `models.<name>.model` | string | any | `claude-sonnet-4-6` | reload |
| `models.<name>.base_url` | string | URL | none | reload |
| `models.<name>.context_window_tokens` | integer | ≥ 0 | none (auto-lookup) | reload |
| `models.<name>.max_tokens` | integer | ≥ 0 | none (8192 in effect) | reload |
| `models.<name>.input_cost_per_mtok` | number | ≥ 0 | none (warns) | reload |
| `models.<name>.output_cost_per_mtok` | number | ≥ 0 | none (warns) | reload |
| `models.<name>.cache_read_cost_per_mtok` | number | ≥ 0 | none | reload |
| `models.<name>.cache_write_cost_per_mtok` | number | ≥ 0 | none | reload |

## `[hosts.<name>]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `hosts.<name>.ssh` | string | any `ssh(1)` destination | required — empty is a config error | reload |
| `hosts.<name>.tags` | array of strings | any | `[]` | reload |
| `hosts.<name>.sudo_scripts` | array of strings | any | `[]` (restricts nothing) | reload |
| `hosts.<name>.sandbox_profile` | string | any (`"none"` or a profile name) | none | reload |

## `[masking]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `masking.extra_patterns` | array of strings (regex) | any | `[]` | reload |

## `[context]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `context.environment` | string | `personal` / `development` / `staging` / `production` | `personal` | reload |

## `[notifications]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `notifications.on_alert` | string (shell command) | any | none | reload |

## `[webhook]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `webhook.enabled` | boolean | true/false | false | restart |
| `webhook.port` | integer | 0–65535 | 9393 | restart |
| `webhook.secret` | string | any (masked) | none | restart |
| `webhook.auto_analyze` | boolean | true/false | true | restart |
| `webhook.severity_threshold` | string | `info` / `warning` / `critical` | `warning` | restart |
| `webhook.dedup_window_secs` | integer | ≥ 0 | 300 | restart |
| `webhook.bind_addr` | string (IP) | any | `127.0.0.1` | restart |

## `[ghost]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `ghost.max_ghost_turns` | integer | ≥ 0 | 20 | reload |
| `ghost.max_concurrent_ghosts` | integer | ≥ 0 (0 = unlimited) | 3 | reload |

## `[daemon]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `daemon.tmux_session` | string | any | `daemoneye` | restart |

## `[digest]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `digest.narrative_enabled` | boolean | true/false | true | reload |

## `[approvals]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `approvals.commands` | boolean | true/false | true | reload |
| `approvals.sudo` | boolean | true/false | false | reload |
| `approvals.scripts` | boolean | true/false | false | reload |
| `approvals.runbooks` | boolean | true/false | false | reload |
| `approvals.file_edits` | boolean | true/false | false | reload |
| `approvals.ghost_commands` | boolean | true/false | false | reload |

## `[limits]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `limits.per_tool_batch` | integer | ≥ 0 (0 = unlimited) | 100 | reload |
| `limits.total_tool_calls_per_turn` | integer | ≥ 0 (0 = unlimited) | 0 | reload |
| `limits.tool_result_chars` | integer | ≥ 0 (0 = unlimited) | 16000 | reload |
| `limits.max_turns` | integer | ≥ 0 (0 = unlimited) | 0 | reload |
| `limits.max_tool_calls_per_session` | integer | ≥ 0 (0 = unlimited) | 0 | reload |
| `limits.per_tool.<tool>` | integer | ≥ 0 (0 = unlimited) | none (per_tool_batch applies) | reload |
| `limits.turn_timeout_secs` | integer | ≥ 0 (0 = no deadline) | 0 | reload |

## `[compaction]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `compaction.elide_at_pct` | integer (% of context window) | ≥ 0 | 50 | reload |
| `compaction.compact_at_pct` | integer | ≥ 0 | 60 | reload |
| `compaction.target_pct` | integer | ≥ 0 | 40 | reload |
| `compaction.emergency_pct` | integer | ≥ 0 | 85 | reload |
| `compaction.rollup_after` | integer | ≥ 0 | 10 | reload |
| `compaction.extract_memories` | boolean | true/false | false | reload |

## `[sessions]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `sessions.auto_name_turn_threshold` | integer | ≥ 0 (0 = never) | 10 | reload |
| `sessions.auto_name_enabled` | boolean | true/false | true | reload |
| `sessions.load_recent_turns` | integer | ≥ 0 (0 = all) | 10 | reload |
| `sessions.archive_retention_days` | integer | ≥ 0 (0 = keep forever) | 0 | reload |
| `sessions.log_retention_days` | integer | ≥ 0 (0 = keep forever) | 0 | reload |

## `[events]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `events.retention_days` | integer | ≥ 0 (0 = keep forever) | 90 | reload |

## `[logging]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `logging.log_max_bytes` | integer (bytes) | ≥ 0 | 5242880 | reload |
| `logging.log_keep_count` | integer | ≥ 0 | 5 | reload |

## `[retention]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `retention.pane_log_retention_days` | integer | ≥ 0 (0 = keep forever) | 7 | reload |
| `retention.mailbox_retention_days` | integer | ≥ 0 (0 = keep forever) | 7 | reload |

## `[sandbox]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `sandbox.enabled` | boolean | true/false | false | restart |
| `sandbox.runtime` | string | `sbx` (only) | `sbx` | restart |
| `sandbox.image` | string | any | `daemoneye-agent-base` | restart |
| `sandbox.proxy_image` | string | any | `daemoneye-egress-proxy` | restart |
| `sandbox.workdir` | string (path) | any | `/de/work` | restart |
| `sandbox.run_as` | string (`uid:gid`) | any | `1000:1000` | restart |
| `sandbox.docker_host` | string (endpoint) | any | `unix:///run/user/1000/docker.sock` | restart |
| `sandbox.limits.memory` | string (Docker `--memory`) | any | `1g` | restart |
| `sandbox.limits.pids` | integer | ≥ 0 | 256 | restart |
| `sandbox.limits.cpus` | number | ≥ 0 | 2.0 | restart |
| `sandbox.limits.scratch` | string (size) | any | `2g` | restart |
| `sandbox.profile.<name>.network` | string | `none` / `proxy` | `none` | restart |
| `sandbox.profile.<name>.proxy_allow` | array of strings | any | `[]` | restart |
| `sandbox.profile.<name>.proxy_deny` | array of strings | any | `[]` | restart |
| `sandbox.ghost_defaults.destroy_on_exit` | boolean | true/false | true | restart |
| `sandbox.ghost_defaults.mount_scripts` | string | `ro` (docker-only: no effect) | `ro` | restart |

## `[execution]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `execution.backend` | string | `pty` (only) | `pty` | restart |

## `[shells]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `shells.max_per_owner` | integer | ≥ 0 (0 = unlimited) | 5 | reload |
| `shells.exited_retention_secs` | integer | ≥ 0 | 300 | reload |
| `shells.detach_close_secs` | integer | ≥ 0 (0 = never close) | 300 | reload |
| `shells.idle_reap_secs` | integer | ≥ 0 (0 = never reap) | 3600 | reload |
| `shells.log_retention_days` | integer | ≥ 0 (0 = keep forever) | 7 | reload |
| `shells.scrollback_lines` | integer | ≥ 0 | 5000 | reload |
| `shells.rows` | integer | 10–200 (clamped) | 40 | reload |
| `shells.cols` | integer | 40–500 (clamped) | 120 | reload |

## `[budget]`

| Key | Type | Range | Default | Takes effect |
|---|---|---|---|---|
| `budget.max_cost_per_day` | number (USD) | ≥ 0 (0 = no cap; negative treated as 0) | 0.0 | reload |
