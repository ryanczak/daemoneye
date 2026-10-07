//! A local ghost's commands run in **its own** sbx sandbox: created on the
//! ghost's first command behind a fail-closed preflight, reached through a
//! sandbox shell opened in that sandbox, and torn down when the ghost exits.
//! The created-sandbox set is process-wide, so one ghost run creates at most
//! one sandbox however many turns and commands it has.

use crate::daemon::executor::sbx::{
    SandboxKind, script_name_is_safe, stage_cp_args, stage_prep_args,
};
use crate::shell::{Owner, SbxRunner, ShellRegistry};
use crate::util::UnpoisonExt;

static CREATED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn created() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    CREATED.get_or_init(Default::default)
}

/// True for a ghost on the PTY backend whose command runs locally (no
/// `ssh_target`) while the sandbox is enabled with runtime "sbx".
pub fn ghost_sandbox_pty(
    is_ghost: bool,
    uses_pty: bool,
    sandbox: &crate::config::SandboxConfig,
    policy: Option<&crate::daemon::policy::GhostPolicy>,
) -> bool {
    is_ghost
        && uses_pty
        && sandbox.enabled
        && sandbox.runtime == "sbx"
        && policy.is_some_and(|p| p.ssh_target.is_none())
}

/// Make sure the sandbox of `kind` for `id` exists, and return its name. The
/// first call runs preflight and creates the sandbox; later calls in the same
/// daemon return at once. `Err` is an operator-facing reason, ready to hand
/// to the model, starting `sandbox unavailable:`.
pub async fn ensure_sandbox(
    run: &SbxRunner,
    cfg: &crate::config::SandboxConfig,
    kind: SandboxKind,
    id: &str,
    profile: Option<&str>,
) -> Result<String, String> {
    let name = crate::daemon::executor::sbx::sandbox_name(kind, id);
    if created().lock().unwrap_or_log().contains(&name) {
        return Ok(name);
    }
    if let Err(r) = crate::daemon::executor::sbx_lifecycle::preflight(run).await {
        return Err(crate::daemon::executor::sbx::describe_unavailable(&r));
    }
    match crate::daemon::executor::sbx_lifecycle::create_sandbox(run, cfg, profile, kind, id).await
    {
        Ok(_) => {
            created().lock().unwrap_or_log().insert(name.clone());
            Ok(name)
        }
        Err(e) => {
            let detail = format!("{e:#}");
            if detail.contains("already exists") {
                created().lock().unwrap_or_log().insert(name.clone());
                return Ok(name);
            }
            Err(format!("sandbox unavailable: {detail}"))
        }
    }
}

/// Make sure the ghost run `session` has its sandbox; returns the sandbox
/// name. The first call runs preflight and creates the sandbox; later calls
/// in the same daemon return at once. `Err` is an operator-facing reason,
/// ready to hand to the model, starting `sandbox unavailable:`.
pub async fn ensure_ghost_sandbox(
    run: &SbxRunner,
    cfg: &crate::config::SandboxConfig,
    session: &str,
    profile: Option<&str>,
) -> Result<String, String> {
    ensure_sandbox(run, cfg, SandboxKind::Ghost, session, profile).await
}

/// Remove `name` from the created set, then tear the sandbox down. The
/// single place that forgets a sandbox, so a name the daemon stops tracking
/// is the same name it stops running.
pub async fn forget_and_teardown(
    run: &SbxRunner,
    name: &str,
) -> crate::daemon::executor::sbx_lifecycle::Teardown {
    created().lock().unwrap_or_log().remove(name);
    crate::daemon::executor::sbx_lifecycle::teardown_sandbox(run, name).await
}

/// Tear the ghost run's sandbox down and forget it. A ghost that never ran a
/// sandboxed command gets `rm` of a missing sandbox, which
/// [`sbx_lifecycle::Teardown::Removed`] covers, so this is harmless for it.
pub async fn teardown_ghost_sandbox(
    run: &SbxRunner,
    session: &str,
) -> crate::daemon::executor::sbx_lifecycle::Teardown {
    let name = crate::daemon::executor::sbx::sandbox_name(SandboxKind::Ghost, session);
    forget_and_teardown(run, &name).await
}

