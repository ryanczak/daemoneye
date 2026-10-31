//! The host gate the host-taking arms share: `resolve_host` parses the call's
//! `host` argument, `refuse_ghost_host` refuses a ghost a host its runbook's
//! `hosts:` list does not name. A child module of
//! `src/daemon/executor/mod.rs`.

use crate::daemon::policy::GhostPolicy;

/// Resolve a tool call's `host` argument **before** the approval prompt.
///
/// A malformed name is refused with [`crate::host::HostId::parse`]'s own
/// message — a name that can never be a host is not a decision the user
/// should be asked to make, so it never reaches the panel. An *unconfigured*
/// well-formed name is deliberately **not** refused here: it is
/// phase-06c's `new_host` case and must reach the prompt, then connect on
/// approval using the bare name as its own ssh destination.
///
/// Returns the parsed id for the prompt's classification and the later run
/// (parsed once, not twice), or the refusal sentence for the model.
pub(crate) fn resolve_host(host: Option<&str>) -> Result<crate::host::HostId, String> {
    match host {
        None => Ok(crate::host::HostId::Local),
        Some(name) => crate::host::HostId::parse(name),
    }
}

/// The sentence a ghost reads when it asks for a host its runbook did not
/// grant: the refused host **and** the allowed set, so the model can correct
/// itself rather than retrying blindly.
pub(crate) fn ghost_host_refusal(policy: &GhostPolicy, host: &crate::host::HostId) -> String {
    let allowed = if policy.hosts.is_empty() {
        "the runbook grants every host (its `hosts:` list is empty, which is \
         unrestricted), so this refusal should not have happened"
            .to_string()
    } else {
        format!("{} ({})", policy.hosts.join(", "), policy.hosts.len())
    };
    format!(
        "Error: this runbook's `hosts:` list does not grant host {}. \
         Hosts this runbook allows: {}. \
         Stay on the hosts the runbook names, or work on the daemon host.",
        host.as_str(),
        allowed
    )
}

/// The arm's host prologue, as one call: `None` when the policy allows the
/// host, otherwise the `ghost_host_refused` event is written (the four
/// fields the event always carries: the session, the tool, the requested
/// host, the allowed set) and the refusal sentence is returned for the arm
/// to hand back as the tool result. The arm keeps its own frames — a
/// foreground call is approval-gated and sends its `ToolResult`; the silent
/// tools report through `ToolFinished` and send none.
pub(crate) fn refuse_ghost_host(
    policy: &GhostPolicy,
    host: &crate::host::HostId,
    tool: &str,
    session_id: Option<&str>,
) -> Option<String> {
    if policy.may_use_host(host) {
        return None;
    }
    crate::daemon::utils::log_event(
        "ghost_host_refused",
        serde_json::json!({
            "session": session_id.unwrap_or("-"),
            "tool": tool,
            "requested_host": host.as_str(),
            "allowed_hosts": policy.hosts,
        }),
    );
    Some(ghost_host_refusal(policy, host))
}

#[cfg(test)]
mod tests {
    use super::{ghost_host_refusal, resolve_host};

    // ── resolve_foreground_host ordering (M21 phase-06c) ────────────────────

    #[test]
    fn a_malformed_host_is_refused_without_prompting() {
        let err = resolve_host(Some("bad host!"))
            .expect_err("a name that can never be a host is refused before any prompt");
        assert!(
            err.contains("bad host!"),
            "the refusal echoes the name the model sent: {err}"
        );
        assert!(
            err.contains("[A-Za-z0-9._-]"),
            "the refusal is HostId::parse's own shape message: {err}"
        );
    }

    #[test]
    fn a_malformed_host_refusal_names_the_character_class() {
        for bad in ["bad host!", "", "web/01", "../etc"] {
            let err = resolve_host(Some(bad)).expect_err("every malformed name is refused");
            assert!(
                err.contains("A-Za-z0-9._-"),
                "the refusal must carry the accepted character class, not shell-limit or \
                 timeout advice: {err}"
            );
            assert!(
                !err.contains("per-owner shell limit") && !err.contains("timed out"),
                "the refusal is a host-shape sentence only: {err}"
            );
        }
    }

    #[test]
    fn an_unconfigured_host_is_not_refused_before_the_prompt() {
        // The whole point of the row: a well-formed name `[hosts]` does not
        // configure must pass this gate and reach the panel. (Whether it is in
        // `[hosts]` is the classification's question, not the parser's.)
        let parsed = resolve_host(Some("nope"))
            .expect("an unconfigured well-formed host is not refused here");
        assert_eq!(
            parsed,
            crate::host::HostId::Named("nope".to_string()),
            "the parsed id is carried on, so the name is parsed once"
        );
    }

