use crate::tmux::cache::SessionCache;
use crate::util::UnpoisonExt;

/// Context passed to prompt builder functions.
/// Follows the `SessionCtx<'a>` pattern — borrow-only, no clones.
pub struct PromptCtx<'a> {
    pub client_pane: Option<&'a str>,
    pub chat_pane: Option<&'a str>,
    pub default_target_pane: Option<&'a str>,
    pub cache: &'a SessionCache,
    pub config: &'a crate::config::Config,
    pub chat_width: Option<usize>,
    pub safe_query: &'a str,
    /// For subsequent turns:
    pub last_prompt_tokens: u32,
    pub history_count: usize,
    pub this_turn_count: usize,
    pub ghost_turn_limit: Option<usize>,
    pub inject_snapshot: bool,
    /// Memory namespaces for this session (e.g. `["agent-name", "global"]`).
    pub memory_namespaces: &'a [&'a str],
    /// Agent-level tool policy, if this session has one.
    pub tool_policy: Option<&'a crate::agents::ToolPolicy>,
}

/// Prepend a `[FOREGROUND TARGET]` line to the session context block.
///
/// This pins the model to a specific pane ID for foreground execution so it
/// never has to infer the target from topology.  If `target_pane` is `None`
/// or the pane is not in the cache, returns the context unchanged.
pub fn prepend_foreground_target(
    ctx: &str,
    target_pane: Option<&str>,
    cache: &SessionCache,
) -> String {
    let Some(pane_id) = target_pane else {
        return ctx.to_string();
    };
    let (cmd, window) = {
        let panes = cache.panes.read().unwrap_or_log();
        if let Some(p) = panes.get(pane_id) {
            (p.current_cmd.clone(), p.window_name.clone())
        } else {
            (String::new(), String::new())
        }
    };
    let detail = if !cmd.is_empty() && !window.is_empty() {
        format!(" ({}, '{}')", cmd, window)
    } else if !cmd.is_empty() {
        format!(" ({})", cmd)
    } else {
        String::new()
    };
    format!(
        "[FOREGROUND TARGET] {}{} — for run_terminal_command(background=false), always pass target_pane=\"{}\"\n{}",
        pane_id, detail, pane_id, ctx
    )
}

/// Format the current time line injected on every turn.
pub fn format_current_time_line() -> String {
    use chrono::Local;
    let now_local = Local::now();
    let now_utc = now_local.to_utc();
    let tz_name = now_local.format("%Z").to_string();
    format!(
        "[Current time: {} UTC ({}: {})]\n",
        now_utc.format("%Y-%m-%d %H:%M:%S"),
        tz_name,
        now_local.format("%Y-%m-%d %H:%M:%S"),
    )
}

/// Build the first-turn prompt: full host context + terminal snapshot + query.
pub fn build_first_turn_prompt(ctx: &PromptCtx) -> String {
    let session_summary = ctx
        .cache
        .get_labeled_context(ctx.client_pane, ctx.chat_pane);
    let session_summary =
        prepend_foreground_target(&session_summary, ctx.default_target_pane, ctx.cache);
    let sys_ctx = crate::sys_context::get_or_init_sys_context().format_for_ai();
    let daemon_host = crate::daemon::daemon_hostname();
    let environment = &ctx.config.context.environment;
    let pane_location = ctx
        .client_pane
        .and_then(crate::daemon::get_pane_remote_host)
        .map(|h| format!("REMOTE — {}", h))
        .unwrap_or_else(|| format!("LOCAL — same host as daemon ({})", daemon_host));
    let width_hint = ctx.chat_width
        .map(|w| format!("\n- Chat display width: {w} columns (write prose as continuous paragraphs; the terminal word-wraps automatically — do not insert hard line breaks within paragraphs)"))
        .unwrap_or_default();
    let memory_block = crate::memory::load_session_memory_block(ctx.memory_namespaces);
    let manifest_block = crate::manifest::build_knowledge_manifest();
    let auto_search_block = crate::manifest::auto_search_context(ctx.safe_query, &session_summary);
    let tool_restriction_block = ctx
        .tool_policy
        .and_then(crate::agents::policy::format_tool_restriction_block)
        .map(|s| format!("{}\n\n", s))
        .unwrap_or_default();
    let current_time_line = format_current_time_line();
    let pane_map = ctx.cache.pane_map_summary(ctx.chat_pane);

    format!(
        "## Host Context\n```\n{sys_ctx}\n```\n\n\
         ## Execution Context\
         - Environment: {environment}\n\
         - Daemon host: {daemon_host}\n\
         - User's terminal pane: {pane_location}\
         {width_hint}\n\
         - background=true  → runs on DAEMON HOST ({daemon_host})\n\
         - background=false → runs in USER'S PANE ({pane_location})\n\n\
         {tool_restriction_block}\
         {memory_block}\
         {manifest_block}\
         {auto_search_block}\
         ## Terminal Session\n```\n{session_summary}\n```\n\n\
         {pane_map}{current_time_line}User: {}",
        ctx.safe_query
    )
}

