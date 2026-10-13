mod ops;
mod read;
mod write;

pub(super) use read::run_read_file;
pub(super) use write::{EditArgs, run_edit_file};

use super::shell_exec::SHELL_READY_TIMEOUT;
use crate::shell::{Owner, ShellId, ShellRegistry, run_command, shell_for_owner, with_shells};
use std::time::Duration;

/// Hex-encode a string (no external crate required).
fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

/// Shell-escape a single-quoted argument by replacing `'` with `'\''`.
/// Note: a single-quoted argument may legitimately span lines, so newlines
/// do NOT break out of the quotes — `'` was the only character needing
/// escaping in POSIX sh.
fn sq_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// True when `s` contains any control character or DEL (U+0000..U+001F,
/// U+007F..U+009F). Used by path/pattern guards so hostile file names or
/// grep patterns cannot reach shell strings or downstream tools with
/// unexpected bytes (M2 defense-in-depth).
fn contains_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Well-known credential locations that read_file/edit_file never disclose,
/// even though they pass every other check (M4).   Private-key material under
/// `~/.ssh`, `.env`-style environment files, and shell/netrc/pgp credential
/// files.  This is not a security boundary — the AI can still reach these via
/// `run_terminal_command` — it removes the silent easy channel so a confused,
/// malicious, or prompt-injected model can't casually dump keys into session
/// output.
fn is_blocked_secret_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    // $HOME/.ssh (and everything beneath it).
    if let Ok(home) = std::env::var("HOME") {
        let home_ssh = std::path::Path::new(&home).join(".ssh");
        if p.starts_with(&home_ssh) {
            return true;
        }
    }
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let base = name.strip_suffix(".pub").unwrap_or(name);
    if matches!(
        base,
        ".env" | ".envrc" | ".netrc" | ".pgpass" | ".npmrc" | ".pypirc"
    ) || name.starts_with(".env.")
    {
        return true;
    }
    matches!(
        base,
        "id_rsa"
            | "id_ecdsa"
            | "id_ed25519"
            | "id_dsa"
            | "id_xmss"
            | "id_ecdsa_sk"
            | "id_ed25519_sk"
    )
}

/// Resolve a path for security-guard checks, following symlinks even when the
/// leaf does not yet exist.  Canonicalizes the full path if it exists; otherwise
/// canonicalizes the parent directory and rejoins the final component.  Falls
/// back to the lexical path if neither can be resolved.
fn resolve_path_for_guard(path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    if let Some(parent) = p.parent()
        && let Some(file_name) = p.file_name()
        && let Ok(cp) = std::fs::canonicalize(parent)
    {
        return cp.join(file_name);
    }
    std::path::PathBuf::from(path)
}

/// Run `cmd` in a shell on `host` and return the shell's **real** captured
/// output — the PTY-substrate transport for a file op, with no tmux pane and
/// no polling.
///
/// The shell comes from [`shell_for_owner`], so an **idle** shell `owner`
/// already holds on `host` is reused and an `interactive` or `dedicated` one
/// is never taken (a shell an interactive command owns eats every byte; a
/// dedicated one exists for exactly one command). The command is run through
/// [`run_command`], which returns the bytes between this command's nonce
/// markers and its real exit code — what `extract_marked` then cuts.
///
/// The read's own command still runs through the plain [`run_command`], so
/// nothing mid-command ever prompts. The **connection** to a cold host is a
/// different story: the executor arm calls `ensure_connected` before this
/// operation, and that may open a credential round trip — by design, 08e
/// reversed the old rule. This function itself remains prompt-free.
pub(super) async fn run_in_host_shell<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    host: crate::host::HostId,
    cmd: &str,
    timeout_secs: u64,
    spawn: F,
) -> anyhow::Result<String>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    if let crate::host::HostId::Named(name) = &host {
        crate::shell::host_entry_or_bare(name)?;
    }
    let max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    let (id, socket) = shell_for_owner(
        reg,
        owner,
        host.clone(),
        max_per_owner,
        SHELL_READY_TIMEOUT,
        spawn,
    )
    .await?;
    let marker_name = with_shells(reg, |store| {
        store
            .get(id.as_str())
            .map(|record| crate::shell::marker_shell_name(&record.host))
    })
    .unwrap_or_else(|| crate::shell::marker_shell_name(&host));
    // `in_flight` spans the wrapped command: set before it is written, and
    // cleared by the guard's Drop on every way the run ends.
    let _in_flight =
        crate::shell::InFlightGuard::new(reg, id.clone(), cmd, crate::shell::now_unix());
    let outcome = run_command(
        &socket,
        cmd,
        &marker_name,
        Duration::from_secs(timeout_secs),
    )
    .await?;
    Ok(String::from_utf8_lossy(&outcome.output).into_owned())
}