/// `run_ghost_remote`, in this ghost's sandbox shell.
pub async fn run_ghost_in_sandbox<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    sandbox: &str,
    cmd: &str,
    call_id: &str,
    spawn: F,
) -> (String, i32)
where
    F: FnOnce(crate::shell::ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    let opened = crate::shell::shell_for_owner_in_sandbox(
        reg,
        owner.clone(),
        sandbox,
        max_per_owner,
        std::time::Duration::from_secs(15),
        spawn,
    )
    .await;
    let (id, _socket) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            return (
                format!("Error: could not open a shell in sandbox {sandbox}: {e:#}"),
                1,
            );
        }
    };
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::BufReader::new(tokio::io::empty());
    let outcome = super::shell_exec::run_in_shell_with_prompts(
        reg,
        owner,
        cmd,
        max_per_owner,
        Some(id.as_str()),
        Some(super::ghost_remote::GHOST_REMOTE_TIMEOUT_SECS),
        call_id,
        &mut sink,
        &mut eof,
        crate::host::HostId::Local,
        |_id| async { Ok(()) },
    )
    .await;
    match outcome {
        Ok((_, result)) => (result.text, result.exit_code),
        Err(e) => (super::shell_run_error_text(id.as_str(), &e), 1),
    }
}

/// Copy the approved script `name` into `sandbox` at [`sbx::STAGED_SCRIPTS_DIR`],
/// every time it is invoked: the agent can overwrite its copy, so the
/// operator's version is re-staged per command. Fails closed: `Err` is an
/// operator-facing reason, and the caller must not run the command.
pub async fn stage_script(
    run: &crate::shell::SbxRunner,
    sandbox: &str,
    name: &str,
    host_path: &str,
) -> Result<(), String> {
    if !script_name_is_safe(name) {
        return Err(format!(
            "sandbox staging failed for `{name}`: `{name}` is not a stageable script name"
        ));
    }
    for args in [
        stage_prep_args(sandbox),
        stage_cp_args(host_path, sandbox, name),
    ] {
        let out = run(args).await;
        if out.code != Some(0) {
            return Err(format!(
                "sandbox staging failed for `{name}`: {}",
                out.stderr.trim()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Fixtures shared by `ghost_sandbox.rs`'s and `chat_sandbox.rs`'s test
    //! modules. `chat_sandbox.rs` tests the same lifecycle against the same
    //! fake `sbx`, so both must drive one shared definition or a copied one
    //! drifts. The created set is process-wide and libtest runs tests in
    //! parallel, so every test also gets a `CreatedSet` guard naming its
    //! sandboxes.

    use super::created;
    use crate::shell::{SbxRunner, VmRun};
    use crate::util::UnpoisonExt;
    use futures_util::FutureExt;
    use std::sync::{Arc, Mutex};

    pub(crate) type CannedRuns = Arc<Mutex<Vec<VmRun>>>;

    /// The fake runner, byte-for-byte as `sbx_lifecycle.rs` tests define it:
    /// every argv is recorded, runs come off a FIFO queue and a drained
    /// queue answers `Some(0)` with empty output.
    pub(crate) fn fake_runner(
        canned: CannedRuns,
        calls: Arc<Mutex<Vec<Vec<String>>>>,
    ) -> SbxRunner {
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
    pub(crate) fn fake() -> (SbxRunner, Arc<Mutex<Vec<Vec<String>>>>, CannedRuns) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let canned: CannedRuns = Arc::new(Mutex::new(Vec::new()));
        let runner = fake_runner(Arc::clone(&canned), Arc::clone(&calls));
        (runner, calls, canned)
    }

    pub(crate) fn run(code: Option<i32>, stdout: &str, stderr: &str) -> VmRun {
        VmRun {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    pub(crate) fn ok_run(stdout: &str) -> VmRun {
        run(Some(0), stdout, "")
    }

    pub(crate) fn sbx_config(runtime: &str, enabled: bool) -> crate::config::SandboxConfig {
        crate::config::SandboxConfig {
            enabled,
            runtime: runtime.to_string(),
            ..Default::default()
        }
    }

    pub(crate) fn enabled_sbx() -> crate::config::SandboxConfig {
        sbx_config("sbx", true)
    }

    pub(crate) fn local_policy() -> crate::daemon::policy::GhostPolicy {
        crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
    }

    pub(crate) fn resolved_policy() -> crate::daemon::policy::GhostPolicy {
        crate::daemon::policy::GhostPolicy {
            ssh_target: Some("matt@db01.lan".to_string()),
            ..crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        }
    }

    /// Forget every name one test put in the process-wide created set, so a
    /// later test starts from an empty set — the set is process-wide and
    /// tests run in parallel.
    pub(crate) struct CreatedSet {
        names: Vec<String>,
    }

    impl CreatedSet {
        pub(crate) fn from_name(name: &str) -> Self {
            Self {
                names: vec![name.to_string()],
            }
        }

        pub(crate) fn forget(&self) {
            let mut set = created().lock().unwrap_or_log();
            for name in &self.names {
                set.remove(name);
            }
        }
    }

    impl Drop for CreatedSet {
        fn drop(&mut self) {
            self.forget();
        }
    }

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1).
    pub(crate) fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// A real in-process PTY host serving the configured socket for `id`: a
    /// bash session standing in for the sandbox's bash. The detached
    /// `spawn_sbx_host` cannot run under a test binary (07b F4).
    pub(crate) async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        let dir = tempfile::tempdir().expect("tempdir for cast");
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0)
            .expect("spawning bash");
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .expect("holding the per-id instance lock");
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket).await.expect("binding");
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    /// The PTY geometry every shell host is spawned with, matching the
    /// detached production spawns (24×80).
    pub(crate) const ROWS: u16 = 24;
    /// The PTY geometry every shell host is spawned with.
    pub(crate) const COLS: u16 = 80;
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        CreatedSet, enabled_sbx, fake, local_policy, ok_run, resolved_policy, run, sbx_config,
        spawn_in_process_host, test_home,
    };
    use super::*;
    use crate::daemon::executor::sbx::{self, NetworkPlan};
    use crate::daemon::executor::sbx_lifecycle::{Teardown, profile_for};
    use crate::shell::shell_for_owner_in_sandbox;

    #[test]
    fn ghost_sandbox_pty_only_for_a_local_sbx_ghost() {
        assert!(
            ghost_sandbox_pty(true, true, &enabled_sbx(), Some(&local_policy())),
            "a local ghost on the PTY backend with sbx enabled takes the branch"
        );
        assert!(
            !ghost_sandbox_pty(false, true, &enabled_sbx(), Some(&local_policy())),
            "chat never routes through this module"
        );
        assert!(
            !ghost_sandbox_pty(true, false, &enabled_sbx(), Some(&local_policy())),
            "backend = \"tmux\" keeps the tmux window"
        );
        assert!(
            !ghost_sandbox_pty(true, true, &sbx_config("sbx", false), Some(&local_policy())),
            "the sandbox off keeps the plain local PTY"
        );
        assert!(
            !ghost_sandbox_pty(
                true,
                true,
                &sbx_config("docker", true),
                Some(&local_policy())
            ),
            "runtime = \"docker\" keeps the container path"
        );
        assert!(
            !ghost_sandbox_pty(true, true, &enabled_sbx(), Some(&resolved_policy())),
            "an ssh_target ghost runs where its runbook names, not in a sandbox"
        );
        assert!(
            !ghost_sandbox_pty(true, true, &enabled_sbx(), None),
            "no policy, no sandbox route"
        );
    }

    #[tokio::test]
    async fn ensure_ghost_sandbox_creates_once() {
        let session = "t-ensure-1";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        let cfg = enabled_sbx();

        let got = ensure_ghost_sandbox(&runner, &cfg, session, None)
            .await
            .expect("the canned version and daemon runs accept preflight");

        assert_eq!(got, name);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                sbx::version_args(),
                sbx::daemon_status_args(),
                sbx::create_args(&name, &NetworkPlan::None, "1g", 2.0),
            ],
            "preflight's two probes, then the create"
        );

        calls.lock().unwrap().clear();
        let again = ensure_ghost_sandbox(&runner, &cfg, session, None)
            .await
            .expect("a second call must return the same sandbox at once");
        assert_eq!(again, name);
        assert!(
            calls.lock().unwrap().is_empty(),
            "the created set answers a second call with no sbx call"
        );
    }

    #[tokio::test]
    async fn ensure_ghost_sandbox_fails_closed_on_preflight() {
        let session = "t-ensure-2";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned
            .lock()
            .unwrap()
            .push(run(None, "", "spawning sbx: No such file"));
        let cfg = enabled_sbx();

        let got = ensure_ghost_sandbox(&runner, &cfg, session, None).await;

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
            *calls.lock().unwrap(),
            vec![sbx::version_args(), sbx::daemon_status_args()],
            "only the two preflight probes ran; no create followed a failed preflight"
        );
    }

    #[tokio::test]
    async fn ensure_ghost_sandbox_adopts_an_existing_sandbox() {
        let session = "t-ensure-3";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        canned.lock().unwrap().push(run(
            Some(1),
            "",
            "error: sandbox 'de-ghost-t-ensure-3' already exists; connect to the existing sandbox",
        ));
        let cfg = enabled_sbx();

        let got = ensure_ghost_sandbox(&runner, &cfg, session, None).await;

        assert_eq!(
            got,
            Ok(name.clone()),
            "a sandbox a previous daemon created is adopted, not an error"
        );
        assert!(
            created().lock().unwrap_or_log().contains(&name),
            "the adopted sandbox is remembered for later calls"
        );
        assert_eq!(calls.lock().unwrap().len(), 3, "version, daemon, create");
    }

    #[tokio::test]
    async fn ensure_ghost_sandbox_reports_a_failed_create() {
        let session = "t-ensure-4";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        canned.lock().unwrap().push(run(Some(1), "", "boom"));
        let cfg = enabled_sbx();

        let first = ensure_ghost_sandbox(&runner, &cfg, session, None).await;

        let msg = first.expect_err("a failed create must fail closed");
        assert!(
            msg.starts_with("sandbox unavailable:"),
            "the reason is operator-facing: {msg}"
        );
        assert!(msg.contains("boom"), "the detail is carried: {msg}");

        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        canned.lock().unwrap().push(ok_run("")); // the retried create succeeds

        let second = ensure_ghost_sandbox(&runner, &cfg, session, None)
            .await
            .expect("a later call tries again: failures are not cached");

        assert_eq!(second, name);
        assert_eq!(
            calls.lock().unwrap().len(),
            6,
            "3 for the failed attempt, 3 for the retry: preflight runs the version \
             probe and the daemon probe, so a retry is 3 calls, not 4"
        );
    }

    #[tokio::test]
    async fn ensure_ghost_sandbox_uses_the_agent_profile() {
        let session = "t-ensure-5";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        let mut cfg = enabled_sbx();
        cfg.profile.insert(
            "researcher".to_string(),
            crate::config::SandboxProfile {
                network: "proxy".to_string(),
                proxy_allow: vec!["a.example".to_string()],
                proxy_deny: Vec::new(),
            },
        );

        let got = ensure_ghost_sandbox(&runner, &cfg, session, Some("researcher"))
            .await
            .expect("the researcher profile is a valid proxy profile");

        assert_eq!(got, name);
        let calls = calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|argv| *argv == sbx::policy_allow_args(&name, "a.example")),
            "the profile's allow must be applied to the ghost's sandbox: {calls:?}"
        );
        assert_eq!(
            *calls,
            vec![
                sbx::version_args(),
                sbx::daemon_status_args(),
                sbx::create_args(
                    &name,
                    &NetworkPlan::Proxy {
                        allow: vec!["a.example".to_string()],
                        deny: Vec::new(),
                    },
                    "1g",
                    2.0
                ),
                sbx::policy_allow_args(&name, "a.example"),
            ],
            "preflight, then create with the profile's plan, then each allow in order"
        );
    }

    #[tokio::test]
    async fn profile_for_is_available_to_the_routing_test() {
        let cfg = enabled_sbx();
        assert_eq!(profile_for(&cfg, None).network, "none");
    }

    #[tokio::test]
    async fn teardown_ghost_sandbox_forgets_the_sandbox() {
        let session = "t-teardown-1";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        let cfg = enabled_sbx();

        ensure_ghost_sandbox(&runner, &cfg, session, None)
            .await
            .expect("the sandbox is created");
        assert!(
            created().lock().unwrap_or_log().contains(&name),
            "the created set holds the sandbox before teardown"
        );

        calls.lock().unwrap().clear();
        let got = teardown_ghost_sandbox(&runner, session).await;
        assert_eq!(
            got,
            Teardown::Removed,
            "the canned rm succeeds, so the sandbox is removed"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![sbx::rm_args(std::slice::from_ref(&name))],
            "teardown runs rm of the ghost's sandbox"
        );
        assert!(
            !created().lock().unwrap_or_log().contains(&name),
            "teardown forgets the sandbox"
        );

        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        calls.lock().unwrap().clear();
        let again = ensure_ghost_sandbox(&runner, &cfg, session, None)
            .await
            .expect("after a teardown the ghost's sandbox is created again");
        assert_eq!(again, name);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                sbx::version_args(),
                sbx::daemon_status_args(),
                sbx::create_args(&name, &NetworkPlan::None, "1g", 2.0),
            ],
            "the next command must re-run preflight and create, not reuse a torn-down sandbox"
        );
    }

    #[tokio::test]
    async fn teardown_ghost_sandbox_of_a_ghost_that_never_created_one_is_removed() {
        let session = "t-teardown-2";
        let name = sbx::sandbox_name(SandboxKind::Ghost, session);
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(run(
            Some(1),
            "",
            "error: sandbox 'de-ghost-t-teardown-2' not found",
        ));

        let got = teardown_ghost_sandbox(&runner, session).await;

        assert_eq!(
            got,
            Teardown::Removed,
            "a ghost that never ran a sandboxed command still exits cleanly"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![sbx::rm_args(std::slice::from_ref(&name))]
        );
    }

    #[tokio::test]
    async fn run_ghost_in_sandbox_runs_in_the_sandbox_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("t-shell-1".into());
        let sandbox = "de-ghost-x";

        let (output, exit_code) = run_ghost_in_sandbox(
            &reg,
            owner.clone(),
            sandbox,
            "sh -c 'exit 4'",
            "c1",
            spawn_in_process_host,
        )
        .await;

        assert_eq!(
            exit_code, 4,
            "the command ran in the sandbox shell: {output:?}"
        );
        let (count, sandboxed) = crate::shell::with_shells(&reg, |store| {
            (
                store
                    .values()
                    .filter(|r| r.owner.key() == owner.key())
                    .count(),
                store
                    .values()
                    .find(|r| r.owner.key() == owner.key())
                    .and_then(|r| r.sandbox.clone()),
            )
        });
        assert_eq!(count, 1, "exactly one shell for the ghost: {count}");
        assert_eq!(
            sandboxed,
            Some(sandbox.to_string()),
            "the shell's record names the sandbox it runs in"
        );
    }

    #[tokio::test]
    async fn run_ghost_in_sandbox_reuses_its_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("t-shell-2".into());
        let sandbox = "de-ghost-y";

        let (_, first_exit) = run_ghost_in_sandbox(
            &reg,
            owner.clone(),
            sandbox,
            "export DE_SBX=kept",
            "c1",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(first_exit, 0, "the export ran cleanly");

        let (output, second_exit) = run_ghost_in_sandbox(
            &reg,
            owner.clone(),
            sandbox,
            "echo \"v=$DE_SBX\"",
            "c2",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(second_exit, 0, "the echo ran cleanly: {output:?}");
        assert!(
            output.contains("v=kept"),
            "the same shell served the second command, so the export held: {output:?}"
        );
        let count = crate::shell::with_shells(&reg, |store| {
            store
                .values()
                .filter(|r| r.owner.key() == owner.key())
                .count()
        });
        assert_eq!(count, 1, "the second command reused, not spawned: {count}");
    }

    #[tokio::test]
    async fn shell_for_owner_in_sandbox_is_reachable_for_the_module_tests() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("t-shell-3".into());
        let (id, _) = shell_for_owner_in_sandbox(
            &reg,
            owner.clone(),
            "de-ghost-z",
            5,
            std::time::Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the sandbox shell's host comes up");
        let sandbox = crate::shell::with_shells(&reg, |store| {
            store.get(id.as_str()).and_then(|r| r.sandbox.clone())
        });
        assert_eq!(sandbox, Some("de-ghost-z".to_string()));
    }

    #[tokio::test]
    async fn stage_script_preps_then_copies() {
        let (runner, calls, _canned) = fake();
        stage_script(&runner, "de-ghost-t", "s.sh", "/h/s.sh")
            .await
            .expect("both runs exited 0");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "prep, then the copy: {calls:?}");
        assert_eq!(calls[0], stage_prep_args("de-ghost-t"), "{calls:?}");
        assert_eq!(
            calls[1],
            stage_cp_args("/h/s.sh", "de-ghost-t", "s.sh"),
            "{calls:?}"
        );
    }

    #[tokio::test]
    async fn stage_script_refuses_an_unsafe_name_without_sbx() {
        let (runner, calls, _canned) = fake();
        let err = stage_script(&runner, "de-ghost-t", "../evil", "/h/evil")
            .await
            .expect_err("an unsafe name is refused");
        assert!(
            err.starts_with("sandbox staging failed for `../evil`: "),
            "got: {err}"
        );
        let calls = calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "no sbx call before the name check: {calls:?}"
        );
    }

    #[tokio::test]
    async fn stage_script_fails_closed_on_a_failed_copy() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(run(Some(0), "", ""));
        canned.lock().unwrap().push(run(
            Some(1),
            "",
            "error: resolve source path: lstat /h/s.sh: no such file or directory",
        ));
        let err = stage_script(&runner, "de-ghost-t", "s.sh", "/h/s.sh")
            .await
            .expect_err("the copy failed");
        assert!(
            err.starts_with("sandbox staging failed for `s.sh`: "),
            "got: {err}"
        );
        assert!(err.contains("no such file"), "got: {err}");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "prep ran, then the failed copy: {calls:?}");
    }

    #[tokio::test]
    async fn ensure_sandbox_names_the_chat_sandbox_by_kind() {
        let name = sbx::sandbox_name(SandboxKind::Chat, "t-chat-kind");
        let _guard = CreatedSet::from_name(&name);
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        ));
        canned.lock().unwrap().push(ok_run("Status: running\n"));
        let cfg = enabled_sbx();

        let got = ensure_sandbox(&runner, &cfg, SandboxKind::Chat, "t-chat-kind", None)
            .await
            .expect("the canned version and daemon runs accept preflight");

        assert_eq!(
            got,
            sbx::sandbox_name(SandboxKind::Chat, "t-chat-kind"),
            "the kind decides the prefix, not the caller"
        );
        assert!(
            got.starts_with("de-chat-"),
            "a chat sandbox is a chat-prefixed sandbox: {got}"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                sbx::version_args(),
                sbx::daemon_status_args(),
                sbx::create_args(&name, &NetworkPlan::None, "1g", 2.0),
            ],
            "the chat kind runs the same preflight-then-create sequence"
        );
    }

    #[tokio::test]
    async fn forget_and_teardown_forgets_then_removes() {
        let name = "de-chat-t-forget".to_string();
        let _guard = CreatedSet::from_name(&name);
        created().lock().unwrap_or_log().insert(name.clone());
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(ok_run(""));

        let got = forget_and_teardown(&runner, &name).await;

        assert_eq!(
            got,
            Teardown::Removed,
            "the canned rm succeeds, so the sandbox is removed"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![sbx::rm_args(std::slice::from_ref(&name))],
            "exactly one rm, of the name handed in"
        );
        assert!(
            !created().lock().unwrap_or_log().contains(&name),
            "the name is forgotten before the rm runs, so a concurrent ensure              can never answer from the created set"
        );
    }

    #[tokio::test]
    async fn stage_script_fails_closed_on_a_failed_prep() {
        let (runner, calls, canned) = fake();
        canned.lock().unwrap().push(run(Some(1), "", "mkdir: oops"));
        let err = stage_script(&runner, "de-ghost-t", "s.sh", "/h/s.sh")
            .await
            .expect_err("the prep failed");
        assert!(
            err.starts_with("sandbox staging failed for `s.sh`: "),
            "got: {err}"
        );
        assert!(err.contains("mkdir: oops"), "got: {err}");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "no cp call after a failed prep: {calls:?}");
        assert_eq!(calls[0], stage_prep_args("de-ghost-t"), "{calls:?}");
    }
}
