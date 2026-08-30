use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::commands::{connect, liveness_line, recv, send_request};
use crate::cli::palette::{self, ColorDepth};
use crate::cli::render::terminal_width;
use crate::config::default_pid_path;
use crate::daemon::daemon_liveness;
use crate::daemon::instance::read_pid;
use crate::ipc::{Request, Response};

// ── Color palette ─────────────────────────────────────────────────────────────

fn depth() -> ColorDepth {
    static DEPTH: std::sync::OnceLock<ColorDepth> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(palette::detect_from_env)
}

fn c_accent(s: &str) -> String {
    format!(
        "\x1b[1m{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (100, 210, 255), 117, 96)
    )
}
fn c_key(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (140, 140, 165), 103, 90)
    )
}
fn c_val(s: &str) -> String {
    format!(
        "\x1b[1m{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (220, 220, 240), 254, 97)
    )
}
fn c_ok(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (80, 210, 130), 78, 92)
    )
}
fn c_err(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (230, 80, 80), 167, 91)
    )
}
fn c_warn(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (250, 190, 50), 214, 93)
    )
}
fn c_num(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (130, 195, 255), 111, 94)
    )
}
fn c_dim(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (80, 80, 105), 60, 90)
    )
}

fn dsep() -> String {
    c_dim("  ·  ")
}

// ── Section builder ───────────────────────────────────────────────────────────

const KEY_W: usize = 14;

struct Section {
    title: &'static str,
    rows: Vec<Row>,
}

enum Row {
    KV(String, String),
    Cont(String),
}

