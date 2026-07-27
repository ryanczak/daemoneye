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
fn sq_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
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
