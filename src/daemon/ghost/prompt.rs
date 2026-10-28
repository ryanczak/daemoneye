//! A ghost run's system prompt: the runbook's policy lines, its seeded
//! steps and the static rules, assembled once per run by `do_ghost_turn`.
//! A child module of `src/daemon/ghost.rs`.

use super::*;

/// What `build_ghost_prompt` hands back: the system prompt and the two
/// values the turn loop reads from the same runbook config.
pub(super) struct GhostPrompt {
    pub system: String,
    pub max_ghost_turns: usize,
    pub steps: Vec<String>,
}

pub(super) fn build_ghost_prompt(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    tmux_session: &str,
) -> GhostPrompt {
    let prompt_name = config.ai.prompt.clone();
    let system_base = load_named_prompt(&prompt_name).system;
    let sys_context = get_or_init_sys_context();

    let daemon_ceiling = config.ghost.max_ghost_turns;
    let (approved_scripts, run_with_sudo, max_ghost_turns, ssh_target, auto_approve_commands) =
        with_sessions(sessions, |store| {
            store
                .get(session_id)
                .and_then(|e| e.ghost_config.as_ref())
                .map(|gc| {
                    let scripts = if gc.auto_approve_scripts.is_empty() {
                        "none".to_string()
                    } else {
                        gc.auto_approve_scripts.join(", ")
                    };
                    let turns = if gc.max_ghost_turns > 0 {
                        gc.max_ghost_turns.min(daemon_ceiling)
                    } else {
                        daemon_ceiling
                    };
                    (
                        scripts,
                        gc.run_with_sudo,
                        turns,
                        gc.ssh_target.clone(),
                        gc.auto_approve_commands,
                    )
                })
                .unwrap_or_else(|| ("none".to_string(), false, daemon_ceiling, None, false))
        });
    let remote_line = if let Some(ref target) = ssh_target {
        format!(
            "Remote SSH Target: {} — all commands are automatically wrapped in \
             `ssh {}` and executed on this host. \
             Do NOT manually SSH to the target; call run_terminal_command with the \
             command directly and the daemon handles SSH transparently.\n         ",
            target, target
        )
    } else {
        String::new()
    };
    let agents_block = crate::daemon::prompt::format_available_agents();
    let steps: Vec<String> = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.ghost_runbook_name.clone())
    })
    .and_then(|name| match crate::runbook::load_runbook(&name) {
        Ok(rb) => Some(crate::runbook::parse_steps(&rb.content)),
        Err(e) => {
            log::warn!(
                "Ghost Shell {}: runbook '{}' unreadable, no seeded steps ({}): {}",
                session_id,
                name,
                e,
                e
            );
            None
        }
    })
    .unwrap_or_default();
    let steps_block = if steps.is_empty() {
        String::new()
    } else {
        let listed: String = steps
            .iter()
            .enumerate()
            .map(|(i, text)| format!("  {}. {}\n", i + 1, text))
            .collect();
        format!(
            "## Seeded Steps ({})\n\
             This runbook defines {} steps. You may not finish until every one is marked.\n\
             Emit, on its own line, one of:\n\
               {} <n>: DONE\n  \
               {} <n>: SKIPPED <reason>\n\
             for each step 1..={}. A step is only counted when that exact line appears —\n\
             describing the step in prose does not mark it.\n\
             {}",
            steps.len(),
            steps.len(),
            STEP_MARKER_PREFIX,
            STEP_MARKER_PREFIX,
            steps.len(),
            listed
        )
    };
    let system = format!(
        "{}\n\n\
         ## Ghost Shell Execution Context\n\
         Daemon Host: {}\n\
         Tmux Session: {}\n\
         {}Command Policy: non-sudo commands run freely (OS permissions are the boundary){}. \
         {}\n\
         Pre-approved Sudo Scripts: {}{}\n\
         Turn Budget: {} (hard limit — shell will be stopped when reached)\n\n\
         {}\n\n\
         {}\
         {}{}",
        system_base,
        daemon_hostname(),
        tmux_session,
        remote_line,
        if auto_approve_commands {
            " — explicitly approved for investigation commands"
        } else {
            ""
        },
        if run_with_sudo {
            "Sudo commands are freely allowed (run_with_sudo is enabled for this runbook)."
                .to_string()
        } else {
            "Sudo commands require a pre-approved script via install-sudoers.".to_string()
        },
        approved_scripts,
        if run_with_sudo {
            " (executed with sudo)"
        } else {
            ""
        },
        max_ghost_turns,
        sys_context.format_for_ai(),
        GHOST_SHELL_RULES,
        agents_block,
        steps_block,
    );
    GhostPrompt {
        system,
        max_ghost_turns,
        steps,
    }
}