impl Section {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            rows: Vec::new(),
        }
    }

    fn kv(&mut self, key: &str, val: impl Into<String>) {
        self.rows.push(Row::KV(key.to_string(), val.into()));
    }

    fn cont(&mut self, text: impl Into<String>) {
        self.rows.push(Row::Cont(text.into()));
    }

    fn render(&self, term_width: usize) {
        let dash_count = term_width
            .saturating_sub(self.title.chars().count() + 4)
            .max(2);
        println!(
            "{} {} {}",
            c_dim("──"),
            c_accent(self.title),
            c_dim(&"─".repeat(dash_count))
        );
        for row in &self.rows {
            match row {
                Row::KV(k, v) => {
                    println!("  {}  {v}", c_key(&format!("{k:<KEY_W$}")));
                }
                Row::Cont(text) => {
                    println!("  {:<KEY_W$}  {text}", "");
                }
            }
        }
        println!();
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn fmt_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

fn fmt_unlimited_u32(v: u32) -> String {
    if v == 0 {
        c_dim("unlimited")
    } else {
        c_num(&v.to_string())
    }
}

fn fmt_unlimited_usize(v: usize) -> String {
    if v == 0 {
        c_dim("unlimited")
    } else {
        c_num(&v.to_string())
    }
}

fn fmt_ok_fail(ok: usize, fail: usize) -> String {
    let ok_s = if ok > 0 {
        c_ok(&format!("{ok} ok"))
    } else {
        c_dim("0 ok")
    };
    let fail_s = if fail > 0 {
        c_err(&format!("{fail} failed"))
    } else {
        c_dim("0 failed")
    };
    format!("{ok_s}{}{fail_s}", dsep())
}

fn fmt_appr_denied(a: usize, d: usize) -> String {
    let ap = if a > 0 {
        c_ok(&format!("{a} approved"))
    } else {
        c_dim("0 approved")
    };
    let dn = if d > 0 {
        c_err(&format!("{d} denied"))
    } else {
        c_dim("0 denied")
    };
    format!("{ap}  {dn}")
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run_status() -> Result<()> {
    match connect().await {
        Err(_) => {
            let liveness = daemon_liveness().await;
            let pid = read_pid(&default_pid_path());
            eprintln!("{}", c_err(&liveness_line(liveness, pid)));
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Status).await?;
            match recv(&mut rx).await {
                Ok(Response::DaemonStatus {
                    uptime_secs,
                    pid,
                    active_sessions,
                    total_turns,
                    provider,
                    model,
                    available_models: _,
                    socket_path,
                    schedule_count,
                    commands_fg_succeeded,
                    commands_fg_failed,
                    commands_fg_approved,
                    commands_fg_denied,
                    commands_bg_succeeded,
                    commands_bg_failed,
                    commands_bg_approved,
                    commands_bg_denied,
                    commands_sched_succeeded,
                    commands_sched_failed,
                    ghosts_launched,
                    ghosts_active,
                    ghosts_completed,
                    ghosts_failed,
                    webhooks_received,
                    webhooks_rejected,
                    webhook_url,
                    runbook_count,
                    runbooks_created,
                    runbooks_executed,
                    runbooks_deleted,
                    script_count,
                    scripts_created,
                    scripts_executed,
                    scripts_deleted,
                    memories_created,
                    memories_recalled,
                    memories_deleted,
                    schedules_created,
                    schedules_executed,
                    schedules_deleted,
                    active_prompt_tokens,
                    context_window_tokens,
                    recent_commands: _,
                    memory_breakdown,
                    redaction_counts,
                    compactions,
                    compaction_ratio,
                    scripts_approved,
                    scripts_denied,
                    runbooks_approved,
                    runbooks_denied,
                    file_edits_approved,
                    file_edits_denied,
                    limits,
                    active_agents,
                    daemon_session_costs: _,
                    daemon_total_cost_today_usd,
                    daemon_cost_by_provider,
                    daemon_cost_by_agent,
                }) => {
                    let tw = terminal_width();

                    // ── Header ────────────────────────────────────────────────
                    let home = std::env::var("HOME").unwrap_or_default();
                    let socket_disp = if !home.is_empty() && socket_path.starts_with(&home) {
                        socket_path.replacen(&home, "~", 1)
                    } else {
                        socket_path.clone()
                    };
                    println!();
                    println!(
                        "  {}  {}{}{}  {}{}{}  {}",
                        c_accent("DAEMONEYE"),
                        c_key("pid "),
                        c_val(&pid.to_string()),
                        dsep(),
                        c_key("uptime "),
                        c_val(&fmt_uptime(uptime_secs)),
                        dsep(),
                        c_dim(&socket_disp),
                    );
                    println!();

                    // ── RUNTIME ───────────────────────────────────────────────
                    let mut runtime = Section::new("RUNTIME");
                    runtime.kv(
                        "model",
                        format!("{}{}{}", c_dim(&provider), c_dim(" / "), c_val(&model)),
                    );
                    runtime.render(tw);

                    // ── SESSION ───────────────────────────────────────────────
                    let mut session_sec = Section::new("SESSION");
                    session_sec.kv("active", c_num(&active_sessions.to_string()));
                    let cmp_str = if compactions == 0 {
                        c_dim("none")
                    } else {
                        format!(
                            "{}{}{}",
                            c_num(&compactions.to_string()),
                            c_dim(" compactions  "),
                            c_dim(&format!("({:.1}:1 ratio)", compaction_ratio))
                        )
                    };
                    session_sec.kv(
                        "turns",
                        format!("{}{}{}", c_num(&total_turns.to_string()), dsep(), cmp_str),
                    );
                    let tokens_pct = if context_window_tokens > 0 {
                        (active_prompt_tokens as f64 / context_window_tokens as f64 * 100.0) as u32
                    } else {
                        0
                    };
                    let pct_s = if tokens_pct > 80 {
                        c_err(&format!("{}%", tokens_pct))
                    } else if tokens_pct > 60 {
                        c_warn(&format!("{}%", tokens_pct))
                    } else {
                        c_ok(&format!("{}%", tokens_pct))
                    };
                    session_sec.kv(
                        "context",
                        format!(
                            "{} {} {} {}  {}",
                            c_num(&active_prompt_tokens.to_string()),
                            c_dim("/"),
                            c_num(&context_window_tokens.to_string()),
                            c_dim("tokens"),
                            pct_s,
                        ),
                    );
                    session_sec.render(tw);

                    // ── EXECUTION ─────────────────────────────────────────────
                    let mut exec = Section::new("EXECUTION");
                    exec.kv(
                        "foreground",
                        format!(
                            "{}{}{}",
                            fmt_appr_denied(commands_fg_approved, commands_fg_denied),
                            dsep(),
                            fmt_ok_fail(commands_fg_succeeded, commands_fg_failed),
                        ),
                    );
                    exec.kv(
                        "background",
                        format!(
                            "{}{}{}",
                            fmt_appr_denied(commands_bg_approved, commands_bg_denied),
                            dsep(),
                            fmt_ok_fail(commands_bg_succeeded, commands_bg_failed),
                        ),
                    );
                    exec.kv(
                        "scheduled",
                        fmt_ok_fail(commands_sched_succeeded, commands_sched_failed),
                    );
                    exec.render(tw);

                    // ── KNOWLEDGE ─────────────────────────────────────────────
                    let mut knowledge = Section::new("KNOWLEDGE");
                    const KNOWN_CATS: &[&str] = &["knowledge", "session", "incident"];
                    let mut cat_parts: Vec<String> = KNOWN_CATS
                        .iter()
                        .map(|cat| {
                            let n = memory_breakdown.get(*cat).copied().unwrap_or(0);
                            format!("{} {}", c_key(cat), c_num(&n.to_string()))
                        })
                        .collect();
                    let mut extra: Vec<_> = memory_breakdown
                        .iter()
                        .filter(|(k, _)| !KNOWN_CATS.contains(&k.as_str()))
                        .collect();
                    extra.sort_by_key(|(k, _)| k.as_str());
                    for (cat, n) in extra {
                        cat_parts.push(format!("{} {}", c_key(cat), c_num(&n.to_string())));
                    }
                    knowledge.kv("namespaces", cat_parts.join(&dsep()));
                    knowledge.kv(
                        "ops",
                        format!(
                            "{}{}{}{}{}",
                            c_num(&memories_created.to_string()),
                            c_dim(" created"),
                            dsep(),
                            c_num(&memories_recalled.to_string()),
                            c_dim(" recalled"),
                        ),
                    );
                    if memories_deleted > 0 {
                        knowledge.cont(format!(
                            "{}{}",
                            c_num(&memories_deleted.to_string()),
                            c_dim(" deleted"),
                        ));
                    }
                    knowledge.render(tw);

                    // ── GHOST SHELLS ──────────────────────────────────────────
                    let mut ghosts = Section::new("GHOST SHELLS");
                    let active_s = if ghosts_active > 0 {
                        c_warn(&format!("{} active", ghosts_active))
                    } else {
                        c_dim("0 active")
                    };
                    let failed_s = if ghosts_failed > 0 {
                        c_err(&format!("{} failed", ghosts_failed))
                    } else {
                        c_dim("0 failed")
                    };
                    ghosts.kv(
                        "activity",
                        format!(
                            "{}{}{}{}{}{}{}",
                            active_s,
                            dsep(),
                            c_num(&ghosts_launched.to_string()),
                            c_dim(" launched"),
                            dsep(),
                            c_ok(&format!("{} completed", ghosts_completed)),
                            if ghosts_failed > 0 {
                                format!("{}{}", dsep(), failed_s)
                            } else {
                                String::new()
                            },
                        ),
                    );
                    ghosts.render(tw);

                    // ── AGENTS (only if any defined or active) ────────────────
                    let agents_defined = crate::agents::list_agents().unwrap_or_default();
                    if !agents_defined.is_empty() || !active_agents.is_empty() {
                        let mut agents = Section::new("AGENTS");
                        let active_set: std::collections::HashSet<&str> =
                            active_agents.iter().map(|(n, _)| n.as_str()).collect();
                        for info in &agents_defined {
                            let status = if active_set.contains(info.name.as_str()) {
                                let job = active_agents
                                    .iter()
                                    .find(|(n, _)| n == &info.name)
                                    .map(|(_, j)| j.as_str())
                                    .unwrap_or("running");
                                format!("{}{}{}", c_warn("active"), c_dim("  "), c_dim(job))
                            } else {
                                c_dim("idle")
                            };
                            agents.kv(&info.name, status);
                        }
                        agents.render(tw);
                    }

                    // ── TOOLING ───────────────────────────────────────────────
                    let mut tooling = Section::new("TOOLING");
                    tooling.kv(
                        "runbooks",
                        format!(
                            "{}{}{}{}{}{}{}{}{}",
                            c_num(&runbook_count.to_string()),
                            c_dim(" stored"),
                            dsep(),
                            c_num(&runbooks_created.to_string()),
                            c_dim(" created  "),
                            c_num(&runbooks_executed.to_string()),
                            c_dim(" executed  "),
                            c_num(&runbooks_deleted.to_string()),
                            c_dim(" deleted"),
                        ),
                    );
                    tooling.cont(fmt_appr_denied(runbooks_approved, runbooks_denied));
                    tooling.kv(
                        "scripts",
                        format!(
                            "{}{}{}{}{}{}{}{}{}",
                            c_num(&script_count.to_string()),
                            c_dim(" stored"),
                            dsep(),
                            c_num(&scripts_created.to_string()),
                            c_dim(" created  "),
                            c_num(&scripts_executed.to_string()),
                            c_dim(" executed  "),
                            c_num(&scripts_deleted.to_string()),
                            c_dim(" deleted"),
                        ),
                    );
                    tooling.cont(fmt_appr_denied(scripts_approved, scripts_denied));
                    tooling.kv(
                        "schedules",
                        format!(
                            "{}{}{}{}{}{}{}{}{}",
                            c_num(&schedule_count.to_string()),
                            c_dim(" active"),
                            dsep(),
                            c_num(&schedules_created.to_string()),
                            c_dim(" created  "),
                            c_num(&schedules_executed.to_string()),
                            c_dim(" executed  "),
                            c_num(&schedules_deleted.to_string()),
                            c_dim(" deleted"),
                        ),
                    );
                    tooling.kv(
                        "file edits",
                        fmt_appr_denied(file_edits_approved, file_edits_denied),
                    );
                    tooling.render(tw);

                    // ── WEBHOOKS ──────────────────────────────────────────────
                    let mut webhooks = Section::new("WEBHOOKS");
                    webhooks.kv("endpoint", c_dim(&webhook_url));
                    let rejected_s = if webhooks_rejected > 0 {
                        c_err(&format!("{} rejected", webhooks_rejected))
                    } else {
                        c_dim("0 rejected")
                    };
                    webhooks.kv(
                        "traffic",
                        format!(
                            "{}{}{}",
                            c_num(&webhooks_received.to_string()),
                            c_dim(" received"),
                            if webhooks_rejected > 0 {
                                format!("{}{}", dsep(), rejected_s)
                            } else {
                                String::new()
                            },
                        ),
                    );
                    webhooks.render(tw);

                    // ── REDACTIONS ────────────────────────────────────────────
                    let mut redact_sorted: Vec<_> = redaction_counts.into_iter().collect();
                    redact_sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    let nonzero: Vec<_> = redact_sorted.iter().filter(|(_, n)| *n > 0).collect();
                    if !nonzero.is_empty() {
                        let mut redactions = Section::new("REDACTIONS");
                        let parts: Vec<String> = nonzero
                            .iter()
                            .map(|(t, n)| format!("{} {}", c_key(t), c_num(&n.to_string())))
                            .collect();
                        redactions.kv("counts", parts.join(&dsep()));
                        redactions.render(tw);
                    }

                    // ── LIMITS ────────────────────────────────────────────────
                    let mut lims = Section::new("LIMITS");
                    lims.kv(
                        "batch / turn",
                        format!(
                            "{} {} {}",
                            fmt_unlimited_u32(limits.per_tool_batch),
                            c_dim("/"),
                            fmt_unlimited_u32(limits.total_tool_calls_per_turn),
                        ),
                    );
                    lims.kv(
                        "result chars",
                        fmt_unlimited_usize(limits.tool_result_chars),
                    );
                    lims.kv("max turns", fmt_unlimited_usize(limits.max_turns));
                    lims.kv(
                        "session tools",
                        fmt_unlimited_usize(limits.max_tool_calls_per_session),
                    );
                    if !limits.per_tool_overrides.is_empty() {
                        let ovr: Vec<String> = limits
                            .per_tool_overrides
                            .iter()
                            .map(|(t, c)| format!("{} {}", c_key(t), fmt_unlimited_u32(*c)))
                            .collect();
                        lims.kv("overrides", ovr.join(&dsep()));
                    }
                    lims.render(tw);

                    // ── COST TODAY (only if non-zero) ─────────────────────────
                    if daemon_total_cost_today_usd > 0.0 {
                        let mut cost = Section::new("COST TODAY");
                        cost.kv(
                            "total",
                            c_warn(&format!("${:.4}", daemon_total_cost_today_usd)),
                        );
                        if !daemon_cost_by_provider.is_empty() {
                            let parts: Vec<String> = daemon_cost_by_provider
                                .iter()
                                .map(|(p, c)| {
                                    format!("{} {}", c_key(p), c_warn(&format!("${:.4}", c)))
                                })
                                .collect();
                            cost.kv("by provider", parts.join(&dsep()));
                        }
                        if !daemon_cost_by_agent.is_empty() {
                            let parts: Vec<String> = daemon_cost_by_agent
                                .iter()
                                .map(|(a, c)| {
                                    format!("{} {}", c_key(a), c_warn(&format!("${:.4}", c)))
                                })
                                .collect();
                            cost.kv("by agent", parts.join(&dsep()));
                        }
                        cost.render(tw);
                    }

                    // ── SANDBOX ───────────────────────────────────────────────
                    send_request(&mut tx, Request::ContainerStatus).await?;
                    if let Ok(Response::ContainerStatus { report }) = recv(&mut rx).await {
                        let mut sbx = Section::new("SANDBOX");
                        sbx.kv(
                            "enabled",
                            if report.enabled {
                                c_ok("yes")
                            } else {
                                c_dim("no")
                            },
                        );
                        sbx.kv(
                            "runtime",
                            if report.runtime_ok {
                                c_ok(&report.runtime_detail)
                            } else {
                                c_err(&report.runtime_detail)
                            },
                        );
                        sbx.kv("image", c_val(&report.image_detail));
                        sbx.kv("containers", c_num(&report.containers.len().to_string()));
                        for ci in &report.containers {
                            sbx.cont(format!(
                                "{} {} {} {}",
                                c_key(&ci.id),
                                c_val(&ci.state),
                                if ci.is_ghost {
                                    c_warn("ghost")
                                } else {
                                    c_dim("session")
                                },
                                c_dim(ci.session.as_deref().unwrap_or("-")),
                            ));
                        }
                        sbx.render(tw);
                    }
                }
                _ => {
                    eprintln!("{}", c_err("Daemon did not return status."));
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