/// Build the subsequent-turn prompt: budget note + optional snapshot + query.
pub fn build_subsequent_turn_prompt(ctx: &PromptCtx) -> String {
    let context_window = ctx.config.resolve_model(None).context_window();
    let token_pct = if context_window > 0 {
        (ctx.last_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
    } else {
        0
    };
    let history_count = ctx.history_count + 1; // + the user turn about to be pushed
    let history_cap_budget = crate::config::LimitsConfig::cap_usize(ctx.config.limits.max_history);
    let history_pct = history_cap_budget
        .map(|cap| (history_count as f64 / cap as f64 * 100.0) as u32)
        .unwrap_or(0);

    // Ghost sessions: resolve the effective turn cap.
    let ghost_turn_limit: Option<usize> = ctx.ghost_turn_limit;
    let turn_pct = ghost_turn_limit
        .filter(|l| *l > 0)
        .map(|l| (ctx.this_turn_count as f64 / l as f64 * 100.0) as u32)
        .unwrap_or(0);

    let mut parts: Vec<String> = Vec::new();
    if let Some(limit) = ghost_turn_limit {
        parts.push(format!("turn {}/{}", ctx.this_turn_count, limit));
    }
    match history_cap_budget {
        Some(cap) => parts.push(format!("history {}/{}", history_count, cap)),
        None => parts.push(format!("history {} (no cap)", history_count)),
    };
    if context_window > 0 && ctx.last_prompt_tokens > 0 {
        parts.push(format!(
            "prompt {}k/{}k ({}%)",
            ctx.last_prompt_tokens / 1000,
            context_window / 1000,
            token_pct
        ));
    }

    let max_pct = turn_pct.max(history_pct).max(token_pct);
    let warning = if max_pct >= 75 {
        " — NEAR LIMIT. Summarize progress, persist critical state to memory, and wrap up."
    } else if max_pct >= 50 {
        " — approaching budget; prefer concise responses and avoid redundant tool calls."
    } else {
        ""
    };
    let budget_note = format!("[BUDGET] {}{}\n\n", parts.join(" · "), warning);
    let fg_target_line = ctx.default_target_pane
        .map(|tp| format!("[FOREGROUND TARGET] {} — target_pane=\"{}\" for run_terminal_command(background=false)\n", tp, tp))
        .unwrap_or_default();

    let current_time_line = format_current_time_line();
    let pane_map = ctx.cache.pane_map_summary(ctx.chat_pane);

    // Build session tags for dynamic memory retrieval.
    let recent_keywords: Vec<String> = ctx
        .safe_query
        .split_whitespace()
        .take(5)
        .map(|s| s.to_lowercase())
        .collect();
    let session_tags =
        crate::memory::tags::infer_session_tags(ctx.client_pane, ctx.cache, &recent_keywords);

    // Dynamic turn-relevant memory block (namespace-aware).
    let dynamic_memory = crate::daemon::memory_prompt::assemble_turn_relevant_memory(
        &session_tags,
        ctx.safe_query,
        ctx.config,
        None, // session_id not available in PromptCtx; memory functions use namespaces directly
        ctx.this_turn_count,
        ctx.memory_namespaces,
    )
    .unwrap_or_default();

    let tool_restriction_block = ctx
        .tool_policy
        .and_then(crate::agents::policy::format_tool_restriction_block)
        .map(|s| format!("{}\n\n", s))
        .unwrap_or_default();

    if ctx.inject_snapshot {
        let session_summary = ctx
            .cache
            .get_labeled_context(ctx.client_pane, ctx.chat_pane);
        let session_summary =
            prepend_foreground_target(&session_summary, ctx.default_target_pane, ctx.cache);
        format!(
            "{budget_note}{fg_target_line}{tool_restriction_block}{dynamic_memory}{pane_map}{current_time_line}\
             [Terminal snapshot — auto-refreshed (pane activity detected)]\n\
             ```\n{session_summary}\n```\n\nUser: {}",
            ctx.safe_query
        )
    } else {
        format!(
            "{budget_note}{fg_target_line}{tool_restriction_block}{dynamic_memory}{pane_map}{current_time_line}User: {}",
            ctx.safe_query
        )
    }
}
