//! `daemoneye reload` — ask the running daemon to re-read `config.toml`.

use std::process;

use crate::ipc::{Request, Response};

use super::ipc_client::request_once;

/// Render the reload report: two labelled lists, or `nothing changed`.
/// Pure, so the wording is unit-testable.
pub fn format_reload(applied: &[String], restart_required: &[String]) -> String {
    if applied.is_empty() && restart_required.is_empty() {
        return "nothing changed".to_string();
    }
    let mut lines = Vec::new();
    if applied.is_empty() {
        lines.push("applied: (none)".to_string());
    } else {
        lines.push(format!(
            "applied ({} key{}):",
            applied.len(),
            if applied.len() == 1 { "" } else { "s" }
        ));
        lines.extend(applied.iter().map(|k| format!("  {k}")));
    }
    if restart_required.is_empty() {
        lines.push("waiting for a restart: (none)".to_string());
    } else {
        lines.push(format!(
            "waiting for a restart ({} key{}):",
            restart_required.len(),
            if restart_required.len() == 1 { "" } else { "s" }
        ));
        lines.extend(restart_required.iter().map(|k| format!("  {k}")));
    }
    lines.join("\n")
}

pub async fn run_reload() -> anyhow::Result<()> {
    match request_once(Request::Reload).await {
        Ok(Response::Reloaded {
            applied,
            restart_required,
        }) => {
            println!("{}", format_reload(&applied, &restart_required));
        }
        Ok(Response::Error(e)) => {
            eprintln!("reload refused: {e}");
            process::exit(1);
        }
        Ok(other) => {
            eprintln!("unexpected reply from daemon ({})", other.kind());
            process::exit(1);
        }
        Err(e) => {
            eprintln!("daemoneye reload: {e:#}");
            process::exit(1);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_reload;

    #[test]
    fn no_change_prints_nothing_changed() {
        assert_eq!(format_reload(&[], &[]), "nothing changed");
    }

    #[test]
    fn applied_and_restart_keys_render_as_two_labelled_lists() {
        let out = format_reload(
            &["limits.total_tool_calls_per_turn".to_string()],
            &["sandbox.enabled".to_string()],
        );
        assert!(out.contains("applied (1 key):"), "output: {out}");
        assert!(
            out.contains("  limits.total_tool_calls_per_turn"),
            "output: {out}"
        );
        assert!(
            out.contains("waiting for a restart (1 key):"),
            "output: {out}"
        );
        assert!(out.contains("  sandbox.enabled"), "output: {out}");
    }

    #[test]
    fn only_restart_keys_still_show_the_applied_header() {
        let out = format_reload(&[], &["daemon.tmux_session".to_string()]);
        assert!(out.contains("applied: (none)"), "output: {out}");
        assert!(out.contains("daemon.tmux_session"), "output: {out}");
    }
}
