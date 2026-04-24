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
/// 4. No siblings (chat pane fills the whole window) → offer to split or pick
///    from other windows in the session.
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
        0 => {
            // 4. No siblings — offer split or cross-window pick.
            offer_no_sibling_options(chat_pane, session)
        }
        1 => {
            let target = siblings.into_iter().next().unwrap();
            crate::pane_prefs::save(session, &target);
            Some(target)
        }
        _ => pick_sibling_pane(chat_pane, siblings, session),
    }
}

/// Read one line from stdin synchronously, temporarily clearing O_NONBLOCK so
/// the call blocks even when AsyncStdin has already set the non-blocking flag.
fn sync_read_line() -> String {
    use std::io::BufRead;
    let fd = libc::STDIN_FILENO;
    // Save and clear O_NONBLOCK so the synchronous read blocks.
    let saved = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if saved >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, saved & !libc::O_NONBLOCK) };
    }
    let mut line = String::new();
    let _ = std::io::BufReader::new(std::io::stdin()).read_line(&mut line);
    // Restore original flags (O_NONBLOCK) so AsyncStdin continues to work.
    if saved >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, saved) };
    }
    line
}

/// When the chat pane is alone in its window, offer three options:
/// split side-by-side (default), pick from another window, or proceed with
/// background-only mode.
fn offer_no_sibling_options(chat_pane: &str, session: &str) -> Option<String> {
    use std::io::Write;

    let other_panes: Vec<String> = crate::tmux::list_pane_ids_in_session(session)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p != chat_pane)
        .collect();

    println!();
    println!("No sibling pane in this window for foreground commands.");
    println!(
        "  [S]  Split this window (side by side) and use the new pane  \x1b[2m← default\x1b[0m"
    );
    if !other_panes.is_empty() {
        println!(
            "  [P]  Pick from another pane in this session ({} available)",
            other_panes.len()
        );
    }
    println!("  [N]  No foreground target (background commands only)");
    let opts = if other_panes.is_empty() {
        "S/N"
    } else {
        "S/P/N"
    };
    print!("Choose [{}] (Enter = S): ", opts);
    let _ = std::io::stdout().flush();

    let input = sync_read_line();
    let choice = input.trim().to_ascii_lowercase();

    match choice.as_str() {
        "" | "s" => {
            let out = std::process::Command::new("tmux")
                .args([
                    "split-window",
                    "-h",
                    "-t",
                    chat_pane,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ])
                .output()
                .ok()?;
            let new_pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if new_pane.is_empty() || !out.status.success() {
                eprintln!("Failed to split window.");
                return None;
            }
            println!("Using pane {} for foreground commands.", new_pane);
            crate::pane_prefs::save(session, &new_pane);
            Some(new_pane)
        }
        "p" if !other_panes.is_empty() => pick_sibling_pane(chat_pane, other_panes, session),
        _ => {
            println!("No foreground target set. Only background commands will run.");
            None
        }
    }
}

/// Present a numbered list of candidate panes and let the user choose one.
fn pick_sibling_pane(_chat_pane: &str, candidates: Vec<String>, session: &str) -> Option<String> {
    use std::io::Write;

    println!();
    println!("Multiple panes available. Which should I use for foreground commands?");
    for (i, pane_id) in candidates.iter().enumerate() {
        let info = std::process::Command::new("tmux")
            .args([
                "display-message",
                "-t",
                pane_id,
                "-p",
                "#{pane_current_command}  #{pane_current_path}",
            ])
            .output()
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
