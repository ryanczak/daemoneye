mod ops;
mod read;
mod write;

pub(super) use read::run_read_file;
pub(super) use write::{EditArgs, run_edit_file};

use crate::tmux;
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

/// Send a command to a pane and poll until a completion marker appears.
async fn remote_run_and_capture(
    pane_id: &str,
    cmd: &str,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let p = pane_id.to_string();
    let c = cmd.to_string();
    tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &c))
        .await
        .ok_or_else(|| anyhow::anyhow!("timed out sending keys to pane {pane_id}"))??;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("Timed out waiting for remote command in pane {}", pane_id);
        }
        let p = pane_id.to_string();
        let snap = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&p, 600))
            .await
            .and_then(|r| r.ok())
            .unwrap_or_default();
        if snap.contains("__DE_DONE__") {
            return Ok(snap);
        }
    }
}
