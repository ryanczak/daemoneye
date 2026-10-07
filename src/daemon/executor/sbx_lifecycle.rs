//! The functions that **run** `sbx` to manage a sandbox's life: preflight
//! checks that `sbx` is usable, create one sandbox from a profile's network
//! plan and limits, tear it down (falling back to `stop` when the locked
//! keychain breaks `rm`), and sweep daemon-owned sandboxes no live record
//! holds. Every `sbx` invocation goes through the injectable
//! [`SbxRunner`](crate::shell::SbxRunner), so tests drive a fake runner.

use crate::daemon::executor::sbx::{self, NetworkPlan, SandboxKind, SbxUnavailable};
use crate::shell::{SbxRunner, VmRun};

/// The profile for `name`, or the default (network "none") when `name` is
/// `None` or is not configured. A job that cannot be matched to a profile
/// must not acquire egress.
pub fn profile_for(
    cfg: &crate::config::SandboxConfig,
    name: Option<&str>,
) -> crate::config::SandboxProfile {
    name.and_then(|n| cfg.profile.get(n))
        .cloned()
        .unwrap_or_default()
}

/// Is `sbx` usable here? Runs the two probes in order — `sbx version`, then
/// `sbx daemon status` — and answers with the sbx version string, or the
/// reason it is unusable.
pub async fn preflight(run: &SbxRunner) -> Result<String, SbxUnavailable> {
    let version = run(sbx::version_args()).await;
    let daemon = run(sbx::daemon_status_args()).await;
    for run in [&version, &daemon] {
        if run.code.is_none() {
            return Err(SbxUnavailable::SpawnFailed {
                detail: run.stderr.trim().to_string(),
            });
        }
    }
    sbx::classify_preflight(&probe(&version), &probe(&daemon))
}

/// The shape [`sbx::classify_preflight`] reads, from one run.
fn probe(run: &VmRun) -> sbx::Probe {
    sbx::Probe {
        spawn_error: None,
        exit_ok: run.code == Some(0),
        stdout: run.stdout.clone(),
        stderr: run.stderr.clone(),
    }
}

/// Create one sandbox for `id` of `kind`, with the named profile's network
/// plan and the configured limits. The returned name is the only handle the
/// caller needs.
///
/// A half-configured sandbox is never handed back: if a per-host allow fails
/// after create, the sandbox is torn down before the error returns.
pub async fn create_sandbox(
    run: &SbxRunner,
    cfg: &crate::config::SandboxConfig,
    profile: Option<&str>,
    kind: SandboxKind,
    id: &str,
) -> anyhow::Result<String> {
    let profile = profile_for(cfg, profile);
    let plan = sbx::network_plan(&profile).map_err(anyhow::Error::msg)?;
    let name = sbx::sandbox_name(kind, id);
    let argv = sbx::create_args(&name, &plan, &cfg.limits.memory, cfg.limits.cpus);
    let created = run(argv).await;
    if created.code != Some(0) {
        return Err(anyhow::anyhow!(
            "creating sandbox {name}: {}",
            created.stderr.trim()
        ));
    }
    if let NetworkPlan::Proxy { allow, .. } = &plan {
        for host in allow {
            let allowed = run(sbx::policy_allow_args(&name, host)).await;
            if allowed.code != Some(0) {
                let _ = teardown_sandbox(run, &name).await;
                return Err(anyhow::anyhow!(
                    "allowing {host} egress in sandbox {name}: {}",
                    allowed.stderr.trim()
                ));
            }
        }
    }
    Ok(name)
}

/// The outcome of removing one sandbox. `Removed` covers the measured
/// already-gone case (`rm` reporting `not found`); `StoppedOnly` is the
/// locked-keychain case — the VM is stopped and removal is left to the next
/// sweep; `Failed` carries both commands' details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Teardown {
    Removed,
    StoppedOnly { rm_error: String },
    Failed { error: String },
}

/// Remove one sandbox, falling back to `stop` when `rm -f` fails. The
/// measured locked-keychain case: `rm` fails while `stop` succeeds, so the
/// VM is stopped now and removal is left to the next sweep.
pub async fn teardown_sandbox(run: &SbxRunner, name: &str) -> Teardown {
    let removed = run(sbx::rm_args(&[name.to_string()])).await;
    if removed.code == Some(0) {
        return Teardown::Removed;
    }
    let rm_error = removed.stderr.trim().to_string();
    if rm_error.contains("not found") {
        return Teardown::Removed;
    }
    let stopped = run(sbx::stop_args(&[name.to_string()])).await;
    match stopped.code {
        Some(0) => Teardown::StoppedOnly { rm_error },
        _ => Teardown::Failed {
            error: format!("rm: {rm_error} ; stop: {}", stopped.stderr.trim()),
        },
    }
}

