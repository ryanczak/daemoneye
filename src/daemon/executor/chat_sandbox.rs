//! A chat session's `wait: false` commands run in that session's own sbx
//! sandbox: created on first use, and removed by the reaper once no shell
//! record names it and it has gone unused for the grace period.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::shell::{SbxRunner, ShellRegistry};
use crate::util::UnpoisonExt;

/// How long a chat sandbox stays after its last background command, so
/// back-to-back commands reuse one sandbox instead of paying a create each
/// time. The reaper removes it after this much disuse.
pub const CHAT_SANDBOX_IDLE_GRACE_SECS: u64 = 600;

/// When each chat sandbox was last handed out, in unix seconds.
static LAST_USED: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn last_used() -> &'static Mutex<HashMap<String, u64>> {
    LAST_USED.get_or_init(Default::default)
}

/// True when `[sandbox]` is enabled with runtime "sbx".
pub fn chat_sandbox_enabled(cfg: &crate::config::SandboxConfig) -> bool {
    cfg.enabled && cfg.runtime == "sbx"
}

/// Make sure the chat session's sandbox exists and stamp it as just used.
/// `Err` is an operator-facing reason, ready to hand to the model, starting
/// `sandbox unavailable:`. A failure stamps nothing, so the reaper never
/// sees a sandbox that was never created.
pub async fn ensure_chat_sandbox(
    run: &SbxRunner,
    cfg: &crate::config::SandboxConfig,
    session: &str,
    now_unix: u64,
) -> Result<String, String> {
    let name = super::ghost_sandbox::ensure_sandbox(
        run,
        cfg,
        super::sbx::SandboxKind::Chat,
        session,
        None,
    )
    .await?;
    last_used()
        .lock()
        .unwrap_or_log()
        .insert(name.clone(), now_unix);
    Ok(name)
}

