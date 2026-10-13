use crate::tmux::cache::SessionCache;

/// Build an `## Available Agents` block for the system prompt.
///
/// Lists all registered agents with their description and model.
/// Returns an empty string if no agents are registered.
pub fn format_available_agents() -> String {
    let agents = match crate::agents::list_agents() {
        Ok(a) => a,
        Err(_) => return String::new(),
    };
    if agents.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push("## Available Agents".to_string());
    for info in &agents {
        let model_str = info.model.as_deref().unwrap_or("(default model)");
        lines.push(format!(
            "- **{}**: {} (model: {})",
            info.name, info.description, model_str
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Context passed to prompt builder functions.
/// Follows the `SessionCtx<'a>` pattern — borrow-only, no clones.
pub struct PromptCtx<'a> {
    pub client_pane: Option<&'a str>,
    pub chat_pane: Option<&'a str>,
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
    /// Current session id, used to exclude this session's own history from the
    /// situational block. `None` for callers with no session.
    pub session_id: Option<&'a str>,
    /// Agent-level tool policy, if this session has one.
    pub tool_policy: Option<&'a crate::agents::ToolPolicy>,
    /// Agent name for briefing injection (ghost shells only).
    pub agent_name: Option<&'a str>,
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
    let in_tmux = ctx.chat_pane.is_some();
    let session_summary = if in_tmux {
        ctx.cache
            .get_labeled_context(ctx.client_pane, ctx.chat_pane)
    } else {
        crate::daemon::executor::knowledge::shells_only_context(&crate::shell::Owner::Chat(
            ctx.session_id.unwrap_or_default().to_string(),
        ))
    };
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
    let briefing_block = ctx
        .agent_name
        .and_then(crate::daemon::briefing::read_briefing)
        .map(|content| format!("## Previous Session Summary\n{}\n\n", content))
        .unwrap_or_default();
    let current_time_line = format_current_time_line();
    let pane_map = if in_tmux {
        ctx.cache.pane_map_summary(ctx.chat_pane)
    } else {
        String::new()
    };
    let shell_block = crate::daemon::executor::knowledge::shell_context_block(
        &crate::shell::Owner::Chat(ctx.session_id.unwrap_or_default().to_string()),
    );
    let hosts_block = crate::daemon::executor::knowledge::hosts_context_block(ctx.config);

    format!(
        "## Host Context\n```\n{sys_ctx}\n```\n\n\
         ## Execution Context\
         - Environment: {environment}\n\
         - Daemon host: {daemon_host}\n\
         - User's terminal pane: {pane_location}\
         {width_hint}\n\
         - Commands run in daemon-owned PTY shells on the daemon host. \
         `wait: false` returns at once. `host` runs the command on a configured \
         `[hosts.*]` host.\n\n\
         {tool_restriction_block}\
         {briefing_block}\
         {memory_block}\
         {manifest_block}\
         {auto_search_block}\
         {hosts_block}{shell_block}\
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
    parts.push(format!("history {}", history_count));
    if context_window > 0 && ctx.last_prompt_tokens > 0 {
        parts.push(format!(
            "prompt {}k/{}k ({}%)",
            ctx.last_prompt_tokens / 1000,
            context_window / 1000,
            token_pct
        ));
    }

    let warning = if let Some(_limit) = ctx.ghost_turn_limit {
        // Ghost sessions: turn budget is real — keep the existing wrap-up wording.
        let ghost_max_pct = turn_pct.max(token_pct);
        if ghost_max_pct >= 75 {
            " — NEAR LIMIT. Summarize progress, persist critical state to memory, and wrap up."
        } else if ghost_max_pct >= 50 {
            " — approaching budget; prefer concise responses and avoid redundant tool calls."
        } else {
            ""
        }
    } else {
        // Interactive sessions: token pressure is the compactor's business, not
        // the model's.  Note that compaction will handle token pressure automatically.
        if token_pct >= 50 {
            " — context compaction will run automatically"
        } else {
            ""
        }
    };
    let budget_note = format!("[BUDGET] {}{}\n\n", parts.join(" · "), warning);

    let current_time_line = format_current_time_line();
    let pane_map = if ctx.chat_pane.is_some() {
        ctx.cache.pane_map_summary(ctx.chat_pane)
    } else {
        String::new()
    };
    let shell_block = crate::daemon::executor::knowledge::shell_context_block(
        &crate::shell::Owner::Chat(ctx.session_id.unwrap_or_default().to_string()),
    );
    let hosts_block = crate::daemon::executor::knowledge::hosts_context_block(ctx.config);

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
        ctx.session_id,
        ctx.this_turn_count,
        ctx.memory_namespaces,
    )
    .unwrap_or_default();

    let situational =
        crate::daemon::situational::assemble_situational_block(ctx.safe_query, ctx.session_id)
            .map(|s| format!("{}\n\n", s))
            .unwrap_or_default();

    let tool_restriction_block = ctx
        .tool_policy
        .and_then(crate::agents::policy::format_tool_restriction_block)
        .map(|s| format!("{}\n\n", s))
        .unwrap_or_default();

    if ctx.inject_snapshot {
        let session_summary = if ctx.chat_pane.is_some() {
            ctx.cache
                .get_labeled_context(ctx.client_pane, ctx.chat_pane)
        } else {
            crate::daemon::executor::knowledge::shells_only_context(&crate::shell::Owner::Chat(
                ctx.session_id.unwrap_or_default().to_string(),
            ))
        };
        format!(
            "{budget_note}{hosts_block}{shell_block}{tool_restriction_block}{dynamic_memory}{situational}{pane_map}{current_time_line}\
             [Terminal snapshot — auto-refreshed (pane activity detected)]\n\
             ```\n{session_summary}\n```\n\nUser: {}",
            ctx.safe_query
        )
    } else {
        format!(
            "{budget_note}{hosts_block}{shell_block}{tool_restriction_block}{dynamic_memory}{situational}{pane_map}{current_time_line}User: {}",
            ctx.safe_query
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::cache::PaneState;
    use crate::util::UnpoisonExt;

    /// A prompt fixture cache that holds one pane the way a live refresh
    /// would: a user shell in the adopted session, enough for
    /// `get_labeled_context` to emit a pane block and `pane_map_summary` a
    /// `[PANE MAP]` line. The buffer is non-empty so the pane's summary is
    /// real text, not a fallback.
    fn cache_with_one_pane(session: &str) -> SessionCache {
        let c = SessionCache::new(session);
        let mut pane = PaneState {
            buffer: String::new(),
            summary: String::new(),
            current_cmd: String::new(),
            current_path: String::new(),
            pane_title: String::new(),
            last_updated: std::time::Instant::now(),
            scroll_position: 0,
            history_size: 0,
            in_copy_mode: false,
            synchronized: false,
            window_name: String::new(),
            session_name: session.to_string(),
            dead: false,
            dead_status: None,
            last_activity: 0,
            start_cmd: String::new(),
            pane_index: 0,
            shell_pid: 0,
            status: crate::shell::status::PaneStatus::Idle(0),
        };
        pane.buffer = "make build".to_string();
        pane.summary = "running make".to_string();
        pane.current_cmd = "bash".to_string();
        pane.current_path = "/home/user".to_string();
        pane.window_name = "editor".to_string();
        pane.pane_index = 0;
        c.panes
            .write()
            .unwrap_or_log()
            .insert("%1".to_string(), pane);
        c
    }

    fn ctx_with<'a>(
        cache: &'a SessionCache,
        config: &'a crate::config::Config,
        chat_pane: Option<&'a str>,
        client_pane: Option<&'a str>,
        session_id: Option<&'a str>,
        inject_snapshot: bool,
    ) -> PromptCtx<'a> {
        PromptCtx {
            client_pane,
            chat_pane,
            cache,
            config,
            chat_width: None,
            safe_query: "hello",
            last_prompt_tokens: 0,
            history_count: 0,
            this_turn_count: 0,
            ghost_turn_limit: None,
            inject_snapshot,
            memory_namespaces: &["global"],
            session_id,
            tool_policy: None,
            agent_name: None,
        }
    }

    fn plain_config() -> crate::config::Config {
        crate::config::Config::default()
    }

    /// PE decision 1, first turn: a chat outside tmux must see the daemon's
    /// own shells, not the panes of a tmux session the user is not in.
    #[test]
    fn first_turn_prompt_without_chat_pane_carries_shells_only() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let reg = crate::shell::ShellRegistry::default();
        let _swap = crate::daemon::executor::knowledge::open_shell_test_seam::swap_in(&reg);
        crate::shell::with_shells(&reg, |store| {
            store.insert(
                "s0".to_string(),
                crate::shell::ShellRecord {
                    id: crate::shell::ShellId::from_index(0),
                    owner: crate::shell::Owner::Chat("sess-a".to_string()),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 1_788_470_000,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });
        let cache = cache_with_one_pane("home");
        let prompt = build_first_turn_prompt(&ctx_with(
            &cache,
            &plain_config(),
            None,
            None,
            Some("sess-a"),
            false,
        ));
        assert!(
            prompt.contains("[SHELLS] 1"),
            "the shells block must count the fixture shell: {prompt}"
        );
        assert!(
            !prompt.contains("PANE %"),
            "no pane block may reach a pane-less chat: {prompt}"
        );
        // sre.toml's guidance prose names `[PANE MAP]` and `[SESSION TOPOLOGY]`
        // in plain words, so the tags themselves cannot be asserted absent.
        // Assert on the fixture's rendered pane data instead: pane `%1`, index
        // 0, summary "running make", window 'editor' — none of it may appear.
        assert!(
            !prompt.contains("idx:0=%1"),
            "no pane map entry may reach a pane-less chat: {prompt}"
        );
        assert!(
            !prompt.contains("running make"),
            "no fixture pane summary may reach a pane-less chat: {prompt}"
        );
        assert!(
            !prompt.contains("[SESSION TOPOLOGY] 1 windows"),
            "the fixture's window must not render as topology: {prompt}"
        );
    }

    /// Preservation: with a chat pane, everything M12 built still ships.
    #[test]
    fn first_turn_prompt_with_chat_pane_keeps_tmux_context() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let cache = cache_with_one_pane("home");
        let prompt = build_first_turn_prompt(&ctx_with(
            &cache,
            &plain_config(),
            Some("%2"),
            Some("%2"),
            Some("sess-a"),
            false,
        ));
        assert!(
            prompt.contains("PANE %1"),
            "the pane snapshot is still there with a chat pane: {prompt}"
        );
        assert!(
            prompt.contains("[PANE MAP]"),
            "the pane map is still there with a chat pane: {prompt}"
        );
    }

    /// The model is never told to pass a `target_pane`: commands run in
    /// daemon-owned PTY shells, so there is no foreground pane to name, even
    /// when a pane context is present.
    #[test]
    fn no_prompt_names_a_foreground_target() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let cache = cache_with_one_pane("home");
        let first = build_first_turn_prompt(&ctx_with(
            &cache,
            &plain_config(),
            Some("%2"),
            Some("%2"),
            Some("sess-a"),
            false,
        ));
        let subsequent = build_subsequent_turn_prompt(&ctx_with(
            &cache,
            &plain_config(),
            Some("%2"),
            Some("%2"),
            Some("sess-a"),
            true,
        ));
        for (label, prompt) in [("first turn", first), ("subsequent turn", subsequent)] {
            assert!(
                !prompt.contains("FOREGROUND TARGET"),
                "{label} must not name a foreground target: {prompt}"
            );
            assert!(
                !prompt.contains("target_pane="),
                "{label} must not tell the model to pass a target_pane: {prompt}"
            );
        }
    }

    /// PE decision 1, subsequent turns: the auto-refreshed snapshot obeys the
    /// same rule as the first turn.
    #[test]
    fn subsequent_turn_snapshot_without_chat_pane_carries_shells_only() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let reg = crate::shell::ShellRegistry::default();
        let _swap = crate::daemon::executor::knowledge::open_shell_test_seam::swap_in(&reg);
        crate::shell::with_shells(&reg, |store| {
            store.insert(
                "s0".to_string(),
                crate::shell::ShellRecord {
                    id: crate::shell::ShellId::from_index(0),
                    owner: crate::shell::Owner::Chat("sess-a".to_string()),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 1_788_470_000,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });
        let cache = cache_with_one_pane("home");
        let prompt = build_subsequent_turn_prompt(&ctx_with(
            &cache,
            &plain_config(),
            None,
            None,
            Some("sess-a"),
            true,
        ));
        assert!(
            prompt.contains("[SHELLS]"),
            "the shells block replaces the pane snapshot: {prompt}"
        );
        assert!(
            !prompt.contains("PANE %"),
            "no pane block may reach a pane-less chat: {prompt}"
        );
        assert!(
            !prompt.contains("[PANE MAP]"),
            "no pane map may reach a pane-less chat: {prompt}"
        );
        assert!(
            !prompt.contains("[FOREGROUND TARGET]"),
            "no foreground-target line may reach a pane-less chat: {prompt}"
        );
    }

    /// The pre-existing fixture: unchanged behavior for the hosts block,
    /// driven through a dedicated `PromptCtx` built with a bare cache.
    #[test]
    fn prompts_carry_the_hosts_block() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        let cache = SessionCache::new("t");
        let mut config = crate::config::Config::default();
        config.hosts.insert(
            "pinky".to_string(),
            crate::config::HostEntry {
                ssh: "pinky".to_string(),
                ..Default::default()
            },
        );
        let mut base = ctx_with(&cache, &config, None, None, None, false);
        base.safe_query = "hello";
        let prompt = build_subsequent_turn_prompt(&base);
        assert!(prompt.contains("[HOSTS] local ("), "{prompt}");
        assert!(prompt.contains("pinky (ssh pinky, "), "{prompt}");
        assert!(
            prompt.find("[HOSTS]") < prompt.find("User: hello"),
            "the block is in the context, ahead of the user's text: {prompt}"
        );

        let first = build_first_turn_prompt(&base);
        assert!(first.contains("pinky (ssh pinky, "), "{first}");

        let plain = crate::config::Config::default();
        let without =
            build_subsequent_turn_prompt(&ctx_with(&cache, &plain, None, None, None, false));
        assert!(
            !without.contains("[HOSTS]"),
            "no hosts, no block: {without}"
        );
    }
}
