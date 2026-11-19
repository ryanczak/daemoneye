//! Section builders and renderers for `daemoneye status`, extracted from
//! `src/cli/status.rs` to keep `run_status` and every section helper concise.

use super::*;

#[allow(clippy::too_many_arguments)]
fn render_header_and_core_sections(
    tw: usize,
    pid: u32,
    uptime_secs: u64,
    socket_path: &str,
    provider: &str,
    model: &str,
    active_sessions: usize,
    total_turns: usize,
    compactions: usize,
    compaction_ratio: f64,
    active_prompt_tokens: u32,
    context_window_tokens: u32,
    commands_fg_approved: usize,
    commands_fg_denied: usize,
    commands_fg_succeeded: usize,
    commands_fg_failed: usize,
    commands_bg_approved: usize,
    commands_bg_denied: usize,
    commands_bg_succeeded: usize,
    commands_bg_failed: usize,
    commands_sched_succeeded: usize,
    commands_sched_failed: usize,
) {
    let home = std::env::var("HOME").unwrap_or_default();
    let socket_disp = if !home.is_empty() && socket_path.starts_with(&home) {
        socket_path.replacen(&home, "~", 1)
    } else {
        socket_path.to_string()
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

    let mut runtime = Section::new("RUNTIME");
    runtime.kv(
        "model",
        format!("{}{}{}", c_dim(provider), c_dim(" / "), c_val(model)),
    );
    runtime.render(tw);

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
}

#[allow(clippy::too_many_arguments)]
fn render_knowledge_and_ghosts_sections(
    tw: usize,
    memory_breakdown: &std::collections::HashMap<String, usize>,
    memories_created: usize,
    memories_recalled: usize,
    memories_deleted: usize,
    ghosts_active: usize,
    ghosts_launched: usize,
    ghosts_completed: usize,
    ghosts_failed: usize,
) {
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
}

#[allow(clippy::too_many_arguments)]
fn render_agents_and_tooling_sections(
    tw: usize,
    active_agents: &[(String, String)],
    runbook_count: usize,
    runbooks_created: usize,
    runbooks_executed: usize,
    runbooks_deleted: usize,
    runbooks_approved: usize,
    runbooks_denied: usize,
    script_count: usize,
    scripts_created: usize,
    scripts_executed: usize,
    scripts_deleted: usize,
    scripts_approved: usize,
    scripts_denied: usize,
    schedule_count: usize,
    schedules_created: usize,
    schedules_executed: usize,
    schedules_deleted: usize,
    file_edits_approved: usize,
    file_edits_denied: usize,
    webhook_url: &str,
    webhooks_received: usize,
    webhooks_rejected: usize,
    redaction_counts: std::collections::HashMap<String, usize>,
) {
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

    let mut webhooks = Section::new("WEBHOOKS");
    webhooks.kv("endpoint", c_dim(webhook_url));
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
}

fn render_limits_and_cost_sections(
    tw: usize,
    limits: &crate::ipc::LimitsSummary,
    daemon_total_cost_today_usd: f64,
    daemon_cost_cap_usd: f64,
    daemon_spend_today_usd: f64,
    daemon_cost_by_provider: &[(String, f64)],
    daemon_cost_by_agent: &[(String, f64)],
) {
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

    if daemon_total_cost_today_usd > 0.0 || daemon_cost_cap_usd > 0.0 {
        let mut cost = Section::new("COST TODAY");
        cost.kv(
            "total",
            c_warn(&format!("${:.4}", daemon_total_cost_today_usd)),
        );
        if daemon_cost_cap_usd > 0.0 {
            cost.kv("cap", cap_line(daemon_spend_today_usd, daemon_cost_cap_usd));
        }
        if !daemon_cost_by_provider.is_empty() {
            let parts: Vec<String> = daemon_cost_by_provider
                .iter()
                .map(|(p, c)| format!("{} {}", c_key(p), c_warn(&format!("${:.4}", c))))
                .collect();
            cost.kv("by provider", parts.join(&dsep()));
        }
        if !daemon_cost_by_agent.is_empty() {
            let parts: Vec<String> = daemon_cost_by_agent
                .iter()
                .map(|(a, c)| format!("{} {}", c_key(a), c_warn(&format!("${:.4}", c))))
                .collect();
            cost.kv("by agent", parts.join(&dsep()));
        }
        cost.render(tw);
    }
}

pub(super) async fn query_and_render_sandbox_section(tw: usize) {
    let sbx_conn = connect().await.ok();
    if let Some(stream) = sbx_conn {
        let (srx, mut stx) = stream.into_split();
        let mut srx = BufReader::new(srx);
        if let Ok(Response::ContainerStatus { report }) =
            match send_request(&mut stx, Request::ContainerStatus).await {
                Ok(()) => recv(&mut srx).await,
                Err(e) => Err(e),
            }
        {
            sandbox_section(&report).render(tw);
        }
    }
}

fn render_daemon_status_core(resp: &Response, tw: usize) -> bool {
    let Response::DaemonStatus {
        uptime_secs,
        pid,
        active_sessions,
        total_turns,
        provider,
        model,
        socket_path,
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
        memories_created,
        memories_recalled,
        memories_deleted,
        active_prompt_tokens,
        context_window_tokens,
        memory_breakdown,
        compactions,
        compaction_ratio,
        ..
    } = resp
    else {
        return false;
    };

    render_header_and_core_sections(
        tw,
        *pid,
        *uptime_secs,
        socket_path,
        provider,
        model,
        *active_sessions,
        *total_turns,
        *compactions,
        *compaction_ratio,
        *active_prompt_tokens,
        *context_window_tokens,
        *commands_fg_approved,
        *commands_fg_denied,
        *commands_fg_succeeded,
        *commands_fg_failed,
        *commands_bg_approved,
        *commands_bg_denied,
        *commands_bg_succeeded,
        *commands_bg_failed,
        *commands_sched_succeeded,
        *commands_sched_failed,
    );
    render_knowledge_and_ghosts_sections(
        tw,
        memory_breakdown,
        *memories_created,
        *memories_recalled,
        *memories_deleted,
        *ghosts_active,
        *ghosts_launched,
        *ghosts_completed,
        *ghosts_failed,
    );
    true
}

fn render_daemon_status_tail(resp: Response, tw: usize) {
    let Response::DaemonStatus {
        schedule_count,
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
        schedules_created,
        schedules_executed,
        schedules_deleted,
        redaction_counts,
        scripts_approved,
        scripts_denied,
        runbooks_approved,
        runbooks_denied,
        file_edits_approved,
        file_edits_denied,
        limits,
        active_agents,
        daemon_total_cost_today_usd,
        daemon_cost_by_provider,
        daemon_cost_by_agent,
        daemon_spend_today_usd,
        daemon_cost_cap_usd,
        ..
    } = resp
    else {
        return;
    };

    render_agents_and_tooling_sections(
        tw,
        &active_agents,
        runbook_count,
        runbooks_created,
        runbooks_executed,
        runbooks_deleted,
        runbooks_approved,
        runbooks_denied,
        script_count,
        scripts_created,
        scripts_executed,
        scripts_deleted,
        scripts_approved,
        scripts_denied,
        schedule_count,
        schedules_created,
        schedules_executed,
        schedules_deleted,
        file_edits_approved,
        file_edits_denied,
        &webhook_url,
        webhooks_received,
        webhooks_rejected,
        redaction_counts,
    );
    render_limits_and_cost_sections(
        tw,
        &limits,
        daemon_total_cost_today_usd,
        daemon_cost_cap_usd,
        daemon_spend_today_usd,
        &daemon_cost_by_provider,
        &daemon_cost_by_agent,
    );
}

pub(super) async fn render_daemon_status(resp: Response) -> bool {
    let tw = terminal_width();
    if !render_daemon_status_core(&resp, tw) {
        return false;
    }
    render_daemon_status_tail(resp, tw);
    query_and_render_sandbox_section(tw).await;
    true
}
