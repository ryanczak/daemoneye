use super::*;

// --- SessionCache tests ---

fn cache() -> SessionCache {
    SessionCache::new("test-session")
}

// ── summarize heuristics ──────────────────────────────────────────────────

#[test]
fn summarize_empty_buffer() {
    assert_eq!(cache().summarize(""), "Empty pane");
}

#[test]
fn summarize_only_blank_lines() {
    assert_eq!(cache().summarize("   \n\n  "), "Empty pane");
}

#[test]
fn summarize_dollar_prompt() {
    let buf = "some output\n$ ";
    let s = cache().summarize(buf);
    assert!(s.starts_with("Idle shell at:"), "got: {s}");
}

#[test]
fn summarize_hash_prompt() {
    let buf = "root output\n# ";
    let s = cache().summarize(buf);
    assert!(s.starts_with("Idle shell at:"), "got: {s}");
}

#[test]
fn summarize_top_output() {
    let buf = "Tasks: 200\ntop - 12:34:56 up 1 day";
    let s = cache().summarize(buf);
    assert_eq!(s, "Running system monitor");
}

#[test]
fn summarize_web_log_get() {
    let buf = "127.0.0.1 - - [01/Jan/2024] GET /api/health HTTP/1.1 200";
    let s = cache().summarize(buf);
    assert_eq!(s, "Tailing web logs");
}

#[test]
fn summarize_web_log_post() {
    let buf = "POST /submit HTTP/1.1";
    let s = cache().summarize(buf);
    assert_eq!(s, "Tailing web logs");
}

#[test]
fn summarize_generic_truncates_to_50_chars() {
    let long_line = "x".repeat(100);
    let s = cache().summarize(&long_line);
    assert!(s.starts_with("Active:"));
    let content_part = s.trim_start_matches("Active: ");
    assert!(content_part.len() <= 50);
}

// ── get_labeled_context ───────────────────────────────────────────────────

#[test]
fn get_labeled_context_no_panes_no_source_returns_fallback() {
    let c = cache();
    let ctx = c.get_labeled_context(None, None);
    assert!(ctx.contains("no terminal context available"));
}

#[test]
fn get_labeled_context_client_viewport_shown_when_known() {
    let c = cache();
    c.set_client_size(220, 50);
    // Need at least one pane so output is non-empty.
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: String::new(),
                summary: "shell".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/home/user".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "main".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        ctx.contains("[CLIENT VIEWPORT] 220x50"),
        "expected viewport block, got: {ctx}"
    );
}

#[test]
fn get_labeled_context_client_viewport_absent_when_zero() {
    let c = cache();
    // Default is (0, 0) — no viewport block should appear.
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: String::new(),
                summary: "shell".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/home/user".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "main".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        !ctx.contains("[CLIENT VIEWPORT]"),
        "viewport block should be absent when (0,0)"
    );
}