/// What one sweep did. `failed` carries `(name, detail)` pairs; `list_error`
/// set means nothing was attempted because the listing itself failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub removed: Vec<String>,
    pub stopped: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub list_error: Option<String>,
}

/// Tear down every daemon sandbox no name in `live` owns. The targets come
/// from [`sbx::stale_daemon_sandboxes`], so a foreign sandbox — anything the
/// daemon did not name — is never touched, whatever its status.
pub async fn sweep_daemon_sandboxes(run: &SbxRunner, live: &[String]) -> SweepReport {
    let listed = run(sbx::ls_args()).await;
    if listed.code != Some(0) {
        return SweepReport {
            list_error: Some(listed.stderr.trim().to_string()),
            ..Default::default()
        };
    }
    let Some(sandboxes) = sbx::parse_ls_json(&listed.stdout) else {
        return SweepReport {
            list_error: Some("unrecognised sbx ls output".to_string()),
            ..Default::default()
        };
    };
    let mut report = SweepReport::default();
    for name in sbx::stale_daemon_sandboxes(&sandboxes, live) {
        match teardown_sandbox(run, &name).await {
            Teardown::Removed => report.removed.push(name),
            Teardown::StoppedOnly { .. } => report.stopped.push(name),
            Teardown::Failed { error } => report.failed.push((name, error)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::sync::Arc;
    use std::sync::Mutex;

    type CannedRuns = Arc<Mutex<Vec<VmRun>>>;

    fn fake_runner(canned: CannedRuns, calls: Arc<Mutex<Vec<Vec<String>>>>) -> SbxRunner {
        Arc::new(move |argv: Vec<String>| {
            calls.lock().unwrap().push(argv.clone());
            let mut queue = canned.lock().unwrap();
            let run = if queue.is_empty() {
                queue.last().cloned().unwrap_or_else(|| VmRun {
                    code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                queue.remove(0)
            };
            async move { run }.boxed()
        })
    }

    /// A runner plus the handles a test reads: every argv in order, and the
    /// FIFO queue of runs the fake hands back (a drained queue answers
    /// `Some(0)` with empty output, so late teardowns succeed by default).
    fn fake() -> (SbxRunner, Arc<Mutex<Vec<Vec<String>>>>, CannedRuns) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let canned: CannedRuns = Arc::new(Mutex::new(Vec::new()));
        let runner = fake_runner(Arc::clone(&canned), Arc::clone(&calls));
        (runner, calls, canned)
    }

    fn run(code: Option<i32>, stdout: &str, stderr: &str) -> VmRun {
        VmRun {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn ok_run(stdout: &str) -> VmRun {
        run(Some(0), stdout, "")
    }

    fn default_config() -> crate::config::SandboxConfig {
        crate::config::SandboxConfig::default()
    }

    fn proxy_profile(allow: &[&str]) -> crate::config::SandboxProfile {
        crate::config::SandboxProfile {
            network: "proxy".to_string(),
            proxy_allow: allow.iter().map(|s| s.to_string()).collect(),
            proxy_deny: Vec::new(),
        }
    }

    fn profiled_config(
        profile: &str,
        p: crate::config::SandboxProfile,
    ) -> crate::config::SandboxConfig {
        let mut cfg = default_config();
        cfg.profile.insert(profile.to_string(), p);
        cfg
    }

    #[test]
    fn profile_for_defaults_to_no_egress() {
        let cfg = default_config();
        assert_eq!(profile_for(&cfg, None).network, "none");
        assert!(profile_for(&cfg, None).proxy_allow.is_empty());
        assert_eq!(
            profile_for(&cfg, Some("researcher")).network,
            "none",
            "an unconfigured name must not acquire egress"
        );

        let cfg = profiled_config("researcher", proxy_profile(&["a.example"]));
        let p = profile_for(&cfg, Some("researcher"));
        assert_eq!(p.network, "proxy");
        assert_eq!(p.proxy_allow, vec!["a.example".to_string()]);
    }

    #[tokio::test]
    async fn preflight_accepts_the_measured_outputs() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));

        let got = preflight(&runner).await;

        assert_eq!(got, Ok("v0.46.0".to_string()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![sbx::version_args(), sbx::daemon_status_args()],
            "preflight must run the version probe, then the daemon probe"
        );
    }

    #[tokio::test]
    async fn preflight_reports_a_spawn_failure() {
        let (runner, _calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned
            .lock()
            .unwrap()
            .push(run(None, "", "spawning sbx: No such file"));

        let got = preflight(&runner).await;

        assert_eq!(
            got,
            Err(SbxUnavailable::SpawnFailed {
                detail: "spawning sbx: No such file".to_string()
            })
        );
    }

    #[tokio::test]
    async fn preflight_reports_a_daemon_spawn_failure() {
        let (runner, _calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(run(None, "", "timed out"));

        let got = preflight(&runner).await;

        assert_eq!(
            got,
            Err(SbxUnavailable::SpawnFailed {
                detail: "timed out".to_string()
            }),
            "the daemon probe's spawn failure must surface too"
        );
    }

    #[tokio::test]
    async fn preflight_reports_a_locked_keychain() {
        let (runner, _calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(run(
            Some(0),
            "Status: running\n",
            "WARN: failed to list stored credentials … the OS keychain is locked\n",
        ));

        let got = preflight(&runner).await;

        assert_eq!(got, Err(SbxUnavailable::KeychainLocked));
    }

    #[tokio::test]
    async fn create_sandbox_runs_create_with_limits_and_no_egress() {
        let (runner, calls, _canned) = fake();
        let cfg = default_config();

        let got = create_sandbox(&runner, &cfg, None, SandboxKind::Ghost, "r1")
            .await
            .expect("the default profile is network none");

        assert_eq!(got, "de-ghost-r1");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![sbx::create_args(
                "de-ghost-r1",
                &NetworkPlan::None,
                "1g",
                2.0
            )],
            "a network-none profile needs no follow-up policy calls"
        );
    }

    #[tokio::test]
    async fn create_sandbox_applies_each_allow_after_create() {
        let (runner, calls, _canned) = fake();
        let cfg = profiled_config("researcher", proxy_profile(&["a.example", "b.example"]));

        let got = create_sandbox(&runner, &cfg, Some("researcher"), SandboxKind::Ghost, "r1")
            .await
            .expect("both allows succeed");

        assert_eq!(got, "de-ghost-r1");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                sbx::create_args(
                    "de-ghost-r1",
                    &NetworkPlan::Proxy {
                        allow: vec!["a.example".to_string(), "b.example".to_string()],
                        deny: Vec::new(),
                    },
                    "1g",
                    2.0
                ),
                sbx::policy_allow_args("de-ghost-r1", "a.example"),
                sbx::policy_allow_args("de-ghost-r1", "b.example"),
            ],
            "create first, then each allow in the profile's order"
        );
    }

    #[tokio::test]
    async fn create_sandbox_tears_down_when_an_allow_fails() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run("")); // create succeeds
        canned.lock().unwrap().push(run(Some(1), "", "boom")); // the first allow fails
        let cfg = profiled_config("researcher", proxy_profile(&["a.example", "b.example"]));

        let got = create_sandbox(&runner, &cfg, Some("researcher"), SandboxKind::Ghost, "r1").await;

        let err = got.expect_err("a failed allow must not hand back the sandbox");
        let msg = err.to_string();
        assert!(
            msg.contains("a.example"),
            "the error must name the host that failed: {msg}"
        );
        assert!(
            msg.contains("boom"),
            "the error must carry sbx's detail: {msg}"
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3, "create, the failed allow, then the rm");
        assert_eq!(
            calls[2],
            sbx::rm_args(&["de-ghost-r1".to_string()]),
            "a half-configured sandbox must be torn down, never handed back"
        );
    }

    #[tokio::test]
    async fn create_sandbox_rejects_an_invalid_profile_without_running_sbx() {
        let (runner, calls, _canned) = fake();
        let cfg = profiled_config(
            "researcher",
            crate::config::SandboxProfile {
                network: "bogus".to_string(),
                ..Default::default()
            },
        );

        let got = create_sandbox(&runner, &cfg, Some("researcher"), SandboxKind::Ghost, "r1").await;

        assert!(got.is_err(), "an unknown network mode must be an Err");
        assert!(
            calls.lock().unwrap().is_empty(),
            "a rejected profile must never reach sbx"
        );
    }

    #[tokio::test]
    async fn create_sandbox_reports_a_failed_create() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(run(Some(1), "", "boom"));

        let got = create_sandbox(&runner, &default_config(), None, SandboxKind::Ghost, "r1").await;

        let err = got.expect_err("a failed create must be an Err");
        assert!(
            err.to_string().contains("boom"),
            "the error must carry sbx's stderr: {err}"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "a failed create must not run any follow-up call"
        );
    }

    #[tokio::test]
    async fn teardown_removes_or_treats_not_found_as_removed() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(""));

        assert_eq!(
            teardown_sandbox(&runner, "de-ghost-r1").await,
            Teardown::Removed
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![sbx::rm_args(&["de-ghost-r1".to_string()])],
            "a clean rm needs no stop"
        );

        calls.lock().unwrap().clear();
        canned
            .lock()
            .unwrap()
            .push(run(Some(1), "", "error: sandbox 'de-ghost-r1' not found"));
        assert_eq!(
            teardown_sandbox(&runner, "de-ghost-r1").await,
            Teardown::Removed,
            "already gone is removed"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "a not-found rm must not fall back to stop"
        );
    }

    #[tokio::test]
    async fn teardown_falls_back_to_stop_when_rm_fails() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(run(
            Some(1),
            "",
            "error: rm de-ghost-r1: snapshot associated scoped secrets: … the OS keychain is locked …",
        ));
        canned.lock().unwrap().push(ok_run(""));

        let got = teardown_sandbox(&runner, "de-ghost-r1").await;

        assert_eq!(
            got,
            Teardown::StoppedOnly {
                rm_error: "error: rm de-ghost-r1: snapshot associated scoped secrets: … the OS keychain is locked …".to_string()
            },
            "the locked keychain stops the VM; removal is the next sweep's job"
        );
        assert!(
            matches!(&got, Teardown::StoppedOnly { rm_error } if rm_error.contains("keychain")),
            "the rm detail is carried for the operator: {got:?}"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                sbx::rm_args(&["de-ghost-r1".to_string()]),
                sbx::stop_args(&["de-ghost-r1".to_string()]),
            ],
            "the stop fallback must actually run"
        );
    }

    #[tokio::test]
    async fn teardown_reports_when_rm_and_stop_both_fail() {
        let (runner, _calls, canned) = fake();
        canned.lock().unwrap().push(run(Some(1), "", "rm-boom"));
        canned.lock().unwrap().push(run(Some(1), "", "stop-boom"));

        let got = teardown_sandbox(&runner, "de-ghost-r1").await;

        assert_eq!(
            got,
            Teardown::Failed {
                error: "rm: rm-boom ; stop: stop-boom".to_string()
            }
        );
    }

    #[tokio::test]
    async fn sweep_removes_only_stale_daemon_sandboxes() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            r#"{"sandboxes":[{"name":"de-ghost-a","status":"running"},{"name":"de-ghost-b","status":"running"},{"name":"de-probe-a","status":"running"},{"name":"claude-x","status":"stopped"}]}"#,
        ));

        let report = sweep_daemon_sandboxes(&runner, &["de-ghost-b".to_string()]).await;

        assert_eq!(report.removed, vec!["de-ghost-a".to_string()]);
        assert!(report.stopped.is_empty());
        assert!(report.failed.is_empty());
        assert_eq!(report.list_error, None);
        for argv in calls.lock().unwrap().iter() {
            for name in ["de-probe-a", "claude-x", "de-ghost-b"] {
                assert!(
                    !argv.iter().any(|a| a.contains(name)),
                    "the sweep must never touch {name}: argv {argv:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn sweep_reports_an_unreadable_listing() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(run(Some(1), "", "ls-boom"));

        let report = sweep_daemon_sandboxes(&runner, &[]).await;

        assert_eq!(report.list_error, Some("ls-boom".to_string()));
        assert!(report.removed.is_empty() && report.stopped.is_empty() && report.failed.is_empty());
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "a failed listing must not be followed by any teardown call"
        );
    }

    #[tokio::test]
    async fn sweep_reports_an_unparsable_listing() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run("No sandboxes found."));

        let report = sweep_daemon_sandboxes(&runner, &[]).await;

        assert_eq!(
            report.list_error,
            Some("unrecognised sbx ls output".to_string())
        );
        assert!(report.removed.is_empty() && report.stopped.is_empty() && report.failed.is_empty());
        assert_eq!(calls.lock().unwrap().len(), 1, "no teardown call follows");
    }

    #[tokio::test]
    async fn sweep_reports_a_stopped_only_teardown_and_a_failure() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            r#"{"sandboxes":[{"name":"de-ghost-a","status":"running"},{"name":"de-ghost-b","status":"running"}]}"#,
        ));
        canned
            .lock()
            .unwrap()
            .push(run(Some(1), "", "rm: keychain locked"));
        canned.lock().unwrap().push(ok_run("")); // stop of a
        canned.lock().unwrap().push(run(Some(1), "", "nope")); // rm of b
        canned.lock().unwrap().push(run(Some(1), "", "nope")); // stop of b

        let report = sweep_daemon_sandboxes(&runner, &[]).await;

        assert_eq!(report.removed, Vec::<String>::new());
        assert_eq!(report.stopped, vec!["de-ghost-a".to_string()]);
        assert_eq!(
            report.failed,
            vec![(
                "de-ghost-b".to_string(),
                "rm: nope ; stop: nope".to_string()
            )]
        );
        let stopped_only = calls.lock().unwrap();
        assert_eq!(stopped_only[0], sbx::ls_args(), "the sweep must list first");
        assert_eq!(
            stopped_only[2],
            sbx::stop_args(&["de-ghost-a".to_string()]),
            "the locked-keychain sandbox gets the stop fallback"
        );
    }
}
