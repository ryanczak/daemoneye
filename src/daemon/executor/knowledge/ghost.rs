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
/// Narrow a child ghost, about to be spawned by `caller`, so it never holds
/// more than its caller: hosts, `ssh_target`, pre-approved sudo scripts, the
/// sudo/command/TUI flags and the agent tool policy. Mutates `child` in
/// place, or refuses with a sentence when the child cannot be narrowed
/// (disjoint host lists, or an `ssh_target` outside the caller's fence).
/// The caller's tool policy comes from the calling session's agent
/// (`None` for a chat caller, which is not narrowed at all).
pub(super) fn narrow_to_caller(
    child: &mut crate::ipc::GhostConfig,
    caller: &crate::daemon::policy::GhostPolicy,
    caller_tool_policy: Option<&crate::agents::ToolPolicy>,
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
) -> Result<(), String> {
    // Hosts: an empty caller list is unrestricted; an empty child list
    // inherits the caller's; otherwise intersect — and disjoint non-empty
    // lists refuse, because an empty result would mean unrestricted.
    if !caller.hosts.is_empty() {
        if child.hosts.is_empty() {
            child.hosts = caller.hosts.clone();
        } else {
            let intersection: Vec<String> = child
                .hosts
                .iter()
                .filter(|h| caller.hosts.contains(h))
                .cloned()
                .collect();
            if intersection.is_empty() {
                let caller_list = caller.hosts.join(", ");
                let child_list = child.hosts.join(", ");
                return Err(format!(
                    "Error: cannot spawn this specialist. Its hosts ({child_list}) \
                     and yours ({caller_list}) share no host, so it could not be \
                     narrowed to your fence."
                ));
            }
            child.hosts = intersection;
        }
    }

    // ssh_target: resolved as at ghost start, it must pass the narrowed
    // hosts: list. A child with no ssh_target passes.
    if let Some(target) = child.ssh_target.as_deref() {
        let remote_host = match crate::host::resolve_ssh_target(target, hosts) {
            crate::host::SshTargetResolution::Configured(key) => {
                crate::host::HostId::parse(&key).ok()
            }
            crate::host::SshTargetResolution::Unconfigured
            | crate::host::SshTargetResolution::Ambiguous(_) => None,
        };
        let granted = match &remote_host {
            Some(crate::host::HostId::Named(name)) => {
                child.hosts.is_empty() || child.hosts.iter().any(|h| h == name)
            }
            Some(crate::host::HostId::Local) => true,
            None => child.hosts.is_empty(),
        };
        if !granted {
            let caller_list = if caller.hosts.is_empty() {
                "all hosts".to_string()
            } else {
                caller.hosts.join(", ")
            };
            return Err(format!(
                "Error: cannot spawn this specialist. Its ssh_target '{target}' \
                 is outside your host fence ({caller_list})."
            ));
        }
    }

    // Pre-approved sudo scripts: keep only the names the caller also holds.
    child.auto_approve_scripts = child
        .auto_approve_scripts
        .iter()
        .filter(|s| caller.auto_approve_scripts.contains(s))
        .cloned()
        .collect();

    // Flags: child and caller.
    child.run_with_sudo = child.run_with_sudo && caller.run_with_sudo;
    child.auto_approve_commands = child.auto_approve_commands && caller.auto_approve_commands;
    child.allow_tui_input = child.allow_tui_input && caller.allow_tui_input;

    // Tool policy: intersect with the caller's; a child with no policy takes
    // the caller's; no caller policy (a chat caller) leaves the child's.
    if let Some(caller_tools) = caller_tool_policy {
        child.tool_policy = Some(match &child.tool_policy {
            Some(child_tools) => child_tools.intersect(caller_tools),
            None => caller_tools.clone(),
        });
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn_ghost(
    runbook: &str,
    message: &str,
    agent_name: Option<&str>,
    host: Option<&str>,
    caller_policy: Option<&crate::daemon::policy::GhostPolicy>,
    caller_tool_policy: Option<&crate::agents::ToolPolicy>,
    sessions: &SessionStore,
    spawn_depth: u8,
    parent_job_id: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::ghost::check_ghost_capacity;

    let spawn_config = crate::config::current();
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

    if let Some(caller) = caller_policy
        && let Err(msg) = narrow_to_caller(
            &mut ghost_config,
            caller,
            caller_tool_policy,
            &spawn_config.hosts,
        )
    {
        crate::daemon::utils::log_event(
            "ghost_spawn_narrowed_refused",
            serde_json::json!({
                "session": "spawn_ghost_shell",
                "tool": "spawn_ghost_shell",
                "runbook": runbook,
                "caller_hosts": caller.hosts,
                "child_hosts": ghost_config.hosts,
            }),
        );
        return Ok(ToolCallOutcome::Result(msg));
    }

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
    use crate::webhook::{GhostEvent, publish_ghost_event};

    let trigger = if ghost_config.parent_job_id.is_some() {
        crate::daemon::GhostTrigger::Delegated
    } else {
        crate::daemon::GhostTrigger::Interactive
    };
    match GhostManager::start_session_with_config(
        sessions.clone(),
        rb,
        &ghost_config,
        message,
        trigger,
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
                if let Some(g) = store.get_mut(&sid).and_then(|e| e.as_ghost_mut()) {
                    g.task_message = Some(task_message);
                }
            });
            publish_ghost_event(
                crate::bus::global(),
                GhostEvent::Started,
                &rb_name,
                &format!(
                    "[Ghost Shell Started] AI-requested ghost shell started for runbook: {} (job_id: {})",
                    rb_name, job_id
                ),
            );
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
mod ghost_narrow_tests {
    use super::narrow_to_caller;
    use crate::agents::ToolPolicy;
    use crate::daemon::policy::GhostPolicy;
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

    #[test]
    fn narrow_to_caller_intersects_hosts() {
        // caller [] (unrestricted): the child keeps its list.
        let mut c = child(&["db01"]);
        narrow_to_caller(&mut c, &caller(&[]), None, &host_map(&["db01", "db02"]))
            .expect("an unrestricted caller changes nothing");
        assert_eq!(c.hosts, vec!["db01".to_string()]);

        // child [] (unrestricted): it takes the caller's list.
        let mut c = child(&[]);
        narrow_to_caller(
            &mut c,
            &caller(&["db01", "db02"]),
            None,
            &host_map(&["db01", "db02", "db03"]),
        )
        .expect("an unfenced child inherits the caller's fence");
        assert_eq!(c.hosts, vec!["db01".to_string(), "db02".to_string()]);

        // both non-empty: intersection.
        let mut c = child(&["db02", "db03"]);
        narrow_to_caller(
            &mut c,
            &caller(&["db01", "db02"]),
            None,
            &host_map(&["db01", "db02", "db03"]),
        )
        .expect("overlapping lists intersect");
        assert_eq!(c.hosts, vec!["db02".to_string()]);

        // disjoint non-empty lists: refuse.
        let mut c = child(&["db02"]);
        let err = narrow_to_caller(
            &mut c,
            &caller(&["db01"]),
            None,
            &host_map(&["db01", "db02"]),
        )
        .expect_err("disjoint host lists must refuse");
        assert!(
            err.contains("db01") && err.contains("db02"),
            "the refusal must name both fences: {err}"
        );
    }

    #[test]
    fn narrow_to_caller_refuses_an_ssh_target_outside_the_caller_fence() {
        let hosts = host_map(&["db01", "db02"]);
        let fence = caller(&["db01"]);

        let mut c = GhostConfig {
            ssh_target: Some("db02".to_string()),
            ..child(&[])
        };
        let err = narrow_to_caller(&mut c, &fence, None, &hosts)
            .expect_err("an ssh_target outside the caller's fence must refuse");
        assert!(
            err.contains("db02"),
            "the refusal must name the target: {err}"
        );

        let mut c = GhostConfig {
            ssh_target: Some("db01".to_string()),
            ..child(&[])
        };
        narrow_to_caller(&mut c, &fence, None, &hosts)
            .expect("an ssh_target inside the caller's fence passes");

        let mut c = child(&[]);
        narrow_to_caller(&mut c, &fence, None, &hosts).expect("a child with no ssh_target passes");
    }

    #[test]
    fn narrow_to_caller_intersects_scripts_and_flags() {
        let mut c = GhostConfig {
            auto_approve_scripts: vec!["b.sh".to_string(), "c.sh".to_string()],
            run_with_sudo: true,
            auto_approve_commands: true,
            allow_tui_input: true,
            ..child(&[])
        };
        let mut f = GhostPolicy::from_config(&GhostConfig::default());
        f.auto_approve_scripts = vec!["a.sh".to_string(), "b.sh".to_string()];
        narrow_to_caller(&mut c, &f, None, &host_map(&[]))
            .expect("a caller with no host fence does not refuse");
        assert_eq!(
            c.auto_approve_scripts,
            vec!["b.sh".to_string()],
            "only scripts the caller also holds survive"
        );

        // Child true / caller false: each flag must be false afterwards.
        let mut c = GhostConfig {
            run_with_sudo: true,
            auto_approve_commands: true,
            allow_tui_input: true,
            ..child(&[])
        };
        narrow_to_caller(&mut c, &f, None, &host_map(&[])).expect("narrowed");
        assert!(!c.run_with_sudo && !c.auto_approve_commands && !c.allow_tui_input);

        // Both true: each flag stays true.
        let mut t = GhostPolicy::from_config(&GhostConfig::default());
        t.run_with_sudo = true;
        t.auto_approve_commands = true;
        t.allow_tui_input = true;
        let mut c = GhostConfig {
            run_with_sudo: true,
            auto_approve_commands: true,
            allow_tui_input: true,
            ..child(&[])
        };
        narrow_to_caller(&mut c, &t, None, &host_map(&[])).expect("narrowed");
        assert!(c.run_with_sudo && c.auto_approve_commands && c.allow_tui_input);
    }

    #[test]
    fn narrow_to_caller_intersects_the_tool_policy() {
        let mut c = GhostConfig {
            tool_policy: Some(ToolPolicy {
                allow: Some(vec!["read_file".to_string(), "send_input".to_string()]),
                deny: None,
            }),
            ..child(&[])
        };
        let caller_policy = ToolPolicy {
            allow: None,
            deny: Some(vec!["send_input".to_string()]),
        };
        narrow_to_caller(&mut c, &caller(&[]), Some(&caller_policy), &host_map(&[]))
            .expect("a caller with no host fence does not refuse");
        let narrowed = c.tool_policy.expect("the child keeps a policy");
        assert!(
            narrowed.permits("read_file"),
            "read_file survives the intersection"
        );
        assert!(
            !narrowed.permits("send_input"),
            "send_input the caller denies must be refused"
        );

        // A child with no policy takes the caller's.
        let mut c = child(&[]);
        let caller_policy = ToolPolicy {
            allow: Some(vec!["read_file".to_string()]),
            deny: None,
        };
        narrow_to_caller(&mut c, &caller(&[]), Some(&caller_policy), &host_map(&[]))
            .expect("narrowed");
        let narrowed = c
            .tool_policy
            .expect("the child inherits the caller's policy");
        assert!(narrowed.permits("read_file"));
        assert!(!narrowed.permits("send_input"));
    }

    /// With no caller tool policy, the child's is unchanged (a chat caller).
    #[test]
    fn narrow_to_caller_keeps_the_child_tool_policy_without_a_caller_policy() {
        let mut c = GhostConfig {
            tool_policy: Some(ToolPolicy {
                allow: Some(vec!["read_file".to_string()]),
                deny: None,
            }),
            ..child(&[])
        };
        narrow_to_caller(&mut c, &caller(&[]), None, &host_map(&[])).expect("narrowed");
        let kept = c.tool_policy.expect("policy unchanged");
        assert!(kept.permits("read_file"));
        assert!(!kept.permits("send_input"), "still only its own allow list");
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

    /// Door test: a spawn whose runbook `hosts:` falls entirely outside the
    /// caller's fence is refused with a `Result` sentence, and no session is
    /// started — even with `host` omitted, where the child's `ssh_target`
    /// would otherwise run unchecked.
    #[test]
    fn spawn_ghost_refuses_a_child_outside_the_caller_fence() {
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
            crate::config::runbooks_dir().join("rb06a.md"),
            "---\nenabled: true\nhosts: [db02]\n---\n# Runbook: rb06a\n\n## Alert Criteria\n- x\n",
        )
        .expect("write runbook");

        let sessions = crate::daemon::session::SessionStore::new();
        let caller = GhostPolicy {
            hosts: vec!["db01".to_string()],
            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let outcome = rt
            .block_on(crate::daemon::executor::knowledge::spawn_ghost(
                "rb06a",
                "task",
                None,
                None,
                Some(&caller),
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
            text.contains("cannot spawn this specialist"),
            "the refusal must say the spawn was narrowed away: {text}"
        );
        let started = with_sessions(&sessions, |store| !store.is_empty());
        assert!(
            !started,
            "no session may be started when the spawn is refused"
        );
        drop(guard);
    }
}
