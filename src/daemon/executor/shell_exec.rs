//! Running a chat turn's `run_terminal_command` on the PTY shell substrate,
//! and formatting what it produced for a model.
//!
//! Sequence for one command (F3): annotate ANSI first — it is the sanitiser as
//! well as the annotator, dropping control bytes a mid-token escape could use
//! to split a secret past its regex — then mask, then normalize.

use std::time::Duration;

use super::foreground;
use crate::ai::mask_sensitive;
use crate::daemon::utils::normalize_output;
use crate::shell::spawn_detached_host;
use crate::shell::{Owner, ShellId, ShellRegistry, login_shell, run_command, shell_for_owner};

/// How long a fresh host may take to come up and answer `Status`.
pub(super) const SHELL_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// The PTY geometry every shell host is spawned with.
const ROWS: u16 = 24;
/// The PTY geometry every shell host is spawned with.
const COLS: u16 = 80;

/// Annotate, mask, then normalize. The order matters (F3): the annotation step
/// must strip control bytes before masking, or an escape embedded mid-token
/// could split a secret past its regex.
pub(super) fn format_for_model(raw: &str) -> String {
    mask_sensitive(&normalize_output(&crate::tmux::annotate_output(raw)))
}

/// What one command produced, already fit for a model.
pub(super) struct ShellRunResult {
    /// Annotated, masked, normalized. Ready to send as the tool result.
    pub text: String,
    pub exit_code: i32,
}

/// Run `cmd` in `owner`'s PTY shell and format the result for the model.
/// Returns the shell's id and the text to send as the tool result.
///
/// On any failure the error names the shell id and the command's own timeout,
/// and the caller must surface "the command may still be running" — a shell
/// the command killed keeps producing output the marker protocol never
/// answers.
pub(super) async fn run_in_shell(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
) -> anyhow::Result<(ShellId, ShellRunResult)> {
    run_in_shell_with(reg, owner, cmd, max_per_owner, |id| {
        spawn_detached_host(id, ROWS, COLS)
    })
    .await
}

