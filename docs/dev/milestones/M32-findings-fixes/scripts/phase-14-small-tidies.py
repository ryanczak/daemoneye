import re

def edit(path, pairs):
    s = open(path).read()
    for old, new, n in pairs:
        assert s.count(old) == n, (path, old[:70], s.count(old))
        s = s.replace(old, new)
    open(path, 'w').write(s)

# ---- M32 #1: the ghost refusal prints the whole error chain ----
F = 'src/daemon/executor/dispatch_foreground.rs'
edit(F, [
    ('''            format!(
                "Error: foreground execution is refused because the configuration \\
                 could not be read: {e}. Fix or remove {path}, then retry; nothing \\
                 was run.",
                path = crate::config::etc_dir().join("config.toml").display(),
            )
''', '''            // `{e:#}` prints the whole chain: `Config::load` wraps the parse
            // error in a `parsing <path>` context, and `{e}` alone would show
            // only that outer layer, dropping the error the operator needs.
            format!(
                "Error: foreground execution is refused because the configuration \\
                 could not be read: {e:#}. Fix or remove that file, then retry; \\
                 nothing was run."
            )
''', 1),
    ('''    #[test]
    fn ghost_foreground_refusal_refuses_a_ghost_when_the_sandbox_is_on() {
''', '''    /// M32 #1: the refusal names the parse error under the context
    /// `Config::load` wraps it in, and names the file once.
    #[test]
    fn ghost_foreground_refusal_names_the_error_under_its_context() {
        let config: anyhow::Result<crate::config::Config> = Err(anyhow::anyhow!(
            "expected `=`, found newline at line 3"
        )
        .context("parsing /home/op/.daemoneye/etc/config.toml"));
        let text = ghost_foreground_refusal(true, &config)
            .expect("a ghost with an unreadable config is refused");
        assert!(
            text.contains("expected `=`, found newline at line 3"),
            "the refusal must carry the parse error itself: {text}"
        );
        assert_eq!(
            text.matches("/home/op/.daemoneye/etc/config.toml").count(),
            1,
            "the file is named once: {text}"
        );
    }

    #[test]
    fn ghost_foreground_refusal_refuses_a_ghost_when_the_sandbox_is_on() {
''', 1),
])
print('the ghost refusal prints the error chain, and a test pins it')

# ---- #10: the two config defaults name default_true ----
edit('src/config/types.rs', [
    ('            auto_name_enabled: true,\n', '            auto_name_enabled: default_true(),\n', 1),
    ('            commands: true,\n            sudo: false,\n',
     '            commands: default_true(),\n            sudo: false,\n', 1),
])
print('SessionsConfig and ApprovalsConfig defaults call default_true')

# ---- #32 #33: ContainerInfo's docs describe the sbx sandbox it carries ----
edit('src/ipc_types.rs', [
    ('''/// One sandboxed container this daemon owns (M19 phase-05).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContainerInfo {
    /// Short (12-char) container id, as docker displays it.
    pub id: String,
    /// `running`, `exited`, `created`, …
    pub state: String,
    pub image: String,
    /// Owning session, from the `de.session` label. `None` for a container
    /// started before the label existed.
    pub session: Option<String>,
    /// Carries `de.ghost=1`.
    pub is_ghost: bool,
}
''', '''/// One sbx sandbox this daemon owns, as `daemoneye status` lists it. The
/// type kept its docker-era name; the fields are filled by
/// `sbx_lifecycle::sandbox_rows` from `sbx ls --json`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContainerInfo {
    /// The sandbox name: `de-ghost-<id>` or `de-chat-<id>`.
    pub id: String,
    /// The sandbox's status as sbx reports it.
    pub state: String,
    /// Always `"shell"` under sbx, and no renderer prints it. Kept because it
    /// is wire format: dropping it would break an older client reading a
    /// newer daemon's status reply.
    pub image: String,
    /// The owning session or job id: the sandbox name after its `de-ghost-`
    /// or `de-chat-` prefix.
    pub session: Option<String>,
    /// True for a ghost run's sandbox (the `de-ghost-` prefix).
    pub is_ghost: bool,
}
''', 1)])
print('ContainerInfo docs describe the sbx sandbox')

# ---- #48: the unread ArtifactHit and EpochHit fields. EventHit is left
# alone: its `event` decode fails on every row of the contentless events
# table, and removing it would change what the events search returns
# (M32 FINDINGS #16).
edit('src/memory/index/search.rs', [
    ('''pub struct ArtifactHit {
    #[allow(dead_code)]
    pub kind: String,
    pub name: String,
    #[allow(dead_code)]
    pub score: f64,
}
''', '''pub struct ArtifactHit {
    pub name: String,
}
''', 1),
    ('''    pub body: String,
    #[allow(dead_code)]
    pub score: f64,
}
''', '''    pub body: String,
}
''', 1),
    ('''            Ok(ArtifactHit {
                kind: r.get(0)?,
                name: r.get(1)?,
                score: r.get(2)?,
            })
''', '''            Ok(ArtifactHit { name: r.get(1)? })
''', 1),
    ('''                body: r.get(3)?,
                score: r.get(4)?,
            })
''', '''                body: r.get(3)?,
            })
''', 1),
])
print('the three unread ArtifactHit and EpochHit fields are gone')

# ---- #17: startup.rs names its imports ----
edit('src/daemon/startup.rs', [('use super::*;\n', '''use super::scheduled::run_scheduled_job;
use super::server::handle_client;
use super::session::SessionStore;
use super::utils::log_event;
use super::{DAEMON_START, detect_session, instance, ready, scheduled, supervise};
use crate::tmux::cache::SessionCache;
''', 1)])
print('startup.rs imports by name')
