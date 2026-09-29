use super::super::ToolCallOutcome;
use crate::daemon::session::{SessionStore, with_sessions};

// ---------------------------------------------------------------------------
// Spawn ghost shell
// ---------------------------------------------------------------------------

/// The `ssh_target` a spawn with `host` gives the child: `Ok(None)` for
/// "local", `Ok(Some(key))` for a granted configured host, `Err(sentence)`
/// otherwise. Checked, in order: the name parses; it is configured; the
/// child's `hosts:` grants it; the caller's (a ghost) grants it.
pub(super) fn spawn_host_target(
    host: &str,
    child: &crate::ipc::GhostConfig,
    caller: Option<&crate::daemon::policy::GhostPolicy>,
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
) -> Result<Option<String>, String> {
    let parsed = crate::host::HostId::parse(host).map_err(|e| format!("Error: {e}"))?;
    let child_policy = crate::daemon::policy::GhostPolicy::from_config(child);
    match &parsed {
        crate::host::HostId::Local => return Ok(None),
        crate::host::HostId::Named(key) => {
            if !hosts.contains_key(key) {
                let configured = if hosts.is_empty() {
                    "no hosts are configured".to_string()
                } else {
                    let mut names: Vec<&String> = hosts.keys().collect();
                    names.sort();
                    names
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return Err(format!(
                    "Error: '{key}' is not a configured host ({configured}); the \
                     specialist cannot run there. Add a [hosts.{key}] entry, or \
                     omit host to use the runbook's ssh_target."
                ));
            }
        }
    }
    if !child_policy.may_use_host(&parsed) {
        return Err(super::super::ghost_host_refusal(&child_policy, &parsed));
    }
    if let Some(caller) = caller
        && !caller.may_use_host(&parsed)
    {
        return Err(super::super::ghost_host_refusal(caller, &parsed));
    }
    Ok(Some(host.to_string()))
}

/// Spawn a ghost session from a runbook. `agent_name` optionally names an
/// agent to run as; `host` names the machine the child's commands run on
/// (a configured `[hosts]` key or `"local"`), overriding the runbook's
/// `ssh_target`, and `caller_policy` is the calling ghost's `hosts:` fence
/// (`None` for a chat caller).
#[allow(clippy::too_many_arguments)]
pub async fn spawn_ghost(
    runbook: &str,
    message: &str,
    agent_name: Option<&str>,
    host: Option<&str>,
    caller_policy: Option<&crate::daemon::policy::GhostPolicy>,
    sessions: &SessionStore,
    spawn_depth: u8,
    parent_job_id: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::ghost::check_ghost_capacity;

    let spawn_config = crate::config::Config::load().unwrap_or_default();
    if !check_ghost_capacity(&spawn_config) {
        return Ok(ToolCallOutcome::Result(format!(
            "Cannot spawn ghost shell: concurrency limit ({}) reached. \
             Wait for an active ghost to complete before spawning another.",
            spawn_config.ghost.max_concurrent_ghosts
        )));
    }

    let rb = match crate::runbook::load_runbook(runbook) {
        Ok(rb) => rb,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Failed to load runbook '{}': {}",
                runbook, e
            )));
        }
    };

    // Build merged ghost config: start from runbook defaults, then apply the
    // agent specified by the AI tool (overrides runbook's own agent: field).
    let mut ghost_config = if let Some(name) = agent_name {
        let mut base = rb.ghost_config.clone();
        base.agent = Some(name.to_string());
        crate::agents::merge_runbook_ghost_config_from(&rb, base)
    } else {
        crate::agents::merge_runbook_ghost_config(&rb)
    };

    ghost_config.spawn_depth = spawn_depth + 1;
    ghost_config.parent_job_id = parent_job_id.map(|s| s.to_string());

    if let Some(host_name) = host {
        match spawn_host_target(host_name, &ghost_config, caller_policy, &spawn_config.hosts) {
            Ok(target) => ghost_config.ssh_target = target,
            Err(msg) => {
                crate::daemon::utils::log_event(
                    "ghost_host_refused",
                    serde_json::json!({
                        "session": "spawn_ghost_shell",
                        "tool": "spawn_ghost_shell",
                        "requested_host": host_name,
                        "allowed_hosts": ghost_config.hosts,
                    }),
                );
                return Ok(ToolCallOutcome::Result(msg));
            }
        }
    }

    start_spawn_session(
        &rb,
        ghost_config,
        message,
        sessions,
        &spawn_config,
        rb.name.clone(),
        agent_name,
    )
    .await
}