/// `run_in_shell` with the spawn step injected: tests pass an in-process host
/// because `spawn_detached_host` re-invokes `current_exe()`, which under a
/// test binary is not the daemon (07b F4).
pub(super) async fn run_in_shell_with<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    spawn: F,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let (id, socket) =
        shell_for_owner(reg, owner, max_per_owner, SHELL_READY_TIMEOUT, spawn).await?;
    let outcome = run_command(&socket, cmd, &login_shell(), foreground::LOCAL_CMD_TIMEOUT)
        .await
        .map_err(|e| {
            e.context(format!(
                "command exceeded the {:#?} limit in shell {}",
                foreground::LOCAL_CMD_TIMEOUT,
                id.as_str()
            ))
        })?;
    let raw = String::from_utf8_lossy(&outcome.output);
    let mut text = format_for_model(&raw);
    if let Some(note) = foreground::exit_status_annotation(Some(outcome.exit_code)) {
        text.push_str(&note);
    }
    Ok((
        id,
        ShellRunResult {
            text,
            exit_code: outcome.exit_code,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::shell::{bind, lock_path, serve, socket_path};
    use crate::test_home_guard;
    use anyhow::Context;
    use std::sync::Arc;

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1).
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// The production spawn substitute: a real in-process PTY host serving the
    /// configured socket for `id`, a bash session whose `.cast` the returned
    /// path points at. `spawn_detached_host` cannot run under a test (07b F4).
    async fn spawn_with_cast(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }

    /// The spawn step `run_in_shell_with` takes, paired with the cast path the
    /// host writes so a test can read the raw log afterwards.
    async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
        let cast = spawn_with_cast(id.clone()).await?;
        std::mem::forget(cast);
        Ok(())
    }

    #[tokio::test]
    async fn shell_exec_returns_the_real_exit_code() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let (id, result) = run_in_shell_with(
            &reg,
            Owner::Chat("exit-code".into()),
            "printf 'HELLO\\n'; sh -c 'exit 3'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("an approved, printing, failing command resolves");
        assert_eq!(result.exit_code, 3, "the child's real exit code surfaces");
        assert!(
            result.text.contains("HELLO"),
            "the output must survive alongside the exit code: {:?}",
            result.text
        );
        assert!(
            result.text.contains("exited with status 3"),
            "the non-zero note must be present: {:?}",
            result.text
        );
        assert!(
            id.as_str().starts_with('s'),
            "a fresh id is allocated for the first command"
        );
    }

    #[tokio::test]
    async fn shell_exec_masks_a_secret_that_stays_raw_in_the_cast() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let cast_path: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> =
            Arc::new(std::sync::Mutex::new(None));
        let spawn_cast_path = Arc::clone(&cast_path);
        let (id, result) = run_in_shell_with(
            &reg,
            Owner::Chat("masking".into()),
            "echo AKIAIOSFODNN7EXAMPLE",
            5,
            move |id: ShellId| {
                let cells = Arc::clone(&spawn_cast_path);
                async move {
                    let path = spawn_with_cast(id).await?;
                    std::mem::forget(path.clone());
                    *cells.lock().expect("cast cell lock") = Some(path);
                    Ok(())
                }
            },
        )
        .await
        .expect("the seeded AWS key resolves");
        assert!(
            !result.text.contains("AKIAIOSFODNN7EXAMPLE"),
            "the secret must not reach the tool result: {:?}",
            result.text
        );
        assert!(
            result.text.contains("<AWS_KEY>"),
            "the mask marker must reach the tool result: {:?}",
            result.text
        );
        let cast = std::fs::read(
            cast_path
                .lock()
                .expect("cast cell lock")
                .clone()
                .expect("the spawn step recorded the host's cast path"),
        )
        .with_context(|| format!("reading the cast the host {} wrote", id.as_str()))
        .expect("the cast is readable");
        let cast = String::from_utf8_lossy(&cast);
        assert!(
            cast.contains("AKIAIOSFODNN7EXAMPLE"),
            "the log stays raw at rest — the M20 masking exit criterion is the pair"
        );
    }

    #[tokio::test]
    async fn shell_exec_annotates_colour_and_strips_control_bytes() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let (_, result) = run_in_shell_with(
            &reg,
            Owner::Chat("colour".into()),
            "printf '\\033[31mred\\033[0m\\n\\033[2J\\033[Hclear\\n'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("the coloured command resolves");
        assert!(
            result.text.contains("[ERROR: red]"),
            "red becomes a semantic marker: {:?}",
            result.text
        );
        assert!(
            !result.text.contains('\x1b'),
            "no ESC byte may reach the model: {:?}",
            result.text
        );
    }

    #[tokio::test]
    async fn shell_exec_reuses_the_owners_shell_across_calls() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("reuse".into());
        let (id1, first) =
            run_in_shell_with(&reg, owner.clone(), "cd /tmp", 5, spawn_in_process_host)
                .await
                .expect("the first call runs");
        assert_eq!(first.exit_code, 0, "cd /tmp is clean");
        let (id2, second) = run_in_shell_with(&reg, owner.clone(), "pwd", 5, spawn_in_process_host)
            .await
            .expect("the second call runs");
        assert_eq!(
            id1.as_str(),
            id2.as_str(),
            "the same owner keeps one shell, so action persists: cd then pwd"
        );
        assert!(
            second.text.contains("/tmp"),
            "the working directory survived the first call: {:?}",
            second.text
        );
    }

    #[test]
    fn shell_exec_pipeline_is_annotate_then_mask_then_normalize() {
        assert_eq!(
            format_for_model("AKIAIOSFODNN7EXAMPLE\r\n\x1b[31mred line\x1b[0m\r\n"),
            "<AWS_KEY>\n[ERROR: red line]",
            "annotate strips ESC bytes before masking; normalize folds CRLF"
        );
    }

    #[test]
    fn shell_exec_pipeline_leaves_a_clean_command_as_its_output() {
        assert_eq!(format_for_model("hello\r\n"), "hello");
        assert_eq!(format_for_model("\r\n\r\nhello\r\n\r\n"), "hello");
    }
}