    #[test]
    fn an_omitted_host_resolves_to_local_before_the_prompt() {
        let parsed = resolve_host(None).expect("no host argument is the daemon host");
        assert_eq!(parsed, crate::host::HostId::Local);
    }

    use crate::daemon::policy::GhostPolicy;

    fn ghost_with_hosts(hosts: &[&str]) -> GhostPolicy {
        GhostPolicy {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        }
    }

    /// The refusal sentence names the host the ghost asked for **and** the
    /// hosts the runbook allows — the model needs both to correct itself
    /// instead of retrying blindly.
    #[test]
    fn a_ghost_naming_an_unlisted_host_is_refused_with_both_lists() {
        let policy = ghost_with_hosts(&["pinky"]);
        let host = resolve_host(Some("web01")).expect("web01 is well-formed");
        let msg = ghost_host_refusal(&policy, &host);
        assert!(
            msg.contains("web01"),
            "the refusal must name the requested host: {msg}"
        );
        assert!(
            msg.contains("pinky"),
            "the refusal must name the allowed host(s): {msg}"
        );
        assert!(
            msg.starts_with("Error:"),
            "a refusal is error text the model reads as a failed call: {msg}"
        );
    }

    /// A chat session has no ghost policy at all, so the gate must be a
    /// no-op for it — both the Foreground and OpenShell doors consult the
    /// policy only when one exists.
    #[test]
    fn a_non_ghost_session_is_not_gated_by_hosts() {
        // `execute_tool_call` derives `ghost_policy` from the session entry:
        // a non-ghost session yields `None`, and every gate below is behind
        // `if let Some(policy) = ghost_policy.as_ref()`. That condition,
        // evaluated at the seam the dispatcher consults, is what "not gated"
        // means here: with no policy the gate expression is false for every
        // host, so a chat session reaches any host untouched.
        let policy: Option<&GhostPolicy> = None;
        let gate_fires = policy
            .is_some_and(|p| !p.may_use_host(&crate::host::HostId::Named("any-host".to_string())));
        assert!(
            !gate_fires,
            "no policy means no gate: a chat session reaches any host"
        );
        // Even a ghost whose runbook grants nothing named is unrestricted.
        let unrestricted = ghost_with_hosts(&[]);
        assert!(
            unrestricted.may_use_host(&crate::host::HostId::Named("any-host".to_string())),
            "an empty hosts list stays unrestricted for the gate too"
        );
    }

    /// The common case: a ghost working on the daemon host is untouched by
    /// the boundary, however restrictive its `hosts:` list is.
    #[test]
    fn a_ghost_may_still_use_the_daemon_host() {
        let policy = ghost_with_hosts(&["pinky"]);
        let local = resolve_host(None).expect("an omitted host is local");
        assert_eq!(local, crate::host::HostId::Local);
        assert!(
            policy.may_use_host(&local),
            "the daemon host is never a hosts: question: a ghost running local \
             commands is the default ghost workflow"
        );
    }

    /// The "recorded" half: the refusal reaches the event log with the
    /// requested host in its fields. `log_event` is fire-and-forget (it
    /// swallows write errors by design), so the test drives the same
    /// helper the gate calls and asserts the segment file gained a record.
    #[test]
    fn an_unlisted_host_refusal_is_recorded() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        crate::config::Config::ensure_dirs().ok();

        let policy = ghost_with_hosts(&["pinky"]);
        let host = resolve_host(Some("web01")).expect("web01 is well-formed");
        let msg = ghost_host_refusal(&policy, &host);
        crate::daemon::utils::log_event(
            "ghost_host_refused",
            serde_json::json!({
                "session": "ghost-test",
                "tool": "run_terminal_command",
                "requested_host": host.as_str(),
                "allowed_hosts": policy.hosts,
            }),
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("the event segment was written");
        let found = content.lines().any(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return false;
            };
            v.get("event").and_then(|e| e.as_str()) == Some("ghost_host_refused")
                && v.get("requested_host").and_then(|h| h.as_str()) == Some("web01")
                && v.get("allowed_hosts").is_some()
        });
        assert!(
            found,
            "the refusal must be recorded naming the requested host and the              allowed list — segment:\n{content}"
        );
        assert!(
            msg.contains("web01"),
            "the refusal text is the model-visible half: {msg}"
        );

        match saved_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
