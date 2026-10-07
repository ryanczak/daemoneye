//! The one module that knows the `sbx` CLI: command argv builders, the
//! `sbx ls --json` parser, daemon-sandbox naming, and the preflight
//! classifier for `sbx version` / `sbx daemon status` probes. Pure by
//! design — every function here returns data; callers spawn the processes.

pub const BINARY: &str = "sbx";

/// Which daemon-owned sandbox a name refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxKind {
    Ghost,
    Chat,
}

const GHOST_PREFIX: &str = "de-ghost-";
const CHAT_PREFIX: &str = "de-chat-";
/// sbx rejects names longer than 63 characters.
const MAX_NAME_LEN: usize = 63;

/// The daemon-internal name for a sandbox of `kind` for a run/chat with `id`.
/// ASCII letters are lower-cased, digits and `-` are kept, every other
/// character becomes `-`, an empty id becomes `x`, and the result is capped
/// at 63 bytes (pure ASCII after sanitizing, so byte truncation is safe).
pub fn sandbox_name(kind: SandboxKind, id: &str) -> String {
    let prefix = match kind {
        SandboxKind::Ghost => GHOST_PREFIX,
        SandboxKind::Chat => CHAT_PREFIX,
    };
    let mut sanitized = String::with_capacity(id.len());
    for c in id.chars() {
        match c {
            'a'..='z' | '0'..='9' | '-' => sanitized.push(c),
            'A'..='Z' => sanitized.push(c.to_ascii_lowercase()),
            _ => sanitized.push('-'),
        }
    }
    if sanitized.is_empty() {
        sanitized.push('x');
    }
    let mut name = String::from(prefix);
    name.push_str(&sanitized);
    name.truncate(MAX_NAME_LEN);
    name
}

/// True only for names the daemon itself minted via `sandbox_name`. A sweep
/// deletes whatever this matches, so it must never match an operator's
/// sandbox: no prefix, no substring, no case-insensitive or truncated hit.
pub fn is_daemon_sandbox(name: &str) -> bool {
    name.starts_with(GHOST_PREFIX) || name.starts_with(CHAT_PREFIX)
}

/// How the sandbox may reach the network. `None` denies all egress with
/// `--deny-network '**'`; `Proxy` narrows the operator's global sbx policy
/// with per-sandbox denies and widens it with `policy allow` calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPlan {
    None,
    Proxy {
        allow: Vec<String>,
        deny: Vec<String>,
    },
}

/// Map a config profile onto the sbx network flags. `none` ignores the
/// allow/deny lists because a deny-all rule beats any allow (measured);
/// `proxy` trims the lists and drops empty or all-whitespace entries in
/// order. Unknown modes are an `Err` naming the bad value.
pub fn network_plan(profile: &crate::config::SandboxProfile) -> Result<NetworkPlan, String> {
    match profile.network.as_str() {
        "none" => Ok(NetworkPlan::None),
        "proxy" => Ok(NetworkPlan::Proxy {
            allow: trimmed_list(&profile.proxy_allow),
            deny: trimmed_list(&profile.proxy_deny),
        }),
        other => Err(format!(
            "unknown sandbox network mode `{other}` (expected \"none\" or \"proxy\")"
        )),
    }
}

fn trimmed_list(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// Render `cpus` as the integer string sbx's `--cpus` flag requires
/// (decimals are rejected): ceil, clamped to at least 1, `NaN`/infinite → 1.
fn cpus_arg(cpus: f64) -> String {
    if !cpus.is_finite() {
        return "1".to_string();
    }
    let n = cpus.ceil().max(1.0) as u64;
    n.to_string()
}

/// Argv for `sbx create` — everything before the positional `shell`, which is
/// always the last element. Allow hosts are handled by later
/// `policy_allow_args` calls, not by create flags.
pub fn create_args(name: &str, plan: &NetworkPlan, memory: &str, cpus: f64) -> Vec<String> {
    let mut argv = vec![
        "create".to_string(),
        "--name".to_string(),
        name.to_string(),
        "-q".to_string(),
        "--pull".to_string(),
        // The OS keychain is usually locked, and `--pull always` fails on it;
        // `missing` only reaches the registry when the image is absent.
        "missing".to_string(),
        "--memory".to_string(),
        memory.to_string(),
        "--cpus".to_string(),
        cpus_arg(cpus),
    ];
    match plan {
        NetworkPlan::None => {
            argv.push("--deny-network".to_string());
            argv.push("**".to_string());
        }
        NetworkPlan::Proxy { deny, .. } => {
            for host in deny {
                argv.push("--deny-network".to_string());
                argv.push(host.clone());
            }
        }
    }
    argv.push("shell".to_string());
    argv
}

/// Argv to allow `host` egress for one sandbox, after create.
pub fn policy_allow_args(name: &str, host: &str) -> Vec<String> {
    vec![
        "policy".to_string(),
        "allow".to_string(),
        "network".to_string(),
        "--sandbox".to_string(),
        name.to_string(),
        host.to_string(),
    ]
}

/// Argv for the interactive shell host: `sbx exec -it [-w <dir>] <n> bash`.
pub fn shell_exec_args(name: &str, workdir: Option<&str>) -> Vec<String> {
    let mut argv = vec!["exec".to_string(), "-it".to_string()];
    if let Some(dir) = workdir {
        argv.push("-w".to_string());
        argv.push(dir.to_string());
    }
    argv.push(name.to_string());
    argv.push("bash".to_string());
    argv
}

/// The sandbox shell argv, tagged so a side `sbx exec` can find this shell's
/// bash among the sandbox's other shells:
/// `exec -it -e DE_SHELL=<tag> <name> bash`.
pub fn tagged_shell_exec_args(name: &str, tag: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        "-it".to_string(),
        "-e".to_string(),
        format!("DE_SHELL={tag}"),
        name.to_string(),
        "bash".to_string(),
    ]
}

