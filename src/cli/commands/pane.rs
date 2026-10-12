//! Pane discovery and selection for foreground tool-call targets.
//!
//! Called from the chat entry flow to pick the tmux pane where shell commands
//! should land when the user approves them. Tries persisted preference first,
//! falls back to exactly-one-sibling auto-pick, then interactive selection.

/// Determine the target pane for foreground commands.
///
/// Resolution order:
/// 1. Persisted preference from a previous session (validated that it still exists).
/// 2. Exactly one sibling in the same window → use it automatically.
/// 3. Multiple siblings → prompt the user to pick one.
/// 4. No siblings (chat pane fills the whole window) → no foreground target.
pub(super) fn resolve_target_pane(chat_pane: &str, session: &str) -> Option<String> {
    // 1. Check persisted preference.
    if let Some(saved) = crate::pane_prefs::get(session)
        && saved != chat_pane
        && crate::tmux::pane_exists(&saved)
    {
        return Some(saved);
    }

    // 2 & 3. Siblings in the same tmux window.
    let window_id = crate::tmux::pane_window_id(chat_pane).unwrap_or_default();
    let siblings: Vec<String> = if !window_id.is_empty() {
        crate::tmux::list_panes_in_window(&window_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p != chat_pane)
            .collect()
    } else {
        vec![]
    };

    match siblings.len() {
        0 => None,
        1 => {
            // INVARIANT: match arm guarantees siblings.len() == 1
            let target = siblings.into_iter().next().unwrap();
            crate::pane_prefs::save(session, &target);
            Some(target)
        }
        _ => pick_sibling_pane(chat_pane, siblings, session),
    }
}

/// Read one line from stdin, clearing O_NONBLOCK around the blocking read
/// and restoring it after.
fn sync_read_line() -> String {
    use std::io::BufRead;
    use std::os::fd::AsFd;
    use std::os::fd::AsRawFd;
    let stdin = std::io::stdin();
    let fd = stdin.as_fd();
    let raw = fd.as_raw_fd();
    let saved = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if saved >= 0 {
        unsafe { libc::fcntl(raw, libc::F_SETFL, saved & !libc::O_NONBLOCK) };
    }
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    // Restore original flags (O_NONBLOCK) so AsyncStdin continues to work.
    if saved >= 0 {
        unsafe { libc::fcntl(raw, libc::F_SETFL, saved) };
    }
    line
}

/// Present a numbered list of candidate panes and let the user choose one.
fn pick_sibling_pane(_chat_pane: &str, candidates: Vec<String>, session: &str) -> Option<String> {
    use std::io::Write;

    println!();
    println!("Multiple panes available. Which should I use for foreground commands?");
    for (i, pane_id) in candidates.iter().enumerate() {
        let info = crate::process::bounded_output(std::process::Command::new("tmux").args([
            "display-message",
            "-t",
            pane_id,
            "-p",
            "#{pane_current_command}  #{pane_current_path}",
        ]))
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
        println!("  [{}]  {}  {}", i + 1, pane_id, info);
    }
    println!("  [N]  No foreground target");
    print!("Choose [1-{}/N]: ", candidates.len());
    let _ = std::io::stdout().flush();

    let input = sync_read_line();
    let input = input.trim().to_ascii_lowercase();

    if input == "n" {
        println!("No foreground target set.");
        return None;
    }
    if let Ok(n) = input.parse::<usize>()
        && n >= 1
        && n <= candidates.len()
    {
        let chosen = candidates[n - 1].clone();
        crate::pane_prefs::save(session, &chosen);
        return Some(chosen);
    }
    println!("Invalid choice. No foreground target set.");
    None
}