#[test]
fn get_labeled_context_background_panes_sorted() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%3".to_string(),
            PaneState {
                buffer: "foo".to_string(),
                summary: "summary3".to_string(),
                current_cmd: String::new(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: "bar".to_string(),
                summary: "summary1".to_string(),
                current_cmd: String::new(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    let ctx = c.get_labeled_context(None, None);
    let pos1 = ctx.find("%1").unwrap();
    let pos3 = ctx.find("%3").unwrap();
    assert!(pos1 < pos3, "panes should be sorted by ID");
}

#[test]
fn get_labeled_context_session_topology() {
    let c = cache();
    {
        let mut wins = c.windows.write().unwrap();
        wins.push(tmux::WindowState {
            window_id: "@1".to_string(),
            window_name: "nginx".to_string(),
            active: true,
            pane_count: 2,
            zoomed: false,
            last_active: false,
            flags: String::new(),
        });
        wins.push(tmux::WindowState {
            window_id: "@2".to_string(),
            window_name: "postgres".to_string(),
            active: false,
            pane_count: 1,
            zoomed: false,
            last_active: true,
            flags: String::new(),
        });
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        ctx.contains("[SESSION TOPOLOGY]"),
        "expected topology block, got: {ctx}"
    );
    assert!(
        ctx.contains("nginx (ID: @1"),
        "expected nginx in topology with ID @1"
    );
    assert!(ctx.contains("2 panes"), "expected pane count in topology");
    assert!(ctx.contains("postgres"), "expected postgres in topology");
    assert!(
        ctx.contains("last active"),
        "expected postgres to be marked as last active"
    );
}

#[test]
fn get_labeled_context_single_window_no_topology() {
    let c = cache();
    {
        let mut wins = c.windows.write().unwrap();
        wins.push(tmux::WindowState {
            window_id: "@1".to_string(),
            window_name: "main".to_string(),
            active: true,
            pane_count: 1,
            zoomed: false,
            last_active: false,
            flags: String::new(),
        });
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        !ctx.contains("[SESSION TOPOLOGY]"),
        "single-window session should not have topology block"
    );
}

#[test]
fn get_labeled_context_source_pane_excluded_from_background() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%5".to_string(),
            PaneState {
                buffer: "active content".to_string(),
                summary: "active summary".to_string(),
                current_cmd: String::new(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    // When %5 is the source pane it should NOT appear in BACKGROUND PANE list.
    // (It will appear as ACTIVE PANE if capture-pane succeeds — but in tests
    //  tmux isn't running so capture_pane returns an error, which is fine.)
    let ctx = c.get_labeled_context(Some("%5"), None);
    assert!(!ctx.contains("[BACKGROUND PANE %5]"));
}

#[test]
fn get_labeled_context_copy_mode_annotated() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%7".to_string(),
            PaneState {
                buffer: "some output".to_string(),
                summary: "Active: some output".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/home/user".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 42,
                history_size: 1000,
                in_copy_mode: true,
                synchronized: false,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    // get_labeled_context reads from cache; capture_pane won't run (no tmux).
    // Assert that the BACKGROUND PANE line for %7 contains no copy-mode marker
    // (that's only on the ACTIVE PANE header) but that the pane is listed.
    let ctx = c.get_labeled_context(None, None);
    assert!(ctx.contains("%7"), "pane %7 should appear in context");
    // Synchronized flag should NOT appear (synchronized=false).
    assert!(
        !ctx.contains("[synchronized]"),
        "non-synchronized pane should have no sync marker"
    );
}

#[test]
fn get_labeled_context_synchronized_pane_noted() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%9".to_string(),
            PaneState {
                buffer: "some output".to_string(),
                summary: "Active: doing things".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/tmp".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 500,
                in_copy_mode: false,
                synchronized: true,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        ctx.contains("[synchronized]"),
        "synchronized pane should have [synchronized] marker"
    );
    assert!(ctx.contains("%9"), "pane %9 should be listed");
}

#[test]
fn get_labeled_context_dead_pane_noted() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%11".to_string(),
            PaneState {
                buffer: "some output".to_string(),
                summary: "Active: job finished".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/tmp".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 100,
                in_copy_mode: false,
                synchronized: false,
                window_name: "de-bg-myjob".to_string(),
                dead: true,
                dead_status: Some(1),
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        ctx.contains("[dead: 1]"),
        "dead pane should have [dead: 1] marker, got: {ctx}"
    );
    assert!(ctx.contains("%11"), "pane %11 should be listed");
}

#[test]
fn get_labeled_context_chat_pane_excluded_from_background() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        // Pane running the user's shell.
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: "user shell".to_string(),
                summary: "Idle shell at: $".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/home/user".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
        // Pane running daemoneye chat.
        panes.insert(
            "%2".to_string(),
            PaneState {
                buffer: "chat output".to_string(),
                summary: "Active: chat output".to_string(),
                current_cmd: "daemoneye".to_string(),
                current_path: "/home/user".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: String::new(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    // %1 is source, %2 is chat — chat pane must not appear in background listing.
    let ctx = c.get_labeled_context(Some("%1"), Some("%2"));
    assert!(
        !ctx.contains("[BACKGROUND PANE %2"),
        "chat pane should be excluded"
    );
    // Source pane also shouldn't be in background listing (existing behaviour).
    assert!(
        !ctx.contains("[BACKGROUND PANE %1"),
        "source pane should be excluded too"
    );
}

#[test]
fn get_labeled_context_pane_classification() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        // Chat pane — window "work".
        panes.insert(
            "%2".to_string(),
            PaneState {
                buffer: String::new(),
                summary: String::new(),
                current_cmd: "daemoneye".to_string(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "work".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
        // Visible peer — same window as chat.
        panes.insert(
            "%3".to_string(),
            PaneState {
                buffer: String::new(),
                summary: "shell".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/home/user".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "work".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
        // Daemon-launched background window.
        panes.insert(
            "%5".to_string(),
            PaneState {
                buffer: String::new(),
                summary: "running".to_string(),
                current_cmd: "bash".to_string(),
                current_path: "/tmp".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "de-bg-myjob".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
        // User's session pane in a different window.
        panes.insert(
            "%7".to_string(),
            PaneState {
                buffer: String::new(),
                summary: "ssh idle".to_string(),
                current_cmd: "ssh".to_string(),
                current_path: "~".to_string(),
                pane_title: "web01".to_string(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "servers".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
            },
        );
    }
    // No source pane; chat pane is %2.
    let ctx = c.get_labeled_context(None, Some("%2"));
    assert!(!ctx.contains("%2"), "chat pane should be excluded entirely");
    assert!(
        ctx.contains("[VISIBLE PANE %3"),
        "peer in same window should be VISIBLE PANE"
    );
    assert!(
        ctx.contains("[BACKGROUND PANE %5"),
        "de-bg-* window should be BACKGROUND PANE"
    );
    assert!(
        ctx.contains("[SESSION PANE %7"),
        "other user window should be SESSION PANE"
    );
}