/// The in-VM signal script, measured on sbx v0.46.0 (see the M26 README).
///
/// Reads `$1` = the shell's tag and `$2` = one of `FIND`, `STOP`, `CONT`,
/// `HUP`. It finds the process whose environment holds `DE_SHELL=<tag>`,
/// which is a **session leader** and whose **parent is not tagged** (the
/// outer bash), then acts on it and on its terminal's foreground group
/// (`tpgid`).
pub const VM_SIGNAL_SCRIPT: &str = r#"id=$1; sig=$2; bash_pid=""
for p in /proc/[0-9]*; do
  pid=${p#/proc/}
  tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" || continue
  # stat: pid (comm) state ppid pgrp session tty_nr tpgid ...
  rest=$(sed 's/.*) //' "$p/stat" 2>/dev/null) || continue
  set -- $rest; state=$1; sid=$4; tpgid=$6
  [ "$sid" = "$pid" ] || continue
  ppid=$2
  tr '\0' '\n' < "/proc/$ppid/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" && continue
  bash_pid=$pid; fg=$tpgid; st=$state; break
done
[ -n "$bash_pid" ] || { echo "no-shell"; exit 3; }
case $sig in
  FIND) echo "bash=$bash_pid fg=$fg state=$st";;
  HUP) kill -CONT "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -CONT -- "-$fg"; kill -HUP "$bash_pid"; echo "hup bash=$bash_pid";;
  STOP) kill -STOP "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -STOP -- "-$fg"; echo "stopped bash=$bash_pid fg=$fg";;
  CONT) [ "$fg" != "$bash_pid" ] && kill -CONT -- "-$fg"; kill -CONT "$bash_pid"; echo "cont bash=$bash_pid fg=$fg";;
esac
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmOp {
    Find,
    Stop,
    Cont,
    Hup,
}

impl VmOp {
    pub fn as_str(self) -> &'static str {
        match self {
            VmOp::Find => "FIND",
            VmOp::Stop => "STOP",
            VmOp::Cont => "CONT",
            VmOp::Hup => "HUP",
        }
    }
}