/// Start the ghost session an already-validated spawn config describes: no
/// policy decision happens here, only the session start and its bookkeeping.
async fn start_spawn_session(
    rb: &crate::runbook::Runbook,
    ghost_config: crate::ipc::GhostConfig,
    message: &str,
    sessions: &SessionStore,
    spawn_config: &crate::config::Config,
    rb_name: String,
    agent_name: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::ghost::GhostManager;
    use crate::webhook::inject_ghost_event;

    match GhostManager::start_session_with_config(
        sessions.clone(),
        rb,
        &ghost_config,
        message,
        crate::daemon::GS_BG_WINDOW_PREFIX,
        spawn_config.approvals.ghost_commands,
    )
    .await
    {
        Err(e) => Ok(ToolCallOutcome::Result(format!(
            "Failed to start ghost shell: {}",
            e
        ))),
        Ok(sid) => {
            let job_id = sid.clone();
            let task_message = message.to_string();
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(&sid) {
                    entry.ghost_task_message = Some(task_message);
                }
            });
            inject_ghost_event(
                sessions,
                &format!(
                    "[Ghost Shell Started] AI-requested ghost shell started for runbook: {} (job_id: {})",
                    rb_name, job_id
                ),
            )
            .await;
            let tool_result = format!(
                "Ghost shell started (session: {}, job_id: {}, agent: {}). It will run autonomously in the background \
                 and inject [Ghost Shell Completed] or [Ghost Shell Failed] events when done. \
                 Use the job_id and agent name with await_agent_result(job_id, agent_name) to wait for the result.",
                sid,
                job_id,
                agent_name.unwrap_or("(default)")
            );
            Ok(ToolCallOutcome::SpawnGhostSession {
                session_id: sid,
                runbook_name: rb_name,
                tool_result,
                job_id,
            })
        }
    }
}

#[cfg(test)]
mod ghost_host_tests {
    use super::spawn_host_target;
    use crate::daemon::policy::GhostPolicy;
    use crate::daemon::session::with_sessions;
    use crate::ipc::GhostConfig;

    fn host_map(names: &[&str]) -> std::collections::HashMap<String, crate::config::HostEntry> {
        names
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    crate::config::HostEntry {
                        ssh: n.to_string(),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn child(hosts: &[&str]) -> GhostConfig {
        GhostConfig {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..Default::default()
        }
    }

    fn caller(hosts: &[&str]) -> GhostPolicy {
        GhostPolicy {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..GhostPolicy::from_config(&GhostConfig::default())
        }
    }

    /// The specialist's `hosts:` list and the calling ghost's must both
    /// grant the host: either alone is not enough. `"local"` needs no
    /// configuration (it is the daemon host), an unparsable name is refused,
    /// and empty lists on both sides are unrestricted.
    #[test]
    fn spawn_host_target_is_granted_only_by_both_hosts_lists() {
        let hosts = host_map(&["db01", "db02"]);

        assert_eq!(
            spawn_host_target("db01", &child(&["db01"]), None, &hosts).expect("granted"),
            Some("db01".to_string()),
            "a configured host the child's hosts: list grants is the target"
        );

        let unlisted = spawn_host_target("db02", &child(&["db01"]), None, &hosts);
        let err = unlisted.expect_err("the child's list must refuse db02");
        assert!(
            err.contains("db02"),
            "the refusal must name the host: {err}"
        );

        let caller_refuses =
            spawn_host_target("db01", &child(&["db01"]), Some(&caller(&["db02"])), &hosts);
        let err =
            caller_refuses.expect_err("a caller whose hosts: list does not grant db01 refuses it");
        assert!(
            err.contains("db01"),
            "the refusal must name the host: {err}"
        );

        assert_eq!(
            spawn_host_target("local", &child(&["db01"]), None, &hosts).expect("local"),
            None,
            "local is the daemon host: no ssh_target for the child"
        );

        let unconfigured = spawn_host_target("nope", &child(&["db01"]), None, &hosts);
        let err = unconfigured.expect_err("an unconfigured host is refused");
        assert!(
            err.contains("nope"),
            "the refusal must name the host: {err}"
        );

        let malformed = spawn_host_target("a b", &child(&["db01"]), None, &hosts);
        assert!(malformed.is_err(), "an unparsable host name is refused");

        assert_eq!(
            spawn_host_target("db02", &child(&[]), Some(&caller(&[])), &hosts)
                .expect("empty lists are unrestricted"),
            Some("db02".to_string()),
            "empty hosts: lists on both sides grant any configured host"
        );
    }

    /// Door test: a spawn whose `host` its `hosts:` list does not grant is
    /// refused with a `Result` sentence naming the host, and **no session is
    /// started** — the ghost never exists.
    #[test]
    fn spawn_ghost_refuses_an_unlisted_host_before_any_ghost_starts() {
        let guard = crate::test_home_guard();
        let home = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        crate::config::Config::ensure_dirs().expect("daemon tree");
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[hosts.db01]\nssh = \"db01\"\n\n[hosts.db02]\nssh = \"db02\"\n",
        )
        .expect("write config");
        std::fs::write(
            crate::config::runbooks_dir().join("rb08d.md"),
            "---\nenabled: true\nhosts: [db01]\n---\n# Runbook: rb08d\n\n## Alert Criteria\n- x\n",
        )
        .expect("write runbook");

        let sessions = crate::daemon::session::SessionStore::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let outcome = rt
            .block_on(crate::daemon::executor::knowledge::spawn_ghost(
                "rb08d",
                "task",
                None,
                Some("db02"),
                None,
                &sessions,
                0,
                None,
            ))
            .expect("a refusal is a normal outcome, not an error");
        let text = match outcome {
            crate::daemon::executor::ToolCallOutcome::Result(text) => text,
            crate::daemon::executor::ToolCallOutcome::UserMessage(t) => t,
            crate::daemon::executor::ToolCallOutcome::SpawnGhostSession { tool_result, .. } => {
                tool_result
            }
        };
        assert!(
            text.contains("db02"),
            "the refusal must name the host: {text}"
        );
        let started = with_sessions(&sessions, |store| !store.is_empty());
        assert!(
            !started,
            "no session may be started when the host is refused"
        );
        drop(guard);
    }
}