/// The chat sandboxes in `last_used` that are idle: the name starts with
/// `de-chat-`, it is not in `live`, and it has been unused for at least
/// `grace_secs`. Sorted ascending.
pub fn idle_chat_sandboxes(
    last_used: &HashMap<String, u64>,
    live: &HashSet<String>,
    now_unix: u64,
    grace_secs: u64,
) -> Vec<String> {
    let mut idle: Vec<String> = last_used
        .iter()
        .filter(|(name, stamp)| {
            name.starts_with("de-chat-")
                && !live.contains(*name)
                && now_unix.saturating_sub(**stamp) >= grace_secs
        })
        .map(|(name, _)| name.clone())
        .collect();
    idle.sort();
    idle
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapedSandbox {
    pub name: String,
    pub egress: Option<super::sbx::EgressAudit>,
    pub teardown: super::sbx_lifecycle::Teardown,
}

/// One reaper pass over `map`: pick the idle names and remove them from the
/// map in one critical section, then read each sandbox's egress and tear it
/// down after the lock is released — the log is read **before** `rm`, because
/// it survives `rm` and accumulates across sandboxes that reuse a name, so a
/// post-teardown read could pick up the next sandbox's entries. Returns one
/// [`ReapedSandbox`] per teardown, in sorted order.
pub async fn reap_idle_in(
    map: &Mutex<HashMap<String, u64>>,
    run: &SbxRunner,
    live: &HashSet<String>,
    now_unix: u64,
) -> Vec<ReapedSandbox> {
    let idle = {
        let mut stamps = map.lock().unwrap_or_log();
        let idle = idle_chat_sandboxes(&stamps, live, now_unix, CHAT_SANDBOX_IDLE_GRACE_SECS);
        for name in &idle {
            stamps.remove(name);
        }
        idle
    };
    let mut reaped = Vec::with_capacity(idle.len());
    for name in idle {
        let egress = super::sbx_lifecycle::read_egress(run, &name).await;
        let teardown = super::ghost_sandbox::forget_and_teardown(run, &name).await;
        reaped.push(ReapedSandbox {
            name,
            egress,
            teardown,
        });
    }
    reaped
}

/// The production form: `live` is every `record.sandbox` in `reg`, then one
/// pass over the process-wide stamp map. Registry and map locks are never
/// held together: the sandbox set is collected first.
pub async fn reap_idle_chat_sandboxes(
    run: &SbxRunner,
    reg: &ShellRegistry,
    now_unix: u64,
) -> Vec<ReapedSandbox> {
    let live: HashSet<String> = crate::shell::with_shells(reg, |store| {
        store
            .values()
            .filter_map(|record| record.sandbox.clone())
            .collect()
    });
    reap_idle_in(last_used(), run, &live, now_unix).await
}

#[cfg(test)]
mod tests {
    use super::super::ghost_sandbox::test_support::{
        CreatedSet, fake, ok_run, resolved_policy, run,
    };
    use super::*;
    use crate::daemon::executor::sbx::{self, NetworkPlan, SandboxKind};
    use crate::daemon::executor::sbx_lifecycle::Teardown;

    #[test]
    fn chat_sandbox_enabled_only_for_enabled_sbx() {
        let enabled_sbx = crate::config::SandboxConfig {
            enabled: true,
            runtime: "sbx".to_string(),
            ..Default::default()
        };
        assert!(
            chat_sandbox_enabled(&enabled_sbx),
            "enabled + sbx is the gate for the sandboxed chat background path"
        );

        let enabled_docker = crate::config::SandboxConfig {
            enabled: true,
            runtime: "docker".to_string(),
            ..Default::default()
        };
        assert!(
            !chat_sandbox_enabled(&enabled_docker),
            "a docker runtime has no chat sandbox path"
        );

        let disabled_sbx = crate::config::SandboxConfig {
            enabled: false,
            runtime: "sbx".to_string(),
            ..Default::default()
        };
        assert!(
            !chat_sandbox_enabled(&disabled_sbx),
            "sandbox disabled means every command stays on the host"
        );
    }

    #[test]
    fn idle_chat_sandboxes_skips_referenced_recent_and_non_chat() {
        let map: HashMap<String, u64> = [
            ("de-chat-a".to_string(), 1000),
            ("de-chat-b".to_string(), 1000),
            ("de-chat-c".to_string(), 9500),
            ("de-chat-d".to_string(), 9400),
            ("de-ghost-x".to_string(), 1000),
        ]
        .into_iter()
        .collect();
        let live: HashSet<String> = ["de-chat-b".to_string()].into_iter().collect();

        let idle = idle_chat_sandboxes(&map, &live, 10000, 600);

        assert_eq!(
            idle,
            vec!["de-chat-a".to_string(), "de-chat-d".to_string()],
            "d is idle at exactly the grace, c is too recent, b is referenced, \
             and x is not a chat sandbox"
        );
    }

    #[tokio::test]
    async fn ensure_chat_sandbox_stamps_last_used() {
        let session = "t-chat-stamp";
        let name = sbx::sandbox_name(SandboxKind::Chat, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().expect("canned").push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned
            .lock()
            .expect("canned")
            .push(ok_run("Status: running\n"));
        let cfg = crate::config::SandboxConfig::default();
        assert!(!chat_sandbox_enabled(&cfg));

        let got = ensure_chat_sandbox(&runner, &cfg, session, 4_000_000_000).await;

        assert_eq!(
            got,
            Ok(name.clone()),
            "the chat sandbox is created behind the same fail-closed preflight"
        );
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![
                sbx::version_args(),
                sbx::daemon_status_args(),
                sbx::create_args(&name, &NetworkPlan::None, "1g", 2.0),
            ],
            "preflight's two probes, then the create with the default network-none plan"
        );
        assert_eq!(
            last_used().lock().expect("last_used").get(&name).copied(),
            Some(4_000_000_000),
            "the stamp is what the reaper's idle test reads"
        );

        last_used().lock().expect("last_used").remove(&name);
        CreatedSet::from_name(&name).forget();
    }

    #[tokio::test]
    async fn ensure_chat_sandbox_failure_stamps_nothing() {
        let session = "t-chat-fail";
        let name = sbx::sandbox_name(SandboxKind::Chat, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned
            .lock()
            .expect("canned")
            .push(run(None, "", "spawning sbx: No such file"));
        let cfg = crate::config::SandboxConfig::default();

        let got = ensure_chat_sandbox(&runner, &cfg, session, 4_000_000_001).await;

        let msg = got.expect_err("a failed preflight must fail closed");
        assert!(
            msg.starts_with("sandbox unavailable:"),
            "the reason is operator-facing: {msg}"
        );
        assert!(
            msg.contains("spawning sbx: No such file"),
            "the reason carries the detail: {msg}"
        );
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![sbx::version_args(), sbx::daemon_status_args()],
            "only the two preflight probes ran; no create followed a failed preflight"
        );
        assert!(
            last_used().lock().expect("last_used").get(&name).is_none(),
            "a sandbox that was never created must never be stamped, so the \
             reaper can never see it"
        );
    }

    #[tokio::test]
    async fn reap_idle_in_tears_down_only_idle_sandboxes() {
        let map = Mutex::new(HashMap::from([
            ("de-chat-t-old".to_string(), 1000),
            ("de-chat-t-new".to_string(), 9900),
        ]));
        let live = HashSet::new();
        let (runner, calls, _canned) = fake();

        let got = reap_idle_in(&map, &runner, &live, 10000).await;

        assert_eq!(
            got,
            vec![ReapedSandbox {
                name: "de-chat-t-old".into(),
                egress: None,
                teardown: Teardown::Removed,
            }],
            "only the idle sandbox is torn down, and a drained queue answers rm 0; \
             the fake answers the policy-log call with empty stdout, so egress is None"
        );
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![
                sbx::policy_log_args("de-chat-t-old"),
                sbx::rm_args(&["de-chat-t-old".to_string()]),
            ],
            "the log is read before the removal, and the reaper removes, it does not stop"
        );
        let stamps = map.lock().expect("map lock");
        assert!(
            stamps.contains_key("de-chat-t-new"),
            "the recent sandbox keeps its stamp"
        );
        assert!(
            !stamps.contains_key("de-chat-t-old"),
            "the torn-down sandbox's stamp is gone with it"
        );
    }

    #[tokio::test]
    async fn ensure_chat_sandbox_adopts_the_profile_deny_default() {
        let cfg = crate::config::SandboxConfig::default();
        assert_eq!(
            super::super::sbx_lifecycle::profile_for(&cfg, None).network,
            "none",
            "a chat sandbox acquires no egress: profile_for's default is network none"
        );
        assert!(resolved_policy().ssh_target.is_some());
    }
}