/// `exec <sandbox> sh -c <VM_SIGNAL_SCRIPT> vmsig <tag> <OP>`.
pub fn vm_signal_args(sandbox: &str, tag: &str, op: VmOp) -> Vec<String> {
    vec![
        "exec".to_string(),
        sandbox.to_string(),
        "sh".to_string(),
        "-c".to_string(),
        VM_SIGNAL_SCRIPT.to_string(),
        "vmsig".to_string(),
        tag.to_string(),
        op.as_str().to_string(),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmShellState {
    pub bash_pid: u32,
    pub fg_pgid: u32,
    pub state: char,
}

impl VmShellState {
    /// The shell is stopped (`T`).
    pub fn is_paused(&self) -> bool {
        self.state == 'T'
    }

    /// A job holds the terminal (`fg_pgid != bash_pid`).
    pub fn has_foreground_job(&self) -> bool {
        self.fg_pgid != self.bash_pid
    }
}

/// Parse `FIND` output: one line `bash=<u32> fg=<u32> state=<one char>`.
pub fn parse_vm_find(stdout: &str) -> Option<VmShellState> {
    let mut fields = stdout.split_whitespace();
    let bash_pid = fields.next()?.strip_prefix("bash=")?.parse().ok()?;
    let fg_pgid = fields.next()?.strip_prefix("fg=")?.parse().ok()?;
    let state = fields.next()?.strip_prefix("state=")?;
    if state.chars().count() != 1 {
        return None;
    }
    let state = state.chars().next()?;
    if fields.next().is_some() {
        return None;
    }
    Some(VmShellState {
        bash_pid,
        fg_pgid,
        state,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmOpOutcome {
    Done,
    NoShell,
    Failed(String),
}

/// The outcome of any `vm_signal_args` run. `exit_code` is `None` when the
/// process was killed by a signal.
pub fn vm_op_outcome(exit_code: Option<i32>, stdout: &str, stderr: &str) -> VmOpOutcome {
    match exit_code {
        Some(0) => VmOpOutcome::Done,
        Some(3) if stdout.trim() == "no-shell" => VmOpOutcome::NoShell,
        other => {
            let detail = match (stderr.trim(), stdout.trim()) {
                ("", "") => match other {
                    Some(code) => format!("exit {code}"),
                    None => "killed by a signal".to_string(),
                },
                ("", out) => out.to_string(),
                (err, _) => err.to_string(),
            };
            VmOpOutcome::Failed(detail)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbxTarget {
    pub sandbox: String,
    pub tag: String,
}

/// `Some` when a shell host's `program` + `args` are exactly what
/// `tagged_shell_exec_args` builds: `program`'s file name is `sbx` and
/// `args == ["exec", "-it", "-e", "DE_SHELL=<tag>", <sandbox>, "bash"]`
/// with a non-empty tag and sandbox.
pub fn sbx_target_from_host(program: &str, args: &[String]) -> Option<SbxTarget> {
    if std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        != Some("sbx")
    {
        return None;
    }
    if args.len() != 6
        || args[0] != "exec"
        || args[1] != "-it"
        || args[2] != "-e"
        || args[5] != "bash"
    {
        return None;
    }
    let tag = args[3]
        .strip_prefix("DE_SHELL=")
        .filter(|t| !t.is_empty())?;
    if args[4].is_empty() {
        return None;
    }
    Some(SbxTarget {
        sandbox: args[4].clone(),
        tag: tag.to_string(),
    })
}

/// Argv for a one-off command whose exit status passes through.
pub fn exec_args(name: &str, argv: &[&str]) -> Vec<String> {
    let mut out = vec!["exec".to_string(), name.to_string()];
    out.extend(argv.iter().map(|a| a.to_string()));
    out
}

/// Argv to stop sandboxes. Empty for no names — callers skip an empty argv,
/// because `sbx stop` with no names must never run.
pub fn stop_args(names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut argv = vec!["stop".to_string()];
    argv.extend(names.iter().cloned());
    argv
}

/// Argv to force-remove sandboxes. Empty for no names — callers skip an
/// empty argv, because `sbx rm -f` with no names must never run (and rm
/// fails outright while the keychain is locked; use `stop` first).
pub fn rm_args(names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut argv = vec!["rm".to_string(), "-f".to_string()];
    argv.extend(names.iter().cloned());
    argv
}

/// Argv for the sandbox listing (`ls --json`, empty list still exits 0).
pub fn ls_args() -> Vec<String> {
    vec!["ls".to_string(), "--json".to_string()]
}

/// Argv for the version probe.
pub fn version_args() -> Vec<String> {
    vec!["version".to_string()]
}

/// Argv for the daemon-status probe.
pub fn daemon_status_args() -> Vec<String> {
    vec!["daemon".to_string(), "status".to_string()]
}

/// One row of `sbx ls --json`, narrowed to what the daemon acts on. Extra
/// fields (`id`, `agent`, `workspaces`, …) are ignored by serde.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct SandboxInfo {
    pub name: String,
    pub status: String,
}

#[derive(serde::Deserialize)]
struct LsOutput {
    sandboxes: Vec<SandboxInfo>,
}

/// Parse `sbx ls --json` output. `None` when the text is not that shape: not
/// JSON at all (e.g. the human text `No sandboxes found.`), a missing
/// `sandboxes` key, or a non-object JSON value.
pub fn parse_ls_json(text: &str) -> Option<Vec<SandboxInfo>> {
    serde_json::from_str::<LsOutput>(text)
        .ok()
        .map(|o| o.sandboxes)
}

/// Names in `listed`, in listed order, that the daemon minted and that are
/// not in the live set — the sweep targets. Foreign sandboxes are never
/// returned, whatever their status.
pub fn stale_daemon_sandboxes(listed: &[SandboxInfo], live: &[String]) -> Vec<String> {
    listed
        .iter()
        .filter(|s| is_daemon_sandbox(&s.name) && !live.contains(&s.name))
        .map(|s| s.name.clone())
        .collect()
}

/// The collected outcome of one preflight probe (`sbx version` or
/// `sbx daemon status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// `Some(kind)` when the spawn itself failed.
    pub spawn_error: Option<std::io::ErrorKind>,
    pub exit_ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Why sandboxed execution is unavailable, in the order preflight checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SbxUnavailable {
    NotInstalled,
    SpawnFailed { detail: String },
    NotAuthenticated,
    KeychainLocked,
    DaemonNotRunning { detail: String },
    UnrecognisedVersion { output: String },
}

/// Classify the two preflight probes; `Ok` is the sbx version string (e.g.
/// `v0.46.0`). The version probe is checked first, then the daemon probe.
/// The keychain and authentication stderr markers are checked even when
/// `exit_ok` is true, because sbx prints the keychain message as a `WARN` on
/// a command that still succeeds; the keychain marker wins when both appear.
pub fn classify_preflight(version: &Probe, daemon: &Probe) -> Result<String, SbxUnavailable> {
    for probe in [version, daemon] {
        if let Some(kind) = probe.spawn_error {
            if kind == std::io::ErrorKind::NotFound {
                return Err(SbxUnavailable::NotInstalled);
            }
            return Err(SbxUnavailable::SpawnFailed {
                detail: format!("{kind:?}"),
            });
        }
        if probe.stderr.contains("the OS keychain is locked") {
            return Err(SbxUnavailable::KeychainLocked);
        }
        if probe.stderr.contains("Not authenticated to Docker") {
            return Err(SbxUnavailable::NotAuthenticated);
        }
    }
    classify_version(version).map_err(|output| SbxUnavailable::UnrecognisedVersion { output })?;
    classify_daemon(daemon).map_err(|detail| SbxUnavailable::DaemonNotRunning { detail })?;
    Ok(version
        .stdout
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(2))
        .unwrap_or_default()
        .to_string())
}

/// `Ok(version)` when stdout's first line is `sbx version: <v…> …`, else
/// `Err(stdout+stderr trimmed)` for the operator-facing dump.
fn classify_version(probe: &Probe) -> Result<String, String> {
    if !probe.exit_ok {
        return Err(format!("{}{}", probe.stdout, probe.stderr)
            .trim()
            .to_string());
    }
    let first = probe.stdout.lines().next().unwrap_or_default().trim();
    let mut tokens = first.split_whitespace();
    if tokens.next() != Some("sbx")
        || tokens.next() != Some("version:")
        || !tokens.next().is_some_and(|v| v.starts_with('v'))
    {
        return Err(probe.stdout.trim().to_string());
    }
    let version = first
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .to_string();
    Ok(version)
}

/// `Ok(())` when stdout's first line is exactly `Status: running`, else
/// `Err(detail)` — the first non-empty stderr line, else the first non-empty
/// stdout line, else "".
fn classify_daemon(probe: &Probe) -> Result<(), String> {
    if !probe.exit_ok {
        let detail = first_non_empty_line(&probe.stderr)
            .or_else(|| first_non_empty_line(&probe.stdout))
            .unwrap_or_default();
        return Err(detail);
    }
    let first = probe.stdout.lines().next().unwrap_or_default().trim();
    if first == "Status: running" {
        Ok(())
    } else {
        Err(first.to_string())
    }
}

fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// One-line, operator-facing explanation with the concrete fix, in the same
/// voice as the docker `describe_unavailable` messages.
pub fn describe_unavailable(reason: &SbxUnavailable) -> String {
    match reason {
        SbxUnavailable::NotInstalled => {
            "sandbox unavailable: `sbx` is not installed — install Docker Sandboxes (sbx) and try again".to_string()
        }
        SbxUnavailable::SpawnFailed { detail } => format!(
            "sandbox unavailable: `sbx` could not be spawned ({detail}) — check the installation and permissions, then try again"
        ),
        SbxUnavailable::NotAuthenticated => {
            "sandbox unavailable: not authenticated to Docker — run `sbx login` and try again".to_string()
        }
        SbxUnavailable::KeychainLocked => {
            "sandbox unavailable: the OS keychain is locked — unlock your keyring and retry".to_string()
        }
        SbxUnavailable::DaemonNotRunning { detail } => format!(
            "sandbox unavailable: the sbx daemon is not running ({detail}) — run `sbx daemon start` and try again"
        ),
        SbxUnavailable::UnrecognisedVersion { output } => format!(
            "sandbox unavailable: unrecognised `sbx version` output ({output}) — check the installed sbx version"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SandboxProfile;

    fn probe(exit_ok: bool, stdout: &str, stderr: &str) -> Probe {
        Probe {
            spawn_error: None,
            exit_ok,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn spawn_probe(kind: std::io::ErrorKind) -> Probe {
        Probe {
            spawn_error: Some(kind),
            exit_ok: false,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn profile(network: &str, allow: &[&str], deny: &[&str]) -> SandboxProfile {
        SandboxProfile {
            network: network.to_string(),
            proxy_allow: allow.iter().map(|s| s.to_string()).collect(),
            proxy_deny: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn sandbox_name_prefixes_by_kind() {
        assert_eq!(sandbox_name(SandboxKind::Ghost, "r1"), "de-ghost-r1");
        assert_eq!(sandbox_name(SandboxKind::Chat, "s1"), "de-chat-s1");
    }

    #[test]
    fn sandbox_name_sanitizes_the_id() {
        assert_eq!(
            sandbox_name(SandboxKind::Ghost, "Run_7.x/y z"),
            "de-ghost-run-7-x-y-z"
        );
        assert_eq!(sandbox_name(SandboxKind::Ghost, "é"), "de-ghost--");
        assert_eq!(sandbox_name(SandboxKind::Ghost, ""), "de-ghost-x");
    }

    #[test]
    fn sandbox_name_caps_at_63_characters() {
        let id = "a".repeat(100);
        let name = sandbox_name(SandboxKind::Ghost, &id);
        assert_eq!(name.len(), 63);
        assert!(name.starts_with("de-ghost-"));
    }

    #[test]
    fn sandbox_name_never_contains_an_underscore() {
        for id in ["a_b", "__", "x_y_z"] {
            let name = sandbox_name(SandboxKind::Ghost, id);
            assert!(!name.contains('_'), "id `{id}` gave `{name}`");
        }
    }

    #[test]
    fn is_daemon_sandbox_matches_only_daemon_prefixes() {
        for name in ["de-ghost-r1", "de-chat-s1"] {
            assert!(is_daemon_sandbox(name), "`{name}` should match");
        }
        for name in [
            "de-probe-a",
            "de-ghost",
            "my-de-ghost-1",
            "De-Ghost-1",
            "claude-daemoneye",
        ] {
            assert!(!is_daemon_sandbox(name), "`{name}` should not match");
        }
    }

    #[test]
    fn network_plan_none_ignores_lists() {
        let p = profile("none", &["a.example"], &["b.example"]);
        assert_eq!(network_plan(&p), Ok(NetworkPlan::None));
    }

    #[test]
    fn network_plan_proxy_copies_and_trims() {
        let p = profile("proxy", &[" a.example ", "", "  "], &["b.example"]);
        assert_eq!(
            network_plan(&p),
            Ok(NetworkPlan::Proxy {
                allow: vec!["a.example".to_string()],
                deny: vec!["b.example".to_string()],
            })
        );
    }

    #[test]
    fn network_plan_rejects_unknown_modes() {
        for mode in ["None", " none", "", "host"] {
            let p = profile(mode, &[], &[]);
            let err = network_plan(&p).expect_err("unknown mode should be an Err");
            assert!(err.contains(mode), "message `{err}` misses `{mode}`");
        }
    }

    #[test]
    fn create_args_none_denies_everything() {
        assert_eq!(
            create_args("de-ghost-r1", &NetworkPlan::None, "1g", 2.0),
            vec![
                "create",
                "--name",
                "de-ghost-r1",
                "-q",
                "--pull",
                "missing",
                "--memory",
                "1g",
                "--cpus",
                "2",
                "--deny-network",
                "**",
                "shell",
            ]
        );
    }

    #[test]
    fn create_args_proxy_lists_each_deny_and_no_allow() {
        let plan = NetworkPlan::Proxy {
            allow: vec!["a.example".to_string(), "b.example".to_string()],
            deny: vec!["a.example".to_string(), "b.example".to_string()],
        };
        let argv = create_args("n", &plan, "1g", 2.0);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(
            refs,
            vec![
                "create",
                "--name",
                "n",
                "-q",
                "--pull",
                "missing",
                "--memory",
                "1g",
                "--cpus",
                "2",
                "--deny-network",
                "a.example",
                "--deny-network",
                "b.example",
                "shell",
            ]
        );
    }

    #[test]
    fn create_args_proxy_with_no_deny_has_no_deny_flag() {
        let plan = NetworkPlan::Proxy {
            allow: vec!["a.example".to_string()],
            deny: vec![],
        };
        let argv = create_args("n", &plan, "1g", 2.0);
        assert!(!argv.iter().any(|a| a == "--deny-network"));
    }

    #[test]
    fn create_args_always_pulls_missing_and_ends_with_shell() {
        let plans = [
            NetworkPlan::None,
            NetworkPlan::Proxy {
                allow: vec!["a.example".to_string()],
                deny: vec!["b.example".to_string()],
            },
        ];
        for plan in &plans {
            let argv = create_args("n", plan, "2g", 4.0);
            assert_eq!(argv.last().unwrap(), "shell");
            let i = argv.iter().position(|a| a == "--pull").unwrap();
            assert_eq!(argv[i + 1], "missing");
        }
    }

    #[test]
    fn create_args_renders_cpus_as_an_integer() {
        let plan = NetworkPlan::None;
        for (cpus, expected) in [
            (2.0, "2"),
            (1.5, "2"),
            (0.0, "1"),
            (-3.0, "1"),
            (f64::NAN, "1"),
            (f64::INFINITY, "1"),
        ] {
            let argv = create_args("n", &plan, "1g", cpus);
            let i = argv.iter().position(|a| a == "--cpus").unwrap();
            assert_eq!(argv[i + 1], expected, "cpus {cpus}");
            assert!(!argv[i + 1].contains('.'), "cpus {cpus} has a decimal");
        }
    }

    #[test]
    fn exec_and_policy_args_match_the_measured_shapes() {
        assert_eq!(
            policy_allow_args("de-ghost-r1", "api.github.com"),
            vec![
                "policy",
                "allow",
                "network",
                "--sandbox",
                "de-ghost-r1",
                "api.github.com"
            ]
        );
        assert_eq!(
            shell_exec_args("de-ghost-r1", None),
            vec!["exec", "-it", "de-ghost-r1", "bash"]
        );
        assert_eq!(
            shell_exec_args("de-ghost-r1", Some("/w")),
            vec!["exec", "-it", "-w", "/w", "de-ghost-r1", "bash"]
        );
        assert_eq!(
            exec_args("de-ghost-r1", &["kill", "-STOP", "--", "-42"]),
            vec!["exec", "de-ghost-r1", "kill", "-STOP", "--", "-42"]
        );
    }

    #[test]
    fn stop_and_rm_args_are_empty_for_no_names() {
        assert!(stop_args(&[]).is_empty());
        assert!(rm_args(&[]).is_empty());
        let names = vec!["a".to_string(), "b".to_string()];
        assert_eq!(stop_args(&names), vec!["stop", "a", "b"]);
        assert_eq!(rm_args(&names), vec!["rm", "-f", "a", "b"]);
    }

    #[test]
    fn parse_ls_json_reads_the_measured_shape() {
        let text = r#"{"sandboxes": [
            {"name": "a", "id": "id-a", "agent": "shell", "status": "running",
             "last_used_at": "t1", "created_at": "t2", "workspaces": ["/w"]},
            {"name": "b", "id": "id-b", "agent": "shell", "status": "exited",
             "last_used_at": "t3", "created_at": "t4"}
        ]}"#;
        let listed = parse_ls_json(text).expect("measured shape parses");
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed[0],
            SandboxInfo {
                name: "a".into(),
                status: "running".into()
            }
        );
        assert_eq!(
            listed[1],
            SandboxInfo {
                name: "b".into(),
                status: "exited".into()
            }
        );
    }

    #[test]
    fn parse_ls_json_reads_an_empty_list() {
        assert_eq!(parse_ls_json(r#"{"sandboxes": []}"#), Some(Vec::new()));
    }

    #[test]
    fn parse_ls_json_rejects_non_json_and_wrong_shapes() {
        assert_eq!(parse_ls_json("No sandboxes found."), None);
        assert_eq!(parse_ls_json("{}"), None);
        assert_eq!(parse_ls_json("[]"), None);
    }

    #[test]
    fn stale_daemon_sandboxes_skips_live_and_foreign() {
        let listed = vec![
            SandboxInfo {
                name: "de-ghost-a".into(),
                status: "running".into(),
            },
            SandboxInfo {
                name: "de-ghost-b".into(),
                status: "running".into(),
            },
            SandboxInfo {
                name: "de-probe-a".into(),
                status: "running".into(),
            },
            SandboxInfo {
                name: "claude-x".into(),
                status: "running".into(),
            },
        ];
        let live = vec!["de-ghost-b".to_string()];
        assert_eq!(
            stale_daemon_sandboxes(&listed, &live),
            vec!["de-ghost-a".to_string()]
        );
    }

    #[test]
    fn classify_preflight_accepts_the_measured_outputs() {
        let version = probe(
            true,
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
            "",
        );
        let daemon = probe(true, "Status: running\n", "");
        assert_eq!(
            classify_preflight(&version, &daemon),
            Ok("v0.46.0".to_string())
        );
    }

    #[test]
    fn classify_preflight_maps_each_failure() {
        let daemon = probe(true, "Status: running\n", "");
        let version = probe(
            true,
            "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
            "",
        );

        // Version probe wins: keychain WARN printed by a still-successful run.
        let warn = probe(
            true,
            "sbx version: v0.46.0 x\n",
            "WARN the OS keychain is locked; unlock your keyring and retry\n",
        );
        assert_eq!(
            classify_preflight(&warn, &daemon),
            Err(SbxUnavailable::KeychainLocked)
        );

        let auth = probe(
            true,
            "sbx version: v0.46.0 x\n",
            "error: Not authenticated to Docker\n",
        );
        assert_eq!(
            classify_preflight(&auth, &daemon),
            Err(SbxUnavailable::NotAuthenticated)
        );

        let both = probe(
            true,
            "sbx version: v0.46.0 x\n",
            "Not authenticated to Docker; the OS keychain is locked\n",
        );
        assert_eq!(
            classify_preflight(&both, &daemon),
            Err(SbxUnavailable::KeychainLocked)
        );

        // Daemon probe wins over a good version probe.
        let daemon_locked = probe(
            true,
            "Status: running\n",
            "WARN the OS keychain is locked\n",
        );
        assert_eq!(
            classify_preflight(&version, &daemon_locked),
            Err(SbxUnavailable::KeychainLocked)
        );
        let daemon_auth = probe(
            true,
            "Status: running\n",
            "error: Not authenticated to Docker\n",
        );
        assert_eq!(
            classify_preflight(&version, &daemon_auth),
            Err(SbxUnavailable::NotAuthenticated)
        );

        assert_eq!(
            classify_preflight(&spawn_probe(std::io::ErrorKind::NotFound), &daemon),
            Err(SbxUnavailable::NotInstalled)
        );
        assert_eq!(
            classify_preflight(&version, &spawn_probe(std::io::ErrorKind::PermissionDenied)),
            Err(SbxUnavailable::SpawnFailed {
                detail: "PermissionDenied".to_string()
            })
        );

        let stopped = probe(true, "Status: stopped\n", "");
        assert_eq!(
            classify_preflight(&version, &stopped),
            Err(SbxUnavailable::DaemonNotRunning {
                detail: "Status: stopped".to_string()
            })
        );

        let wrong = probe(true, "sbx 0.46\n", "");
        assert_eq!(
            classify_preflight(&wrong, &daemon),
            Err(SbxUnavailable::UnrecognisedVersion {
                output: "sbx 0.46".to_string()
            })
        );
    }

    #[test]
    fn describe_unavailable_names_the_fix() {
        let cases = vec![
            (SbxUnavailable::NotInstalled, vec!["sbx"]),
            (
                SbxUnavailable::SpawnFailed {
                    detail: "d".to_string(),
                },
                vec!["d"],
            ),
            (SbxUnavailable::NotAuthenticated, vec!["sbx login"]),
            (SbxUnavailable::KeychainLocked, vec!["keychain", "unlock"]),
            (
                SbxUnavailable::DaemonNotRunning {
                    detail: "x".to_string(),
                },
                vec!["sbx daemon start", "x"],
            ),
            (
                SbxUnavailable::UnrecognisedVersion {
                    output: "y".to_string(),
                },
                vec!["y"],
            ),
        ];
        for (reason, must_contain) in cases {
            let msg = describe_unavailable(&reason);
            assert!(
                msg.starts_with("sandbox unavailable: "),
                "`{msg}` has no prefix for {reason:?}"
            );
            for needle in must_contain {
                assert!(
                    msg.contains(needle),
                    "`{msg}` misses `{needle}` for {reason:?}"
                );
            }
        }
    }

    #[test]
    fn tagged_shell_exec_args_match_the_measured_shape() {
        let argv = tagged_shell_exec_args("de-ghost-r1", "s3");
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(
            refs,
            vec!["exec", "-it", "-e", "DE_SHELL=s3", "de-ghost-r1", "bash"]
        );
    }

    #[test]
    fn vm_signal_args_pass_tag_and_op_as_positionals() {
        for (op, word) in [
            (VmOp::Find, "FIND"),
            (VmOp::Stop, "STOP"),
            (VmOp::Cont, "CONT"),
            (VmOp::Hup, "HUP"),
        ] {
            let argv = vm_signal_args("de-ghost-r1", "s3", op);
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            assert_eq!(
                refs,
                vec![
                    "exec",
                    "de-ghost-r1",
                    "sh",
                    "-c",
                    VM_SIGNAL_SCRIPT,
                    "vmsig",
                    "s3",
                    word
                ],
                "op {word}"
            );
        }
    }

    #[test]
    fn vm_signal_script_is_valid_posix_sh() {
        let status = std::process::Command::new("sh")
            .args(["-n", "-c", VM_SIGNAL_SCRIPT])
            .status()
            .unwrap_or_else(|e| panic!("sh should spawn: {e}"));
        assert!(status.success(), "sh -n rejected the script: {status}");
    }

    #[test]
    fn vm_signal_script_matches_the_measured_text() {
        for line in [
            r#"[ "$sid" = "$pid" ] || continue"#,
            r#"tr '\0' '\n' < "/proc/$ppid/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" && continue"#,
            r#"[ -n "$bash_pid" ] || { echo "no-shell"; exit 3; }"#,
            r#"STOP) kill -STOP "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -STOP -- "-$fg"; echo "stopped bash=$bash_pid fg=$fg";;"#,
        ] {
            assert!(
                VM_SIGNAL_SCRIPT.contains(line),
                "script misses the measured line `{line}`"
            );
        }
    }

    #[test]
    fn parse_vm_find_reads_the_measured_lines() {
        let idle = parse_vm_find("bash=366 fg=366 state=S\n").expect("idle line parses");
        assert_eq!(
            idle,
            VmShellState {
                bash_pid: 366,
                fg_pgid: 366,
                state: 'S'
            }
        );
        assert!(!idle.is_paused());
        assert!(!idle.has_foreground_job());

        let running = parse_vm_find("bash=366 fg=414 state=S").expect("running line parses");
        assert_eq!(running.fg_pgid, 414);
        assert!(running.has_foreground_job());
        assert!(!running.is_paused());

        let paused = parse_vm_find("bash=366 fg=450 state=T").expect("paused line parses");
        assert!(paused.is_paused());
        assert!(paused.has_foreground_job());
    }

    #[test]
    fn parse_vm_find_rejects_other_output() {
        for text in [
            "no-shell",
            "",
            "bash=x fg=1 state=S",
            "bash=1 fg=1",
            "bash=1 fg=1 state=ST",
            "fg=1 bash=1 state=S",
        ] {
            assert_eq!(parse_vm_find(text), None, "text `{text}` should reject");
        }
    }

    #[test]
    fn vm_op_outcome_maps_exit_codes() {
        assert_eq!(
            vm_op_outcome(Some(0), "stopped bash=1 fg=2", ""),
            VmOpOutcome::Done
        );
        assert_eq!(
            vm_op_outcome(Some(3), "no-shell\n", ""),
            VmOpOutcome::NoShell
        );
        assert_eq!(
            vm_op_outcome(Some(3), "other", ""),
            VmOpOutcome::Failed("other".to_string())
        );
        assert_eq!(
            vm_op_outcome(Some(1), "", "error: sandbox 'x' not found"),
            VmOpOutcome::Failed("error: sandbox 'x' not found".to_string())
        );
        assert_eq!(
            vm_op_outcome(None, "", ""),
            VmOpOutcome::Failed("killed by a signal".to_string())
        );
    }

    #[test]
    fn sbx_target_from_host_reads_a_tagged_shell() {
        let args: Vec<String> = tagged_shell_exec_args("de-ghost-r1", "s3");
        assert_eq!(
            sbx_target_from_host("/usr/bin/sbx", &args),
            Some(SbxTarget {
                sandbox: "de-ghost-r1".to_string(),
                tag: "s3".to_string()
            })
        );
        assert_eq!(
            sbx_target_from_host("sbx", &args),
            Some(SbxTarget {
                sandbox: "de-ghost-r1".to_string(),
                tag: "s3".to_string()
            })
        );
    }

    #[test]
    fn sbx_target_from_host_rejects_everything_else() {
        let tagged: Vec<String> = tagged_shell_exec_args("de-ghost-r1", "s3");
        assert_eq!(
            sbx_target_from_host("/usr/bin/ssh", &tagged),
            None,
            "wrong program"
        );
        let untagged = vec![
            "exec".to_string(),
            "-it".to_string(),
            "de-ghost-r1".to_string(),
            "bash".to_string(),
        ];
        assert_eq!(sbx_target_from_host("sbx", &untagged), None, "untagged");
        let wrong_var = vec![
            "exec".to_string(),
            "-it".to_string(),
            "-e".to_string(),
            "FOO=s3".to_string(),
            "de-ghost-r1".to_string(),
            "bash".to_string(),
        ];
        assert_eq!(sbx_target_from_host("sbx", &wrong_var), None, "wrong var");
        let empty_tag = vec![
            "exec".to_string(),
            "-it".to_string(),
            "-e".to_string(),
            "DE_SHELL=".to_string(),
            "de-ghost-r1".to_string(),
            "bash".to_string(),
        ];
        assert_eq!(sbx_target_from_host("sbx", &empty_tag), None, "empty tag");
        let seventh: Vec<String> = tagged
            .iter()
            .chain(["extra".to_string()].iter())
            .cloned()
            .collect();
        assert_eq!(sbx_target_from_host("sbx", &seventh), None, "seventh arg");
        assert_eq!(
            sbx_target_from_host("/usr/bin/sbx-helper", &tagged),
            None,
            "sbx-helper program"
        );
    }

    #[test]
    fn sbx_target_round_trips_the_builder() {
        let args = tagged_shell_exec_args("de-chat-x", "s7");
        assert_eq!(
            sbx_target_from_host("sbx", &args),
            Some(SbxTarget {
                sandbox: "de-chat-x".to_string(),
                tag: "s7".to_string()
            })
        );
    }
}
